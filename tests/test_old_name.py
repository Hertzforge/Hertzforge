"""The old product name stays out of the tree, but for the places that let
people bring their work over. A scan of every tracked file, its path and its
text. Each entry below is a place the name must stay: a format identifier a
reader has to match, the file-type label of `.vdyn`, the one sentence telling
a user of the old product that their projects open here, the legal notice MIT
requires, and the address the plate's test data is fetched from until it has
a home of its own."""

from __future__ import annotations

import pathlib
import re
import subprocess

import pytest

ROOT = pathlib.Path(__file__).resolve().parent.parent

#: the name, however it is spelled or wrapped over a line of a comment
NAME = re.compile(r'visual[\s_\-*#/]*dynamics|vdreport', re.IGNORECASE)
ABBREVIATION = re.compile(r'\bVD\b|\bVD_')

#: paths whose whole text may hold the name
WHOLE_FILES = frozenset({
    'tests/test_old_name.py',
    'tests/test_vdyn_corpus.py',
    'testdata/vdlog_corpus/v1.vdlog',
    'testdata/vdyn_corpus/schema1.vdyn',
    'testdata/vdyn_corpus/schema2.vdyn',
    'testdata/vdyn_corpus/schema3.vdyn',
})

#: built from the UI's sources, which are scanned themselves: its minifier names variables with
#: two letters of its own choosing, and once it chose VD (2026-11-13), so only the name is
#: looked for here, not the abbreviation
MINIFIED = frozenset({'src/hertzforge/report/viewer.html'})
#: path: the exact text in it that may hold the name
ALLOWED = {
    'ACKNOWLEDGMENTS.md': ("- **Visual Dynamics**, Brandon Zwink's structural dynamics toolset",),
    'site/public/credits.html': (
        "<li><strong>Visual Dynamics</strong>, Brandon Zwink's structural dynamics toolset",),
    'README.md': ('If you used Visual Dynamics, your `.vdyn` projects open here.',),
    'THIRD_PARTY_NOTICES.md': (
        '## Code and data from Visual Dynamics',
        'were moved from Visual\nDynamics 0.1.0a82 (github.com/visualdynamics/visualdynamics, MIT',
    ),
    'scripts/fetch_testdata.sh': (
        'https://codeload.github.com/visualdynamics/visualdynamics/tar.gz/refs/tags/$TAG',
        '"visualdynamics-${TAG#v}/testdata/plate"',
    ),
    'src/hertzforge/core/run_log.py': ("VDLOG_FORMAT = 'visualdynamics-run-log'",),
    'src/hertzforge/io/__init__.py': ("return 'Visual Dynamics file (.vdyn)'",),
    'src/hertzforge/io/escdf_objects.py': (
        "VDYN_WHOLE = 'visualdynamics.vdyn'",
        "OLDER_PROJECT_RECORD = 'visualdynamics_project'",
    ),
    'src/hertzforge/io/native.py': ("VDYN_STAMP = 'visualdynamics_schema'",),
    'src/hertzforge/io/report_template.py': (
        "OLDER_SUFFIX = '.vdreport'",
        "OLDER_MARKER = 'visualdynamics_report_template'",
    ),
}


def _tracked() -> list[str]:
    try:
        out = subprocess.run(['git', 'ls-files', '-z'], cwd=ROOT, capture_output=True, check=True)
    except (OSError, subprocess.CalledProcessError):
        pytest.skip('not a git checkout')
    return [name for name in out.stdout.decode('utf-8').split('\0') if name]


def _hits(text: str, minified: bool = False) -> list[str]:
    """Each place `text` names the old product, with its line number."""
    found = [*NAME.finditer(text), *(() if minified else ABBREVIATION.finditer(text))]
    return [f'{text.count(chr(10), 0, hit.start()) + 1}: {hit.group()!r}' for hit in found]


def test_the_name_is_only_where_it_is_allowed():
    found = []
    for name in _tracked():
        if name in WHOLE_FILES:
            continue
        found += [f'{name}: in its path' for _hit in _hits(name)]
        path = ROOT / name
        if path.is_symlink() or not path.is_file():
            continue
        try:
            text = path.read_text(encoding='utf-8')
        except UnicodeDecodeError:
            continue
        for allowed in ALLOWED.get(name, ()):
            # blanked, keeping the line numbers of what follows
            text = text.replace(allowed, '\n' * allowed.count('\n'))
        found += [f'{name}:{hit}' for hit in _hits(text, name in MINIFIED)]
    assert not found, 'the old name, outside the allowlist:\n' + '\n'.join(found)


def test_every_allowed_place_still_exists():
    """An entry whose text is gone would let the name back in unnoticed."""
    for name, places in ALLOWED.items():
        text = (ROOT / name).read_text(encoding='utf-8')
        for place in places:
            assert place in text, f'{name} no longer holds {place!r}'
    tracked = set(_tracked())
    assert WHOLE_FILES <= tracked, WHOLE_FILES - tracked


@pytest.mark.parametrize(('text', 'count'), [
    ('as Visual Dynamics draws it', 1),
    ('as Visual\n# Dynamics draws it', 1),
    ('as Visual\n * Dynamics draws it', 1),
    ('import visualdynamics', 1),
    ('VISUALDYNAMICS_TEMPLATES', 1),
    ('house.vdreport', 1),
    ("as VD's pane does", 1),
    ('VD_REPO', 1),
    ('a .vdyn file, VDYN_STAMP, VDLOG_FORMAT, .vdlog', 0),
    ('a visual check of the dynamics', 0),
])
def test_the_scan_sees_each_spelling(text, count):
    assert len(_hits(text)) == count


def test_a_minified_bundle_is_scanned_for_the_name_but_not_its_two_letters():
    assert _hits("var VD=Qe('<b>');", minified=True) == []
    assert len(_hits("title='Visual Dynamics'", minified=True)) == 1
    assert len(_hits('var VD=1;')) == 1
