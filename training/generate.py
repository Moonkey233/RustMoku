"""Bounded, resumable shard generation around the existing Rust teacher CLI."""

import argparse
import hashlib
import json
import subprocess
import time
from pathlib import Path

from audit import audit
from common import make_split_manifest, save_split_manifest
from dataset import DatasetBundle, describe_shard, file_hash, publish_shard
from manifest import read_manifest


def generate(args):
    if not 1 <= args.games <= 10_000 or not 1 <= args.shard_games <= 32:
        raise ValueError('games must be 1..10000 and shard-games 1..32')
    if not 1 <= args.workers <= 8 or args.nodes < 1 or not 1 <= args.depth <= 255:
        raise ValueError('invalid explicit teacher budget')
    args.output.mkdir(parents=True, exist_ok=True)
    teacher = {'executable_sha256': file_hash(args.engine), 'evaluator': 'pattern',
               'depth': args.depth, 'work_per_move': args.nodes,
               'workers': args.workers, 'threads_per_teacher': 1,
               'tt_mib': 64, 'tt_policy': 'cold-per-game', 'random_prefix_plies': args.random_plies,
               'seed': args.seed, 'games': args.games, 'shard_games': args.shard_games}
    run_id = hashlib.sha256(json.dumps(teacher, sort_keys=True).encode()).hexdigest()
    save_split_manifest(args.output / 'run.json', {'version': 1, 'run_id': run_id, 'teacher': teacher})
    shards = []
    start = time.perf_counter()
    for shard_index, first in enumerate(range(0, args.games, args.shard_games)):
        count = min(args.shard_games, args.games - first)
        seed = int.from_bytes(hashlib.sha256(f'{args.seed}:{shard_index}'.encode()).digest()[:8], 'little')
        path = args.output / f'shard-{shard_index:05d}.rmd'
        descriptor_path = path.with_suffix('.json')
        if descriptor_path.exists():
            descriptor = read_manifest(descriptor_path)
            if descriptor['run_id'] != run_id or descriptor['sha256'] != file_hash(path):
                raise ValueError('completed shard identity mismatch')
        else:
            temporary = path.with_suffix('.partial')
            # No shell interpolation; the executable is frozen in run.json.
            if file_hash(args.engine) != teacher['executable_sha256']:
                raise ValueError('teacher executable changed during generation')
            subprocess.run([str(args.engine.resolve()), 'selfplay', '--games', str(count),
                            '--seed', str(seed), '--workers', str(args.workers),
                            '--depth', str(args.depth), '--nodes', str(args.nodes),
                            '--random-plies', str(args.random_plies), '--cold-games', 'true',
                            '--output', str(temporary.resolve())], check=True, timeout=args.timeout)
            descriptor = describe_shard(temporary, teacher, run_id)
            if len(descriptor['games']) != count:
                raise ValueError('incomplete generated shard')
            publish_shard(temporary, path)
            temporary.unlink()
            descriptor['path'] = str(path.resolve())
            save_split_manifest(descriptor_path, descriptor)
        shards.append(descriptor)
    bundle = args.output / 'dataset.json'
    save_split_manifest(bundle, {'version': 1, 'shards': shards})
    with DatasetBundle(bundle) as dataset:
        manifest = make_split_manifest(dataset, args.seed)
        save_split_manifest(args.output / 'split.json', manifest)
        report = audit(dataset, manifest)
        report['elapsed_seconds'] = time.perf_counter() - start
        report['unique_qualified_positions_per_second'] = report['unique_qualified_positions'] / report['elapsed_seconds']
        print(json.dumps(report, indent=2))
    return bundle


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--games', type=int, required=True)
    parser.add_argument('--seed', type=int, required=True)
    parser.add_argument('--workers', type=int, default=1)
    parser.add_argument('--shard-games', type=int, default=8)
    parser.add_argument('--depth', type=int, default=4)
    parser.add_argument('--nodes', type=int, default=5000)
    parser.add_argument('--random-plies', type=int, default=2)
    parser.add_argument('--timeout', type=float, default=60, help='maximum seconds per shard')
    generate(parser.parse_args())


if __name__ == '__main__':
    main()
