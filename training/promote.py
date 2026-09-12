"""Publish an immutable candidate only after independent fixed-sample evidence."""

import argparse
import json
import os
import shutil
import sys
import tempfile
from pathlib import Path

from dataset import file_hash, open_dataset


def competition_identity(effective, index, process_players=None):
    if process_players:
        process = process_players[index]
        config = process['effective']
        return {'engine_sha256': process['executable_sha256'], 'rules': effective['rules'],
                'evaluator': config.get('evaluator', 'learned' if '--model' in process['command'] else 'pattern'),
                'model_sha256': config['model_fingerprint'] if '--model' in process['command'] else None,
                'threads': config['threads'], 'tt_mib': config['tt_mib'], 'profile': config['profile'],
                'book': False, 'tt_policy': 'fresh-per-game-warm-between-moves'}
    player = effective['players'][index]
    return {'engine_sha256': player.get('executable_sha256', effective['engine']['sha256']), 'rules': effective['rules'],
            'evaluator': player['evaluator'], 'model_sha256': (player['model']['sha256']
                if player['model'] is not None else None),
            'threads': player['threads'], 'tt_mib': player['tt_mib'], 'profile': player['profile'],
            'book': player.get('book'), 'tt_policy': player.get('tt_policy')}


def promote(experiment, candidate, champion, dataset_path=None):
    from manifest import read_manifest
    from provenance import validate_evidence, file_identity
    arena_tools = Path(__file__).resolve().parents[1] / 'apps' / 'rustmoku-arena'
    sys.path.insert(0, str(arena_tools))
    from experiment import completed_games, read_events, statistics, describe, verify_events
    manifest = read_manifest(experiment / 'manifest.json')
    if manifest.get('version') != 2:
        raise ValueError('promotion requires actual effective configuration and bound game events')
    effective = manifest['effective']
    if describe(Path(manifest['configuration']['arena']), manifest['configuration'].get('arguments', [])) != effective:
        raise ValueError('current Arena effective configuration differs from the experiment')
    for path, digest in manifest['inputs_sha256'].items():
        if file_hash(path) != digest:
            raise ValueError(f'frozen experiment input changed: {path}')
    completed = completed_games(read_events(experiment / 'events.jsonl'))
    verify_events(completed, manifest)
    digest = file_hash(candidate)
    a, b = [competition_identity(effective, index, manifest['configuration'].get('process_players')) for index in (0, 1)]
    if a['evaluator'] != 'learned' or a['model_sha256'] != digest:
        raise ValueError('candidate is not the actual Arena player A model')
    evidence = validate_evidence(candidate, dataset_path)
    from provenance import sidecar
    receipt = file_identity(sidecar(candidate, 'evidence'))
    if manifest['model_evidence'].get(digest) != receipt:
        raise ValueError('model evidence was not frozen in this experiment')
    if any(manifest['inputs_sha256'].get(path) != value for path, value in evidence['inputs_sha256'].items()):
        raise ValueError('export/checkpoint/data/check receipts were not frozen together')
    kind = manifest['configuration'].get('promotion_kind', 'model')
    if kind not in ('model', 'engine-model-profile'):
        raise ValueError('unsupported promotion kind')
    if kind == 'model':
        # A model-only claim requires identical engine/search/resource settings.
        # Both the actual profile and its engine binary survive publication.
        keys = ('engine_sha256', 'rules', 'threads', 'tt_mib', 'profile', 'book', 'tt_policy')
        if any(a[key] != b[key] for key in keys):
            raise ValueError('model-only promotion requires matching actual A/B profiles')
    pointer = champion / 'champion.json'
    previous = read_manifest(pointer) if pointer.exists() else None
    if previous is not None:
        if previous.get('competition_identity') == a:
            return {'status': 'already-champion', 'model_sha256': digest}
        if previous.get('competition_identity') != b:
            raise ValueError('actual Arena player B is not the current champion combination')
    elif b['evaluator'] != 'pattern':
        raise ValueError('initial champion must be an actual Pattern player')
    confirmation_starts = {event['row']['opening_key'] for event in completed.values()}
    with open_dataset(evidence['dataset_path'], resolver=evidence.get('resolver')) as dataset:
        if any(record.position_key.hex() in confirmation_starts for record in dataset):
            raise ValueError('confirmation opening overlaps the model-bound training corpus')
    report = statistics(completed, manifest['configuration'], effective)
    if not report['promotion_eligible']:
        return {'status': 'rejected', 'reason': 'independent fixed-time fixed-sample gate not passed'}
    champion.mkdir(parents=True, exist_ok=True)
    model = champion / f'{digest}.rmlp'
    if model.exists() and file_hash(model) != digest:
        raise ValueError('immutable model hash collision')
    if not model.exists():
        with tempfile.NamedTemporaryFile(dir=champion, suffix='.partial', delete=False) as output:
            temporary = Path(output.name)
            with candidate.open('rb') as source:
                shutil.copyfileobj(source, output)
            output.flush()
            os.fsync(output.fileno())
        if file_hash(temporary) != digest:
            temporary.unlink()
            raise ValueError('candidate changed during publication')
        try:
            os.link(temporary, model)
        except FileExistsError:
            if file_hash(model) != digest:
                raise ValueError('immutable model hash collision during publication')
        finally:
            temporary.unlink(missing_ok=True)
    # Keep identical receipts next to the stored model. Their original input
    # references remain auditable; copying is not a new calibration or export.
    from manifest import save_manifest
    for name in ('export', 'calibration', 'integer', 'evidence'):
        save_manifest(sidecar(model, name), read_manifest(sidecar(candidate, name)))
    result = {'status': 'promoted', 'model_sha256': digest, 'model': str(model.resolve()),
              'experiment_manifest_sha256': file_hash(experiment / 'manifest.json'), 'previous': previous,
              'promotion_kind': kind, 'competition_identity': a, 'match_limits': effective['limits'],
              'model_evidence': receipt}
    with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', dir=champion, suffix='.partial', delete=False) as output:
        temporary = Path(output.name)
        json.dump(result, output, sort_keys=True, indent=2)
        output.flush()
        os.fsync(output.fileno())
    temporary.replace(pointer)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('experiment', 'candidate', 'champion'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--dataset', type=Path, help='optional assertion; must match export-bound corpus')
    args = parser.parse_args()
    print(json.dumps(promote(args.experiment, args.candidate, args.champion, args.dataset), indent=2))


if __name__ == '__main__':
    main()
