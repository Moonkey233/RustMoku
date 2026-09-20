import copy
import dataclasses
import random
import tempfile
import unittest
from pathlib import Path
import numpy as np
import torch
from common import DataRecord, transform_position, transform_index, make_split_manifest, validate_split_manifest, DatasetFile
from mixlite import features
from mixlite_cache import canonical_inputs, transformed_inputs, FeatureCache
from mixlite_loss import targets, soft_loss
from mixlite_production import BatchedMixLite, reference_loss, train
from checkpoint import load_checkpoint
from teacher import comparison
from test_compact import fixture


def key(board,side):
    packed=bytearray(58)
    for i,c in enumerate(board):packed[i//4]|=c << (2*(3-i%4))
    packed[-1]=side
    return bytes(packed)


class HotPathTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):torch.set_num_threads(1)

    def test_d4_exact_inputs_and_targets_both_sides(self):
        rng=random.Random(47)
        boards=[[0]*225]
        # Subsets of a no-five full-board pattern give legal ongoing boards.
        colors=[1 if (i//15+2*(i%15))%4<2 else 2 for i in range(225)]
        for n in range(16):
            board=[0]*225
            cells=[[i for i,c in enumerate(colors) if c==s] for s in (1,2)]
            for group in cells:rng.shuffle(group)
            for ply in range(n*8):board[cells[ply%2].pop()]=ply%2+1
            boards.append(board)
        for cell in (0,14,112,210,224):
            board=[0]*225;board[cell]=1;boards.append(board)
        for board in boards:
            for side in (0,1):
                keys,centers=canonical_inputs([key(board,side)])
                for symmetry in range(8):
                    actual=transformed_inputs(keys,centers,[symmetry])
                    transformed,move=transform_position(board,113,symmetry)
                    expected=features(transformed,side)
                    self.assertTrue(np.array_equal(actual[0][0],expected[0].numpy()))
                    self.assertTrue(np.array_equal(actual[1][0],expected[1].numpy()))
                    self.assertEqual(move,transform_index(113,symmetry))

    def test_cache_identity_integrity_and_bounded_allocation(self):
        with tempfile.TemporaryDirectory() as raw:
            root=Path(raw);path=root/'data.rmd';fixture(path)
            with DatasetFile(path) as dataset:
                manifest=make_split_manifest(dataset,7);indices=validate_split_manifest(dataset,manifest)['train']
                with self.assertRaisesRegex(ValueError,'limit'):
                    FeatureCache(root/'small',dataset,manifest,indices,max_bytes=1)
                cache=FeatureCache(root/'cache',dataset,manifest,indices)
                self.assertEqual(cache.report['cache_bytes'],len(indices)*2025)
                cached=cache.batch([0],[7]);cache.close()
                cache=FeatureCache(root/'cache',dataset,manifest,indices)
                self.assertFalse(cache.report['built']);self.assertTrue(np.array_equal(cache.batch([0],[7])[0],cached[0]));cache.close()
                with self.assertRaisesRegex(ValueError,'mismatch'):
                    FeatureCache(root/'cache',dataset,{**manifest,'seed':8},indices)
                payload=root/'cache/features.bin'
                with payload.open('r+b') as stream:stream.write(b'bad')
                with self.assertRaisesRegex(ValueError,'mismatch'):FeatureCache(root/'cache',dataset,manifest,indices)

    def batch(self):
        rows=[]
        for i in range(5):
            board=[0]*225;board[112]=1;board[0]=2
            symmetry=i;board,move=transform_position(board,50+i,symmetry)
            c=comparison(dict(perspective='root-side-to-move',completed_depth=2,candidates=[dict(move=at,score=score,completed_depth=2,nominal_depth_valid=True,bound='Exact',source='AlphaBeta') for at,score in [(50,100),(70,200),(100,-300)]]),1000)
            record=DataRecord(i,2,0,move,(-1 if i==0 else i*100),2,i==0,key(board,0),outcome=(i%3)-1,
                comparison=c if i in (1,2) else None,sample_weight=.5+i/4)
            k,center=features(board,0)
            rows.append((record,board,0,move if i!=4 else None,symmetry,k,center))
        return rows

    def test_soft_targets_loss_gradients_and_adam_step(self):
        batch=self.batch();target=targets(batch,500)
        self.assertEqual(target['q'].tolist(),[-1.,float(np.float32(1/6)),float(np.float32(2/7)),.375,float(np.float32(4/9))])
        for device in ['cpu']+(['cuda'] if torch.cuda.is_available() else []):
            with self.subTest(device=device):
                torch.manual_seed(31);old=BatchedMixLite(500).float().to(device);new=copy.deepcopy(old)
                a=torch.optim.Adam(old.parameters(),lr=.001);b=torch.optim.Adam(new.parameters(),lr=.001)
                keys=torch.stack([r[5] for r in batch]).to(device);centers=torch.stack([r[6] for r in batch]).to(device)
                w,p,_=old(keys,centers)
                reference=reference_loss(batch,w,p,scale=500,selected=torch.device(device),do_mining=False,
                    rankings=None,predictions=None,policy_target='soft',exact_weight=4.,hard_weight=2.,outcome_weight=.25)
                w,p,_=new(keys,centers)
                actual=soft_loss(w,p,{k:v.to(device) for k,v in target.items()},w.new_tensor([4,1,1,1,1]),.25)
                torch.testing.assert_close(actual,reference,atol=2e-5,rtol=1e-6)
                reference.backward();actual.backward()
                for x,y in zip(old.parameters(),new.parameters(),strict=True):
                    torch.testing.assert_close(x.grad,y.grad,atol=2e-5,rtol=2e-6)
                a.step();b.step()
                for x,y in zip(old.parameters(),new.parameters(),strict=True):
                    torch.testing.assert_close(x,y,atol=2e-6,rtol=1e-7)

    def test_padded_soft_observations_and_missing_labels(self):
        batch=self.batch()
        smaller=comparison(dict(perspective='root-side-to-move',completed_depth=2,candidates=[
            dict(move=at,score=score,completed_depth=2,nominal_depth_valid=True,bound='Exact',source='AlphaBeta')
            for at,score in [(20,500),(80,-500)]]),1000)
        row=batch[2];batch[2]=(dataclasses.replace(row[0],comparison=smaller,outcome=None),*row[1:])
        torch.manual_seed(53)
        policy=(torch.randn(5,225)*10).requires_grad_()
        wdl=torch.randn(5,3).softmax(1).requires_grad_()
        expected=reference_loss(batch,wdl,policy,scale=500,selected=torch.device('cpu'),do_mining=False,
            rankings=None,predictions=None,policy_target='soft',exact_weight=4.,hard_weight=2.,outcome_weight=.25)
        actual=soft_loss(wdl,policy,targets(batch,500),wdl.new_tensor([4,1,1,1,1]),.25)
        torch.testing.assert_close(actual,expected,atol=2e-6,rtol=1e-6)
        left=torch.autograd.grad(expected,(wdl,policy),retain_graph=True)
        right=torch.autograd.grad(actual,(wdl,policy))
        for a,b in zip(left,right,strict=True):torch.testing.assert_close(a,b,atol=2e-6,rtol=1e-6)

    def test_optimized_resume_preserves_schedule_and_checkpoint_contract(self):
        with tempfile.TemporaryDirectory() as raw:
            root=Path(raw);data=root/'data.rmd';fixture(data)
            options=dict(epochs=4,batch_size=4,seed=17,mining_every=1,cache_dir=root/'cache')
            train(data,root/'full.pt',steps=3,**options)
            train(data,root/'part.pt',steps=1,**options)
            train(data,root/'resumed.pt',steps=2,resume=root/'part.pt',**options)
            full=load_checkpoint(root/'full.pt');resumed=load_checkpoint(root/'resumed.pt')
            self.assertEqual(full['production'],resumed['production'])
            self.assertEqual((full['epoch'],full['cursor'],full['steps']),(resumed['epoch'],resumed['cursor'],resumed['steps']))
            for key,value in full['model_state'].items():self.assertTrue(torch.equal(value,resumed['model_state'][key]))
            train(data,root/'legacy.pt',steps=1,reference_path=True,**options)
            # Both paths mine this first step, so this also protects hard-example
            # tagging/weighting and cached augmentation through the real trainer.
            legacy=load_checkpoint(root/'legacy.pt');optimized=load_checkpoint(root/'part.pt')
            for name,value in legacy['model_state'].items():
                torch.testing.assert_close(value,optimized['model_state'][name],atol=2e-6,rtol=1e-7)
            train(data,root/'from-legacy.pt',steps=1,resume=root/'legacy.pt',**options)
            self.assertEqual(load_checkpoint(root/'from-legacy.pt')['steps'],2)

    def test_ranking_path_uses_unchanged_reference_loss(self):
        with tempfile.TemporaryDirectory() as raw:
            root=Path(raw);data=root/'data.rmd';fixture(data)
            options=dict(steps=1,epochs=1,batch_size=4,seed=17,policy_target='ranking',mining_every=1)
            train(data,root/'cached.pt',**options)
            train(data,root/'reference.pt',reference_path=True,**options)
            cached=load_checkpoint(root/'cached.pt');reference=load_checkpoint(root/'reference.pt')
            for name,value in reference['model_state'].items():
                self.assertTrue(torch.equal(value,cached['model_state'][name]))

if __name__=='__main__':unittest.main()
