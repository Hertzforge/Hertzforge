"""The bridge from a control run to the analysis side, on a folder the
control service wrote: the golden Random run folder
(`schema/golden/runs/random/`, written by the control service's own test
`random_golden_run` on the simulator) converts with
`hertzforge.io.run_convert` into a project that `Project.open` opens, with
the channel table that was set, every enabled input and the drive as time
histories, the specification the run was controlled to, the spectra, the
transfer function and the coherence, and the run record; and
`docs/run-folder.md` says exactly what a Random run saves."""

from __future__ import annotations

import json
import os
import re
import shutil

import numpy as np
import pytest

from hertzforge import Project
from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.data import Coherence, Frf, Psd, Specification, TimeHistory
from hertzforge.core.run_record import RunRecord
from hertzforge.io import run_convert, run_folder

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GOLDEN = os.path.join(ROOT, 'schema', 'golden', 'runs', 'random')
G = 9.80665


def _json(*parts):
    with open(os.path.join(GOLDEN, *parts), encoding='utf-8') as f:
        return json.load(f)


@pytest.fixture(scope='module')
def converted(tmp_path_factory):
    folder = str(tmp_path_factory.mktemp('runs') / 'random')
    shutil.copytree(GOLDEN, folder)
    done = run_convert.convert(folder)
    return folder, done, Project.open(done.path)


def test_the_folder_converts_and_opens_with_nothing_lacking(converted):
    _folder, done, project = converted
    assert done.lacked == []
    assert list(project) == done.objects
    assert project.project_type == 'Random Vibration'
    assert project.name == 'Golden Random'


def test_the_channel_table_is_the_table_that_was_set_column_by_column(converted):
    _folder, _done, project = converted
    table = project['Channel Table']
    assert isinstance(table, ChannelTable) and table.has_controller_settings
    expected = {
        'channel': [1, 2, 3], 'node': ['1', '2', '3'],
        'comment': ['Control', 'Monitor', 'Drive'],
        'role': ['response', 'response', 'reference'],
        'control': ['True', 'False', 'False'], 'drive': ['False', 'False', 'True'],
        'enabled': ['True', 'True', 'True'], 'unit': ['g', 'g', 'V'],
        'sensitivity': ['100', '100', '1000'], 'range_max': ['100', '100', '10'],
        'physical': ['', '', ''], 'iepe': ['False', 'False', 'False'],
        'iepe_bias': ['', '', ''], 'polarity': ['positive'] * 3,
        'control_group': ['', '', ''], 'abort_accel_g': ['', '', ''],
        'abort_velocity_ips': ['', '', ''], 'abort_displacement_mil': ['', '', '']}
    for name, values in expected.items():
        assert list(table[name]) == [v if name == 'channel' else str(v) for v in values], name
    # and the table is the one the run armed with, row for row
    rows = _json('table.json')['rows']
    back = run_convert.rows_from_channel_table(table)
    for given, read in zip(rows, back, strict=True):
        assert {k: v for k, v in read.items() if k != 'extra'} == given


def test_every_enabled_input_and_the_drive_is_a_time_history(converted):
    folder, _done, project = converted
    inputs, drives = project['inputs'], project['drives']
    assert isinstance(inputs, TimeHistory) and isinstance(drives, TimeHistory)
    frames = _json('streams', 'inputs', 'stream.json')['frames']
    assert frames > 0
    assert inputs.ordinate.shape == (2, frames)
    assert drives.ordinate.shape == (1, _json('streams', 'drives', 'stream.json')['frames'])
    assert inputs.response_dof == ['1', '2'] and drives.response_dof == ['3']
    assert inputs.ordinate_unit == ['g', 'g'] and drives.ordinate_unit == ['V']
    assert inputs.source == drives.source == 'SIM'
    assert drives.roles == {('3', 'voltage'): 'reference'}
    raw = np.fromfile(os.path.join(folder, 'streams', 'inputs', 'telemetry.f32'),
                      dtype='<f4').reshape(-1, 2)
    assert np.array_equal(inputs.ordinate, (raw.T.astype(np.float64) * G).astype(np.float32))
    assert inputs.abscissa[1] == pytest.approx(1 / 1280)
    assert inputs.run_identity.run == _json('run.json')['run_id']


def test_the_specification_is_the_setups_profile(converted):
    _folder, _done, project = converted
    spec = project['Specification']
    assert isinstance(spec, Specification)
    profile = _json('setup.json')['resolved']['profile']['breakpoints']
    assert spec.response_dof == ['1']
    assert list(spec.abscissa) == [p['freq_hz'] for p in profile]
    assert np.allclose(spec.ordinate.real[0], [p['psd_g2_hz'] * G ** 2 for p in profile])
    levels = np.array([p['psd_g2_hz'] for p in profile]) * G ** 2
    assert np.allclose(spec.limits['abort_upper'], levels * 10 ** 0.6)


def test_the_spectra_transfer_function_and_coherence_are_there(converted):
    _folder, _done, project = converted
    for name, kind in (('control_psd', Psd), ('response_psd', Psd), ('drive_psd', Psd),
                       ('identification_frf', Frf), ('identification_coherence', Coherence)):
        assert isinstance(project[name], kind), name
    frf = project['identification_frf']
    assert frf.response_dof == ['1'] and frf.reference_dof == ['3']
    assert np.iscomplexobj(frf.ordinate) and np.all(np.isfinite(frf.ordinate))
    assert frf.ordinate_dim == ['acceleration/voltage']
    coherence = project['identification_coherence']
    assert np.all((coherence.ordinate >= 0) & (coherence.ordinate <= 1))
    with open(os.path.join(GOLDEN, 'results', 'control_psd.jsonl')) as f:
        records = len(f.readlines())
    assert project['response_psd'].ordinate.shape[0] == 2 * records
    assert project['control_psd'].block[-1] == f'record {records - 1}'


def test_the_run_record_holds_the_setup_the_log_and_the_outcome(converted):
    _folder, _done, project = converted
    record = project['Run Record']
    assert isinstance(record, RunRecord)
    assert record.setup == _json('setup.json')
    assert record.config['operator'] == 'Operator'
    with open(os.path.join(GOLDEN, 'log.jsonl')) as f:
        log = [json.loads(line) for line in f]
    assert record.log == log
    assert [e['seq'] for e in record.log] == list(range(len(log)))
    assert record.complete and record.outcome == 'completed' == _json('end.json')['end'][
        'outcome']
    assert record.source == 'SIM' and record.test == 'random'
    assert 'limit_ch2' in record.arrays and 'reference' in record.arrays
    assert record.missing == []


def test_save_and_reopen_loses_nothing(converted, tmp_path):
    _folder, _done, project = converted
    again = Project.open(project.save(tmp_path / 'again.hforge'))
    assert list(again) == list(project)
    for name in project:
        assert again[name] == project[name], name
    assert again.object_groups == project.object_groups


def _saved_by_the_guide():
    with open(os.path.join(ROOT, 'docs', 'run-folder.md'), encoding='utf-8') as f:
        text = f.read()
    section = text.split('## What a Random Run Saves', 1)[1].split('\n## ', 1)[0]
    return re.findall(r'^\| `([^`]+)` \|', section, flags=re.MULTILINE)


def test_the_guide_says_exactly_what_a_random_run_saves():
    named = _saved_by_the_guide()
    assert named, 'the guide lists nothing'
    run = run_folder.read(GOLDEN)
    on_disk = ([f'streams/{name}' for name in run.streams]
               + [f'results/{name}' for name in run.results if not name.startswith('limit_ch')]
               + ['end.json'])
    assert sorted(named) == sorted(on_disk)
    # and a run with a limit adds one result per limited channel, as the guide says
    assert [n for n in run.results if n.startswith('limit_ch')] == ['limit_ch2']
