"""The run record kind: saved and read back whole, alone or in a project;
a file holding one is stamped schema 3 and any other keeps the earlier
stamp; an older reader refuses it as written by a newer Hertzforge; an
object of a kind the reader does not know is refused by name."""

from __future__ import annotations

import h5py
import numpy as np
import pytest

from hertzforge import Project
from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.run_record import RunRecord
from hertzforge.io import native


def _record():
    return RunRecord(
        {'schema': 'hertzforge-run/1', 'test': 'random', 'run_uid': 'uid-1',
         'run_id': 'r-0003', 'title': 'Qualification', 'source': 'HARDWARE',
         'complete': True},
        {'revision': 4, 'rows': [{'index': 1, 'role': 'control'}]},
        {'config': {'acquisition': {'n_pts': 1024}}, 'resolved': {'schedule': []}},
        log=[{'seq': k, 'kind': 'state', 'text': f'entry {k}'} for k in range(3)],
        end={'end': {'outcome': 'aborted', 'message': 'envelope abort'}},
        law={'name': 'Diagonal'}, law_file=('law.py', b'\x00\x01law'),
        streams={'inputs': {'kind': 'inputs', 'gaps': [{'at_frame': 9,
                                                        'dropped_frames': 2}]}},
        results={'verdicts': {'kind': 'verdicts'}},
        arrays={'verdicts': {'passed': np.array([1.0, 0.0]),
                             'line': np.arange(4, dtype=np.int64)}},
        missing=['the stream inputs did not complete'])


def test_a_run_record_reads_back_whole(tmp_path):
    record = _record()
    assert (record.test, record.outcome, record.complete, record.source) == (
        'random', 'aborted', True, 'HARDWARE')
    native.save(record, tmp_path / 'alone')
    assert native.load(tmp_path / 'alone.hforge') == record
    project = Project('run')
    project.add('Run Record', record)
    held = Project.open(project.save(tmp_path / 'project'))['Run Record']
    assert held == record
    assert held.law_file == ('law.py', b'\x00\x01law')
    assert held.arrays['verdicts']['line'].dtype == np.int64
    assert [entry['text'] for entry in held.log] == ['entry 0', 'entry 1', 'entry 2']


def test_only_a_file_holding_a_run_record_is_stamped_3(tmp_path):
    project = Project('plain')
    project.add('Table', ChannelTable({'channel': [1], 'node': ['1'], 'direction': ['X+'],
                                       'unit': ['g']}))
    with h5py.File(project.save(tmp_path / 'plain.hforge')) as f:
        assert f.attrs[native.STAMP] == native.BASE_SCHEMA
    project.add('Run Record', _record())
    with h5py.File(project.save(tmp_path / 'run.hforge')) as f:
        assert f.attrs[native.STAMP] == native.RUN_RECORD_SCHEMA
    native.save(_record(), tmp_path / 'alone')
    with h5py.File(tmp_path / 'alone.hforge') as f:
        assert f.attrs[native.STAMP] == native.RUN_RECORD_SCHEMA


def test_an_older_reader_refuses_a_run_record_file(tmp_path, monkeypatch):
    project = Project('run')
    project.add('Run Record', _record())
    path = project.save(tmp_path / 'run.hforge')
    monkeypatch.setattr(native, 'SCHEMA_VERSION', native.REFERENCE_SCHEMA)
    with pytest.raises(ValueError, match=(
            r'newer Hertzforge \(schema 3; this build reads up to 2\)')):
        native.load(path)


def test_an_unknown_kind_is_refused_by_name(tmp_path):
    project = Project('run')
    project.add('Run Record', _record())
    path = project.save(tmp_path / 'future.hforge')
    with h5py.File(path, 'r+') as f:
        f['objects/0000'].attrs['kind'] = 'future_kind'
    with pytest.raises(ValueError, match=(
            r"'Run Record' is of kind 'future_kind'.*newer Hertzforge")):
        native.load(path)
