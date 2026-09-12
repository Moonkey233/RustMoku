"""Production V2 float training and bit-exact integer specification.

Ordinary scores use q=score/(S+abs(score)); S is frozen from training data.
The integer inverse uses Q15, clamps to +/-32735, then clamps ordinary score.
No network prediction represents a proof or a mate distance.
"""

import array
import dataclasses
import struct
import sys

import torch
from torch import nn
from torch.nn import functional as F

from common import (CELL_COUNT, EVALUATION_LIMIT, feature_keys, line_keys_at,
                    truncating_division)

WIDTH = 8
FEATURES = 65536
UNIT = 32768
CLIP = 32735
HEADER = struct.Struct('<8sHHIHHiiiqI')
WEIGHTS = (FEATURES + 3 + 2) * WIDTH
MAX_HEAD_SUM = CELL_COUNT * WIDTH * 5 * 32768**2
FORMAT = 'rustmoku-nonlinear-v2'
ARCHITECTURE = 'local-pattern-relu-width8-v2'
CONTRACT = 'stm-rational-q15-v2'


def normalized_target(record, score_scale):
    if record.exact:
        return float((record.value > 0) - (record.value < 0))
    return record.value / (score_scale + abs(record.value))


def model_features(board, side, device='cpu'):
    keys = torch.tensor(feature_keys(board, side), dtype=torch.long, device=device).reshape(CELL_COUNT, 4)
    centers = torch.tensor([0 if cell == 0 else (1 if cell == side + 1 else 2) for cell in board],
                           dtype=torch.long, device=device)
    return torch.cat((keys, centers.unsqueeze(-1)), dim=-1)


class NonlinearModel(nn.Module):
    def __init__(self, qat=False):
        super().__init__()
        self.embedding = nn.Embedding(FEATURES, WIDTH)
        self.center = nn.Embedding(3, WIDTH)
        self.value_head = nn.Linear(WIDTH, 1)
        self.policy_head = nn.Parameter(torch.empty(WIDTH))
        self.qat = qat
        for parameter in self.parameters():
            nn.init.normal_(parameter, std=.01)
        nn.init.zeros_(self.value_head.bias)

    def weight(self, tensor):
        if not self.qat:
            return tensor
        # Fixed PTQ scales; straight-through derivative, exact rounded forward.
        integer = torch.round(tensor * 16384).clamp(-32768, 32767) / 16384
        return tensor + (integer - tensor).detach()

    def activated(self, keys, centers):
        return torch.relu(F.embedding(keys, self.weight(self.embedding.weight)).sum(-2)
                          + F.embedding(centers, self.weight(self.center.weight)))

    def value(self, keys):
        local = self.activated(keys[..., :4], keys[..., 4])
        return F.linear(local.mean(-2), self.weight(self.value_head.weight), self.value_head.bias).squeeze(-1)

    def policy(self, keys):
        centers = torch.zeros(keys.shape[:-1], dtype=torch.long, device=keys.device)
        return self.activated(keys, centers) @ self.weight(self.policy_head)

    def board_value(self, board, side):
        return self.value(model_features(board, side, self.embedding.weight.device))


@dataclasses.dataclass(frozen=True)
class QuantizedNonlinear:
    embeddings: array.array
    centers: tuple
    value_head: tuple
    policy_head: tuple
    bias: int
    value_divisor: int
    policy_divisor: int
    score_scale: int

    def activated(self, board, side, at):
        center = 0 if board[at] == 0 else (1 if board[at] == side + 1 else 2)
        keys = line_keys_at(board, at, side)
        return [max(0, self.centers[center * WIDTH + d] +
                    sum(self.embeddings[key * WIDTH + d] for key in keys)) for d in range(WIDTH)]

    def normalized(self, board, side):
        value = self.bias + sum(sum(a * h for a, h in zip(self.activated(board, side, at), self.value_head))
                                for at in range(CELL_COUNT))
        return max(-CLIP, min(CLIP, truncating_division(value, CELL_COUNT * self.value_divisor)))

    def value(self, board, side):
        q = self.normalized(board, side)
        return max(-EVALUATION_LIMIT, min(EVALUATION_LIMIT, truncating_division(self.score_scale * q, UNIT - abs(q))))

    def policy(self, board, side, move):
        if not 0 <= move < CELL_COUNT or board[move] != 0:
            raise ValueError('policy move is occupied or out of range')
        value = sum(a * h for a, h in zip(self.activated(board, side, move), self.policy_head))
        return max(-32768, min(32767, truncating_division(value, self.policy_divisor)))


def read_integer(data):
    if len(data) != HEADER.size + WEIGHTS * 2:
        raise ValueError('V2 model length mismatch')
    magic, version, architecture, features, width, contract, vd, pd, scale, bias, flags = HEADER.unpack_from(data)
    if (magic != b'RMLPV002' or (version, architecture, features, width, contract, flags) != (2, 2, FEATURES, WIDTH, 2, 0)
            or vd <= 0 or pd <= 0 or not 1 <= scale <= EVALUATION_LIMIT
            or abs(bias) > 2**63 - 1 - MAX_HEAD_SUM):
        raise ValueError('invalid V2 architecture, scale or arithmetic bounds')
    values = array.array('h')
    values.frombytes(data[HEADER.size:])
    if sys.byteorder != 'little':
        values.byteswap()
    n = FEATURES * WIDTH
    return QuantizedNonlinear(values[:n], tuple(values[n:n+3*WIDTH]),
                              tuple(values[n+3*WIDTH:n+4*WIDTH]), tuple(values[n+4*WIDTH:]), bias, vd, pd, scale)


def export_integer(model, args, score_scale):
    from export import quantize, calibrated_divisor
    # Direction and center embeddings must share exactly the same units before ReLU.
    both = torch.cat((model.embedding.weight, model.center.weight))
    embeddings, es = quantize(both, args.embedding_scale, 'V2 embeddings and centers')
    value, vs = quantize(model.value_head.weight.flatten(), args.value_head_scale, 'V2 value head')
    policy, ps = quantize(model.policy_head, args.policy_head_scale, 'V2 policy head')
    vd, pd = calibrated_divisor(es * vs, UNIT), calibrated_divisor(es * ps, 4096)
    if max(vd, pd) > 2**31 - 1:
        raise ValueError('V2 divisor exceeds i32')
    bias = round(float(model.value_head.bias.item()) * es * vs * CELL_COUNT)
    if abs(bias) > 2**63 - 1 - MAX_HEAD_SUM:
        raise ValueError('V2 bias exceeds arithmetic bound')
    flat = torch.cat((embeddings.flatten(), value, policy)).tolist()
    payload = HEADER.pack(b'RMLPV002', 2, 2, FEATURES, WIDTH, 2, vd, pd, score_scale, bias, 0)
    payload += struct.pack(f'<{len(flat)}h', *flat)
    read_integer(payload)
    return payload, {'embedding_scale': es, 'value_head_scale': vs, 'policy_head_scale': ps,
                     'value_divisor': vd, 'policy_divisor': pd, 'score_scale': score_scale,
                     'activation': 'relu', 'division': 'truncate-toward-zero-after-global-sum',
                     'normalization_unit': UNIT, 'normalization_clip': CLIP}
