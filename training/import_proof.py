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
    parser.add_argument('--max-positions', type=int, default=1000)
    args = parser.parse_args()
    shards = base_shards(args.base_dataset) if args.base_dataset else []
    args.output.mkdir(parents=True, exist_ok=True)
    shard = args.output / 'proof.rmd'
    teacher = {'book_sha256': file_hash(args.book), 'executable_sha256': file_hash(args.engine),
               'source': 'independently-verified-book', 'policy': 'proven-attacker-action-only'}
    save_split_manifest(args.output / 'import.json', {'version': 2, 'teacher': teacher,
        'base_shards': shards, 'max_positions': args.max_positions})
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
    descriptor = describe_shard(shard, teacher, teacher['book_sha256'])
    for game_id, game in descriptor['games'].items():
        sample = provenance['games'][game_id]
        game['lineage_id'] = sample['lineage_id']
        game['parent_lineage_id'] = sample['lineage_id']
        game['opening_family'] = sample['lineage_id']
        game['proof'] = sample
    shards.append(descriptor)
    save_split_manifest(args.output / 'dataset.json', {'version': 1, 'shards': shards})
    print(f'saved={args.output / "dataset.json"}')


if __name__ == '__main__':
    main()
