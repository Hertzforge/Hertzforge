"""The bridge from a MIMO Mixed Mode run to the analysis side, on a folder the
control service wrote: the golden MIMO Mixed Mode run folder
(`schema/golden/runs/mimo_mixed/`, written by the control service's own test
`mimo_mixed_golden_run` on the simulator) converts with
`hertzforge.io.run_convert` into a project that `Project.open` opens, with
the channel table that was set, every enabled input and every drive as time
histories, the floor's reference spectral density matrix as a specification
with its cross terms, the identification's transfer function matrix,
coherence and spectra, and the run record with the setup, the log in order,
both laws and their calls, each part's history and the outcome; a run killed
in mid-test converts with what it has; and `docs/run-folder.md` says exactly
what a MIMO Mixed Mode run saves.

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
from hertzforge.core.data import Frf, MultipleCoherence, Psd, Specification, TimeHistory
from hertzforge.core.run_record import RunRecord
from hertzforge.io import run_convert, run_folder

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GOLDEN = os.path.join(ROOT, 'schema', 'golden', 'runs', 'mimo_mixed')
G = 9.80665
CONTROLS = ['1', '2', '3', '4']
DRIVES = ['5', '6', '7', '8']


def _json(*parts):
    with open(os.path.join(GOLDEN, *parts), encoding='utf-8') as f:
        return json.load(f)


@pytest.fixture(scope='module')
def converted(tmp_path_factory):
    folder = str(tmp_path_factory.mktemp('runs') / 'mimo_mixed')
    shutil.copytree(GOLDEN, folder)
    done = run_convert.convert(folder)
    return folder, done, Project.open(done.path)


@pytest.fixture(scope='module')
def run():
    return run_folder.read(GOLDEN)


def test_the_mimo_mixed_folder_converts_and_opens_with_nothing_lacking(converted):
    _folder, done, project = converted
    assert done.lacked == []
    assert list(project) == done.objects
    assert project.project_type == 'Random Vibration'
    assert project.name == 'Golden MIMO Mixed Mode'


def test_the_mimo_mixed_channel_table_is_the_table_that_was_set(converted):
    _folder, _done, project = converted
    table = project['Channel Table']
    assert list(table['channel']) == [1, 2, 3, 4, 5, 6, 7, 8]
    assert list(table['control']) == ['True'] * 4 + ['False'] * 4
    assert list(table['drive']) == ['False'] * 4 + ['True'] * 4
    rows = _json('table.json')['rows']
    back = run_convert.rows_from_channel_table(table)
    assert [r['index'] for r in back] == [r['index'] for r in rows]
    assert _json('run.json')['channel_table']['revision'] == _json('table.json')['revision']


def test_every_mimo_mixed_input_and_drive_is_a_time_history(converted):
    _folder, _done, project = converted
    inputs, drives = project['inputs'], project['drives']
    assert isinstance(inputs, TimeHistory) and isinstance(drives, TimeHistory)
    side = _json('streams', 'inputs', 'stream.json')
    assert 0 < side['frames'] < side['cut_from_frames']
    assert inputs.ordinate.shape == (4, side['frames'])
    assert inputs.response_dof == CONTROLS and drives.response_dof == DRIVES
    assert inputs.ordinate_unit == ['g'] * 4 and drives.ordinate_unit == ['V'] * 4
    assert inputs.source == drives.source == 'SIM'
    assert inputs.abscissa[1] == pytest.approx(1 / 4096)


def test_the_floor_reference_is_a_specification_with_its_cross_terms(converted, run):
    _folder, _done, project = converted
    spec = project['Specification']
    assert isinstance(spec, Specification)
    values = run.results['reference'].arrays['values']
    n = values.shape[0]
    pairs = [(a, b) for a in range(n) for b in range(a, n)]
    assert list(zip(spec.response_dof, spec.reference_dof, strict=True)) == [
        (str(a + 1), str(b + 1)) for a, b in pairs]
    for k, (a, b) in enumerate(pairs):
        assert np.allclose(spec.ordinate[k], values[a, b] * G * G)
    # the floor's band, as the configuration gave it
    band = _json('setup.json')['config']['reference']['freqs_hz']
    assert spec.abscissa[0] >= band[0] and spec.abscissa[-1] <= band[1]


def test_the_mimo_mixed_identification_is_there_as_analysis_objects(converted, run):
    _folder, _done, project = converted
    frf = project['identification_frf']
    assert isinstance(frf, Frf)
    assert frf.response_dof == [c for c in CONTROLS for _ in DRIVES]
    assert frf.reference_dof == DRIVES * 4
    given = run.results['identification_frf'].arrays['values']
    assert np.allclose(frf.ordinate[1], given[0, 1] * G)
    coherence = project['identification_coherence']
    assert isinstance(coherence, MultipleCoherence)
    assert coherence.response_dof == CONTROLS
    assert np.all((coherence.ordinate > 0.5) & (coherence.ordinate <= 1 + 1e-9))
    for name in ('identification_response', 'identification_drive'):
        assert isinstance(project[name], Psd), name


def test_the_mimo_mixed_run_record_holds_the_setup_the_log_the_laws_and_the_outcome(
        converted, run):
    _folder, _done, project = converted
    record = project['Run Record']
    assert isinstance(record, RunRecord)
    assert record.setup == _json('setup.json')
    with open(os.path.join(GOLDEN, 'log.jsonl'), encoding='utf-8') as f:
        log = [json.loads(line) for line in f]
    assert record.log == log
    assert [e['seq'] for e in record.log] == list(range(len(log)))
    assert record.complete and record.outcome == 'completed'
    assert record.source == 'SIM' and record.test == 'mimo_mixed'
    assert record.missing == []
    # both laws, each in its role and from its own world
    assert record.law['floor']['law']['id'] == 'pinv_closed'
    assert record.law['floor']['law']['world'] == 'mimo'
    assert record.law['tones']['law']['id'] == 'matrix'
    assert record.law['tones']['law']['world'] == 'sine_mimo'
    assert record.law['calls']['format'] == 'hertzforge.mimo_mixed.law_ticks.v1'
    assert record.law['calls']['result'] == 'control_trace'
    for name in ('control_spectra', 'drive_spectra', 'tone_levels', 'band_levels',
                 'drive_levels', 'control_trace', 'identification'):
        assert name in record.arrays, name
    # the law calls: the floor's once a loop, the tone's once a tick, each law's
    # in time order (a loop's floor call is taken after the tone ticks it spans)
    trace = record.arrays['control_trace']
    filled = np.isfinite(trace['role'])
    roles, times = trace['role'][filled], trace['time'][filled]
    assert set(roles) == {0.0, 1.0}
    for role in (0.0, 1.0):
        assert np.all(np.diff(times[roles == role]) > 0), role
    # the golden folder's call times are zeroed; every call was usable
    assert np.all(trace['call_us'][filled] == 0)
    assert np.all(trace['miss'][filled] == 0)
    end = _json('end.json')
    assert end['end']['outcome'] == 'completed'


def test_a_mimo_mixed_run_killed_in_mid_test_converts_with_what_it_has(tmp_path):
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


def test_save_and_reopen_a_mimo_mixed_project_loses_nothing(converted, tmp_path):
    _folder, _done, project = converted
    again = Project.open(project.save(tmp_path / 'again.hforge'))
    assert list(again) == list(project)
    for name in project:
        assert again[name] == project[name], name


def _saved_by_the_guide():
    with open(os.path.join(ROOT, 'docs', 'run-folder.md'), encoding='utf-8') as f:
        text = f.read()
    section = text.split('## What a MIMO Mixed Mode Run Saves', 1)[1].split('\n## ', 1)[0]
    return re.findall(r'^\| `([^`]+)` \|', section, flags=re.MULTILINE)


def test_the_guide_says_exactly_what_a_mimo_mixed_run_saves():
    named = _saved_by_the_guide()
    assert named, 'the guide lists nothing'
    run = run_folder.read(GOLDEN)
    on_disk = ([f'streams/{name}' for name in run.streams]
               + [f'results/{name}' for name in run.results]
               + ['end.json'])
    assert sorted(named) == sorted(on_disk)
