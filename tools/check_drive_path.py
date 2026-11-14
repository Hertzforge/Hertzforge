"""The drive path's checks (docs/drive-path.md), run on every CI run.

The drive path is the code that can change what reaches an output channel,
or whether and how a run stops. Two lists say what it is and what keeps it
safe, and this script holds the tree and each pull request to them.

On every run, the lists:

1. **Every path is classified.** Every path in `tools/drive_path.txt`
   exists, none is on both lists, and every crate under `crates/`, every
   folder under `modules/`, `laws/`, `wit/` and `abi/`, and every entry
   directly under `schema/` is on exactly one of the two lists.
2. **Every safety test is whole.** Every test in `tools/safety_tests.txt`
   exists in its file, is a test, is not ignored (`#[ignore]`, or a
   `cfg_attr` that can add one), and is compiled into the test build: no
   `cfg` on it, its module, its file or the `mod` lines that reach the file
   leaves it out on every CI platform with the features the workspace
   enables, and no `required-features`, `test = false` or `autotests =
   false` leaves its target out.

On a pull request, against the base it would merge into:

3. **The drive-path section.** When the diff touches a drive-path path
   (on the base's list or the head's), the description carries the
   template's drive-path section with its four answers filled in.
4. **Weakening says so.** A line removed from `tools/safety_tests.txt`, or a
   path moved off the drive-path list, fails unless the title carries
   `[weakens safety]`. A test that moves to another file with its name and
   crate unchanged is not a removal. Edits inside a listed test's body are
   reported as warnings for the reviewer, never failures.

    python tools/check_drive_path.py                          # the lists (1, 2)
    python tools/check_drive_path.py --event "$GITHUB_EVENT_PATH"
    python tools/check_drive_path.py --base origin/main --title "..." --body-file pr.md

Exit status: 0 clean, 1 with findings, 2 when the check could not run (a
usage error, or git could not answer).
"""

from __future__ import annotations

import argparse
import bisect
import dataclasses
import json
import os
import pathlib
import re
import subprocess
import sys
import tomllib
from collections.abc import Iterable, Mapping, Sequence

ROOT = pathlib.Path(__file__).resolve().parent.parent
DRIVE_PATH = 'tools/drive_path.txt'
SAFETY_TESTS = 'tools/safety_tests.txt'

DRIVE = 'drive path'
NOT_DRIVE = 'not drive path'

#: The folders whose every direct child must be classified.
CLASSIFIED_PARENTS = ('crates', 'modules', 'laws', 'wit', 'abi', 'schema')

#: What the title of a pull request that weakens a protection carries.
WEAKENS = '[weakens safety]'

#: The kinds a safety test may be listed under (tools/safety_tests.txt).
KINDS = frozenset({
    'drive-ceiling', 'drive-abort', 'stop-ramp', 'abort-to-zero', 'run-thread-failure',
    'law-guard', 'envelope', 'kinematic', 'divergence', 'overload', 'underrun',
    'dropped-controller', 'late-tick', 'watchdog', 'arm-gate', 'checked-input',
    'authority', 'shutdown',
})

#: The pull request template's drive-path section: its heading, the line a
#: pull request that touches no drive-path code keeps, and the four answers
#: one that does gives.
SECTION = 'Drive path and hardware'
UNTOUCHED = 'No drive-path code touched.'
ANSWERS = (
    'What can now reach the drive',
    'How it fails safe',
    'What ran on the simulator',
    'What ran on hardware',
)
#: Answers that leave the question open.
PLACEHOLDERS = frozenset({'', 'todo', 'tbd', 'n/a', 'na', '-', '...', '?', 'tba'})

#: The platforms CI builds the workspace on, as the facts a `cfg` reads.
PLATFORMS = (
    {'target_os': 'linux', 'target_family': 'unix', 'target_arch': 'x86_64',
     'target_env': 'gnu', 'target_pointer_width': '64', 'target_endian': 'little',
     'target_vendor': 'unknown'},
    {'target_os': 'macos', 'target_family': 'unix', 'target_arch': 'aarch64',
     'target_env': '', 'target_pointer_width': '64', 'target_endian': 'little',
     'target_vendor': 'apple'},
    {'target_os': 'windows', 'target_family': 'windows', 'target_arch': 'x86_64',
     'target_env': 'msvc', 'target_pointer_width': '64', 'target_endian': 'little',
     'target_vendor': 'pc'},
)


# --------------------------------------------------------------------- lists

@dataclasses.dataclass(frozen=True)
class Listed:
    """One path on a drive-path list."""
    path: str
    section: str
    reason: str
    line: int


@dataclasses.dataclass(frozen=True)
class SafetyTest:
    """One line of tools/safety_tests.txt."""
    crate: str
    file: str
    test: str
    kind: str
    protects: str
    line: int

    @property
    def key(self) -> tuple[str, str, str]:
        return (self.crate, self.file, self.test)

    @property
    def where(self) -> str:
        return f'crates/{self.crate}/{self.file}::{self.test}'


def _lines(text: str) -> Iterable[tuple[int, str]]:
    """Each line's number and its text with any comment and the line ending
    taken off, skipping lines that hold nothing else."""
    for number, raw in enumerate(text.splitlines(), start=1):
        line = raw.split('#', 1)[0].strip()
        if line:
            yield number, line


def parse_drive_path(text: str, name: str = DRIVE_PATH) -> tuple[list[Listed], list[str]]:
    """The entries of a drive-path list, and a finding for each line that
    cannot be read."""
    entries, findings = [], []
    section = None
    for number, line in _lines(text):
        header = re.fullmatch(r'\[(.+)\]', line)
        if header:
            section = header.group(1).strip()
            if section not in (DRIVE, NOT_DRIVE):
                findings.append(f'{name}:{number}: unknown section [{section}]; the sections '
                                f'are [{DRIVE}] and [{NOT_DRIVE}]')
                section = None
            continue
        if section is None:
            findings.append(f'{name}:{number}: a path before any section; put it under '
                            f'[{DRIVE}] or [{NOT_DRIVE}]')
            continue
        parts = line.split(None, 1)
        path = parts[0]
        if len(parts) < 2:
            findings.append(f'{name}:{number}: {path} has no reason; add a short one after it')
            continue
        if path.endswith('/') or path.startswith(('/', './')) or '\\' in path:
            findings.append(f'{name}:{number}: write {path} from the repository root, with '
                            'forward slashes and no leading or trailing slash')
            continue
        entries.append(Listed(path, section, parts[1].strip(), number))
    return entries, findings


def parse_safety_tests(text: str, name: str = SAFETY_TESTS) -> tuple[list[SafetyTest], list[str]]:
    """The entries of a safety-test list, and a finding for each line that
    cannot be read."""
    entries, findings, seen = [], [], {}
    for number, line in _lines(text):
        parts = line.split(None, 4)
        if len(parts) < 5:
            findings.append(f'{name}:{number}: expected "crate file test kind what it '
                            f'protects", got: {line}')
            continue
        crate, file, test, kind, protects = parts
        if kind not in KINDS:
            findings.append(f'{name}:{number}: {kind} is not a kind; the kinds are '
                            f'{", ".join(sorted(KINDS))}')
            continue
        if not re.fullmatch(r'(?:[A-Za-z_]\w*::)*[A-Za-z_]\w*', test):
            findings.append(f'{name}:{number}: {test} is not a test name or module::name')
            continue
        entry = SafetyTest(crate, file, test, kind, protects.strip(), number)
        if entry.key in seen:
            findings.append(f'{name}:{number}: {entry.where} is already listed on line '
                            f'{seen[entry.key]}')
            continue
        seen[entry.key] = number
        entries.append(entry)
    return entries, findings


def covered(path: str, listed: Iterable[str]) -> bool:
    """Whether `path` is one of `listed` or under one of them."""
    return any(path == entry or path.startswith(entry + '/') for entry in listed)


def check_lists(entries: Sequence[Listed], root: pathlib.Path = ROOT) -> list[str]:
    """Rule 1: every listed path exists, none is listed twice, and every
    child of a classified parent is on exactly one list."""
    findings = []
    by_path: dict[str, list[Listed]] = {}
    for entry in entries:
        by_path.setdefault(entry.path, []).append(entry)
        if not (root / entry.path).exists():
            findings.append(f'{DRIVE_PATH}:{entry.line}: {entry.path} does not exist; remove '
                            'the line, or correct the path')
    for path, same in by_path.items():
        if len(same) > 1:
            lines = ', '.join(str(e.line) for e in same)
            findings.append(f'{DRIVE_PATH}: {path} is listed more than once (lines {lines}); '
                            'keep one line, on the list it belongs to')
    paths = list(by_path)
    for entry in entries:
        inside = [p for p in paths if p != entry.path and p.startswith(entry.path + '/')]
        if inside:
            findings.append(f'{DRIVE_PATH}:{entry.line}: {entry.path} contains '
                            f'{", ".join(inside)}, also listed; list each path once')
    for parent in CLASSIFIED_PARENTS:
        folder = root / parent
        if not folder.is_dir():
            continue
        for child in sorted(folder.iterdir()):
            if child.name.startswith('.'):
                continue
            rel = f'{parent}/{child.name}'
            if not covered(rel, paths) and not any(p.startswith(rel + '/') for p in paths):
                findings.append(f'{rel} is on neither list in {DRIVE_PATH}; add it under '
                                f'[{DRIVE}] if it can change what reaches an output channel '
                                f'or whether and how a run stops, otherwise under [{NOT_DRIVE}], '
                                'with a short reason (docs/drive-path.md)')
    return findings


# ------------------------------------------------------------ reading Rust

@dataclasses.dataclass
class RustFn:
    """A function found in a Rust file."""
    name: str
    modules: tuple[str, ...]
    attrs: list[str]
    module_attrs: list[str]
    line: int
    end: int


@dataclasses.dataclass
class RustFile:
    """What the checks need from one Rust file."""
    fns: list[RustFn]
    inner_attrs: list[str]
    #: `mod name;` lines at the top level: {name: their attributes}
    mod_decls: dict[str, list[str]]


def _clean(text: str) -> str:
    """`text` with comments blanked and every bracket inside a string or a
    character literal replaced, so that brackets count only where they are
    code. Line breaks and offsets are kept."""
    out = list(text)
    i, n = 0, len(text)

    def blank(start: int, stop: int, brackets_only: bool = False) -> None:
        for k in range(start, stop):
            if out[k] == '\n':
                continue
            if not brackets_only or out[k] in '{}[]()':
                out[k] = ' ' if not brackets_only else '_'

    while i < n:
        c = text[i]
        if text.startswith('//', i):
            j = text.find('\n', i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
        elif text.startswith('/*', i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith('/*', j):
                    depth, j = depth + 1, j + 2
                elif text.startswith('*/', j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif c in 'br' and (raw := re.match(r'b?r(#*)"', text[i:i + 300])) and (
                i == 0 or not (text[i - 1].isalnum() or text[i - 1] == '_')):
            close = '"' + raw.group(1)
            j = text.find(close, i + raw.end())
            j = n if j < 0 else j + len(close)
            blank(i + raw.end(), j - len(close), brackets_only=True)
            i = j
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == '\\' else 1
            blank(i + 1, min(j, n), brackets_only=True)
            i = j + 1
        elif c == "'":
            # a character literal ('x', '\n', '\u{1F600}') or a lifetime ('a)
            m = re.match(r"'(?:\\(?:u\{[0-9A-Fa-f]*\}|x[0-9A-Fa-f]{2}|.)|[^\\'\n])'", text[i:i + 12])
            if m:
                blank(i + 1, i + m.end() - 1, brackets_only=True)
                i += m.end()
            else:
                i += 1
        else:
            i += 1
    return ''.join(out)


_TOKEN = re.compile(r'''
    (?P<attr>\#!?\[)
  | \bmod\s+(?P<mod>[A-Za-z_]\w*)\s*(?P<modend>[;{])
  | \bfn\s+(?P<fn>[A-Za-z_]\w*)
  | (?P<open>\{) | (?P<close>\}) | (?P<semi>;)
''', re.VERBOSE)


def scan_rust(text: str) -> RustFile:
    """The functions, their attributes and their modules, the file's inner
    attributes and its top-level `mod name;` lines."""
    code = _clean(text)
    starts = [0] + [m.end() for m in re.finditer('\n', code)]

    def line_of(pos: int) -> int:
        return bisect.bisect_right(starts, pos)

    # frames: (kind, name, attrs, fn record or None)
    stack: list[tuple[str, str, list[str], RustFn | None]] = []
    pending: list[str] = []
    inner: list[str] = []
    fns: list[RustFn] = []
    decls: dict[str, list[str]] = {}
    awaiting: RustFn | None = None
    pos = 0
    while (m := _TOKEN.search(code, pos)) is not None:
        pos = m.end()
        if m.group('attr'):
            depth, j = 1, pos
            while j < len(code) and depth:
                depth += {'[': 1, ']': -1}.get(code[j], 0)
                j += 1
            body = ' '.join(code[pos:j - 1].split())
            pos = j
            if m.group('attr').startswith('#!'):
                (stack[-1][2] if stack else inner).append(body)
            else:
                pending.append(body)
        elif m.group('mod'):
            if m.group('modend') == ';':
                if not stack:
                    decls[m.group('mod')] = pending
            else:
                stack.append(('mod', m.group('mod'), pending, None))
            pending = []
        elif m.group('fn'):
            modules = tuple(f[1] for f in stack if f[0] == 'mod')
            module_attrs = [a for f in stack if f[0] == 'mod' for a in f[2]]
            awaiting = RustFn(m.group('fn'), modules, pending, module_attrs,
                              line_of(m.start()), line_of(m.start()))
            fns.append(awaiting)
            pending = []
        elif m.group('open'):
            if awaiting is not None:
                stack.append(('fn', awaiting.name, [], awaiting))
                awaiting = None
            else:
                stack.append(('block', '', [], None))
            pending = []
        elif m.group('close'):
            if stack:
                frame = stack.pop()
                if frame[3] is not None:
                    frame[3].end = line_of(m.start())
            pending = []
        else:  # ';' ends an item: a declaration with no body, or a statement
            awaiting = None
            pending = []
    return RustFile(fns, inner, decls)


# ------------------------------------------------------------- cfg and attrs

def _attr_path(attr: str) -> str:
    """An attribute's path: `tokio::test` for `tokio::test(flavor = ...)`."""
    m = re.match(r'\s*([A-Za-z_][\w:]*)', attr)
    return m.group(1) if m else ''


def is_test_attr(attr: str) -> bool:
    """`#[test]`, `#[tokio::test(...)]` and the like."""
    return _attr_path(attr).split('::')[-1] == 'test'


def _split_args(text: str) -> list[str]:
    """Top-level comma-separated parts of an argument list."""
    parts, depth, start = [], 0, 0
    for k, c in enumerate(text):
        if c == '(':
            depth += 1
        elif c == ')':
            depth -= 1
        elif c == ',' and depth == 0:
            parts.append(text[start:k])
            start = k + 1
    parts.append(text[start:])
    return [p.strip() for p in parts if p.strip()]


def eval_cfg(pred: str, platform: Mapping[str, str], features: frozenset[str]) -> bool:
    """Whether a `cfg` predicate holds in a test build on `platform` with
    `features` enabled. A name this function does not know (miri, a custom
    cfg, a misspelling) is false, as rustc treats an unset cfg."""
    pred = pred.strip()
    call = re.fullmatch(r'(all|any|not)\s*\((.*)\)', pred, re.DOTALL)
    if call:
        args = [eval_cfg(a, platform, features) for a in _split_args(call.group(2))]
        if call.group(1) == 'all':
            return all(args)
        if call.group(1) == 'any':
            return any(args)
        return len(args) == 1 and not args[0]
    pair = re.fullmatch(r'([A-Za-z_]\w*)\s*=\s*"([^"]*)"', pred)
    if pair:
        key, value = pair.groups()
        if key == 'feature':
            return value in features
        return platform.get(key) == value
    if pred in ('test', 'debug_assertions'):
        return True
    if pred in ('unix', 'windows'):
        return platform['target_family'] == pred
    return False


def _cfgs(attrs: Iterable[str]) -> list[str]:
    return [m.group(1) for a in attrs if (m := re.fullmatch(r'cfg\s*\((.*)\)', a, re.DOTALL))]


def compiled(attrs: Iterable[str], features: frozenset[str]) -> bool:
    """Whether code under every `cfg` in `attrs` is compiled on at least one
    CI platform."""
    preds = _cfgs(attrs)
    return any(all(eval_cfg(p, platform, features) for p in preds) for platform in PLATFORMS)


def ignored(attrs: Iterable[str], features: frozenset[str]) -> bool:
    """Whether `attrs` ignore a test on some CI platform."""
    for attr in attrs:
        if _attr_path(attr) == 'ignore':
            return True
        m = re.fullmatch(r'cfg_attr\s*\((.*)\)', attr, re.DOTALL)
        if m:
            args = _split_args(m.group(1))
            if any(_attr_path(a) == 'ignore' for a in args[1:]) and any(
                    eval_cfg(args[0], platform, features) for platform in PLATFORMS):
                return True
    return False


# ------------------------------------------------------------ the workspace

def _members(root: pathlib.Path) -> list[pathlib.Path]:
    manifest = tomllib.loads((root / 'Cargo.toml').read_text(encoding='utf-8'))
    members = []
    for pattern in manifest.get('workspace', {}).get('members', []):
        members += [p for p in sorted(root.glob(pattern)) if (p / 'Cargo.toml').is_file()]
    return members


def _dependency_tables(manifest: Mapping) -> Iterable[Mapping]:
    for kind in ('dependencies', 'dev-dependencies', 'build-dependencies'):
        yield manifest.get(kind, {})
    for target in manifest.get('target', {}).values():
        for kind in ('dependencies', 'dev-dependencies', 'build-dependencies'):
            yield target.get(kind, {})


def enabled_features(root: pathlib.Path = ROOT) -> dict[str, frozenset[str]]:
    """The features `cargo test --workspace` enables in each workspace
    crate: its defaults and every feature another member asks of it,
    unified as Cargo unifies them, by package name."""
    manifests = {}
    for member in _members(root):
        manifest = tomllib.loads((member / 'Cargo.toml').read_text(encoding='utf-8'))
        manifests[manifest['package']['name']] = manifest
    shared = tomllib.loads((root / 'Cargo.toml').read_text(encoding='utf-8')).get(
        'workspace', {}).get('dependencies', {})
    requested: dict[str, set[str]] = {name: {'default'} for name in manifests}
    # each member's dependency keys, as package names
    renames: dict[str, dict[str, str]] = {}
    for name, manifest in manifests.items():
        renames[name] = {}
        for table in _dependency_tables(manifest):
            for key, spec in table.items():
                spec = spec if isinstance(spec, dict) else {}
                package = spec.get('package', key)
                renames[name][key] = package
                features = list(spec.get('features', []))
                if spec.get('workspace') and isinstance(shared.get(key), dict):
                    features += shared[key].get('features', [])
                if package in requested:
                    requested[package].update(features)
    changed = True
    enabled: dict[str, set[str]] = {}
    while changed:
        changed = False
        for name, manifest in manifests.items():
            table = manifest.get('features', {})
            done, todo = set(), list(requested[name])
            while todo:
                feature = todo.pop()
                if feature in done:
                    continue
                done.add(feature)
                for item in table.get(feature, []):
                    if item.startswith('dep:'):
                        continue
                    if '/' in item:
                        dep, sub = item.split('/', 1)
                        package = renames[name].get(dep.rstrip('?'), dep.rstrip('?'))
                        if package in requested and sub not in requested[package]:
                            requested[package].add(sub)
                            changed = True
                    else:
                        todo.append(item)
            enabled[name] = done
    return {name: frozenset(features) for name, features in enabled.items()}


class Source:
    """The Rust sources of the workspace, read once each."""

    def __init__(self, root: pathlib.Path = ROOT):
        self.root = root
        self._files: dict[pathlib.Path, RustFile | None] = {}
        self._features: dict[str, frozenset[str]] | None = None

    def file(self, path: pathlib.Path) -> RustFile | None:
        if path not in self._files:
            self._files[path] = (scan_rust(path.read_text(encoding='utf-8', errors='replace'))
                                 if path.is_file() else None)
        return self._files[path]

    def features(self, crate: str) -> frozenset[str]:
        if self._features is None:
            self._features = enabled_features(self.root)
        return self._features.get(crate, frozenset({'default'}))

    def manifest(self, crate_dir: pathlib.Path) -> Mapping:
        return tomllib.loads((crate_dir / 'Cargo.toml').read_text(encoding='utf-8'))

    def reach(self, crate_dir: pathlib.Path, rel: pathlib.PurePosixPath,
              features: frozenset[str]) -> str | None:
        """Why the file `rel` of a crate is not compiled into its test build,
        or None when it is."""
        manifest = self.manifest(crate_dir)
        parts = rel.parts
        if parts[0] == 'tests' and len(parts) == 2:
            target = next((t for t in manifest.get('test', [])
                           if t.get('path') == rel.as_posix() or t.get('name') == rel.stem),
                          None)
            if target is None and manifest.get('package', {}).get('autotests') is False:
                return 'autotests = false and no [[test]] names this file'
            needed = set((target or {}).get('required-features', []))
            if needed - features:
                return (f'its [[test]] target needs the features {sorted(needed - features)}, '
                        'which the workspace does not enable')
            return None
        if parts[0] != 'src':
            return 'it is neither under src/ nor a tests/<name>.rs file'
        if manifest.get('lib', {}).get('test') is False and parts[1:2] != ('main.rs',):
            return 'its crate sets [lib] test = false'
        roots = {'lib.rs', 'main.rs'}
        if len(parts) == 2 and parts[1] in roots:
            return None
        if len(parts) == 3 and parts[1] == 'bin':
            return None
        # the parent module's file, and this file's module name in it
        if rel.name == 'mod.rs':
            name, parent_dir = rel.parent.name, rel.parent.parent
        else:
            name, parent_dir = rel.stem, rel.parent
        if parent_dir == pathlib.PurePosixPath('src'):
            candidates = [parent_dir / r for r in sorted(roots)]
        else:
            candidates = [parent_dir.with_suffix('.rs'), parent_dir / 'mod.rs']
        found = False
        for parent in candidates:
            scanned = self.file(crate_dir / parent)
            if scanned is None or name not in scanned.mod_decls:
                continue
            found = True
            attrs = scanned.mod_decls[name] + scanned.inner_attrs
            if not compiled(attrs, features):
                continue
            if self.reach(crate_dir, parent, features) is None:
                return None
        if not found:
            return 'no `mod` line reaches it from the crate root'
        return 'a `cfg` on the `mod` lines that reach it leaves it out'


def check_safety_tests(entries: Sequence[SafetyTest], root: pathlib.Path = ROOT,
                       source: Source | None = None) -> list[str]:
    """Rule 2: each listed test exists, is a test, is not ignored and is
    compiled into the test build."""
    source = source or Source(root)
    findings = []
    for entry in entries:
        crate_dir = root / 'crates' / entry.crate
        where = f'{SAFETY_TESTS}:{entry.line}: {entry.where}'
        if not (crate_dir / 'Cargo.toml').is_file():
            findings.append(f'{where}: there is no crate crates/{entry.crate}')
            continue
        rel = pathlib.PurePosixPath(entry.file)
        scanned = source.file(crate_dir / rel)
        if scanned is None:
            findings.append(f'{where}: the file does not exist; if the test moved, update its '
                            'line; if it was removed, see docs/drive-path.md')
            continue
        *mods, name = entry.test.split('::')
        matches = [f for f in scanned.fns if f.name == name
                   and (not mods or f.modules[-len(mods):] == tuple(mods))]
        tests = [f for f in matches if any(is_test_attr(a) for a in f.attrs)]
        if not matches:
            findings.append(f'{where}: no function of that name in the file; if the test was '
                            'renamed or moved, update its line; if it was removed, see '
                            'docs/drive-path.md')
            continue
        if not tests:
            findings.append(f'{where}: the function is not marked as a test')
            continue
        if len(tests) > 1:
            paths = ', '.join('::'.join((*f.modules, f.name)) for f in tests)
            findings.append(f'{where}: names more than one test ({paths}); write module::name')
            continue
        fn = tests[0]
        features = source.features(entry.crate)
        if ignored(fn.attrs, features):
            findings.append(f'{where}: is ignored; a safety test must run. Remove the ignore, '
                            f'or remove the line with "{WEAKENS}" in the title')
            continue
        if not compiled(fn.attrs + fn.module_attrs + scanned.inner_attrs, features):
            findings.append(f'{where}: a `cfg` leaves it out of the test build on every CI '
                            'platform')
            continue
        why = source.reach(crate_dir, rel, features)
        if why is not None:
            findings.append(f'{where}: the file is not compiled into the test build: {why}')
    return findings


# ---------------------------------------------------------- pull requests

class GitError(Exception):
    """git could not answer."""


def git(root: pathlib.Path, *args: str) -> str:
    out = subprocess.run(['git', *args], cwd=root, capture_output=True, text=True,
                         check=False)
    if out.returncode != 0:
        raise GitError(f'git {" ".join(args)}: {out.stderr.strip()}')
    return out.stdout


def show(root: pathlib.Path, rev: str, path: str) -> str | None:
    """A file's text at `rev`, or None when it is not there."""
    try:
        return git(root, 'show', f'{rev}:{path}')
    except GitError:
        return None


def changed_paths(root: pathlib.Path, base: str) -> list[str]:
    """Every path the diff from `base` to the working tree's HEAD adds,
    changes, removes or renames (both names of a rename)."""
    out = git(root, 'diff', '--name-status', '-M', '-z', base, 'HEAD')
    fields = out.split('\0')
    paths, k = [], 0
    while k < len(fields) and fields[k]:
        status = fields[k]
        count = 2 if status[0] in 'RC' else 1
        paths += fields[k + 1:k + 1 + count]
        k += 1 + count
    return paths


def section_text(body: str) -> str | None:
    """The drive-path section of a pull request description, with HTML
    comments taken out, or None when it has none."""
    body = re.sub(r'<!--.*?-->', '', body.replace('\r\n', '\n'), flags=re.DOTALL)
    m = re.search(rf'^#{{2,3}}\s*{re.escape(SECTION)}\s*$(.*?)(?=^#{{1,3}}\s|\Z)', body,
                  flags=re.MULTILINE | re.DOTALL | re.IGNORECASE)
    return None if m is None else m.group(1)


def answers(section: str) -> dict[str, str]:
    """Each of the four answers in the section, as written."""
    found: dict[str, list[str]] = {}
    current = None
    for line in section.splitlines():
        plain = line.strip().lstrip('-*').strip().replace('**', '').replace('__', '')
        label = next((a for a in ANSWERS if plain.lower().startswith(a.lower())), None)
        if label is not None:
            current = label
            rest = plain[len(label):].lstrip(' ?').lstrip(':').strip()
            found[current] = [rest]
        elif current is not None:
            found[current].append(plain)
    return {label: ' '.join(' '.join(parts).split()) for label, parts in found.items()}


def check_description(body: str, touched: Sequence[str]) -> list[str]:
    """Rule 3: a pull request that touches the drive path carries the
    section, filled in."""
    if not touched:
        return []
    shown = ', '.join(touched[:5]) + (f' and {len(touched) - 5} more' if len(touched) > 5
                                      else '')
    section = section_text(body)
    if section is None:
        return [(f'This pull request changes drive-path code ({shown}), and its description '
                 f'has no "## {SECTION}" section. Add the section from '
                 '.github/pull_request_template.md and answer its four questions '
                 '(docs/drive-path.md).')]
    findings = []
    if UNTOUCHED.lower().rstrip('.') in ' '.join(section.lower().split()):
        findings.append(f'This pull request changes drive-path code ({shown}), but its '
                        f'description says "{UNTOUCHED}" Replace that line with the four '
                        'answers.')
    given = answers(section)
    for label in ANSWERS:
        value = given.get(label, '')
        if value.lower().strip(' .') in PLACEHOLDERS:
            findings.append(f'The "{SECTION}" section does not answer "{label}". This pull '
                            f'request changes drive-path code ({shown}); answer it in a '
                            'sentence or two (docs/drive-path.md).')
    return findings


def check_weakening(base_tests: Sequence[SafetyTest], head_tests: Sequence[SafetyTest],
                    base_paths: Sequence[Listed], head_paths: Sequence[Listed],
                    title: str, root: pathlib.Path = ROOT) -> list[str]:
    """Rule 4: a removed safety test line, or a path moved off the drive
    path, needs the title to say so."""
    if WEAKENS.lower() in title.lower():
        return []
    findings = []
    head_keys = {t.key for t in head_tests}
    head_names = {(t.crate, t.test.split('::')[-1]) for t in head_tests}
    for test in base_tests:
        if test.key in head_keys or (test.crate, test.test.split('::')[-1]) in head_names:
            continue
        findings.append(f'{test.where} ({test.kind}) is no longer in {SAFETY_TESTS}. Removing '
                        f'a safety test weakens a protection: put "{WEAKENS}" in the pull '
                        'request title and say why in its description, or restore the line.')
    head_drive = {p.path for p in head_paths if p.section == DRIVE}
    for listed in base_paths:
        if listed.section != DRIVE or listed.path in head_drive:
            continue
        if not (root / listed.path).exists():
            continue
        findings.append(f'{listed.path} is no longer on the drive-path list. Moving a path off '
                        f'it loosens the rules that path is held to: put "{WEAKENS}" in the '
                        'pull request title and say why, or restore the line.')
    return findings


def changed_lines(root: pathlib.Path, base: str, path: str) -> set[int]:
    """The lines of `path` at HEAD that the diff from `base` adds or changes."""
    out = git(root, 'diff', '-U0', base, 'HEAD', '--', path)
    lines = set()
    for m in re.finditer(r'^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@', out, re.MULTILINE):
        start, count = int(m.group(1)), int(m.group(2) or 1)
        lines.update(range(start, start + max(count, 1)))
    return lines


def edited_tests(tests: Sequence[SafetyTest], changed: Sequence[str], base: str,
                 root: pathlib.Path = ROOT, source: Source | None = None) -> list[str]:
    """Warnings: listed tests that were there at the base and whose body the
    diff edits."""
    source = source or Source(root)
    warnings = []
    for test in tests:
        path = f'crates/{test.crate}/{test.file}'
        if path not in changed:
            continue
        scanned = source.file(root / path)
        before = show(root, base, path)
        name = test.test.split('::')[-1]
        if scanned is None or before is None or not any(
                fn.name == name for fn in scan_rust(before).fns):
            continue  # a test new in this pull request weakens nothing
        lines = changed_lines(root, base, path)
        for fn in scanned.fns:
            if fn.name == name and any(fn.line <= n <= fn.end for n in lines):
                warnings.append(f'{test.where} ({test.kind}) is edited. If the edit weakens it '
                                f'(a looser bound, a smaller case, an assertion taken out), the '
                                f'title must carry "{WEAKENS}".')
                break
    return warnings


@dataclasses.dataclass
class PullRequest:
    """What the pull-request checks read."""
    base: str
    title: str
    body: str


def check_pull_request(pr: PullRequest, root: pathlib.Path = ROOT,
                       source: Source | None = None) -> tuple[list[str], list[str]]:
    """Rules 3 and 4: the findings, and the warnings for the reviewer."""
    base = git(root, 'merge-base', pr.base, 'HEAD').strip()
    changed = changed_paths(root, base)
    base_paths, _ = parse_drive_path(show(root, base, DRIVE_PATH) or '')
    head_paths, _ = parse_drive_path(show(root, 'HEAD', DRIVE_PATH) or '')
    base_tests, _ = parse_safety_tests(show(root, base, SAFETY_TESTS) or '')
    head_tests, _ = parse_safety_tests(show(root, 'HEAD', SAFETY_TESTS) or '')
    drive = {p.path for p in base_paths + head_paths if p.section == DRIVE}
    touched = [path for path in changed if covered(path, drive)]
    findings = (check_description(pr.body, touched)
                + check_weakening(base_tests, head_tests, base_paths, head_paths, pr.title,
                                  root))
    warnings = edited_tests(head_tests, changed, base, root, source)
    if touched:
        print(f'check_drive_path: this pull request touches the drive path: {", ".join(touched)}')
    else:
        print('check_drive_path: this pull request touches no drive-path path')
    return findings, warnings


def pull_request_from_event(path: pathlib.Path) -> PullRequest | None:
    """The pull request a GitHub event payload describes, or None for an
    event that is not about one."""
    event = json.loads(path.read_text(encoding='utf-8'))
    pr = event.get('pull_request')
    if not pr:
        return None
    return PullRequest(pr['base']['sha'], pr.get('title') or '', pr.get('body') or '')


# ------------------------------------------------------------------- main

def _say(kind: str, message: str) -> None:
    if os.environ.get('GITHUB_ACTIONS') == 'true':
        print(f'::{kind}::{message}')
    else:
        print(f'{kind}: {message}')


def main(argv: Sequence[str] = (), root: pathlib.Path = ROOT) -> int:
    parser = argparse.ArgumentParser(prog='check_drive_path.py', description=__doc__.split(
        '\n\n')[0])
    parser.add_argument('--event', type=pathlib.Path,
                        help='a GitHub event payload; on a pull request event, also check '
                             'the pull request')
    parser.add_argument('--base', help='check this branch as a pull request into BASE')
    parser.add_argument('--title', default='', help='the pull request title (with --base)')
    parser.add_argument('--body-file', type=pathlib.Path,
                        help='a file holding the pull request description (with --base)')
    args = parser.parse_args(list(argv))
    if args.event and args.base:
        parser.error('give --event or --base, not both')

    paths_text = (root / DRIVE_PATH).read_text(encoding='utf-8') \
        if (root / DRIVE_PATH).is_file() else None
    tests_text = (root / SAFETY_TESTS).read_text(encoding='utf-8') \
        if (root / SAFETY_TESTS).is_file() else None
    findings, warnings = [], []
    if paths_text is None or tests_text is None:
        findings.append(f'{DRIVE_PATH} and {SAFETY_TESTS} must both exist')
        paths, tests = [], []
    else:
        paths, problems = parse_drive_path(paths_text)
        findings += problems + check_lists(paths, root)
        tests, problems = parse_safety_tests(tests_text)
        source = Source(root)
        findings += problems + check_safety_tests(tests, root, source)

    try:
        pr = None
        if args.event:
            pr = pull_request_from_event(args.event)
            if pr is None:
                print('check_drive_path: not a pull request event; the lists only')
        elif args.base:
            body = args.body_file.read_text(encoding='utf-8') if args.body_file else ''
            pr = PullRequest(args.base, args.title, body)
        if pr is not None:
            more, warned = check_pull_request(pr, root)
            findings += more
            warnings += warned
    except GitError as error:
        _say('error', f'the pull-request checks could not run: {error}')
        return 2

    for warning in warnings:
        _say('warning', warning)
    for finding in findings:
        _say('error', finding)
    drive = sum(p.section == DRIVE for p in paths)
    print(f'check_drive_path: {drive} drive-path paths, {len(paths) - drive} not, '
          f'{len(tests)} safety tests, {len(findings)} findings, {len(warnings)} warnings')
    return 1 if findings else 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
