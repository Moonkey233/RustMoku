import os
import tempfile
import unittest
from pathlib import Path
from offline.native import NativeFacts
from offline.solve import DiskSolver
from offline.storage import DiskNodes

ENGINE=Path(__file__).resolve().parents[1]/('target/debug/rustmoku-solver.exe' if os.name=='nt' else 'target/debug/rustmoku-solver')

class DiskSolveTests(unittest.TestCase):
    def test_native_win_export_and_missing_mapping_rejected(self):
        with tempfile.TemporaryDirectory() as directory, NativeFacts(ENGINE) as native:
            with DiskNodes(Path(directory)/'nodes.db',{},cache_bytes=4096) as db:
                moves=bytes([105,0,106,2,107,4,108,6])
                with db.transaction():root=db.intern(bytes.fromhex(native.facts(moves)['key']),moves)
                solver=DiskSolver(db,native,0)
                self.assertEqual(solver.solve(root,work=1,seconds=10)['outcome'],'UnverifiedWin')
                self.assertEqual(solver.verify_witness(root,max_nodes=10,seconds=10),'ProvenWin')
                solver.export(root,Path(directory)/'verified.rmp')
                with db.transaction():db.connection.execute('DELETE FROM edges WHERE parent=? AND move=(SELECT min(move) FROM edges WHERE parent=?)',(root,root))
                with self.assertRaisesRegex(ValueError,'coverage'):solver.verify_witness(root,max_nodes=10,seconds=10)

    def test_terminal_refutation_and_unknown_resume(self):
        with tempfile.TemporaryDirectory() as directory, NativeFacts(ENGINE) as native:
            path=Path(directory)/'nodes.db'
            with DiskNodes(path,{},cache_bytes=4096) as db:
                moves=bytes([105,0,106,2,107,4,108,6,109])
                with db.transaction():root=db.intern(bytes.fromhex(native.facts(moves)['key']),moves)
                solver=DiskSolver(db,native,1)
                self.assertEqual(solver.solve(root,work=1,seconds=10)['outcome'],'UnverifiedRefutation')
                self.assertEqual(solver.verify_witness(root,max_nodes=1,seconds=10),'Refuted')
                with db.transaction():empty=db.intern(bytes.fromhex(native.facts(b'')['key']),b'')
                self.assertEqual(solver.solve(empty,work=1,seconds=10)['outcome'],'Unknown')
            with DiskNodes(path,{},cache_bytes=4096,resume=True) as db:
                self.assertEqual(db.node(empty).outcome,0)
                self.assertEqual(DiskSolver(db,native,1).verify_witness(root,max_nodes=1,seconds=10),'Refuted')

if __name__=='__main__':unittest.main()
