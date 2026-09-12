import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from budget import ExplorationBudget


class BudgetTests(unittest.TestCase):
    def test_receipt_survives_restart_and_changed_limit_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / 'budget.json'
            budget = ExplorationBudget(path, seconds=5)
            budget.run([sys.executable, '-c', 'pass'], timeout=1, artifact_root=root, check=True)
            state = json.loads(path.read_text())
            self.assertGreater(state['charged_seconds'], 0)
            state['charged_seconds'] = 5
            path.write_text(json.dumps(state))
            with self.assertRaisesRegex(ValueError, 'exhausted'):
                ExplorationBudget(path, seconds=5).run([sys.executable, '-c', 'pass'], timeout=1, artifact_root=root)
            with self.assertRaisesRegex(ValueError, 'mismatch'):
                ExplorationBudget(path, seconds=6).run([sys.executable, '-c', 'pass'], timeout=1, artifact_root=root)

    def test_timeout_is_charged_and_failed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            budget = ExplorationBudget(root / 'budget.json', seconds=5)
            with self.assertRaises(subprocess.TimeoutExpired):
                budget.run([sys.executable, '-c', 'import time; time.sleep(10)'], timeout=.1, artifact_root=root)
            state = json.loads((root / 'budget.json').read_text())
            self.assertGreaterEqual(state['charged_seconds'], .1)
            self.assertEqual(state['runs'][0]['status'], 'interrupted-or-failed')
