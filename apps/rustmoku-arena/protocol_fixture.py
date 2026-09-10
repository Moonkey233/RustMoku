"""Deterministic protocol test opponent, not a strength reference."""

import sys
import time

occupied = set()
mode = sys.argv[1] if len(sys.argv) > 1 else 'legal'
info = {}
fresh_clock = False


def reply(text):
    ending = b'\r' if mode == 'cr' else b'\n'
    sys.stdout.buffer.write(text.encode() + ending)
    sys.stdout.buffer.flush()


def choose():
    global fresh_clock
    if mode == 'clock':
        if not fresh_clock or not (0 < info.get('time_left', -1) <= info.get('timeout_match', 0)):
            reply('ERROR missing or invalid INFO time_left')
            return
        if info.get('timeout_turn') != 1000:
            reply('ERROR timeout_turn is not the real turn limit')
            return
        fresh_clock = False
    if mode == 'error':
        reply('ERROR fixture refusal details')
        return
    if mode == 'diagnostic-flood':
        for _ in range(200):
            reply('MESSAGE diagnostic')
        return
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
    reply(f'{at % 15},{at // 15}')


for raw in sys.stdin.buffer:
    if not raw.endswith(b'\r\n'):
        reply('ERROR manager did not send CRLF')
        continue
    line = raw.decode().strip()
    if line == 'START 15':
        reply('OK')
    elif line == 'BOARD':
        occupied.clear()
    elif line == 'DONE' or line == 'BEGIN':
        choose()
    elif line.startswith('TURN '):
        x, y = map(int, line[5:].split(','))
        occupied.add(y * 15 + x)
        choose()
    elif line.startswith('INFO '):
        _, key, value = line.split()
        info[key] = int(value)
        if key == 'time_left':
            fresh_clock = True
    elif line == 'END':
        break
    else:
        x, y, _ = map(int, line.split(','))
        occupied.add(y * 15 + x)
