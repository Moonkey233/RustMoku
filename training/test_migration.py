import tempfile
import unittest
from pathlib import Path
from common import DatasetFile, DATA_HEADER, LEGACY_RECORD_BYTES
from test_compact import fixture
from dataset import DatasetBundle, file_hash
from migrate import migrate


class MigrationTests(unittest.TestCase):
    def test_interleaved_legacy_records_migrate_without_changing_original(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, output = root / 'legacy.rmd', root / 'ordered.rmd'
            fixture(source, games=3, plies=3)
            data = source.read_bytes()
            rows = [data[DATA_HEADER.size + i*LEGACY_RECORD_BYTES:DATA_HEADER.size+(i+1)*LEGACY_RECORD_BYTES] for i in range(9)]
            source.write_bytes(data[:DATA_HEADER.size] + b''.join(rows[i] for i in (0,3,6,1,4,7,2,5,8)))
            before = file_hash(source)
            migrate(source, output)
            self.assertEqual(file_hash(source), before)
            with DatasetFile(output) as records:
                self.assertEqual([(row.game_id, row.ply) for row in records], [(g,p) for g in range(3) for p in range(3)])
            with DatasetBundle(output.with_suffix('.json')) as bundle:
                self.assertEqual(len(bundle), 9)
            with self.assertRaisesRegex(ValueError, 'distinct'):
                migrate(source, source)
