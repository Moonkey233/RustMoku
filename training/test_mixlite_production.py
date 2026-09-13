import tempfile
import unittest
from pathlib import Path
import torch
from mixlite import MixLite, features, IntegerMixLite, export, verify
from mixlite_production import BatchedMixLite, train, hard_example_tags
from test_compact import fixture
from checkpoint import load_checkpoint

class ProductionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls): torch.set_num_threads(1)
    def test_batch_forward_matches_scalar_qat(self):
        torch.manual_seed(12)
        scalar=MixLite(); batch=BatchedMixLite(); batch.load_state_dict(scalar.state_dict())
        boards=[[0]*225,[0]*225]; boards[1][0]=1; boards[1][112]=2
        inputs=[features(board,0) for board in boards]
        result=batch(torch.stack([x[0] for x in inputs]),torch.stack([x[1] for x in inputs]))
        for i,x in enumerate(inputs):
            expected=scalar(*x)
            for a,b in zip(expected,result): self.assertTrue(torch.equal(a,b[i]))
    def test_batched_resume_and_export_smoke(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); data=root/'data.rmd'; checkpoint=root/'v3.pt'; model=root/'v3.bin'
            fixture(data)
            self.assertEqual(train(data,checkpoint,steps=1,epochs=10,batch_size=2),1)
            self.assertEqual(train(data,checkpoint,steps=1,epochs=10,batch_size=2,resume=checkpoint),2)
            with self.assertRaises(ValueError): train(data,checkpoint,steps=1,epochs=10,batch_size=3,resume=checkpoint)
            export(checkpoint,data,model); IntegerMixLite(model.read_bytes())
    def test_shard_readers_bound_handles_and_reopen_exact_records(self):
        from dataset import ShardReaders
        with tempfile.TemporaryDirectory() as directory:
            pool=ShardReaders(capacity=2)
            try:
                for i in range(4):
                    path=Path(directory)/f'{i}.rmd'; fixture(path); pool.append(path)
                expected=pool[0][0]
                for i in [1,2,3,0,3,2,1,0]:
                    self.assertEqual(pool[i][0],expected)
                    self.assertLessEqual(len(pool.readers),2)
            finally: pool.close()
    def test_mining_labels_are_explicit(self):
        tags=hard_example_tags(teacher_move=5,student_order=[1,2],teacher_value=-1,student_value=1,
            candidate_universe=[1,2],exact=True,forced_loss=True)
        self.assertEqual(len(tags),6)
        ordinary=hard_example_tags(teacher_move=None,student_order=[],teacher_value=-1,student_value=1)
        self.assertNotIn('tactical-error',ordinary)
