"""The public tree's own checks, run on every pull request.

1. **Unaffiliated.** No employer, client, laboratory or partner is named in
   any tracked file. The names are held as truncated SHA-256 hashes of the
   normalized phrase (lower case, punctuation as spaces), so the list that
   keeps them out does not publish them. Every one-, two- and three-word
   run of every line is hashed and looked up.
2. **No copyleft code.** No tracked Python file imports a copyleft package,
   by statement or by name.
3. **Permissive npm licenses.** Every package in the shell's lockfile
   carries a license on the allowlist (an `OR` expression needs one
   allowed choice, an `AND` expression needs all of them).

    python tools/check_tree.py                # exit 0 clean, 1 with findings
    python tools/check_tree.py --hash "a name"  # the line to add to NAMES

Rust crate licenses are checked by cargo-deny once the controller crates
are in the tree; until then the control service does not build here.
"""

from __future__ import annotations

import ast
import hashlib
import json
import pathlib
import re
import subprocess
import sys
from collections.abc import Iterable, Sequence

ROOT = pathlib.Path(__file__).resolve().parent.parent

#: Hashes of the names the tree must never contain (see the docstring).
#: Add one with `python tools/check_tree.py --hash "the name"`.
NAMES = frozenset({
    '4a654a97330bd592', 'd6c6d1247e12746b', 'e07681543d5c76d6', 'e2a3af9c1d2aa535',
    'bd3b238df5c6bcde', '2d52485274940e77', 'bef51514796755a0', 'd9a88fab7e0b2a48',
    'd603337bb0e81eb6', 'b9dc4892374c9d9f', '846be5fc541c5039', '49e172a78edef3db',
    '8b53b6c84bab7387', '31175d1308492e88', '9c9af8df9f2079ba', '63f87d6c2ab47659',
    '70cabe495f52902c', '78de28dddab5e164', '80cdad1dcf688de4', '1d7cc59f0ff10ab7',
    'ced8f61cd041bb50', '5e4f06885c0ac383', '436f604f3bb414c1', 'e78bdebae031095b',
    '05f6ecf01ac4ebec', 'cb5c36fe59056a6a', '799dda30b3121bbf', 'b426df21b43a09fa',
    '16f45b99abfda636', 'a4074992974e30ed', '4db54b27d524b4a1', '1916eb4a702ef0dd',
    '5bd79a8938028958', '3e4d26effdc5f34d', 'fce9f2383131cd07',
})

#: Copyleft packages this project is closest to, never imported here
#: (the same set Visual Dynamics guards).
COPYLEFT = frozenset({'sdynpy', 'rattlesnake', 'forcefinder'})

#: Licenses a shipped or build-time npm package may carry.
ALLOWED_LICENSES = frozenset({
    'MIT', 'ISC', 'BSD-2-Clause', 'BSD-3-Clause', 'Apache-2.0', '0BSD', 'Zlib',
    'OFL-1.1', 'CC0-1.0', 'Unlicense', 'BlueOak-1.0.0', 'Python-2.0', 'CC-BY-4.0',
})

#: Lockfiles name third-party packages, not this project's words.
SKIPPED = frozenset({'package-lock.json', 'uv.lock', 'Cargo.lock'})

_WORD = re.compile(r'[a-z0-9]+')


def phrase_hash(text: str) -> str:
    """The hash a name is stored and looked up under."""
    normalized = ' '.join(_WORD.findall(text.lower()))
    return hashlib.sha256(normalized.encode('utf-8')).hexdigest()[:16]


def tracked_files(root: pathlib.Path = ROOT) -> list[pathlib.Path]:
    """Every file git tracks, as paths under `root`."""
    out = subprocess.run(['git', 'ls-files', '-z'], cwd=root, check=True,
                         capture_output=True).stdout
    return [root / name for name in out.decode('utf-8').split('\0') if name]


def _text(path: pathlib.Path) -> str | None:
    """The file as text, or None for a binary file."""
    data = path.read_bytes()
    if b'\0' in data[:8192]:
        return None
    return data.decode('utf-8', errors='replace')


def check_names(files: Iterable[pathlib.Path], root: pathlib.Path = ROOT,
                names: frozenset[str] = NAMES) -> list[str]:
    """Every line where a run of one to three words hashes to a listed name.
    The finding gives the place, never the word."""
    found = []
    for path in files:
        if path.name in SKIPPED:
            continue
        text = _text(path)
        if text is None:
            continue
        # the path itself is part of the tree too
        lines = [str(path.relative_to(root)), *text.splitlines()]
        for number, line in enumerate(lines):
            words = _WORD.findall(line.lower())
            runs = (' '.join(words[i:i + size]) for size in (1, 2, 3)
                    for i in range(len(words) - size + 1))
            if any(phrase_hash(run) in names for run in runs):
                where = 'its path' if number == 0 else f'line {number}'
                found.append(f'{path.relative_to(root)}: {where} names a listed '
                             'organization')
    return found


def _copyleft_imports(source: str) -> list[str]:
    tree = ast.parse(source)
    hits = []
    for node in ast.walk(tree):
        modules: list[str] = []
        if isinstance(node, ast.Import):
            modules = [alias.name for alias in node.names]
        elif isinstance(node, ast.ImportFrom) and node.module and not node.level:
            modules = [node.module]
        elif (isinstance(node, ast.Call) and node.args
              and isinstance(node.args[0], ast.Constant)
              and isinstance(node.args[0].value, str)
              and getattr(node.func, 'id', getattr(node.func, 'attr', None))
              in {'__import__', 'import_module'}):
            modules = [node.args[0].value]
        hits += [m for m in modules if m.split('.')[0] in COPYLEFT]
    return hits


def check_copyleft(files: Iterable[pathlib.Path], root: pathlib.Path = ROOT) -> list[str]:
    """Every tracked Python file that reaches a copyleft package."""
    found = []
    for path in files:
        if path.suffix != '.py':
            continue
        for module in _copyleft_imports(path.read_text(encoding='utf-8')):
            found.append(f'{path.relative_to(root)}: imports {module}, which is '
                         'copyleft')
    return found


def license_allowed(expression: str | None) -> bool:
    """Whether an SPDX expression is on the allowlist."""
    if not expression:
        return False
    expression = expression.strip().strip('()')
    if ' OR ' in expression:
        return any(license_allowed(part) for part in expression.split(' OR '))
    if ' AND ' in expression:
        return all(license_allowed(part) for part in expression.split(' AND '))
    return expression.strip('() ') in ALLOWED_LICENSES


def check_npm_licenses(lockfile: pathlib.Path) -> list[str]:
    """Every package in an npm lockfile whose license is missing or not allowed."""
    packages = json.loads(lockfile.read_text(encoding='utf-8')).get('packages', {})
    found = []
    for key, package in packages.items():
        if not key:
            continue
        name = key.split('node_modules/')[-1]
        license_ = package.get('license')
        if not license_allowed(license_):
            found.append(f'{lockfile.name}: {name} is licensed '
                         f'{license_ or "with no license stated"}, which is not '
                         'on the allowlist')
    return found


def main(argv: Sequence[str] = ()) -> int:
    if len(argv) == 2 and argv[0] == '--hash':
        print(f"    '{phrase_hash(argv[1])}',")
        return 0
    files = tracked_files()
    findings = (check_names(files) + check_copyleft(files)
                + check_npm_licenses(ROOT / 'ui' / 'package-lock.json'))
    for finding in findings:
        print(finding)
    print(f'check_tree: {len(files)} tracked files, {len(findings)} findings')
    return 1 if findings else 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
