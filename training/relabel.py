"""Bounded comparison relabeling from Core-validated legal record prefixes."""
import argparse
import contextlib
import json
import sqlite3
import subprocess
import tempfile
from pathlib import Path
from dataset import file_hash, publish_shard
from common import save_split_manifest
from manifest import read_manifest
from teacher import comparison


def relabel(args):
    if not 1 <= args.max_positions <= 10000 or not 1 <= args.top_k <= 16:
        raise ValueError('explicit relabel bounds exceeded')
    identity = {'engine': file_hash(args.engine), 'records': [file_hash(p) for p in args.record],
                'model': file_hash(args.model) if args.model else None,
                'depth': args.depth, 'nodes': args.nodes, 'top_k': args.top_k,
                'temperature': args.temperature, 'max_positions': args.max_positions}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=args.output.parent) as directory:
        root = Path(directory)
        database = root / 'comparisons.sqlite'
        count = 0
        with contextlib.closing(sqlite3.connect(database)) as connection:
            connection.execute('PRAGMA cache_size=-4096')
            connection.execute('CREATE TABLE comparisons(position TEXT PRIMARY KEY, payload TEXT NOT NULL)')
            for source in args.record:
                lines = source.read_text().splitlines()
                moves_line = [i for i, line in enumerate(lines) if line.startswith('moves=')]
                if len(moves_line) != 1:
                    raise ValueError('record needs one moves= line')
                at = moves_line[0]
                moves = lines[at][6:].split()
                for ply in range(len(moves) + 1):
                    if count >= args.max_positions:
                        break
                    prefix = lines.copy()
                    prefix[at] = 'moves=' + ' '.join(moves[:ply])
                    path = root / 'prefix.txt'
                    path.write_text('\n'.join(prefix) + '\n')
                    if file_hash(args.engine) != identity['engine'] or (args.model and file_hash(args.model) != identity['model']):
                        raise ValueError('teacher changed during relabeling')
                    command = [str(args.engine.resolve()), 'analyze', '--record', str(path.resolve()),
                               '--depth', str(args.depth), '--nodes', str(args.nodes), '--top-k', str(args.top_k)]
                    if args.model:
                        command += ['--model', str(args.model.resolve())]
                    result = json.loads(subprocess.run(command, check=True, capture_output=True, text=True,
                                                       timeout=args.timeout).stdout)
                    count += 1
                    value = comparison(result, args.temperature)
                    if value is not None:
                        payload = json.dumps(value, sort_keys=True)
                        old = connection.execute('SELECT payload FROM comparisons WHERE position=?', (result['position_key'],)).fetchone()
                        if old is not None and old[0] != payload:
                            raise ValueError('inconsistent repeated teacher position')
                        connection.execute('INSERT OR IGNORE INTO comparisons VALUES (?,?)', (result['position_key'], payload))
                if count >= args.max_positions:
                    break
            connection.commit()
        if [file_hash(p) for p in args.record] != identity['records']:
            raise ValueError('source records changed during relabeling')
        publish_shard(database, args.output)
    sidecar = {'path': str(args.output.resolve()), 'sha256': file_hash(args.output), 'teacher': identity}
    if args.dataset:
        bundle = read_manifest(args.dataset)
        bundle['comparisons'] = sidecar
        save_split_manifest(args.output.with_suffix('.dataset.json'), bundle)
    save_split_manifest(args.output.with_suffix('.json'), sidecar)
    return sidecar


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--model', type=Path)
    parser.add_argument('--record', type=Path, action='append', required=True)
    parser.add_argument('--dataset', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--max-positions', type=int, default=32)
    parser.add_argument('--top-k', type=int, default=8)
    parser.add_argument('--depth', type=int, default=3)
    parser.add_argument('--nodes', type=int, default=5000)
    parser.add_argument('--temperature', type=float, default=1000)
    parser.add_argument('--timeout', type=float, default=30)
    relabel(parser.parse_args())


if __name__ == '__main__':
    main()
