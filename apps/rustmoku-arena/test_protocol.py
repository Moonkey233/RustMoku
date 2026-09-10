"""Real subprocess protocol/config tests; the fixture is not strength evidence.

Build rustmoku-arena in Release before this suite (also done in CI).
"""
import csv
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'training'))
from common import MODEL_HEADER, MODEL_MAGIC, MODEL_FEATURE_COUNT, MODEL_HIDDEN
from dataset import file_hash


class ArenaIdentityAndProtocol(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.arena = Path(os.environ.get('RUSTMOKU_ARENA', ROOT / 'target/release' /
                                       ('rustmoku-arena.exe' if os.name == 'nt' else 'rustmoku-arena')))
        if not cls.arena.is_file():
            raise RuntimeError('build --release -p rustmoku-arena before the protocol suite')
        cls.directory = tempfile.TemporaryDirectory(prefix='arena identity ')
        cls.model = Path(cls.directory.name) / 'valid model.rmlp'
        cls.model.write_bytes(MODEL_HEADER.pack(MODEL_MAGIC, 1, 1, MODEL_FEATURE_COUNT,
            MODEL_HIDDEN, 0, 27, 0, 65536, MODEL_FEATURE_COUNT * MODEL_HIDDEN, MODEL_HIDDEN, MODEL_HIDDEN)
            + bytes(2 * (MODEL_FEATURE_COUNT * MODEL_HIDDEN + 2 * MODEL_HIDDEN)))

    @classmethod
    def tearDownClass(cls):
        cls.directory.cleanup()

    def describe(self, args, success=True):
        result = subprocess.run([str(self.arena), '--describe', *args], capture_output=True,
                                text=True, timeout=15, cwd=ROOT)
        if not success:
            self.assertNotEqual(result.returncode, 0)
            return result
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_selector_order_duplicates_missing_and_invalid_models(self):
        for player in ('a', 'b'):
            model = [f'--{player}-model', str(self.model)]
            learned = [f'--{player}-evaluator', 'learned']
            first = self.describe(model + learned)
            second = self.describe(learned + model)
            self.assertEqual(first, second)
            self.assertEqual(first['players'][player == 'b']['model']['sha256'], file_hash(self.model))
            for selector in ('pattern', 'classical'):
                override = [f'--{player}-evaluator', selector]
                self.describe(model + override, False)
                self.describe(override + model, False)
            self.describe(model + model, False)
            self.describe(learned, False)
            self.describe(learned + learned, False)
            self.describe([f'--{player}-model', str(self.model.with_suffix('.missing'))], False)
        bad = Path(self.directory.name) / 'bad.rmlp'
        bad.write_bytes(b'not a model')
        self.describe(['--a-model', str(bad)], False)

    def test_effective_profiles_are_independent_and_external_files_frozen(self):
        base = self.describe(['--a-model', str(self.model), '--b-model', str(self.model)])
        changed = self.describe(['--a-model', str(self.model), '--b-model', str(self.model),
                                 '--a-disable', 'lmr', '--a-threads', '2', '--a-tt-mib', '1'])
        self.assertEqual(base['players'][1], changed['players'][1])
        self.assertNotEqual(base['players'][0], changed['players'][0])
        fixture = Path(__file__).with_name('protocol_fixture.py').resolve()
        external = ['--b-external', sys.executable, '--b-external-arg', str(fixture), '--move-ms', '1000']
        described = self.describe(external)
        self.assertEqual(described['inputs_sha256'][str(fixture)], file_hash(fixture))
        self.assertIsNone(described['players'][1]['threads'])
        self.describe(external + ['--b-threads', '1'], False)
        self.describe(external + ['--b-tt-mib', '16'], False)
        self.describe(external + ['--b-model', str(self.model)], False)

    def run_fixture(self, mode):
        args = [str(self.arena), '--depth', '1', '--pairs', '1', '--leg', '1', '--a-tt-mib', '1',
                '--move-ms', '1000', '--b-external', sys.executable, '--b-external-arg',
                str(Path(__file__).with_name('protocol_fixture.py')), '--b-external-arg', mode]
        if mode == 'clock':
            args += ['--clock-ms', '5000']
        result = subprocess.run(args, check=True, capture_output=True, text=True, timeout=15, cwd=ROOT)
        rows = list(csv.DictReader(io.StringIO(result.stdout)))
        self.assertEqual(len(rows), 1)
        return rows[0]

    def test_cr_only_handshake_and_real_match_clock(self):
        for mode in ('cr', 'clock'):
            row = self.run_fixture(mode)
            self.assertEqual(row['failure'], '', (mode, row))
            self.assertGreater(int(row['searched_moves']), 1)

    def test_error_details_and_diagnostic_flood_are_preserved(self):
        self.assertIn('ERROR fixture refusal details', self.run_fixture('error')['failure'])
        self.assertIn('protocol output flood', self.run_fixture('diagnostic-flood')['failure'])
        self.assertIn('exceeds 4096', self.run_fixture('flood')['failure'])


if __name__ == '__main__':
    unittest.main()
