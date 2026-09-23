import json
import sqlite3
import contextlib
import tempfile
import unittest
from pathlib import Path

import numpy as np
import torch

from checkpoint import load_checkpoint
from common import DataRecord, load_training_model
from mixlite_cache import canonical_inputs, transformed_inputs
from probe_source2_memorization import (
    FloatRelaxedMixLite, _select_corpus, digest_json, model_for, run_arm,
    safe_load_diagnostic, step_plan, validate_topology,
)
from test_mixlite_hotpath import key
from train_value_only import initialize_model


def position(cell):
    board = [0] * 225
    board[cell] = 1
    return key(board, 1)


class TinyCache:
    def __init__(self, records):
        self.keys, self.centers = canonical_inputs([r.position_key for r in records])

    def batch(self, locals_, symmetries):
        return transformed_inputs(self.keys[locals_], self.centers[locals_], symmetries)


class Source2MemorizationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        torch.set_num_threads(1)

    def test_disk_selection_unique_conflict_filtered_and_split_isolated(self):
        records = [DataRecord(i, 1, 0, 20, value, 2, exact, position(cell), completed_depth=depth)
                   for i, (cell, value, depth, exact) in enumerate([
                       (10, 20, 3, False), (10, 20, 3, False),
                       (11, 30, 3, False), (11, 40, 4, False),
                       (12, 50, 3, False), (13, 60, 3, False),
                       (14, 70, 3, True),
                       (12, 50, 3, False), (15, 80, 4, False), (16, 90, 4, False)])]
        partitions = {'train': range(7), 'opening_heldout': range(7, 10)}
        with tempfile.TemporaryDirectory() as raw, contextlib.closing(sqlite3.connect(Path(raw) / 'selection.sqlite')) as database:
            samples = _select_corpus(database, records, partitions, 17, 2, 2)
            self.assertEqual(len(samples['train']), 2)
            self.assertEqual(len(samples['opening_heldout']), 2)
            self.assertNotIn(position(11).hex(), [r['position_key'] for r in samples['train']])
            self.assertNotIn(position(12).hex(), [r['position_key'] for r in samples['opening_heldout']])
            self.assertEqual(len({r['position_key'] for name in ('train', 'opening_heldout') for r in samples[name]}), 4)
            with contextlib.closing(sqlite3.connect(Path(raw) / 'second.sqlite')) as repeated:
                self.assertEqual(samples, _select_corpus(repeated, records, partitions, 17, 2, 2))

    def test_same_initial_tensors_topology_and_d4_sequence(self):
        qat = model_for('qat', 500, 17, 'cpu')
        relaxed = model_for('float-relaxed', 500, 17, 'cpu')
        validate_topology(qat, relaxed)
        self.assertIsInstance(relaxed, FloatRelaxedMixLite)
        self.assertEqual({name: tuple(p.shape) for name, p in qat.named_parameters()},
                         {name: tuple(p.shape) for name, p in relaxed.named_parameters()})
        self.assertEqual(step_plan([13, 17, 29, 31], 17, 4, 0, 3),
                         step_plan([13, 17, 29, 31], 17, 4, 0, 3))
        keys, centers = canonical_inputs([position(0), position(112)])
        k = torch.from_numpy(keys.astype(np.int64)); c = torch.from_numpy(centers.astype(np.int64))
        w0, p0, e0 = qat(k, c); w1, p1, e1 = relaxed(k, c)
        reference = initialize_model(500, 17, 'small-positive')
        for actual, expected in zip((w0, p0, e0), reference(k, c), strict=True):
            self.assertTrue(torch.equal(actual, expected))
        self.assertEqual(w0.shape, w1.shape)
        self.assertEqual(p0.shape, p1.shape)
        self.assertEqual(e0.shape, e1.shape)
        self.assertFalse(torch.equal(w0, w1))
        self.assertEqual(tuple(qat.state_dict()), tuple(relaxed.state_dict()))

    def test_resume_matches_continuous_and_checkpoint_is_not_exportable(self):
        records = [DataRecord(i, 1, 0, 20, 10 + i * 7, 2, False, position(30 + i),
                              completed_depth=2 + i % 2) for i in range(4)]
        train = [dict(index=i, cache_local=i, position_key=r.position_key.hex(),
                      completed_depth=r.completed_depth, raw_value=r.value) for i, r in enumerate(records)]
        heldout = [dict(index=i, cache_local=None, position_key=r.position_key.hex(),
                        completed_depth=r.completed_depth, raw_value=r.value) for i, r in enumerate(records[:2])]
        samples = dict(train=train, opening_heldout=heldout, selected_sha256=digest_json([train, heldout]))
        template = dict(score_scale=500, split_manifest=dict(dataset_sha256='fixture'), production={})
        cache = TinyCache(records)
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            for arm in ('qat', 'float-relaxed'):
                continuous = root / arm / 'continuous'
                interrupted = root / arm / 'interrupted'
                run_arm(arm=arm, output=continuous, samples=samples, dataset=records,
                        cache=cache, template=template, device='cpu', stop_at=2)
                run_arm(arm=arm, output=interrupted, samples=samples, dataset=records,
                        cache=cache, template=template, device='cpu', stop_at=1)
                run_arm(arm=arm, output=interrupted, samples=samples, dataset=records,
                        cache=cache, template=template, device='cpu', resume=True, stop_at=2)
                a = safe_load_diagnostic(continuous / 'latest.pt')
                b = safe_load_diagnostic(interrupted / 'latest.pt')
                self.assertEqual(a['training_sequence_sha256'], b['training_sequence_sha256'])
                self.assertEqual(a['epoch'], b['epoch'])
                self.assertEqual(a['cursor'], b['cursor'])
                for name, tensor in a['model_state'].items():
                    self.assertTrue(torch.equal(tensor, b['model_state'][name]), name)
                with self.assertRaisesRegex(ValueError, 'unsupported training checkpoint'):
                    load_checkpoint(continuous / 'latest.pt')
                with self.assertRaisesRegex(ValueError, 'unsupported training checkpoint'):
                    load_training_model(continuous / 'latest.pt', 'cpu')
                with self.assertRaisesRegex(ValueError, 'identity mismatch'):
                    changed = dict(samples, selected_sha256='different')
                    run_arm(arm=arm, output=interrupted, samples=changed, dataset=records,
                            cache=cache, template=template, device='cpu', resume=True, stop_at=2)


if __name__ == '__main__': unittest.main()
