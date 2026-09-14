"""Bounded JSON artifacts published by same-directory atomic replacement."""
import json
import os
import tempfile
from pathlib import Path

MAX_MANIFEST=4*1024*1024

def read_json(path):
    with Path(path).open('rb') as source:raw=source.read(MAX_MANIFEST+1)
    if len(raw)>MAX_MANIFEST:raise ValueError('oversized offline artifact')
    return json.loads(raw)

def publish(path,value):
    path=Path(path);encoded=json.dumps(value,sort_keys=True,separators=(',',':'),allow_nan=False).encode()
    if len(encoded)>MAX_MANIFEST:raise ValueError('oversized offline artifact')
    handle,temporary=tempfile.mkstemp(prefix='.'+path.name+'.',suffix='.partial',dir=path.parent)
    try:
        with os.fdopen(handle,'wb') as output:output.write(encoded);output.flush();os.fsync(output.fileno())
        os.replace(temporary,path)
    finally:
        if os.path.exists(temporary):os.unlink(temporary)

def freeze(path,value):
    path=Path(path)
    if path.exists():
        if read_json(path)!=value:raise ValueError('incompatible offline frozen configuration')
    else:publish(path,value)
