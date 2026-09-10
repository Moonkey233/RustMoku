"""Pentanomial pair statistics with constrained multinomial profile likelihood.

The observational unit is one independently selected opening with exchanged
colors. The generalized SPRT uses the maximum multinomial likelihood at each
hypothesized mean. Its Wald boundaries are asymptotic for composite hypotheses;
finite-sample operating characteristics must be calibrated for a registered
opening distribution. LOS is a pair-CLT descriptive statistic, not a stop rule.
"""

import math
from statistics import NormalDist


SCORES = (0, .25, .5, .75, 1)


def logistic(elo):
    return 1 / (1 + 10 ** (-elo / 400))


def elo(score):
    if score <= 0:
        return '-infinity'
    if score >= 1:
        return '+infinity'
    return 400 * math.log10(score / (1 - score))


def fitted_probabilities(counts, mean):
    """Concave constrained MLE, including unobserved boundary support.

    KKT gives p_i=f_i/(1+t*(x_i-mean)). If the optimum is at an
    endpoint, the missing mass belongs to its unobserved endpoint cell.
    No pseudocount changes the likelihood or sparse-sample meaning.
    """
    if len(counts) != 5 or any(type(n) is not int or n < 0 for n in counts):
        raise ValueError('expected five nonnegative integer pair counts')
    total = sum(counts)
    if not total or not 0 < mean < 1:
        raise ValueError('positive sample and interior hypothesis mean required')
    frequencies = [n / total for n in counts]
    low, high = -1 / (1 - mean), 1 / mean

    def gradient(t):
        result = 0.0
        for f, x in zip(frequencies, SCORES):
            if f:
                denominator = 1 + t * (x - mean)
                if denominator <= 0:
                    return math.copysign(math.inf, x - mean)
                result += f * (x - mean) / denominator
        return result

    boundary = None
    if gradient(low) <= 0:
        t, boundary = low, 4
    elif gradient(high) >= 0:
        t, boundary = high, 0
    else:
        for _ in range(80):
            mid = (low + high) / 2
            if gradient(mid) > 0:
                low = mid
            else:
                high = mid
        t = (low + high) / 2
    probabilities = [f / (1 + t * (x - mean)) if f else 0
                     for f, x in zip(frequencies, SCORES)]
    if boundary is not None:
        probabilities[boundary] = max(0, 1 - sum(probabilities))
    if (abs(sum(probabilities) - 1) > 1e-9
            or abs(sum(p * x for p, x in zip(probabilities, SCORES)) - mean) > 1e-9):
        raise ArithmeticError('constrained likelihood failed normalization')
    return probabilities


def log_likelihood(counts, mean):
    probabilities = fitted_probabilities(counts, mean)
    return sum(n * math.log(p) for n, p in zip(counts, probabilities) if n)


def summarize(counts, h0=0, h1=5, alpha=.05, beta=.05, max_pairs=1000):
    if not h0 < h1 or not 0 < alpha < .5 or not 0 < beta < .5 or max_pairs < 1:
        raise ValueError('invalid preregistered SPRT parameters')
    if len(counts) != 5 or any(type(n) is not int or n < 0 for n in counts):
        raise ValueError('expected five nonnegative integer pair counts')
    n = sum(counts)
    lower, upper = math.log(beta / (1 - alpha)), math.log((1 - beta) / alpha)
    result = {'model': 'pentanomial-profile-likelihood-v1', 'sample_unit': 'independent-opening-pair',
              'pairs': n, 'counts': list(counts), 'h0_elo': h0, 'h1_elo': h1,
              'alpha': alpha, 'beta': beta, 'max_pairs': max_pairs,
              'lower_boundary': lower, 'upper_boundary': upper, 'decision': 'inconclusive',
              'boundary_calibration': 'asymptotic-composite-hypotheses'}
    if not n:
        return {**result, 'elo': None, 'los_pair_clt': None, 'llr': 0, 'ci95_hoeffding': ['-infinity', '+infinity'], 'score_ci95_hoeffding': [0, 1]}
    mean = sum(count * score for count, score in zip(counts, SCORES)) / n
    variance = sum(count * (score - mean) ** 2 for count, score in zip(counts, SCORES)) / max(1, n - 1)
    llr = log_likelihood(counts, logistic(h1)) - log_likelihood(counts, logistic(h0))
    radius = math.sqrt(math.log(40) / (2 * n))
    result.update(elo=elo(mean), mean_score=mean, llr=llr,
                  ci95_hoeffding=[elo(max(0, mean - radius)), elo(min(1, mean + radius))],
                  score_ci95_hoeffding=[max(0, mean - radius), min(1, mean + radius)],
                  los_pair_clt=NormalDist().cdf((mean - .5) / math.sqrt(variance / n)) if variance > 0 else None)
    if llr >= upper:
        result['decision'] = 'accept-h1'
    elif llr <= lower:
        result['decision'] = 'accept-h0'
    elif n >= max_pairs:
        result['cap_reached'] = True
    return result
