import json
import copy
import contextlib
import io
import tempfile
import unittest
from argparse import Namespace
from unittest.mock import patch
from pathlib import Path
import torch
from common import DatasetFile
from diagnose_value import components,weights,diagnose,parameter_report,batch_inputs,sample_indices,nonnegative_head_range
from mixlite_production import BatchedMixLite,reference_loss
import test_mixlite_hotpath
from test_compact import fixture
from mixlite_loss import targets
from train_value_only import run
from checkpoint import atomic_save,load_checkpoint
from dataset import DatasetBundle,describe_shard,file_hash
from common import make_split_manifest
from mixlite import FORMAT


class ValueDiagnosticTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):torch.set_num_threads(1)

    def test_nonnegative_head_bound_includes_integer_context_samples(self):
        torch.manual_seed(17);model=BatchedMixLite(500).float();h=model.wdl_head.detach().round().double()
        bound=nonnegative_head_range(h,16)
        context=torch.cat([torch.zeros(1,8),torch.eye(8).repeat_interleave(255,dim=0)*torch.arange(1,256).repeat(8)[:,None],
                           torch.randint(0,256,(1024,8)).float()]).double()
        evidence=(context@h.T/16).trunc().relu()+1
        q=(evidence[:,0]-evidence[:,2])/evidence.sum(1)
        self.assertGreaterEqual(float(q.min()),bound[0]-1e-12)
        self.assertLessEqual(float(q.max()),bound[1]+1e-12)
        h[0,0]=-1;self.assertIsNone(nonnegative_head_range(h,16))

    def test_components_weighted_sum_matches_production_and_value_gradient_isolated(self):
        rows=test_mixlite_hotpath.HotPathTests().batch();target=targets(rows,500)
        torch.manual_seed(17);model=BatchedMixLite(500).float()
        k=torch.stack([r[5] for r in rows]);c=torch.stack([r[6] for r in rows]);w,p,_=model(k,c)
        config=dict(exact_weight=4.,hard_weight=2.,outcome_weight=.25,mining_every=16)
        rank=p.detach().masked_fill(~target['legal'],float('-inf')).argsort(dim=1,descending=True,stable=True)[:,:5].tolist()
        for mining in (False,True):
            weight=weights(rows,w,p,target,config,mining,scale=500)
            part=components(w,p,target)
            total=sum((v*weight*(.25 if name=='outcome' else 1.)).mean() for name,v in part.items())
            expected=reference_loss(rows,w,p,scale=500,selected=torch.device('cpu'),do_mining=mining,
                rankings=rank,predictions=(w[:,0]-w[:,2]).detach().tolist(),policy_target='soft',
                exact_weight=4.,hard_weight=2.,outcome_weight=.25)
            torch.testing.assert_close(total,expected,atol=2e-5,rtol=1e-6)
        ((part['teacher_wdl_ce']+part['q_squared'])*weight).mean().backward()
        self.assertIsNone(model.policy_head.grad);self.assertIsNone(model.policy_context.grad)
        self.assertIsNotNone(model.wdl_head.grad)

    def test_deterministic_samples_reports_and_constant_correlation(self):
        self.assertEqual(sample_indices(range(100),16,17),sample_indices(range(100),16,17))
        with tempfile.TemporaryDirectory() as raw:
            path=Path(raw)/'data.rmd';fixture(path)
            config=dict(seed=17,exact_weight=4.,hard_weight=2.,outcome_weight=.25,mining_every=16,policy_target='soft')
            torch.manual_seed(17);model=BatchedMixLite(500).float()
            with DatasetFile(path) as dataset:
                result=diagnose(model,dict(score_scale=500,production=config),dataset,range(len(dataset)),
                    'train',count=8,batch_size=4,gradients=True)
                self.assertEqual(result['sample_count'],8)
                self.assertEqual(set(result['gradients']),{'ordinary','mining'})
                json.dumps(result,allow_nan=False)
            params=parameter_report(model,17)
            self.assertEqual(params['wdl_head']['integer_sign_counts']['positive'],24)
            self.assertEqual(params['wdl_head']['displacement_from_seeded_initialization']['std'],0)

    def test_isolated_two_step_experiment_preserves_reference_and_policy_parameters(self):
        with tempfile.TemporaryDirectory() as raw:
            root=Path(raw);data=root/'data.rmd';fixture(data)
            shard=describe_shard(data,{},'fixture',compact=True)
            for game,metadata in shard['games'].items():metadata['opening_family']=f'family-{int(game)//4}'
            descriptor=root/'dataset.json';descriptor.write_text(json.dumps(dict(version=2,shards=[shard])))
            with DatasetBundle(descriptor) as dataset:manifest=make_split_manifest(dataset,17)
            torch.manual_seed(17);initial=BatchedMixLite(500).float()
            config=dict(sampler='block-shuffle-v1',seed=17,batch_size=4,learning_rate=.001,
                policy_target='soft',exact_weight=4.,hard_weight=2.,outcome_weight=.25,mining_every=16)
            reference=root/'reference.pt'
            atomic_save(reference,dict(format=FORMAT,score_scale=500,model_state=initial.state_dict(),production=config,
                configuration={},split_manifest=manifest,steps=0))
            before=file_hash(reference)
            args=Namespace(steps=[1,2],reference=reference,output=root/'experiment',device='cpu',dataset=descriptor,
                samples=4,sample_seed=17,baseline=[],skip_reference_diagnostics=True,cache_dir=None)
            run(args)
            result=load_checkpoint(args.output/'step-2.pt')
            self.assertNotIn('production',result)
            self.assertEqual(result['split_manifest'],manifest)
            self.assertEqual(before,file_hash(reference))
            for name in ('policy_head','policy_context'):
                self.assertTrue(torch.equal(initial.state_dict()[name],result['model_state'][name]))
            self.assertFalse(torch.equal(initial.wdl_head,result['model_state']['wdl_head']))
            report=json.loads((args.output/'step-2.json').read_text())
            self.assertEqual(report['splits']['train']['actual_training_contributions']['outcome'],0)
            self.assertEqual(report['splits']['train']['actual_training_contributions']['hard_policy_ce'],0)
            with self.assertRaisesRegex(ValueError,'new isolated'):run(args)
            interrupted=copy.copy(args);interrupted.output=root/'interrupted'
            with contextlib.redirect_stdout(io.StringIO()):
                with patch('train_value_only.diagnose',side_effect=RuntimeError('injected interruption')):
                    with self.assertRaisesRegex(RuntimeError,'injected'):run(interrupted)
                self.assertEqual(load_checkpoint(interrupted.output/'latest.pt')['steps'],1)
                interrupted.resume=True
                run(interrupted)
            resumed=load_checkpoint(interrupted.output/'step-2.pt')
            for name,value in result['model_state'].items():self.assertTrue(torch.equal(value,resumed['model_state'][name]))
            interrupted.sample_seed+=1
            with self.assertRaisesRegex(ValueError,'resume identity mismatch'):run(interrupted)


if __name__=='__main__':unittest.main()
