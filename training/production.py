"""Production orchestration: generate or reuse data, batched QAT, common evidence.
Never runs Arena or promotes a model. Presets are configuration, not scale caps.
"""
import argparse
import json
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace
from dataset import file_hash, open_dataset
from manifest import save_manifest
from generate import generate
from mixlite_production import train
from mixlite import export
from verify_integer import verify


def run(args):
    config=json.loads(args.config.read_text(encoding='utf-8'))
    if config.pop('schema') != 1: raise ValueError('unsupported production config')
    config.pop('preset',None)
    for name in ('batch_size','games','steps','epochs'):
        value=getattr(args,name,None)
        if value is not None: config[name]=value
    output=args.output.resolve(); output.mkdir(parents=True,exist_ok=True)
    if args.dataset:
        dataset=args.dataset.resolve()
        with open_dataset(dataset) as data:
            if args.teacher_model or args.teacher_profile:
                if not hasattr(data,'descriptor'): raise ValueError('teacher identity requires a dataset descriptor')
                for shard in data.descriptor['shards']:
                    teacher=shard['teacher']
                    if args.teacher_model and teacher.get('model_sha256') != file_hash(args.teacher_model): raise ValueError('teacher model mismatch')
                    if args.teacher_profile and teacher.get('profile') != args.teacher_profile: raise ValueError('teacher profile mismatch')
    else:
        generation={key:config[key] for key in ('games','workers','shard_games','depth','nodes','timeout','random_plies','explore_top_k','explore_temperature','explore_plies')}
        dataset=generate(SimpleNamespace(**generation,engine=args.engine,output=output/'data',seed=args.seed,
                                        model=args.teacher_model,profile=args.teacher_profile))
    # Immutable run identity binds data, teacher inputs, seed and training configuration.
    identity=dict(schema=1,config=config,seed=args.seed,device=args.device,dataset=str(dataset),dataset_sha256=file_hash(dataset),
                  teacher_model_sha256=file_hash(args.teacher_model) if args.teacher_model else None,teacher_profile=args.teacher_profile)
    save_manifest(output/'production.json',identity)
    checkpoint=output/'checkpoint.pt'
    completed=train(dataset,checkpoint,steps=config['steps'],epochs=config['epochs'],batch_size=config['batch_size'],
        device=args.device,seed=args.seed,resume=args.resume,checkpoint_every=config['checkpoint_every'],
        learning_rate=config['learning_rate'],policy_target=config['policy_target'])
    model=output/('model-'+file_hash(checkpoint)[:16]+'.rmlp')
    export(checkpoint,dataset,model)
    subprocess.run([sys.executable,str(Path(__file__).with_name('calibrate.py')),'--dataset',str(dataset),
                    '--checkpoint',str(checkpoint),'--model',str(model)],check=True)
    verify(args.engine.resolve(),model)
    print(json.dumps(dict(model=str(model),steps=completed,status='evidence-ready-not-promoted')))
    return model


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--config',type=Path,required=True); p.add_argument('--output',type=Path,required=True)
    p.add_argument('--engine',type=Path,required=True); p.add_argument('--dataset',type=Path)
    p.add_argument('--device',default='cpu'); p.add_argument('--batch-size',type=int); p.add_argument('--resume',type=Path)
    p.add_argument('--seed',type=int,default=1); p.add_argument('--teacher-model',type=Path); p.add_argument('--teacher-profile')
    for name in ('games','steps','epochs'): p.add_argument('--'+name,type=int)
    run(p.parse_args())
if __name__=='__main__': main()
