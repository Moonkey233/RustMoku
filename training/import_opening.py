"""Export empirical OpeningDatabase examples with frozen source lineage."""
import argparse
import subprocess
import tempfile
from pathlib import Path
from common import save_split_manifest
from dataset import DatasetBundle, describe_shard, file_hash, publish_shard

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--engine',type=Path,required=True);p.add_argument('--opening-db',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True);p.add_argument('--max-positions',type=int,default=1000)
    args=p.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    book_hash=file_hash(args.opening_db);engine_hash=file_hash(args.engine)
    teacher={'source':'empirical-opening-database','opening_db_sha256':book_hash,'executable_sha256':engine_hash,'exact':False}
    save_split_manifest(args.output/'import.json',{'version':1,'teacher':teacher,'max_positions':args.max_positions})
    shard=args.output/'opening.rmd'
    with tempfile.TemporaryDirectory(dir=args.output) as directory:
        staged=Path(directory)/'opening.rmd'
        subprocess.run([str(args.engine.resolve()),'opening','--opening-db',str(args.opening_db.resolve()),'--output',str(staged),'--max-positions',str(args.max_positions)],check=True)
        if file_hash(args.opening_db)!=book_hash or file_hash(args.engine)!=engine_hash:raise ValueError('opening import input changed')
        publish_shard(staged,shard)
    description=describe_shard(shard,teacher,book_hash,compact=True)
    for meta in description['games'].values():
        # One generation tree stays in one split, even when sampled at many nodes.
        meta.update(lineage_id='opening:'+book_hash,parent_lineage_id='opening:'+book_hash,
                    opening_family='opening:'+book_hash)
    save_split_manifest(args.output/'dataset.json',{'version':2,'shards':[description]})
    with DatasetBundle(args.output/'dataset.json') as data:
        if any(r.exact for r in data):raise ValueError('opening import unexpectedly contains exact labels')
    print(args.output/'dataset.json')

if __name__=='__main__':main()
