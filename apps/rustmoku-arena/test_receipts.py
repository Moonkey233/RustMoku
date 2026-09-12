"""Small adversarial receipt fixtures; no engine processes or match budget."""
import copy
import json
import unittest
from unittest.mock import patch
from types import SimpleNamespace
from experiment import verify_game_record


class ReceiptTests(unittest.TestCase):
    def fixture(self):
        row = dict(pair='1', leg='1', winner='B', a_color='Black', searched_moves='2',
                   plies='2', opening_key='opening', failure='player-0:time-forfeit')
        record = dict(schema=1, pair=1, leg=1, winner='B', record='legal replay fixture',
                      termination=row['failure'], clocks_ms=[0, 985], move_clocks=[
                          dict(move=112, player=0, elapsed_ms=20, clocks_ms=[980, 990]),
                          dict(move=113, player=1, elapsed_ms=15, clocks_ms=[980, 985])])
        manifest = dict(configuration={'arena': 'fixture'}, effective={'limits': {
            'clock_ms': 1000, 'increment_ms': 10, 'turn_hard_ms': 100}})
        replay = dict(plies=2, prefix_keys=['opening'], moves=[112, 113], winner='ongoing')
        return dict(row=row, game_record=record), manifest, replay

    def verify(self, event, manifest, replay):
        with patch('experiment.subprocess.run', return_value=SimpleNamespace(stdout=json.dumps(replay))):
            verify_game_record(event, manifest)

    def test_valid_clock_and_forfeit(self):
        self.verify(*self.fixture())

    def test_semantically_invalid_receipts(self):
        event, manifest, replay = self.fixture()
        changes = [
            lambda e: e['game_record'].update(termination='player-0:protocol-error'),
            lambda e: e['game_record'].update(termination='terminal'),
            lambda e: e['game_record']['move_clocks'][0].update(player=1),
            lambda e: e['game_record']['move_clocks'][1].update(clocks_ms=[979, 985]),
            lambda e: e['game_record']['move_clocks'][1].update(clocks_ms=[980, 999]),
            lambda e: e['game_record']['move_clocks'][1].update(elapsed_ms=101),
            lambda e: e['game_record'].update(clocks_ms=[0, 984]),
            lambda e: e['game_record'].update(clocks_ms=[None, None]),
        ]
        for change in changes:
            with self.subTest(change=change):
                invalid = copy.deepcopy(event)
                change(invalid)
                with self.assertRaises(ValueError):
                    self.verify(invalid, manifest, replay)

    def test_wrong_forfeit_winner_and_startup_after_moves(self):
        for reason in ('player-1:time-forfeit', 'player-0-startup:error'):
            event, manifest, replay = self.fixture()
            event['row']['failure'] = reason
            event['game_record']['termination'] = reason
            with self.assertRaises(ValueError):
                self.verify(event, manifest, replay)

    def test_duration_rounding_and_no_clock(self):
        event, manifest, replay = self.fixture()
        event['game_record']['move_clocks'][1]['clocks_ms'][1] -= 1
        event['game_record']['clocks_ms'][1] -= 1
        self.verify(event, manifest, replay)
        manifest['effective']['limits']['clock_ms'] = None
        for move in event['game_record']['move_clocks']:
            move['clocks_ms'] = [None, None]
        event['game_record']['clocks_ms'] = [None, None]
        self.verify(event, manifest, replay)


if __name__ == '__main__':
    unittest.main()
