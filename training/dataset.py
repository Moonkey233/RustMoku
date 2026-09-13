"""Versioned, content-addressed provenance for raw Rust dataset shards.

The JSON descriptor is the training input. Raw legacy shards can still be
audited, but cannot invent missing lineage or historical teacher metadata.
"""

from __future__ import annotations

import argparse
import bisect
import dataclasses
import hashlib
import json
import os
import sqlite3
from collections import defaultdict, OrderedDict
from pathlib import Path
from typing import Sequence

from common import DataRecord, DatasetFile, save_split_manifest
from manifest import read_manifest


def file_hash(path: Path | str) -> str:
    path = Path(path)
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def publish_shard(source: Path, destination: Path) -> None:
    """Publish a completed same-filesystem shard/companion without clobbering it."""
    if destination.exists() or destination.is_symlink():
        if destination.is_symlink() or file_hash(source) != file_hash(destination):
            raise ValueError(f'immutable shard output changed: {destination}')
        return
    with source.open('rb+') as stream:
        os.fsync(stream.fileno())
    try:
        os.link(source, destination)
    except FileExistsError:
        if destination.is_symlink() or file_hash(source) != file_hash(destination):
            raise ValueError(f'immutable shard output changed: {destination}')


def game_ranges(data):
    """One bounded game at a time; compact formats require physical ordering."""
    seen = set()
    start = 0
    records = []
    for index, record in enumerate(data):
        if records and record.game_id != records[0].game_id:
            yield start, records
            seen.add(records[0].game_id)
            start, records = index, []
        if record.game_id in seen:
            raise ValueError('interleaved game records require offline migration')
        if record.ply > 225 or (records and record.ply <= records[-1].ply):
            raise ValueError('invalid or non-increasing game_id/ply')
        records.append(record)
    if records:
        yield start, records


def trajectory_content(records):
    return [[r.ply, r.position_key.hex()] for r in records]


def describe_shard(path: Path, teacher: dict, run_id: str, *, compact=False) -> dict:
    descriptions = {}
    with DatasetFile(path) as data:
        for start, records in game_ranges(data):
            content = trajectory_content(records)
            identity = hashlib.sha256(json.dumps(content, separators=(',', ':')).encode()).hexdigest()
            descriptions[str(records[0].game_id)] = {
                'trajectory_id': identity,
                'lineage_id': identity,
                'parent_lineage_id': None,
                'opening_family': records[0].position_key.hex(),
                **({'start': start} if compact else {'identity_content': content}),
                'records': len(records),
                'outcome': game_outcome(records),
            }
    return {'path': str(path.resolve()), 'sha256': file_hash(path),
            'run_id': run_id, 'teacher': teacher, 'games': descriptions,
            **({'index_version': 2} if compact else {})}


def game_outcome(records):
    final = records[-1]
    if final.source != 3 or not final.exact:
        return {"status": "truncated-or-unknown", "winner": None}
    winner = None if final.value == 0 else (final.position_key[-1] if final.value > 0 else 1 - final.position_key[-1])
    return {"status": "terminal", "winner": winner}


class ShardView(Sequence):
    def __init__(self, pool, index): self.pool, self.index = pool, index
    def __len__(self): return len(self.pool.reader(self.index))
    def __getitem__(self, item): return self.pool.reader(self.index)[item]


class ShardReaders:
    """Bounded mmap/file handles; immutable records survive reader eviction."""
    def __init__(self, capacity=32):
        self.capacity = capacity
        self.paths = []
        self.views = []
        self.readers = OrderedDict()
    def __len__(self): return len(self.paths)
    def append(self, path):
        self.paths.append(Path(path))
        self.views.append(ShardView(self,len(self.paths)-1))
        return self.views[-1]
    def __getitem__(self, index): return self.views[index]
    def reader(self, index):
        if index in self.readers:
            self.readers.move_to_end(index)
            return self.readers[index]
        if len(self.readers) >= self.capacity:
            _, old = self.readers.popitem(last=False); old.close()
        reader = DatasetFile(self.paths[index]); self.readers[index] = reader
        return reader
    def close(self):
        for reader in self.readers.values(): reader.close()
        self.readers.clear()


class DatasetBundle(Sequence[DataRecord]):
    """Shard IDs are namespaced by content, not concatenated local game IDs."""

    def __init__(self, path: Path, resolver=None):
        if path.stat().st_size > 64 * 1024 * 1024:
            raise ValueError('dataset descriptor exceeds 64 MiB')
        self.descriptor = read_manifest(path)
        if self.descriptor.get('version') not in (1, 2):
            raise ValueError('unsupported dataset descriptor')
        self.comparisons = None
        companion = self.descriptor.get('comparisons')
        if companion is not None:
            companion_path = Path((resolver or {}).get(companion['path'], companion['path']))
            if file_hash(companion_path) != companion['sha256']:
                raise ValueError('comparison sidecar identity mismatch')
            self.comparisons = sqlite3.connect(companion_path.resolve().as_uri() + '?mode=ro&immutable=1', uri=True)
            self.comparisons.execute('PRAGMA cache_size=-4096')
        self.shards = ShardReaders()
        self.ranges = []
        self.ends = []
        self.count = 0
        seen_files = set()
        identities = {}
        game_index = 0
        try:
            for shard in self.descriptor['shards']:
                source = Path(shard['path'])
                if not source.is_absolute():
                    source = path.parent / source
                source = Path((resolver or {}).get(shard['path'], (resolver or {}).get(str(source.resolve()), source)))
                digest = file_hash(source)
                if digest != shard['sha256']:
                    raise ValueError('shard fingerprint mismatch')
                if digest in seen_files:
                    raise ValueError('duplicate shard')
                seen_files.add(digest)
                data = self.shards.append(source)
                compact = shard.get('index_version', 1) == 2
                if shard.get('index_version', 1) not in (1, 2):
                    raise ValueError('unsupported shard index schema')
                covered = set()
                for start, records in game_ranges(data):
                    local_id = records[0].game_id
                    covered.add(str(local_id))
                    if str(local_id) not in shard['games']:
                        raise ValueError('descriptor game coverage mismatch')
                    meta = shard['games'][str(local_id)]
                    content = trajectory_content(records)
                    if (type(meta['records']) is not int or len(records) != meta['records']
                            or (compact and (type(meta.get('start')) is not int or meta['start'] != start))
                            or (not compact and content != meta['identity_content'])):
                        raise ValueError('trajectory content mismatch')
                    if meta.get('outcome', game_outcome(records)) != game_outcome(records):
                        raise ValueError('trajectory outcome mismatch')
                    identity = hashlib.sha256(json.dumps(content, separators=(',', ':')).encode()).hexdigest()
                    if identity != meta['trajectory_id']:
                        raise ValueError('trajectory fingerprint mismatch')
                    if identity in identities:
                        old_shard, old_start, old_count = identities[identity]
                        previous = self.shards[old_shard]
                        if old_count != len(records) or any(
                                (previous[old_start + j].ply, previous[old_start + j].position_key)
                                != (r.ply, r.position_key) for j, r in enumerate(records)):
                            raise ValueError('trajectory digest collision')
                    else:
                        identities[identity] = (len(self.shards) - 1, start, len(records))
                    lineage = meta['lineage_id']
                    if not isinstance(lineage, str) or not lineage:
                        raise ValueError('missing lineage identity')
                    if meta['parent_lineage_id'] not in (None, lineage):
                        raise ValueError('branch must retain its original parent lineage')
                    self.ranges.append((len(self.shards) - 1, start, game_index, meta, shard['run_id']))
                    self.count += len(records)
                    if self.count > 10_000_000:
                        raise ValueError('bundle exceeds 10000000 record limit')
                    self.ends.append(self.count)
                    game_index += 1
                if covered != set(shard['games']):
                    raise ValueError('descriptor game coverage mismatch')
        except Exception:
            self.close()
            raise

    def __len__(self):
        return self.count

    def __getitem__(self, index):
        if isinstance(index, slice):
            return [self[i] for i in range(*index.indices(len(self)))]
        if index < 0:
            index += len(self)
        if not 0 <= index < len(self):
            raise IndexError(index)
        group = bisect.bisect_right(self.ends, index)
        shard, start, game, meta, run = self.ranges[group]
        local = start + index - (self.ends[group - 1] if group else 0)
        record = self.shards[shard][local]
        outcome = meta.get('outcome', {})
        value = None
        if outcome.get('status') == 'terminal':
            value = 0 if outcome['winner'] is None else (1 if outcome['winner'] == record.position_key[-1] else -1)
        comparison = None
        if self.comparisons is not None:
            row = self.comparisons.execute('SELECT payload FROM comparisons WHERE position=?', (record.position_key.hex(),)).fetchone()
            if row is not None:
                from teacher import validate_comparison
                comparison = validate_comparison(json.loads(row[0]))
        return dataclasses.replace(record, game_id=game, outcome=value, comparison=comparison,
                                   run_id=run, trajectory_id=meta['trajectory_id'],
                                   lineage_id=meta['lineage_id'], opening_family=meta['opening_family'])

    def close(self):
        if self.comparisons is not None:
            self.comparisons.close()
            self.comparisons = None
        self.shards.close()

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        self.close()


def open_dataset(path: Path, resolver=None):
    return DatasetBundle(path, resolver=resolver) if path.suffix == '.json' else DatasetFile(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--shard', type=Path, action='append', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    # Importing existing data cannot establish which executable created it.
    # The generator wrapper supplies actual teacher/config identity instead.
    shards = [describe_shard(path, {'status': 'unknown-legacy'}, file_hash(path), compact=True) for path in args.shard]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    save_split_manifest(args.output, {'version': 2, 'shards': shards})


if __name__ == '__main__':
    main()
