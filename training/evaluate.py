"""Evaluate a float checkpoint on its frozen, game-isolated canonical split."""
from __future__ import annotations

import argparse
import json
import math
from collections import defaultdict
from pathlib import Path

import torch

from checkpoint import load_checkpoint
from common import decode_position_key, load_training_model, validate_split_manifest
from dataset import open_dataset
from train import make_example


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dataset', type=Path, required=True)
    parser.add_argument('--checkpoint', type=Path, required=True)
    parser.add_argument('--split', choices=('train', 'validation', 'test', 'opening_heldout'), default='validation')
    parser.add_argument('--seed', type=int, default=None)
    parser.add_argument('--device', default='cpu')
    return parser.parse_args()


def _group():
    return dict(samples=0, value_absolute_error=0.0, saturated=0, policy_samples=0,
                top1=0, top3=0, top5=0, target_pairwise_agreement=0.0,
                outcome_samples=0, outcome_brier_linear_clamp=0.0,
                teacher_wdl_samples=0, teacher_wdl_brier=0.0, wdl_sum=[0.0]*3,
                wdl_samples=0, wdl_brier=0.0, wdl_accuracy=0)


def _wdl_target(value):
    return [max(value, 0), 1-abs(value), max(-value, 0)]


def _predict(model, record, device, architecture, score_scale):
    if architecture == 'v3':
        from mixlite import features
        from nonlinear_model import normalized_target
        board, side = decode_position_key(record.position_key)
        keys, centers = features(board, side)
        wdl, policy, _ = model(keys.to(device), centers.to(device))
        # V3 has a contextual whole-board policy, not a candidate-local head.
        moves = [at for at, stone in enumerate(board) if stone == 0]
        if record.policy_move is not None and record.policy_move not in moves:
            raise ValueError('policy target is not a legal empty cell')
        target = moves.index(record.policy_move) if record.policy_move is not None else -1
        return (float((wdl[0]-wdl[2]).item()), normalized_target(record, score_scale),
                policy[moves], target, wdl.tolist())
    keys, candidates, target, value, _, _ = make_example(record, 0, device, architecture, score_scale)
    return (float(model.value(keys.unsqueeze(0)).item()), value,
            model.policy(candidates) if target >= 0 else None, target, None)


def evaluate_dataset(model, checkpoint, dataset, split='validation', device='cpu', seed=None):
    """Unweighted diagnostics only; does not alter data, models or train targets."""
    formats = {'rustmoku-local-pattern-v1': 'v1', 'rustmoku-nonlinear-v2': 'v2',
               'rustmoku-mixlite-v3-d4': 'v3'}
    architecture = formats[checkpoint['format']]
    score_scale = (checkpoint['score_scale'] if architecture == 'v3'
                   else checkpoint.get('configuration', {}).get('score_scale'))
    partitions = validate_split_manifest(dataset, checkpoint.get('split_manifest'), seed)
    indices = partitions.get(split, [])
    if not indices:
        raise ValueError(f'split {split!r} is empty or unavailable')
    strata = defaultdict(_group)
    model.eval()
    with torch.no_grad():
        for index in indices:
            record = dataset[index]
            prediction, value, logits, target, wdl = _predict(model, record, device, architecture, score_scale)
            if not math.isfinite(prediction):
                raise ValueError('nonfinite value prediction')
            phase = 'opening' if record.ply < 20 else ('middle' if record.ply < 80 else 'late')
            groups = [strata['all'], strata[f'source-{record.source}'], strata[phase]]
            for group in groups:
                group['samples'] += 1
                group['value_absolute_error'] += abs(prediction-value)
                group['saturated'] += int(abs(prediction) >= 1)
                if record.outcome is not None:
                    expected_score = (max(-1, min(1, prediction))+1)/2
                    group['outcome_samples'] += 1
                    group['outcome_brier_linear_clamp'] += (expected_score-(record.outcome+1)/2)**2
                if wdl is not None:
                    teacher = _wdl_target(value)
                    group['teacher_wdl_samples'] += 1
                    group['teacher_wdl_brier'] += sum((p-t)**2 for p,t in zip(wdl, teacher, strict=True))
                    group['wdl_sum'] = [a+p for a,p in zip(group['wdl_sum'], wdl, strict=True)]
                    if record.outcome is not None:
                        outcome = _wdl_target(record.outcome)
                        group['wdl_samples'] += 1
                        group['wdl_brier'] += sum((p-t)**2 for p,t in zip(wdl, outcome, strict=True))
                        group['wdl_accuracy'] += int(max(range(3), key=wdl.__getitem__) == outcome.index(1))
            if target >= 0:
                ranking = torch.argsort(logits, descending=True, stable=True).tolist()
                rank = ranking.index(target)
                for group in groups:
                    group['policy_samples'] += 1
                    for k in (1, 3, 5):
                        group[f'top{k}'] += int(rank < k)
                    group['target_pairwise_agreement'] += (len(ranking)-1-rank)/max(1, len(ranking)-1)
    for group in strata.values():
        group['value_mae'] = group.pop('value_absolute_error')/group['samples']
        group['saturation_rate'] = group.pop('saturated')/group['samples']
        for metric in ('top1', 'top3', 'top5', 'target_pairwise_agreement'):
            group[metric] = group[metric]/group['policy_samples'] if group['policy_samples'] else None
        for metric, count in [('outcome_brier_linear_clamp','outcome_samples'),
                              ('teacher_wdl_brier','teacher_wdl_samples'),
                              ('wdl_brier','wdl_samples'), ('wdl_accuracy','wdl_samples')]:
            group[metric] = group[metric]/group[count] if group[count] else None
        wdl_sum = group.pop('wdl_sum')
        group['mean_wdl'] = ([p/group['teacher_wdl_samples'] for p in wdl_sum]
                             if group['teacher_wdl_samples'] else None)
    return {'version': 2, 'split': split, 'architecture': architecture,
            'wdl_order': ['win', 'draw', 'loss'], 'perspective': 'side-to-move',
            'wdl_brier_target': 'record.outcome only; sum of three squared errors (range 0..2)',
            'teacher_wdl_target': '[max(q,0),1-abs(q),max(-q,0)]; existing training target, not observed outcome',
            'policy_universe': 'all-legal-empty-cells; stable move-index ties', 'strata': dict(strata)}


def main() -> None:
    args = parse_args()
    checkpoint = load_checkpoint(args.checkpoint)
    model = load_training_model(args.checkpoint, args.device)
    with open_dataset(args.dataset) as dataset:
        report = evaluate_dataset(model, checkpoint, dataset, args.split, args.device, args.seed)
    summary = report['strata']['all']
    policy = ' '.join(f'policy_top{k}={summary[f"top{k}"]:.4f}' for k in (1,3,5)) if summary['policy_samples'] else 'policy_top1=unavailable policy_top3=unavailable policy_top5=unavailable'
    print(f'split={args.split} samples={summary["samples"]} value_mae_normalized={summary["value_mae"]:.6f} {policy} policy_samples={summary["policy_samples"]}')
    print(json.dumps(report, indent=2, allow_nan=False))


if __name__ == '__main__':
    main()
