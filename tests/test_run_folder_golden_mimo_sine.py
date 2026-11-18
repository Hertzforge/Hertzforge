"""The bridge from a MIMO Sine run to the analysis side, on a folder the
control service wrote: the golden MIMO Sine run folder
(`schema/golden/runs/mimo_sine/`, written by the control service's own test
`mimo_sine_golden_run` on the simulator) converts with
`hertzforge.io.run_convert` into a project that `Project.open` opens, with
the channel table that was set, every enabled input and every drive as time
histories, the references as a `SineSweepSpecification` with each control's
own amplitude, phase and bands, every control's, limit channel's and drive's
tracked amplitude and phase as a `SineLevelSet`, the identification's
transfer function matrix and multiple coherence, and the run record with the
setup, the log in order, every law call and the outcome; a run killed in
mid-test converts with what it has; and `docs/run-folder.md` says exactly
what a MIMO Sine run saves.

The golden folder's streams are cut to their first frames, and say so
(`cut_from_frames`). The run is simulated."""

from __future__ import annotations

import json
import os
import re
import shutil

import numpy as np
import pytest

from hertzforge import Project
from hertzforge.core.data import Frf, MultipleCoherence, TimeHistory
from hertzforge.core.run_record import RunRecord
from hertzforge.core.sine import SineLevelSet, SineSweepSpecification
from hertzforge.io import run_convert, run_folder

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GOLDEN = os.path.join(ROOT, 'schema', 'golden', 'runs', 'mimo_sine')
G = 9.80665
CONTROLS = ['1', '2', '3', '4']
DRIVES = ['5', '6', '7', '8']


def _json(*parts):
    with open(os.path.join(GOLDEN, *parts), encoding='utf-8') as f:
        return json.load(f)


@pytest.fixture(scope='module')
def converted(tmp_path_factory):
    folder = str(tmp_path_factory.mktemp('runs') / 'mimo_sine')
    shutil.copytree(GOLDEN, folder)
    done = run_convert.convert(folder)
    return folder, done, Project.open(done.path)


def test_the_mimo_sine_folder_converts_and_opens_with_nothing_lacking(converted):
    _folder, done, project = converted
    assert done.lacked == []
    assert list(project) == done.objects
    assert project.project_type == 'Sine Sweep'
    assert project.name == 'Golden MIMO Sine'


def test_the_mimo_sine_channel_table_is_the_table_that_was_set(converted):
    _folder, _done, project = converted
    table = project['Channel Table']
    assert list(table['channel']) == [1, 2, 3, 4, 5, 6, 7, 8]
    assert list(table['control']) == ['True'] * 4 + ['False'] * 4
    assert list(table['drive']) == ['False'] * 4 + ['True'] * 4
    rows = _json('table.json')['rows']
    back = run_convert.rows_from_channel_table(table)
    assert [r['index'] for r in back] == [r['index'] for r in rows]


def test_every_mimo_sine_input_and_drive_is_a_time_history(converted):
    _folder, _done, project = converted
    inputs, drives = project['inputs'], project['drives']
    assert isinstance(inputs, TimeHistory) and isinstance(drives, TimeHistory)
    side = _json('streams', 'inputs', 'stream.json')
    assert 0 < side['frames'] < side['cut_from_frames']
    assert inputs.ordinate.shape == (4, side['frames'])
    assert inputs.response_dof == CONTROLS and drives.response_dof == DRIVES
    assert inputs.ordinate_unit == ['g'] * 4 and drives.ordinate_unit == ['V'] * 4
    assert inputs.source == drives.source == 'SIM'


def test_the_mimo_sine_specification_holds_each_controls_reference(converted):
    _folder, _done, project = converted
    spec = project['Specification']
    assert isinstance(spec, SineSweepSpecification)
    assert spec.response_dof == CONTROLS
    (tone,) = spec.tones
    test = _json('setup.json')['resolved']['test']
    assert tone.frequency[0] == test['band_hz'][0] and tone.frequency[-1] == test['band_hz'][1]
    assert np.all(tone.segment_rate == test['rate']['oct_per_min'])
    ref = run_folder.read(GOLDEN).results['reference'].arrays
    # one column per control, each its own reference, in SI
    assert tone.amplitude.shape == (len(ref['frequency']), 4)
    assert np.allclose(tone.amplitude, ref['reference'].T * G)
    assert np.allclose(tone.phase, np.deg2rad(ref['phase'].T))
    assert np.allclose(tone.limits['abort_upper'], ref['abort_high'].T * G)


def test_the_mimo_sine_levels_identification_and_coherence_are_there(converted):
    _folder, _done, project = converted
    levels = project['Sine Levels']
    assert isinstance(levels, SineLevelSet)
    (level,) = levels
    assert level.response_dof == CONTROLS + DRIVES
    assert level.ordinate_unit == ['g'] * 4 + ['V'] * 4 and level.source == 'SIM'
    assert np.iscomplexobj(level.ordinate) and np.all(np.isfinite(level.ordinate))
    frf = project['identification_frf']
    assert isinstance(frf, Frf)
    # every control's response to every drive
    assert frf.response_dof == [c for c in CONTROLS for _ in DRIVES]
    assert frf.reference_dof == DRIVES * 4
    assert np.all(np.isfinite(frf.ordinate))
    coherence = project['identification_coherence']
    assert isinstance(coherence, MultipleCoherence)
    assert coherence.response_dof == CONTROLS
    assert np.all((coherence.ordinate >= 0) & (coherence.ordinate <= 1 + 1e-9))


def test_the_mimo_sine_run_record_holds_the_setup_the_log_and_the_outcome(converted):
    _folder, _done, project = converted
    record = project['Run Record']
    assert isinstance(record, RunRecord)
    assert record.setup == _json('setup.json')
    with open(os.path.join(GOLDEN, 'log.jsonl')) as f:
        log = [json.loads(line) for line in f]
    assert record.log == log
    assert record.complete and record.outcome == 'completed'
    assert record.source == 'SIM' and record.test == 'mimo_sine'
    for name in ('reference', 'channel_levels', 'control_trace', 'identification_trust'):
        assert name in record.arrays, name
    trace = record.arrays['control_trace']
    assert trace['drive_asked_re'].shape[-2:] == (4, trace['time'].shape[-1])
    end = _json('end.json')
    assert end['report']['controls'][0]['channel'] == 1
    assert record.missing == []


def test_a_mimo_sine_run_killed_in_mid_test_converts_with_what_it_has(tmp_path):
    folder = str(tmp_path / 'killed')
    shutil.copytree(GOLDEN, folder)
    # killed: no end, the manifest never marked complete
    os.remove(os.path.join(folder, 'end.json'))
    with open(os.path.join(folder, 'run.json'), encoding='utf-8') as f:
        manifest = json.load(f)
    manifest['complete'] = False
    with open(os.path.join(folder, 'run.json'), 'w', encoding='utf-8') as f:
        json.dump(manifest, f)
    done = run_convert.convert(folder)
    project = Project.open(done.path)
    record = project['Run Record']
    assert not record.complete
    assert 'Specification' in project and 'Channel Table' in project


def test_save_and_reopen_a_mimo_sine_project_loses_nothing(converted, tmp_path):
    _folder, _done, project = converted
    again = Project.open(project.save(tmp_path / 'again.hforge'))
    assert list(again) == list(project)
    for name in project:
        assert again[name] == project[name], name


def _saved_by_the_guide():
    with open(os.path.join(ROOT, 'docs', 'run-folder.md'), encoding='utf-8') as f:
        text = f.read()
    section = text.split('## What a MIMO Sine Run Saves', 1)[1].split('\n## ', 1)[0]
    return re.findall(r'^\| `([^`]+)` \|', section, flags=re.MULTILINE)


def test_the_guide_says_exactly_what_a_mimo_sine_run_saves():
    named = _saved_by_the_guide()
    assert named, 'the guide lists nothing'
    run = run_folder.read(GOLDEN)
    on_disk = ([f'streams/{name}' for name in run.streams]
               + [f'results/{name}' for name in run.results if not name.startswith('limit_ch')]
               + ['end.json'])
    assert sorted(named) == sorted(on_disk + ['results/limit_ch<n>'])
