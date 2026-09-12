"""Explicit bounded offline ordering migration for interleaved legacy raw shards.

The original bytes and historical fingerprints are never overwritten. Unknown
teacher/lineage metadata cannot be reconstructed from raw records.
"""
import argparse
import contextlib
import sqlite3
import tempfile
from pathlib import Path
from common import DatasetFile, DATA_HEADER
from dataset import describe_shard, file_hash, publish_shard
from manifest import save_manifest


def migrate(source, destination, max_records=1_000_000):
    source, destination = Path(source), Path(destination)
    if source.resolve() == destination.resolve():
        raise ValueError('migration requires a distinct immutable output path')
    before = file_hash(source)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with DatasetFile(source) as records, tempfile.TemporaryDirectory(dir=destination.parent) as directory:
        if not 1 <= max_records <= 1_000_000 or len(records) > max_records:
            raise ValueError('migration exceeds one-million-record offline safety cap')
        root = Path(directory)
        with contextlib.closing(sqlite3.connect(root / 'ordering.sqlite')) as connection:
            connection.execute('PRAGMA cache_size=-8192')
            connection.execute('PRAGMA temp_store=FILE')
            connection.execute('CREATE TABLE records(game TEXT, ply INTEGER, payload BLOB, PRIMARY KEY(game,ply)) WITHOUT ROWID')
            with source.open('rb') as stream:
                header = stream.read(DATA_HEADER.size)
                for record in records:
                    if record.ply > 225:
                        raise ValueError('invalid ply in legacy record')
                    payload = stream.read(records.record_bytes)
                    connection.execute('INSERT INTO records VALUES (?,?,?)', (f'{record.game_id:016x}', record.ply, payload))
            connection.commit()
            temporary = root / 'ordered.rmd'
            with temporary.open('wb') as stream:
                stream.write(header)
                for (payload,) in connection.execute('SELECT payload FROM records ORDER BY game,ply'):
                    stream.write(payload)
        descriptor = describe_shard(temporary, {'status': 'unknown-legacy-offline-migration', 'original_sha256': before}, before, compact=True)
        if file_hash(source) != before:
            raise ValueError('legacy source changed during migration')
        publish_shard(temporary, destination)
    descriptor['path'] = str(destination.resolve())
    save_manifest(destination.with_suffix('.json'), {'version': 2, 'shards': [descriptor],
        'migration': {'original_sha256': before, 'operation': 'sort-game-id-ply-preserve-record-bytes'}})
    return descriptor


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--max-records', type=int, default=1_000_000)
    args = parser.parse_args()
    migrate(args.input, args.output, args.max_records)
