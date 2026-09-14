"""Versioned composition of immutable source bundles; never promotes labels.

Whole trajectories and original lineage/proof metadata remain intact. Canonical
deduplication and optional proof subsampling happen only in the training split.
"""
import argparse
import contextlib
import copy
import json
import math
import sqlite3
import tempfile
from pathlib import Path
from common import make_split_manifest, save_split_manifest
from dataset import DatasetBundle, file_hash, publish_shard
from manifest import read_manifest
from teacher import validate_comparison

KINDS=('selfplay','reanalysis','verified-proof','tactical','opening')

def compose(config_path,output):
    config_path=Path(config_path).resolve();output=Path(output).resolve()
    config=read_manifest(config_path)
    if config.get('version')!=1 or not 1<=len(config.get('sources',[]))<=1024:raise ValueError('invalid composition version/source count')
    seed=config.get('seed',1)
    if type(seed) is not int:raise ValueError('composition seed must be integer')
    output.mkdir(parents=True,exist_ok=True)
    shards=[];inputs=[];seen={};has_comparisons=False
    with tempfile.TemporaryDirectory(dir=output) as directory:
        directory=Path(directory);comparison_path=directory/'comparisons.sqlite'
        with contextlib.closing(sqlite3.connect(comparison_path)) as comparisons:
            comparisons.execute('PRAGMA cache_size=-4096')
            comparisons.execute('CREATE TABLE comparisons(position TEXT PRIMARY KEY, payload TEXT NOT NULL)')
            # Preserve proof supervision when canonical duplicates also have AB labels.
            for source in sorted(config['sources'],key=lambda item:(item['kind']!='verified-proof',item['path'])):
                kind=source['kind'];weight=source.get('weight',1.);keep=source.get('keep',1.)
                if kind not in KINDS or not all(isinstance(v,(int,float)) and math.isfinite(v) for v in (weight,keep)) or not 0<weight<=100 or not 0<keep<=1:raise ValueError('invalid source controls')
                if kind!='verified-proof' and keep!=1:raise ValueError('subsampling currently applies only to proof samples')
                path=Path(source['path']);path=path if path.is_absolute() else config_path.parent/path
                path=path.resolve();digest=file_hash(path)
                inputs.append({'path':str(path),'sha256':digest,'kind':kind,'weight':weight,'keep':keep})
                with DatasetBundle(path) as bundle:
                    if bundle.descriptor.get('composition'):raise ValueError('compose original sources, not recursively composed datasets')
                    for original in bundle.descriptor['shards']:
                        shard=copy.deepcopy(original)
                        raw=Path(shard['path']);shard['path']=str((raw if raw.is_absolute() else path.parent/raw).resolve())
                        for meta in shard['games'].values():
                            meta.update(composition_kind=kind,sample_weight=weight,sample_keep=keep)
                        if shard['sha256'] in seen:
                            if shard!=seen[shard['sha256']]:raise ValueError('conflicting repeated shard provenance/controls')
                            continue
                        seen[shard['sha256']]=shard;shards.append(shard)
                    if bundle.comparisons is not None:
                        for key,payload in bundle.comparisons.execute('SELECT position,payload FROM comparisons ORDER BY position'):
                            if len(payload)>65536 or len(bytes.fromhex(key))!=58:raise ValueError('invalid comparison row')
                            normalized=json.dumps(validate_comparison(json.loads(payload)),sort_keys=True)
                            old=comparisons.execute('SELECT payload FROM comparisons WHERE position=?',(key,)).fetchone()
                            if old and old[0]!=normalized:raise ValueError('conflicting teacher comparison; relabel explicitly')
                            comparisons.execute('INSERT OR IGNORE INTO comparisons VALUES (?,?)',(key,normalized));has_comparisons=True
                if file_hash(path)!=digest:raise ValueError('composition source changed during read')
            comparisons.commit()
        descriptor={'version':2,'shards':shards,'composition':{'version':1,'inputs':inputs,'seed':seed,
            'policy':'lineage-split-then-train-canonical-dedupe-proof-hash-subsample-v1'}}
        if has_comparisons:
            final_comparisons=output/'comparisons.sqlite';publish_shard(comparison_path,final_comparisons)
            descriptor['comparisons']={'path':str(final_comparisons),'sha256':file_hash(final_comparisons)}
        staged=directory/'dataset.json';staged.write_text(json.dumps(descriptor),encoding='utf-8')
        with DatasetBundle(staged) as bundle:split=make_split_manifest(bundle,seed)
        # Same inputs resume identically; incompatible composition cannot replace it.
        save_split_manifest(output/'composition.json',{'version':1,'config_sha256':file_hash(config_path),'inputs':inputs,'seed':seed})
        save_split_manifest(output/'dataset.json',descriptor)
        save_split_manifest(output/'split.json',split)
    return output/'dataset.json'

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config',type=Path,required=True);parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args();print(compose(args.config,args.output))

if __name__=='__main__':main()
