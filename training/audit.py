"""Audit dataset provenance, duplicate trajectories, quality and split overlap."""

import argparse
import itertools
import json
from collections import Counter, defaultdict
from pathlib import Path

from common import eligible_label, make_split_manifest, save_split_manifest
from dataset import open_dataset


def audit(dataset, manifest, prefix_plies=8):
    if manifest['version'] == 2:
        from compact import audit as compact_audit
        return compact_audit(dataset, manifest, prefix_plies)
    games = defaultdict(list)
    for record in dataset:
        games[record.game_id].append(record)
    trajectories = Counter(tuple((r.ply, r.position_key, r.policy_move) for r in records)
                           for records in games.values())
    splits = manifest['indices']
    overlaps = {}
    for a, b in itertools.combinations(splits, 2):
        all_a = {dataset[i].position_key for i in splits[a]}
        all_b = {dataset[i].position_key for i in splits[b]}
        later_a = {dataset[i].position_key for i in splits[a] if dataset[i].ply > prefix_plies}
        later_b = {dataset[i].position_key for i in splits[b] if dataset[i].ply > prefix_plies}
        overlaps[f'{a}/{b}'] = {'canonical_positions': len(all_a & all_b),
                               'after_prefix': len(later_a & later_b)}
    sources = Counter(record.source for record in dataset)
    return {
        'records': len(dataset), 'games': len(games),
        'unique_complete_position_policy_trajectories': len(trajectories),
        'duplicate_complete_trajectories': sum(count - 1 for count in trajectories.values()),
        'qualified_records': sum(eligible_label(r) for r in dataset),
        'unique_qualified_positions': len({r.position_key for r in dataset if eligible_label(r)}),
        'unknown_quality_records': sum(r.completed_depth is None for r in dataset),
        'sources': dict(sorted(sources.items())),
        'short_prefix_max_ply': prefix_plies, 'overlaps': overlaps,
        'split_records': {name: len(indices) for name, indices in splits.items()},
        'qualified_split_records': {name: sum(eligible_label(dataset[i]) for i in indices)
                                    for name, indices in splits.items()},
        'opening_family_heldout_available': bool(splits.get('opening_heldout')),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dataset', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--seed', type=int, default=7)
    parser.add_argument('--prefix-plies', type=int, default=8)
    args = parser.parse_args()
    with open_dataset(args.dataset) as dataset:
        manifest = make_split_manifest(dataset, args.seed)
        save_split_manifest(args.manifest, manifest)
        print(json.dumps(audit(dataset, manifest, args.prefix_plies), indent=2))


if __name__ == '__main__':
    main()
