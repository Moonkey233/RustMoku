import math
import random
import tempfile
import unittest
from pathlib import Path

from experiment import append, completed_games, read_events, statistics
from paired_stats import fitted_probabilities, log_likelihood, summarize


class PairedStatistics(unittest.TestCase):
    def test_endpoint_bernoulli_analytic_likelihood(self):
        counts = [13, 0, 0, 0, 7]
        for mean in (.1, .3, .5, .8):
            self.assertAlmostEqual(log_likelihood(counts, mean), 13 * math.log(1 - mean) + 7 * math.log(mean))

    def test_degenerate_and_missing_support(self):
        for counts in ([0, 0, 20, 0, 0], [20, 0, 0, 0, 0], [0, 0, 0, 0, 20], [1, 3, 5, 7, 11]):
            for mean in (.1, .49, .5, .51, .9):
                p = fitted_probabilities(counts, mean)
                self.assertAlmostEqual(sum(p), 1)
                self.assertAlmostEqual(sum(i / 4 * x for i, x in enumerate(p)), mean)
        self.assertIsNone(summarize([0] * 5)['elo'])
        self.assertEqual(summarize([0, 0, 20, 0, 0], max_pairs=20)['decision'], 'inconclusive')
        self.assertIsNone(summarize([0, 0, 20, 0, 0])['los_pair_clt'])

    def test_independent_grid_reference(self):
        # Independently enumerate feasible p0,p1,p2; solve p3,p4 using the
        # normalization and mean constraints. Coarse grid is a lower bound.
        counts, mean = [2, 3, 4, 3, 2], .55
        optimum = log_likelihood(counts, mean)
        best = -math.inf
        for a in range(1, 19):
            for b in range(1, 19 - a):
                for c in range(1, 19 - a - b):
                    p0, p1, p2 = a / 20, b / 20, c / 20
                    remaining = 1 - p0 - p1 - p2
                    p4 = 4 * (mean - .25 * p1 - .5 * p2 - .75 * remaining)
                    p3 = remaining - p4
                    if min(p3, p4) > 0:
                        best = max(best, sum(n * math.log(p) for n, p in zip(counts, [p0, p1, p2, p3, p4])))
        self.assertGreaterEqual(optimum + 1e-10, best)
        self.assertLess(optimum - best, .15)

    def test_torn_journal_and_duplicate_completed_rejection(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'events.jsonl'
            event = {'status': 'completed', 'game_id': '0:1'}
            append(path, event)
            with path.open('ab') as output:
                output.write(b'{"status":')
            self.assertEqual(read_events(path), [event])
            self.assertEqual(len(list(Path(directory).glob('*.torn-*'))), 1)
            with self.assertRaises(ValueError):
                completed_games([event, event])

    def test_half_pairs_and_opening_clusters(self):
        config = {'max_pairs': 2, 'sprt': {'h0': 0, 'h1': 5, 'alpha': .05, 'beta': .05}}
        completed = {'0:1': {'row': {'opening_key': 'same', 'winner': 'A'}}}
        self.assertEqual(statistics(completed, config)['pairs'], 0)
        completed['0:2'] = {'row': {'opening_key': 'same', 'winner': 'B'}}
        completed['1:1'] = completed['0:1']
        completed['1:2'] = completed['0:2']
        report = statistics(completed, config)
        self.assertEqual(report['counts'], [0, 0, 1, 0, 0])
        self.assertEqual(report['repeated_opening_clusters'], [1])


if __name__ == '__main__':
    unittest.main()
