"""A Playback run opens in the analysis side: every input and every drive
as played becomes a time history, the check report and the outcome are kept
in the run record as the record keeps a module's results, and the setup it
ran opens again. The folder is synthetic (`run_folders.playback_run`),
written to `docs/run-folder.md`'s layout."""

from __future__ import annotations

import numpy as np
import pytest
import run_folders as rf

from hertzforge import Project
from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.data import TimeHistory
from hertzforge.core.run_record import RunRecord
from hertzforge.core.test_setup import TestSetup
from hertzforge.io import run_convert, run_folder

PRE, PLAY, POST = 512, 2048, 1024


@pytest.fixture
def playback(tmp_path):
    folder = rf.playback_run(str(tmp_path / 'playback'), PRE, PLAY, POST)
    done = run_convert.convert(folder)
    return folder, done, Project.open(done.path)


def _samples(stream):
    return np.concatenate([block for _start, block in stream.blocks(4096)])


def _record(project):
    (record,) = [project[n] for n in project if isinstance(project[n], RunRecord)]
    return record


def test_every_input_and_every_drive_is_a_time_history(playback):
    folder, done, project = playback
    assert isinstance(project['Channel Table'], ChannelTable)
    inputs, drives = project['inputs'], project['drives']
    assert isinstance(inputs, TimeHistory) and isinstance(drives, TimeHistory)
    assert inputs.ordinate.shape == (2, PRE + PLAY + POST)
    assert drives.ordinate.shape == (2, PRE + PLAY + POST)
    assert inputs.sample_rate == drives.sample_rate == rf.FS
    # inputs in SI from g; the drives in volts as played, the level and the rolls included
    run = run_folder.read(folder)
    np.testing.assert_allclose(inputs.ordinate, _samples(run.streams['inputs']).T * rf.G,
                               rtol=1e-6)
    np.testing.assert_array_equal(drives.ordinate.astype(np.float32),
                                  _samples(run.streams['drives']).T)
    assert drives.ordinate_unit == ['V', 'V']
    assert not np.any(drives.ordinate[:, :PRE]) and not np.any(drives.ordinate[:, -POST:])
    assert set(drives.roles.values()) == {'reference'}
    assert inputs.source == drives.source == 'SIM'
    assert done.lacked == []


def test_the_check_and_the_outcome_are_kept_in_the_run_record(playback):
    _folder, _done, project = playback
    record = _record(project)
    assert record.test == 'playback' and record.outcome == 'completed'
    check = record.results['playback_check']
    assert check['kind'] == 'playback_check' and check['passed'] is True
    assert check['drive'] == rf.PLAYBACK_DRIVE
    assert [c['channel'] for c in check['checks']] == [3, 4]
    np.testing.assert_array_equal(record.arrays['playback_check']['peak'], [0.5, 0.75])
    assert record.arrays['playback_check']['envelope'].shape == (2, 2, 16)
    outcome = record.results['playback_outcome']
    assert (outcome['kind'], outcome['samples_played'], outcome['outcome']) == (
        'playback_outcome', PLAY, 'completed')
    assert [e['kind'] for e in record.log] == ['check', 'state', 'state', 'state', 'state',
                                              'state']
    # the drive file the run played, pinned, and its sidecar as resolved
    assert record.config['drive'] == rf.PLAYBACK_DRIVE
    assert record.resolved['drive']['sha256'] == rf.PLAYBACK_DRIVE['sha256']


def test_the_setup_it_ran_opens_from_the_run(playback):
    _folder, _done, project = playback
    setup = TestSetup.from_run(_record(project))
    assert setup.module == 'playback'
    assert setup.config['drive'] == rf.PLAYBACK_DRIVE
    method, params = setup.configure
    assert method == 'playback.configure'
    assert params['config']['level_db'] == -12.0
    assert params['config']['map'] == [3, 4]
    assert setup.table['revision'] == 5


def test_the_same_folder_converts_to_the_same_bytes(tmp_path):
    folder = rf.playback_run(str(tmp_path / 'playback'))
    first = run_convert.convert(folder, str(tmp_path / 'a.hforge')).path
    second = run_convert.convert(folder, str(tmp_path / 'b.hforge')).path
    with open(first, 'rb') as a, open(second, 'rb') as b:
        assert a.read() == b.read()


def test_a_report_names_the_test():
    from hertzforge.report.run_records import TEST_NAMES

    assert TEST_NAMES['playback'] == 'Playback'
