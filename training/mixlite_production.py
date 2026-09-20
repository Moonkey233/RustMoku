"""Batched V3 QAT on CUDA or CPU; scalar mixlite.py remains the export oracle.

Records are read lazily from the existing mmap/sharded dataset. Canonical features
use a bounded immutable disk mmap; only indices, a batch and model/optimizer
tensors need Python-owned RAM. Split validation precedes augmentation/sampling.
"""
import argparse
import math
import contextlib
import json
import time
from pathlib import Path
import torch
from torch.nn import functional as F
from mixlite import MixLite, FORMAT, GROUPS, COUNTS, features, ste_integer, ste_trunc
from common import (decode_position_key, transform_position, transform_index,
                    make_split_manifest, validate_split_manifest, eligible_label)
from dataset import open_dataset
from checkpoint import atomic_save, load_checkpoint
from teacher import validate_comparison, masked_loss


class BatchedMixLite(MixLite):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        for band in range(4):
            self.register_buffer(f'group_{band}', torch.tensor([i for i,g in enumerate(GROUPS) if g == band]), persistent=False)
    def forward(self, keys, centers):
        local = (F.embedding(keys, ste_integer(self.embedding, -128, 127)).sum(-2)
                 + F.embedding(centers, ste_integer(self.center, -128, 127))).clamp(0, 255)
        # Four small reductions; no per-record model calls or whole-board CNN.
        groups = torch.stack([local.index_select(1, getattr(self, f'group_{band}')).sum(1)
                              for band in range(4)], dim=1)
        pooled = torch.cat([ste_trunc(groups.sum(1)/225)] +
                           [ste_trunc(groups[:,g]/COUNTS[g]) for g in range(4)], dim=-1)
        context = ste_trunc((F.linear(pooled, ste_integer(self.mixing,-128,127)) +
                             ste_integer(self.bias,-1048576,1048576))/256).clamp(0,255)
        evidence = ste_trunc(F.linear(context,ste_integer(self.wdl_head,-32768,32767))/self.value_divisor).relu()+1
        wdl = evidence/evidence.sum(-1,keepdim=True)
        dot = local @ ste_integer(self.policy_head,-32768,32767)
        cross = (local[:,:,:8]*context[:,None,:]) @ ste_integer(self.policy_context,-32768,32767)
        policy = ste_trunc((dot+ste_trunc(cross/256))/self.policy_divisor).clamp(-32768,32767)
        return wdl, policy, evidence


def hard_example_tags(*, teacher_move, student_order, teacher_value, student_value,
                      candidate_universe=None, exact=False, forced_loss=False,
                      value_error_threshold=.25, top_k=5):
    """Diagnostic mining labels; they confer no proof authority on AB scores."""
    tags = []
    if teacher_move is not None:
        if not student_order or teacher_move != student_order[0]: tags.append('teacher-student-disagreement')
        if teacher_move not in student_order[:top_k]: tags.append('policy-top-k-miss')
        if candidate_universe is not None and teacher_move not in candidate_universe: tags.append('candidate-universe-miss')
    if abs(teacher_value-student_value) > value_error_threshold: tags.append('high-value-error')
    if exact and teacher_value*student_value <= 0 and teacher_value != 0: tags.append('tactical-error')
    if exact and forced_loss: tags.append('exact-forced-loss')
    return tags


def reference_loss(batch,wdl,policy,*,scale,selected,do_mining,rankings,predictions,policy_target,exact_weight,hard_weight,outcome_weight):
    losses = []
    for j,(record,board,side,move,symmetry,_,_) in enumerate(batch):
        q = float((record.value>0)-(record.value<0)) if record.exact else record.value/(scale+abs(record.value))
        target = wdl.new_tensor([max(q,0),1-abs(q),max(-q,0)])
        loss = -(target*wdl[j].log()).sum()+(wdl[j,0]-wdl[j,2]-q).square()
        outcome = record.outcome
        if outcome is not None and not record.exact:
            outcome_target = wdl.new_tensor([max(outcome,0),1-abs(outcome),max(-outcome,0)])
            loss = loss + outcome_weight * -(outcome_target*wdl[j].log()).sum()
        legal = [i for i,c in enumerate(board) if c==0]
        if move is not None:
            if move not in legal: raise ValueError('illegal policy target')
            loss = loss+F.cross_entropy(policy[j,legal][None,:],torch.tensor([legal.index(move)],device=selected))
        candidate_universe = None
        if record.comparison is not None and not record.exact:
            comparison = validate_comparison(record.comparison)
            observed = [transform_index(at,symmetry) for at in comparison['moves']]
            if any(at not in legal for at in observed): raise ValueError('illegal comparison target')
            loss = loss+masked_loss(policy[j],observed,comparison['probabilities'],
                ranking_scores=comparison['scores'] if policy_target=='ranking' else None)
            # Masked teacher comparisons do not describe the
            # production candidate universe; never infer recall here.
        tags = []
        if do_mining:
            ranking = [i for i in rankings[j] if i in legal]
            tags = hard_example_tags(teacher_move=move,student_order=ranking,teacher_value=q,
                student_value=predictions[j],candidate_universe=candidate_universe,
                exact=record.exact,forced_loss=record.exact and record.value<0)
        weight = exact_weight if record.exact else hard_weight if tags else 1.
        losses.append(loss*weight*record.sample_weight)
    return torch.stack(losses).mean()

class TrainingTimer:
    def __init__(self,device,enabled):
        self.device=device;self.enabled=enabled;self.active=False;self.seconds={};self.records=0;self.steps=0;self.total=0.
    def sync(self):
        if self.device.type=='cuda':torch.cuda.synchronize(self.device)
    @contextlib.contextmanager
    def section(self,name):
        if not self.enabled or not self.active:
            yield;return
        self.sync();started=time.perf_counter()
        yield
        self.sync();self.seconds[name]=self.seconds.get(name,0)+time.perf_counter()-started
    def finish_step(self,started,records):
        if self.enabled and self.active:
            self.sync();self.total+=time.perf_counter()-started;self.records+=records;self.steps+=1
    def report(self):
        return dict(measured_steps=self.steps,records=self.records,seconds=self.total,stage_seconds=self.seconds,
            seconds_per_step=self.total/self.steps if self.steps else None,
            records_per_second=self.records/self.total if self.total else None,
            steps_per_second=self.steps/self.total if self.total else None,warmup_steps=5)


def train(dataset_path, checkpoint_path, *, steps, epochs=1, batch_size=256,
          device='cpu', seed=1, resume=None, learning_rate=.001, checkpoint_every=100,
          policy_target='soft', exact_weight=4., hard_weight=2., outcome_weight=.25, mining_every=0,
          cache_dir=None, cache_max_bytes=4*1024**3, reference_path=False, timing_output=None,
          reference_loss_path=False):
    if min(steps,epochs,batch_size,checkpoint_every) < 1: raise ValueError('positive training limits required')
    if type(mining_every) is not int or mining_every < 0: raise ValueError('mining cadence must be nonnegative')
    if policy_target not in ('soft','ranking'): raise ValueError('invalid policy target')
    if not all(math.isfinite(x) and x > 0 for x in (learning_rate,exact_weight,hard_weight)) or not 0 <= outcome_weight <= 1:
        raise ValueError('invalid optimizer/loss configuration')
    selected = torch.device(device)
    if selected.type == 'cuda' and not torch.cuda.is_available(): raise ValueError('CUDA requested but unavailable; select cpu explicitly')
    from mixlite_cache import FeatureCache
    from mixlite_loss import targets, soft_loss
    timer=TrainingTimer(selected,timing_output is not None)
    use_reference_loss=reference_path or reference_loss_path or policy_target=='ranking'
    setup_started=time.perf_counter()
    torch.manual_seed(seed)
    # Float32 QAT approximates integer rounding during learning. Export always
    # replays rounded weights through the independent exact integer oracle.
    with open_dataset(Path(dataset_path)) as dataset:
        previous = load_checkpoint(resume) if resume else None
        if previous and previous.get('format') != FORMAT: raise ValueError('V3 architecture mismatch')
        if previous and previous.get('production') is None: raise ValueError('resume needs a production checkpoint')
        manifest = previous['split_manifest'] if previous else make_split_manifest(dataset,seed)
        indices = validate_split_manifest(dataset,manifest,seed)['train']
        if not indices: raise ValueError('no training records')
        config = dict(sampler='block-shuffle-v1',batch_size=batch_size,seed=seed,learning_rate=learning_rate,policy_target=policy_target,
                      exact_weight=exact_weight,hard_weight=hard_weight,outcome_weight=outcome_weight,mining_every=mining_every)
        if previous and {**previous['production'], 'mining_every': previous['production'].get('mining_every',1)} != config: raise ValueError('resume training configuration changed')
        sample_indices = [indices[j*len(indices)//min(4096,len(indices))] for j in range(min(4096,len(indices)))]
        sample = [abs(dataset[i].value) for i in sample_indices if not dataset[i].exact]
        scale = previous['score_scale'] if previous else max(1,min(10000000,sorted(sample)[len(sample)//2] if sample else 500))
        model = BatchedMixLite(scale).float().to(selected)
        optimizer = torch.optim.Adam(model.parameters(),lr=learning_rate)
        completed = epoch = cursor = 0
        if previous:
            model.load_state_dict(previous['model_state']); optimizer.load_state_dict(previous['optimizer_state'])
            completed,epoch,cursor = previous['steps'],previous['epoch'],previous['cursor']
        setup_seconds=time.perf_counter()-setup_started
        if timing_output is not None:
            print(json.dumps(dict(profile_setup_seconds=setup_seconds,start_steps=completed)),flush=True)
        start = completed
        checkpoint_path = Path(checkpoint_path); checkpoint_path.parent.mkdir(parents=True,exist_ok=True)
        def save():
            atomic_save(checkpoint_path,dict(format=FORMAT,model_state=model.state_dict(),optimizer_state=optimizer.state_dict(),
                steps=completed,epoch=epoch,cursor=cursor,score_scale=scale,split_manifest=manifest,production=config,
                configuration=dict(architecture='mixlite-width32-context8-d4-v3',value_contract='stm-rational-q15-v2',
                    score_scale=scale,training='batched-qat-v1')))
        with contextlib.ExitStack() as resources:
            cache=None
            if not reference_path:
                cache=FeatureCache(cache_dir or checkpoint_path.parent/'feature-cache',dataset,manifest,indices,cache_max_bytes)
                resources.callback(cache.close)
                print(json.dumps(cache.report),flush=True)
            training_started=time.perf_counter()
            while completed-start < steps and epoch < epochs:
                generator = torch.Generator().manual_seed(seed+epoch)
                # Shuffle bounded contiguous blocks, preserving disk locality on
                # large sharded corpora while changing order every epoch.
                block_size = max(batch_size, 1024)
                blocks = torch.randperm((len(indices)+block_size-1)//block_size,generator=generator).tolist()
                order = []
                for block in blocks:
                    start_index = block*block_size
                    order.extend(start_index+i for i in torch.randperm(min(block_size,len(indices)-start_index),generator=generator).tolist())
                while cursor < len(order) and completed-start < steps:
                    timer.active=completed-start>=5
                    step_started=time.perf_counter()
                    local_indices=order[cursor:cursor+batch_size]
                    with timer.section('dataset_metadata'):
                        records=[dataset[indices[local]] for local in local_indices]
                    with timer.section('feature_cache'):
                        symmetries=[(seed+epoch+indices[local])%8 for local in local_indices]
                        batch=[]
                        if cache is not None:
                            cached_keys,cached_centers=cache.batch(local_indices,symmetries)
                        for j,(record,symmetry) in enumerate(zip(records,symmetries,strict=True)):
                            if not eligible_label(record):raise ValueError('ineligible label in training split')
                            board,side=decode_position_key(record.position_key)
                            board,move=transform_position(board,record.policy_move,symmetry)
                            key,center=features(board,side) if cache is None else (None,None)
                            batch.append((record,board,side,move,symmetry,key,center))
                        keys=torch.stack([row[5] for row in batch]) if cache is None else torch.from_numpy(cached_keys)
                        centers=torch.stack([row[6] for row in batch]) if cache is None else torch.from_numpy(cached_centers)
                        cpu_target=targets(batch,scale) if not use_reference_loss else None
                    with timer.section('h2d'):
                        keys=keys.to(selected);centers=centers.to(selected)
                        target={key:value.to(selected) for key,value in cpu_target.items()} if cpu_target is not None else None
                    with timer.section('forward'):
                        wdl,policy,_=model(keys,centers)
                    rankings=predictions=None
                    do_mining = mining_every > 0 and completed % mining_every == 0
                    with timer.section('mining_sync'):
                        if do_mining:
                            rankings = policy.detach().masked_fill(centers != 0, float('-inf')).argsort(dim=1,descending=True,stable=True)[:,:5].cpu().tolist()
                            predictions = (wdl[:,0]-wdl[:,2]).detach().cpu().tolist()
                    with timer.section('loss'):
                        if use_reference_loss:
                            loss=reference_loss(batch,wdl,policy,scale=scale,selected=selected,do_mining=do_mining,
                                rankings=rankings,predictions=predictions,policy_target=policy_target,
                                exact_weight=exact_weight,hard_weight=hard_weight,outcome_weight=outcome_weight)
                        else:
                            weights=[]
                            for j,(record,board,_,move,_,_,_) in enumerate(batch):
                                tags=[]
                                if do_mining:
                                    legal=[i for i,c in enumerate(board) if c==0]
                                    q=float((record.value>0)-(record.value<0)) if record.exact else record.value/(scale+abs(record.value))
                                    tags=hard_example_tags(teacher_move=move,student_order=[i for i in rankings[j] if i in legal],
                                        teacher_value=q,student_value=predictions[j],candidate_universe=None,
                                        exact=record.exact,forced_loss=record.exact and record.value<0)
                                weights.append(exact_weight if record.exact else hard_weight if tags else 1.)
                            loss=soft_loss(wdl,policy,target,wdl.new_tensor(weights),outcome_weight)
                    if (selected.type == 'cpu' or do_mining or (completed+1)%checkpoint_every==0) and not torch.isfinite(loss):
                        raise ValueError('nonfinite training loss')
                    with timer.section('backward_optimizer'):
                        optimizer.zero_grad(set_to_none=True); loss.backward(); optimizer.step()
                    timer.finish_step(step_started,len(batch))
                    cursor += len(batch); completed += 1
                    if timing_output is not None and (completed-start)%25==0:
                        print(json.dumps(dict(profile_steps=completed-start,seconds_per_step=timer.report()['seconds_per_step'])),flush=True)
                    if completed%checkpoint_every==0: save()
                if cursor == len(order): epoch += 1; cursor = 0
        if completed > start and not torch.isfinite(loss): raise ValueError('nonfinite final loss')
        validate_split_manifest(dataset,manifest,seed)
        save()
        if timing_output is not None:
            report=dict(timer.report(),cache=cache.report if cache else None,completed_steps=completed,
                        start_steps=start,epoch=epoch,cursor=cursor,qualified_records=len(indices),
                        setup_seconds=setup_seconds,
                        training_seconds=time.perf_counter()-training_started)
            Path(timing_output).write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
            print(json.dumps(report))
        return completed


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--dataset',type=Path,required=True); p.add_argument('--output',type=Path,required=True)
    p.add_argument('--device',default='cpu'); p.add_argument('--batch-size',type=int,default=256)
    p.add_argument('--steps',type=int,required=True); p.add_argument('--epochs',type=int,default=1)
    p.add_argument('--resume',type=Path); p.add_argument('--seed',type=int,default=1)
    p.add_argument('--checkpoint-every',type=int,default=100); p.add_argument('--learning-rate',type=float,default=.001)
    p.add_argument('--cache-dir',type=Path); p.add_argument('--cache-max-bytes',type=int,default=4*1024**3)
    p.add_argument('--reference-path',action='store_true'); p.add_argument('--timing-output',type=Path)
    p.add_argument('--reference-loss-path',action='store_true',help='diagnostic: cached inputs with the original loss')
    p.add_argument('--mining-every',type=int,default=0)
    for name,default in [('exact-weight',4.),('hard-weight',2.),('outcome-weight',.25)]: p.add_argument('--'+name,type=float,default=default)
    p.add_argument('--policy-target',choices=('soft','ranking'),default='soft')
    a=p.parse_args(); options=vars(a); dataset=options.pop('dataset'); output=options.pop('output')
    print('steps=',train(dataset,output,**options))
if __name__=='__main__': main()
