"""Immutable experiment identity and crash-resumable paired Arena journal.

Each subprocess runs one leg. Completed legs are never counted twice; unfinished
pairs remain raw events and contribute nothing to the primary paired statistic.
Concurrency is deliberately one match here, distinct from engine SMP threads.
"""

import argparse
import csv
import hashlib
import io
import json
import os
import platform
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'training'))
from manifest import save_manifest
from provenance import object_hash, sidecar, validate_evidence

from paired_stats import summarize


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def immutable(path, value):
    save_manifest(path, value)


def append(path, event):
    with path.open('a', encoding='utf-8') as output:
        output.write(json.dumps(event, sort_keys=True) + '\n')
        output.flush()
        os.fsync(output.fileno())


def read_events(path):
    if not path.exists():
        return []
    if path.stat().st_size > 256 * 1024 * 1024:
        raise ValueError('experiment journal exceeds 256 MiB')
    raw = path.read_bytes()
    if raw and not raw.endswith(b'\n'):
        # Preserve the torn tail before resuming; never ignore a complete event.
        split = raw.rfind(b'\n') + 1
        tail = raw[split:]
        backup = path.with_suffix('.torn-' + hashlib.sha256(tail).hexdigest()[:16])
        if not backup.exists():
            backup.write_bytes(tail)
        path.write_bytes(raw[:split])
        raw = raw[:split]
    return [json.loads(line) for line in raw.splitlines()]


def completed_games(events):
    result = {}
    for event in events:
        if event['status'] == 'completed':
            key = event['game_id']
            if key in result:
                raise ValueError('duplicate completed game in journal')
            result[key] = event
        elif event['status'] == 'infrastructure-failure':
            raise ValueError('journal contains infrastructure failure; inspect before a new experiment')
    return result


def statistics(completed, configuration, effective=None):
    counts = [0] * 5
    clusters = set()
    excluded = []
    half_pairs = []
    for pair in range(configuration['max_pairs']):
        legs = [completed.get(f'{pair}:{leg}') for leg in (1, 2)]
        if any(leg is None for leg in legs):
            if any(leg is not None for leg in legs):
                half_pairs.append(pair)
            continue
        first, second = legs
        cluster = first['row']['opening_key']
        if second['row']['opening_key'] != cluster:
            raise ValueError('paired opening identity mismatch')
        if cluster in clusters:
            excluded.append(pair)
            continue
        clusters.add(cluster)
        score = sum({'A': 2, 'B': 0, 'draw': 1}[leg['row']['winner']] for leg in legs)
        counts[score] += 1
    summary = summarize(counts, **configuration['sprt'], max_pairs=configuration['max_pairs'])
    eligible = (configuration.get('suite_role') == 'confirmation'
                and configuration.get('stop_rule') == 'fixed_pairs'
                and summary['pairs'] == configuration['max_pairs']
                and effective is not None
                and effective['limits']['turn_hard_ms'] is not None
                and effective['limits']['work'] is None
                and summary['score_ci95_hoeffding'][0] > .5)
    return {**summary,
            'incomplete_pairs': half_pairs, 'repeated_opening_clusters': excluded,
            'completed_games': len(completed), 'promotion_eligible': eligible}


def describe(arena, arguments):
    """The Rust parser is the only interpreter of player/limit options."""
    if not isinstance(arguments, list) or any(not isinstance(arg, str) for arg in arguments):
        raise ValueError('Arena arguments must be a string array')
    result = subprocess.run([str(arena), *arguments, '--describe'], check=True,
                            capture_output=True, text=True, timeout=15)
    effective = json.loads(result.stdout)
    if effective.get('schema') != 2 or effective['engine']['sha256'] != digest(arena):
        raise ValueError('Arena configuration identity/schema mismatch')
    return effective


def verify_events(completed, manifest):
    manifest_hash = object_hash(manifest)
    effective = manifest['effective']
    effective_hash = object_hash(effective)
    for game_id, event in completed.items():
        if event.get('manifest_sha256') != manifest_hash or event.get('effective_sha256') != effective_hash:
            raise ValueError('game event belongs to a different experiment/configuration')
        pair, leg = map(int, game_id.split(':'))
        if not 0 <= pair < manifest['configuration']['max_pairs'] or leg not in (1, 2):
            raise ValueError('event pair/leg out of range')
        row = event['row']
        if (int(row['pair']) != pair + 1 or int(row['leg']) != leg
                or row['opening_key'] != effective['openings'][pair]
                or row['a_color'] != ('Black' if leg == 1 else 'White')
                or row['winner'] not in ('A', 'B', 'draw')):
            raise ValueError('game event does not match scheduled pair/opening/colors')


def run(configuration, output):
    arena = Path(configuration['arena']).resolve()
    arguments = configuration.get('arguments', [])
    if any(arg in ('--pairs', '--pair-start', '--leg', '--describe') for arg in arguments):
        raise ValueError('pair selection belongs to the experiment runner')
    if not 1 <= configuration['max_pairs'] <= 10000:
        raise ValueError('max_pairs must be explicitly bounded')
    if configuration.get('stop_rule', 'paired_sprt') not in ('paired_sprt', 'fixed_pairs'):
        raise ValueError('unsupported stop rule')
    if configuration.get('suite_role', 'smoke') not in ('smoke', 'tuning', 'confirmation'):
        raise ValueError('unsupported opening suite role')
    # Validate statistical parameters before launching any games.
    summarize([0] * 5, **configuration['sprt'], max_pairs=configuration['max_pairs'])
    effective = describe(arena, arguments)
    if configuration['max_pairs'] > len(effective['openings']):
        raise ValueError('requested pair cap exceeds described opening suite')
    inputs = dict(effective['inputs_sha256'])
    for path in (Path(__file__), Path(__file__).with_name('paired_stats.py')):
        inputs[str(path.resolve())] = digest(path)
    threads = [player['threads'] for player in effective['players']]
    if any(t is not None and (t < 1 or t > (os.cpu_count() or 1)) for t in threads):
        raise ValueError('engine threads exceed available logical CPUs')
    model_evidence = {}
    for player in effective['players']:
        if player['evaluator'] != 'learned':
            continue
        model = Path(player['model']['path'])
        if sidecar(model, 'evidence').exists():
            evidence = validate_evidence(model)
            inputs.update(evidence['inputs_sha256'])
            model_evidence[player['model']['sha256']] = {'path': str(sidecar(model, 'evidence').resolve()),
                                                        'sha256': digest(sidecar(model, 'evidence'))}
        elif configuration.get('suite_role') == 'confirmation':
            raise ValueError('confirmation requires completed export/calibration/integer evidence')
    for path in configuration.get('extra_inputs', []):
        inputs[str(Path(path).resolve())] = digest(path)
    manifest = {'version': 2, 'configuration': configuration, 'inputs_sha256': inputs,
                'effective': effective, 'model_evidence': model_evidence,
                'platform': platform.platform(), 'cpu': platform.processor(),
                'logical_cpus': os.cpu_count(), 'python': sys.version,
                'concurrent_games': 1, 'engine_threads': threads,
                'rules': '15x15-freestyle', 'paired_colors': True,
                'tt_policy': 'fresh-per-game-warm-between-moves', 'book': False,
                'duplicate_cluster_policy': 'first-pair-only',
                'failures': 'Rust player failure forfeits; runner failure halts experiment'}
    output.mkdir(parents=True, exist_ok=True)
    immutable(output / 'manifest.json', manifest)
    journal = output / 'events.jsonl'
    completed = completed_games(read_events(journal))
    verify_events(completed, manifest)
    for pair in range(configuration['max_pairs']):
        before = statistics(completed, configuration, effective)
        if configuration.get('stop_rule', 'paired_sprt') == 'paired_sprt' and before['decision'] != 'inconclusive':
            break
        for leg in (1, 2):
            game_id = f'{pair}:{leg}'
            if game_id in completed:
                continue
            if any(digest(path) != value for path, value in inputs.items()):
                raise ValueError('experiment input changed while running')
            append(journal, {'status': 'started', 'game_id': game_id})
            command = [str(arena), *arguments, '--pairs', '1', '--pair-start', str(pair), '--leg', str(leg)]
            try:
                process = subprocess.run(command, capture_output=True, text=True, check=True,
                                         timeout=configuration.get('game_timeout_seconds', 60))
                descriptions = [line.removeprefix('EFFECTIVE_CONFIG ') for line in process.stderr.splitlines()
                                if line.startswith('EFFECTIVE_CONFIG ')]
                if len(descriptions) != 1 or json.loads(descriptions[0]) != effective:
                    raise ValueError('actual Arena configuration differs from preflight')
                if any(digest(path) != value for path, value in inputs.items()):
                    raise ValueError('experiment input changed during a game')
                rows = list(csv.DictReader(io.StringIO(process.stdout)))
                if len(rows) != 1 or rows[0]['leg'] != str(leg) or int(rows[0]['pair']) != pair + 1:
                    raise ValueError('Arena did not return exactly the requested completed leg')
                row = rows[0]
                if row['winner'] not in ('A', 'B', 'draw'):
                    raise ValueError('invalid winner')
                event = {'status': 'completed', 'game_id': game_id, 'row': row,
                         'manifest_sha256': object_hash(manifest), 'effective_sha256': object_hash(effective),
                         'configuration_log': process.stderr}
                verify_events({game_id: event}, manifest)
            except Exception as error:
                append(journal, {'status': 'infrastructure-failure', 'game_id': game_id, 'error': str(error)})
                raise
            append(journal, event)
            completed[game_id] = event
    result = statistics(completed, configuration, effective)
    result['scheduled_pair_cap_reached'] = len(completed) == 2 * configuration['max_pairs']
    (output / 'statistics.json').write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(result, indent=2))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    run(json.loads(args.config.read_text(encoding='utf-8')), args.output)


if __name__ == '__main__':
    main()
