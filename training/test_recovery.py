"""Publication failure must preserve the previous readable artifact."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from checkpoint import load_checkpoint
from pipeline import publish_state
from dataset import file_hash


class RecoveryTests(unittest.TestCase):
    def test_json_recovered_artifact_paths_are_hashable(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'artifact'
            path.write_bytes(b'completed stage')
            saved = json.loads(json.dumps({str(path): file_hash(path)}))
            for name, digest in saved.items():
                self.assertEqual(file_hash(name), digest)

    def test_interrupted_state_publication_keeps_old_state(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'state.json'
            publish_state(path, {'status': 'complete'})
            with patch.object(Path, 'replace', side_effect=OSError('interrupted')):
                with self.assertRaises(OSError):
                    publish_state(path, {'status': 'running'})
            self.assertEqual(json.loads(path.read_text()), {'status': 'complete'})
            self.assertEqual(list(Path(directory).glob('*.partial')), [])

    def test_truncated_checkpoint_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'bad.pt'
            path.write_bytes(b'PK\x03\x04truncated')
            with self.assertRaises(ValueError):
                load_checkpoint(path)


if __name__ == '__main__':
    unittest.main()
