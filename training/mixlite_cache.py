"""Bounded immutable mmap cache of canonical MixLite V3 inputs (not labels)."""
import hashlib
import json
import os
import tempfile
import time
from pathlib import Path

import numpy as np
from common import DIRECTIONS, OFFSETS, transform_index
from dataset import file_hash
from manifest import save_manifest, read_manifest

SCHEMA = 'mixlite-v3-d4-reversal-min-relative-lines-v1'
ROW = np.dtype([('keys','<u2',(225,4)),('centers','u1',(225,))])
# Out-of-board sentinel is a separate cell with code 3.
NEIGHBORS = np.array([[[((r+dr*k)*15+c+dc*k if 0<=r+dr*k<15 and 0<=c+dc*k<15 else 225)
                       for k in OFFSETS] for dr,dc in DIRECTIONS] for r in range(15) for c in range(15)])
CELL_INVERSE = np.argsort([[transform_index(i,s) for i in range(225)] for s in range(8)],axis=1)
DIRECTION_INVERSE = np.empty((8,4),dtype=np.int64)
for s in range(8):
    for d,(dr,dc) in enumerate(DIRECTIONS):
        at=transform_index(112+dr*15+dc,s)
        vector=(at//15-7,at%15-7)
        out=next(i for i,v in enumerate(DIRECTIONS) if vector==v or vector==(-v[0],-v[1]))
        DIRECTION_INVERSE[s,out]=d


def canonical_inputs(position_keys):
    """Vectorized exact integer construction; slow mixlite.features stays independent."""
    if any(len(key)!=58 for key in position_keys):raise ValueError('invalid feature-cache position length')
    packed=np.frombuffer(b''.join(position_keys),dtype=np.uint8).reshape(-1,58)
    cells=np.arange(225)
    boards=(packed[:,cells//4] >> (2*(3-cells%4))) & 3
    side=packed[:,-1:]
    if np.any(boards==3) or np.any(side>1) or np.any(packed[:,56]&63):
        raise ValueError('invalid feature-cache position')
    relative=np.where(boards==0,0,np.where(boards==side+1,1,2)).astype(np.uint8)
    padded=np.concatenate([relative,np.full((len(packed),1),3,dtype=np.uint8)],axis=1)
    codes=padded[:,NEIGHBORS].astype(np.uint32)
    shifts=2*np.arange(8,dtype=np.uint32)
    forward=(codes << shifts).sum(-1,dtype=np.uint32)
    reverse=(codes << shifts[::-1]).sum(-1,dtype=np.uint32)
    return np.minimum(forward,reverse).astype(np.uint16),relative


def transformed_inputs(keys, centers, symmetries):
    """Inverse spatial/direction permutation; line reversal is already canonical.

    D4 maps each direction to +/- another direction. min(key, reversed-key)
    removes the sign; permuting columns as well gives exact reference tensors,
    not merely an embedding-sum equivalence that could change float reductions.
    """
    symmetry=np.asarray(symmetries,dtype=np.int64)
    cells=CELL_INVERSE[symmetry]
    rows=np.arange(len(symmetry))[:,None,None]
    return (keys[rows,cells[:,:,None],DIRECTION_INVERSE[symmetry][:,None,:]].astype(np.int64),
            centers[np.arange(len(symmetry))[:,None],cells].astype(np.int64))


class FeatureCache:
    def __init__(self,directory,dataset,manifest,indices,max_bytes=4*1024**3):
        self.directory=Path(directory);self.rows=None
        count=len(indices);size=count*ROW.itemsize
        if size>max_bytes:raise ValueError(f'feature cache needs {size} bytes; limit is {max_bytes}')
        digest=hashlib.sha256()
        for start in range(0,count,4096):
            digest.update(np.asarray([indices[i] for i in range(start,min(start+4096,count))],dtype='<u8').tobytes())
        identity=dict(schema=SCHEMA,split_sha256=hashlib.sha256(json.dumps(manifest,sort_keys=True).encode()).hexdigest(),
                      dataset_sha256=manifest['dataset_sha256'],partition='train',indices_sha256=digest.hexdigest(),
                      count=count,bytes=size,row_bytes=ROW.itemsize)
        started=time.perf_counter();built=False
        if not self.directory.exists():
            print(json.dumps(dict(feature_cache='building',records=count,estimated_bytes=size)),flush=True)
            self.directory.parent.mkdir(parents=True,exist_ok=True)
            with tempfile.TemporaryDirectory(dir=self.directory.parent) as temporary:
                staging=Path(temporary)/'cache';staging.mkdir()
                mapped=np.memmap(staging/'features.bin',dtype=ROW,mode='w+',shape=(count,))
                try:
                    for start in range(0,count,256):
                        stop=min(start+256,count)
                        positions=[dataset[indices[i]].position_key for i in range(start,stop)]
                        keys,centers=canonical_inputs(positions)
                        mapped['keys'][start:stop]=keys;mapped['centers'][start:stop]=centers
                    mapped.flush()
                finally:mapped._mmap.close()
                with (staging/'features.bin').open('rb+') as stream:os.fsync(stream.fileno())
                save_manifest(staging/'manifest.json',dict(identity=identity,sha256=file_hash(staging/'features.bin')))
                try:os.rename(staging,self.directory);built=True
                except FileExistsError:pass  # A concurrent publisher must still match below.
        saved=read_manifest(self.directory/'manifest.json')
        payload=self.directory/'features.bin'
        if saved['identity']!=identity or payload.stat().st_size!=size or file_hash(payload)!=saved['sha256']:
            raise ValueError('feature cache identity/content mismatch; use a fresh cache directory')
        self.rows=np.memmap(payload,dtype=ROW,mode='r',shape=(count,))
        self.report=dict(cache_bytes=size,cache_records=count,built=built,cache_seconds=time.perf_counter()-started)

    def batch(self,locals,symmetries):
        rows=self.rows[np.asarray(locals,dtype=np.int64)]
        return transformed_inputs(rows['keys'],rows['centers'],symmetries)

    def close(self):
        if self.rows is not None:self.rows._mmap.close();self.rows=None
