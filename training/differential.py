"""Generate exact Python integer outputs for Rust/Python comparison."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from common import format_move, parse_game_record, parse_move, read_quantized_model


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--record", type=Path, required=True)
    parser.add_argument("--moves", nargs="*", default=[])
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    model = read_quantized_model(args.model)
    board, side = parse_game_record(args.record)
    moves = [parse_move(text) for text in args.moves]
    result = {
        "value": model.value(board, side),
        "policy": {
            format_move(move): model.policy(board, side, move) for move in moves
        },
    }
    encoded = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(encoded, encoding="utf-8")
        print(f"saved={args.output}")
    else:
        print(encoded, end="")


if __name__ == "__main__":
    main()
