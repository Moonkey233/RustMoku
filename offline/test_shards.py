import tempfile
import unittest
from pathlib import Path
from offline.native import NativeFacts, fingerprint
from offline.solve import DiskSolver
from offline.shards import frontier, solve_batch, merge, publish, read_json
from offline.storage import DiskNodes
from offline.test_solve import ENGINE

class ShardTests(unittest.TestCase):
    def test_verified_merge_and_unknown_cannot_refute(self):
        with tempfile.TemporaryDirectory() as directory, NativeFacts(ENGINE) as native:
            directory=Path(directory);moves=bytes([105,0,106,2,107,4,108,6])
            config={'version':2,'engine_sha256':fingerprint(ENGINE),'moves':moves.hex(),'attacker':0,'root_ply':len(moves),'max_additional_plies':225,'rules':'freestyle'}
            options={'cache_bytes':4096,'max_disk_bytes':1024*1024}
            with DiskNodes(directory/'coordinator.db',config,**options) as db:
                with db.transaction():root=db.intern(bytes.fromhex(native.facts(moves)['key']),moves)
                manifest=frontier(db,directory/'frontier.json',shards=2)
                outputs=[directory/'one',directory/'two']
                for shard in range(2):solve_batch(manifest,ENGINE,outputs[shard],shard=shard,workers=2,work=10,seconds=10,**options)
                job=manifest['jobs'][0];receipt_path=outputs[job['shard']]/(job['id']+'.json')
                receipt=read_json(receipt_path);unknown={**receipt,'result':{'outcome':'Unknown'}}
                publish(receipt_path,unknown)
                self.assertEqual(merge(db,manifest,ENGINE,outputs,work=10,seconds=10,**options),0)
                self.assertEqual(db.node(root).outcome,0)
                publish(receipt_path,receipt)
                self.assertEqual(merge(db,manifest,ENGINE,list(reversed(outputs)),work=10,seconds=10,**options),1)
                self.assertEqual(DiskSolver(db,native,0).verify_witness(root,max_nodes=10,seconds=10),'ProvenWin')
                self.assertEqual(merge(db,manifest,ENGINE,outputs,work=10,seconds=10,**options),1)
                self.assertEqual(db.node(root).outcome,1)
                with db.transaction():
                    db.working_result(root,outcome=2,pn=10**18,dn=0)
                with self.assertRaisesRegex(ValueError,'conflicting coordinator/shard witness'):
                    merge(db,manifest,ENGINE,outputs,work=10,seconds=10,**options)
                self.assertEqual(db.node(root).outcome,2)

if __name__=='__main__':unittest.main()
