"""Crash-safe, no-clobber publication for RustMoku's small JSON manifests.

The final name is linked atomically only after a same-directory file has been
flushed, fsynced and parsed. This requires a filesystem with atomic hard links
(NTFS and normal Linux filesystems support them); there is no overwrite fallback.
Unpublished .partial files are never inputs. A damaged published file requires
explicit operator recovery, not replacement based on an assumed identity.
"""
import json
import os
import tempfile
from pathlib import Path

MAX_MANIFEST_BYTES = 64 * 1024 * 1024


class CorruptManifestError(ValueError):
    pass


class ManifestMismatchError(ValueError):
    pass


def _fault_point(_name):
    """Test hook at publication boundaries; no production action."""


def _read(path):
    if path.is_symlink() or not path.is_file() or path.stat().st_size > MAX_MANIFEST_BYTES:
        raise CorruptManifestError(f'invalid published manifest: {path}')
    try:
        raw = path.read_bytes()
        value = json.loads(raw)
        if not isinstance(value, dict):
            raise ValueError('manifest must be an object')
    except (ValueError, UnicodeError) as error:
        raise CorruptManifestError(f'corrupt published manifest: {path}; explicit recovery required') from error
    return raw, value


def read_manifest(path):
    return _read(Path(path))[1]


def _sync_directory(directory):
    # Windows does not expose directory fsync via Python's POSIX descriptor API.
    # fsync above still flushes the file; post-crash corruption fails closed.
    if os.name != 'nt':
        descriptor = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)


def save_manifest(path, value):
    path = Path(path)
    payload = (json.dumps(value, sort_keys=True, indent=2, allow_nan=False) + '\n').encode('utf-8')
    if not isinstance(value, dict) or len(payload) > MAX_MANIFEST_BYTES:
        raise ValueError('manifest must be an object of at most 64 MiB')

    def verify_existing():
        raw, _ = _read(path)
        if raw != payload:
            raise ManifestMismatchError(f'immutable manifest identity mismatch: {path}')

    if path.exists() or path.is_symlink():
        verify_existing()
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, prefix=f'.{path.name}.',
                                         suffix='.partial', delete=False) as stream:
            temporary = Path(stream.name)
            middle = len(payload) // 2
            stream.write(payload[:middle])
            _fault_point('after_partial_write')
            stream.write(payload[middle:])
            stream.flush()
            os.fsync(stream.fileno())
        if _read(temporary)[0] != payload:
            raise CorruptManifestError('temporary manifest validation failed')
        _fault_point('before_publish')
        try:
            os.link(temporary, path)
        except FileExistsError:
            verify_existing()
        _fault_point('after_publish')
        _sync_directory(path.parent)
    finally:
        if temporary is not None and temporary.exists():
            temporary.unlink()
