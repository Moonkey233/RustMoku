import contextlib
import tempfile
import unittest
from pathlib import Path

from common import DataRecord
from audit_ab_labels import connect,ingest,consistency,depth_report,additional_checks


def record(key,value=0,depth=2,policy=1,exact=False,source=2,game=0):
    return DataRecord(game,1,0,policy,value,source,exact,bytes([key])+bytes(57),
        completed_depth=depth,requested_depth=6,termination=1,work=20000,
        trajectory_id=f'trajectory-{game}',lineage_id=f'lineage-{game}')


class LabelAuditTests(unittest.TestCase):
    def test_streaming_aggregation_and_qualified_view(self):
        with contextlib.ExitStack() as stack:
            directory=stack.enter_context(tempfile.TemporaryDirectory())
            db=stack.enter_context(contextlib.closing(connect(Path(directory)/'audit.sqlite',1,8)))
            ingest(db,((i,record(1,100 if i%2 else -100)) for i in range(5000)),
                   'train',100,qualified=lambda i:i%2==0)
            raw=consistency(db,'train','raw','1')
            self.assertEqual(raw['duplicate_occurrences_excluding_first'],4999)
            self.assertAlmostEqual(raw['deterministic_predictor_empirical_floor']['all_occurrences']['mae'],.5)
            selected=consistency(db,'train','qualified','qualified=1')
            self.assertEqual(selected['occurrences'],2500)
            self.assertEqual(selected['deterministic_predictor_empirical_floor']['all_occurrences']['mae'],0)
            self.assertLessEqual(db.execute('PRAGMA page_count').fetchone()[0],2048)

    def test_duplicates_depth_contradictions_and_policy(self):
        with contextlib.ExitStack() as stack:
            directory=stack.enter_context(tempfile.TemporaryDirectory())
            db=stack.enter_context(contextlib.closing(connect(Path(directory)/'audit.sqlite',1,8)))
            rows=[record(1,100),record(1,100),record(2,0,2),record(2,100,3),
                  record(3,-100,2,1,game=1),record(3,100,2,2,game=2),record(4,50)]
            ingest(db,enumerate(rows),'train',100)
            r=consistency(db,'train','ab','source=2 AND exact=0')
            self.assertEqual((r['occurrences'],r['unique_positions'],r['singleton_positions'],r['duplicate_positions']),(7,4,1,3))
            self.assertEqual(r['duplicate_occurrences_excluding_first'],3)
            s=r['duplicate_consistency']
            self.assertEqual(s['identical_raw_value']['positions'],1)
            self.assertEqual(s['different_raw_value']['positions'],2)
            self.assertEqual(s['different_policy_move']['positions'],1)
            self.assertEqual(s['different_completed_depth']['positions'],1)
            self.assertEqual(r['same_depth']['contradicting_groups'],1)
            floor=r['deterministic_predictor_empirical_floor']['all_occurrences']
            self.assertAlmostEqual(floor['mae'],1.5/7)
            self.assertAlmostEqual(floor['mse'],.625/7)
            self.assertEqual(db.execute('SELECT count(*) FROM position_stats').fetchone()[0],3)
            self.assertEqual(db.execute('SELECT max(raw_range),max(q_range) FROM duplicate_positions').fetchone()[:],(200,1.0))
            extra=additional_checks(db,r)
            self.assertEqual(extra['conflicting_keys_with_multiple_depths'],1)
            self.assertAlmostEqual(extra['board_and_depth_empirical_floor_all_source2_occurrences']['mae'],1/7)
            depths=depth_report(db,'source=2')['completed_depth']
            self.assertEqual([x['occurrences'] for x in depths],[6,1])
            self.assertEqual(db.execute('PRAGMA cache_size').fetchone()[0],-1024)
            self.assertEqual(db.execute('PRAGMA temp_store').fetchone()[0],1)
            self.assertEqual(db.execute('PRAGMA max_page_count').fetchone()[0],2048)

    def test_split_isolation_exact_exclusion_and_quality(self):
        with contextlib.ExitStack() as stack:
            directory=stack.enter_context(tempfile.TemporaryDirectory())
            db=stack.enter_context(contextlib.closing(connect(Path(directory)/'audit.sqlite',1,8)))
            ingest(db,[(0,record(1,0)),(1,record(2,0,exact=True)),(2,record(3,50,depth=0))],'train',100)
            ingest(db,[(3,record(1,100))],'test',100)
            all_ab=consistency(db,'overall','ab','source=2 AND exact=0')
            self.assertEqual(all_ab['occurrences'],2)
            self.assertEqual(all_ab['duplicate_positions'],1)
            for split in ('train','test'):
                isolated=consistency(db,split,'ab','source=2 AND exact=0 AND split=?',(split,))
                self.assertEqual(isolated['duplicate_positions'],0)
                self.assertEqual(isolated['deterministic_predictor_empirical_floor']['all_occurrences']['mae'],0)

    def test_all_source_floor_and_unknown_depth_not_claimed_as_same_horizon(self):
        with contextlib.ExitStack() as stack:
            directory=stack.enter_context(tempfile.TemporaryDirectory())
            db=stack.enter_context(contextlib.closing(connect(Path(directory)/'audit.sqlite',1,8)))
            rows=[record(1,-100,depth=None),record(1,100,depth=None),record(2,-10,source=3,exact=True),record(2,10,source=4,exact=True)]
            ingest(db,enumerate(rows),'train',100)
            r=consistency(db,'overall','all','1')
            self.assertAlmostEqual(r['deterministic_predictor_empirical_floor']['all_occurrences']['mae'],.75)
            self.assertAlmostEqual(r['deterministic_predictor_empirical_floor']['all_occurrences']['mse'],.625)
            ab=consistency(db,'overall','ab','source=2 AND exact=0')
            self.assertEqual(ab['same_depth']['contradicting_groups'],0)
            self.assertEqual(ab['same_depth']['unknown_depth_occurrences'],2)


if __name__=='__main__':unittest.main()
