"""Each of the tree's checks, run on the tree and on a planted case that
must trip it. A check that cannot fail proves nothing."""

from __future__ import annotations

import json
import pathlib
import sys

import pytest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))

import check_tree

# a phrase no real tree contains, listed only for these tests
PLANTED = frozenset({check_tree.phrase_hash('quokka foundry')})


def test_the_tree_is_clean(capsys):
    assert check_tree.main([]) == 0
    assert ', 0 findings' in capsys.readouterr().out


@pytest.mark.parametrize('text', [
    'Built with the Quokka Foundry team.',
    'see quokka-foundry/notes for the rest',
    'QUOKKA_FOUNDRY = 1',
])
def test_a_listed_name_is_found_however_it_is_written(tmp_path, text):
    path = tmp_path / 'notes.md'
    path.write_text(f'first line\n{text}\n', encoding='utf-8')
    found = check_tree.check_names([path], tmp_path, names=PLANTED)
    assert found == ['notes.md: line 2 names a listed organization']


def test_a_listed_name_in_a_path_is_found(tmp_path):
    path = tmp_path / 'quokka_foundry.py'
    path.write_text('x = 1\n', encoding='utf-8')
    assert check_tree.check_names([path], tmp_path, names=PLANTED) == [
        'quokka_foundry.py: its path names a listed organization']


def test_one_word_of_a_listed_pair_is_not_a_hit(tmp_path):
    path = tmp_path / 'notes.md'
    path.write_text('a quokka and a foundry, apart\n', encoding='utf-8')
    assert check_tree.check_names([path], tmp_path, names=PLANTED) == []


def test_the_finding_never_repeats_the_name(tmp_path):
    path = tmp_path / 'notes.md'
    path.write_text('Quokka Foundry\n', encoding='utf-8')
    (finding,) = check_tree.check_names([path], tmp_path, names=PLANTED)
    assert 'quokka' not in finding.lower()


@pytest.mark.parametrize('source,module', [
    ('import sdynpy\n', 'sdynpy'),
    ('from rattlesnake.components import x\n', 'rattlesnake.components'),
    ('import importlib\nimportlib.import_module("forcefinder")\n', 'forcefinder'),
    ('__import__("sdynpy.core")\n', 'sdynpy.core'),
])
def test_a_copyleft_import_is_found(tmp_path, source, module):
    path = tmp_path / 'reader.py'
    path.write_text(source, encoding='utf-8')
    assert check_tree.check_copyleft([path], tmp_path) == [
        f'reader.py: imports {module}, which is copyleft']


def test_a_relative_import_of_the_same_name_is_ours(tmp_path):
    path = tmp_path / 'reader.py'
    path.write_text('from .sdynpy import names\n', encoding='utf-8')
    assert check_tree.check_copyleft([path], tmp_path) == []


@pytest.mark.parametrize('expression,allowed', [
    ('MIT', True), ('(MIT OR GPL-3.0)', True), ('MIT AND OFL-1.1', True),
    ('GPL-3.0', False), ('MIT AND GPL-3.0', False), ('LGPL-3.0', False),
    (None, False), ('', False),
    # a parenthesized choice joined to a copyleft license by AND, either side
    ('(MIT OR Apache-2.0) AND GPL-3.0', False),
    ('GPL-3.0 AND (MIT OR Apache-2.0)', False),
    ('(MIT OR Apache-2.0) AND (ISC OR GPL-3.0)', True),
    ('((MIT OR GPL-3.0) AND (Apache-2.0 OR LGPL-3.0))', True),
    ('MIT AND (GPL-3.0 OR (LGPL-3.0 AND ISC))', False),
    # AND binds tighter than OR: MIT OR (Apache-2.0 AND GPL-3.0), allowed by MIT
    ('MIT OR Apache-2.0 AND GPL-3.0', True),
    ('GPL-3.0 AND MIT OR Apache-2.0', True),
    ('GPL-3.0 OR LGPL-3.0 AND MIT', False),
    # an exception only adds permissions: the license before WITH decides
    ('Apache-2.0 WITH LLVM-exception', True),
    ('GPL-2.0-only WITH Classpath-exception-2.0', False),
    ('(Apache-2.0 WITH LLVM-exception) AND GPL-3.0', False),
    # not an expression: refused, never guessed at
    ('MIT OR', False), ('(MIT', False), ('MIT)', False), ('MIT ISC', False),
    ('AND MIT', False), ('MIT WITH', False), ('()', False),
    ('MIT or Apache-2.0', False), ('SEE LICENSE IN LICENSE.md', False),
])
def test_license_expressions(expression, allowed):
    assert check_tree.license_allowed(expression) is allowed


def test_a_disallowed_or_missing_npm_license_is_found(tmp_path):
    lockfile = tmp_path / 'package-lock.json'
    lockfile.write_text(json.dumps({'packages': {
        '': {'name': 'shell'},
        'node_modules/fine': {'license': 'MIT'},
        'node_modules/viral': {'license': 'GPL-3.0'},
        'node_modules/@scope/quiet': {},
    }}), encoding='utf-8')
    assert check_tree.check_npm_licenses(lockfile) == [
        'package-lock.json: viral is licensed GPL-3.0, which is not on the allowlist',
        ('package-lock.json: @scope/quiet is licensed with no license stated, which is '
         'not on the allowlist'),
    ]


def test_every_offending_line_is_reported(tmp_path):
    path = tmp_path / 'notes.md'
    path.write_text('quokka foundry\nclean\nQuokka Foundry again\n', encoding='utf-8')
    assert check_tree.check_names([path], tmp_path, names=PLANTED) == [
        'notes.md: line 1 names a listed organization',
        'notes.md: line 3 names a listed organization',
    ]
