import argparse
import tempfile
import unittest
from pathlib import Path

import torch

from checkpoint import load_checkpoint
from common import DATA_HEADER, DATA_MAGIC, DATA_RECORD_PREFIX
from train import train


class TrainingResume(unittest.TestCase):
    def test_cpu_interruption_is_identical_to_uninterrupted_training(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            dataset = root / 'fixture.rmd'
            payload = bytearray(DATA_HEADER.pack(DATA_MAGIC, 1, 0, 8))
            for game in range(8):
                key = bytearray(58)
                key[game] = 0x60  # Black then White, Black to move.
                payload.extend(DATA_RECORD_PREFIX.pack(game, 2, 0, 100, 100 + game, 2, 0))
                payload.extend(key)
            dataset.write_bytes(payload)
            args = argparse.Namespace(dataset=dataset, output=root / 'full.pt', epochs=2,
                                      batch_size=2, learning_rate=.001, policy_weight=1., exact_weight=4.,
                                      seed=7, device='cpu', resume=None, max_steps=None,
                                      checkpoint_every=2, torch_threads=1, max_proof_fraction=.25)
            full = train(args)
            args.output = root / 'resumed.pt'
            args.max_steps = 2
            interrupted = train(args)
            self.assertEqual(interrupted['step'], 2)
            self.assertGreater(interrupted['cursor'], 0)
            args.resume = args.output
            args.max_steps = None
            resumed = train(args)
            self.assertEqual(full['step'], resumed['step'])
            self.assertEqual(full['epoch'], resumed['epoch'])
            for key in full['state_dict']:
                self.assertTrue(torch.equal(full['state_dict'][key], resumed['state_dict'][key]), key)
            for parameter, state in full['optimizer']['state'].items():
                for key, value in state.items():
                    self.assertTrue(torch.equal(value, resumed['optimizer']['state'][parameter][key]))
            self.assertTrue(torch.equal(full['shuffle_rng'], resumed['shuffle_rng']))
            args.learning_rate = .002
            with self.assertRaisesRegex(ValueError, 'configuration'):
                train(args)
            saved = args.output.read_bytes()
            args.output.write_bytes(saved[:100])
            with self.assertRaises(Exception):
                load_checkpoint(args.output)


if __name__ == '__main__':
    unittest.main()
