"""Set Hertzforge's version in every file that states it.

    python3 scripts/bump_version.py 0.1.17

The eleven files `tests/test_versions.py` holds to one version: the Python
package and `pyproject.toml`, the Cargo workspace and the desktop crate, both
Cargo lock files (the workspace's own crates, those with no `source`), the
desktop app's manifests and the shell's, with their npm lock files. Third-party
crates that share the old number are left alone. Prints the files changed.
"""

from __future__ import annotations

import json
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TOML = ('pyproject.toml', 'Cargo.toml', 'desktop/Cargo.toml')
JSON = ('desktop/tauri.conf.json', 'desktop/package.json', 'ui/package.json')
NPM_LOCKS = ('desktop/package-lock.json', 'ui/package-lock.json')
CARGO_LOCKS = ('Cargo.lock', 'desktop/Cargo.lock')


def current() -> str:
    return tomllib.loads((ROOT / 'pyproject.toml').read_text(encoding='utf-8'))['project'][
        'version']


def _once(text: str, pattern: str, new: str, name: str, count: int = 1) -> str:
    out, made = re.subn(pattern, new, text, count=count, flags=re.MULTILINE)
    if made != count:
        raise SystemExit(f'{name}: found {made} of the {count} version lines expected')
    return out


def _own_crates(text: str, old: str, new: str) -> str:
    """A Cargo lock with every crate that has no `source` (this repository's own)
    moved from `old` to `new`."""
    blocks = text.split('\n[[package]]\n')
    for k, block in enumerate(blocks[1:], start=1):
        if 'source = ' not in block:
            blocks[k] = block.replace(f'\nversion = "{old}"\n', f'\nversion = "{new}"\n', 1)
    return '\n[[package]]\n'.join(blocks)


def bump(new: str) -> list[str]:
    old = current()
    if not re.fullmatch(r'\d+\.\d+\.\d+', new):
        raise SystemExit(f'{new!r} is not a version like 0.1.17')
    if tuple(map(int, new.split('.'))) <= tuple(map(int, old.split('.'))):
        raise SystemExit(f'{new} is not after {old}')
    quoted = re.escape(f'"{old}"')
    edits: dict[str, str] = {}
    for name in TOML:
        text = (ROOT / name).read_text(encoding='utf-8')
        edits[name] = _once(text, rf'^version = {quoted}', f'version = "{new}"', name)
    init = 'src/hertzforge/__init__.py'
    edits[init] = _once((ROOT / init).read_text(encoding='utf-8'),
                        rf'^__version__ = {quoted}', f'__version__ = "{new}"', init)
    for name in JSON:
        edits[name] = _once((ROOT / name).read_text(encoding='utf-8'),
                            rf'"version": {quoted}', f'"version": "{new}"', name)
    for name in NPM_LOCKS:
        # the lock's own version, then its root package's
        edits[name] = _once((ROOT / name).read_text(encoding='utf-8'),
                            rf'"version": {quoted}', f'"version": "{new}"', name, count=2)
    for name in CARGO_LOCKS:
        edits[name] = _own_crates((ROOT / name).read_text(encoding='utf-8'), old, new)
    for name in NPM_LOCKS:
        lock = json.loads(edits[name])
        if not lock['version'] == lock['packages']['']['version'] == new:
            raise SystemExit(f'{name}: the first two versions were not its own')
    for name, text in edits.items():
        (ROOT / name).write_text(text, encoding='utf-8')
    return list(edits)


if __name__ == '__main__':
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    for changed in bump(sys.argv[1]):
        print(changed)
