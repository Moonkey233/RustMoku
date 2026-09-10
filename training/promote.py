"""Publish an immutable candidate only after independent fixed-sample evidence."""

import argparse
import json
import os
import shutil
import sys
import tempfile
from pathlib import Path

from dataset import file_hash, open_dataset


def promote(experiment, candidate, champion, dataset_path):
    arena_tools = Path(__file__).resolve().parents[1] / 'apps' / 'rustmoku-arena'
    sys.path.insert(0, str(arena_tools))
    from experiment import completed_games, read_events, statistics
    manifest = json.loads((experiment / 'manifest.json').read_text(encoding='utf-8'))
    completed = completed_games(read_events(experiment / 'events.jsonl'))
    report = statistics(completed, manifest['configuration'])
    if not report['promotion_eligible']:
        return {'status': 'rejected', 'reason': 'independent fixed-time fixed-sample gate not passed'}
    digest = file_hash(candidate)
    if manifest['inputs_sha256'].get(str(candidate.resolve())) != digest:
        raise ValueError('candidate is not the frozen Arena model')
    arguments = manifest['configuration'].get('arguments', [])
    if '--a-model' not in arguments or Path(arguments[arguments.index('--a-model') + 1]).resolve() != candidate.resolve():
        raise ValueError('candidate must be Arena player A')
    with open_dataset(dataset_path) as dataset:
        training_positions = {r.position_key.hex() for r in dataset}
    if any(event['row']['opening_key'] in training_positions for event in completed.values()):
        raise ValueError('confirmation opening overlaps the supplied training corpus')
    pointer = champion / 'champion.json'
    previous = json.loads(pointer.read_text(encoding='utf-8')) if pointer.exists() else {'evaluator': 'pattern'}
    if previous.get('model_sha256') == digest:
        return {'status': 'already-champion', 'model_sha256': digest}
    if previous.get('model_sha256'):
        if '--b-model' not in arguments:
            raise ValueError('Arena opponent is not the current champion')
        opponent = Path(arguments[arguments.index('--b-model') + 1])
        if manifest['inputs_sha256'].get(str(opponent.resolve())) != previous['model_sha256']:
            raise ValueError('Arena opponent champion hash mismatch')
    elif ('--b-model' in arguments or '--b-external' in arguments
          or ('--b-evaluator' in arguments and arguments[arguments.index('--b-evaluator') + 1] != 'pattern')):
        raise ValueError('initial champion is Pattern')
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
        temporary.replace(model)
    result = {'status': 'promoted', 'model_sha256': digest, 'model': str(model.resolve()),
              'experiment_manifest_sha256': file_hash(experiment / 'manifest.json'), 'previous': previous}
    with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', dir=champion, suffix='.partial', delete=False) as output:
        temporary = Path(output.name)
        json.dump(result, output, sort_keys=True, indent=2)
        output.flush()
        os.fsync(output.fileno())
    temporary.replace(pointer)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('experiment', 'candidate', 'champion', 'dataset'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(promote(args.experiment, args.candidate, args.champion, args.dataset), indent=2))


if __name__ == '__main__':
    main()
