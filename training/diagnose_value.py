"""Read-only MixLite V3 value/loss/gradient diagnostics on frozen split samples."""
import argparse
import hashlib
import json
import random
from pathlib import Path

import numpy as np
import torch
from torch.nn import functional as F
from checkpoint import load_checkpoint
from common import decode_position_key, transform_position, validate_split_manifest
from dataset import open_dataset, file_hash
from mixlite import FORMAT, COUNTS, ste_integer, ste_trunc
from mixlite_cache import canonical_inputs, transformed_inputs
from mixlite_loss import targets
from mixlite_production import BatchedMixLite, hard_example_tags

FAMILIES=('embedding','center','mixing','bias','wdl_head','policy_head','policy_context')
QUANTILES=[0,.01,.1,.25,.5,.75,.9,.99,1]


def statistics(values):
    x=np.asarray(values,dtype=np.float64)
    if not x.size:return None
    if not np.isfinite(x).all():raise ValueError('nonfinite diagnostic data')
    return dict(mean=float(x.mean()),std=float(x.std()),min=float(x.min()),max=float(x.max()),
                quantiles=dict(zip(map(str,QUANTILES),map(float,np.quantile(x,QUANTILES)))))


def sample_indices(indices,count,seed):
    if count<1:raise ValueError('sample count must be positive')
    return [indices[i] for i in sorted(random.Random(seed).sample(range(len(indices)),min(count,len(indices))))]


def batch_inputs(dataset,indices,scale,device,symmetries=None,cached=None):
    records=[dataset[i] for i in indices]
    symmetries=[0]*len(records) if symmetries is None else symmetries
    rows=[]
    for record,symmetry in zip(records,symmetries,strict=True):
        board,side=decode_position_key(record.position_key)
        board,move=transform_position(board,record.policy_move,symmetry)
        rows.append((record,board,side,move,symmetry,None,None))
    if cached is None:
        keys,centers=canonical_inputs([r.position_key for r in records])
        keys,centers=transformed_inputs(keys,centers,symmetries)
    else:keys,centers=cached
    target={k:v.to(device) for k,v in targets(rows,scale).items()}
    return rows,torch.from_numpy(keys).to(device),torch.from_numpy(centers).to(device),target


def components(wdl,policy,target):
    log=wdl.log()
    hard=policy.masked_fill(~target['legal'],float('-inf'))
    hard=torch.where((target['hard']!=-100)[:,None],hard,torch.zeros_like(hard))
    selected=policy.gather(1,target['indices']).masked_fill(~target['observed'],float('-inf'))
    return dict(teacher_wdl_ce=-(target['wdl']*log).sum(1),
                q_squared=(wdl[:,0]-wdl[:,2]-target['q']).square(),
                outcome=-(target['outcomes']*log).sum(1)*target['outcome_mask'],
                hard_policy_ce=F.cross_entropy(hard,target['hard'],reduction='none',ignore_index=-100),
                soft_comparison=-(target['probabilities']*selected.log_softmax(1).masked_fill(~target['observed'],0)).sum(1))


def weights(rows,wdl,policy,target,config,mining,*,scale):
    tags=[False]*len(rows)
    if mining:
        rankings=policy.detach().masked_fill(~target['legal'],float('-inf')).argsort(dim=1,descending=True,stable=True)[:,:5].cpu().tolist()
        values=(wdl[:,0]-wdl[:,2]).detach().cpu().tolist()
        q=[float((r[0].value>0)-(r[0].value<0)) if r[0].exact else r[0].value/(scale+abs(r[0].value)) for r in rows]
        for j,(record,board,_,move,_,_,_) in enumerate(rows):
            tags[j]=bool(hard_example_tags(teacher_move=move,student_order=[i for i in rankings[j] if board[i]==0],
                teacher_value=q[j],student_value=values[j],candidate_universe=None,
                exact=record.exact,forced_loss=record.exact and record.value<0))
    return wdl.new_tensor([config['exact_weight'] if r[0].exact else config['hard_weight'] if tag else 1.
                          for r,tag in zip(rows,tags,strict=True)])*target['sample']


def model_from(checkpoint,device):
    if checkpoint.get('format')!=FORMAT:raise ValueError('diagnostic requires existing D4 MixLite V3')
    model=BatchedMixLite(checkpoint['score_scale']).float().to(device)
    model.load_state_dict(checkpoint['model_state']);return model


def nonnegative_head_range(integer,divisor):
    """Conservative evidence-ratio bound, before runtime Q15 probability rounding.

    For nonzero integer context, T=sum(Hc/divisor)>=min(column sums)/divisor.
    Each evidence is its nonnegative raw head plus epsilon in (0,1]. Therefore
    (r_min*T-1)/(T+1) <= q <= (r_max*T+1)/(T+1). Zero context gives q=0.
    Signed/zero-sum columns deliberately return no bound.
    """
    h=integer.detach().cpu().double();s=h.sum(0)
    if bool((h<0).any()) or bool((s<=0).any()):return None
    ratios=(h[0]-h[2])/s;t=float(s.min())/divisor
    return [min(0.,(float(ratios.min())*t-1)/(t+1)),max(0.,(float(ratios.max())*t+1)/(t+1))]


def parameter_report(model,seed,*,initial=None):
    if initial is None:
        with torch.random.fork_rng(devices=[]):
            torch.manual_seed(seed);initial=BatchedMixLite(model.score_scale).float()
    result={}
    for name,p in model.named_parameters():
        x=p.detach().cpu();lo,hi=(-128,127) if name in ('embedding','center','mixing') else ((-1048576,1048576) if name=='bias' else (-32768,32767))
        integer=x.round().clamp(lo,hi)
        item=statistics(x.numpy())
        item.update(clamp_low=lo,clamp_high=hi,percent_at_low=float((integer==lo).float().mean()*100),
                    percent_at_high=float((integer==hi).float().mean()*100),
                    displacement_from_seeded_initialization=statistics((x-getattr(initial,name).detach()).numpy()))
        if name=='wdl_head':
            item['distinct_integer_weights']=int(integer.unique().numel())
            item['integer_sign_counts']=dict(negative=int((integer<0).sum()),zero=int((integer==0).sum()),positive=int((integer>0).sum()))
            item['integer_weights']=integer.tolist()
            item['conservative_evidence_q_range']=nonnegative_head_range(integer,model.value_divisor)
            if bool((integer>=0).all()):
                denominator=integer.sum(0)
                ratios=(integer[0]-integer[2])/denominator.clamp_min(1)
                item['pre_truncation_context_column_q_range']=[min(0.,float(ratios.min())),max(0.,float(ratios.max()))]
        result[name]=item
    return result


def gradient_report(model,rows,keys,centers,target,config):
    w,p,_=model(keys,centers);parts=components(w,p,target)
    cadence=config.get('mining_every',0)
    # A fixed representative batch; report the two conditional production cases.
    result={}
    for mining in ([False,True] if cadence else [False]):
        weight=weights(rows,w,p,target,config,mining,scale=model.score_scale)
        terms={name:(value*weight*(config['outcome_weight'] if name=='outcome' else 1.)).mean() for name,value in parts.items()}
        terms['value_total']=terms['teacher_wdl_ce']+terms['q_squared']+terms['outcome']
        terms['teacher_value_only']=terms['teacher_wdl_ce']+terms['q_squared']
        terms['policy_total']=terms['hard_policy_ce']+terms['soft_comparison']
        terms['total']=terms['value_total']+terms['policy_total']
        gradients={name:torch.autograd.grad(loss,tuple(model.parameters()),retain_graph=True,allow_unused=True)
                   for name,loss in terms.items()}
        norms={name:{family:float(g.norm()) if g is not None else 0. for family,g in zip(FAMILIES,gs,strict=True)}
               for name,gs in gradients.items()}
        alignment={}
        for family,a,b in zip(FAMILIES,gradients['value_total'],gradients['policy_total'],strict=True):
            an=float(a.norm()) if a is not None else 0.;bn=float(b.norm()) if b is not None else 0.
            alignment[family]=dict(policy_to_value_norm=bn/an if an else None,
                cosine=float((a*b).sum()/(a.norm()*b.norm())) if an and bn else None)
        result['mining' if mining else 'ordinary']=dict(norms=norms,value_policy_alignment=alignment)
    return result


def value_metrics(target,prediction):
    q=np.asarray(target);v=np.asarray(prediction);mask=np.abs(q)>=.05
    return dict(samples=len(q),value_mae=float(np.abs(v-q).mean()),
        value_corr=float(np.corrcoef(q,v)[0,1]) if len(q)>1 and q.std()>0 and v.std()>0 else None,
        sign_samples=int(mask.sum()),sign_accuracy=float((np.sign(q[mask])==np.sign(v[mask])).mean()) if mask.any() else None,
        target=statistics(q),prediction=statistics(v))


def pre_relu_evidence(model,keys,centers):
    """Read-only instrument of the unchanged production QAT value path."""
    local=(F.embedding(keys,ste_integer(model.embedding,-128,127)).sum(-2)
           +F.embedding(centers,ste_integer(model.center,-128,127))).clamp(0,255)
    groups=torch.stack([local.index_select(1,getattr(model,f'group_{g}')).sum(1) for g in range(4)],dim=1)
    pooled=torch.cat([ste_trunc(groups.sum(1)/225)]+[ste_trunc(groups[:,g]/COUNTS[g]) for g in range(4)],dim=-1)
    context=ste_trunc((F.linear(pooled,ste_integer(model.mixing,-128,127))+
                      ste_integer(model.bias,-1048576,1048576))/256).clamp(0,255)
    return ste_trunc(F.linear(context,ste_integer(model.wdl_head,-32768,32767))/model.value_divisor)


def diagnose(model,checkpoint,dataset,indices,split,*,count=2048,seed=17,batch_size=256,device='cpu',gradients=True):
    selected=sample_indices(indices,count,seed)
    if not selected:raise ValueError('empty diagnostic split')
    config=checkpoint.get('production') or checkpoint.get('source_production')
    if not config or config['policy_target']!='soft':raise ValueError('diagnostic components currently require soft production target')
    groups={};qs=[];predictions=[];probabilities=[];evidence=[];pre_relu=[];hits=[];losses={};first=None
    cadence=config.get('mining_every',0)
    for start in range(0,len(selected),batch_size):
        rows,k,c,t=batch_inputs(dataset,selected[start:start+batch_size],checkpoint['score_scale'],device)
        if first is None:first=(rows,k,c,t)
        with torch.no_grad():
            w,p,e=model(k,c);parts=components(w,p,t)
            raw=pre_relu_evidence(model,k,c)
            if not torch.equal(raw.relu()+1,e):raise ValueError('diagnostic evidence disagrees with production forward')
            pre_relu.extend(raw.cpu().tolist())
            ordinary=weights(rows,w,p,t,config,False,scale=model.score_scale)
            mined=weights(rows,w,p,t,config,True,scale=model.score_scale) if cadence else ordinary
            rank=p.masked_fill(~t['legal'],float('-inf')).argsort(dim=1,descending=True,stable=True)[:,:5].cpu().tolist()
            q=t['q'].cpu().tolist();v=(w[:,0]-w[:,2]).cpu().tolist()
            for name,values in parts.items():
                factor=config['outcome_weight'] if name=='outcome' else 1.
                entry=losses.setdefault(name,dict(raw_sum=0.,ordinary_sum=0.,mining_sum=0.))
                entry['raw_sum']+=float(values.double().sum())
                entry['ordinary_sum']+=float((values*ordinary*factor).double().sum())
                entry['mining_sum']+=float((values*mined*factor).double().sum())
            for j,row in enumerate(rows):
                r=row[0];index=len(qs)+j
                for group in ('all',f'source-{r.source}','exact' if r.exact else 'non-exact'):
                    groups.setdefault(group,[]).append(index)
                if row[3] is not None:hits.append([int(row[3] in rank[j][:n]) for n in (1,3,5)])
            qs.extend(q);predictions.extend(v);probabilities.extend(w.cpu().tolist());evidence.extend(e.cpu().tolist())
    n=len(selected)
    for entry in losses.values():
        entry.update(raw_mean=entry.pop('raw_sum')/n,weighted_ordinary_mean=entry.pop('ordinary_sum')/n,
                     weighted_mining_mean=entry.pop('mining_sum')/n)
        entry['weighted_cadence_mean']=entry['weighted_ordinary_mean']+(entry['weighted_mining_mean']-entry['weighted_ordinary_mean'])/cadence if cadence else entry['weighted_ordinary_mean']
    q=np.asarray(qs);v=np.asarray(predictions)
    report=dict(schema=1,split=split,sample_seed=seed,sample_count=n,
        sample_indices_sha256=hashlib.sha256(np.asarray(selected,dtype='<u8').tobytes()).hexdigest(),
        sample_rule='seeded-uniform-without-replacement-sorted-v1; canonical symmetry zero for evaluation',
        wdl_order=['win','draw','loss'],perspective='side-to-move',
        strata={name:value_metrics(q[ix],v[ix]) for name,ix in groups.items()},
        wdl={name:statistics(np.asarray(probabilities)[:,i]) for i,name in enumerate(('win','draw','loss'))},
        evidence={name:statistics(np.asarray(evidence)[:,i]) for i,name in enumerate(('win','draw','loss'))},
        pre_relu_evidence={name:dict(statistics=statistics(np.asarray(pre_relu)[:,i]),
            percent_nonpositive=float((np.asarray(pre_relu)[:,i]<=0).mean()*100)) for i,name in enumerate(('win','draw','loss'))},
        probability_concentration={str(threshold):float((np.asarray(probabilities).max(1)>threshold).mean()) for threshold in (.8,.9,.95)},
        policy_samples=len(hits),policy={f'top{k}':float(np.asarray(hits)[:,i].mean()) if hits else None for i,k in enumerate((1,3,5))},
        losses=losses,loss_denominator='all sampled records; absent components contribute zero',
        weight_configuration=config,weighted_cadence_note='conditional mining/no-mining weights on this fixed batch; cadence mean is diagnostic expectation, not a replay of historical minibatches')
    report['losses_and_gradients_objective']='reference production objective, including policy/outcome; diagnostic-only checkpoints report counterfactual contributions'
    if checkpoint.get('diagnostic',{}).get('kind')=='value-only-diagnostic-v1':
        report['actual_training_contributions']={name:(entry['weighted_cadence_mean'] if name in ('teacher_wdl_ce','q_squared') else 0.) for name,entry in losses.items()}
    if gradients:report['gradients']=gradient_report(model,*first,config)
    return report


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dataset',type=Path,required=True);parser.add_argument('--checkpoint',type=Path,required=True)
    parser.add_argument('--split',nargs='+',choices=('train','validation','test','opening_heldout'),default=['train','opening_heldout'])
    parser.add_argument('--samples',type=int,default=2048);parser.add_argument('--seed',type=int,default=17)
    parser.add_argument('--batch-size',type=int,default=256);parser.add_argument('--device',default='cpu')
    parser.add_argument('--output',type=Path,required=True);args=parser.parse_args()
    if args.output.exists():raise ValueError('diagnostic output already exists')
    checkpoint=load_checkpoint(args.checkpoint);model=model_from(checkpoint,args.device)
    seed=checkpoint.get('production',checkpoint.get('source_production',{}))['seed']
    initial=None
    if checkpoint.get('diagnostic',{}).get('head_init')=='small-positive':
        from train_value_only import initialize_model
        initial=initialize_model(checkpoint['score_scale'],seed,'small-positive')
    with open_dataset(args.dataset) as dataset:
        partitions=validate_split_manifest(dataset,checkpoint['split_manifest'])
        report=dict(checkpoint_sha256=file_hash(args.checkpoint),steps=checkpoint['steps'],
            parameters=parameter_report(model,seed,initial=initial),
            splits={name:diagnose(model,checkpoint,dataset,partitions[name],name,count=args.samples,seed=args.seed,
                batch_size=args.batch_size,device=args.device) for name in args.split})
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(report,indent=2,allow_nan=False)+'\n',encoding='utf-8')
    print(json.dumps({s:v['strata']['all'] for s,v in report['splits'].items()}))


if __name__=='__main__':main()
