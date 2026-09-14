"""Bounded, resumable shard generation around the existing Rust teacher CLI."""

import argparse
import hashlib
import json
import subprocess
import time
import shutil
from pathlib import Path

from audit import audit
from common import make_split_manifest, save_split_manifest
from dataset import DatasetBundle, describe_shard, file_hash, publish_shard
from manifest import read_manifest


def generate(args):
    if args.games < 1 or not 1 <= args.shard_games <= 32:
        raise ValueError('games must be positive and shard-games 1..32')
    if not 1 <= args.workers <= 4 or args.nodes < 1 or not 1 <= args.depth <= 255:
        raise ValueError('invalid explicit teacher budget')
    args.output.mkdir(parents=True, exist_ok=True)
    estimated_bytes = args.games * 226 * 95 + args.games * 2048
    if shutil.disk_usage(args.output).free < estimated_bytes * 2:
        raise ValueError('generation exceeds artifact or available-disk budget')
    teacher = {'executable_sha256': file_hash(args.engine), 'evaluator': 'learned' if getattr(args, 'model', None) else 'pattern',
               'model_sha256': file_hash(args.model) if getattr(args, 'model', None) else None,
               'profile': getattr(args, 'profile', None), 'depth': args.depth, 'work_per_move': args.nodes,
               'workers': args.workers, 'threads_per_teacher': 1,
               'tt_mib': 64, 'tt_policy': 'cold-per-game', 'random_prefix_plies': args.random_plies,
               'explore_top_k': getattr(args, 'explore_top_k', 0),
               'explore_temperature': getattr(args, 'explore_temperature', 1000.0),
               'explore_plies': getattr(args, 'explore_plies', 80),
               'comparison_domain': 'distillation', 'root_universe': 'all-legal',
               'descendant_universe': 'production-radius-two',
               'analysis_work_per_move': args.nodes if getattr(args, 'explore_top_k', 0) else 0,
               'seed': args.seed, 'games': args.games, 'shard_games': args.shard_games}
    opening_db=getattr(args,'opening_db',None)
    if opening_db:
        teacher['opening_starts']={'source':'empirical-database-positions-only','sha256':file_hash(opening_db),'min_plies':2,'max_plies':16,'selection':'splitmix64-game-id-canonical-v1'}
    elif getattr(args,'opening_starts',None)=='builtin':
        teacher['opening_starts']={'source':'builtin-suite','selection':'splitmix64-game-id-v1'}
    run_id = hashlib.sha256(json.dumps(teacher, sort_keys=True).encode()).hexdigest()
    save_split_manifest(args.output / 'run.json', {'version': 1, 'run_id': run_id, 'teacher': teacher})
    shards = []
    comparison_paths = []
    start = time.perf_counter()
    for shard_index, first in enumerate(range(0, args.games, args.shard_games)):
        count = min(args.shard_games, args.games - first)
        seed = args.seed
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
            model_args = ['--profile', args.profile] if getattr(args, 'profile', None) else []
            model_args += ['--explore-top-k', str(teacher['explore_top_k']), '--explore-temperature', str(teacher['explore_temperature']),
                           '--explore-plies', str(teacher['explore_plies'])]
            if getattr(args, 'model', None):
                if file_hash(args.model) != teacher['model_sha256']:
                    raise ValueError('teacher model changed during generation')
                model_args += ['--model', str(args.model.resolve())]
            if opening_db:
                if file_hash(opening_db)!=teacher['opening_starts']['sha256']:raise ValueError('opening database changed during generation')
                model_args += ['--opening-db',str(Path(opening_db).resolve())]
            subprocess.run([str(args.engine.resolve()), 'selfplay', '--games', str(count),
                            '--seed', str(seed), '--first-game', str(first), '--workers', str(args.workers),
                            '--depth', str(args.depth), '--nodes', str(args.nodes),
                            '--random-plies', str(args.random_plies), '--cold-games', 'true',
                            '--output', str(temporary.resolve())] + model_args, check=True, timeout=args.timeout)
            descriptor = describe_shard(temporary, teacher, run_id, compact=True)
            if teacher['explore_top_k']:
                companion = temporary.with_suffix('.policy.jsonl')
                descriptor['comparisons_raw'] = {'path': str(companion.resolve()), 'sha256': file_hash(companion)}
            if len(descriptor['games']) != count:
                raise ValueError('incomplete generated shard')
            publish_shard(temporary, path)
            temporary.unlink()
            descriptor['path'] = str(path.resolve())
            save_split_manifest(descriptor_path, descriptor)
        if teacher['explore_top_k']:
            companion = descriptor['comparisons_raw']
            if file_hash(companion['path']) != companion['sha256']:
                raise ValueError('teacher comparison companion changed')
            comparison_paths.append(Path(companion['path']))
        shards.append(descriptor)
    bundle = args.output / 'dataset.json'
    description = {'version': 2, 'shards': shards}
    if comparison_paths:
        from teacher import build_sidecar
        description['comparisons'] = build_sidecar(comparison_paths, args.output / 'comparisons.sqlite', teacher['explore_temperature'])
    save_split_manifest(bundle, description)
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
    parser.add_argument('--explore-top-k', type=int, default=0)
    parser.add_argument('--explore-temperature', type=float, default=1000.0)
    parser.add_argument('--explore-plies', type=int, default=80)
    parser.add_argument('--profile', help='versioned RMPROFILE1 value')
    parser.add_argument('--model', type=Path, help='frozen V1/V2 teacher model; default Pattern')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--games', type=int, required=True)
    parser.add_argument('--seed', type=int, required=True)
    parser.add_argument('--workers', type=int, default=1)
    parser.add_argument('--shard-games', type=int, default=8)
    parser.add_argument('--depth', type=int, default=4)
    parser.add_argument('--nodes', type=int, default=5000)
    parser.add_argument('--random-plies', type=int, default=2)
    parser.add_argument('--opening-db',type=Path,help='empirical canonical 2..16-ply starts; no book scores imported')
    parser.add_argument('--opening-starts',choices=['builtin'])
    parser.add_argument('--timeout', type=float, default=60, help='maximum seconds per shard')
    generate(parser.parse_args())


if __name__ == '__main__':
    main()
