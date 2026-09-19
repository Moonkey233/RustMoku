"""One explicit executable policy for Python integration tests.

RUSTMOKU_DATA_EXE / RUSTMOKU_ARENA_EXE override the default release
executable. Never fall back to a possibly stale developer debug binary.
"""
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def executable(name):
    if name not in ('rustmoku-data', 'rustmoku-arena'):
        raise ValueError('unsupported integration executable')
    variable = name.upper().replace('-', '_') + '_EXE'
    override = os.environ.get(variable)
    path = Path(override).expanduser() if override else ROOT / 'target/release' / (name + ('.exe' if os.name == 'nt' else ''))
    path = path.resolve()
    if not path.is_file():
        raise RuntimeError(f'Missing integration executable: {path}. Set {variable} or run cargo build --release -p rustmoku-data -p rustmoku-arena.')
    return path
