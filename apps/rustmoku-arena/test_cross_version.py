"""Protocol fixtures validate independent process identity, never playing strength."""
import contextlib
import io
import tempfile
import unittest
from pathlib import Path
from cross_version import prepare
from experiment import run, completed_games, read_events, verify_events
from manifest import read_manifest


class IndependentProcessTests(unittest.TestCase):
    def test_independent_pipe_fixture_binds_resources_and_replays_both_legs(self):
        root = Path(__file__).resolve().parents[2]
        engine = root / 'target/release/rustmoku-data.exe'
        arena = root / 'target/release/rustmoku-arena.exe'
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
