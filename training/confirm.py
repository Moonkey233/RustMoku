"""Run only an explicitly configured independent fixed-time confirmation suite."""
import argparse
import json
import sys
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'apps/rustmoku-arena'))
from experiment import run, describe
from manifest import read_manifest


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--config',type=Path,required=True); p.add_argument('--output',type=Path,required=True)
    args=p.parse_args(); config=json.loads(args.config.read_text(encoding='utf-8'))
    if config.get('suite_role') != 'confirmation' or config.get('stop_rule') != 'fixed_pairs':
        raise ValueError('confirmation requires independent confirmation openings and fixed_pairs')
    arguments=config.get('arguments',[])
    if '--nodes' in arguments or '--opening-record' not in arguments or not any(flag in arguments for flag in ('--move-ms','--clock-ms')):
        raise ValueError('supply an explicit independent opening suite and time control')
    if config.get('tuning_manifest'):
        path=Path(config['tuning_manifest']).resolve()
        tuning=read_manifest(path)
        effective=describe(Path(config['arena']),arguments)
        keys=set(effective['openings'])
        if keys & set(tuning['tuning_keys']) or not keys <= set(tuning['confirmation_keys']):
            raise ValueError('confirmation openings differ from the independently frozen tuning holdout')
        config['extra_inputs']=[*config.get('extra_inputs',[]),str(path)]
    run(config,args.output)
if __name__=='__main__': main()
