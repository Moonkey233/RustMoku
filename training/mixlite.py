"""MixLite V3 scalar/QAT specification and bounded standalone training pipeline.

Local int8 mapping -> four groups plus global mean -> eight nonlinear context
lanes -> positive WDL evidence and bilinear contextual policy. Promotion remains
closed until this architecture has calibrated independent experiment evidence.
"""
import argparse
import array
import json
import math
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import torch
from torch import nn
from torch.nn import functional as F
from common import (feature_keys, decode_position_key, parse_game_record, parse_move, transform_position, transform_index,
                    truncating_division, make_split_manifest, validate_split_manifest)
from dataset import open_dataset, file_hash
from checkpoint import atomic_save, load_checkpoint
from manifest import save_manifest

WIDTH, CONTEXT, FEATURES = 32, 8, 65536
HEADER = struct.Struct('<8sHHIHHiii')
BYTE_WEIGHTS = (FEATURES + 3) * WIDTH + CONTEXT * 5 * WIDTH
FILE_BYTES = HEADER.size + BYTE_WEIGHTS + CONTEXT * 4 + (3 * CONTEXT + WIDTH + CONTEXT) * 2
GROUPS = [max(abs(i // 15 - 7), abs(i % 15 - 7)) // 2 for i in range(225)]
COUNTS = [9, 40, 72, 104]
FORMAT = 'rustmoku-mixlite-v3-d4'

def symmetric_keys(board, side):
    return [min(k, sum(((k >> (2*i)) & 3) << (2*(7-i)) for i in range(8)))
            for k in feature_keys(board, side)]


def ste_integer(x, low, high):
    integer = x.round().clamp(low, high)
    return x + (integer - x).detach()


def ste_trunc(x):
    return x + (x.trunc() - x).detach()


def features(board, side):
    keys = torch.tensor(symmetric_keys(board, side)).reshape(225, 4)
    centers = torch.tensor([0 if c == 0 else 1 if c == side + 1 else 2 for c in board])
    return keys, centers


class MixLite(nn.Module):
    def __init__(self, score_scale=500, value_divisor=16, policy_divisor=64):
        super().__init__()
        if not 1 <= score_scale <= 10_000_000 or not 1 <= value_divisor <= 2**31-1 or not 1 <= policy_divisor <= 2**31-1:
            raise ValueError('invalid V3 scale or divisors')
        self.score_scale, self.value_divisor, self.policy_divisor = score_scale, value_divisor, policy_divisor
        self.embedding = nn.Parameter(torch.empty(FEATURES, WIDTH, dtype=torch.float64))
        self.center = nn.Parameter(torch.empty(3, WIDTH, dtype=torch.float64))
        self.mixing = nn.Parameter(torch.empty(CONTEXT, 5 * WIDTH, dtype=torch.float64))
        self.bias = nn.Parameter(torch.full((CONTEXT,), 512., dtype=torch.float64))
        self.wdl_head = nn.Parameter(torch.empty(3, CONTEXT, dtype=torch.float64))
        self.policy_head = nn.Parameter(torch.empty(WIDTH, dtype=torch.float64))
        self.policy_context = nn.Parameter(torch.empty(CONTEXT, dtype=torch.float64))
        for parameter in (self.embedding, self.center, self.mixing):
            nn.init.normal_(parameter, std=12)
        for parameter in (self.wdl_head, self.policy_head, self.policy_context):
            nn.init.uniform_(parameter, 1, 256)

    def forward(self, keys, centers):
        local = (F.embedding(keys, ste_integer(self.embedding, -128, 127)).sum(1)
                 + F.embedding(centers, ste_integer(self.center, -128, 127))).clamp(0, 255)
        groups = torch.stack([local[[i for i, group in enumerate(GROUPS) if group == g]].sum(0) for g in range(4)])
        pooled = torch.cat([ste_trunc(groups.sum(0) / 225)] + [ste_trunc(groups[g] / COUNTS[g]) for g in range(4)])
        context = ste_trunc((F.linear(pooled, ste_integer(self.mixing, -128, 127))
                             + ste_integer(self.bias, -1048576, 1048576)) / 256).clamp(0, 255)
        evidence = ste_trunc(F.linear(context, ste_integer(self.wdl_head, -32768, 32767)) / self.value_divisor).relu() + 1
        wdl = evidence / evidence.sum()
        dot = local @ ste_integer(self.policy_head, -32768, 32767)
        cross = (local[:, :CONTEXT] * context) @ ste_integer(self.policy_context, -32768, 32767)
        policy = ste_trunc((dot + ste_trunc(cross / 256)) / self.policy_divisor).clamp(-32768, 32767)
        return wdl, policy, evidence

    def bytes(self):
        tensors = ((self.embedding, 'b', -128, 127), (self.center, 'b', -128, 127),
                   (self.mixing, 'b', -128, 127), (self.bias, 'i', -1048576, 1048576),
                   (self.wdl_head, 'h', -32768, 32767), (self.policy_head, 'h', -32768, 32767),
                   (self.policy_context, 'h', -32768, 32767))
        payload = bytearray(HEADER.pack(b'RMLPV003', 3, 4, FEATURES, WIDTH, 2, self.value_divisor, self.policy_divisor, self.score_scale))
        for tensor, code, low, high in tensors:
            if not torch.isfinite(tensor).all():
                raise ValueError('nonfinite V3 tensor')
            values = array.array(code, tensor.detach().round().clamp(low, high).flatten().to(torch.int64).tolist())
            if sys.byteorder != 'little': values.byteswap()
            payload.extend(values.tobytes())
        assert len(payload) == FILE_BYTES
        return bytes(payload)


class IntegerMixLite:
    """Independent integer oracle; intentionally rebuilds all centers offline."""
    def __init__(self, payload):
        if len(payload) != FILE_BYTES:
            raise ValueError('invalid V3 tensor length')
        header = HEADER.unpack_from(payload)
        if header[:6] != (b'RMLPV003', 3, 4, FEATURES, WIDTH, 2):
            raise ValueError('invalid V3 header')
        self.value_divisor, self.policy_divisor, self.score_scale = header[6:]
        if self.value_divisor <= 0 or self.policy_divisor <= 0 or not 1 <= self.score_scale <= 10_000_000:
            raise ValueError('invalid V3 numeric contract')
        cursor = HEADER.size
        def take(code, count):
            nonlocal cursor
            values = array.array(code)
            size = values.itemsize * count
            values.frombytes(payload[cursor:cursor+size]); cursor += size
            if sys.byteorder != 'little': values.byteswap()
            return values
        self.embedding = take('b', FEATURES * WIDTH)
        self.center = take('b', 3 * WIDTH)
        self.mixing = take('b', CONTEXT * 5 * WIDTH)
        self.bias = take('i', CONTEXT)
        if any(abs(v) > 1048576 for v in self.bias): raise ValueError('invalid V3 accumulator bias')
        self.wdl_head = take('h', 3 * CONTEXT)
        self.policy_head = take('h', WIDTH)
        self.policy_context = take('h', CONTEXT)

    def value(self, board, side):
        return self.infer(board, side)[0]

    def policy(self, board, side, at):
        return self.infer(board, side)[1][at]

    def infer(self, board, side):
        keys = symmetric_keys(board, side)
        local, groups = [], [[0] * WIDTH for _ in range(4)]
        for at in range(225):
            c = 0 if board[at] == 0 else 1 if board[at] == side + 1 else 2
            row = [min(255, max(0, sum(self.embedding[k * WIDTH+d] for k in keys[at*4:at*4+4]) + self.center[c*WIDTH+d])) for d in range(WIDTH)]
            local.append(row)
            for d in range(WIDTH): groups[GROUPS[at]][d] += row[d]
        pooled = [sum(groups[g][d] for g in range(4)) // 225 for d in range(WIDTH)]
        pooled += [v // COUNTS[g] for g in range(4) for v in groups[g]]
        context = [min(255, max(0, truncating_division(sum(a*b for a,b in zip(pooled, self.mixing[c*160:(c+1)*160])) + self.bias[c], 256))) for c in range(CONTEXT)]
        evidence = [max(0, truncating_division(sum(a*b for a,b in zip(context, self.wdl_head[c*8:(c+1)*8])), self.value_divisor)) + 1 for c in range(3)]
        total = sum(evidence)
        q = truncating_division((evidence[0]-evidence[2])*32768, total)
        value = max(-10_000_000, min(10_000_000, truncating_division(self.score_scale*q, 32768-abs(q))))
        policies = []
        for row in local:
            dot = sum(a*b for a,b in zip(row, self.policy_head))
            cross = sum(row[i]*context[i]*self.policy_context[i] for i in range(CONTEXT))
            policies.append(max(-32768, min(32767, truncating_division(dot+truncating_division(cross,256), self.policy_divisor))))
        win, loss = evidence[0]*32768//total, evidence[2]*32768//total
        return value, policies, [win,32768-win-loss,loss]


def train(dataset_path, checkpoint_path, steps, seed=0, max_seconds=60, resume=None):
    if not 1 <= steps <= 1_000_000 or not math.isfinite(max_seconds) or max_seconds <= 0:
        raise ValueError('training requires explicit bounded steps/time')
    torch.set_num_threads(1); torch.manual_seed(seed)
    started = time.monotonic()
    with open_dataset(Path(dataset_path)) as dataset:
        previous = load_checkpoint(resume) if resume else None
        if previous and previous.get('format') != FORMAT: raise ValueError('V3 resume architecture mismatch')
        manifest = previous['split_manifest'] if previous else make_split_manifest(dataset, seed)
        splits = validate_split_manifest(dataset, manifest, seed)
        indices = splits['train']
        if not indices: raise ValueError('no eligible train partition')
        # Training partition only; frozen on resume, never tuned on heldout.
        sample = [abs(dataset[i].value) for i in indices[:4096] if not dataset[i].exact]
        scale = previous['score_scale'] if previous else max(1, min(10_000_000, sorted(sample)[len(sample)//2] if sample else 500))
        model = MixLite(scale)
        optimizer = torch.optim.Adam(model.parameters(), lr=.01)
        completed = 0
        if previous:
            model.load_state_dict(previous['model_state']); optimizer.load_state_dict(previous['optimizer_state']); completed = previous['steps']
        done = 0
        for step in range(steps):
            if time.monotonic() - started >= max_seconds: break
            record = dataset[indices[(completed+step) % len(indices)]]
            board, side = decode_position_key(record.position_key)
            symmetry = (seed + completed + step) % 8
            board, policy_move = transform_position(board, record.policy_move, symmetry)
            wdl, policy, _ = model(*features(board, side))
            q = float((record.value > 0)-(record.value < 0)) if record.exact else record.value/(scale+abs(record.value))
            target = torch.tensor([max(q,0),1-abs(q),max(-q,0)], dtype=torch.float64)
            loss = -(target*wdl.log()).sum() + (wdl[0]-wdl[2]-q).square()
            moves = [i for i,c in enumerate(board) if c == 0]
            if policy_move is not None:
                loss = loss + F.cross_entropy(policy[moves].unsqueeze(0), torch.tensor([moves.index(policy_move)]))
            if record.comparison is not None and not record.exact:
                from teacher import masked_loss, validate_comparison
                comparison = validate_comparison(record.comparison)
                observed = [transform_index(at, symmetry) for at in comparison['moves']]
                if any(board[at] != 0 for at in observed): raise ValueError('occupied comparison move')
                loss = loss + masked_loss(policy, observed, comparison['probabilities'])
            optimizer.zero_grad(); loss.backward(); optimizer.step()
            done += 1
        if done == 0 and not previous: raise ValueError('budget exhausted before first step')
        validate_split_manifest(dataset, manifest, seed)
        atomic_save(checkpoint_path, dict(format=FORMAT, model_state=model.state_dict(), optimizer_state=optimizer.state_dict(),
                    steps=completed+done, seed=seed, split_manifest=manifest, score_scale=scale,
                    configuration=dict(architecture='mixlite-width32-context8-d4-v3', value_contract='stm-rational-q15-v2', training='bounded-scalar-qat')))
    return completed+done


def export(checkpoint_path, dataset_path, model_path):
    from export import publish_model
    from provenance import export_identity, write_export, check_file
    from common import load_training_model
    identity = export_identity(checkpoint_path, dataset_path)
    model = load_training_model(checkpoint_path, 'cpu')
    if not isinstance(model, MixLite): raise ValueError('not a V3 checkpoint')
    payload = model.bytes(); IntegerMixLite(payload)
    check_file(identity['checkpoint']); check_file(identity['dataset'])
    publish_model(Path(model_path), payload)
    write_export(model_path, identity, dict(contract='mixlite-d4-int8-int16-v1',
        value_divisor=model.value_divisor, policy_divisor=model.policy_divisor, score_scale=model.score_scale))


def verify(engine, model_path):
    engine, model_path = Path(engine).resolve(), Path(model_path).resolve()
    before = file_hash(model_path), file_hash(engine)
    integer = IntegerMixLite(model_path.read_bytes()); checks = 0
    with tempfile.TemporaryDirectory() as directory:
        record = Path(directory)/'position.rmg'
        for moves in ('', 'H8', 'H8 I8', 'H8 I8 H9', 'A1 O15 A2 O14 B1 N15'):
            record.write_text(f'RustMoku 1\nrules=freestyle\nmoves={moves}\n', encoding='utf-8')
            board, side = parse_game_record(record); value, policy, _ = integer.infer(board, side)
            for at, backend in ((at, backend) for at in ('A15','O1','G7') for backend in ('scalar','auto')):
                result = subprocess.run([str(engine),'model-check','--model',str(model_path),'--record',str(record),'--move',at,'--backend',backend], capture_output=True,text=True,check=True,timeout=10)
                actual = dict(line.split('=',1) for line in result.stdout.splitlines())
                if actual != dict(value=str(value), policy=str(policy[parse_move(at)])): raise ValueError(f'V3 integer mismatch: {moves}, {at}: {actual}')
                checks += 1
    if before != (file_hash(model_path), file_hash(engine)): raise ValueError('verification inputs changed')
    save_manifest(Path(str(model_path)+'.v3-integer.json'), dict(model_sha256=before[0],engine_sha256=before[1], checks=checks, status='bit-exact-scalar-and-dispatch'))
    return checks


def main():
    parser = argparse.ArgumentParser(description=__doc__); sub = parser.add_subparsers(dest='command',required=True)
    t = sub.add_parser('train'); t.add_argument('--dataset',type=Path,required=True); t.add_argument('--checkpoint',type=Path,required=True)
    t.add_argument('--steps',type=int,required=True); t.add_argument('--seed',type=int,default=0); t.add_argument('--max-seconds',type=float,default=60); t.add_argument('--resume',type=Path)
    e = sub.add_parser('export')
    for flag in ('checkpoint','dataset','model'): e.add_argument('--'+flag,type=Path,required=True)
    v = sub.add_parser('verify'); v.add_argument('--engine',type=Path,required=True); v.add_argument('--model',type=Path,required=True)
    args = parser.parse_args()
    if args.command == 'train': print('steps=',train(args.dataset,args.checkpoint,args.steps,args.seed,args.max_seconds,args.resume))
    elif args.command == 'export': export(args.checkpoint,args.dataset,args.model)
    else: print('integer_checks=',verify(args.engine,args.model))


if __name__ == '__main__': main()
