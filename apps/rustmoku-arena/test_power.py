import unittest
from paired_stats import power_budget, monte_carlo_fixed_gate


class PowerTests(unittest.TestCase):
    def test_power_is_planning_and_zero_variance_is_not_free_evidence(self):
        budget = power_budget([0, 0, 100, 0, 0])
        self.assertGreater(budget['normal_approximate_independent_pairs'], 1000)
        self.assertGreater(budget['worst_case_hoeffding_independent_pairs'], budget['normal_approximate_independent_pairs'])

    def test_known_null_coverage_and_power_report_mc_uncertainty(self):
        null = monte_carlo_fixed_gate([.5, 0, 0, 0, .5], pairs=64, trials=1000, seed=7)
        alternative = monte_carlo_fixed_gate([0, 0, 0, 0, 1], pairs=64, trials=1000, seed=7)
        self.assertLess(null['type_I']['estimate'], .05)
        self.assertGreater(null['coverage']['estimate'], .95)
        self.assertGreater(alternative['power']['mc_ci95_wilson'][0], .99)
        self.assertGreater(null['type_I']['mc_ci95_wilson'][1], 0)
