"""Four isolated WDL-head arms, sharing one validated read-only corpus admission."""
import argparse
import copy
import json
import time
from pathlib import Path

from checkpoint import load_checkpoint
from common import validate_split_manifest
from dataset import open_dataset, file_hash
from train_value_only import run, json_hash

ARMS={'A':('production',1),'B':('small-positive',1),
      'C':('production',16),'D':('small-positive',16)}


def inventory(dataset,reference):
    descriptor=json.loads(dataset.read_text(encoding='utf-8'))
    paths=[dataset,reference]
    for shard in descriptor['shards']:
        path=Path(shard['path'])
        paths.append(path if path.is_absolute() else dataset.parent/path)
    if descriptor.get('comparisons'):paths.append(Path(descriptor['comparisons']['path']))
    return {str(p.resolve()):file_hash(p) for p in paths}


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--dataset',type=Path,required=True)
    p.add_argument('--reference',type=Path,required=True)
    p.add_argument('--cache-dir',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True)
    p.add_argument('--device',default='cuda')
    p.add_argument('--resume',action='store_true')
    args=p.parse_args()
    if args.output.exists() and not args.resume:raise ValueError('output exists; use explicit --resume')
    args.output.mkdir(parents=True,exist_ok=args.resume)
    before=inventory(args.dataset,args.reference)
    identity=dict(schema=1,arms=ARMS,inputs=before,device=args.device,steps=[2000,5000],samples=2048,sample_seed=17)
    identity=json.loads(json.dumps(identity))
    manifest=args.output/'matrix.json'
    if args.resume:
        if json.loads(manifest.read_text())!=identity:raise ValueError('matrix resume identity mismatch')
    else:manifest.write_text(json.dumps(identity,indent=2)+'\n',encoding='utf-8')
    print('Admitting frozen corpus once for all four arms...',flush=True)
    started=time.perf_counter();reference=load_checkpoint(args.reference)
    if reference['production']['seed']!=17 or reference['production']['batch_size']!=256 or reference['production']['learning_rate']!=.001:
        raise ValueError('experiment requires seed 17, batch 256, LR .001')
    with open_dataset(args.dataset) as dataset:
        partitions=validate_split_manifest(dataset,reference['split_manifest'],17)
        print('Corpus admitted; starting independent arms',flush=True)
        for label,(init,multiplier) in ARMS.items():
            arm=copy.copy(args);arm.output=args.output/label
            arm.resume=args.resume and arm.output.exists()
            arm.steps=[2000,5000];arm.samples=2048;arm.sample_seed=17
            arm.baseline=[];arm.skip_reference_diagnostics=True
            arm.head_init=init;arm.head_lr_multiplier=multiplier;arm.head_experiment=True
            print(f'ARM {label}: {init}, head LR {multiplier}x',flush=True)
            try:run(arm,admitted=(dataset,partitions))
            except FloatingPointError as error:
                (arm.output/'failure.json').write_text(json.dumps(dict(nonfinite_event=str(error),arm=label))+'\n')
                print(f'ARM {label} FAILED: {error}',flush=True)
    after=inventory(args.dataset,args.reference)
    if before!=after:raise ValueError('original input bytes changed during experiment')
    receipt=dict(inputs_unchanged=True,input_inventory_sha256=json_hash(before),wall_seconds=time.perf_counter()-started)
    (args.output/'completion.json').write_text(json.dumps(receipt,indent=2)+'\n',encoding='utf-8')
    print('MATRIX COMPLETE',json.dumps(receipt),flush=True)


if __name__=='__main__':main()
