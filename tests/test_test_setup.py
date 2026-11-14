"""The test setup kind: one test module's configuration, saved into a
project and read back whole, alone or in a project, across a save and
reopen of the project; a file holding one is stamped schema 4 and any
other keeps its earlier stamp, so an older file still opens and an
older reader refuses the new one by its stamp; its origin is checked;
and a setup taken from a run's record is that run's configuration."""

from __future__ import annotations

import h5py
import pytest

from hertzforge import Project
from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.run_record import RunRecord
from hertzforge.core.test_setup import MODULES, TestSetup, origin
from hertzforge.io import native

CONFIG = {'title': 'Qualification', 'profile': {'breakpoints': [
    {'freq_hz': 20.0, 'psd_g2_hz': 0.01}, {'freq_hz': 2000.0, 'psd_g2_hz': 0.01}]},
    'acquisition': {'fs_hz': 5120.0, 'n_pts': 1024}, 'schedule': {'levels': [{'db': 0}]},
    'a_member_this_build_does_not_know': {'kept': [1, 2.5, None, 'words']}}

TABLE = {'revision': 3, 'sha256': 'ab' * 32,
         'rows': [{'index': 1, 'role': 'control', 'units': 'g', 'enabled': True}]}


def _setup(**changes):
    held = {'module': 'random', 'config': CONFIG, 'table': TABLE, 'title': 'Qualification',
            'notes': 'first article\nsecond line', 'origin': origin('setup', name='Earlier')}
    held.update(changes)
    return TestSetup(**held)


def test_a_setup_reads_back_whole_alone_and_in_a_project(tmp_path):
    setup = _setup()
    native.save(setup, tmp_path / 'alone')
    assert native.load(tmp_path / 'alone.hforge') == setup
    project = Project('setups')
    project.add('Qualification', setup)
    once = Project.open(project.save(tmp_path / 'once'))
    assert once['Qualification'] == setup
    twice = Project.open(once.save(tmp_path / 'twice'))
    held = twice['Qualification']
    assert held == setup
    assert held.config['a_member_this_build_does_not_know'] == {'kept': [1, 2.5, None, 'words']}
    assert held.notes == 'first article\nsecond line'
    assert held.configure == ('random.configure', {'config': CONFIG})


def test_only_a_file_holding_a_setup_is_stamped_4(tmp_path):
    project = Project('plain')
    project.add('Table', ChannelTable({'channel': [1], 'node': ['1'], 'direction': ['X+'],
                                       'unit': ['g']}))
    with h5py.File(project.save(tmp_path / 'plain.hforge')) as f:
        assert f.attrs[native.STAMP] == native.BASE_SCHEMA
    project.add('Setup', _setup())
    with h5py.File(project.save(tmp_path / 'setup.hforge')) as f:
        assert f.attrs[native.STAMP] == native.TEST_SETUP_SCHEMA == 4
        assert dict(f['objects/0001'].attrs)['origin_kind'] == 'setup'


def test_an_older_reader_refuses_a_setup_file_and_an_older_file_opens(tmp_path, monkeypatch):
    project = Project('setups')
    project.add('Setup', _setup())
    path = project.save(tmp_path / 'setup.hforge')
    record_only = Project('run')
    record_only.add('Run Record', RunRecord({'test': 'random', 'run_uid': 'u'}, {}, {}))
    older = record_only.save(tmp_path / 'record.hforge')
    monkeypatch.setattr(native, 'SCHEMA_VERSION', native.RUN_RECORD_SCHEMA)
    with pytest.raises(ValueError, match=(
            r'newer Hertzforge \(schema 4; this build reads up to 3\)')):
        native.load(path)
    assert isinstance(native.load(older)['Run Record'], RunRecord)


def test_an_origin_names_what_its_kind_needs():
    assert origin() == {'kind': 'typed'}
    assert origin('run', run_id='r-0001', name=None) == {'kind': 'run', 'run_id': 'r-0001'}
    seed = origin('seed', object='Spec', checksum='cd' * 32, fills='profile')
    assert _setup(origin=seed).origin == seed
    for kind, members, words in (('pasted', {}, 'one of typed, setup, run, seed'),
                                 ('setup', {}, 'names its name'),
                                 ('run', {'name': 'Run Record'}, 'run_uid or run_id'),
                                 ('seed', {'object': 'Spec'}, 'names its checksum')):
        with pytest.raises(ValueError, match=words):
            origin(kind, **members)
    with pytest.raises(ValueError, match='not \'shaker\''):
        _setup(module='shaker')


def test_a_setup_from_a_run_is_the_configuration_the_run_ran_with():
    record = RunRecord(
        {'test': 'classical_shock', 'run_uid': 'uid-7', 'run_id': 'r-0007',
         'title': 'Drop', 'notes': 'after the fixture change'},
        {'revision': 5, 'sha256': 'ef' * 32, 'rows': TABLE['rows'], 'extra': 'not kept'},
        {'config': {'title': 'Drop', 'pulse': {'shape': 'half_sine'}}, 'resolved': {'x': 1}})
    setup = TestSetup.from_run(record, 'Run Record')
    assert setup.config == record.config
    assert setup.table == {'rows': TABLE['rows'], 'revision': 5, 'sha256': 'ef' * 32}
    assert (setup.module, setup.title, setup.notes) == ('classical_shock', 'Drop',
                                                        'after the fixture change')
    assert setup.origin == {'kind': 'run', 'run_uid': 'uid-7', 'run_id': 'r-0007',
                            'name': 'Run Record'}
    assert setup.configure[0] == 'shock.configure'
    with pytest.raises(ValueError, match='holds no configuration'):
        TestSetup.from_run(RunRecord({'test': 'random'}, {}, {}))
    with pytest.raises(ValueError, match='no setup'):
        TestSetup.from_run(RunRecord({'test': 'modal'}, {}, {'config': {'a': 1}}))


def test_every_module_has_a_configure_request():
    from hertzforge.core.test_setup import CONFIGURE

    assert set(CONFIGURE) == set(MODULES)
