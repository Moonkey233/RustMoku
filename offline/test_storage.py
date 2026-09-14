import tempfile
import unittest
from pathlib import Path
from offline.storage import DiskNodes, ResourceStop

def key(i):
    data=bytearray(58);data[i//4]=1<<(2*(3-i%4));data[-1]=1;return bytes(data)

class StorageTests(unittest.TestCase):
    def test_spill_resume_stable_ids_frontier_and_transaction_rollback(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'work.db'; config={'rules':'freestyle','attacker':'black','root':0}
            with DiskNodes(path,config,cache_bytes=4096) as db:
                with db.transaction():
                    ids=[db.intern(key(i),[i]) for i in range(4)]
                    db.expand(ids[0],[(1,ids[1]),(2,ids[2])])
                before=db.node(ids[0]); frontier=db.frontier()
                with self.assertRaises(RuntimeError):
                    with db.transaction():db.intern(key(5),[5]);raise RuntimeError('crash')
            with DiskNodes(path,config,cache_bytes=4096,resume=True) as db:
                self.assertEqual(db.node(ids[0]),before);self.assertEqual(db.frontier(),frontier)
                with db.transaction():self.assertEqual(db.intern(key(1),[1]),ids[1])
            with self.assertRaises(ValueError):DiskNodes(path,{'wrong':True},resume=True)
            payload=path.read_bytes();path.write_bytes(payload[:50])
            with self.assertRaises(Exception):DiskNodes(path,config,resume=True)
    def test_resource_stop_is_unknown(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ResourceStop) as caught:DiskNodes(Path(directory)/'w.db',{},cache_bytes=1)
            self.assertEqual(caught.exception.outcome,'Unknown')
