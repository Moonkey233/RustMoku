"""Bounded operating-characteristic smoke for the paired profile SPRT."""

import argparse
import json
import random

from paired_stats import logistic, summarize


def simulate(trials, pairs, seed):
    rng = random.Random(seed)
    result = {'trials': trials, 'pair_cap': pairs, 'seed': seed, 'h0': 0, 'h1': 100, 'cases': {}}
    for correlation in ('binomial-four-half-points', 'perfectly-correlated-legs'):
        for hypothesis, elo in (('h0', 0), ('h1', 100)):
            decisions = {'accept-h0': 0, 'accept-h1': 0, 'inconclusive': 0}
            p = logistic(elo)
            for _ in range(trials):
                counts = [0] * 5
                for _ in range(pairs):
                    score = (sum(rng.random() < p for _ in range(4)) if correlation.startswith('binomial')
                             else 4 * int(rng.random() < p))
                    counts[score] += 1
                    decision = summarize(counts, h0=0, h1=100, max_pairs=pairs)['decision']
                    if decision != 'inconclusive':
                        break
                decisions[decision] += 1
            result['cases'][f'{correlation}/{hypothesis}'] = decisions
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--trials', type=int, default=100)
    parser.add_argument('--max-pairs', type=int, default=100)
    parser.add_argument('--seed', type=int, default=7)
    args = parser.parse_args()
    if not 1 <= args.trials * args.max_pairs <= 100000:
        raise ValueError('simulation budget must be 1..100000 trial-pairs')
    print(json.dumps(simulate(args.trials, args.max_pairs, args.seed), indent=2))


if __name__ == '__main__':
    main()
