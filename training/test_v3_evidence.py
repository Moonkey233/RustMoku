"""Synthetic evidence admission, never a playing-strength experiment."""
import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
import torch
from mixlite_production import train
from mixlite import export
from provenance import validate_evidence, sidecar, seal
from promote import promote
import test_promotion
ROOT = test_promotion.ROOT
from test_compact import fixture
import calibrate
from verify_integer import verify

class V3Evidence(unittest.TestCase):
    fixture = test_promotion.PromotionEvidence.fixture
    @classmethod
    def setUpClass(cls):
        torch.set_num_threads(1)
        cls.storage=tempfile.TemporaryDirectory(prefix='synthetic-v3-evidence-')
        cls.root=Path(cls.storage.name)
        suffix='.exe' if os.name=='nt' else ''
        cls.arena=ROOT/f'target/debug/rustmoku-arena{suffix}'
        cls.engine=ROOT/f'target/debug/rustmoku-data{suffix}'
        cls.data=cls.root/'data.rmd'; cls.cp=cls.root/'v3.pt'; cls.model=cls.root/'v3.bin'
        fixture(cls.data)
        train(cls.data,cls.cp,steps=1,epochs=2,batch_size=2)
        export(cls.cp,cls.data,cls.model)
        with contextlib.redirect_stdout(io.StringIO()):
            with patch('sys.argv',['calibrate','--dataset',str(cls.data),'--checkpoint',str(cls.cp),'--model',str(cls.model)]):
                calibrate.main()
            verify(cls.engine,cls.model)
        cls.evidence=validate_evidence(cls.model)
    @classmethod
    def tearDownClass(cls): cls.storage.cleanup()
    def test_v3_common_freeze_and_synthetic_promotion(self):
        self.assertEqual(self.evidence['export']['architecture']['architecture_id'],4)
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            self.fixture(root,kind='engine-model-profile')
            result=promote(root,self.model,root/'champion',self.data)
            self.assertEqual(result['status'],'promoted')  # SYNTHETIC, not champion evidence.
            validate_evidence(Path(result['model']))
    def test_portable_receipt_is_valid_but_explicit_avx2_target_requires_receipt(self):
        validate_evidence(self.model)
        with self.assertRaises(ValueError):
            validate_evidence(self.model,target_backend_policy='avx2')
        receipt=sidecar(self.model,'integer'); evidence=sidecar(self.model,'evidence')
        original=receipt.read_bytes(); original_evidence=evidence.read_bytes()
        try:
            value=json.loads(original); value['report']['backends']=['scalar']
            receipt.write_text(json.dumps(value),encoding='utf-8'); evidence.unlink(); seal(self.model)
            with self.assertRaisesRegex(ValueError,'portable backend evidence'): validate_evidence(self.model)
        finally:
            receipt.write_bytes(original); evidence.write_bytes(original_evidence)
    def test_no_independent_arena_events_cannot_promote_or_change_pointer(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); manifest,_=self.fixture(root,kind='engine-model-profile')
            (root/'events.jsonl').write_text('',encoding='utf-8')
            from promote import competition_identity
            champion=root/'champion'; champion.mkdir(); pointer=champion/'champion.json'
            before=json.dumps({'competition_identity':competition_identity(manifest['effective'],1)}).encode(); pointer.write_bytes(before)
            self.assertNotEqual(promote(root,self.model,champion,self.data)['status'],'promoted')
            self.assertEqual(pointer.read_bytes(),before)
