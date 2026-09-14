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

class DiskSolver:
    def __init__(self,db,native,attacker):
        self.db=db;self.native=native;self.attacker=attacker;self.work=0
    def replay(self,node):
        facts=self.native.facts(node.moves)
        if bytes.fromhex(facts['key'])!=node.key:raise ValueError('working node does not match native replay')
        return facts
    def result(self,id,outcome,pn,dn):
        old=self.db.node(id)
        if (old.outcome,old.pn,old.dn)==(outcome,pn,dn):return
        self.db.working_result(id,outcome=outcome,pn=pn,dn=dn)
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
    def expand(self,id):
        node=self.db.node(id);facts=self.replay(node);self.work+=1
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
    def select(self,root,max_depth):
        node=self.db.node(root)
        for _ in range(226):
            if len(node.moves)>=max_depth:raise ResourceStop('tree depth limit')
            if not node.expanded:return node.id
            children=[(move,self.db.node(child)) for move,child in self.db.children(node.id)]
            pending=[(move,child) for move,child in children if child.outcome==0]
            if not pending:raise ValueError('inconsistent unresolved working node')
            if any(len(child.moves)!=len(node.moves)+1 for _,child in children):raise ValueError('invalid selected child depth/cycle')
            is_or=node.key[-1]==self.attacker
            node=min(pending,key=lambda pair:(pair[1].pn if is_or else pair[1].dn,pair[0]))[1]
        raise ValueError('cycle/depth overflow in working tree')
    def solve(self,root,*,work,seconds,max_depth=225):
        if work<1 or seconds<=0 or not 0<=max_depth<=225:raise ValueError('invalid solve budget')
        deadline=time.monotonic()+seconds
        try:
            while self.db.node(root).outcome==0:
                if self.work>=work or time.monotonic()>=deadline:raise ResourceStop('work/time limit')
                id=self.select(root,max_depth)
                with self.db.transaction():self.expand(id);self.propagate()
        except ResourceStop as stop:return {'outcome':'Unknown','reason':str(stop),'work':self.work}
        return {'outcome':'UnverifiedWin' if self.db.node(root).outcome==1 else 'UnverifiedRefutation','work':self.work}

    def verify_witness(self,root,*,max_nodes,seconds):
        """Replay a bounded witness; DB flags never suffice, including Refuted.

        A refutation requires every attacker choice or one real defender reply.
        A win requires one attacker choice or every legal defender reply.
        """
        deadline=time.monotonic()+seconds;visited=0;stack=set()
        self.db.connection.execute('DELETE FROM certificates')
        def visit(id):
            nonlocal visited
            visited+=1
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
                for key,distance,action in self.db.connection.execute('SELECT n.key,c.distance,c.action FROM certificates c JOIN nodes n ON n.id=c.id WHERE c.action>=0 ORDER BY n.key'):
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
    parser.add_argument('--seconds',type=float,required=True);parser.add_argument('--ram-mib',type=int,default=32)
    parser.add_argument('--disk-mib',type=int,default=1024);parser.add_argument('--max-depth',type=int,default=225)
    args=parser.parse_args();engine=str(Path(args.engine).resolve())
    # Core validates the chronological record; Python does not parse game rules.
    raw=subprocess.check_output([engine,'root-moves','--record',args.record],text=True).strip()
    moves=bytes.fromhex(raw);attacker=0 if args.attacker=='black' else 1
    config={'version':1,'engine_sha256':fingerprint(engine),'moves':raw,'attacker':attacker,'max_depth':args.max_depth,'rules':'freestyle'}
    try:
        with NativeFacts(engine) as native, DiskNodes(args.checkpoint,config,cache_bytes=args.ram_mib*1024*1024,max_disk_bytes=args.disk_mib*1024*1024,resume=args.resume) as db:
            freeze(str(args.checkpoint)+'.config.json',config)
            with db.transaction():root=db.intern(bytes.fromhex(native.facts(moves)['key']),moves)
            solver=DiskSolver(db,native,attacker)
            result=solver.solve(root,work=args.nodes,seconds=args.seconds,max_depth=args.max_depth)
            if result['outcome']!='Unknown':
                result['outcome']=solver.verify_witness(root,max_nodes=args.nodes,seconds=args.seconds)
                if result['outcome']=='ProvenWin':solver.export(root,args.output)
            print(json.dumps(result,sort_keys=True))
    except ResourceStop as stop:print(json.dumps({'outcome':'Unknown','reason':str(stop)}))

if __name__=='__main__':main()
