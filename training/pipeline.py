"""Finite single-machine generate/audit/train/check/match/reject workflow.

This entry point is deliberately a smoke pipeline. Formal experiments require
their own explicit resource and confirmation-opening configuration.
"""

import argparse
import json
import os
import tempfile
import subprocess
import sys
from pathlib import Path

from common import save_split_manifest
from dataset import file_hash, DatasetBundle
from promote import promote
from provenance import sidecar


def publish_state(path, state):
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', dir=path.parent,
                                         suffix='.partial', delete=False) as stream:
            temporary = Path(stream.name)
            json.dump(state, stream, indent=2)
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(path)
    finally:
        if temporary is not None and temporary.exists():
            temporary.unlink()


def run(config, root, only=None):
    if not (1 <= config['games'] <= 32 and 1 <= config['epochs'] <= 3 and 1 <= config['pairs'] <= 4):
        raise ValueError('smoke caps: 32 games, 3 epochs, 4 pairs')
    root.mkdir(parents=True, exist_ok=True)
    scripts = Path(__file__).resolve().parent
    repository = scripts.parent
    data_engine = Path(config['data_engine']).resolve()
    arena_engine = Path(config['arena_engine']).resolve()
    identity = {'version': 1, 'configuration': config,
                'data_engine_sha256': file_hash(data_engine), 'arena_engine_sha256': file_hash(arena_engine),
                'scripts': {path.name: file_hash(path) for path in sorted(scripts.glob('*.py'))}}
    save_split_manifest(root / 'pipeline.json', identity)
    data = root / 'data' / 'dataset.json'
    if data.exists():
        with DatasetBundle(data):
            pass
    checkpoint = root / 'candidate.pt'
    model = root / 'candidate.rmlp'
    arena_config = {'arena': str(arena_engine), 'arguments': [
        '--depth', '8', '--move-ms', str(config.get('move_ms', 10)),
        '--a-model', str(model.resolve()), '--a-tt-mib', '1', '--b-tt-mib', '1'],
        'max_pairs': config['pairs'], 'suite_role': 'smoke', 'stop_rule': 'fixed_pairs',
        'sprt': {'h0': 0, 'h1': 5, 'alpha': .05, 'beta': .05}, 'game_timeout_seconds': 30}
    save_split_manifest(root / 'arena.json', arena_config)
    python = sys.executable
    stages = [
        ('generate', [python, str(scripts / 'generate.py'), '--engine', str(data_engine), '--output', str(root / 'data'),
                      '--games', str(config['games']), '--seed', str(config['seed']), '--depth', '1', '--nodes', '100'], [data]),
        ('audit-split', [python, str(scripts / 'audit.py'), '--dataset', str(data), '--manifest', str(root / 'split.json'),
                        '--seed', str(config['seed'])], [root / 'split.json']),
        ('train', [python, str(scripts / 'train.py'), '--dataset', str(data), '--output', str(checkpoint),
                   '--epochs', str(config['epochs']), '--seed', str(config['seed']), '--batch-size', '8'], [checkpoint]),
        ('evaluate', [python, str(scripts / 'evaluate.py'), '--dataset', str(data), '--checkpoint', str(checkpoint)], []),
        ('export', [python, str(scripts / 'export.py'), '--checkpoint', str(checkpoint), '--dataset', str(data),
                    '--output', str(model)], [model, sidecar(model, 'export')]),
        ('calibrate', [python, str(scripts / 'calibrate.py'), '--dataset', str(data), '--checkpoint', str(checkpoint),
                       '--model', str(model), '--samples', '8'], [sidecar(model, 'calibration')]),
        ('integer', [python, str(scripts / 'verify_integer.py'), '--engine', str(data_engine), '--model', str(model)],
                    [sidecar(model, 'integer'), sidecar(model, 'evidence')]),
        ('tactical', ['cargo', 'test', '--release', '-p', 'rustmoku-engine', 'search::'], []),
        ('arena', [python, str(repository / 'apps/rustmoku-arena/experiment.py'), '--config', str(root / 'arena.json'),
                   '--output', str(root / 'arena')], [root / 'arena/statistics.json']),
    ]
    names = [name for name, _, _ in stages] + ['promote']
    if only is not None and only not in names:
        raise ValueError(f'unknown stage {only}')
    for name, command, outputs in stages:
        state_path = root / f'{name}.state.json'
        if state_path.exists():
            state = json.loads(state_path.read_text(encoding='utf-8'))
            if state['status'] == 'complete':
                for path, digest in state['outputs'].items():
                    if file_hash(path) != digest:
                        raise ValueError(f'completed stage artifact changed: {path}')
                continue
        if only is not None and only != name:
            if names.index(name) < names.index(only):
                raise ValueError(f'required stage {name} is not complete')
            continue
        if name == 'train' and checkpoint.exists():
            command += ['--resume', str(checkpoint)]
        publish_state(state_path, {'status': 'running', 'command': command})
        try:
            result = subprocess.run(command, cwd=repository, capture_output=True, text=True, check=True, timeout=60)
            (root / f'{name}.log').write_text(result.stdout + result.stderr, encoding='utf-8')
            publish_state(state_path, {'status': 'complete', 'command': command,
                'outputs': {str(path.resolve()): file_hash(path) for path in outputs + [root / f'{name}.log']}})
            print(f'{name}: complete', flush=True)
        except Exception as error:
            publish_state(state_path, {'status': 'failed', 'error': str(error)})
            if isinstance(error, subprocess.CalledProcessError):
                (root / f'{name}.log').write_text(error.stdout + error.stderr, encoding='utf-8')
            raise
        if only == name:
            return
    if only in (None, 'promote'):
        result = promote(root / 'arena', model, root / 'champion', data)
        save_split_manifest(root / 'promotion.json', result)
        print(json.dumps(result))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--stage')
    args = parser.parse_args()
    run(json.loads(args.config.read_text(encoding='utf-8')), args.output.resolve(), args.stage)


if __name__ == '__main__':
    main()
