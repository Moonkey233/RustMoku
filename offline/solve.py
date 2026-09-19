"""Disk PN baseline. Working outcomes are untrusted until witness replay/export.

All legal edges are persisted before a node is aggregated. Resident Python state
is bounded by board depth and one legal move list, not total explored nodes.
The existing native solver remains the tactical accelerator/reference path.
"""
import argparse
import json
import os
import struct
import subprocess
import time
import tempfile
from pathlib import Path
from offline.native import NativeFacts, fingerprint
from offline.artifacts import freeze
from offline.storage import DiskNodes, INFINITY, ResourceStop

def _numbers(outcome):
    return (0,INFINITY) if outcome==1 else (INFINITY,0) if outcome==2 else (1,1)

def leaf_entry(book_hex,attacker,key):
    book=bytes.fromhex(book_hex)
    if len(book)>4096 or len(book)<80 or book[:8]!=b'RMPBOOK1' or struct.unpack_from('<HBII',book,8)!=(1,0,1,1):raise ValueError('invalid single-root leaf book')
    count=struct.unpack_from('<H',book,20)[0]
    offset=80+count
    if count>225 or book[19]!=attacker or book[22+count:offset]!=key:raise ValueError('leaf root identity mismatch')
    entry=book[offset:]
    if len(entry)<62 or entry[0]!=attacker or entry[1:59]!=key:raise ValueError('leaf entry identity mismatch')
    return entry

class DiskSolver:
    def __init__(self,db,native,attacker):
        self.db=db;self.native=native;self.attacker=attacker;self.work=0
        config=json.loads(db.connection.execute('SELECT config FROM metadata WHERE id=1').fetchone()[0])
        self.root_ply=config.get('root_ply')
        self.max_additional_plies=config.get('max_additional_plies',225)
        self.witness_nodes=0;self.started=time.perf_counter()
        self.witness_tactical_work=0
        self.leaf_limits=config.get('leaf_limits')
        self.pn_expansions=0
        self.leaf_statistics=dict(immediate_hits=0,vcf_attempts=0,vcf_proven=0,vcf_work=0,vct_attempts=0,vct_proven=0,vct_work=0)
    def telemetry(self):
        return {**self.native.statistics,**self.leaf_statistics,'pn_expansions':self.pn_expansions,'sqlite_transactions':self.db.transactions,'witness_verification_nodes':self.witness_nodes,'witness_tactical_work':self.witness_tactical_work,'wall_seconds':time.perf_counter()-self.started}
    def replay(self,node):
        facts=self.native.facts(node.moves)
        if bytes.fromhex(facts['key'])!=node.key:raise ValueError('working node does not match native replay')
        return facts
    def effective_leaf_limits(self,node):
        limits=dict(self.leaf_limits)
        remaining=self.max_additional_plies-(len(node.moves)-self.root_ply)
        for key in ('vcf_plies','vct_plies'):limits[key]=min(limits[key],max(0,remaining))
        return limits
    def result(self,id,outcome,pn,dn,evidence=b''):
        old=self.db.node(id)
        if (old.outcome,old.pn,old.dn,old.evidence)==(outcome,pn,dn,evidence):return
        self.db.working_result(id,outcome=outcome,pn=pn,dn=dn,evidence=evidence)
        self.db.connection.execute('INSERT OR IGNORE INTO dirty SELECT parent FROM edges WHERE child=?',(id,))
    def aggregate(self,id):
        node=self.db.node(id)
        if not node.expanded:return
        children=[self.db.node(child) for _,child in self.db.children(id)]
        if any(len(child.moves)!=len(node.moves)+1 for child in children):raise ValueError('invalid child depth/cycle')
        if not children:return  # Terminal facts were assigned during expansion.
        if node.key[-1]==self.attacker:
            pn=min(child.pn for child in children);dn=min(INFINITY,sum(child.dn for child in children))
        else:
            pn=min(INFINITY,sum(child.pn for child in children));dn=min(child.dn for child in children)
        outcome=1 if pn==0 else 2 if dn==0 else 0
        self.result(id,outcome,pn,dn)
    def propagate(self):
        while row:=self.db.connection.execute('SELECT id FROM dirty ORDER BY id LIMIT 1').fetchone():
            self.db.connection.execute('DELETE FROM dirty WHERE id=?',row)
            self.aggregate(row[0])
    def expand(self,id,remaining):
        node=self.db.node(id)
        if self.leaf_limits is not None:
            limits=self.effective_leaf_limits(node)
            reply=self.native.leaf(node.moves,self.attacker,limits,remaining)
            if bytes.fromhex(reply['key'])!=node.key:raise ValueError('tactical replay/key mismatch')
            if not 0<=reply['work']<=remaining:raise ValueError('native leaf exceeded work admission')
            self.work+=reply['work'];remaining-=reply['work']
            for key in self.leaf_statistics:self.leaf_statistics[key]+=reply[key]
            if reply['outcome']=='ProvenWin' and not reply['terminal'] and leaf_entry(reply['book'],self.attacker,node.key)[60]>self.max_additional_plies-(len(node.moves)-self.root_ply):
                reply['outcome']='Unknown'
            if reply['outcome'] in ('ProvenWin','Refuted'):
                outcome=1 if reply['outcome']=='ProvenWin' else 2
                evidence=json.dumps({'kind':'native-leaf-v1','limits':limits,'work_limit':remaining+reply['work'],
                    'outcome':reply['outcome'],'book':reply['book']},sort_keys=True).encode()
                self.result(id,outcome,*_numbers(outcome),evidence=evidence);return
            if reply['outcome']!='Unknown':raise ValueError('invalid tactical outcome')
            if remaining==0:raise ResourceStop('tactical work exhausted; Unknown')
        facts=self.replay(node);self.work+=1;self.pn_expansions+=1
        if facts['winner'] is not None or facts['full']:
            outcome=1 if facts['winner']==self.attacker else 2
            self.db.expand(id,[]);self.result(id,outcome,*_numbers(outcome));return
        children=[]
        for original,canonical,key,winner,full in sorted(facts['children'],key=lambda c:c[1]):
            child=self.db.intern(bytes.fromhex(key),node.moves+bytes([original]))
            children.append((canonical,child))
            if winner is not None or full:
                outcome=1 if winner==self.attacker else 2
                self.result(child,outcome,*_numbers(outcome))
        self.db.expand(id,children);self.aggregate(id)
    def select(self,root,max_additional_plies):
        node=self.db.node(root)
        for _ in range(226):
            relative_depth=len(node.moves)-self.root_ply
            if relative_depth<0:raise ValueError('node precedes frozen proof root')
            if relative_depth>=max_additional_plies:raise ResourceStop('additional proof ply limit')
            if not node.expanded:return node.id
            children=[(move,self.db.node(child)) for move,child in self.db.children(node.id)]
            pending=[(move,child) for move,child in children if child.outcome==0]
            if not pending:raise ValueError('inconsistent unresolved working node')
            if any(len(child.moves)!=len(node.moves)+1 for _,child in children):raise ValueError('invalid selected child depth/cycle')
            is_or=node.key[-1]==self.attacker
            node=min(pending,key=lambda pair:(pair[1].pn if is_or else pair[1].dn,pair[0]))[1]
        raise ValueError('cycle/depth overflow in working tree')
    def solve(self,root,*,work,seconds,max_additional_plies=225):
        if work<1 or seconds<=0 or not 0<=max_additional_plies<=225:raise ValueError('invalid solve budget')
        if self.root_ply is None:self.root_ply=len(self.db.node(root).moves)
        self.max_additional_plies=max_additional_plies
        deadline=time.monotonic()+seconds
        try:
            while self.db.node(root).outcome==0:
                if self.work>=work or time.monotonic()>=deadline:raise ResourceStop('work/time limit')
                id=self.select(root,max_additional_plies)
                with self.db.transaction():self.expand(id,work-self.work);self.propagate()
        except ResourceStop as stop:return {'outcome':'Unknown','reason':str(stop),'work':self.work}
        return {'outcome':'UnverifiedWin' if self.db.node(root).outcome==1 else 'UnverifiedRefutation','work':self.work}

    def verify_witness(self,root,*,max_nodes,seconds):
        """Replay a bounded witness; DB flags never suffice, including Refuted.

        A refutation requires every attacker choice or one real defender reply.
        A win requires one attacker choice or every legal defender reply.
        """
        deadline=time.monotonic()+seconds;visited=0;tactical_work=0;stack=set()
        if self.root_ply is None:self.root_ply=len(self.db.node(root).moves)
        self.db.connection.execute('DELETE FROM certificates')
        def visit(id):
            nonlocal visited,tactical_work
            visited+=1;self.witness_nodes+=1
            if visited>max_nodes or time.monotonic()>=deadline:raise ResourceStop('witness verification budget')
            if id in stack:raise ValueError('cycle in working witness')
            node=self.db.node(id)
            if node.outcome not in (1,2):raise ValueError('unknown witness child')
            memo=self.db.connection.execute('SELECT outcome,distance FROM certificates WHERE id=?',(id,)).fetchone()
            if memo:
                if memo[0]!=node.outcome:raise ValueError('conflicting witness')
                return memo[1]
            facts=self.replay(node)
            if facts['winner'] is not None or facts['full']:
                expected=1 if facts['winner']==self.attacker else 2
                if node.outcome!=expected:raise ValueError('false terminal claim')
                distance=0;action=-1
            elif node.evidence:
                evidence=json.loads(node.evidence)
                if self.leaf_limits is None or evidence.get('kind')!='native-leaf-v1' or evidence.get('limits')!=self.effective_leaf_limits(node):raise ValueError('incompatible leaf evidence')
                if type(evidence.get('work_limit')) is not int or evidence['work_limit']<0:raise ValueError('invalid leaf work limit')
                allowance=min(evidence['work_limit'],max_nodes-tactical_work)
                if allowance<=0:raise ResourceStop('witness tactical work budget')
                fresh=self.native.leaf(node.moves,self.attacker,evidence['limits'],allowance)
                if not 0<=fresh['work']<=allowance:raise ValueError('witness tactical work overflow')
                tactical_work+=fresh['work'];self.witness_tactical_work+=fresh['work']
                expected='ProvenWin' if node.outcome==1 else 'Refuted'
                if allowance<evidence['work_limit'] and (fresh['outcome']=='Unknown' or fresh['book']!=evidence['book']):raise ResourceStop('witness tactical work budget')
                if fresh['key']!=node.key.hex() or fresh['outcome']!=expected or fresh['book']!=evidence['book']:raise ValueError('false tactical witness')
                distance=leaf_entry(fresh['book'],self.attacker,node.key)[60] if node.outcome==1 else 0
                action=226
            else:
                actual={move:self.db.node(child) for move,child in self.db.children(id)}
                expected={canonical:bytes.fromhex(key) for _,canonical,key,_,_ in facts['children']}
                if {move:child.key for move,child in actual.items()}!=expected:raise ValueError('missing or malformed legal/D4 child coverage')
                existential=(facts['side']==self.attacker)==(node.outcome==1)
                candidates=sorted((move,child.id) for move,child in actual.items() if child.outcome==node.outcome)
                if not candidates or (not existential and len(candidates)!=len(expected)):raise ValueError('incomplete witness')
                chosen=candidates[:1] if existential else candidates
                stack.add(id)
                distance=1+max(visit(child) for _,child in chosen)
                stack.remove(id)
                action=chosen[0][0] if existential else 225
            self.db.connection.execute('INSERT INTO certificates VALUES(?,?,?,?)',(id,node.outcome,distance,action))
            return distance
        with self.db.transaction():visit(root)
        return 'ProvenWin' if self.db.node(root).outcome==1 else 'Refuted'

    def export(self,root,path):
        """Stream the verified working witness, then fresh-parse native verification.

        Publication happens only after the native independent verifier succeeds.
        """
        root=self.db.node(root);path=Path(path)
        if root.outcome!=1:raise ValueError('only wins export a ProofBook')
        if self.db.connection.execute('SELECT outcome FROM certificates WHERE id=?',(root.id,)).fetchone()!=(1,):raise ValueError('verify witness before export')
        count=self.db.connection.execute('SELECT count(*) FROM certificates WHERE action>=0').fetchone()[0]
        # Match the native parser limit explicitly, independent of disk capacity.
        if count>1_000_000:raise ResourceStop('ProofBook export entry limit')
        handle,name=tempfile.mkstemp(prefix='.'+path.name+'.',suffix='.unverified',dir=path.parent)
        temporary=Path(name)
        try:
            with os.fdopen(handle,'wb') as output:
                output.write(b'RMPBOOK1'+struct.pack('<HBII',1,0,1,count))
                output.write(struct.pack('<BH',self.attacker,len(root.moves))+root.moves+root.key)
                for key,distance,action,evidence in self.db.connection.execute('SELECT n.key,c.distance,c.action,n.evidence FROM certificates c JOIN nodes n ON n.id=c.id WHERE c.action>=0 ORDER BY n.key'):
                    if action==226:
                        output.write(leaf_entry(json.loads(evidence)['book'],self.attacker,key))
                    else:
                        output.write(bytes([self.attacker])+key+bytes([1,distance]))
                        output.write(bytes([1]) if action==225 else bytes([0,action]))
                output.flush();os.fsync(output.fileno())
            subprocess.run([self.native.engine,'verify','--book',str(temporary)],check=True)
            os.replace(temporary,path)
        finally:
            if temporary.exists():temporary.unlink()

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine',required=True);parser.add_argument('--record',required=True)
    parser.add_argument('--attacker',choices=['black','white'],required=True)
    parser.add_argument('--checkpoint',required=True);parser.add_argument('--output',required=True)
    parser.add_argument('--resume',action='store_true');parser.add_argument('--nodes',type=int,required=True)
    parser.add_argument('--seconds',type=float,required=True);parser.add_argument('--sqlite-cache-mib','--ram-mib',dest='sqlite_cache_mib',type=int,default=32,help='SQLite page cache only; not process RSS')
    parser.add_argument('--disk-mib',type=int,default=1024);parser.add_argument('--max-additional-plies',type=int,default=225)
    for name,default in [('vcf-plies',8),('vcf-nodes',1000),('vct-plies',6),('vct-nodes',1000)]:parser.add_argument('--'+name,type=int,default=default)
    args=parser.parse_args();engine=str(Path(args.engine).resolve())
    # Core validates the chronological record; Python does not parse game rules.
    raw=subprocess.check_output([engine,'root-moves','--record',args.record],text=True).strip()
    moves=bytes.fromhex(raw);attacker=0 if args.attacker=='black' else 1
    config={'version':2,'engine_sha256':fingerprint(engine),'moves':raw,'attacker':attacker,'root_ply':len(moves),'max_additional_plies':args.max_additional_plies,'rules':'freestyle'}
    config['leaf_limits']={name:getattr(args,name) for name in ('vcf_plies','vcf_nodes','vct_plies','vct_nodes')}
    if any(not 0<=value<=(225 if name.endswith('plies') else (1<<63)-1) for name,value in config['leaf_limits'].items()):raise ValueError('invalid tactical leaf limits')
    try:
        with NativeFacts(engine) as native, DiskNodes(args.checkpoint,config,cache_bytes=args.sqlite_cache_mib*1024*1024,max_disk_bytes=args.disk_mib*1024*1024,resume=args.resume) as db:
            freeze(str(args.checkpoint)+'.config.json',config)
            with db.transaction():root=db.intern(bytes.fromhex(native.facts(moves)['key']),moves)
            solver=DiskSolver(db,native,attacker)
            result=solver.solve(root,work=args.nodes,seconds=args.seconds,max_additional_plies=args.max_additional_plies)
            if result['outcome']!='Unknown':
                result['outcome']=solver.verify_witness(root,max_nodes=args.nodes,seconds=args.seconds)
                if result['outcome']=='ProvenWin':solver.export(root,args.output)
            result['resources']={'sqlite_cache_mib':args.sqlite_cache_mib,'disk_budget_mib':args.disk_mib,'work_budget':args.nodes,'work_unit':'pn-expansions-plus-tactical-search-visits','wall_budget':args.seconds,'wall_scope':'search-phase-cooperative-between-native-calls','witness_node_budget':args.nodes,'witness_tactical_work_budget':args.nodes,'witness_wall_budget':args.seconds,'native_certificate_replay':'separately-bounded-by-declared-leaf-and-final-verifier-limits','rss_enforced':False}
            result['statistics']=solver.telemetry()
            print(json.dumps(result,sort_keys=True))
    except ResourceStop as stop:print(json.dumps({'outcome':'Unknown','reason':str(stop)}))

if __name__=='__main__':main()
