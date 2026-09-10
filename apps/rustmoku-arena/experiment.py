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

from paired_stats import summarize


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def immutable(path, value):
    text = json.dumps(value, sort_keys=True, indent=2) + '\n'
    if path.exists():
        if path.read_text(encoding='utf-8') != text:
            raise ValueError(f'experiment identity mismatch: {path}')
    else:
        with path.open('x', encoding='utf-8') as output:
            output.write(text)


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


def statistics(completed, configuration):
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
                and '--move-ms' in configuration.get('arguments', [])
                and '--nodes' not in configuration.get('arguments', [])
                and summary['score_ci95_hoeffding'][0] > .5)
    return {**summary,
            'incomplete_pairs': half_pairs, 'repeated_opening_clusters': excluded,
            'completed_games': len(completed), 'promotion_eligible': eligible}


def run(configuration, output):
    arena = Path(configuration['arena']).resolve()
    arguments = configuration.get('arguments', [])
    if any(arg in ('--pairs', '--pair-start', '--leg') for arg in arguments):
        raise ValueError('pair selection belongs to the experiment runner')
    if not 1 <= configuration['max_pairs'] <= 10000:
        raise ValueError('max_pairs must be explicitly bounded')
    if configuration.get('stop_rule', 'paired_sprt') not in ('paired_sprt', 'fixed_pairs'):
        raise ValueError('unsupported stop rule')
    if configuration.get('suite_role', 'smoke') not in ('smoke', 'tuning', 'confirmation'):
        raise ValueError('unsupported opening suite role')
    # Validate statistical parameters before launching any games.
    summarize([0] * 5, **configuration['sprt'], max_pairs=configuration['max_pairs'])
    inputs = {str(arena): digest(arena)}
    threads = [1, 1]
    for i, flag in enumerate(arguments):
        if flag in ('--a-model', '--b-model', '--a-external', '--b-external', '--opening-record'):
            path = Path(arguments[i + 1]).resolve()
            inputs[str(path)] = digest(path)
        if flag in ('--a-threads', '--b-threads'):
            threads[0 if flag == '--a-threads' else 1] = int(arguments[i + 1])
    if any(t < 1 or t > (os.cpu_count() or 1) for t in threads):
        raise ValueError('engine threads exceed available logical CPUs')
    for path in configuration.get('extra_inputs', []):
        inputs[str(Path(path).resolve())] = digest(path)
    manifest = {'version': 1, 'configuration': configuration, 'inputs_sha256': inputs,
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
    for pair in range(configuration['max_pairs']):
        before = statistics(completed, configuration)
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
                rows = list(csv.DictReader(io.StringIO(process.stdout)))
                if len(rows) != 1 or rows[0]['leg'] != str(leg) or int(rows[0]['pair']) != pair + 1:
                    raise ValueError('Arena did not return exactly the requested completed leg')
                row = rows[0]
                if row['winner'] not in ('A', 'B', 'draw'):
                    raise ValueError('invalid winner')
                event = {'status': 'completed', 'game_id': game_id, 'row': row,
                         'configuration_log': process.stderr}
            except Exception as error:
                append(journal, {'status': 'infrastructure-failure', 'game_id': game_id, 'error': str(error)})
                raise
            append(journal, event)
            completed[game_id] = event
    result = statistics(completed, configuration)
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
