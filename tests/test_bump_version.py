"""`scripts/bump_version.py` moves every file that states the version, and
nothing else: the eleven files `test_versions` holds to one version, on a
copy of the repository's own."""

from __future__ import annotations

import importlib.util
import json
import shutil
import tomllib
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


def _bump_module():
    spec = importlib.util.spec_from_file_location('bump_version', ROOT / 'scripts' / 'bump_version.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def copy(tmp_path, monkeypatch):
    bump = _bump_module()
    names = (*bump.TOML, *bump.JSON, *bump.NPM_LOCKS, *bump.CARGO_LOCKS,
             'src/hertzforge/__init__.py')
    for name in names:
        (tmp_path / name).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(ROOT / name, tmp_path / name)
    monkeypatch.setattr(bump, 'ROOT', tmp_path)
    return bump, tmp_path


def _next(version: str) -> str:
    major, minor, patch = map(int, version.split('.'))
    return f'{major}.{minor}.{patch + 1}'


def test_every_file_moves_to_the_new_version(copy):
    bump, root = copy
    old = bump.current()
    new = _next(old)
    changed = bump.bump(new)
    assert len(changed) == 11
    assert bump.current() == new
    for name in bump.JSON:
        assert json.loads((root / name).read_text())['version'] == new, name
    for name in bump.NPM_LOCKS:
        lock = json.loads((root / name).read_text())
        assert lock['version'] == lock['packages']['']['version'] == new, name
    for name in bump.CARGO_LOCKS:
        lock = tomllib.loads((root / name).read_text())
        own = {p['name']: p['version'] for p in lock['package'] if 'source' not in p}
        assert own and set(own.values()) == {new}, name
        # a crate from the registry that happened to share the old number keeps it
        before = tomllib.loads((ROOT / name).read_text())
        theirs = [(p['name'], p['version']) for p in before['package'] if 'source' in p]
        after = [(p['name'], p['version']) for p in lock['package'] if 'source' in p]
        assert theirs == after, name


def test_a_version_not_after_the_current_one_is_refused(copy):
    bump, _root = copy
    with pytest.raises(SystemExit, match='not after'):
        bump.bump(bump.current())
    with pytest.raises(SystemExit, match='not a version'):
        bump.bump('next')
