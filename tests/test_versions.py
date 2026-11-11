"""Every file that states Hertzforge's version states the same one.

A release bumps them all together (`pyproject.toml`, the Python package,
the Cargo workspace and both lock files, the desktop app's manifests, the
shell's). The desktop crate's own lock file was missed at 0.1.2 and 0.1.3,
and the desktop build's `--locked` lint refused it after the release had
published (2026-11-11).
"""

from __future__ import annotations

import json
import re
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _version() -> str:
    return tomllib.loads((ROOT / 'pyproject.toml').read_text(encoding='utf-8'))['project'][
        'version']


def _lock_versions(path: Path) -> dict[str, str]:
    """{name: version} of the lock file's own crates (those with no source)."""
    lock = tomllib.loads(path.read_text(encoding='utf-8'))
    return {p['name']: p['version'] for p in lock['package'] if 'source' not in p}


def test_every_manifest_states_one_version():
    version = _version()
    package = (ROOT / 'src' / 'hertzforge' / '__init__.py').read_text(encoding='utf-8')
    assert re.search(r'^__version__ = "([^"]+)"', package, re.M)[1] == version
    workspace = tomllib.loads((ROOT / 'Cargo.toml').read_text(encoding='utf-8'))
    assert workspace['workspace']['package']['version'] == version
    desktop = tomllib.loads((ROOT / 'desktop' / 'Cargo.toml').read_text(encoding='utf-8'))
    assert desktop['package']['version'] == version
    for name in ('desktop/tauri.conf.json', 'desktop/package.json', 'ui/package.json'):
        assert json.loads((ROOT / name).read_text(encoding='utf-8'))['version'] == version, name
    for name in ('desktop/package-lock.json', 'ui/package-lock.json'):
        lock = json.loads((ROOT / name).read_text(encoding='utf-8'))
        assert lock['version'] == lock['packages']['']['version'] == version, name


def test_both_cargo_locks_hold_the_version():
    version = _version()
    workspace = _lock_versions(ROOT / 'Cargo.lock')
    stale = {name: v for name, v in workspace.items() if v != version}
    assert workspace and not stale, f'Cargo.lock: {stale}'
    assert _lock_versions(ROOT / 'desktop' / 'Cargo.lock')['hertzforge-desktop'] == version
