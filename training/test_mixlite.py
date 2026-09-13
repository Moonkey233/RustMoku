import tempfile
import unittest
from pathlib import Path
import torch
from mixlite import MixLite, IntegerMixLite, features, train, export
from common import truncating_division
from test_compact import fixture


class MixLiteTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        torch.set_num_threads(1)

    def test_qat_forward_matches_independent_integer_reference(self):
        torch.manual_seed(3)
        model = MixLite()
        integer = IntegerMixLite(model.bytes())
        board = [0]*225
        for at, stone in [(112,1),(113,2),(80,1),(224,2)]: board[at] = stone
        for side in [0,1]:
            value, policy, wdl_q15 = integer.infer(board, side)
            wdl, actual, evidence = model(*features(board, side))
            self.assertEqual(actual.detach().to(torch.int64).tolist(), policy)
            e = [int(x) for x in evidence.detach()]
            q = truncating_division((e[0]-e[2])*32768, sum(e))
            expected = max(-10000000,min(10000000,truncating_division(model.score_scale*q,32768-abs(q))))
            self.assertEqual(value, expected)
            self.assertEqual(sum(wdl_q15),32768)
            self.assertAlmostEqual(float(wdl.detach().sum()),1)

    def test_d4_value_invariance_and_policy_equivariance(self):
        from common import transform_position, transform_index
        torch.manual_seed(9)
        integer = IntegerMixLite(MixLite().bytes())
        board = [0]*225
        for at, stone in [(0,1),(14,2),(17,1),(95,2),(111,1),(167,2),(224,1)]: board[at] = stone
        for side in (0,1):
            value, policy, wdl = integer.infer(board, side)
            for symmetry in range(8):
                transformed, _ = transform_position(board, None, symmetry)
                actual, logits, actual_wdl = integer.infer(transformed, side)
                self.assertEqual((value, wdl), (actual, actual_wdl))
                for at in range(225): self.assertEqual(policy[at], logits[transform_index(at, symmetry)])
        old = bytearray(MixLite().bytes()); old[10] = 3
        with self.assertRaises(ValueError): IntegerMixLite(old)

    def test_remote_center_changes_value_and_policy_without_local_change(self):
        model = MixLite()
        with torch.no_grad():
            for p in model.parameters(): p.zero_()
            model.center[:,0] = 16
            model.center[1,0] = 127
            model.mixing[0,128] = 127
            model.bias[1] = 256
            model.wdl_head[0,0] = 100
            model.wdl_head[2,1] = 100
            model.policy_context[0] = 32767
        integer = IntegerMixLite(model.bytes())
        board = [0]*225
        before = integer.infer(board,0)
        board[0] = 1
        after = integer.infer(board,0)
        # O1's local four line keys do not contain A1; only context has changed.
        self.assertNotEqual(before[0], after[0])
        self.assertNotEqual(before[1][14], after[1][14])

    def test_bounded_train_export_and_resume_bind_the_dataset(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); data=root/'data.rmd'; checkpoint=root/'v3.pt'; model=root/'v3.bin'
            fixture(data)
            self.assertEqual(train(data,checkpoint,1,seed=4,max_seconds=30),1)
            export(checkpoint,data,model)
            IntegerMixLite(model.read_bytes())
            self.assertEqual(train(data,checkpoint,1,seed=4,max_seconds=30,resume=checkpoint),2)
            payload=bytearray(data.read_bytes()); payload[-64] ^= 1; data.write_bytes(payload)
            with self.assertRaises(ValueError): export(checkpoint,data,root/'wrong.bin')
