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


def resolve_input(value, base):
    """Resolve declared file inputs relative to the experiment configuration."""
    path = Path(value)
    if base is None and not path.is_absolute():
        raise ValueError('relative inputs require an explicit config_directory')
    return (base / path).resolve() if not path.is_absolute() else path.resolve()


def freeze_command(command, base, inputs):
    if (not isinstance(command, list) or len(command) < 2 or command[1] != 'pipe'
            or any(not isinstance(item, str) for item in command) or '--describe' in command):
        raise ValueError('each player must be an executable + pipe + explicit arguments')
    # The executable and --model are file inputs. --profile is serialized text,
    # not a path, even when a file with that name happens to exist.
    frozen = [snapshot_file(file_identity(resolve_input(command[0], base)), inputs)['path'], 'pipe']
    if len(command[2:]) % 2:
        raise ValueError('pipe options require explicit flag/value pairs')
    for flag, value in zip(command[2::2], command[3::2]):
        if not flag.startswith('--'):
            raise ValueError('pipe options require explicit flag/value pairs')
        if flag == '--model':
            value = snapshot_file(file_identity(resolve_input(value, base)), inputs)['path']
        frozen.extend((flag, value))
    return frozen


def prepare(configuration, output, *, config_directory=None):
    base = Path(config_directory).resolve() if config_directory is not None else None
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    inputs = output / 'inputs'
    players = []
    arguments = ['--move-ms', str(configuration['move_ms']), '--depth', '64']
    for index, name in enumerate(('candidate', 'baseline')):
        command = configuration[name]
        frozen = freeze_command(command, base, inputs)
        result = subprocess.run([*frozen, '--describe', 'true'], capture_output=True, text=True, check=True, timeout=15)
        effective = json.loads(result.stdout)
        if effective.get('protocol') != 'rustmoku-pipe-v1' or not 1 <= effective['threads'] <= 8 or effective['tt_mib'] > 1024:
            raise ValueError('unsupported process configuration/resource declaration')
        player = {'command': frozen, 'effective': effective, 'executable_sha256': file_hash(frozen[0])}
        if '--model' in command:
            original_model = resolve_input(command[command.index('--model') + 1], base)
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
    config = {'arena': str(resolve_input(configuration['arena'], base)), 'arguments': arguments,
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
    prepare(read_manifest(args.config), args.output, config_directory=args.config.resolve().parent)


if __name__ == '__main__':
    main()
