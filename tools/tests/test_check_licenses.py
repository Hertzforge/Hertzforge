"""The license and import check, on planted packages and files that must
trip it, and on expressions whose grouping decides the answer."""

from __future__ import annotations

import pathlib
import sys
from importlib import metadata

import pytest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))

import check_licenses as cl

PYTHON = cl.ALLOWED_PYTHON


@pytest.mark.parametrize('expression,verdict', [
    ('MIT', cl.ALLOWED),
    ('GPL-3.0-only', cl.REFUSED),
    ('GPL-2.0-or-later', cl.REFUSED),
    ('GPL-2.0+', cl.REFUSED),
    ('AGPL-3.0-or-later', cl.REFUSED),
    ('SSPL-1.0', cl.REFUSED),
    # a free-text License field's spelling, read as the family it names
    ('GPLv3', cl.REFUSED),
    ('GPLv2+', cl.REFUSED),
    ('AGPLv3', cl.REFUSED),
    ('GPL', cl.REFUSED),
    ('LGPLv3', cl.UNKNOWN),
    ('GPLish-1.0', cl.UNKNOWN),
    ('LGPL-3.0-only', cl.ALLOWED),
    ('MPL-2.0', cl.ALLOWED),
    ('LGPL-3.0-only OR GPL-2.0-only OR GPL-3.0-only', cl.ALLOWED),
    ('MIT AND GPL-3.0', cl.REFUSED),
    ('(MIT OR Apache-2.0) AND GPL-3.0', cl.REFUSED),
    ('GPL-3.0 AND (MIT OR Apache-2.0)', cl.REFUSED),
    ('MIT AND GPL-3.0 OR BSD-3-Clause', cl.ALLOWED),
    ('MIT AND (GPL-3.0 OR BSD-3-Clause)', cl.ALLOWED),
    ('GPL-2.0-only WITH Classpath-exception-2.0', cl.REFUSED),
    ('Apache-2.0 WITH LLVM-exception', cl.ALLOWED),
    ('mit or gpl-3.0', cl.UNKNOWN),
    ('LicenseRef-Proprietary', cl.UNKNOWN),
    ('BSD-3-Clause AND 0BSD AND MIT AND Zlib AND CC0-1.0', cl.ALLOWED),
    ('Copyright (c) 2001, Somebody', cl.UNKNOWN),
    ('(MIT', cl.UNKNOWN),
    ('MIT OR', cl.UNKNOWN),
    ('', cl.UNKNOWN),
    (None, cl.UNKNOWN),
])
def test_expressions(expression, verdict):
    assert cl.judge_text(expression, PYTHON) == verdict


def _dist(root: pathlib.Path, name: str, *lines: str,
          requires: tuple[str, ...] = ()) -> None:
    """A minimal installed distribution: `name`-1.0.dist-info/METADATA."""
    info = root / f'{name.replace("-", "_")}-1.0.dist-info'
    info.mkdir()
    header = ['Metadata-Version: 2.4', f'Name: {name}', 'Version: 1.0', *lines,
              *(f'Requires-Dist: {r}' for r in requires)]
    (info / 'METADATA').write_text('\n'.join(header) + '\n', encoding='utf-8')


def _installed(root: pathlib.Path) -> list[metadata.Distribution]:
    return list(metadata.distributions(path=[str(root)]))


def test_packages_are_judged_by_their_own_metadata(tmp_path):
    _dist(tmp_path, 'plain', 'License-Expression: MIT')
    _dist(tmp_path, 'qtlike', 'License: LGPL-3.0-only OR GPL-2.0-only OR GPL-3.0-only')
    _dist(tmp_path, 'oldstyle', 'License: Copyright (c) 2001, Somebody',
          'Classifier: License :: OSI Approved :: BSD License')
    _dist(tmp_path, 'viral', 'License-Expression: GPL-3.0-or-later')
    _dist(tmp_path, 'classified', 'Classifier: License :: OSI Approved :: '
          'GNU General Public License v3 (GPLv3)', requires=())
    _dist(tmp_path, 'mixed', 'License: MIT', 'Classifier: License :: OSI Approved :: '
          'GNU General Public License v2 (GPLv2)')
    _dist(tmp_path, 'silent')
    _dist(tmp_path, 'app', 'License-Expression: MIT', requires=('viral>=1', 'silent'))
    assert cl.check_installed(_installed(tmp_path), reviewed={}) == [
        ('classified 1.0 is copyleft: classifiers: '
         'GNU General Public License v3 (GPLv3)'),
        ('mixed 1.0 is copyleft: classifiers: '
         'GNU General Public License v2 (GPLv2)'),
        ('silent 1.0 (required by app) has a license this check cannot read or does '
         'not allow: no readable license (License field: none stated)'),
        ('viral 1.0 (required by app) is copyleft: '
         'License-Expression: GPL-3.0-or-later'),
    ]


def test_a_reviewed_package_is_let_through_but_never_a_copyleft_one(tmp_path):
    _dist(tmp_path, 'silent')
    _dist(tmp_path, 'viral', 'License-Expression: GPL-3.0-only')
    reviewed = {'silent': 'MIT, read from its LICENSE file', 'viral': 'no'}
    assert cl.check_installed(_installed(tmp_path), reviewed=reviewed) == [
        'viral 1.0 is copyleft: License-Expression: GPL-3.0-only']


def test_an_expression_outranks_the_classifiers(tmp_path):
    _dist(tmp_path, 'relicensed', 'License-Expression: MIT',
          'Classifier: License :: OSI Approved :: GNU General Public License (GPL)')
    assert cl.check_installed(_installed(tmp_path), reviewed={}) == []


def _write(path: pathlib.Path, text: str) -> pathlib.Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding='utf-8')
    return path


def test_every_import_must_be_accounted_for(tmp_path):
    reader = _write(tmp_path / 'src' / 'pkg' / 'reader.py', """\
import json
import numpy as np
from pkg import helpers
from .local import thing
import sdynpy
from rattlesnake.components import x
import importlib
importlib.import_module("forcefinder.core")
__import__("sdynpy")
""")
    helpers = _write(tmp_path / 'src' / 'pkg' / 'helpers.py', 'import pkg\n')
    init = _write(tmp_path / 'src' / 'pkg' / '__init__.py', '')
    files = [reader, helpers, init]
    found = cl.check_imports(files, cl.first_party(files),
                             provided={'numpy': ['numpy']}, root=tmp_path)
    tail = ('which no installed package provides; declare the package that does, so its '
            'license is checked')
    assert found == [
        f'src/pkg/reader.py:5: imports sdynpy, {tail}',
        f'src/pkg/reader.py:6: imports rattlesnake, {tail}',
        f'src/pkg/reader.py:8: imports forcefinder, {tail}',
        f'src/pkg/reader.py:9: imports sdynpy, {tail}',
    ]


def test_a_module_inside_a_package_is_not_a_top_level_name(tmp_path):
    reader = _write(tmp_path / 'src' / 'pkg' / 'io' / 'rattlesnake.py', 'x = 1\n')
    init = _write(tmp_path / 'src' / 'pkg' / '__init__.py', '')
    io_init = _write(tmp_path / 'src' / 'pkg' / 'io' / '__init__.py', '')
    user = _write(tmp_path / 'src' / 'pkg' / 'io' / 'reads.py', 'import rattlesnake\n')
    own = cl.first_party([reader, init, io_init, user])
    assert 'pkg' in own[user]
    assert 'rattlesnake' not in own[user]
    (finding,) = cl.check_imports([user], own, provided={}, root=tmp_path)
    assert finding.startswith('src/pkg/io/reads.py:1: imports rattlesnake, ')


def test_a_loose_module_is_ours_beside_it_and_below_it_only(tmp_path):
    tool = _write(tmp_path / 'tools' / 'helper.py', 'x = 1\n')
    test = _write(tmp_path / 'tools' / 'tests' / 'test_helper.py', 'import helper\n')
    elsewhere = _write(tmp_path / 'src' / 'reader.py', 'import helper\n')
    own = cl.first_party([tool, test, elsewhere])
    assert 'helper' in own[test]
    assert 'helper' not in own[elsewhere]


def test_an_installed_package_is_found_by_the_name_it_is_imported_as():
    """`packages_distributions` maps import names to the distributions that
    provide them: pytest is imported as `pytest` and `_pytest`."""
    provided = metadata.packages_distributions()
    assert 'pytest' in provided['pytest']


def test_scopes_cover_the_tree():
    files = cl.tracked_python()
    assert cl.check_scopes(files) == []
    assert all(sum(cl.in_scope(f, s) for s in cl.SCOPES) == 1 for f in files)
