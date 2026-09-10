"""Minimal architecture-2 float oracle, isolated from production V1 export.

One cross-direction ReLU and center occupancy are the only added operations.
This reference recomputes the global head; no quantization or strength claim.
"""

import argparse
import array
import struct
import sys
from pathlib import Path

import torch
from torch import nn

from common import CELL_COUNT, feature_keys, line_keys_at, parse_game_record, parse_move


class NonlinearReference(nn.Module):
    def __init__(self):
        super().__init__()
        self.embedding = nn.Embedding(65536, 8, dtype=torch.float64)
        self.center = nn.Embedding(3, 8, dtype=torch.float64)
        self.value_head = nn.Linear(8, 1, dtype=torch.float64)
        self.policy_head = nn.Parameter(torch.empty(8, dtype=torch.float64))
        for parameter in self.parameters():
            nn.init.normal_(parameter, std=.01)

    def value(self, keys, centers):
        local = torch.relu(self.embedding(keys).sum(-2) + self.center(centers))
        return self.value_head(local.mean(-2)).squeeze(-1)

    def policy(self, keys):
        local = torch.relu(self.embedding(keys).sum(-2) + self.center.weight[0])
        return local @ self.policy_head

    def board_value(self, board, side):
        keys = torch.tensor(feature_keys(board, side)).reshape(CELL_COUNT, 4)
        centers = torch.tensor([0 if code == 0 else (1 if code == side + 1 else 2) for code in board])
        return self.value(keys, centers)

    def export_reference(self, path):
        tensors = (self.embedding.weight, self.center.weight, self.value_head.weight,
                   self.policy_head, self.value_head.bias)
        values = torch.cat([tensor.detach().flatten() for tensor in tensors])
        if not torch.isfinite(values).all() or values.abs().max() > 16:
            raise ValueError('reference weights must be finite and within +/-16')
        payload = array.array('d', values.tolist())
        if sys.byteorder != 'little':
            payload.byteswap()
        Path(path).write_bytes(struct.pack('<8sHHI', b'RMLREF02', 2, 8, 65536) + payload.tobytes())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--record', type=Path, required=True)
    parser.add_argument('--move', required=True)
    args = parser.parse_args()
    torch.manual_seed(7)
    torch.set_num_threads(1)
    model = NonlinearReference()
    model.export_reference(args.output)
    board, side = parse_game_record(args.record)
    at = parse_move(args.move)
    with torch.no_grad():
        print(f'value={model.board_value(board, side).item():.17f}')
        print(f'policy={model.policy(torch.tensor(line_keys_at(board, at, side))).item():.17f}')


if __name__ == '__main__':
    main()
