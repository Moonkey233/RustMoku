"""Deterministic, copyable frontier jobs. No shared mutable PN tree.

Workers have independent databases. Merge replays each result before copying its
witness; Unknown artifacts are resumable work, never parent disproofs.
"""
import argparse
import concurrent.futures
import hashlib
import json
from pathlib import Path
from offline.native import NativeFacts, fingerprint
from offline.solve import DiskSolver
from offline.storage import DiskNodes, ResourceStop

from offline.artifacts import read_json, publish

def frontier(db,output,*,shards,limit=256):
    if not 1<=shards<=256 or not 1<=limit<=4096:raise ValueError('invalid shard/frontier limit')
    config=json.loads(db.connection.execute('SELECT config FROM metadata WHERE id=1').fetchone()[0])
    jobs=[]
    for key,moves in db.connection.execute('SELECT key,moves FROM nodes WHERE outcome=0 AND expanded=0 ORDER BY key LIMIT ?',(limit,)):
        job_id=hashlib.sha256(key).hexdigest()
        jobs.append({'id':job_id,'key':key.hex(),'moves':moves.hex(),'shard':int(job_id,16)%shards})
    manifest={'version':1,'protocol':'rustmoku-exact-frontier-v1','config':config,'shards':shards,'jobs':jobs}
    publish(output,manifest);return manifest

def validate(manifest,engine):
    if manifest.get('version')!=1 or manifest.get('protocol')!='rustmoku-exact-frontier-v1':raise ValueError('unsupported shard protocol')
    config=manifest['config']
    if config.get('version')!=2 or 'root_ply' not in config or 'max_additional_plies' not in config:raise ValueError('shards require relative-depth configuration v2')
    if config['engine_sha256']!=fingerprint(engine) or config['attacker'] not in (0,1) or config['rules']!='freestyle':raise ValueError('incompatible frozen shard engine/rules')
    jobs=manifest['jobs'];shards=manifest['shards']
    if not 1<=shards<=256 or len(jobs)>4096:raise ValueError('invalid shard counts')
    seen=set()
    for job in jobs:
        key=bytes.fromhex(job['key']);moves=bytes.fromhex(job['moves'])
        identity=hashlib.sha256(key).hexdigest()
        if len(key)!=58 or len(moves)>225 or job['id']!=identity or job['id'] in seen or job['shard']!=int(identity,16)%shards:raise ValueError('invalid shard identity')
        seen.add(identity)
    return config

def worker_config(manifest,job):
    return {**manifest['config'],'moves':job['moves'],'frontier_job':job['id']}

def solve_batch(manifest,engine,output,*,shard,workers,work,seconds,cache_bytes,max_disk_bytes,resume=False):
    config=validate(manifest,engine)
    if not 0<=shard<manifest['shards'] or not 1<=workers<=64:raise ValueError('invalid worker selection')
    output=Path(output);output.mkdir(parents=True,exist_ok=True)
    jobs=sorted((job for job in manifest['jobs'] if job['shard']==shard),key=lambda j:j['key'])
    def run(job):
        path=output/(job['id']+'.db');artifact=output/(job['id']+'.json')
        # Per-job budgets; multiplying worker count never disguises total cost.
        try:
            with NativeFacts(engine) as native, DiskNodes(path,worker_config(manifest,job),cache_bytes=cache_bytes,max_disk_bytes=max_disk_bytes,resume=resume and path.exists()) as db:
                moves=bytes.fromhex(job['moves']);facts=native.facts(moves)
                if facts['key']!=job['key']:raise ValueError('shard replay/key mismatch')
                with db.transaction():root=db.intern(bytes.fromhex(job['key']),moves)
                solver=DiskSolver(db,native,config['attacker'])
                result=solver.solve(root,work=work,seconds=seconds,max_additional_plies=config['max_additional_plies'])
                if result['outcome']!='Unknown':result['outcome']=solver.verify_witness(root,max_nodes=work,seconds=seconds)
                result['statistics']=solver.telemetry()
        except ResourceStop as stop:result={'outcome':'Unknown','reason':str(stop)}
        result['resources']={'sqlite_cache_bytes':cache_bytes,'disk_budget_bytes':max_disk_bytes,'work_budget':work,
            'wall_budget':seconds,'scope':'per-job-search; separate bounded witness verification','rss_enforced':False}
        value={'version':1,'job':job,'config':worker_config(manifest,job),'result':result}
        publish(artifact,value);return value
    # Each thread owns a separate native process and SQLite connection.
    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
        return list(pool.map(run,jobs))

def merge(db,manifest,engine,directories,*,work,seconds,cache_bytes,max_disk_bytes):
    config=validate(manifest,engine)
    current=json.loads(db.connection.execute('SELECT config FROM metadata WHERE id=1').fetchone()[0])
    if current!=config:raise ValueError('frontier belongs to a different coordinator')
    directories=sorted({Path(path).resolve() for path in directories})
    merged=0
    with NativeFacts(engine) as native:
        coordinator=DiskSolver(db,native,config['attacker'])
        for job in sorted(manifest['jobs'],key=lambda j:j['key']):
            matches=[path for path in directories if (path/(job['id']+'.json')).exists()]
            if len(matches)>1:raise ValueError('duplicate worker artifact; choose one checkpoint explicitly')
            if not matches:continue
            directory=matches[0];receipt=read_json(directory/(job['id']+'.json'))
            if receipt.get('version')!=1 or receipt.get('job')!=job or receipt.get('config')!=worker_config(manifest,job):raise ValueError('incompatible worker artifact')
            if receipt['result']['outcome']=='Unknown':continue
            with DiskNodes(directory/(job['id']+'.db'),worker_config(manifest,job),cache_bytes=cache_bytes,max_disk_bytes=max_disk_bytes,resume=True) as source:
                row=source.connection.execute('SELECT id FROM nodes WHERE key=?',(bytes.fromhex(job['key']),)).fetchone()
                if row is None:raise ValueError('worker omits root')
                checked=DiskSolver(source,native,config['attacker']).verify_witness(row[0],max_nodes=work,seconds=seconds)
                if checked!=receipt['result']['outcome']:raise ValueError('worker claim differs from fresh verification')
                # Stream only the independently replayed witness. Unknown siblings
                # are included as edges, but their unverified flags are not copied.
                with db.transaction():
                    for source_id,outcome in source.connection.execute('SELECT id,outcome FROM certificates ORDER BY id'):
                        node=source.node(source_id);target=db.intern(node.key,node.moves)
                        existing=db.node(target)
                        if existing.outcome not in (0,outcome):
                            raise ValueError('conflicting coordinator/shard witness')
                        edges=[]
                        for move,child_id in source.children(source_id):
                            child=source.node(child_id);edges.append((move,db.intern(child.key,child.moves)))
                        if node.expanded:db.expand(target,edges)
                        coordinator.result(target,outcome,0 if outcome==1 else (1<<62)-1,0 if outcome==2 else (1<<62)-1,evidence=node.evidence)
                    coordinator.propagate()
                merged+=1
    return merged

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('command',choices=['frontier','solve-batch','merge'])
    p.add_argument('--engine',required=True);p.add_argument('--manifest',required=True)
    p.add_argument('--checkpoint');p.add_argument('--config',help='frozen coordinator config JSON')
    p.add_argument('--output');p.add_argument('--inputs',nargs='+');p.add_argument('--resume',action='store_true')
    p.add_argument('--shards',type=int,default=1);p.add_argument('--shard',type=int,default=0);p.add_argument('--workers',type=int,default=1)
    p.add_argument('--limit',type=int,default=256);p.add_argument('--nodes',type=int,default=1000);p.add_argument('--seconds',type=float,default=60)
    p.add_argument('--sqlite-cache-mib','--ram-mib',dest='sqlite_cache_mib',type=int,default=32);p.add_argument('--disk-mib',type=int,default=1024)
    args=p.parse_args();options={'cache_bytes':args.sqlite_cache_mib*1024*1024,'max_disk_bytes':args.disk_mib*1024*1024}
    try:
        if args.command=='solve-batch':
            if not args.output:raise ValueError('--output required')
            solve_batch(read_json(args.manifest),args.engine,args.output,shard=args.shard,workers=args.workers,work=args.nodes,seconds=args.seconds,resume=args.resume,**options)
        else:
            if not args.checkpoint:raise ValueError('--checkpoint required')
            manifest=read_json(args.manifest) if args.command=='merge' else None
            config=manifest['config'] if manifest else read_json(args.config)
            with DiskNodes(args.checkpoint,config,resume=True,**options) as db:
                if manifest:print(json.dumps({'merged':merge(db,manifest,args.engine,args.inputs or [],work=args.nodes,seconds=args.seconds,**options)}))
                else:frontier(db,args.manifest,shards=args.shards,limit=args.limit)
    except ResourceStop as stop:print(json.dumps({'outcome':'Unknown','reason':str(stop)}))

if __name__=='__main__':main()
