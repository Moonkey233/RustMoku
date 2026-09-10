"""Executable Rust/Python differential over fixed legal records and candidates."""

import argparse
import subprocess
import tempfile
from pathlib import Path

from common import parse_game_record, parse_move, read_quantized_model


def verify(engine, model_path):
    model = read_quantized_model(model_path)
    checks = 0
    with tempfile.TemporaryDirectory() as directory:
        record = Path(directory) / 'position.rmg'
        for moves in ('', 'H8', 'H8 I8', 'H8 I8 H9', 'A1 O15 A2 O14 B1 N15'):
            record.write_text(f'RustMoku 1\nrules=freestyle\nmoves={moves}\n', encoding='utf-8')
            board, side = parse_game_record(record)
            for at in ('A15', 'O1', 'G7'):
                result = subprocess.run([str(engine.resolve()), 'model-check', '--model', str(model_path.resolve()),
                                         '--record', str(record), '--move', at],
                                        capture_output=True, text=True, check=True, timeout=10)
                actual = dict(line.split('=', 1) for line in result.stdout.splitlines())
                expected = {'value': str(model.value(board, side)),
                            'policy': str(model.policy(board, side, parse_move(at)))}
                if actual != expected:
                    raise ValueError(f'integer mismatch: {moves}, {at}: {actual} != {expected}')
                checks += 1
    print(f'integer_differential_checks={checks} passed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--model', type=Path, required=True)
    args = parser.parse_args()
    verify(args.engine, args.model)


if __name__ == '__main__':
    main()
