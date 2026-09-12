"""Bounded synthetic legal corpus scaling comparison against the starting HEAD implementation."""
import argparse
import ctypes
import json
import os
import random
import subprocess
import sys
import tempfile
import time
import types
from pathlib import Path


def rss_peak():
    if os.name != 'nt':
        import resource
        value = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
        return value if sys.platform == 'darwin' else value * 1024
    class Counters(ctypes.Structure):
        _fields_ = [('cb', ctypes.c_ulong), ('faults', ctypes.c_ulong)] + [(name, ctypes.c_size_t) for name in
            ('peak_working', 'working', 'peak_paged', 'paged', 'peak_nonpaged', 'nonpaged', 'pagefile', 'peak_pagefile')]
    counters = Counters(); counters.cb = ctypes.sizeof(counters)
    kernel = ctypes.windll.kernel32
    kernel.GetCurrentProcess.restype = ctypes.c_void_p
    get_info = ctypes.windll.psapi.GetProcessMemoryInfo
    get_info.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulong]
    if not get_info(kernel.GetCurrentProcess(), ctypes.byref(counters), counters.cb):
        raise OSError('GetProcessMemoryInfo failed')
    return counters.peak_working


def synthetic(path, games):
    from common import DATA_HEADER, DATA_MAGIC, DATA_RECORD_PREFIX, transform_index
    with path.open('wb') as stream:
        stream.write(DATA_HEADER.pack(DATA_MAGIC, 1, 0, games * 8))
        for game in range(games):
            moves = random.Random(game).sample(range(225), 8)
            for ply in range(8):
                candidates = []
                for symmetry in range(8):
                    key = bytearray(58)
                    for index, at in enumerate(moves[:ply]):
                        cell = transform_index(at, symmetry)
                        key[cell//4] |= (1 + index % 2) << ((3-cell%4)*2)
                    key[-1] = ply % 2
                    candidates.append(bytes(key))
                # At most four stones per side: all prefixes are legally nonterminal.
                key = min(candidates)
                stream.write(DATA_RECORD_PREFIX.pack(game, ply, 0, 255, game % 1000, 2, 0))
                stream.write(key)


def measure(mode, games):
    import dataset
    from common import dataset_fingerprint, make_split_manifest
    if mode == 'baseline':
        source = subprocess.check_output(['git', 'show', '31b71a57251304778de45d495b2b99609dd26683:training/dataset.py'], text=True)
        module = types.ModuleType('baseline_dataset')
        exec(compile(source, 'baseline_dataset.py', 'exec'), module.__dict__)
    else:
        module = dataset
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        raw = root / 'data.rmd'
        synthetic(raw, games)
        before = rss_peak()
        start = time.perf_counter()
        shard = module.describe_shard(raw, {'source': 'synthetic-legal-scaling-only'}, 'scaling',
                                      **({'compact': True} if mode == 'compact' else {}))
        descriptor = root / 'dataset.json'
        descriptor.write_text(json.dumps({'version': 2 if mode == 'compact' else 1, 'shards': [shard]}, indent=2))
        described = time.perf_counter()-start
        start = time.perf_counter()
        with module.DatasetBundle(descriptor) as data:
            opened = time.perf_counter()-start
            start = time.perf_counter(); dataset_fingerprint(data); hashed = time.perf_counter()-start
            start = time.perf_counter(); split = make_split_manifest(data, 7); split_time = time.perf_counter()-start
            split_bytes = len(json.dumps(split, indent=2).encode())
            records = len(data)
            peak = rss_peak()
        return {'mode': mode, 'records': records, 'synthetic_games': games, 'peak_rss_bytes': peak,
                'post_runtime_import_peak_bytes': before, 'descriptor_bytes': descriptor.stat().st_size,
                'split_bytes': split_bytes, 'bytes_per_record': (raw.stat().st_size + descriptor.stat().st_size + split_bytes)/records,
                'describe_seconds': described, 'open_seconds': opened, 'fingerprint_seconds': hashed,
                'split_seconds': split_time, 'warning': 'synthetic legal prefixes measure scale only, not useful training sample count'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mode', choices=('baseline', 'compact'), required=True)
    parser.add_argument('--games', type=int, required=True)
    args = parser.parse_args()
    if not 1 <= args.games <= 4096:
        raise ValueError('scaling sample cap is 4096 games')
    print(json.dumps(measure(args.mode, args.games)))
