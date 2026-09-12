"""Protocol fixtures validate independent process identity, never playing strength."""
import contextlib
import io
import os
import tempfile
import unittest
from pathlib import Path
from cross_version import prepare, freeze_command
from experiment import run, completed_games, read_events, verify_events
from manifest import read_manifest


class IndependentProcessTests(unittest.TestCase):
    def test_only_declared_file_inputs_are_frozen_relative_to_config(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            (base / 'engine.exe').write_bytes(b'engine')
            (base / 'model.bin').write_bytes(b'model')
            (base / '64').write_bytes(b'not a nodes argument')
            command = ['engine.exe', 'pipe', '--nodes', '64', '--model', 'model.bin']
            frozen = freeze_command(command, base, base / 'snapshots')
            self.assertEqual(frozen[1:4], ['pipe', '--nodes', '64'])
            self.assertEqual(Path(frozen[0]).read_bytes(), b'engine')
            self.assertEqual(Path(frozen[5]).read_bytes(), b'model')
            with self.assertRaises(FileNotFoundError):
                freeze_command(['missing.exe', 'pipe'], base, base / 'snapshots')
            with self.assertRaisesRegex(ValueError, 'flag/value'):
                freeze_command(['engine.exe', 'pipe', '--model'], base, base / 'snapshots')

    def test_independent_pipe_fixture_binds_resources_and_replays_both_legs(self):
        root = Path(__file__).resolve().parents[2]
        suffix = '.exe' if os.name == 'nt' else ''
        engine = root / f'target/release/rustmoku-data{suffix}'
        arena = root / f'target/release/rustmoku-arena{suffix}'
        command = [str(engine), 'pipe', '--depth', '1', '--nodes', '64', '--tt-mib', '1']
        config = {'arena': str(arena), 'candidate': command, 'baseline': command,
                  'move_ms': 1000, 'max_pairs': 1, 'game_timeout_seconds': 20}
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with self.assertRaisesRegex(ValueError, 'different actual executable hashes'):
                prepare(config, output / 'inputs')
            config['require_distinct_versions'] = False  # Same-build fixture only.
            prepared = prepare(config, output / 'inputs')
            self.assertEqual(prepared['process_players'][0]['effective']['nodes'], 64)
            with contextlib.redirect_stdout(io.StringIO()):
                report = run(prepared, output / 'match')
            self.assertEqual(report['completed_games'], 2)
            self.assertFalse(report['promotion_eligible'])
            manifest = read_manifest(output / 'match/manifest.json')
            events = completed_games(read_events(output / 'match/events.jsonl'))
            first = next(iter(events.values()))
            self.assertIn('record', first['game_record'])
            first['game_record']['record'] += 'invalid trailing content'
            with self.assertRaises(Exception):
                verify_events(events, manifest)
