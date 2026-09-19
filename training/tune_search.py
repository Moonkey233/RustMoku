"""Offline paired ablations and deterministic, bounded-dimensional SPSA.

No search implementation lives here. Arena admission validates every serialized
profile. Tuning evidence never substitutes for independent model promotion.
"""
import argparse
import hashlib
import json
import math
import sys
from pathlib import Path

from manifest import save_manifest, read_manifest
from dataset import file_hash

ARENA_TOOLS = Path(__file__).resolve().parents[1] / 'apps' / 'rustmoku-arena'
sys.path.insert(0, str(ARENA_TOOLS))
import experiment

# Adapter for the published RMPROFILE1/2 base schema; Rust remains the validator.
BASE_FIELDS = ('contract', 'scale', 'aspiration', 'futility_base', 'futility_depth',
    'rfp_base', 'rfp_depth', 'razor_base', 'razor_depth', 'lmp1', 'lmp2', 'lmp3',
    'lmr_min_depth', 'lmr_min_index', 'iir_min_depth', 'strong_history',
    'history_bonus_factor', 'extension_budget', 'policy_lmr', 'singular',
    'lmr_stage1_depth', 'lmr_stage1_index', 'lmr_stage2_depth', 'lmr_stage2_index', 'qsearch_threes')
RESEARCH = dict(lmr_v2=0, lmr_divisor=4, lmr_cut_bonus=1, improving=0,
    improving_lmr_discount=1, improving_margin_percent=125, improving_lmp_bonus=4,
    iid=0, iid_min_depth=6, iid_reduction=3, policy_pruning=0, policy_max_depth=2,
    policy_tail_percent=10, competitive_tt=0, null_move=0, null_min_depth=6,
    null_reduction=3, normalized_scores=0, time_stability=250, time_drop=10000,
    growth_min_q8=64, growth_max_q8=2048, growth_initial_q8=384, growth_ema_weight=3,
    lmr_v2_policy=1)
FEATURES = ('lmr_v2', 'lmr_v2_policy', 'improving', 'iid', 'policy_lmr',
    'policy_pruning', 'singular', 'competitive_tt', 'null_move', 'qsearch_threes',
    'interior_vcf', 'interior_vct', 'calibrated_probcut', 'adaptive_root_candidates')
GROUPS = {
    'lmr': ('lmr_divisor', 'lmr_cut_bonus', 'lmr_min_index'),
    'improving': ('improving_lmr_discount', 'improving_margin_percent', 'improving_lmp_bonus'),
    'lmp': ('lmp1', 'lmp2', 'lmp3'),
    'futility': ('futility_base', 'futility_depth', 'rfp_base', 'rfp_depth'),
    'razor': ('razor_base', 'razor_depth'),
    'history': ('strong_history', 'history_bonus_factor'),
    'policy': ('policy_max_depth', 'policy_tail_percent'),
    'null': ('null_min_depth', 'null_reduction'),
    'time': ('time_stability', 'time_drop', 'growth_initial_q8', 'growth_ema_weight'),
}


def parse_profile(text):
    text = text.strip()
    research = dict(RESEARCH)
    if text.startswith('RMPROFILE3;'):
        parts = text.split(';')
        if not parts[1].startswith('base='):
            raise ValueError('missing profile base')
        text = parts[1][5:]
        seen = set()
        for part in parts[2:]:
            key, value = part.split('=')
            if key not in research or key in seen:
                raise ValueError('unknown/duplicate profile field')
            seen.add(key)
            research[key] = int(value)
        if seen | {'lmr_v2_policy'} != set(research):
            raise ValueError('incomplete named profile')
    parts = text.split(',')
    if parts[0] not in ('RMPROFILE1', 'RMPROFILE2') or len(parts) != (25 if parts[0] == 'RMPROFILE1' else 26):
        raise ValueError('unsupported profile schema')
    base = dict(zip(BASE_FIELDS, map(int, parts[1:]), strict=False))
    base.setdefault('qsearch_threes', 0)
    return {**base, **research}


def serialize_profile(values):
    # Always emit all named fields and the explicit qsearch bit. No defaults are
    # inferred by a future parser when replaying an old tuning candidate.
    base = 'RMPROFILE2,' + ','.join(str(int(values[key])) for key in BASE_FIELDS)
    return 'RMPROFILE3;base=' + base + ''.join(f';{key}={int(values[key])}' for key in RESEARCH)


def frozen_text(path, text):
    raw = (text + '\n').encode('utf-8')
    try:
        with path.open('xb') as stream:
            stream.write(raw)
    except FileExistsError:
        if path.read_bytes() != raw:
            raise ValueError(f'candidate profile changed: {path}')


def perturb(theta, bounds, seed, iteration, c):
    plus, minus = {}, {}
    for key in sorted(bounds):
        low, high = bounds[key]
        sign = 1 if hashlib.sha256(f'{seed}:{iteration}:{key}'.encode()).digest()[0] & 1 else -1
        radius = max(1.0, c * (high - low) / (iteration + 1) ** .101)
        plus[key] = max(low, min(high, round(theta[key] + sign * radius)))
        minus[key] = max(low, min(high, round(theta[key] - sign * radius)))
    return plus, minus


def update(theta, plus, minus, bounds, score, iteration, a):
    result = dict(theta)
    for key, (low, high) in bounds.items():
        separation = (plus[key] - minus[key]) / (high - low)
        if separation:
            gradient = (2 * score - 1) / separation
            result[key] = max(low, min(high, theta[key] + a / (iteration + 1) ** .602 * gradient * (high - low)))
    return result


def arguments(config, profiles, suite, overrides=None):
    args = ['--depth', str(config.get('depth', 64))]
    if config.get('stage') == 'time':
        args += ['--move-ms', str(config['move_ms'])]
    else:
        args += ['--nodes', str(config['nodes'])]
    for path in suite:
        args += ['--opening-record', str(Path(path).resolve())]
    for index, prefix in enumerate(('a', 'b')):
        args += [f'--{prefix}-profile', profiles[index]]
        if config.get('model'):
            args += [f'--{prefix}-model', str(Path(config['model']).resolve())]
        player = dict(config.get('player_options', {}))
        if overrides:
            player.update(overrides[index])
        for key, value in sorted(player.items()):
            if key in ('profile', 'model', 'evaluator', 'external', 'opening-db', 'opening-policy'):
                raise ValueError('tuning freezes one model/profile; empirical books are disabled')
            args += [f'--{prefix}-{key}', str(value)]
    return args


def feature_pair(config, base, name):
    if name not in FEATURES:
        raise ValueError('unknown ablation')
    profiles = [dict(base), dict(base)]
    overrides = [{}, {}]
    if name in base:
        if name == 'lmr_v2_policy' and not base['lmr_v2']:
            raise ValueError('lmr_v2_policy ablation requires an explicitly enabled lmr_v2 base')
        profiles[0][name], profiles[1][name] = 1, 0
    elif name in ('interior_vcf', 'interior_vct'):
        key = name.replace('_', '-')
        enabled = config.get('feature_values', {}).get(name)
        if not enabled:
            raise ValueError(f'{name} requires explicit plies:probe-work:total-work')
        overrides = [{key: enabled}, {key: '0:0:0'}]
    elif name == 'calibrated_probcut':
        calibration = config.get('feature_values', {}).get(name)
        if not calibration or 'probcut' in config.get('player_options', {}):
            raise ValueError('ProbCut ablation requires explicit valid calibration and a baseline without ProbCut')
        overrides[0]['probcut'] = calibration
    else:
        overrides = [{'adaptive-root-candidates': 'true'}, {'adaptive-root-candidates': 'false'}]
    return [serialize_profile(p) for p in profiles], overrides


def telemetry(output):
    totals = [dict(moves=0, depth_sum=0, elapsed_us=0, counters={}) for _ in range(2)]
    for event in experiment.completed_games(experiment.read_events(output / 'events.jsonl')).values():
        for total, counters in zip(totals, event['game_record'].get('search_counters', [{}, {}]), strict=True):
            for key, value in counters.items():
                total['counters'][key] = total['counters'].get(key, 0) + value
        for move in event['game_record']['move_clocks']:
            trace = move.get('search')
            if trace is None:
                continue
            total = totals[move['player']]
            total['moves'] += 1
            total['depth_sum'] += trace['completed_depth']
            total['elapsed_us'] += move.get('elapsed_us', 0)
    for total in totals:
        total['mean_depth'] = total['depth_sum'] / max(1, total['moves'])
        total['work_per_second'] = (total['counters'].get('work_nodes', 0) * 1e6 / total['elapsed_us']
                                    if total['elapsed_us'] else None)
    return totals


def run(config, output, execute=False):
    if config.get('version') != 1 or config.get('mode') not in ('ablation', 'spsa'):
        raise ValueError('expected version 1 tuning configuration')
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
    if config.get('stage') == 'time':
        if config['mode'] != 'spsa' or config.get('nodes') is not None or config.get('move_ms', 0) <= 0:
            raise ValueError('time tuning is a separate fixed-time SPSA stage')
    elif config.get('nodes', 0) <= 0 or config.get('move_ms') is not None:
        raise ValueError('search tuning requires paired fixed positive work, no time cap')
    suite, confirmation = config['tuning_openings'], config['confirmation_openings']
    if not suite or not confirmation or not 1 <= config['pairs'] <= len(suite):
        raise ValueError('explicit tuning and independent confirmation suites required')
    profile_path = Path(config['base_profile']).resolve()
    if profile_path.stat().st_size > 1024:
        raise ValueError('base profile too large')
    base = parse_profile(profile_path.read_text(encoding='utf-8-sig'))
    profile = serialize_profile(base)
    arena = Path(config['arena']).resolve()
    descriptions = [experiment.describe(arena, arguments(config, [profile, profile], paths)) for paths in (suite, confirmation)]
    if config['mode'] == 'ablation':
        names = config['features']
        if not names or len(set(names)) != len(names):
            raise ValueError('explicit unique ablations required')
        for name in names:
            profiles, overrides = feature_pair(config, base, name)
            experiment.describe(arena, arguments(config, profiles, suite, overrides))
    else:
        stage, bounds = config['stage'], config['bounds']
        if stage not in GROUPS or not bounds or not set(bounds) <= set(GROUPS[stage]):
            raise ValueError('SPSA parameters must belong to one small named stage')
        for key, limits in bounds.items():
            if (len(limits) != 2 or any(type(v) is not int for v in limits)
                    or not limits[0] <= base[key] <= limits[1] or limits[0] == limits[1]):
                raise ValueError('invalid bounds or base outside bounds')
        controls = config.get('spsa', {})
        a, c = controls.get('a', .01), controls.get('c', .1)
        if not all(math.isfinite(v) and 0 < v <= 1 for v in (a, c)) or config['steps'] <= 0:
            raise ValueError('invalid SPSA controls')
        theta = {key: float(base[key]) for key in bounds}
        candidates = perturb(theta, bounds, config['seed'], 0, c)
        experiment.describe(arena, arguments(config, [serialize_profile({**base, **values}) for values in candidates], suite))
    tuning_keys, confirmation_keys = [set(d['openings']) for d in descriptions]
    if tuning_keys & confirmation_keys or len(tuning_keys) != len(suite):
        raise ValueError('D4 duplicate tuning openings or tuning/confirmation leakage')
    inputs = {str(profile_path): file_hash(profile_path), str(Path(__file__).resolve()): file_hash(Path(__file__))}
    for path in (Path(experiment.__file__), ARENA_TOOLS / 'paired_stats.py',
                 Path(__file__).with_name('manifest.py'), Path(__file__).with_name('provenance.py')):
        inputs[str(path.resolve())] = file_hash(path)
    for description in descriptions:
        inputs.update(description['inputs_sha256'])
    identity = {'version': 1, 'configuration': config, 'inputs_sha256': inputs,
                'python': sys.version, 'base_profile': profile, 'tuning_keys': sorted(tuning_keys), 'confirmation_keys': sorted(confirmation_keys)}
    save_manifest(output / 'manifest.json', identity)
    if not execute:
        return identity

    def duel(name, profiles, overrides=None):
        if any(file_hash(Path(path)) != digest for path, digest in inputs.items()):
            raise ValueError('frozen tuning input changed')
        folder = output / name
        folder.mkdir(exist_ok=True)
        for label, candidate in zip(('a', 'b'), profiles, strict=True):
            frozen_text(folder / f'{label}.profile', candidate)
        match = {'arena': str(arena), 'arguments': arguments(config, profiles, suite, overrides),
                 'max_pairs': config['pairs'], 'stop_rule': 'fixed_pairs', 'suite_role': 'tuning',
                 'sprt': {'h0': 0, 'h1': 5, 'alpha': .05, 'beta': .05},
                 'game_timeout_seconds': config.get('game_timeout_seconds', 600), 'extra_inputs': list(inputs)}
        result = experiment.run(match, folder)
        if result['pairs'] != config['pairs'] or result.get('repeated_opening_clusters'):
            raise ValueError('incomplete or excluded tuning pairs')
        save_manifest(folder / 'tuning-result.json', {'statistics': result, 'telemetry': telemetry(folder)})
        return result['mean_score']

    if config['mode'] == 'ablation':
        names = config['features']
        if not names or len(set(names)) != len(names):
            raise ValueError('explicit unique ablations required')
        for name in names:
            profiles, overrides = feature_pair(config, base, name)
            duel('ablation-' + name, profiles, overrides)
        return
    for iteration in range(config['steps']):
        plus, minus = perturb(theta, bounds, config['seed'], iteration, c)
        profiles = [serialize_profile({**base, **values}) for values in (plus, minus)]
        # Immutable state is recomputed on resume, including every completed duel.
        score = duel(f'spsa-{iteration:06d}', profiles)
        theta = update(theta, plus, minus, bounds, score, iteration, a)
        save_manifest(output / f'state-{iteration:06d}.json', {'iteration': iteration, 'theta': theta, 'score': score})
        frozen_text(output / f'candidate-{iteration:06d}.profile', serialize_profile({**base, **{key: round(value) for key, value in theta.items()}}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--run', action='store_true', help='otherwise freeze/validate only; do not play games')
    args = parser.parse_args()
    run(read_manifest(args.config), args.output, args.run)


if __name__ == '__main__':
    main()
