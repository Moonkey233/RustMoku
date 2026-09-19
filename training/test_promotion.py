"""Synthetic outcomes exercise admission/rejection only, NEVER match evidence."""
import argparse
import contextlib
import copy
import io
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import torch
from checkpoint import atomic_save
from common import DATA_HEADER, DATA_MAGIC, DATA_RECORD_PREFIX, DatasetFile, LocalPatternModel, make_split_manifest
from dataset import file_hash
from manifest import save_manifest
from provenance import ARCHITECTURE, SCORE_CONTRACT, file_identity, object_hash, sidecar, validate_evidence
from promote import promote
import export
import calibrate
from verify_integer import verify
from integration_paths import executable

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'apps/rustmoku-arena'))
from experiment import describe, verify_events


class PromotionEvidence(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        torch.set_num_threads(1)
        cls.storage = tempfile.TemporaryDirectory(prefix='synthetic promotion only ')
        cls.root = Path(cls.storage.name)
        cls.arena = executable('rustmoku-arena')
        cls.data_engine = executable('rustmoku-data')
        cls.data = cls.root / 'training.rmd'
        payload = bytearray(DATA_HEADER.pack(DATA_MAGIC, 1, 0, 8))
        for game in range(8):
            key = bytearray(58)
            key[game] = 0x60
            payload.extend(DATA_RECORD_PREFIX.pack(game, 2, 0, 100, 100 + game, 2, 0))
            payload.extend(key)
        cls.data.write_bytes(payload)
        with DatasetFile(cls.data) as data:
            split = make_split_manifest(data, 7)
        model = LocalPatternModel()
        with torch.no_grad():
            for parameter in model.parameters():
                parameter.zero_()
        cls.checkpoint = cls.root / 'synthetic.pt'
        atomic_save(cls.checkpoint, {'format': 'rustmoku-local-pattern-v1', 'state_dict': model.state_dict(),
            'split_manifest': split, 'configuration': {'architecture': ARCHITECTURE, 'value_contract': SCORE_CONTRACT}})
        cls.model = cls.root / 'candidate.rmlp'
        with contextlib.redirect_stdout(io.StringIO()):
            with patch('sys.argv', ['export', '--checkpoint', str(cls.checkpoint), '--dataset', str(cls.data),
                                    '--output', str(cls.model)]):
                export.main()
            with patch('sys.argv', ['calibrate', '--checkpoint', str(cls.checkpoint), '--dataset', str(cls.data),
                                    '--model', str(cls.model)]):
                calibrate.main()
            verify(cls.data_engine, cls.model)
        cls.evidence = validate_evidence(cls.model)

    def test_archived_candidate_survives_original_training_input_mutation(self):
        original_checkpoint = self.checkpoint.read_bytes()
        original_data = self.data.read_bytes()
        try:
            self.checkpoint.write_bytes(b'continued training replaced the mutable checkpoint')
            self.data.write_bytes(b'original corpus is no longer present')
            evidence = validate_evidence(self.model)
            self.assertNotEqual(evidence['dataset_path'], self.data)
            with self.assertRaises(ValueError):
                validate_evidence(self.model, self.data)
        finally:
            self.checkpoint.write_bytes(original_checkpoint)
            self.data.write_bytes(original_data)

    @classmethod
    def tearDownClass(cls):
        cls.storage.cleanup()

    def fixture(self, root, extra=(), kind='model'):
        args = ['--a-model', str(self.model), '--depth', '1', '--move-ms', '1000', *extra]
        effective = describe(self.arena, args)
        configuration = {'arena': str(self.arena), 'arguments': args, 'max_pairs': 8,
            'suite_role': 'confirmation', 'stop_rule': 'fixed_pairs', 'promotion_kind': kind,
            'sprt': {'h0': 0, 'h1': 5, 'alpha': .05, 'beta': .05}}
        manifest = {'version': 2, 'configuration': configuration, 'effective': effective,
                    'inputs_sha256': {**effective['inputs_sha256'], **self.evidence['inputs_sha256']},
                    'model_evidence': {file_hash(self.model): file_identity(sidecar(self.model, 'evidence'))}}
        save_manifest(root / 'manifest.json', manifest)
        events = [{'status': 'completed', 'game_id': f'{pair}:{leg}',
                   'manifest_sha256': object_hash(manifest), 'effective_sha256': object_hash(effective),
                   'row': {'pair': str(pair + 1), 'leg': str(leg), 'a_color': 'Black' if leg == 1 else 'White',
                           'opening_key': effective['openings'][pair], 'winner': 'A'}}
                  for pair in range(8) for leg in (1, 2)]
        (root / 'events.jsonl').write_text(''.join(json.dumps(event) + '\n' for event in events), encoding='utf-8')
        return manifest, events

    def test_legacy_synthetic_override_cannot_promote(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            save_manifest(root / 'manifest.json', {'configuration': {'arguments': [
                '--a-model', str(self.model), '--a-evaluator', 'pattern']}})
            with self.assertRaisesRegex(ValueError, 'effective configuration'):
                promote(root, self.model, root / 'champion', self.data)
            self.assertFalse((root / 'champion').exists())

    def test_model_only_rejects_different_profile_but_combination_preserves_it(self):
        for kind in ('model', 'engine-model-profile'):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                self.fixture(root, ('--a-disable', 'lmr'), kind)
                if kind == 'model':
                    with self.assertRaisesRegex(ValueError, 'matching actual A/B profiles'):
                        promote(root, self.model, root / 'champion', self.data)
                    self.assertFalse((root / 'champion').exists())
                else:
                    result = promote(root, self.model, root / 'champion', self.data)
                    self.assertEqual(result['status'], 'promoted')  # SYNTHETIC fixture, not a real candidate.
                    self.assertFalse(result['competition_identity']['profile']['selectivity']['lmr'])
                    self.assertEqual(result['promotion_kind'], kind)
                    self.assertEqual(file_hash(result['model']), file_hash(self.model))
                    validate_evidence(Path(result['model']))

    def test_arbitrary_training_corpus_and_other_model_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            unrelated = root / 'unrelated.rmd'
            unrelated.write_bytes(DATA_HEADER.pack(DATA_MAGIC, 1, 0, 0))
            with self.assertRaisesRegex(ValueError, 'training corpus'):
                promote(root, self.model, root / 'champion', unrelated)
            other = root / 'other.rmlp'
            other.write_bytes(b'other model')
            with self.assertRaisesRegex(ValueError, 'actual Arena player A'):
                promote(root, other, root / 'champion', self.data)

    def test_missing_or_modified_calibration_and_split_evidence_rejects(self):
        for kind in ('calibration', 'integer', 'export'):
            path = sidecar(self.model, kind)
            original = path.read_bytes()
            try:
                value = json.loads(original)
                if kind == 'export':
                    value['split']['manifest']['seed'] += 1
                else:
                    value['model_sha256'] = '0' * 64
                path.write_text(json.dumps(value), encoding='utf-8')
                with self.assertRaises(ValueError):
                    validate_evidence(self.model)
                path.unlink()
                with self.assertRaises(FileNotFoundError):
                    validate_evidence(self.model)
            finally:
                path.write_bytes(original)

    def test_events_from_another_effective_profile_are_not_counted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest, events = self.fixture(root)
            events[0]['effective_sha256'] = '0' * 64
            with self.assertRaisesRegex(ValueError, 'different experiment'):
                verify_events({event['game_id']: event for event in events}, manifest)

    def test_actual_champion_profile_must_be_player_b_and_failure_keeps_pointer(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root, kind='engine-model-profile')
            champion = root / 'champion'
            champion.mkdir()
            pointer = champion / 'champion.json'
            save_manifest(pointer, {'competition_identity': {'model_sha256': 'not-the-opponent'}})
            before = pointer.read_bytes()
            with self.assertRaisesRegex(ValueError, 'current champion combination'):
                promote(root, self.model, champion)
            self.assertEqual(pointer.read_bytes(), before)


if __name__ == '__main__':
    unittest.main()
