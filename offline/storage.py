"""Compact SQLite working nodes/frontier. This database is NOT proof evidence.

Node IDs are stable disk row IDs. Canonical keys, replay moves, PN/DN and edges
are explicit bounded fields; no Position/Vec object graph is persisted. SQLite
provides transactions, a disk B-tree index, bounded page cache and recovery.
Fresh native replay/certificate verification is required before exact admission.
"""
import contextlib
import dataclasses
import json
import sqlite3
from pathlib import Path

VERSION=1
INFINITY=(1<<62)-1
SCHEMA=(
    'CREATE TABLE metadata (id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL, config BLOB NOT NULL)',
    'CREATE TABLE nodes (id INTEGER PRIMARY KEY, key BLOB NOT NULL UNIQUE CHECK(length(key)=58), moves BLOB NOT NULL CHECK(length(moves)<=225), expanded INTEGER NOT NULL DEFAULT 0 CHECK(expanded IN (0,1)), outcome INTEGER NOT NULL DEFAULT 0 CHECK(outcome BETWEEN 0 AND 2), pn INTEGER NOT NULL DEFAULT 1 CHECK(pn>=0), dn INTEGER NOT NULL DEFAULT 1 CHECK(dn>=0), evidence BLOB NOT NULL DEFAULT (zeroblob(0)) CHECK(length(evidence)<=4096))',
    'CREATE TABLE edges (parent INTEGER NOT NULL REFERENCES nodes(id), move INTEGER NOT NULL CHECK(move BETWEEN 0 AND 224), child INTEGER NOT NULL REFERENCES nodes(id), PRIMARY KEY(parent,move))',
    'CREATE INDEX frontier ON nodes(outcome,expanded,id)',
    'CREATE INDEX parents ON edges(child,parent)',
    'CREATE TABLE dirty (id INTEGER PRIMARY KEY REFERENCES nodes(id))',
    'CREATE TABLE certificates (id INTEGER PRIMARY KEY REFERENCES nodes(id), outcome INTEGER NOT NULL CHECK(outcome IN (1,2)), distance INTEGER NOT NULL CHECK(distance BETWEEN 0 AND 225), action INTEGER NOT NULL CHECK(action BETWEEN -1 AND 225))',
)

@dataclasses.dataclass(frozen=True)
class Node:
    id:int
    key:bytes
    moves:bytes
    expanded:bool
    outcome:int  # 0 Unknown, 1 working ProvenWin, 2 working Refuted; never authority.
    pn:int
    dn:int
    evidence:bytes

class ResourceStop(Exception):
    outcome='Unknown'

class DiskNodes:
    def __init__(self,path,config,*,cache_bytes=4*1024*1024,max_disk_bytes=1024*1024*1024,resume=False):
        if cache_bytes<4096 or max_disk_bytes<65536: raise ResourceStop('insufficient working storage budget')
        self.path=Path(path)
        encoded=json.dumps(config,sort_keys=True,separators=(',',':'),allow_nan=False).encode()
        if len(encoded)>65536: raise ValueError('working config exceeds limit')
        if self.path.exists() != resume: raise ValueError('use explicit compatible resume for an existing database')
        if resume and self.path.stat().st_size>max_disk_bytes: raise ValueError('working database exceeds disk budget')
        self.path.parent.mkdir(parents=True,exist_ok=True)
        self.connection=sqlite3.connect(self.path)
        c=self.connection
        try:
            c.setlimit(sqlite3.SQLITE_LIMIT_LENGTH,1024*1024)
            c.execute('PRAGMA trusted_schema=OFF')
            c.execute('PRAGMA foreign_keys=ON')
            c.execute('PRAGMA synchronous=FULL')
            c.execute('PRAGMA journal_mode=DELETE')
            c.execute(f'PRAGMA cache_size=-{cache_bytes//1024}')
            c.execute('PRAGMA mmap_size=0')
            page=c.execute('PRAGMA page_size').fetchone()[0]
            # Reserve half the explicit disk budget for rollback journal pages.
            limit=max_disk_bytes//(2*page)
            if c.execute('PRAGMA page_count').fetchone()[0]>limit: raise ResourceStop('disk budget below existing database')
            c.execute(f'PRAGMA max_page_count={limit}')
            if resume:
                if c.execute('PRAGMA quick_check').fetchone()[0]!='ok': raise ValueError('corrupt working database')
                actual={row[0] for row in c.execute("SELECT sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'")}
                if actual!=set(SCHEMA): raise ValueError('unexpected working database schema')
                row=c.execute('SELECT version,config FROM metadata WHERE id=1').fetchone()
                if row!=(VERSION,encoded): raise ValueError('incompatible working database resume')
            else:
                with c:
                    for sql in SCHEMA:c.execute(sql)
                    c.execute('INSERT INTO metadata VALUES(1,?,?)',(VERSION,encoded))
        except BaseException:
            c.close();raise
    @contextlib.contextmanager
    def transaction(self):
        try:
            with self.connection:yield self
        except sqlite3.OperationalError as e:
            if 'full' in str(e).lower() or 'memory' in str(e).lower(): raise ResourceStop(str(e)) from e
            raise
    def intern(self,key,moves):
        key=bytes(key);moves=bytes(moves)
        if len(key)!=58 or key[-1]>1 or key[-2]&63 or len(moves)>225 or any(m>=225 for m in moves): raise ValueError('invalid compact node')
        if any(((key[i//4]>>(2*(3-i%4)))&3)==3 for i in range(225)): raise ValueError('reserved key cell')
        self.connection.execute('INSERT OR IGNORE INTO nodes(key,moves) VALUES(?,?)',(key,moves))
        return self.connection.execute('SELECT id FROM nodes WHERE key=?',(key,)).fetchone()[0]
    def node(self,id):
        row=self.connection.execute('SELECT id,key,moves,expanded,outcome,pn,dn,evidence FROM nodes WHERE id=?',(id,)).fetchone()
        if row is None:raise ValueError('missing NodeId')
        node=Node(*row)
        if (not isinstance(node.key,bytes) or len(node.key)!=58 or node.key[-1]>1
            or not isinstance(node.moves,bytes) or len(node.moves)>225
            or node.expanded not in (0,1) or node.outcome not in (0,1,2)
            or not 0<=node.pn<=INFINITY or not 0<=node.dn<=INFINITY
            or not isinstance(node.evidence,bytes) or len(node.evidence)>4096):
            raise ValueError('corrupt compact node')
        return node
    def expand(self,parent,children):
        children=sorted(children)
        if len(children)>225 or len({move for move,_ in children})!=len(children): raise ValueError('invalid child coverage')
        if self.node(parent).expanded:
            if children!=self.children(parent):raise ValueError('incompatible child coverage')
            return
        self.connection.executemany('INSERT INTO edges VALUES(?,?,?)',[(parent,move,child) for move,child in children])
        self.connection.execute('UPDATE nodes SET expanded=1 WHERE id=?',(parent,))
    def children(self,parent):
        return self.connection.execute('SELECT move,child FROM edges WHERE parent=? ORDER BY move',(parent,)).fetchall()
    def frontier(self,limit=256):
        if not 1<=limit<=4096:raise ValueError('frontier read limit')
        return [row[0] for row in self.connection.execute('SELECT id FROM nodes WHERE outcome=0 AND expanded=0 ORDER BY id LIMIT ?',(limit,))]
    def working_result(self,id,*,outcome,pn,dn,evidence=b''):
        if outcome not in (0,1,2) or not 0<=pn<=INFINITY or not 0<=dn<=INFINITY or len(evidence)>4096:raise ValueError('invalid working result')
        if (outcome==1 and pn!=0) or (outcome==2 and dn!=0) or (outcome==0 and (pn==0 or dn==0)):raise ValueError('inconsistent PN/DN')
        if self.connection.execute('UPDATE nodes SET outcome=?,pn=?,dn=?,evidence=? WHERE id=?',(outcome,pn,dn,evidence,id)).rowcount!=1:
            raise ValueError('missing result NodeId')
    def close(self):self.connection.close()
    def __enter__(self):return self
    def __exit__(self,*_):self.close()
