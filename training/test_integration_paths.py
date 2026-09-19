import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from integration_paths import executable

class ExecutablePaths(unittest.TestCase):
    def test_override_is_exact_and_missing_override_never_falls_back(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'explicit engine';path.write_bytes(b'fixture')
            with patch.dict(os.environ,{'RUSTMOKU_DATA_EXE':str(path)}):
                self.assertEqual(executable('rustmoku-data'),path.resolve())
            with patch.dict(os.environ,{'RUSTMOKU_DATA_EXE':str(path)+'-missing'}):
                with self.assertRaisesRegex(RuntimeError,'RUSTMOKU_DATA_EXE'):executable('rustmoku-data')
