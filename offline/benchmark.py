"""Short disk/native-boundary diagnostic, never proof or strength evidence."""
import argparse
import json
import tempfile
from pathlib import Path
from offline.native import NativeFacts
from offline.storage import DiskNodes
from offline.solve import DiskSolver

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine',required=True)
    parser.add_argument('--expansions',type=int,default=16)
    args=parser.parse_args()
    if not 1<=args.expansions<=256:raise ValueError('diagnostic accepts 1..256 expansions')
    with tempfile.TemporaryDirectory(prefix='rustmoku-offline-micro-') as directory:
        with NativeFacts(args.engine) as native, DiskNodes(Path(directory)/'nodes.db',{},cache_bytes=1024*1024,max_disk_bytes=32*1024*1024) as db:
            with db.transaction():root=db.intern(bytes.fromhex(native.facts(b'')['key']),b'')
            solver=DiskSolver(db,native,0)
            result=solver.solve(root,work=args.expansions,seconds=10)
            result['statistics']=solver.telemetry()
            result['diagnostic']='empty-freestyle-root; bounded disk PN; no strength inference'
            print(json.dumps(result,sort_keys=True))

if __name__=='__main__':main()
