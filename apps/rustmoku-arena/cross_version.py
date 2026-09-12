"""Freeze and describe two independent RustMoku pipe processes for paired Arena."""
import argparse
import json
import subprocess
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'training'))
from dataset import file_hash
from provenance import file_identity, snapshot_file, sidecar, validate_evidence
from manifest import read_manifest, save_manifest


def prepare(configuration, output):
    output.mkdir(parents=True, exist_ok=True)
    inputs = output / 'inputs'
    players = []
    arguments = ['--move-ms', str(configuration['move_ms']), '--depth', '64']
    for index, name in enumerate(('candidate', 'baseline')):
        command = configuration[name]
        if not isinstance(command, list) or len(command) < 2 or command[1] != 'pipe' or '--describe' in command:
            raise ValueError('each player must be an executable + pipe + explicit arguments')
        frozen = []
        for item in command:
            path = Path(item)
            if path.is_file():
                frozen.append(snapshot_file(file_identity(path), inputs)['path'])
            else:
                frozen.append(item)
        result = subprocess.run([*frozen, '--describe', 'true'], capture_output=True, text=True, check=True, timeout=15)
        effective = json.loads(result.stdout)
        if effective.get('protocol') != 'rustmoku-pipe-v1' or not 1 <= effective['threads'] <= 8 or effective['tt_mib'] > 1024:
            raise ValueError('unsupported process configuration/resource declaration')
        player = {'command': frozen, 'effective': effective, 'executable_sha256': file_hash(frozen[0])}
        if '--model' in command:
            original_model = Path(command[command.index('--model') + 1]).resolve()
            if sidecar(original_model, 'evidence').exists():
                evidence = validate_evidence(original_model)
                if evidence['export']['model']['sha256'] != effective['model_fingerprint']:
                    raise ValueError('pipe model differs from archived evidence')
                player['model_evidence_source'] = str(original_model)
        players.append(player)
        prefix = '--a-' if index == 0 else '--b-'
        arguments += [prefix + 'external', frozen[0]]
        for item in frozen[1:]:
            arguments += [prefix + 'external-arg', item]
    if players[0]['executable_sha256'] == players[1]['executable_sha256'] and configuration.get('require_distinct_versions', True):
        raise ValueError('cross-version mode requires different actual executable hashes')
    # Role is explicit; fixture/process validation never grants confirmation.
    config = {'arena': str(Path(configuration['arena']).resolve()), 'arguments': arguments,
              'process_players': players, 'max_pairs': configuration['max_pairs'],
              'suite_role': configuration.get('suite_role', 'smoke'), 'stop_rule': 'fixed_pairs',
              'sprt': configuration.get('sprt', {'h0': 0, 'h1': 5, 'alpha': .05, 'beta': .05}),
              'game_timeout_seconds': configuration.get('game_timeout_seconds', 60),
              'comparison_object': 'engine-model-profile', 'promotion_kind': 'engine-model-profile',
              'extra_inputs': [str(path.resolve()) for path in inputs.iterdir() if path.is_file()]}
    save_manifest(output / 'arena.json', config)
    return config


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    prepare(read_manifest(args.config), args.output)


if __name__ == '__main__':
    main()
