"""Batched soft losses; the production module retains its old loss as oracle."""
import torch
from torch.nn import functional as F
from common import transform_index
from teacher import validate_comparison


def targets(batch,scale):
    q=[];wdl=[];outcomes=[];outcome_mask=[];hard=[];legal=[];observed=[];probabilities=[];sample=[]
    for record,board,_,move,symmetry,_,_ in batch:
        value=float((record.value>0)-(record.value<0)) if record.exact else record.value/(scale+abs(record.value))
        q.append(value);wdl.append([max(value,0),1-abs(value),max(-value,0)])
        outcome=record.outcome
        outcomes.append([max(outcome,0),1-abs(outcome),max(-outcome,0)] if outcome is not None else [0.,0.,0.])
        outcome_mask.append(outcome is not None and not record.exact)
        mask=[c==0 for c in board];legal.append(mask)
        if move is not None and not mask[move]:raise ValueError('illegal policy target')
        hard.append(-100 if move is None else move)
        comparison=validate_comparison(record.comparison) if record.comparison is not None and not record.exact else None
        cells=[transform_index(at,symmetry) for at in comparison['moves']] if comparison else []
        if any(not mask[at] for at in cells):raise ValueError('illegal comparison target')
        observed.append(cells);probabilities.append(comparison['probabilities'] if comparison else [])
        sample.append(record.sample_weight)
    width=max(1,max(map(len,observed)))
    indices=[row+[0]*(width-len(row)) for row in observed]
    mask=[[True]*len(row)+[False]*(width-len(row)) if row else [True]+[False]*(width-1) for row in observed]
    probability=[row+[0.]*(width-len(row)) for row in probabilities]
    return dict(q=torch.tensor(q,dtype=torch.float32),wdl=torch.tensor(wdl,dtype=torch.float32),
        outcomes=torch.tensor(outcomes,dtype=torch.float32),outcome_mask=torch.tensor(outcome_mask),
        hard=torch.tensor(hard),legal=torch.tensor(legal),indices=torch.tensor(indices),
        observed=torch.tensor(mask),probabilities=torch.tensor(probability,dtype=torch.float32),
        sample=torch.tensor(sample,dtype=torch.float32))


def soft_loss(wdl,policy,target,weights,outcome_weight):
    log=wdl.log()
    losses=-(target['wdl']*log).sum(1)+(wdl[:,0]-wdl[:,2]-target['q']).square()
    losses=losses+outcome_weight*(-(target['outcomes']*log).sum(1))*target['outcome_mask']
    hard_logits=policy.masked_fill(~target['legal'],float('-inf'))
    hard_logits=torch.where((target['hard']!=-100)[:,None],hard_logits,torch.zeros_like(hard_logits))
    losses=losses+F.cross_entropy(hard_logits,target['hard'],reduction='none',ignore_index=-100)
    selected=policy.gather(1,target['indices']).masked_fill(~target['observed'],float('-inf'))
    log_policy=selected.log_softmax(1).masked_fill(~target['observed'],0)
    losses=losses-(target['probabilities']*log_policy).sum(1)
    return (losses*weights*target['sample']).mean()
