import tempfile
import unittest
from pathlib import Path
from offline.native import NativeFacts
from offline.storage import DiskNodes, ResourceStop
from offline.solve import DiskSolver
from offline.test_solve import ENGINE

class ExactLeaves(unittest.TestCase):
    def test_terminal_leaf_and_tactical_horizon(self):
        with tempfile.TemporaryDirectory() as directory, NativeFacts(ENGINE) as native:
            limits={'vcf_plies':8,'vcf_nodes':1000,'vct_plies':8,'vct_nodes':1000}
            for name,moves,horizon,expected in [
                    ('terminal',[105,0,106,2,107,4,108,6,109],1,'UnverifiedWin'),
                    ('short',[110,0,111,2,112,15],1,'Unknown')]:
                config={'leaf_limits':limits,'root_ply':len(moves),'max_additional_plies':horizon}
                with DiskNodes(Path(directory)/(name+'.db'),config) as db:
                    with db.transaction():root=db.intern(bytes.fromhex(native.facts(moves)['key']),moves)
                    solver=DiskSolver(db,native,0)
                    self.assertEqual(solver.solve(root,work=100,seconds=5,max_additional_plies=horizon)['outcome'],expected)
                    if name=='terminal':self.assertEqual(solver.verify_witness(root,max_nodes=1,seconds=5),'ProvenWin')

    def test_immediate_vcf_vct_export_and_budget_unknown(self):
        with tempfile.TemporaryDirectory() as directory, NativeFacts(ENGINE) as native:
            cases=[
                ([105,0,106,2,107,4,108,6],0,0,1,'immediate_hits'),
                ([110,0,111,2,112,15],1000,0,1001,'vcf_proven'),
                ([110,0,111,2,112,15],0,1000,1001,'vct_proven'),
            ]
            for number,(moves,vcf,vct,work,counter) in enumerate(cases):
                limits={'vcf_plies':8,'vcf_nodes':vcf,'vct_plies':8,'vct_nodes':vct}
                with DiskNodes(Path(directory)/f'{number}.db',{'leaf_limits':limits}) as db:
                    with db.transaction():root=db.intern(bytes.fromhex(native.facts(moves)['key']),moves)
                    solver=DiskSolver(db,native,0)
                    self.assertEqual(solver.solve(root,work=work,seconds=10)['outcome'],'UnverifiedWin')
                    self.assertEqual(solver.telemetry()[counter],1)
                    self.assertEqual(solver.pn_expansions,0)
                    if counter!='immediate_hits':
                        with self.assertRaises(ResourceStop):solver.verify_witness(root,max_nodes=1,seconds=10)
                    self.assertEqual(solver.verify_witness(root,max_nodes=work,seconds=10),'ProvenWin')
                    solver.export(root,Path(directory)/f'{number}.rmp')
            moves=[110,0,111,2,112,15]
            limits={'vcf_plies':8,'vcf_nodes':1,'vct_plies':8,'vct_nodes':1}
            with DiskNodes(Path(directory)/'unknown.db',{'leaf_limits':limits}) as db:
                with db.transaction():root=db.intern(bytes.fromhex(native.facts(moves)['key']),moves)
                solver=DiskSolver(db,native,0)
                self.assertEqual(solver.solve(root,work=3,seconds=10)['outcome'],'Unknown')
                self.assertEqual(db.node(root).outcome,0)
                self.assertEqual(solver.pn_expansions,0)

if __name__=='__main__':unittest.main()
