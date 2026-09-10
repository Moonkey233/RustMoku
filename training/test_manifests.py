"""Crash injection for immutable JSON publication, including real abrupt exit."""
import concurrent.futures
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from manifest import CorruptManifestError, ManifestMismatchError, read_manifest, save_manifest
from common import save_split_manifest


class ImmutableManifests(unittest.TestCase):
    def test_all_manifest_roles_use_no_clobber_atomic_publication(self):
        with tempfile.TemporaryDirectory() as directory:
            for role in ('run', 'dataset', 'shard', 'split', 'pipeline', 'experiment'):
                path = Path(directory) / f'{role}.json'
                for phase in ('after_partial_write', 'before_publish', 'after_publish'):
                    def interrupt(name):
                        if name == phase:
                            raise OSError('injected interruption')
                    with patch('manifest._fault_point', side_effect=interrupt):
                        with self.assertRaises(OSError):
                            save_split_manifest(path, {'version': 1, 'role': role})
                    if path.exists():
                        self.assertEqual(read_manifest(path)['role'], role)
                    save_split_manifest(path, {'version': 1, 'role': role})
                    before = path.read_bytes()
                    with self.assertRaises(ManifestMismatchError):
                        save_split_manifest(path, {'version': 2, 'role': role})
                    self.assertEqual(path.read_bytes(), before)
                    path.unlink()  # Synthetic fixture only, for the next fault point.

    def test_abrupt_process_exit_before_and_after_publish_is_resumable(self):
        with tempfile.TemporaryDirectory() as directory:
            for phase in ('after_partial_write', 'before_publish', 'after_publish'):
                path = Path(directory) / f'{phase}.json'
                code = ("import os,sys; from pathlib import Path; import manifest; "
                        "manifest._fault_point=lambda name: os._exit(31) if name==sys.argv[2] else None; "
                        "manifest.save_manifest(Path(sys.argv[1]), {'version':1})")
                result = subprocess.run([sys.executable, '-c', code, str(path), phase],
                                        cwd=Path(__file__).parent, timeout=10)
                self.assertEqual(result.returncode, 31)
                self.assertEqual(path.exists(), phase == 'after_publish')
                save_manifest(path, {'version': 1})
                self.assertEqual(read_manifest(path), {'version': 1})

    def test_published_truncation_is_distinct_and_never_silently_replaced(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'run.json'
            path.write_bytes(b'{"version":')
            with self.assertRaisesRegex(CorruptManifestError, 'explicit recovery'):
                save_manifest(path, {'version': 1})
            self.assertEqual(path.read_bytes(), b'{"version":')
            path.write_text(json.dumps({'version': 2}))
            with self.assertRaises(ManifestMismatchError):
                save_manifest(path, {'version': 1})

    def test_concurrent_publish_cannot_replace_the_winner(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'run.json'
            def publish(value):
                try:
                    save_manifest(path, {'version': value})
                    return value
                except ManifestMismatchError:
                    return None
            with concurrent.futures.ThreadPoolExecutor(2) as pool:
                results = list(pool.map(publish, (1, 2)))
            winner = next(value for value in results if value is not None)
            self.assertEqual(results.count(None), 1)
            self.assertEqual(read_manifest(path), {'version': winner})


if __name__ == '__main__':
    unittest.main()
