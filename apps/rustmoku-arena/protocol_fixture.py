"""Deterministic protocol test opponent, not a strength reference."""

import sys
import time

occupied = set()
mode = sys.argv[1] if len(sys.argv) > 1 else 'legal'


def choose():
    if mode == 'hang':
        time.sleep(60)
    if mode == 'crash':
        sys.exit(3)
    if mode == 'flood':
        print('x' * 5000, flush=True)
        return
    if mode == 'illegal':
        print('15,15', flush=True)
        return
    at = next(i for i in range(225) if i not in occupied)
    occupied.add(at)
    print(f'{at % 15},{at // 15}', flush=True)


for raw in sys.stdin:
    line = raw.strip()
    if line == 'START 15':
        print('OK', flush=True)
    elif line == 'BOARD':
        occupied.clear()
    elif line == 'DONE' or line == 'BEGIN':
        choose()
    elif line.startswith('TURN '):
        x, y = map(int, line[5:].split(','))
        occupied.add(y * 15 + x)
        choose()
    elif line.startswith('INFO '):
        pass
    elif line == 'END':
        break
    else:
        x, y, _ = map(int, line.split(','))
        occupied.add(y * 15 + x)
