"""The bridge from a Sine run to the analysis side, on a folder the
control service wrote: the golden Sine run folder
(`schema/golden/runs/sine/`, written by the control service's own test
`sine_golden_run` on the simulator) converts with
`hertzforge.io.run_convert` into a project that `Project.open` opens,
with the channel table that was set, every enabled input and the drive
as time histories, the sine specification the run was controlled to,
the inputs' levels, the identification's transfer function and
coherence, and the run record; and `docs/run-folder.md` says exactly
what a Sine run saves.

The golden folder's streams are cut to their first frames, and say so
(`cut_from_frames`): the identification's burst alone is 131 072 frames,
which no small golden folder holds."""

from __future__ import annotations

import json
import os
import re
import shutil

import numpy as np
import pytest

from hertzforge import Project
from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.data import Coherence, Frf, TimeHistory
from hertzforge.core.run_record import RunRecord
from hertzforge.core.sine import SineLevelSet, SineSweepSpecification
from hertzforge.io import run_convert, run_folder

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GOLDEN = os.path.join(ROOT, 'schema', 'golden', 'runs', 'sine')
G = 9.80665


def _json(*parts):
    with open(os.path.join(GOLDEN, *parts), encoding='utf-8') as f:
        return json.load(f)


@pytest.fixture(scope='module')
def converted(tmp_path_factory):
    folder = str(tmp_path_factory.mktemp('runs') / 'sine')
    shutil.copytree(GOLDEN, folder)
    done = run_convert.convert(folder)
    return folder, done, Project.open(done.path)


def test_the_folder_converts_and_opens_with_nothing_lacking(converted):
    _folder, done, project = converted
    assert done.lacked == []
    assert list(project) == done.objects
    assert project.project_type == 'Sine Sweep'
    assert project.name == 'Golden Sine'


def test_the_channel_table_is_the_table_that_was_set(converted):
    _folder, _done, project = converted
    table = project['Channel Table']
    assert isinstance(table, ChannelTable) and table.has_controller_settings
    assert list(table['channel']) == [1, 2, 3]
    assert list(table['comment']) == ['Control', 'Monitor', 'Drive']
    assert list(table['control']) == ['True', 'False', 'False']
    assert list(table['drive']) == ['False', 'False', 'True']
    assert list(table['unit']) == ['g', 'g', 'V']
    rows = _json('table.json')['rows']
    back = run_convert.rows_from_channel_table(table)
    for given, read in zip(rows, back, strict=True):
        assert {k: v for k, v in read.items() if k != 'extra'} == given


def test_every_enabled_input_and_the_drive_is_a_time_history(converted):
    folder, _done, project = converted
    inputs, drives = project['inputs'], project['drives']
    assert isinstance(inputs, TimeHistory) and isinstance(drives, TimeHistory)
    side = _json('streams', 'inputs', 'stream.json')
    frames = side['frames']
    # the golden folder's cut, stated, and the length it states
    assert 0 < frames < side['cut_from_frames']
    assert inputs.ordinate.shape == (2, frames)
    assert drives.ordinate.shape == (1, _json('streams', 'drives', 'stream.json')['frames'])
    assert inputs.response_dof == ['1', '2'] and drives.response_dof == ['3']
    assert inputs.ordinate_unit == ['g', 'g'] and drives.ordinate_unit == ['V']
    assert inputs.source == drives.source == 'SIM'
    raw = np.fromfile(os.path.join(folder, 'streams', 'inputs', 'telemetry.f32'),
                      dtype='<f4').reshape(-1, 2)
    assert np.array_equal(inputs.ordinate, (raw.T.astype(np.float64) * G).astype(np.float32))
    assert inputs.abscissa[1] == pytest.approx(1 / 4096)
    assert inputs.run_identity.run == _json('run.json')['run_id']


def test_the_specification_is_the_setups_profile(converted):
    _folder, _done, project = converted
    spec = project['Specification']
    assert isinstance(spec, SineSweepSpecification)
    assert spec.response_dof == ['1']
    (tone,) = spec.tones
    resolved = _json('setup.json')['resolved']
    test = resolved['test']
    # one pass up the band, from the band's low edge, at the run's log rate
    assert tone.frequency[0] == test['band_hz'][0] and tone.frequency[-1] == test['band_hz'][1]
    assert np.all(tone.segment_type == 1)
    assert np.all(tone.segment_rate == test['rate']['oct_per_min'])
    # the profile: the target at every point the service resolved it at, and at
    # the breakpoints' own frequencies as the setup gives them
    ref = run_folder.read(GOLDEN).results['reference'].arrays
    assert np.allclose(tone.target(ref['frequency'])[:, 0], ref['reference'] * G)
    points = resolved['profile']['breakpoints']
    lo, hi = points[0], points[-1]
    slope = np.log(hi['accel_g_pk'] / lo['accel_g_pk']) / np.log(hi['freq_hz'] / lo['freq_hz'])
    inside = np.linspace(test['band_hz'][0], test['band_hz'][1], 7)
    profile = lo['accel_g_pk'] * (inside / lo['freq_hz']) ** slope * G
    assert np.allclose(tone.target(inside)[:, 0], profile, rtol=1e-4)
    # the bands, the setup's dB about the target
    assert np.allclose(tone.limits['abort_upper'][:, 0],
                       tone.amplitude[:, 0] * 10 ** (points[0]['abort_db'][1] / 20))
    assert np.allclose(tone.limits['warning_lower'][:, 0],
                       tone.amplitude[:, 0] * 10 ** (-points[0]['alarm_db'][0] / 20))


def test_the_levels_and_the_transfer_function_are_there(converted):
    _folder, _done, project = converted
    levels = project['Sine Levels']
    assert isinstance(levels, SineLevelSet) and levels.tone_names == ['Tone 1']
    (level,) = levels
    assert level.response_dof == ['1', '2']
    assert level.ordinate_unit == ['g', 'g'] and level.source == 'SIM'
    assert np.iscomplexobj(level.ordinate) and np.all(np.isfinite(level.ordinate))
    assert np.all(np.diff(level.abscissa) >= 0)
    assert level.seconds is not None and len(level.seconds) == len(level.abscissa)
    # the last pass at full level: the down sweep, the drive ramping up to the new
    # level as it starts (the level change came at the top of the up sweep), then
    # the control channel holding the profile
    spec = project['Specification']
    raw = run_folder.read(GOLDEN).results['channel_levels'].arrays
    top = np.nanmax(raw['level_db'])
    last = np.nanmax(raw['pass'][raw['level_db'] == top])
    moving = (raw['pass'] == last) & (raw['direction'] != 0)
    assert len(level.abscissa) == int(moving.sum())
    assert np.all(raw['direction'][moving] == -1)
    late = np.argsort(level.seconds)[len(level.seconds) // 2:]
    wanted = level.target_on(spec.tone('Tone 1'))[late, 0]
    got = np.abs(level.ordinate[0][late])
    assert np.all(np.abs(20 * np.log10(got / wanted)) < 1.0)
    frf = project['identification_frf']
    assert isinstance(frf, Frf)
    assert frf.response_dof == ['1'] and frf.reference_dof == ['3']
    assert np.iscomplexobj(frf.ordinate) and np.all(np.isfinite(frf.ordinate))
    assert frf.ordinate_dim == ['acceleration/voltage']
    coherence = project['identification_coherence']
    assert isinstance(coherence, Coherence)
    assert np.all((coherence.ordinate >= 0) & (coherence.ordinate <= 1))


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
    assert record.source == 'SIM' and record.test == 'sine'
    for name in ('reference', 'limit_ch2', 'channel_levels', 'control_trace'):
        assert name in record.arrays, name
    trace = record.arrays['control_trace']
    assert trace['target'].shape[-1] == trace['measured'].shape[-1] == trace['applied'].shape[-1]
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
    section = text.split('## What a Sine Run Saves', 1)[1].split('\n## ', 1)[0]
    return re.findall(r'^\| `([^`]+)` \|', section, flags=re.MULTILINE)


def test_the_guide_says_exactly_what_a_sine_run_saves():
    named = _saved_by_the_guide()
    assert named, 'the guide lists nothing'
    run = run_folder.read(GOLDEN)
    on_disk = ([f'streams/{name}' for name in run.streams]
               + [f'results/{name}' for name in run.results if not name.startswith('limit_ch')]
               + ['end.json'])
    assert sorted(named) == sorted(on_disk)
    assert [n for n in run.results if n.startswith('limit_ch')] == ['limit_ch2']
