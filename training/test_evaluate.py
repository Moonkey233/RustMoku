import dataclasses
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import torch

from common import DatasetFile, decode_position_key, make_split_manifest, validate_split_manifest, load_training_model
from evaluate import evaluate_dataset, _predict
from mixlite import MixLite, FORMAT, features
from checkpoint import atomic_save, load_checkpoint
from test_compact import fixture


class WholeBoardModel(torch.nn.Module):
    def forward(self, keys, centers):
        assert keys.shape == (225, 4)
        assert centers.shape == (225,)
        return (torch.tensor([.6, .3, .1], dtype=torch.float64),
                torch.arange(225, dtype=torch.float64), torch.ones(3))


class EvaluationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        torch.set_num_threads(1)

    def records(self, root):
        raw = root/'fixture.rmd'
        fixture(raw, games=12, plies=3)
        with DatasetFile(raw) as dataset:
            records = list(dataset)
        result = []
        for record in records:
            board, _ = decode_position_key(record.position_key)
            # Synthetic metadata covers all reporting buckets without changing
            # the stored fixture board used by inference.
            ply = (2, 20, 80)[record.ply]
            legal = [at for at, stone in enumerate(board) if not stone]
            result.append(dataclasses.replace(record, ply=ply, policy_move=legal[-2],
                value=500, source=2 if record.ply%2==0 else 8, completed_depth=1,
                outcome=(1,0,-1)[record.ply], opening_family=('A','B','C')[record.game_id%3]))
        return result

    def test_v3_all_frozen_splits_metrics_and_strata(self):
        with tempfile.TemporaryDirectory() as raw:
            records = self.records(Path(raw))
            manifest = make_split_manifest(records, 7)
            checkpoint = dict(format=FORMAT, score_scale=500, split_manifest=manifest)
            partitions = validate_split_manifest(records, manifest)
            for split in ('validation','test','opening_heldout'):
                with self.subTest(split=split):
                    with patch('mixlite.features', wraps=features) as mapping:
                        report = evaluate_dataset(WholeBoardModel(), checkpoint, records, split)
                    all_metrics = report['strata']['all']
                    self.assertEqual(mapping.call_count, len(partitions[split]))
                    self.assertEqual(all_metrics['samples'], len(partitions[split]))
                    self.assertAlmostEqual(all_metrics['value_mae'], 0)
                    self.assertEqual([all_metrics[f'top{k}'] for k in (1,3,5)], [0,1,1])
                    self.assertAlmostEqual(all_metrics['teacher_wdl_brier'], .06)
                    # W/D/L outcome targets appear equally often in each game.
                    expected = sum(sum((p-float(i==target))**2 for i,p in enumerate((.6,.3,.1))) for target in range(3))/3
                    self.assertAlmostEqual(all_metrics['wdl_brier'], expected)
                    self.assertAlmostEqual(all_metrics['wdl_accuracy'], 1/3)
                    for actual, expected_probability in zip(all_metrics['mean_wdl'], [.6,.3,.1], strict=True):
                        self.assertAlmostEqual(actual, expected_probability)
                    self.assertEqual(sum(report['strata'][key]['samples'] for key in ('opening','middle','late')), all_metrics['samples'])
                    self.assertEqual(sum(report['strata'][key]['samples'] for key in ('source-2','source-8')), all_metrics['samples'])
            with self.assertRaisesRegex(ValueError, 'seed conflicts'):
                evaluate_dataset(WholeBoardModel(), checkpoint, records, seed=8)
            changed = list(records); changed[0]=dataclasses.replace(changed[0],value=100)
            with self.assertRaisesRegex(ValueError,'fingerprint'):
                evaluate_dataset(WholeBoardModel(),checkpoint,changed)

    def test_real_v3_checkpoint_forward_and_legal_policy_mask(self):
        with tempfile.TemporaryDirectory() as raw:
            root=Path(raw);records=self.records(root)
            torch.manual_seed(8)
            model=MixLite(500)
            checkpoint=dict(format=FORMAT,score_scale=500,model_state=model.state_dict(),
                            split_manifest=make_split_manifest(records,7))
            path=root/'v3.pt';atomic_save(path,checkpoint)
            loaded=load_training_model(path,'cpu')
            record=next(r for r in records if any(decode_position_key(r.position_key)[0]))
            board,side=decode_position_key(record.position_key)
            with torch.no_grad():
                wdl,policy,_=model(*features(board,side))
                predicted,target,logits,at,actual_wdl=_predict(loaded,record,'cpu','v3',500)
            self.assertEqual(predicted,float(wdl[0]-wdl[2]))
            self.assertEqual(target,.5)
            self.assertEqual(actual_wdl,wdl.tolist())
            legal=[i for i,stone in enumerate(board) if not stone]
            self.assertTrue(torch.equal(logits,policy[legal]))
            self.assertEqual(legal[at],record.policy_move)
            exact_loss=dataclasses.replace(record,exact=True,value=-1000000)
            self.assertEqual(_predict(loaded,exact_loss,'cpu','v3',500)[1],-1)
            report=evaluate_dataset(loaded,load_checkpoint(path),records,'test')
            self.assertEqual(report['architecture'],'v3')
            bad=dataclasses.replace(record,policy_move=next(i for i,stone in enumerate(board) if stone))
            with self.assertRaisesRegex(ValueError,'legal empty'):_predict(loaded,bad,'cpu','v3',500)

    def test_missing_policy_outcomes_and_legacy_models(self):
        from common import LocalPatternModel
        from nonlinear_model import NonlinearModel
        with tempfile.TemporaryDirectory() as raw:
            records=[dataclasses.replace(r,policy_move=None,outcome=None) for r in self.records(Path(raw))]
            manifest=make_split_manifest(records,7)
            for format,model in [('rustmoku-local-pattern-v1',LocalPatternModel()),
                                 ('rustmoku-nonlinear-v2',NonlinearModel()),(FORMAT,WholeBoardModel())]:
                checkpoint=dict(format=format,score_scale=500,configuration={'score_scale':500},split_manifest=manifest)
                report=evaluate_dataset(model,checkpoint,records)
                metrics=report['strata']['all']
                self.assertIsNone(metrics['top1'])
                self.assertIsNone(metrics['wdl_brier'])
                self.assertIsNone(metrics['outcome_brier_linear_clamp'])
                self.assertEqual(metrics['teacher_wdl_samples']>0,format==FORMAT)
            records=[r for r in records if r.opening_family!='C']
            checkpoint=dict(format=FORMAT,score_scale=500,split_manifest=make_split_manifest(records,7))
            with self.assertRaisesRegex(ValueError,'empty or unavailable'):
                evaluate_dataset(WholeBoardModel(),checkpoint,records,'opening_heldout')

    def test_v3_cli_without_legacy_configuration(self):
        import contextlib
        import io
        import json
        import evaluate
        with tempfile.TemporaryDirectory() as raw:
            root=Path(raw); dataset=root/'data.rmd';fixture(dataset,games=3,plies=2)
            with DatasetFile(dataset) as records:
                manifest=make_split_manifest(records,7)
            model=MixLite(500); checkpoint=root/'v3.pt'
            atomic_save(checkpoint,dict(format=FORMAT,score_scale=500,model_state=model.state_dict(),split_manifest=manifest))
            output=io.StringIO()
            with patch('sys.argv',['evaluate.py','--dataset',str(dataset),'--checkpoint',str(checkpoint),'--split','test']), contextlib.redirect_stdout(output):
                evaluate.main()
            summary,_,json_text=output.getvalue().partition('\n')
            self.assertIn('policy_top5=unavailable',summary)
            report=json.loads(json_text)
            self.assertEqual(report['architecture'],'v3')
            self.assertEqual(report['strata']['all']['samples'],2)


if __name__=='__main__':unittest.main()
