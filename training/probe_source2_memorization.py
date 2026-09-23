"""Isolated source-2 V3 memorization probe; never a production checkpoint.

Both arms share samples, ordering, augmentation, initialization and loss.  The
float arm changes numerical discretization only.  No empirical result here is
an exportable evaluator or a generalization claim.
"""
import argparse
import hashlib
import json
import sqlite3
import time
import zipfile
from collections import Counter
from pathlib import Path

import numpy as np
import torch
from torch.nn import functional as F

from checkpoint import MAX_CHECKPOINT_BYTES, atomic_save, load_checkpoint
from common import validate_split_manifest
from dataset import file_hash, open_dataset
from diagnose_value import parameter_report, statistics, value_metrics
from mixlite import COUNTS, FORMAT
from mixlite_cache import FeatureCache, ROW, SCHEMA, canonical_inputs, transformed_inputs
from mixlite_production import BatchedMixLite
from train_value_only import initialize_model, make_optimizer
from manifest import read_manifest

KIND = 'rustmoku-source2-memorization-diagnostic-v1'
MILESTONES = (2000, 5000, 10000)


def digest_json(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()).hexdigest()


def digest_indices(indices):
    return hashlib.sha256(np.asarray(indices, dtype='<u8').tobytes()).hexdigest()


def selected_rows(database, split, limit):
    rows = database.execute('SELECT idx, local, key, depth, value FROM positions '
                            'WHERE split=? AND conflict=0 AND '
                            '(split=? OR NOT EXISTS (SELECT 1 FROM positions AS train '
                            'WHERE train.split=? AND train.key=positions.key)) '
                            'ORDER BY rank, key LIMIT ?', (split, 'train', 'train', limit)).fetchall()
    if len(rows) != limit:
        raise ValueError(f'{split} has only {len(rows)} unambiguous unique source-2 positions')
    return [dict(index=i, cache_local=local if split == 'train' else None,
                 position_key=key.hex(), completed_depth=depth, raw_value=value)
            for i, local, key, depth, value in rows]


def _select_corpus(database, dataset, partitions, seed, train_count, heldout_count):
    """Disk-backed distinct-key sample, excluding contradictory raw targets."""
    database.execute('CREATE TABLE positions(split TEXT, key BLOB, idx INTEGER, local INTEGER, '
                     'rank BLOB, depth INTEGER, value INTEGER, conflict INTEGER DEFAULT 0, '
                     'PRIMARY KEY(split,key)) WITHOUT ROWID')
    counts = {}
    for split in ('train', 'opening_heldout'):
        seen = 0
        for local, i in enumerate(partitions[split]):
            record = dataset[i]
            if record.source != 2 or record.exact:
                continue
            seen += 1
            rank = hashlib.sha256(seed.to_bytes(8, 'little') + record.position_key).digest()
            database.execute('INSERT INTO positions(split,key,idx,local,rank,depth,value) '
                             'VALUES(?,?,?,?,?,?,?) ON CONFLICT(split,key) DO UPDATE SET '
                             'conflict=max(conflict,excluded.value != positions.value)',
                             (split, record.position_key, i, local, rank, record.completed_depth, record.value))
        counts[split] = seen
    train = selected_rows(database, 'train', train_count)
    heldout = selected_rows(database, 'opening_heldout', heldout_count)
    if len({r['position_key'] for r in train + heldout}) != train_count + heldout_count:
        raise ValueError('train/heldout position overlap in diagnostic selection')
    samples = dict(schema=1, seed=seed, rule='sha256(seed-le64 || canonical-key), first qualified occurrence; conflicting raw labels excluded',
                   source=2, exact=False, counts=counts, train=train, opening_heldout=heldout)
    for split in ('train', 'opening_heldout'):
        samples[f'{split}_indices_sha256'] = digest_indices([r['index'] for r in samples[split]])
        samples[f'{split}_completed_depth'] = dict(sorted(Counter(str(r['completed_depth']) for r in samples[split]).items()))
    samples['selected_sha256'] = digest_json(samples)
    return samples


class FloatRelaxedMixLite(BatchedMixLite):
    """The V3 learned tensors and information flow, with discretization removed."""
    def value_path(self, keys, centers):
        local = (F.embedding(keys, self.embedding).sum(-2) + F.embedding(centers, self.center)).clamp(0, 255)
        groups = torch.stack([local.index_select(1, getattr(self, f'group_{g}')).sum(1)
                              for g in range(4)], dim=1)
        pooled = torch.cat([groups.sum(1) / 225] + [groups[:, g] / COUNTS[g] for g in range(4)], dim=-1)
        context = ((F.linear(pooled, self.mixing) + self.bias) / 256).clamp(0, 255)
        raw = F.linear(context, self.wdl_head) / self.value_divisor
        return local, context, raw

    def forward(self, keys, centers):
        local, context, raw = self.value_path(keys, centers)
        evidence = raw.relu() + 1
        wdl = evidence / evidence.sum(-1, keepdim=True)
        dot = local @ self.policy_head
        cross = (local[:, :, :8] * context[:, None, :]) @ self.policy_context
        policy = ((dot + cross / 256) / self.policy_divisor).clamp(-32768, 32767)
        return wdl, policy, evidence


def model_for(arm, scale, seed, device):
    initial = initialize_model(scale, seed, 'small-positive')
    if arm == 'qat':
        return initial.to(device)
    if arm == 'float-relaxed':
        model = FloatRelaxedMixLite(scale).float()
        model.load_state_dict(initial.state_dict())
        return model.to(device)
    raise ValueError('unknown diagnostic arm')


def validate_topology(qat, relaxed):
    a, b = qat.state_dict(), relaxed.state_dict()
    if a.keys() != b.keys() or any(not torch.equal(a[k].cpu(), b[k].cpu()) for k in a):
        raise ValueError('diagnostic arms do not have identical initial tensors/topology')


def step_plan(indices, seed, epoch, cursor, batch_size):
    order = torch.randperm(len(indices), generator=torch.Generator().manual_seed(seed + epoch)).tolist()
    local = order[cursor:cursor + batch_size]
    actual = [indices[i] for i in local]
    symmetries = [(seed + epoch + i) % 8 for i in actual]
    return local, actual, symmetries


def raw_targets(rows, scale):
    q = np.asarray([r['raw_value'] / (scale + abs(r['raw_value'])) for r in rows], dtype=np.float32)
    wdl = np.stack([np.maximum(q, 0), 1 - np.abs(q), np.maximum(-q, 0)], axis=1)
    return q, wdl


def diagnostic_loss(wdl, q, target_wdl, sample_weight):
    return ((-(target_wdl * wdl.log()).sum(1) + (wdl[:, 0] - wdl[:, 2] - q).square()) * sample_weight).mean()


def validate_samples(samples, input_hashes):
    if (samples['input_hashes'] != input_hashes or samples['seed'] != 17 or
            len(samples['train']) != 4096 or len(samples['opening_heldout']) != 2048):
        raise ValueError('frozen sample/input identity mismatch')
    if digest_json({k: v for k, v in samples.items() if k not in ('selected_sha256', 'input_hashes')}) != samples['selected_sha256']:
        raise ValueError('frozen sample content digest mismatch')
    if any(digest_indices([r['index'] for r in samples[split]]) != samples[f'{split}_indices_sha256']
           for split in ('train', 'opening_heldout')):
        raise ValueError('frozen sample indices digest mismatch')
    if len({r['position_key'] for split in ('train', 'opening_heldout') for r in samples[split]}) != 6144:
        raise ValueError('diagnostic sample position overlap')


def verify_frozen_shards(dataset_path):
    """Check raw inputs against the already frozen descriptor, without reopening games."""
    descriptor = read_manifest(dataset_path)
    count = 0
    for shard in descriptor['shards']:
        path = Path(shard['path'])
        if not path.is_absolute(): path = dataset_path.parent / path
        if file_hash(path) != shard['sha256']:
            raise ValueError(f'frozen diagnostic shard changed: {path}')
        count += 1
    companion = descriptor.get('comparisons')
    if companion is not None and file_hash(companion['path']) != companion['sha256']:
        raise ValueError('frozen diagnostic comparison sidecar changed')
    return dict(verified_shards=count, verified_comparison_sidecar=companion is not None)


def safe_load_diagnostic(path):
    path = Path(path)
    if path.stat().st_size > MAX_CHECKPOINT_BYTES or not zipfile.is_zipfile(path):
        raise ValueError('invalid or oversized diagnostic checkpoint')
    with zipfile.ZipFile(path) as archive:
        if len(archive.infolist()) > 2048 or sum(x.file_size for x in archive.infolist()) > MAX_CHECKPOINT_BYTES:
            raise ValueError('diagnostic checkpoint expanded storage exceeds limit')
    state = torch.load(path, map_location='cpu', weights_only=True)
    if not isinstance(state, dict) or state.get('format') != KIND:
        raise ValueError('not a source-2 memorization diagnostic')
    return state


def make_input_bank(rows):
    keys, centers = canonical_inputs([bytes.fromhex(r['position_key']) for r in rows])
    return keys, centers


class FrozenResumeCache:
    """Read only a previously admitted full-train cache after an interruption."""
    def __init__(self, path, split_manifest, selected):
        path = Path(path)
        receipt = read_manifest(path / 'manifest.json')
        identity = receipt['identity']
        split_sha = hashlib.sha256(json.dumps(split_manifest, sort_keys=True).encode()).hexdigest()
        if (identity['schema'] != SCHEMA or identity['split_sha256'] != split_sha or
                identity['dataset_sha256'] != split_manifest['dataset_sha256'] or
                identity['row_bytes'] != ROW.itemsize or
                max(r['cache_local'] for r in selected) >= identity['count']):
            raise ValueError('frozen resume feature-cache identity mismatch')
        payload = path / 'features.bin'
        if payload.stat().st_size != identity['bytes'] or file_hash(payload) != receipt['sha256']:
            raise ValueError('frozen resume feature-cache content mismatch')
        self.rows = np.memmap(payload, dtype=ROW, mode='r', shape=(identity['count'],))

    def batch(self, locals_, symmetries):
        rows = self.rows[np.asarray(locals_, dtype=np.int64)]
        return transformed_inputs(rows['keys'], rows['centers'], symmetries)

    def close(self):
        self.rows._mmap.close()


def _raw_evidence(model, arm, keys, centers):
    if arm == 'float-relaxed':
        return model.value_path(keys, centers)[2]
    from diagnose_value import pre_relu_evidence
    return pre_relu_evidence(model, keys, centers)


def evaluate(model, arm, rows, bank, scale, device, batch_size=256):
    target_q, _ = raw_targets(rows, scale)
    predictions, probabilities, evidence, raw = [], [], [], []
    model.eval()
    with torch.no_grad():
        for start in range(0, len(rows), batch_size):
            end = min(start + batch_size, len(rows))
            k = torch.from_numpy(bank[0][start:end].astype(np.int64)).to(device)
            c = torch.from_numpy(bank[1][start:end].astype(np.int64)).to(device)
            w, _, e = model(k, c)
            r = _raw_evidence(model, arm, k, c)
            if not torch.allclose(r.relu() + 1, e, atol=1e-6 if arm == 'float-relaxed' else 0, rtol=0):
                raise ValueError('evidence instrument disagrees with forward')
            predictions.extend((w[:, 0] - w[:, 2]).cpu().tolist())
            probabilities.extend(w.cpu().tolist()); evidence.extend(e.cpu().tolist()); raw.extend(r.cpu().tolist())
    model.train()
    v = np.asarray(predictions); p = np.asarray(probabilities); e = np.asarray(evidence); raw = np.asarray(raw)
    strata = {'all': value_metrics(target_q, v), 'source-2': value_metrics(target_q, v)}
    for depth in sorted({r['completed_depth'] for r in rows}):
        mask = np.asarray([r['completed_depth'] == depth for r in rows])
        strata[f'depth-{depth}'] = value_metrics(target_q[mask], v[mask])
    names = ('win', 'draw', 'loss')
    return dict(strata=strata,
                wdl={name: statistics(p[:, j]) for j, name in enumerate(names)},
                evidence={name: statistics(e[:, j]) for j, name in enumerate(names)},
                pre_relu={name: dict(statistics=statistics(raw[:, j]), percent_nonpositive=float((raw[:, j] <= 0).mean() * 100))
                          for j, name in enumerate(names)},
                probability_concentration={str(x): float((p.max(1) > x).mean()) for x in (.8, .9, .95)})


def gradients(model, arm, bank, rows, scale, device):
    q, target = raw_targets(rows[:256], scale)
    k = torch.from_numpy(bank[0][:len(q)].astype(np.int64)).to(device)
    c = torch.from_numpy(bank[1][:len(q)].astype(np.int64)).to(device)
    w, _, _ = model(k, c)
    loss = diagnostic_loss(w, torch.from_numpy(q).to(device), torch.from_numpy(target).to(device),
                           torch.ones(len(q), device=device))
    grads = torch.autograd.grad(loss, tuple(model.parameters()), allow_unused=True)
    return {name: float(grad.norm()) if grad is not None else 0.
            for (name, _), grad in zip(model.named_parameters(), grads, strict=True)}


def run_arm(*, arm, output, samples, dataset, cache, template, device, resume=False,
            stop_at=10000, report_hook=None):
    output = Path(output); output.mkdir(parents=True, exist_ok=True)
    config = template['production']; scale = template['score_scale']; seed = 17; batch_size = 256
    identity = dict(kind=KIND, arm=arm, samples_sha256=samples['selected_sha256'],
                    dataset_sha256=template['split_manifest']['dataset_sha256'],
                    split_sha256=digest_json(template['split_manifest']), score_scale=scale,
                    seed=seed, batch_size=batch_size, learning_rate=.001, head_lr_multiplier=16,
                    milestones=list(MILESTONES), objective='teacher-wdl-ce+q-squared; sample-weight only; no online mining')
    manifest_path = output / 'experiment.json'; latest = output / 'latest.pt'
    if manifest_path.exists():
        if not resume or json.loads(manifest_path.read_text()) != identity:
            raise ValueError('diagnostic resume identity mismatch or output already exists')
    else:
        if resume: raise ValueError('missing diagnostic resume manifest')
        manifest_path.write_text(json.dumps(identity, indent=2) + '\n', encoding='utf-8')
    model = model_for(arm, scale, seed, device)
    optimizer = make_optimizer(model, .001, 16)
    train = samples['train']; indices = [r['index'] for r in train]; locals_ = [r['cache_local'] for r in train]
    q_np, wdl_np = raw_targets(train, scale)
    # Serious Gen0 has no composition controls.  The resume path verifies this
    # from the frozen descriptor and needs no repeat scan of 625 shards.
    sample_np = np.asarray([dataset[i].sample_weight if dataset is not None else 1.
                            for i in indices], dtype=np.float32)
    q = torch.from_numpy(q_np).to(device); target = torch.from_numpy(wdl_np).to(device)
    sample_weight = torch.from_numpy(sample_np).to(device)
    banks = dict(train=make_input_bank(train), opening_heldout=make_input_bank(samples['opening_heldout']))
    done = epoch = cursor = 0; elapsed = 0.; sequence = hashlib.sha256(b'source2-memorization-sequence-v1').hexdigest()
    if resume:
        state = safe_load_diagnostic(latest)
        if state['identity'] != identity: raise ValueError('diagnostic checkpoint identity mismatch')
        model.load_state_dict(state['model_state']); optimizer.load_state_dict(state['optimizer_state'])
        done, epoch, cursor = state['steps'], state['epoch'], state['cursor']
        elapsed, sequence = state['elapsed_seconds'], state['training_sequence_sha256']
        if not 0 <= done <= 10000 or not 0 <= cursor < len(indices) or epoch < 0:
            raise ValueError('invalid diagnostic resume cursor')
    start_time = time.perf_counter(); final_loss = None
    while done < min(stop_at, MILESTONES[-1]):
        local, actual, symmetry = step_plan(indices, seed, epoch, cursor, batch_size)
        cache_positions = [locals_[i] for i in local]
        keys_np, centers_np = cache.batch(cache_positions, symmetry)
        keys = torch.from_numpy(keys_np).to(device); centers = torch.from_numpy(centers_np).to(device)
        selected = torch.as_tensor(local, dtype=torch.long, device=device)
        wdl, _, _ = model(keys, centers)
        loss = diagnostic_loss(wdl, q[selected], target[selected], sample_weight[selected])
        if not torch.isfinite(loss): raise FloatingPointError(f'{arm} nonfinite objective at step {done+1}')
        optimizer.zero_grad(set_to_none=True); loss.backward()
        if any(p.grad is not None and not torch.isfinite(p.grad).all() for p in model.parameters()):
            raise FloatingPointError(f'{arm} nonfinite gradient at step {done+1}')
        optimizer.step()
        sequence = hashlib.sha256(bytes.fromhex(sequence) + json.dumps([actual, symmetry], separators=(',', ':')).encode()).hexdigest()
        final_loss = float(loss.detach()); done += 1; cursor += len(local)
        if cursor >= len(indices): epoch += 1; cursor = 0
        if done % 100 == 0 or done in MILESTONES or done == stop_at:
            state = dict(format=KIND, identity=identity, model_state=model.state_dict(), optimizer_state=optimizer.state_dict(),
                         steps=done, epoch=epoch, cursor=cursor, elapsed_seconds=elapsed + time.perf_counter()-start_time,
                         final_objective=final_loss, nonfinite_events=0, training_sequence_sha256=sequence)
            atomic_save(latest, state)
            if done % 500 == 0: print(json.dumps(dict(arm=arm, step=done, loss=final_loss,
                elapsed_seconds=state['elapsed_seconds'])), flush=True)
            if done in MILESTONES:
                at = output / f'step-{done}.pt'; atomic_save(at, state)
                report = dict(schema=1, identity=identity, checkpoint_sha256=file_hash(at), step=done,
                              wall_seconds=state['elapsed_seconds'], final_objective=final_loss,
                              nonfinite_events=0, training_sequence_sha256=sequence,
                              parameter_statistics=parameter_report(model, seed, initial=model_for(arm, scale, seed, 'cpu')),
                              gradient_norms=gradients(model, arm, banks['train'], train, scale, device),
                              splits={name:evaluate(model, arm, samples[name], bank, scale, device)
                                      for name, bank in banks.items()})
                path = output / f'step-{done}.json'; path.write_text(json.dumps(report, indent=2, allow_nan=False) + '\n', encoding='utf-8')
                if report_hook: report_hook(arm, done, report)
                print(json.dumps(dict(arm=arm, milestone=done,
                    train=report['splits']['train']['strata']['all'],
                    heldout=report['splits']['opening_heldout']['strata']['all'])), flush=True)
    return done


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--dataset', type=Path, default=Path('datasets/v3-serious-gen0/dataset.json'))
    p.add_argument('--reference', type=Path, default=Path('runs/v3-serious-gen0/checkpoint.pt'))
    p.add_argument('--cache', type=Path, default=Path('runs/v3-serious-gen0/feature-cache'))
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--device', default='cuda')
    p.add_argument('--resume', action='store_true')
    p.add_argument('--stop-at', type=int, default=10000, help='bounded interruption fixture; normal run is 10000')
    args = p.parse_args()
    if args.device.startswith('cuda') and not torch.cuda.is_available(): raise ValueError('CUDA unavailable')
    if not 1 <= args.stop_at <= 10000: raise ValueError('stop-at outside diagnostic budget')
    torch.set_num_threads(1)
    template = load_checkpoint(args.reference)
    if template.get('format') != FORMAT or not template.get('production'): raise ValueError('frozen reference must be production V3')
    config = template['production']
    if (config.get('seed'), config.get('batch_size'), config.get('learning_rate')) != (17, 256, .001):
        raise ValueError('frozen Serious configuration must use seed 17, batch 256, LR .001')
    before = dict(dataset_sha256=file_hash(args.dataset), checkpoint_sha256=file_hash(args.reference))
    args.output.mkdir(parents=True, exist_ok=True)
    samples_path = args.output / 'samples.json'
    if args.resume:
        if not samples_path.is_file(): raise ValueError('missing frozen sample manifest')
        samples = json.loads(samples_path.read_text())
        validate_samples(samples, before)
        descriptor = read_manifest(args.dataset)
        if descriptor.get('composition'):
            raise ValueError('lightweight diagnostic resume requires unweighted Serious Gen0 data')
        cache = FrozenResumeCache(args.cache, template['split_manifest'], samples['train'])
        try:
            validate_topology(model_for('qat', template['score_scale'], 17, 'cpu'),
                              model_for('float-relaxed', template['score_scale'], 17, 'cpu'))
            for arm in ('qat', 'float-relaxed'):
                arm_output = args.output / arm
                if (arm_output / 'experiment.json').exists() and (arm_output / 'step-10000.json').exists():
                    continue
                run_arm(arm=arm, output=arm_output, samples=samples, dataset=None,
                        cache=cache, template=template, device=args.device,
                        resume=(arm_output / 'experiment.json').exists(), stop_at=args.stop_at)
        finally: cache.close()
        after = dict(dataset_sha256=file_hash(args.dataset), checkpoint_sha256=file_hash(args.reference))
        if after != before: raise ValueError('original dataset/checkpoint changed during diagnostic')
        receipt = dict(before=before, after=after, unchanged=True, **verify_frozen_shards(args.dataset))
        (args.output / 'input-hashes.json').write_text(json.dumps(receipt, indent=2) + '\n')
        return
    with open_dataset(args.dataset) as dataset:
        partitions = validate_split_manifest(dataset, template['split_manifest'], template['production']['seed'])
        if not samples_path.exists():
            if args.resume: raise ValueError('missing frozen sample manifest')
            database = sqlite3.connect(args.output / 'selection.sqlite')
            try:
                database.execute('PRAGMA cache_size=-8192')
                database.execute('PRAGMA temp_store=FILE')
                database.execute('PRAGMA max_page_count=262144')
                samples = _select_corpus(database, dataset, partitions, 17, 4096, 2048)
            finally: database.close()
            samples['input_hashes'] = before
            samples_path.write_text(json.dumps(samples, indent=2) + '\n', encoding='utf-8')
        samples = json.loads(samples_path.read_text())
        validate_samples(samples, before)
        cache = FeatureCache(args.cache, dataset, template['split_manifest'], partitions['train'])
        try:
            validate_topology(model_for('qat', template['score_scale'], 17, 'cpu'),
                              model_for('float-relaxed', template['score_scale'], 17, 'cpu'))
            for arm in ('qat', 'float-relaxed'):
                arm_output = args.output / arm
                run_arm(arm=arm, output=arm_output, samples=samples, dataset=dataset,
                        cache=cache, template=template, device=args.device,
                        resume=args.resume and (arm_output / 'experiment.json').exists(),
                        stop_at=args.stop_at)
        finally: cache.close()
    after = dict(dataset_sha256=file_hash(args.dataset), checkpoint_sha256=file_hash(args.reference))
    if after != before: raise ValueError('original dataset/checkpoint changed during diagnostic')
    receipt = dict(before=before, after=after, unchanged=True, **verify_frozen_shards(args.dataset))
    (args.output / 'input-hashes.json').write_text(json.dumps(receipt, indent=2) + '\n')


if __name__ == '__main__': main()
