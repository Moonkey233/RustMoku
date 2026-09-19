import dataclasses
import tempfile
import unittest
from pathlib import Path
from common import DATA_HEADER, DATA_MAGIC, DATA_RECORD_PREFIX, DATA_QUALITY, DatasetFile, eligible_label

class ForcedBlockLabels(unittest.TestCase):
    def test_forced_move_tag_never_supplies_an_exact_or_heuristic_value_target(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'forced.rmd'
            def payload(exact):
                return (DATA_HEADER.pack(DATA_MAGIC,2,0,1)+DATA_RECORD_PREFIX.pack(0,0,0,112,-123,9,exact)
                        +bytes(58)+DATA_QUALITY.pack(0,255,0,1,10000))
            path.write_bytes(payload(0))
            with DatasetFile(path) as data:
                record=data[0]
                self.assertFalse(record.exact)
                self.assertEqual(record.policy_move,112)
                self.assertFalse(eligible_label(record))
                self.assertFalse(eligible_label(dataclasses.replace(record,exact=True)))
            path.write_bytes(payload(1))
            with DatasetFile(path) as data:
                with self.assertRaisesRegex(ValueError,'forced-block'):data[0]
