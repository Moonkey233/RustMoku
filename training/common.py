"""Shared RustMoku V0.12 dataset, feature, and model definitions."""

from __future__ import annotations

import array
import dataclasses
import mmap
import os
import random
import struct
import sys
from pathlib import Path
from typing import Iterator, Sequence

import torch
from torch import Tensor, nn


BOARD_SIZE = 15
CELL_COUNT = BOARD_SIZE * BOARD_SIZE
POSITION_KEY_BYTES = (CELL_COUNT + 3) // 4 + 1
DATA_MAGIC = b"RMDATA01"
DATA_VERSION = 1
DATA_HEADER = struct.Struct("<8sHHI")
DATA_RECORD_PREFIX = struct.Struct("<QHBBiBB")
DATA_RECORD_BYTES = DATA_RECORD_PREFIX.size + POSITION_KEY_BYTES
MAX_DATA_RECORDS = 10_000_000

MODEL_MAGIC = b"RMLPV001"
MODEL_FORMAT_VERSION = 1
MODEL_ARCHITECTURE_ID = 1
MODEL_FEATURE_COUNT = 1 << 16
MODEL_HIDDEN = 16
MODEL_HEADER = struct.Struct("<8sHHIHHiqiIHH")
MAX_MODEL_BYTES = 4 * 1024 * 1024

EVALUATION_LIMIT = 10_000_000
POLICY_OUTPUT_SCALE = 4_096
DIRECTIONS = ((0, 1), (1, 0), (1, 1), (1, -1))
OFFSETS = (-4, -3, -2, -1, 1, 2, 3, 4)


@dataclasses.dataclass(frozen=True)
class DataRecord:
    game_id: int
    ply: int
    canonical_symmetry: int
    policy_move: int | None
    value: int
    source: int
    exact: bool
    position_key: bytes


class DatasetFile(Sequence[DataRecord]):
    """Checked, random-access view of a RustMoku dataset.

    The file remains memory mapped so opening a large dataset does not copy its
    whole payload into Python objects before game-level splitting.
    """

    def __init__(self, path: str | os.PathLike[str]) -> None:
        self.path = Path(path)
        self._file = self.path.open("rb")
        try:
            size = self.path.stat().st_size
            if size < DATA_HEADER.size:
                raise ValueError("truncated dataset header")
            maximum = DATA_HEADER.size + MAX_DATA_RECORDS * DATA_RECORD_BYTES
            if size > maximum:
                raise ValueError("dataset file exceeds safety limit")
            self._map = mmap.mmap(self._file.fileno(), 0, access=mmap.ACCESS_READ)
            magic, version, flags, count = DATA_HEADER.unpack_from(self._map)
            if magic != DATA_MAGIC or version != DATA_VERSION or flags != 0:
                raise ValueError("invalid dataset magic, version, or flags")
            if count > MAX_DATA_RECORDS:
                raise ValueError("dataset record count exceeds safety limit")
            expected = DATA_HEADER.size + count * DATA_RECORD_BYTES
            if size != expected:
                raise ValueError("dataset record count does not match file length")
            self._count = count
        except Exception:
            if hasattr(self, "_map"):
                self._map.close()
            self._file.close()
            raise

    def __len__(self) -> int:
        return self._count

    def __getitem__(self, index: int | slice) -> DataRecord | list[DataRecord]:
        if isinstance(index, slice):
            return [self[item] for item in range(*index.indices(len(self)))]
        if index < 0:
            index += len(self)
        if not 0 <= index < len(self):
            raise IndexError(index)
        offset = DATA_HEADER.size + index * DATA_RECORD_BYTES
        game_id, ply, symmetry, policy, value, source, exact = (
            DATA_RECORD_PREFIX.unpack_from(self._map, offset)
        )
        key_start = offset + DATA_RECORD_PREFIX.size
        key = bytes(self._map[key_start : key_start + POSITION_KEY_BYTES])
        if symmetry >= 8:
            raise ValueError(f"record {index} has an invalid symmetry tag")
        if policy != 0xFF and policy >= CELL_COUNT:
            raise ValueError(f"record {index} has an invalid policy move")
        if source >= 8:
            raise ValueError(f"record {index} has an invalid result source")
        if exact not in (0, 1):
            raise ValueError(f"record {index} has an invalid exact flag")
        decode_position_key(key)
        return DataRecord(
            game_id=game_id,
            ply=ply,
            canonical_symmetry=symmetry,
            policy_move=None if policy == 0xFF else policy,
            value=value,
            source=source,
            exact=bool(exact),
            position_key=key,
        )

    def close(self) -> None:
        if hasattr(self, "_map"):
            self._map.close()
            del self._map
        self._file.close()

    def __enter__(self) -> "DatasetFile":
        return self

    def __exit__(self, *_args: object) -> None:
        self.close()


def decode_position_key(key: bytes) -> tuple[list[int], int]:
    if len(key) != POSITION_KEY_BYTES:
        raise ValueError("invalid canonical-position key length")
    board = []
    for index in range(CELL_COUNT):
        code = (key[index // 4] >> ((3 - index % 4) * 2)) & 3
        if code == 3:
            raise ValueError("canonical-position key contains a reserved cell code")
        board.append(code)
    used_cells = CELL_COUNT % 4
    if used_cells and key[-2] & ((1 << ((4 - used_cells) * 2)) - 1):
        raise ValueError("canonical-position key contains nonzero padding")
    side = key[-1]
    if side not in (0, 1):
        raise ValueError("canonical-position key contains an invalid side")
    return board, side


def transform_index(index: int, symmetry: int) -> int:
    row, column = divmod(index, BOARD_SIZE)
    last = BOARD_SIZE - 1
    transformed = (
        (row, column),
        (column, last - row),
        (last - row, last - column),
        (last - column, row),
        (row, last - column),
        (last - row, column),
        (column, row),
        (last - column, last - row),
    )
    if not 0 <= symmetry < len(transformed):
        raise ValueError("symmetry must be in 0..8")
    new_row, new_column = transformed[symmetry]
    return new_row * BOARD_SIZE + new_column


def transform_position(
    board: Sequence[int], policy_move: int | None, symmetry: int
) -> tuple[list[int], int | None]:
    transformed = [0] * CELL_COUNT
    for index, stone in enumerate(board):
        transformed[transform_index(index, symmetry)] = stone
    policy = None if policy_move is None else transform_index(policy_move, symmetry)
    return transformed, policy


def relative_line_key(key: int, side: int) -> int:
    if side == 0:
        return key
    result = 0
    for field in range(8):
        code = (key >> (field * 2)) & 3
        if code == 1:
            code = 2
        elif code == 2:
            code = 1
        result |= code << (field * 2)
    return result


def line_keys_at(board: Sequence[int], center: int, side: int) -> tuple[int, ...]:
    row, column = divmod(center, BOARD_SIZE)
    keys = []
    for row_step, column_step in DIRECTIONS:
        key = 0
        for field, offset in enumerate(OFFSETS):
            other_row = row + row_step * offset
            other_column = column + column_step * offset
            code = (
                board[other_row * BOARD_SIZE + other_column]
                if 0 <= other_row < BOARD_SIZE and 0 <= other_column < BOARD_SIZE
                else 3
            )
            key |= code << (field * 2)
        keys.append(relative_line_key(key, side))
    return tuple(keys)


def feature_keys(board: Sequence[int], side: int) -> list[int]:
    return [
        key
        for center in range(CELL_COUNT)
        for key in line_keys_at(board, center, side)
    ]


def legal_policy_features(
    board: Sequence[int], side: int
) -> tuple[list[int], list[tuple[int, ...]]]:
    moves = [index for index, cell in enumerate(board) if cell == 0]
    return moves, [line_keys_at(board, index, side) for index in moves]


def calibrated_value_target(record: DataRecord) -> float:
    if record.exact:
        if record.value > 0:
            return 1.0
        if record.value < 0:
            return -1.0
        return 0.0
    return float(max(-EVALUATION_LIMIT, min(EVALUATION_LIMIT, record.value))) / EVALUATION_LIMIT


def split_indices(
    dataset: Sequence[DataRecord], seed: int
) -> dict[str, list[int]]:
    """Split whole game IDs before any position augmentation."""
    game_ids = sorted({dataset[index].game_id for index in range(len(dataset))})
    random.Random(seed).shuffle(game_ids)
    game_count = len(game_ids)
    test_count = 1 if game_count >= 3 else 0
    validation_count = 1 if game_count >= 2 else 0
    if game_count >= 20:
        test_count = max(1, round(game_count * 0.1))
        validation_count = max(1, round(game_count * 0.1))
    while test_count + validation_count >= game_count and test_count:
        test_count -= 1
    train_count = game_count - validation_count - test_count
    split_by_game: dict[int, str] = {}
    for game_id in game_ids[:train_count]:
        split_by_game[game_id] = "train"
    for game_id in game_ids[train_count : train_count + validation_count]:
        split_by_game[game_id] = "validation"
    for game_id in game_ids[train_count + validation_count :]:
        split_by_game[game_id] = "test"
    result = {"train": [], "validation": [], "test": []}
    for index in range(len(dataset)):
        result[split_by_game[dataset[index].game_id]].append(index)
    return result


class LocalPatternModel(nn.Module):
    """Small float training model matching the production integer topology."""

    def __init__(self) -> None:
        super().__init__()
        self.embedding = nn.Embedding(MODEL_FEATURE_COUNT, MODEL_HIDDEN)
        self.value_head = nn.Linear(MODEL_HIDDEN, 1)
        self.policy_head = nn.Parameter(torch.empty(MODEL_HIDDEN))
        nn.init.normal_(self.embedding.weight, mean=0.0, std=0.002)
        nn.init.normal_(self.value_head.weight, mean=0.0, std=0.002)
        nn.init.zeros_(self.value_head.bias)
        nn.init.normal_(self.policy_head, mean=0.0, std=0.002)

    def value(self, global_keys: Tensor) -> Tensor:
        accumulator = self.embedding(global_keys).sum(dim=-2)
        return self.value_head(accumulator).squeeze(-1)

    def policy(self, candidate_keys: Tensor) -> Tensor:
        local = self.embedding(candidate_keys).sum(dim=-2)
        return local @ self.policy_head


def load_training_model(path: str | os.PathLike[str], device: str) -> LocalPatternModel:
    checkpoint = torch.load(path, map_location=device, weights_only=True)
    if checkpoint.get("format") != "rustmoku-local-pattern-v1":
        raise ValueError("unsupported training checkpoint")
    model = LocalPatternModel().to(device)
    model.load_state_dict(checkpoint["state_dict"])
    return model


@dataclasses.dataclass(frozen=True)
class QuantizedModel:
    embeddings: array.array
    value_head: tuple[int, ...]
    policy_head: tuple[int, ...]
    value_bias: int
    value_scale: int
    policy_scale: int

    def embedding(self, key: int, dimension: int) -> int:
        return self.embeddings[key * MODEL_HIDDEN + dimension]

    def value(self, board: Sequence[int], side: int) -> int:
        accumulator = [0] * MODEL_HIDDEN
        for key in feature_keys(board, side):
            offset = key * MODEL_HIDDEN
            for dimension in range(MODEL_HIDDEN):
                accumulator[dimension] += self.embeddings[offset + dimension]
        dot = self.value_bias + sum(
            feature * weight
            for feature, weight in zip(accumulator, self.value_head, strict=True)
        )
        score = truncating_division(dot, self.value_scale)
        return max(-EVALUATION_LIMIT, min(EVALUATION_LIMIT, score))

    def policy(self, board: Sequence[int], side: int, move: int) -> int:
        if board[move] != 0:
            raise ValueError("policy move is occupied")
        score = 0
        for dimension in range(MODEL_HIDDEN):
            local = sum(
                self.embedding(key, dimension)
                for key in line_keys_at(board, move, side)
            )
            score += local * self.policy_head[dimension]
        result = truncating_division(score, self.policy_scale)
        return max(-(1 << 15), min((1 << 15) - 1, result))


def truncating_division(numerator: int, denominator: int) -> int:
    quotient = abs(numerator) // denominator
    return -quotient if numerator < 0 else quotient


def read_quantized_model(path: str | os.PathLike[str]) -> QuantizedModel:
    model_path = Path(path)
    size = model_path.stat().st_size
    if size > MAX_MODEL_BYTES:
        raise ValueError("model file exceeds safety limit")
    data = model_path.read_bytes()
    if len(data) < MODEL_HEADER.size:
        raise ValueError("truncated model header")
    (
        magic,
        format_version,
        architecture,
        feature_count,
        hidden,
        flags,
        value_scale,
        value_bias,
        policy_scale,
        embedding_count,
        value_count,
        policy_count,
    ) = MODEL_HEADER.unpack_from(data)
    if (
        magic != MODEL_MAGIC
        or format_version != MODEL_FORMAT_VERSION
        or architecture != MODEL_ARCHITECTURE_ID
        or feature_count != MODEL_FEATURE_COUNT
        or hidden != MODEL_HIDDEN
        or flags != 0
        or embedding_count != MODEL_FEATURE_COUNT * MODEL_HIDDEN
        or value_count != MODEL_HIDDEN
        or policy_count != MODEL_HIDDEN
        or value_scale <= 0
        or policy_scale <= 0
    ):
        raise ValueError("model header does not match the V0.12 architecture")
    maximum_accumulator = CELL_COUNT * 4 * 32768
    maximum_value_dot = maximum_accumulator * 32768 * MODEL_HIDDEN
    if abs(value_bias) > (2**63 - 1) - maximum_value_dot:
        raise ValueError("model Value bias violates Rust arithmetic bounds")
    value_count_total = embedding_count + value_count + policy_count
    if len(data) != MODEL_HEADER.size + value_count_total * 2:
        raise ValueError("model payload length mismatch")
    values = array.array("h")
    values.frombytes(data[MODEL_HEADER.size :])
    if sys.byteorder != "little":
        values.byteswap()
    embeddings = array.array("h", values[:embedding_count])
    value_head = tuple(values[embedding_count : embedding_count + value_count])
    policy_head = tuple(values[embedding_count + value_count :])
    return QuantizedModel(
        embeddings=embeddings,
        value_head=value_head,
        policy_head=policy_head,
        value_bias=value_bias,
        value_scale=value_scale,
        policy_scale=policy_scale,
    )


def parse_game_record(path: str | os.PathLike[str]) -> tuple[list[int], int]:
    lines = Path(path).read_text(encoding="utf-8").splitlines()
    if len(lines) != 3 or lines[0] != "RustMoku 1" or lines[1] != "rules=freestyle":
        raise ValueError("expected a RustMoku 1 Freestyle game record")
    if not lines[2].startswith("moves="):
        raise ValueError("record is missing moves")
    board = [0] * CELL_COUNT
    side = 0
    for token in lines[2][len("moves=") :].split():
        move = parse_move(token)
        if board[move] != 0:
            raise ValueError(f"duplicate move {token}")
        board[move] = side + 1
        side = 1 - side
    return board, side


def parse_move(text: str) -> int:
    text = text.upper()
    if len(text) not in (2, 3) or not "A" <= text[0] <= "O":
        raise ValueError(f"invalid move {text!r}")
    human_row = int(text[1:])
    if not 1 <= human_row <= BOARD_SIZE:
        raise ValueError(f"invalid move {text!r}")
    row = BOARD_SIZE - human_row
    column = ord(text[0]) - ord("A")
    return row * BOARD_SIZE + column


def format_move(index: int) -> str:
    row, column = divmod(index, BOARD_SIZE)
    return f"{chr(ord('A') + column)}{BOARD_SIZE - row}"


def iter_records(dataset: Sequence[DataRecord], indices: Sequence[int]) -> Iterator[DataRecord]:
    for index in indices:
        yield dataset[index]
