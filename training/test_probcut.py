import unittest
from probcut import fit_bucket


class ProbCutTests(unittest.TestCase):
    def test_empirical_fit_and_heldout_failure_are_separate(self):
        training = [dict(shallow=i, deep=2*i+7) for i in range(-40, 40)]
        heldout = [dict(shallow=i, deep=2*i+7) for i in range(-16, 16)]
        bucket, report = fit_bucket(training, heldout, 3, 1, 0)
        self.assertEqual(report['status'], 'accepted')
        self.assertEqual(bucket[3:5], [131072, 7])
        self.assertGreater(report['zero_failure_rate_95pct_upper'], .08)
        heldout[0]['deep'] -= 100
        rejected, report = fit_bucket(training, heldout, 3, 1, 0)
        self.assertIsNone(rejected)
        self.assertEqual(report['false_lower_predictions'], 1)

    def test_insufficient_and_out_of_distribution_disable(self):
        training = [dict(shallow=i, deep=i) for i in range(64)]
        self.assertIsNone(fit_bucket(training, training[:31], 3, 1, 0)[0])
        self.assertIsNone(fit_bucket(training, [dict(shallow=100, deep=100)] * 32, 3, 1, 0)[0])
