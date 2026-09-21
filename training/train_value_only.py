"""Isolated <=5000-step V3 value experiment; never a production resume path."""
import argparse
import json
import time
from pathlib import Path

import torch
from checkpoint import atomic_save, load_checkpoint
from common import validate_split_manifest
from dataset import open_dataset, file_hash
from diagnose_value import batch_inputs, weights, diagnose, parameter_report, model_from
from mixlite import FORMAT
from mixlite_cache import FeatureCache
from mixlite_production import BatchedMixLite


def run(args):
    milestones=sorted(set(args.steps))
    if not milestones or milestones[0]<1 or milestones[-1]>5000:raise ValueError('diagnostic budget must be 1..5000 steps')
    resume=getattr(args,'resume',False)
    if args.output.exists() and not resume:raise ValueError('use a new isolated output directory or --resume')
    if resume and not (args.output/'experiment.json').is_file():raise ValueError('resume requires an existing diagnostic manifest')
    template=load_checkpoint(args.reference)
    if template.get('format')!=FORMAT or not template.get('production'):raise ValueError('reference must be a production V3 checkpoint')
    config=template['production']
    if config['sampler']!='block-shuffle-v1' or config['policy_target']!='soft':raise ValueError('unsupported reference training configuration')
    if args.device.startswith('cuda') and not torch.cuda.is_available():raise ValueError('CUDA unavailable')
    args.output.mkdir(parents=True,exist_ok=resume)
    identity=dict(schema=1,kind='value-only-diagnostic-v1',reference_sha256=file_hash(args.reference),
        dataset_sha256=template['split_manifest']['dataset_sha256'],milestones=milestones,seed=config['seed'],
        batch_size=config['batch_size'],learning_rate=config['learning_rate'],outcome_weight=0,
        preserved_weight_semantics='exact/sample weights and production hard-mining cadence; policy predictions only select detached mining weights',
        samples=args.samples,sample_seed=args.sample_seed)
    if resume:
        if json.loads((args.output/'experiment.json').read_text(encoding='utf-8'))!=identity:
            raise ValueError('diagnostic resume identity mismatch')
    else:
        (args.output/'experiment.json').write_text(json.dumps(identity,indent=2)+'\n',encoding='utf-8')
    print('Validating frozen dataset and split...',flush=True)
    with open_dataset(args.dataset) as dataset:
        print('Dataset loaded; validating split and qualified indices...',flush=True)
        partitions=validate_split_manifest(dataset,template['split_manifest'],config['seed'])
        indices=partitions['train']
        print(f'Frozen split admitted: {len(indices)} qualified training records',flush=True)
        def evaluate(checkpoint,model,label,checkpoint_path):
            report_path=args.output/f'{label}.json'
            digest=file_hash(checkpoint_path)
            if report_path.exists():
                saved=json.loads(report_path.read_text(encoding='utf-8'))
                if saved.get('experiment')!=identity or saved.get('checkpoint_sha256')!=digest:
                    raise ValueError('existing diagnostic report identity mismatch')
                print(f'Reusing completed report: {label}',flush=True);return
            report=dict(experiment=identity,checkpoint_sha256=digest,steps=checkpoint['steps'],parameters=parameter_report(model,config['seed']),splits={})
            for split in ('train','opening_heldout'):
                report['splits'][split]=diagnose(model,checkpoint,dataset,partitions[split],split,
                    count=args.samples,seed=args.sample_seed,batch_size=config['batch_size'],device=args.device)
                print(label,split,json.dumps(report['splits'][split]['strata']['all']),flush=True)
            temporary=report_path.with_suffix('.json.partial')
            temporary.write_text(json.dumps(report,indent=2,allow_nan=False)+'\n',encoding='utf-8')
            temporary.replace(report_path)
        if not args.skip_reference_diagnostics:
            evaluate(template,model_from(template,args.device),'reference',args.reference)
            for number,path in enumerate(args.baseline):
                checkpoint=load_checkpoint(path)
                if checkpoint['split_manifest']!=template['split_manifest'] or checkpoint['score_scale']!=template['score_scale']:
                    raise ValueError('baseline split/target scale mismatch')
                evaluate(checkpoint,model_from(checkpoint,args.device),f'baseline-{number}-step-{checkpoint["steps"]}',path)
        # Reset after diagnostic model constructors so initialization is exactly
        # the production seed. The optimizer is fresh, never restored from template.
        torch.manual_seed(config['seed'])
        model=BatchedMixLite(template['score_scale']).float().to(args.device)
        optimizer=torch.optim.Adam(model.parameters(),lr=config['learning_rate'])
        cache=FeatureCache(args.cache_dir or args.output/'feature-cache',dataset,template['split_manifest'],indices)
        try:
            completed=epoch=cursor=0;started=time.perf_counter()
            latest=args.output/'latest.pt'
            if resume and latest.exists():
                restored=load_checkpoint(latest)
                if restored.get('diagnostic')!=identity or restored['split_manifest']!=template['split_manifest']:
                    raise ValueError('diagnostic checkpoint identity mismatch')
                model.load_state_dict(restored['model_state']);optimizer.load_state_dict(restored['optimizer_state'])
                completed,epoch,cursor=restored['steps'],restored['epoch'],restored['cursor']
                if not 0<=completed<=milestones[-1] or not 0<=cursor<=len(indices) or epoch<0:
                    raise ValueError('invalid diagnostic resume cursor')
                print(f'Resuming diagnostic step {completed}',flush=True)
                if completed in milestones:
                    milestone=args.output/f'step-{completed}.pt'
                    if not milestone.exists():atomic_save(milestone,restored)
                    evaluate(restored,model,f'step-{completed}',milestone)
            while completed<milestones[-1]:
                generator=torch.Generator().manual_seed(config['seed']+epoch)
                block_size=max(config['batch_size'],1024);order=[]
                for block in torch.randperm((len(indices)+block_size-1)//block_size,generator=generator).tolist():
                    start=block*block_size
                    order.extend(start+i for i in torch.randperm(min(block_size,len(indices)-start),generator=generator).tolist())
                while cursor<len(order) and completed<milestones[-1]:
                    local=order[cursor:cursor+config['batch_size']]
                    actual=[indices[i] for i in local]
                    symmetries=[(config['seed']+epoch+i)%8 for i in actual]
                    rows,k,c,t=batch_inputs(dataset,actual,template['score_scale'],args.device,symmetries,cache.batch(local,symmetries))
                    w,p,_=model(k,c)
                    mining=config.get('mining_every',0)>0 and completed%config['mining_every']==0
                    weight=weights(rows,w,p,t,config,mining,scale=model.score_scale)
                    # No policy loss and no outcome loss. Policy is used only as
                    # detached input to the unchanged optional hard-mining weights.
                    loss=(-(t['wdl']*w.log()).sum(1)+(w[:,0]-w[:,2]-t['q']).square())
                    loss=(loss*weight).mean()
                    optimizer.zero_grad(set_to_none=True);loss.backward();optimizer.step()
                    completed+=1;cursor+=len(local)
                    if completed%100==0:
                        if not torch.isfinite(loss):raise ValueError('nonfinite diagnostic loss')
                        print(json.dumps(dict(step=completed,loss=float(loss.detach()),elapsed_seconds=time.perf_counter()-started)),flush=True)
                    if completed%100==0 or completed in milestones:
                        checkpoint=dict(format=FORMAT,model_state=model.state_dict(),optimizer_state=optimizer.state_dict(),
                            score_scale=template['score_scale'],split_manifest=template['split_manifest'],
                            source_production=config,diagnostic=identity,steps=completed,epoch=epoch,cursor=cursor,
                            configuration=dict(template['configuration'],training='diagnostic-value-only-v1'))
                        atomic_save(latest,checkpoint)
                        if completed in milestones:
                            milestone=args.output/f'step-{completed}.pt'
                            atomic_save(milestone,checkpoint)
                            evaluate(checkpoint,model,f'step-{completed}',milestone)
                epoch+=1;cursor=0
        finally:cache.close()
    if file_hash(args.reference)!=identity['reference_sha256']:raise ValueError('reference checkpoint changed externally')


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--dataset',type=Path,required=True);p.add_argument('--reference',type=Path,required=True)
    p.add_argument('--baseline',type=Path,action='append',default=[])
    p.add_argument('--output',type=Path,required=True);p.add_argument('--cache-dir',type=Path)
    p.add_argument('--device',default='cuda');p.add_argument('--steps',type=int,nargs='+',default=[2000,5000])
    p.add_argument('--samples',type=int,default=2048);p.add_argument('--sample-seed',type=int,default=17)
    p.add_argument('--skip-reference-diagnostics',action='store_true')
    p.add_argument('--resume',action='store_true',help='resume only this isolated directory with identical inputs/configuration')
    run(p.parse_args())


if __name__=='__main__':main()
