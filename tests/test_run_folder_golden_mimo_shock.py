"""The bridge from a MIMO Shock run to the analysis side, on a folder the
control service wrote: the golden MIMO Shock run folder
(`schema/golden/runs/mimo_shock/`, written by the control service's own
test `mimo_shock_golden_run` on the simulator) converts with
`hertzforge.io.run_convert` into a project that `Project.open` opens,
with the channel table that was set, every enabled input and every drive
as time histories of one record per shot timed from its firing, an `Srs`
of every pulse at every control, each control's target SRS with its band
as a `ShockSpecification`, the identification's transfer function matrix
and multiple coherence, and the run record with the setup, the log in
order, every pulse's verdict, the drives it played, the law's calls and
the outcome; a waveform target converts to a `TransientSpecification`;
and `docs/run-folder.md` says exactly what a MIMO Shock run saves.

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
from hertzforge.core.data import (
    Frf,
    MultipleCoherence,
    ShockSpecification,
    Srs,
    TimeHistory,
    TransientSpecification,
)
from hertzforge.core.run_record import RunRecord
from hertzforge.io import run_convert, run_folder

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GOLDEN = os.path.join(ROOT, 'schema', 'golden', 'runs', 'mimo_shock')
G = 9.80665
CONTROLS = ['1', '2', '3', '4']


def _json(*parts):
    with open(os.path.join(GOLDEN, *parts), encoding='utf-8') as f:
        return json.load(f)


def _lines(*parts):
    with open(os.path.join(GOLDEN, *parts), encoding='utf-8') as f:
        return [json.loads(line) for line in f]


@pytest.fixture(scope='module')
def converted(tmp_path_factory):
    folder = str(tmp_path_factory.mktemp('runs') / 'mimo_shock')
    shutil.copytree(GOLDEN, folder)
    done = run_convert.convert(folder)
    return folder, done, Project.open(done.path)


def test_the_mimo_shock_folder_converts_and_opens_with_nothing_lacking(converted):
    _folder, done, project = converted
    assert done.lacked == []
    assert list(project) == done.objects
    assert project.project_type == 'Shock'
    assert project.name == 'Golden MIMO Shock'


def test_the_mimo_shock_channel_table_is_the_table_that_was_set(converted):
    _folder, _done, project = converted
    table = project['Channel Table']
    assert table.has_controller_settings
    assert list(table['channel']) == [1, 2, 3, 4, 5, 6, 7, 8]
    assert list(table['control']) == ['True'] * 4 + ['False'] * 4
    assert list(table['drive']) == ['False'] * 4 + ['True'] * 4
    rows = _json('table.json')['rows']
    back = run_convert.rows_from_channel_table(table)
    for given, read in zip(rows, back, strict=True):
        assert {k: v for k, v in read.items() if k not in run_convert.ANALYSIS_MEMBERS} == given


@pytest.mark.parametrize('stream, unit, scale', [('inputs', 'g', G), ('drives', 'V', 1.0)])
def test_each_mimo_shock_shot_is_a_record_of_its_length_unit_and_source(converted, stream,
                                                                        unit, scale):
    folder, _done, project = converted
    side = _json('streams', stream, 'stream.json')
    width = len(side['channels'])
    assert width == 4
    raw = np.fromfile(os.path.join(folder, 'streams', stream, 'telemetry.f32'),
                      dtype='<f4').reshape(-1, width)
    shots = side['shots']
    pulses = [s for s in shots if s['what'] == 'pulse']
    identification = [s for s in shots if s['what'] == 'identification']
    assert len(pulses) == len(_lines('results', 'pulses.jsonl')) == 2
    assert [s['step'] for s in identification] == ['probe', 'estimate']
    assert len(shots) == 4
    # the golden folder keeps each shot's first frames and says how many it had
    assert side['cut_from_frames'] > side['frames']
    assert all(s['cut_from_frames'] >= s['frames'] for s in shots)
    history = project[f'{stream} pulse']
    assert isinstance(history, TimeHistory)
    frames = pulses[0]['frames']
    assert history.ordinate.shape == (width * len(pulses), frames)
    assert history.ordinate_unit == [unit] * (width * len(pulses))
    assert history.source == 'SIM'
    assert history.block == [f'pulse {s["pulse"] + 1}' for _c in range(width) for s in pulses]
    # every record follows the quiet frames before it: its firing is time zero
    rate = side['fs_hz']
    assert history.abscissa[0] == pytest.approx(-pulses[0]['pre_frames'] / rate)
    assert history.abscissa[1] - history.abscissa[0] == pytest.approx(1 / rate)
    for k, shot in enumerate(pulses):
        span = raw[shot['first_frame']:shot['first_frame'] + shot['frames']]
        for c in range(width):
            want = (span[:, c].astype(np.float64) * scale).astype(np.float32)
            assert np.array_equal(history.ordinate[c * len(pulses) + k], want), (k, c)
    ident = project[f'{stream} identification']
    assert ident.block == ['probe', 'estimate'] * width


def test_the_srs_of_every_pulse_at_every_control_and_the_targets_are_there(converted):
    folder, _done, project = converted
    run = run_folder.read(folder)
    pulses = _lines('results', 'pulses.jsonl')
    srs = project['pulse_srs']
    assert isinstance(srs, Srs) and srs.kind == 'maximax'
    assert srs.ordinate.shape[0] == len(CONTROLS) * len(pulses)
    assert srs.response_dof == [c for c in CONTROLS for _p in pulses]
    assert srs.block == [f'pulse {p["index"] + 1}' for _c in CONTROLS for p in pulses]
    measured = run.results['pulse_srs'].arrays['values']
    assert measured.shape[:2] == (len(pulses), len(CONTROLS))
    for c in range(len(CONTROLS)):
        assert np.allclose(srs.ordinate[c * len(pulses):(c + 1) * len(pulses)],
                           measured[:, c, :] * G)
    spec = project['srs_reference']
    assert isinstance(spec, ShockSpecification)
    resolved = _json('setup.json')['resolved']['target']
    assert spec.q == resolved['bands']['q'] == 10
    assert np.allclose(spec.abscissa, resolved['natural_hz'])
    reference = run.results['srs_reference'].arrays
    assert np.allclose(spec.ordinate, reference['reference'] * G)
    assert spec.response_dof == CONTROLS
    for array, limit in (('band_low', 'warning_lower'), ('band_high', 'warning_upper')):
        assert np.allclose(spec.limits[limit], reference[array] * G), limit
    assert np.all(spec.limits['warning_upper'] > spec.ordinate)
    frf = project['identification_frf']
    assert isinstance(frf, Frf)
    assert np.iscomplexobj(frf.ordinate) and np.any(frf.ordinate.imag != 0)
    assert frf.ordinate.shape[0] == 16
    assert frf.response_dof == [c for c in CONTROLS for _d in range(4)]
    assert frf.reference_dof == ['5', '6', '7', '8'] * 4
    values = run.results['identification_frf'].arrays['values']
    assert np.allclose(frf.ordinate[1], values[0, 1] * G)
    coherence = project['identification_coherence']
    assert isinstance(coherence, MultipleCoherence)
    assert coherence.response_dof == CONTROLS
    assert np.all((coherence.ordinate > 0.9) & (coherence.ordinate <= 1 + 1e-9))


def test_the_mimo_shock_run_record_holds_the_setup_the_log_and_the_outcome(converted):
    _folder, _done, project = converted
    record = project['Run Record']
    assert isinstance(record, RunRecord)
    assert record.setup == _json('setup.json')
    assert record.resolved['target']['kind'] == 'srs'
    assert record.config['operator'] == 'Operator'
    log = _lines('log.jsonl')
    assert record.log == log
    assert [e['seq'] for e in record.log] == list(range(len(log)))
    fired = [e for e in record.log if e['kind'] == 'pulse']
    assert [e['data']['pulse'] for e in fired] == [0, 1]
    pulses = _lines('results', 'pulses.jsonl')
    assert [e['data']['passed'] for e in fired] == [p['passed'] for p in pulses]
    assert record.log[-1]['data']['outcome'] == 'completed'
    held = record.arrays['pulses']
    assert held['control_passed'].shape == (len(pulses), 4)
    assert list(held['passed'][:, 0]) == [1.0 if p['passed'] else 0.0 for p in pulses]
    assert np.all(held['peak'] > 0) and held['drive_peak'].shape == (len(pulses), 4)
    assert record.results['pulses']['records'] == pulses
    # the drives each pulse played, the law's calls, the identification's facts,
    # the seed and the synthesized waveforms have no analysis class: arrays
    drives = record.arrays['pulse_drives']['values']
    assert drives.shape == (len(pulses), 4, 64) and np.max(np.abs(drives)) > 0
    calls = _lines('results', 'law_calls.jsonl')
    assert [c['call'] for c in calls] == ['seed', 'iterate']
    assert record.arrays['law_calls']['drives'].shape == (len(calls), 4, 64)
    assert set(record.arrays['identification']) == {
        'drive_rms', 'drive_peak', 'mean_coherence', 'coverage', 'response_peak'}
    assert record.arrays['seed']['reachable'].shape == (4,)
    assert record.arrays['synthesis']['waveform'].shape == (4, 64)
    assert set(record.arrays['pulse_srs']) == {'reference', 'band_low', 'band_high'}
    assert record.complete and record.outcome == 'completed' == _json('end.json')['end'][
        'outcome']
    assert record.source == 'SIM' and record.test == 'mimo_shock'
    assert record.missing == []


def test_a_mimo_shock_waveform_target_is_a_transient_specification(tmp_path):
    folder = str(tmp_path / 'mimo_shock')
    shutil.copytree(GOLDEN, folder)
    # the golden run is an SRS run: give its folder a waveform target as a
    # waveform run writes it, every control's reference over the record
    waveform = run_folder.read(folder).results['synthesis'].arrays['waveform']
    fs = _json('run.json')['grid']['fs_hz']
    side = {'name': 'target', 'kind': 'transient_reference',
            'abscissa': {'kind': 'time', 'unit': 's', 'array': 'time'},
            'channels': {'responses': [1, 2, 3, 4]},
            'description': 'each control\'s reference waveform at full level',
            'arrays': [{'name': 'time', 'dtype': 'f8', 'shape': [64], 'unit': 's',
                        'offset': 0},
                       {'name': 'reference', 'dtype': 'f8', 'shape': [4, 64], 'unit': 'g',
                        'offset': 64 * 8}]}
    with open(os.path.join(folder, 'results', 'target.json'), 'w', encoding='utf-8') as f:
        json.dump(side, f)
    with open(os.path.join(folder, 'results', 'target.bin'), 'wb') as f:
        f.write((np.arange(64) / fs).astype('<f8').tobytes())
        f.write(waveform.astype('<f8').tobytes())
    manifest = _json('run.json')
    manifest['results'].append({'name': 'target', 'kind': 'transient_reference'})
    with open(os.path.join(folder, 'run.json'), 'w', encoding='utf-8') as f:
        json.dump(manifest, f)
    done = run_convert.convert(folder)
    spec = Project.open(done.path)['Specification']
    assert isinstance(spec, TransientSpecification)
    assert spec.response_dof == CONTROLS
    assert np.allclose(spec.ordinate, waveform * G)
    assert spec.abscissa[1] == pytest.approx(1 / fs)


def test_save_and_reopen_a_mimo_shock_project_loses_nothing(converted, tmp_path):
    _folder, _done, project = converted
    again = Project.open(project.save(tmp_path / 'again.hforge'))
    assert list(again) == list(project)
    for name in project:
        assert again[name] == project[name], name
    assert again.object_groups == project.object_groups


def _saved_by_the_guide():
    with open(os.path.join(ROOT, 'docs', 'run-folder.md'), encoding='utf-8') as f:
        text = f.read()
    section = text.split('## What a MIMO Shock Run Saves', 1)[1].split('\n## ', 1)[0]
    return re.findall(r'^\| `([^`]+)` \|', section, flags=re.MULTILINE)


def test_the_guide_says_exactly_what_a_mimo_shock_run_saves():
    named = _saved_by_the_guide()
    assert named, 'the guide lists nothing'
    run = run_folder.read(GOLDEN)
    on_disk = ([f'streams/{name}' for name in run.streams]
               + [f'results/{name}' for name in run.results] + ['end.json'])
    assert sorted(named) == sorted(on_disk)
