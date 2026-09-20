"""Bounded before/after benchmark from one immutable checkpoint snapshot."""
import argparse
import json
import shutil
from pathlib import Path
import torch

from checkpoint import load_checkpoint
from dataset import file_hash
from mixlite_production import train


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dataset',type=Path,required=True)
    parser.add_argument('--resume',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--steps',type=int,default=100)
    parser.add_argument('--device',default='cuda')
    parser.add_argument('--mode',choices=('reference','cache-only','optimized'),required=True)
    args=parser.parse_args()
    if not 1<=args.steps<=200:parser.error('benchmark steps must be in 1..200')
    args.output.mkdir(parents=True,exist_ok=True)
    snapshot=args.output/'input.pt'
    source_hash=file_hash(args.resume)
    if snapshot.exists():
        if file_hash(snapshot)!=source_hash:raise ValueError('benchmark snapshot differs from source checkpoint')
    else:shutil.copyfile(args.resume,snapshot)
    checkpoint=load_checkpoint(snapshot)
    config=dict(checkpoint['production']);config.pop('sampler')
    destination=args.output/f'{args.mode}.pt'
    report=args.output/f'{args.mode}.json'
    if destination.exists() or report.exists():raise ValueError('benchmark outputs already exist')
    print(json.dumps(dict(mode=args.mode,input_sha256=source_hash,start_steps=checkpoint['steps'])),flush=True)
    train(args.dataset,destination,steps=args.steps,epochs=checkpoint['epoch']+2,
          device=args.device,resume=snapshot,checkpoint_every=100,
          cache_dir=args.output/'feature-cache',reference_path=args.mode=='reference',
          reference_loss_path=args.mode=='cache-only',
          timing_output=report,**config)
    if file_hash(args.resume)!=source_hash:raise ValueError('source checkpoint changed during benchmark')
    result=json.loads(report.read_text(encoding='utf-8'))
    result.update(input_sha256=source_hash,mode=args.mode,device=args.device,
                  torch_version=str(torch.__version__),production=checkpoint['production'],
                  dataset_sha256=checkpoint['split_manifest']['dataset_sha256'],
                  gpu=torch.cuda.get_device_name() if args.device.startswith('cuda') else None)
    report.write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8')


if __name__=='__main__':main()
