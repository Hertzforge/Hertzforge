"""A run names the channel table it used: every golden run folder's
`run.json` carries the table's revision and the SHA-256 of its rows
(`channel_table`), equal to what its `table.json` holds, and the converter
carries them into the run record."""

from __future__ import annotations

import json
import os
import shutil

import pytest

from hertzforge import Project
from hertzforge.core.run_record import RunRecord
from hertzforge.io import run_convert

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RUNS = os.path.join(ROOT, 'schema', 'golden', 'runs')
MODULES = sorted(os.listdir(RUNS))


def _json(*parts):
    with open(os.path.join(RUNS, *parts), encoding='utf-8') as f:
        return json.load(f)


@pytest.mark.parametrize('module', MODULES)
def test_run_json_names_the_table_its_table_json_holds(module):
    stamp = _json(module, 'run.json')['channel_table']
    table = _json(module, 'table.json')
    assert stamp == {'revision': table['revision'], 'sha256': table['sha256']}


def test_the_converter_carries_the_table_stamp_into_the_run_record(tmp_path):
    folder = str(tmp_path / 'random')
    shutil.copytree(os.path.join(RUNS, 'random'), folder)
    project = Project.open(run_convert.convert(folder).path)
    records = [project[name] for name in project if isinstance(project[name], RunRecord)]
    assert len(records) == 1
    table = _json('random', 'table.json')
    assert records[0].table_revision == table['revision']
    assert records[0].table_sha256 == table['sha256']


def test_a_record_without_the_stamp_says_nothing():
    record = RunRecord({'test': 'random'}, {'revision': 3, 'sha256': 'ab'}, {})
    assert record.table_revision is None
    assert record.table_sha256 is None
