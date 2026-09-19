"""Persistent bounded native replay worker; the core owns every game rule."""
import hashlib
import json
import subprocess
import time
from pathlib import Path

def fingerprint(path):
    digest=hashlib.sha256()
    with Path(path).open('rb') as source:
        for block in iter(lambda:source.read(1024*1024),b''):digest.update(block)
    return digest.hexdigest()

class NativeFacts:
    def __init__(self,engine):
        self.statistics=dict(native_facts_calls=0,native_leaf_calls=0,replayed_plies=0,children_generated=0,request_bytes=0,response_bytes=0,native_roundtrip_seconds=0.,json_parse_seconds=0.)
        self.engine=str(Path(engine).resolve())
        self.process=subprocess.Popen([self.engine,'facts-worker'],stdin=subprocess.PIPE,stdout=subprocess.PIPE)
    def _request(self,request,moves,kind):
        if len(moves)>225:raise ValueError('oversized replay')
        started=time.perf_counter()
        self.process.stdin.write(request);self.process.stdin.flush()
        line=self.process.stdout.readline(40001)
        if len(line)>40000 or not line.endswith(b'\n'):raise ValueError('invalid native response')
        received=time.perf_counter();value=json.loads(line)
        self.statistics[kind]+=1;self.statistics['replayed_plies']+=len(moves)
        self.statistics['request_bytes']+=len(request);self.statistics['response_bytes']+=len(line)
        self.statistics['native_roundtrip_seconds']+=received-started
        self.statistics['json_parse_seconds']+=time.perf_counter()-received
        self.statistics['children_generated']+=len(value.get('children',[]))
        if value['version']!=1:raise ValueError('invalid native protocol')
        return value
    def facts(self,moves):
        moves=bytes(moves)
        value=self._request(moves.hex().encode()+b'\n',moves,'native_facts_calls')
        if len(value['children'])>225:raise ValueError('invalid child count')
        return value
    def leaf(self,moves,attacker,limits,work):
        moves=bytes(moves)
        request='|'.join(map(str,('L',attacker,limits['vcf_plies'],limits['vcf_nodes'],limits['vct_plies'],limits['vct_nodes'],work,moves.hex())))
        return self._request(request.encode()+b'\n',moves,'native_leaf_calls')
    def close(self):
        self.process.stdin.close()
        try:self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:self.process.kill();self.process.wait()
        self.process.stdout.close()
    def __enter__(self):return self
    def __exit__(self,*_):self.close()
