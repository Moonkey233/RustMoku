"""Export only independently verified strategy positions and bind their lineage."""

import argparse
import json
import subprocess
from pathlib import Path

from common import save_split_manifest
from dataset import DatasetBundle, describe_shard, file_hash


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--book', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--base-dataset', type=Path)
    parser.add_argument('--max-positions', type=int, default=1000)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    shard = args.output / 'proof.rmd'
    teacher = {'book_sha256': file_hash(args.book), 'executable_sha256': file_hash(args.engine),
               'source': 'independently-verified-book', 'policy': 'proven-attacker-action-only'}
    save_split_manifest(args.output / 'import.json', {'version': 1, 'teacher': teacher})
    subprocess.run([str(args.engine.resolve()), 'proof', '--book', str(args.book.resolve()),
                    '--output', str(shard.resolve()), '--max-positions', str(args.max_positions)],
                   check=True, timeout=60)
    provenance = json.loads(shard.with_suffix('.proof.json').read_text(encoding='utf-8'))
    descriptor = describe_shard(shard, teacher, teacher['book_sha256'])
    for game_id, game in descriptor['games'].items():
        sample = provenance['games'][game_id]
        game['lineage_id'] = sample['lineage_id']
        game['parent_lineage_id'] = sample['lineage_id']
        game['opening_family'] = sample['lineage_id']
        game['proof'] = sample
    shards = []
    if args.base_dataset:
        with DatasetBundle(args.base_dataset) as base:
            shards.extend(base.descriptor['shards'])
    shards.append(descriptor)
    save_split_manifest(args.output / 'dataset.json', {'version': 1, 'shards': shards})
    print(f'saved={args.output / "dataset.json"}')


if __name__ == '__main__':
    main()
