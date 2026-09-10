"""Versioned, content-addressed provenance for raw Rust dataset shards.

The JSON descriptor is the training input. Raw legacy shards can still be
audited, but cannot invent missing lineage or historical teacher metadata.
"""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
from collections import defaultdict
from pathlib import Path
from typing import Sequence

from common import DataRecord, DatasetFile, save_split_manifest


def file_hash(path: Path | str) -> str:
    path = Path(path)
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def describe_shard(path: Path, teacher: dict, run_id: str) -> dict:
    games = defaultdict(list)
    with DatasetFile(path) as data:
        for record in data:
            games[record.game_id].append(record)
    descriptions = {}
    for game_id, records in games.items():
        records.sort(key=lambda record: record.ply)
        content = [[r.ply, r.position_key.hex()] for r in records]
        identity = hashlib.sha256(json.dumps(content, separators=(',', ':')).encode()).hexdigest()
        descriptions[str(game_id)] = {
            'trajectory_id': identity,
            'lineage_id': identity,
            'parent_lineage_id': None,
            'opening_family': records[0].position_key.hex(),
            'identity_content': content,
            'records': len(records),
            'outcome': game_outcome(records),
        }
    return {'path': str(path.resolve()), 'sha256': file_hash(path),
            'run_id': run_id, 'teacher': teacher, 'games': descriptions}


def game_outcome(records):
    final = records[-1]
    if final.source != 3 or not final.exact:
        return {"status": "truncated-or-unknown", "winner": None}
    winner = None if final.value == 0 else (final.position_key[-1] if final.value > 0 else 1 - final.position_key[-1])
    return {"status": "terminal", "winner": winner}


class DatasetBundle(Sequence[DataRecord]):
    """Shard IDs are namespaced by content, not concatenated local game IDs."""

    def __init__(self, path: Path):
        if path.stat().st_size > 64 * 1024 * 1024:
            raise ValueError('dataset descriptor exceeds 64 MiB')
        self.descriptor = json.loads(path.read_text(encoding='utf-8'))
        if self.descriptor.get('version') != 1:
            raise ValueError('unsupported dataset descriptor')
        self.shards = []
        self.rows = []
        seen_files = set()
        identities = {}
        game_index = 0
        try:
            for shard in self.descriptor['shards']:
                source = Path(shard['path'])
                if not source.is_absolute():
                    source = path.parent / source
                digest = file_hash(source)
                if digest != shard['sha256']:
                    raise ValueError('shard fingerprint mismatch')
                if digest in seen_files:
                    raise ValueError('duplicate shard')
                seen_files.add(digest)
                data = DatasetFile(source)
                self.shards.append(data)
                groups = defaultdict(list)
                for i, record in enumerate(data):
                    groups[record.game_id].append(i)
                if set(map(str, groups)) != set(shard['games']):
                    raise ValueError('descriptor game coverage mismatch')
                for local_id, indices in groups.items():
                    meta = shard['games'][str(local_id)]
                    content = [[data[i].ply, data[i].position_key.hex()] for i in indices]
                    if content != meta['identity_content'] or len(indices) != meta['records']:
                        raise ValueError('trajectory content mismatch')
                    if meta.get('outcome', game_outcome([data[i] for i in indices])) != game_outcome([data[i] for i in indices]):
                        raise ValueError('trajectory outcome mismatch')
                    identity = hashlib.sha256(json.dumps(content, separators=(',', ':')).encode()).hexdigest()
                    if identity != meta['trajectory_id']:
                        raise ValueError('trajectory fingerprint mismatch')
                    if identity in identities and identities[identity] != content:
                        raise ValueError('trajectory digest collision')
                    identities[identity] = content
                    lineage = meta['lineage_id']
                    if not isinstance(lineage, str) or not lineage:
                        raise ValueError('missing lineage identity')
                    if meta['parent_lineage_id'] not in (None, lineage):
                        raise ValueError('branch must retain its original parent lineage')
                    for i in indices:
                        self.rows.append((len(self.shards) - 1, i, game_index, meta, shard['run_id']))
                    game_index += 1
        except Exception:
            self.close()
            raise

    def __len__(self):
        return len(self.rows)

    def __getitem__(self, index):
        if isinstance(index, slice):
            return [self[i] for i in range(*index.indices(len(self)))]
        shard, local, game, meta, run = self.rows[index]
        record = self.shards[shard][local]
        outcome = meta.get('outcome', {})
        value = None
        if outcome.get('status') == 'terminal':
            value = 0 if outcome['winner'] is None else (1 if outcome['winner'] == record.position_key[-1] else -1)
        return dataclasses.replace(record, game_id=game, outcome=value,
                                   run_id=run, trajectory_id=meta['trajectory_id'],
                                   lineage_id=meta['lineage_id'], opening_family=meta['opening_family'])

    def close(self):
        for shard in self.shards:
            shard.close()
        self.shards = []

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        self.close()


def open_dataset(path: Path):
    return DatasetBundle(path) if path.suffix == '.json' else DatasetFile(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--shard', type=Path, action='append', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    # Importing existing data cannot establish which executable created it.
    # The generator wrapper supplies actual teacher/config identity instead.
    shards = [describe_shard(path, {'status': 'unknown-legacy'}, file_hash(path)) for path in args.shard]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    save_split_manifest(args.output, {'version': 1, 'shards': shards})


if __name__ == '__main__':
    main()
