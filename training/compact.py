"""Bounded disk-backed grouping and auditing for compact dataset bundles."""

import array
import bisect
import contextlib
import hashlib
import itertools
import json
import random
import sqlite3
import tempfile
from collections import Counter
from pathlib import Path
from typing import Sequence


class RangeIndices(Sequence):
    def __init__(self, ranges):
        self.ranges = ranges
        self.ends = array.array('I')
        total = last = 0
        for start, count in ranges:
            if (type(start) is not int or type(count) is not int or start < last
                    or count <= 0 or start + count > 10_000_000):
                raise ValueError('invalid split range')
            total += count
            self.ends.append(total)
            last = start + count

    def __len__(self):
        return self.ends[-1] if self.ends else 0

    def __getitem__(self, index):
        if isinstance(index, slice):
            return [self[i] for i in range(*index.indices(len(self)))]
        if index < 0:
            index += len(self)
        if not 0 <= index < len(self):
            raise IndexError(index)
        group = bisect.bisect_right(self.ends, index)
        return self.ranges[group][0] + index - (self.ends[group - 1] if group else 0)

    def __iter__(self):
        for start, count in self.ranges:
            yield from range(start, start + count)


def append_range(ranges, index):
    if ranges and sum(ranges[-1]) == index:
        ranges[-1][1] += 1
    else:
        ranges.append([index, 1])


def partitions(manifest):
    if manifest['version'] == 1:
        return manifest['indices']
    return {name: RangeIndices(ranges) for name, ranges in manifest['ranges'].items()}


@contextlib.contextmanager
def index_database():
    with tempfile.TemporaryDirectory(prefix='rustmoku-index-') as directory:
        with contextlib.closing(sqlite3.connect(Path(directory) / 'index.sqlite')) as database:
            database.execute('PRAGMA cache_size=-8192')
            database.execute('PRAGMA temp_store=FILE')
            database.execute('PRAGMA max_page_count=262144')  # <= 1 GiB at 4096 bytes/page
            yield database


def exact_index(database, dataset):
    from common import eligible_label
    database.execute('CREATE TABLE exact (key BLOB PRIMARY KEY, sign INTEGER) WITHOUT ROWID')
    for record in dataset:
        if record.exact and eligible_label(record):
            sign = (record.value > 0) - (record.value < 0)
            old = database.execute('SELECT sign FROM exact WHERE key=?', (record.position_key,)).fetchone()
            if old is not None and old[0] != sign:
                raise ValueError('conflicting exact labels for canonical position')
            database.execute('INSERT OR IGNORE INTO exact VALUES (?,?)', (record.position_key, sign))


def make_manifest(dataset, seed):
    from common import dataset_fingerprint
    # Bundle loading already checks full trajectory equality on digest matches.
    groups = dataset.ranges
    parent = list(range(len(groups)))
    def root(i):
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i
    owners = {}
    for i, (_, _, _, meta, _) in enumerate(groups):
        for kind in ('trajectory_id', 'lineage_id'):
            key = kind, meta[kind]
            if key in owners:
                parent[root(i)] = root(owners[key])
            else:
                owners[key] = i
    families = sorted({group[3]['opening_family'] for group in groups})
    # Frozen hash-ranked empirical starting-position family, explicitly not a
    # semantic opening taxonomy. This policy is part of the format identity.
    heldout_family = max(families, key=lambda x: hashlib.sha256(x.encode()).digest()) if len(families) >= 3 else None
    heldout = {root(i) for i, group in enumerate(groups) if group[3]['opening_family'] == heldout_family}
    identities = sorted({root(i) for i in range(len(groups))} - heldout)
    random.Random(seed).shuffle(identities)
    count = len(identities)
    test = max(1, round(count * .1)) if count >= 3 else 0
    validation = max(1, round(count * .1)) if count >= 2 else 0
    membership = {key: ('train' if i < count - test - validation else
                       'validation' if i < count - test else 'test') for i, key in enumerate(identities)}
    ranges = {name: [] for name in ('train', 'validation', 'test', 'opening_heldout')}
    start = 0
    for i, group in enumerate(groups):
        name = 'opening_heldout' if root(i) in heldout else membership[root(i)]
        count = group[3]['records']
        if ranges[name] and sum(ranges[name][-1]) == start:
            ranges[name][-1][1] += count
        else:
            ranges[name].append([start, count])
        start += count
    with index_database() as database:
        exact_index(database, dataset)
    return {'version': 2, 'dataset_sha256': dataset_fingerprint(dataset), 'seed': seed,
            'grouping': 'trajectory-lineage-connected-ranges-v2',
            'quality_filter': 'exclude-analysis-fallback-v1',
            'opening_policy': 'sha256-ranked-canonical-start-v1',
            'heldout_family': heldout_family, 'ranges': ranges}


def filtered_partitions(dataset, manifest):
    from common import eligible_label
    result = {}
    with index_database() as database:
        database.execute('CREATE TABLE selected (key BLOB PRIMARY KEY) WITHOUT ROWID')
        database.execute('CREATE TABLE composition_seen (key BLOB PRIMARY KEY) WITHOUT ROWID')
        for name, indices in partitions(manifest).items():
            database.execute('DELETE FROM selected')
            for i in indices:
                record = dataset[i]
                if record.exact and eligible_label(record):
                    database.execute('INSERT OR IGNORE INTO selected VALUES (?)', (record.position_key,))
            ranges = []
            for i in indices:
                record = dataset[i]
                if name=='train' and dataset.descriptor.get('composition'):
                    # Sampling happens after lineage-safe splitting. Heldout data
                    # never influences retention or supplies training labels.
                    if record.composition_kind=='verified-proof':
                        digest=hashlib.sha256(str(manifest['seed']).encode()+record.lineage_id.encode()+record.position_key).digest()
                        if int.from_bytes(digest[:8],'big') >= int(record.sample_keep*(1<<64)):
                            continue
                if eligible_label(record) and (record.exact or not database.execute(
                        'SELECT 1 FROM selected WHERE key=?', (record.position_key,)).fetchone()):
                    if name=='train' and dataset.descriptor.get('composition'):
                        if not database.execute('INSERT OR IGNORE INTO composition_seen VALUES (?)',(record.position_key,)).rowcount:
                            continue
                    append_range(ranges, i)
            result[name] = RangeIndices(ranges)
    return result


def audit(dataset, manifest, prefix_plies=8):
    from common import eligible_label
    from dataset import game_ranges
    splits = partitions(manifest)
    sources = Counter()
    qualified = unknown = games = 0
    with index_database() as database:
        database.execute('CREATE TABLE trajectories (content BLOB PRIMARY KEY, count INTEGER) WITHOUT ROWID')
        database.execute('CREATE TABLE positions (key BLOB PRIMARY KEY) WITHOUT ROWID')
        database.execute('CREATE TABLE split (name TEXT, key BLOB, later INTEGER, PRIMARY KEY(name,key)) WITHOUT ROWID')
        for _, records in game_ranges(dataset):
            games += 1
            content = json.dumps([(r.ply, r.position_key.hex(), r.policy_move) for r in records]).encode()
            database.execute('INSERT INTO trajectories VALUES (?,1) ON CONFLICT(content) DO UPDATE SET count=count+1', (content,))
            for record in records:
                sources[record.source] += 1
                unknown += record.completed_depth is None
                qualified += eligible_label(record)
                if eligible_label(record):
                    database.execute('INSERT OR IGNORE INTO positions VALUES (?)', (record.position_key,))
        split_qualified = {}
        for name, indices in splits.items():
            split_qualified[name] = 0
            for i in indices:
                record = dataset[i]
                split_qualified[name] += eligible_label(record)
                database.execute('INSERT INTO split VALUES (?,?,?) ON CONFLICT(name,key) DO UPDATE SET later=max(later,excluded.later)',
                                 (name, record.position_key, int(record.ply > prefix_plies)))
        overlaps = {}
        for a, b in itertools.combinations(splits, 2):
            total, later = database.execute('SELECT count(*), coalesce(sum(a.later AND b.later),0) FROM split a JOIN split b ON a.key=b.key WHERE a.name=? AND b.name=?', (a, b)).fetchone()
            overlaps[f'{a}/{b}'] = {'canonical_positions': total, 'after_prefix': later}
        unique = database.execute('SELECT count(*) FROM trajectories').fetchone()[0]
        positions = database.execute('SELECT count(*) FROM positions').fetchone()[0]
    return {'records': len(dataset), 'games': games,
            'unique_complete_position_policy_trajectories': unique,
            'duplicate_complete_trajectories': games - unique, 'qualified_records': qualified,
            'unique_qualified_positions': positions, 'unknown_quality_records': unknown,
            'sources': dict(sorted(sources.items())), 'short_prefix_max_ply': prefix_plies,
            'overlaps': overlaps, 'split_records': {k: len(v) for k, v in splits.items()},
            'qualified_split_records': split_qualified,
            'opening_family_heldout_available': bool(splits.get('opening_heldout'))}
