"""Bounded checkpoint loading and atomic publication of offline training state."""

import os
import tempfile
import zipfile
from pathlib import Path

import torch


MAX_CHECKPOINT_BYTES = 128 * 1024 * 1024


def load_checkpoint(path, device='cpu'):
    path = Path(path)
    if path.stat().st_size > MAX_CHECKPOINT_BYTES:
        raise ValueError('training checkpoint exceeds 128 MiB limit')
    if not zipfile.is_zipfile(path):
        raise ValueError('unsupported or truncated checkpoint container')
    with zipfile.ZipFile(path) as archive:
        entries = archive.infolist()
        if len(entries) > 2048 or sum(entry.file_size for entry in entries) > MAX_CHECKPOINT_BYTES:
            raise ValueError('checkpoint expanded storage exceeds safety limit')
    checkpoint = torch.load(path, map_location=device, weights_only=True)
    if not isinstance(checkpoint, dict) or checkpoint.get('format') not in ('rustmoku-local-pattern-v1', 'rustmoku-nonlinear-v2'):
        raise ValueError('unsupported training checkpoint')
    return checkpoint


def atomic_save(path, checkpoint):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, suffix='.partial', delete=False) as output:
            temporary = Path(output.name)
            torch.save(checkpoint, output)
            if output.tell() > MAX_CHECKPOINT_BYTES:
                raise ValueError('training checkpoint exceeds 128 MiB limit')
            output.flush()
            os.fsync(output.fileno())
        temporary.replace(path)
    finally:
        if temporary is not None and temporary.exists():
            temporary.unlink()
