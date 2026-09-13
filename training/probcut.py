"""Bounded quiet-node score-pair collection and conservative empirical ProbCut fit.

Each input record contributes one position and must declare its source lineage
and train/heldout role. Heldout never determines fit parameters or tail margin.
"""
import argparse
import json
import math
import statistics
from pathlib import Path
from budget import ExplorationBudget
from dataset import file_hash
from manifest import read_manifest, save_manifest
from common import truncating_division

PATTERN_ID = b'RustMoku-Pattern-weights-v1.....'.hex()


def collect(args):
    manifest = read_manifest(args.records)
    items = manifest['records']
    if not 1 <= len(items) <= 4096 or not 3 <= args.deep <= 32 or not 1 <= args.shallow <= args.deep - 2:
        raise ValueError('invalid bounded collection size or depth pair')
    lineages = set()
    for item in items:
        if not item['lineage'] or item['lineage'] in lineages or item['split'] not in ('train', 'heldout'):
            raise ValueError('each collection position must have an independent declared lineage')
        lineages.add(item['lineage'])
    identity = {'engine_sha256': file_hash(args.engine), 'model_sha256': file_hash(args.model) if args.model else PATTERN_ID,
                'profile': args.profile, 'selectivity': args.selectivity, 'records_sha256': file_hash(args.records),
                'record_files': [file_hash(Path(item['path'])) for item in items], 'deep': args.deep, 'shallow': args.shallow}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    budget = ExplorationBudget(args.budget)
    rows, seen = [], set()
    for item in items:
        scores = []
        for depth in (args.shallow, args.deep):
            if file_hash(args.engine) != identity['engine_sha256'] or (args.model and file_hash(args.model) != identity['model_sha256']):
                raise ValueError('frozen analysis engine/model changed')
            command = [str(args.engine.resolve()), 'analyze', '--score-only', 'true', '--record', str(Path(item['path']).resolve()),
                       '--depth', str(depth), '--nodes', str(args.nodes), '--profile', args.profile]
            if args.model:
                command += ['--model', str(args.model.resolve())]
            result = budget.run(command, timeout=args.timeout, artifact_root=args.output.parent,
                                check=True, capture_output=True, text=True)
            scores.append(json.loads(result.stdout))
        shallow, deep = scores
        if shallow['position_key'] != deep['position_key']:
            raise ValueError('analysis position changed between depth samples')
        key = deep['position_key']
        if key in seen:
            raise ValueError('duplicate canonical position in calibration corpus')
        seen.add(key)
        qualified = all(result['quiet'] and result['completed_depth'] == depth and result['termination'] == 'Completed'
                        and type(result['score']) is int and abs(result['score']) <= 10_000_000
                        for result, depth in zip(scores, (args.shallow, args.deep)))
        if qualified and 8 <= deep['stones'] <= 190:
            phase = 0 if deep['stones'] < 20 else 1 if deep['stones'] < 80 else 2
            rows.append({'lineage': item['lineage'], 'split': item['split'], 'position': key, 'phase': phase,
                         'shallow': shallow['score'], 'deep': deep['score'], 'work': sum(row['work'] for row in scores)})
    if file_hash(args.records) != identity['records_sha256'] or [file_hash(Path(item['path'])) for item in items] != identity['record_files']:
        raise ValueError('collection inputs changed')
    save_manifest(args.output, {'version': 1, 'identity': identity, 'attempted': len(items), 'qualified': rows})


def fit_bucket(training, heldout, deep, shallow, phase):
    if len(training) < 512 or len(heldout) < 2995:
        return None, {'status': 'insufficient-independent-samples', 'training': len(training), 'heldout': len(heldout)}
    xmean = statistics.mean(row['shallow'] for row in training)
    ymean = statistics.mean(row['deep'] for row in training)
    variance = sum((row['shallow'] - xmean)**2 for row in training)
    if variance == 0:
        return None, {'status': 'degenerate-shallow-distribution'}
    slope = round(65536 * sum((row['shallow'] - xmean) * (row['deep'] - ymean) for row in training) / variance)
    if not 1 <= slope <= 4 * 65536:
        return None, {'status': 'unsupported-slope'}
    intercept = round(ymean - slope * xmean / 65536)
    predicted = lambda row: truncating_division(slope * row['shallow'], 65536) + intercept
    low, high = min(row['shallow'] for row in training), max(row['shallow'] for row in training)
    tail = max(0, max(predicted(row) - row['deep'] for row in training)) + max(1, math.ceil((high - low) * .01))
    if abs(intercept) > 10_000_000 or tail > 10_000_000:
        return None, {'status': 'unsupported-numeric-range'}
    qualified = [row for row in heldout if low <= row['shallow'] <= high]
    false = sum(predicted(row) - tail > row['deep'] for row in qualified)
    report = {'status': 'accepted' if len(qualified) >= 2995 and false == 0 and 1 - .05**(1 / len(qualified)) <= .001 else 'rejected-heldout',
              'training': len(training), 'heldout': len(qualified), 'heldout_out_of_distribution': len(heldout) - len(qualified),
              'false_lower_predictions': false,
              'zero_failure_rate_95pct_upper': 1 - .05**(1 / len(qualified)) if qualified and false == 0 else None}
    bucket = [deep, shallow, phase, slope, intercept, tail, len(training), len(qualified), false, low, high]
    return (bucket if report['status'] == 'accepted' else None), report


def fit(args):
    data = read_manifest(args.samples)
    if data.get('version') != 1:
        raise ValueError('unsupported collection version')
    identity, rows = data['identity'], data['qualified']
    if len(rows) > 4096 or len({row['lineage'] for row in rows}) != len(rows) or len({row['position'] for row in rows}) != len(rows):
        raise ValueError('duplicated calibration lineage or position')
    buckets, reports = [], []
    for phase in range(3):
        training = [row for row in rows if row['phase'] == phase and row['split'] == 'train']
        heldout = [row for row in rows if row['phase'] == phase and row['split'] == 'heldout']
        bucket, report = fit_bucket(training, heldout, identity['deep'], identity['shallow'], phase)
        reports.append({'phase': phase, **report})
        if bucket is not None:
            buckets.append(bucket)
    save_manifest(args.output.with_suffix('.json'), {'samples_sha256': file_hash(args.samples), 'reports': reports,
                                                    'status': 'experimental-default-off' if buckets else 'inconclusive-disabled'})
    if buckets:
        from export import publish_model
        content = '\n'.join(['RMPROBCUT2', identity['model_sha256'], identity['profile'], identity['selectivity']]
                            + [','.join(map(str, bucket)) for bucket in buckets]) + '\n'
        publish_model(args.output, content.encode())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest='command', required=True)
    collect_parser = subparsers.add_parser('collect')
    for flag in ('engine', 'records', 'output', 'budget'):
        collect_parser.add_argument('--' + flag, type=Path, required=True)
    collect_parser.add_argument('--model', type=Path)
    collect_parser.add_argument('--profile', required=True)
    collect_parser.add_argument('--selectivity', default='1111111')
    collect_parser.add_argument('--deep', type=int, default=3)
    collect_parser.add_argument('--shallow', type=int, default=1)
    collect_parser.add_argument('--nodes', type=int, default=50000)
    collect_parser.add_argument('--timeout', type=float, default=10)
    fit_parser = subparsers.add_parser('fit')
    fit_parser.add_argument('--samples', type=Path, required=True)
    fit_parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    collect(args) if args.command == 'collect' else fit(args)


if __name__ == '__main__':
    main()
