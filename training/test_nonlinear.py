import argparse
import array
import dataclasses
import tempfile
import unittest
from pathlib import Path

import torch

from common import read_quantized_model
from nonlinear_model import (HEADER, WEIGHTS, MAX_HEAD_SUM, QuantizedNonlinear, NonlinearModel,
                             export_integer, read_integer)
from test_compact import fixture
from train import train


class NonlinearTests(unittest.TestCase):
    def test_integer_header_and_extreme_bounds(self):
        payload = array.array('h', [-32768, 32767] * (WEIGHTS // 2)).tobytes()
        for bias in (-(2**63 - 1 - MAX_HEAD_SUM), 2**63 - 1 - MAX_HEAD_SUM, -1, 0):
            header = HEADER.pack(b'RMLPV002', 2, 2, 65536, 8, 2, 1, 1, 1000, bias, 0)
            integer = read_integer(header + payload)
            self.assertLessEqual(abs(integer.value([0] * 225, 0)), 10_000_000)
            self.assertLessEqual(abs(integer.policy([0] * 225, 1, 0)), 32768)
        for field, value in ((1, 1), (2, 1), (3, 65535), (4, 16), (5, 1), (6, 0), (7, -1), (8, 0), (9, -2**63), (10, 1)):
            fields = [b'RMLPV002', 2, 2, 65536, 8, 2, 1, 1, 1000, 0, 0]
            fields[field] = value
            with self.assertRaises(ValueError): read_integer(HEADER.pack(*fields) + payload)

    def test_center_is_included_and_negative_division_truncates(self):
        model = QuantizedNonlinear(array.array('h', [0]) * (65536 * 8),
                                  (0,) * 8 + (2,) * 8 + (-2,) * 8, (-1,) * 8, (1,) * 8,
                                  -1, 1, 1, 1000)
        board = [0] * 225
        self.assertEqual(model.normalized(board, 0), 0)
        for i in range(20): board[i] = 1
        self.assertEqual(model.normalized(board, 0), -1)
        self.assertEqual(model.normalized(board, 1), 0)

    def test_v2_cpu_resume_parameters_optimizer_selection(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            raw = root / 'data.rmd'
            fixture(raw, games=10, plies=3)
            args = argparse.Namespace(dataset=raw, output=root/'full.pt', epochs=2, batch_size=4,
                learning_rate=.001, policy_weight=1., exact_weight=4., seed=7, device='cpu',
                resume=None, max_steps=None, checkpoint_every=2, torch_threads=1, max_proof_fraction=.25,
                architecture='v2', qat=False, outcome_weight=.25, early_stop_patience=5)
            full = train(args)
            args.output, args.max_steps = root/'resume.pt', 2
            train(args)
            args.resume, args.max_steps = args.output, None
            resumed = train(args)
            for group in ('state_dict', 'selected_state_dict'):
                for name in full[group]: self.assertTrue(torch.equal(full[group][name], resumed[group][name]), name)
            self.assertEqual(full['best_validation_loss'], resumed['best_validation_loss'])
            self.assertEqual(full['configuration']['score_scale'], 525)
            self.assertEqual(full['history'], resumed['history'])
            model = NonlinearModel()
            model.load_state_dict(full['selected_state_dict'])
            scales = argparse.Namespace(embedding_scale=16384, value_head_scale=16384, policy_head_scale=16384)
            payload, _ = export_integer(model, scales, full['configuration']['score_scale'])
            path = root/'model.rmlp'
            path.write_bytes(payload)
            self.assertIsInstance(read_quantized_model(path), QuantizedNonlinear)


if __name__ == '__main__': unittest.main()
