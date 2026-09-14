"""Persistent bounded native replay worker; the core owns every game rule."""
import hashlib
import json
import subprocess
from pathlib import Path

def fingerprint(path):
    digest=hashlib.sha256()
    with Path(path).open('rb') as source:
        for block in iter(lambda:source.read(1024*1024),b''):digest.update(block)
    return digest.hexdigest()

class NativeFacts:
    def __init__(self,engine):
        self.engine=str(Path(engine).resolve())
        self.process=subprocess.Popen([self.engine,'facts-worker'],stdin=subprocess.PIPE,stdout=subprocess.PIPE)
    def facts(self,moves):
        moves=bytes(moves)
        if len(moves)>225:raise ValueError('oversized replay')
        self.process.stdin.write(moves.hex().encode()+b'\n');self.process.stdin.flush()
        line=self.process.stdout.readline(40001)
        if len(line)>40000 or not line.endswith(b'\n'):raise ValueError('invalid native facts response')
        value=json.loads(line)
        if value['version']!=1 or len(value['children'])>225:raise ValueError('invalid facts protocol')
        return value
    def close(self):
        self.process.stdin.close()
        try:self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:self.process.kill();self.process.wait()
        self.process.stdout.close()
    def __enter__(self):return self
    def __exit__(self,*_):self.close()
