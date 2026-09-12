"""Masked teacher comparisons; unobserved actions carry no negative label."""
import math
import random


def comparison(analysis, temperature):
    if not math.isfinite(temperature) or temperature <= 0:
        raise ValueError('temperature must be finite and positive')
    if analysis.get('perspective') != 'root-side-to-move':
        raise ValueError('unsupported score perspective')
    depth = analysis['completed_depth']
    accepted = []
    seen = set()
    for candidate in analysis['candidates']:
        at = candidate['move']
        if type(at) is not int or not 0 <= at < 225 or at in seen:
            raise ValueError('invalid or duplicate candidate')
        seen.add(at)
        if (depth > 0 and candidate['completed_depth'] == depth
                and candidate['nominal_depth_valid'] is True
                and candidate['bound'] == 'Exact'
                and candidate['source'] == 'AlphaBeta'
                and type(candidate['score']) is int
                and abs(candidate['score']) <= 10_000_000):
            accepted.append((at, candidate['score']))
    if len(accepted) < 2:
        return None
    maximum = max(score for _, score in accepted)
    weights = [math.exp((score - maximum) / temperature) for _, score in accepted]
    total = sum(weights)
    return {'version': 1, 'depth': depth, 'temperature': temperature,
            'moves': [at for at, _ in accepted], 'scores': [score for _, score in accepted],
            'probabilities': [weight / total for weight in weights]}


def explore(comparison, seed, *, protected_move=None):
    if protected_move is not None:
        return protected_move
    if comparison is None:
        return None
    return random.Random(seed).choices(comparison['moves'], comparison['probabilities'], k=1)[0]


def masked_loss(logits, indices, probabilities, *, ranking_scores=None):
    import torch
    selected = logits[indices]
    if ranking_scores is None:
        target = torch.tensor(probabilities, device=logits.device, dtype=logits.dtype)
        return -(target * selected.log_softmax(0)).sum()
    losses = []
    for i, left in enumerate(ranking_scores):
        for j in range(i + 1, len(ranking_scores)):
            right = ranking_scores[j]
            if left != right:
                direction = 1 if left > right else -1
                losses.append(torch.nn.functional.softplus(-direction * (selected[i] - selected[j])))
    return torch.stack(losses).mean() if losses else selected.sum() * 0


def validate_comparison(value):
    moves, scores, probabilities = (value[key] for key in ('moves', 'scores', 'probabilities'))
    if (value.get('version') != 1 or type(value.get('depth')) is not int or not 1 <= value['depth'] <= 255
            or not 2 <= len(moves) <= 225 or len(set(moves)) != len(moves)
            or len(scores) != len(moves) or len(probabilities) != len(moves)
            or any(type(at) is not int or not 0 <= at < 225 for at in moves)
            or any(type(score) is not int or abs(score) > 10_000_000 for score in scores)
            or not math.isfinite(value['temperature']) or value['temperature'] <= 0):
        raise ValueError('invalid teacher comparison')
    weights = [math.exp((score - max(scores)) / value['temperature']) for score in scores]
    total = sum(weights)
    if any(not math.isfinite(p) or abs(p - w / total) > 1e-12 for p, w in zip(probabilities, weights)):
        raise ValueError('teacher distribution does not match scores and temperature')
    return value

def build_sidecar(paths, destination, temperature):
    import contextlib
    import json
    import sqlite3
    import tempfile
    from pathlib import Path
    from dataset import publish_shard, file_hash
    with tempfile.TemporaryDirectory(dir=destination.parent) as directory:
        temporary = Path(directory) / 'comparisons.sqlite'
        with contextlib.closing(sqlite3.connect(temporary)) as connection:
            connection.execute('PRAGMA cache_size=-4096')
            connection.execute('CREATE TABLE comparisons(position TEXT PRIMARY KEY, payload TEXT NOT NULL)')
            for path in paths:
                with Path(path).open(encoding='utf-8') as stream:
                    while True:
                        line = stream.readline(65537)
                        if not line:
                            break
                        if len(line) > 65536:
                            raise ValueError('teacher candidate row exceeds size limit')
                        analysis = json.loads(line)
                        value = comparison(analysis, temperature)
                        if value is None:
                            continue
                        payload = json.dumps(value, sort_keys=True)
                        key = analysis['position_key']
                        if len(bytes.fromhex(key)) != 58:
                            raise ValueError('invalid comparison position identity')
                        old = connection.execute('SELECT payload FROM comparisons WHERE position=?', (key,)).fetchone()
                        if old is not None and old[0] != payload:
                            raise ValueError('inconsistent comparison for repeated position')
                        connection.execute('INSERT OR IGNORE INTO comparisons VALUES (?,?)', (key, payload))
                connection.commit()
        publish_shard(temporary, destination)
    return {'path': str(destination.resolve()), 'sha256': file_hash(destination)}
