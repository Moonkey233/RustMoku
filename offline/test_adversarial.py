import tempfile
import unittest
from pathlib import Path
from offline.native import NativeFacts
from offline.solve import DiskSolver
from offline.storage import DiskNodes, INFINITY
from offline.test_solve import ENGINE

class AdversarialWitnesses(unittest.TestCase):
    def test_missing_defender_reply_and_malformed_d4_child_reject(self):
        with tempfile.TemporaryDirectory() as directory, NativeFacts(ENGINE) as native:
            for corruption in ('missing','mapping'):
                with self.subTest(corruption=corruption), DiskNodes(Path(directory)/(corruption+'.db'),{}) as db:
                    moves=bytes([112])  # White defends against the Black attacker.
                    facts=native.facts(moves)
                    with db.transaction():
                        root=db.intern(bytes.fromhex(facts['key']),moves)
                        edges=[(canonical,db.intern(bytes.fromhex(key),moves+bytes([original])))
                               for original,canonical,key,_,_ in facts['children']]
                        db.expand(root,edges)
                        db.working_result(root,outcome=1,pn=0,dn=INFINITY)
                        move,child=edges[0]
                        if corruption=='missing':
                            db.connection.execute('DELETE FROM edges WHERE parent=? AND move=?',(root,move))
                        else:
                            wrong=next(other for _,other in edges if db.node(other).key!=db.node(child).key)
                            db.connection.execute('UPDATE edges SET child=? WHERE parent=? AND move=?',(wrong,root,move))
                    with self.assertRaisesRegex(ValueError,'legal/D4 child coverage'):
                        DiskSolver(db,native,0).verify_witness(root,max_nodes=2,seconds=5)

    def test_false_terminal_refutation_rejects(self):
        with tempfile.TemporaryDirectory() as directory, NativeFacts(ENGINE) as native:
            with DiskNodes(Path(directory)/'nodes.db',{}) as db:
                moves=bytes([105,0,106,2,107,4,108,6,109])
                with db.transaction():
                    root=db.intern(bytes.fromhex(native.facts(moves)['key']),moves)
                    db.working_result(root,outcome=2,pn=INFINITY,dn=0)
                with self.assertRaisesRegex(ValueError,'false terminal'):
                    DiskSolver(db,native,0).verify_witness(root,max_nodes=1,seconds=5)

if __name__=='__main__':unittest.main()
