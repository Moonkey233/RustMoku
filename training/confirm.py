"""Run only an explicitly configured independent fixed-time confirmation suite."""
import argparse
import json
import sys
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'apps/rustmoku-arena'))
from experiment import run


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--config',type=Path,required=True); p.add_argument('--output',type=Path,required=True)
    args=p.parse_args(); config=json.loads(args.config.read_text(encoding='utf-8'))
    if config.get('suite_role') != 'confirmation' or config.get('stop_rule') != 'fixed_pairs':
        raise ValueError('confirmation requires independent confirmation openings and fixed_pairs')
    arguments=config.get('arguments',[])
    if '--opening-record' not in arguments or not any(flag in arguments for flag in ('--move-ms','--clock-ms')):
        raise ValueError('supply an explicit independent opening suite and time control')
    run(config,args.output)
if __name__=='__main__': main()
