"""Export only independently verified strategy positions and bind their lineage."""

import argparse
import json
import subprocess
import tempfile
from pathlib import Path

from common import save_split_manifest
from dataset import DatasetBundle, describe_shard, file_hash, publish_shard


def base_shards(path):
    """Resolve each reference in its source descriptor's directory before copying."""
    path = Path(path).resolve()
    with DatasetBundle(path) as base:
        return [{**shard, 'path': str((path.parent / shard['path']).resolve())}
                for shard in base.descriptor['shards']]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--book', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--base-dataset', type=Path)
    parser.add_argument('--neighbors', type=int, default=0, help='at most 16 legal history-prefix branches')
    parser.add_argument('--neighbor-depth', type=int, default=2)
    parser.add_argument('--neighbor-nodes', type=int, default=1000)
    parser.add_argument('--max-positions', type=int, default=1000)
    args = parser.parse_args()
    if not 0 <= args.neighbors <= 16:
        raise ValueError('neighbors must be 0..16')
    shards = base_shards(args.base_dataset) if args.base_dataset else []
    args.output.mkdir(parents=True, exist_ok=True)
    shard = args.output / 'proof.rmd'
    teacher = {'book_sha256': file_hash(args.book), 'executable_sha256': file_hash(args.engine),
               'source': 'independently-verified-book', 'policy': 'proven-attacker-action-only'}
    save_split_manifest(args.output / 'import.json', {'version': 2, 'teacher': teacher,
        'base_shards': shards, 'max_positions': args.max_positions, 'neighbors': args.neighbors,
        'neighbor_depth': args.neighbor_depth, 'neighbor_nodes': args.neighbor_nodes})
    with tempfile.TemporaryDirectory(dir=args.output, prefix='.proof-') as directory:
        staged = Path(directory) / 'proof.rmd'
        subprocess.run([str(args.engine.resolve()), 'proof', '--book', str(args.book.resolve()),
                        '--output', str(staged.resolve()), '--max-positions', str(args.max_positions)],
                       check=True, timeout=60)
        describe_shard(staged, teacher, teacher['book_sha256'])
        json.loads(staged.with_suffix('.proof.json').read_text(encoding='utf-8'))
        publish_shard(staged.with_suffix('.proof.json'), shard.with_suffix('.proof.json'))
        publish_shard(staged, shard)
    provenance = json.loads(shard.with_suffix('.proof.json').read_text(encoding='utf-8'))
    descriptor = describe_shard(shard, teacher, teacher['book_sha256'], compact=True)
    for game_id, game in descriptor['games'].items():
        sample = provenance['games'][game_id]
        game['lineage_id'] = sample['lineage_id']
        game['parent_lineage_id'] = sample['lineage_id']
        game['opening_family'] = sample['lineage_id']
        game['proof'] = sample
    shards.append(descriptor)
    neighbors = 0
    for sample in provenance['games'].values():
        moves = sample['moves']
        if not moves or neighbors >= args.neighbors:
            continue
        if file_hash(args.engine) != teacher['executable_sha256'] or file_hash(args.book) != teacher['book_sha256']:
            raise ValueError('proof inputs changed during neighborhood labeling')
        from common import format_move
        with tempfile.TemporaryDirectory(dir=args.output) as directory:
            record = Path(directory) / 'source.rmg'
            record.write_text('RustMoku 1\nrules=freestyle\nmoves=' + ' '.join(format_move(at) for at in moves) + '\n')
            staged = Path(directory) / 'neighbor.rmd'
            subprocess.run([str(args.engine.resolve()), 'record', '--record', str(record.resolve()),
                            '--branch-ply', str(len(moves)-1), '--branch-choice', str(neighbors),
                            '--depth', str(args.neighbor_depth), '--nodes', str(args.neighbor_nodes),
                            '--output', str(staged.resolve())], check=True, timeout=30)
            neighbor = args.output / f'neighbor-{neighbors:02d}.rmd'
            publish_shard(staged, neighbor)
        branch = describe_shard(neighbor, {**teacher, 'source': 'legal-prefix-neighborhood-pattern-relabel',
                                          'depth': args.neighbor_depth, 'nodes': args.neighbor_nodes}, teacher['book_sha256'], compact=True)
        for game in branch['games'].values():
            game.update(lineage_id=sample['lineage_id'], parent_lineage_id=sample['lineage_id'], opening_family=sample['lineage_id'])
        shards.append(branch)
        neighbors += 1
    bundle = {'version': 2, 'shards': shards}
    if args.base_dataset:
        original = json.loads(args.base_dataset.read_text())
        if 'comparisons' in original:
            bundle['comparisons'] = original['comparisons']
    save_split_manifest(args.output / 'dataset.json', bundle)
    print(f'saved={args.output / "dataset.json"}')


if __name__ == '__main__':
    main()
