"""Persistent finite exploration budget; crashed children retain their reservation."""
import contextlib
import json
import math
import os
import subprocess
import tempfile
import signal
import time
from pathlib import Path


def atomic_json(path, value):
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', dir=path.parent, delete=False) as stream:
            temporary = Path(stream.name)
            json.dump(value, stream, sort_keys=True)
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(path)
    finally:
        if temporary is not None and temporary.exists():
            temporary.unlink()


class ExplorationBudget:
    def __init__(self, path, *, seconds=1200, artifact_bytes=2 * 1024**3):
        if not math.isfinite(seconds) or seconds <= 0 or artifact_bytes <= 0:
            raise ValueError('invalid explicit exploration budget')
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.limits = {'seconds': seconds, 'artifact_bytes': artifact_bytes}

    @contextlib.contextmanager
    def locked(self):
        with self.path.with_suffix('.lock').open('a+b') as lock:
            lock.seek(0)
            if os.name == 'nt':
                import msvcrt
                if lock.read(1) == b'':
                    lock.write(b'0')
                    lock.flush()
                lock.seek(0)
                msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            try:
                yield
            finally:
                lock.seek(0)
                if os.name == 'nt':
                    msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)
                else:
                    fcntl.flock(lock, fcntl.LOCK_UN)

    def run(self, command, *, timeout, artifact_root, **kwargs):
        with self.locked():
            state = json.loads(self.path.read_text()) if self.path.exists() else {
                'version': 1, 'limits': self.limits, 'charged_seconds': 0, 'runs': []}
            if state['version'] != 1 or state['limits'] != self.limits:
                raise ValueError('persisted experiment budget mismatch')
            used = sum(p.stat().st_size for p in Path(artifact_root).rglob('*') if p.is_file())
            if used >= self.limits['artifact_bytes']:
                raise ValueError('experiment artifact limit exhausted')
            remaining = self.limits['seconds'] - state['charged_seconds']
            cap = min(timeout, remaining)
            if cap <= 0:
                raise ValueError('cumulative exploration wall-time exhausted')
            # Reserve before launching. If the coordinator crashes this full cap
            # remains charged; a resume cannot accidentally reset elapsed work.
            state['charged_seconds'] += cap
            receipt = {'command': list(map(str, command)), 'reserved_seconds': cap, 'status': 'reserved'}
            state['runs'].append(receipt)
            atomic_json(self.path, state)
            started = time.monotonic()
            try:
                def guard():
                    size = sum(p.stat().st_size for p in Path(artifact_root).rglob('*') if p.is_file())
                    if size >= self.limits['artifact_bytes']:
                        raise ValueError('experiment artifact limit reached')
                result = run_tree(command, timeout=cap, guard=guard, **kwargs)
                receipt['status'] = 'complete' if result.returncode == 0 else 'failed'
                return result
            finally:
                elapsed = time.monotonic() - started
                state['charged_seconds'] += elapsed - cap
                receipt['elapsed_seconds'] = elapsed
                if receipt['status'] == 'reserved':
                    receipt['status'] = 'interrupted-or-failed'
                atomic_json(self.path, state)


def run_tree(command, *, timeout, check=False, capture_output=False, guard=None, **kwargs):
    if capture_output:
        kwargs.update(stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if os.name != 'nt':
        kwargs['start_new_session'] = True
    with subprocess.Popen(command, **kwargs) as process:
        try:
            deadline = time.monotonic() + timeout
            while True:
                if guard is not None:
                    guard()
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise subprocess.TimeoutExpired(command, timeout)
                try:
                    stdout, stderr = process.communicate(timeout=min(1, remaining))
                    break
                except subprocess.TimeoutExpired:
                    continue
        except BaseException:
            if os.name == 'nt':
                subprocess.run(['taskkill.exe', '/PID', str(process.pid), '/T', '/F'],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10, check=False)
            else:
                os.killpg(process.pid, signal.SIGKILL)
            process.kill()
            process.communicate()
            raise
        result = subprocess.CompletedProcess(command, process.returncode, stdout, stderr)
        if check:
            result.check_returncode()
        return result
