"""No GPL package in the Python environments, and nothing imported that is
not accounted for.

Run inside an installed environment, once per scope:

    python tools/check_licenses.py --scope package     # exit 0 clean, 1 with findings

1. **Every installed package's license.** Every distribution in the
   environment, direct or pulled in by another, is read from its own
   metadata: the SPDX `License-Expression` when it has one (PEP 639),
   otherwise its `License` field and its trove classifiers. GPL, AGPL and
   SSPL are refused unless the expression offers an allowed alternative
   (`OR`); LGPL and MPL-2.0 are allowed, since Python loads them as
   separate libraries. A package whose license cannot be read is refused
   unless `REVIEWED` names it with the reason.
2. **Every import accounted for.** Each module a tracked Python file in
   the scope imports is the standard library, this repository's own code,
   or provided by an installed distribution, which check 1 has passed. An
   import nothing installed provides is refused, so a copyleft package
   cannot come in by being imported without being declared.

`SCOPES` says which directories each environment owns, and `check_tree.py`
fails when a tracked Python file is in no scope, so no file escapes check 2.
The expression parser is shared with `check_tree.py`'s npm check, which
allows a narrower set because the shell bundles its packages.
"""

from __future__ import annotations

import argparse
import ast
import pathlib
import re
import subprocess
import sys
from collections.abc import Iterable, Mapping, Sequence
from importlib import metadata

ROOT = pathlib.Path(__file__).resolve().parent.parent

#: the directories whose Python files each environment imports from
SCOPES = {
    'package': ('src', 'tests', 'tools', 'schema', 'testdata'),
}

#: license families refused outright
BLOCKED = ('GPL', 'AGPL', 'SSPL')

#: licenses a Python dependency may carry
ALLOWED_PYTHON = frozenset({
    'MIT', 'MIT-CMU', 'BSD', 'BSD-2-Clause', 'BSD-3-Clause', '0BSD', 'ISC',
    'Apache-2.0', 'PSF-2.0', 'Python-2.0', 'Zlib', 'CC0-1.0', 'Unlicense',
    'BlueOak-1.0.0', 'HPND', 'LGPL-2.0', 'LGPL-2.1', 'LGPL-3.0', 'LGPL',
    'MPL-2.0',
})

#: packages whose metadata states no license this checker can read, each
#: with what was checked by hand: {normalized name: reason}
REVIEWED: dict[str, str] = {}

#: trove classifiers as SPDX identifiers (an unlisted classifier is unknown)
CLASSIFIERS = {
    'MIT License': 'MIT',
    'MIT No Attribution License (MIT-0)': 'MIT-0',
    'BSD License': 'BSD',
    'Apache Software License': 'Apache-2.0',
    'ISC License (ISCL)': 'ISC',
    'Python Software Foundation License': 'PSF-2.0',
    'Mozilla Public License 2.0 (MPL 2.0)': 'MPL-2.0',
    'The Unlicense (Unlicense)': 'Unlicense',
    'zlib/libpng License': 'Zlib',
    'Historical Permission Notice and Disclaimer (HPND)': 'HPND',
    'GNU Lesser General Public License v2 (LGPLv2)': 'LGPL-2.0',
    'GNU Lesser General Public License v2 or later (LGPLv2+)': 'LGPL-2.0',
    'GNU Lesser General Public License v3 (LGPLv3)': 'LGPL-3.0',
    'GNU Lesser General Public License v3 or later (LGPLv3+)': 'LGPL-3.0',
    'GNU Library or Lesser General Public License (LGPL)': 'LGPL',
    'GNU General Public License (GPL)': 'GPL',
    'GNU General Public License v2 (GPLv2)': 'GPL-2.0',
    'GNU General Public License v2 or later (GPLv2+)': 'GPL-2.0',
    'GNU General Public License v3 (GPLv3)': 'GPL-3.0',
    'GNU General Public License v3 or later (GPLv3+)': 'GPL-3.0',
    'GNU Affero General Public License v3': 'AGPL-3.0',
    'GNU Affero General Public License v3 or later (AGPLv3+)': 'AGPL-3.0',
    'Server Side Public License (SSPL)': 'SSPL',
}

ALLOWED, UNKNOWN, REFUSED = 'allowed', 'unknown', 'refused'
_RANK = {REFUSED: 0, UNKNOWN: 1, ALLOWED: 2}


# ---- SPDX expressions ---------------------------------------------------------

_TOKEN = re.compile(r'\s*(\(|\)|[^\s()]+)')


class ExpressionError(ValueError):
    """Text that is not an SPDX license expression."""


def parse(text: str) -> object:
    """An SPDX expression as a tree: a license id (str), or ('AND' | 'OR',
    [children]). AND binds tighter than OR; `X WITH exception` reads as X,
    since an exception never changes which family a license is in.
    Operators are upper case, as SPDX writes them: `MIT or GPL-3.0` is not
    an expression."""
    tokens = _TOKEN.findall(text)
    if not tokens or ''.join(tokens) != re.sub(r'\s+', '', text):
        raise ExpressionError(f'not an SPDX expression: {text!r}')
    position = 0

    def peek() -> str | None:
        return tokens[position] if position < len(tokens) else None

    def take() -> str:
        nonlocal position
        if position >= len(tokens):
            raise ExpressionError(f'expression ends early: {text!r}')
        position += 1
        return tokens[position - 1]

    def either() -> object:
        parts = [both()]
        while peek() == 'OR':
            take()
            parts.append(both())
        return parts[0] if len(parts) == 1 else ('OR', parts)

    def both() -> object:
        parts = [single()]
        while peek() == 'AND':
            take()
            parts.append(single())
        return parts[0] if len(parts) == 1 else ('AND', parts)

    def single() -> object:
        token = take()
        if token == '(':
            inner = either()
            if take() != ')':
                raise ExpressionError(f'unbalanced parentheses: {text!r}')
            out = inner
        elif token == ')' or token in ('AND', 'OR', 'WITH'):
            raise ExpressionError(f'unexpected {token!r} in {text!r}')
        else:
            out = token
        if peek() == 'WITH':
            take()
            exception = take()
            if exception in ('(', ')'):
                raise ExpressionError(f'WITH needs an exception: {text!r}')
        return out

    tree = either()
    if position != len(tokens):
        raise ExpressionError(f'unexpected {tokens[position]!r} in {text!r}')
    return tree


def _base(license_id: str) -> str:
    """A license id without its version qualifier: 'GPL-3.0-or-later',
    'GPL-3.0-only' and 'GPL-3.0+' are all 'GPL-3.0'."""
    return re.sub(r'(-only|-or-later|\+)$', '', license_id)


#: an id in a blocked family: the family, then a version (`GPL-3.0`, and the
#: `GPLv3` a free-text License field writes), a `+`, or nothing
_BLOCKED_ID = re.compile(rf'(?:{"|".join(BLOCKED)})(?:-|\+|v?\d|$)', re.IGNORECASE)


def judge_id(license_id: str, allowed: frozenset[str]) -> str:
    """One license id: refused when its family is in `BLOCKED`, allowed when
    it is on `allowed`, unknown otherwise."""
    base = _base(license_id)
    if _BLOCKED_ID.match(base):
        return REFUSED
    return ALLOWED if base in allowed else UNKNOWN


def judge(tree: object, allowed: frozenset[str]) -> str:
    """A parsed expression: OR takes its best choice, AND its worst part."""
    if isinstance(tree, str):
        return judge_id(tree, allowed)
    operator, parts = tree
    verdicts = [judge(part, allowed) for part in parts]
    pick = max if operator == 'OR' else min
    return pick(verdicts, key=_RANK.__getitem__)


def judge_text(text: str | None, allowed: frozenset[str]) -> str:
    """An expression as text; text that is not one is unknown."""
    if not text or not text.strip():
        return UNKNOWN
    try:
        return judge(parse(text), allowed)
    except ExpressionError:
        return UNKNOWN


# ---- check 1: installed packages ----------------------------------------------

def normalized(name: str) -> str:
    """A distribution name as PEP 503 compares them."""
    return re.sub(r'[-_.]+', '-', name).lower()


def license_of(dist: metadata.Distribution) -> tuple[str, str]:
    """(verdict, what it rests on) for one installed distribution.

    The SPDX expression decides when there is one. Otherwise the `License`
    field (when it reads as an expression) and the classifiers are each
    judged, and the worst of those that say anything decides, so a stale
    permissive field cannot hide a GPL classifier."""
    meta = dist.metadata
    expression = meta.get('License-Expression')
    if expression:
        return judge_text(expression, ALLOWED_PYTHON), f'License-Expression: {expression}'
    said = []
    field = (meta.get('License') or '').strip()
    if field and '\n' not in field:
        verdict = judge_text(field, ALLOWED_PYTHON)
        if verdict != UNKNOWN:
            said.append((verdict, f'License: {field}'))
    classifiers = [c.split('::')[-1].strip() for c in meta.get_all('Classifier') or []
                   if c.startswith('License ::')]
    ids = [CLASSIFIERS[c] for c in classifiers if c in CLASSIFIERS]
    if ids:
        # a package lists each license it is offered under
        verdict = judge(('OR', ids), ALLOWED_PYTHON)
        said.append((verdict, 'classifiers: ' + ', '.join(classifiers)))
    if not said:
        shown = field.splitlines()[0][:60] if field else 'none stated'
        return UNKNOWN, f'no readable license (License field: {shown})'
    return min(said, key=lambda s: _RANK[s[0]])


def _required_by(dists: Sequence[metadata.Distribution]) -> dict[str, list[str]]:
    """{normalized name: names of the installed distributions that require it}."""
    out: dict[str, list[str]] = {}
    for dist in dists:
        for requirement in dist.requires or ():
            name = re.match(r'\s*([A-Za-z0-9][A-Za-z0-9._-]*)', requirement)
            if name:
                out.setdefault(normalized(name.group(1)), []).append(dist.metadata['Name'])
    return out


def check_installed(dists: Iterable[metadata.Distribution] | None = None,
                    reviewed: Mapping[str, str] = REVIEWED) -> list[str]:
    """Every installed distribution whose license is refused or unreadable."""
    dists = list(metadata.distributions() if dists is None else dists)
    required_by = _required_by(dists)
    found = []
    for dist in sorted(dists, key=lambda d: normalized(d.metadata['Name'])):
        name = dist.metadata['Name']
        verdict, basis = license_of(dist)
        if verdict == ALLOWED or (verdict == UNKNOWN and normalized(name) in reviewed):
            continue
        parents = sorted(set(required_by.get(normalized(name), [])))
        via = f' (required by {", ".join(parents)})' if parents else ''
        what = ('is copyleft' if verdict == REFUSED
                else 'has a license this check cannot read or does not allow')
        found.append(f'{name} {dist.version}{via} {what}: {basis}')
    return found


# ---- check 2: imports -----------------------------------------------------------

def tracked_python(root: pathlib.Path = ROOT) -> list[pathlib.Path]:
    out = subprocess.run(['git', 'ls-files', '-z', '*.py'], cwd=root, check=True,
                         capture_output=True).stdout
    return [root / name for name in out.decode('utf-8').split('\0') if name]


def in_scope(path: pathlib.Path, scope: str, root: pathlib.Path = ROOT) -> bool:
    return path.relative_to(root).parts[0] in SCOPES[scope]


def first_party(files: Iterable[pathlib.Path],
                root: pathlib.Path = ROOT) -> dict[pathlib.Path, frozenset[str]]:
    """For each file, the top-level names of this repository's own code it
    can import: every top-level package (the outermost directory holding an
    `__init__.py`), every directory directly under the root that holds
    Python (a namespace package, as `testdata.generate_plate` is imported
    with the root on the path), and each module outside a package that
    sits in the file's directory or one above it, as a test reaches its
    conftest or a tool beside its tests. A module inside a package is not
    a top-level name, so a reader at `io/rattlesnake.py` does not make
    `import rattlesnake` this repository's."""
    files = list(files)
    package_dirs = {path.parent for path in files if path.name == '__init__.py'}
    packages = {d.name for d in package_dirs if d.parent not in package_dirs}
    packages |= {path.relative_to(root).parts[0] for path in files
                 if len(path.relative_to(root).parts) > 1}
    loose: dict[pathlib.Path, set[str]] = {}
    for path in files:
        if path.parent not in package_dirs:
            loose.setdefault(path.parent, set()).add(path.stem)
    return {path: frozenset(packages.union(*(loose.get(d, ()) for d in
                                             (path.parent, *path.parent.parents))))
            for path in files}


def imports(source: str) -> list[tuple[int, str]]:
    """(line, top-level module) for every absolute import in a file,
    `importlib.import_module` and `__import__` of a literal name included."""
    out = []
    for node in ast.walk(ast.parse(source)):
        if isinstance(node, ast.Import):
            out += [(node.lineno, alias.name) for alias in node.names]
        elif isinstance(node, ast.ImportFrom) and node.module and not node.level:
            out.append((node.lineno, node.module))
        elif (isinstance(node, ast.Call) and node.args
              and isinstance(node.args[0], ast.Constant)
              and isinstance(node.args[0].value, str)
              and getattr(node.func, 'id', getattr(node.func, 'attr', None))
              in {'__import__', 'import_module'}):
            out.append((node.lineno, node.args[0].value))
    return [(line, module.split('.')[0]) for line, module in out]


def check_imports(files: Iterable[pathlib.Path], own: Mapping[pathlib.Path, frozenset[str]],
                  provided: Mapping[str, Sequence[str]] | None = None,
                  root: pathlib.Path = ROOT) -> list[str]:
    """Every import that is not the standard library, not one of the file's
    own names (`first_party`), and not provided by an installed
    distribution."""
    provided = metadata.packages_distributions() if provided is None else provided
    found = []
    for path in files:
        for line, module in imports(path.read_text(encoding='utf-8')):
            if (module in sys.stdlib_module_names or module in own[path]
                    or module in provided):
                continue
            found.append(f'{path.relative_to(root)}:{line}: imports {module}, which no '
                         'installed package provides; declare the package that does, '
                         'so its license is checked')
    return found


def check_scopes(files: Iterable[pathlib.Path], root: pathlib.Path = ROOT) -> list[str]:
    """Every tracked Python file that no scope covers."""
    return [f'{path.relative_to(root)}: in no scope of check_licenses.SCOPES, so '
            'its imports are never checked'
            for path in files if not any(in_scope(path, s, root) for s in SCOPES)]


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('--scope', required=True, choices=sorted(SCOPES))
    scope = parser.parse_args(argv).scope
    files = tracked_python()
    mine = [path for path in files if in_scope(path, scope)]
    findings = check_installed() + check_imports(mine, first_party(files, ROOT))
    for finding in findings:
        print(finding)
    count = sum(1 for _ in metadata.distributions())
    print(f'check_licenses: {scope}: {count} installed packages, {len(mine)} files, '
          f'{len(findings)} findings')
    return 1 if findings else 0


if __name__ == '__main__':
    sys.exit(main())
