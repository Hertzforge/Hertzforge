"""The bridge from a Classical Shock run to the analysis side, on a folder
the control service wrote: the golden Classical Shock run folder
(`schema/golden/runs/classical_shock/`, written by the control service's
own test `classical_shock_golden_run` on the simulator) converts with
`hertzforge.io.run_convert` into a project that `Project.open` opens,
with the channel table that was set, every enabled input and the drive as
time histories of one record per shot timed from its firing, an `Srs` for
every pulse, the reference SRS and its band as a `ShockSpecification`,
the identification's transfer function and coherence, and the run record
with the setup, the log, every pulse's verdict and the outcome; and
`docs/run-folder.md` says exactly what a Classical Shock run saves."""

from __future__ import annotations

import json
import os
import re
import shutil

import numpy as np
import pytest

from hertzforge import Project
from hertzforge.core.data import Coherence, Frf, ShockSpecification, Srs, TimeHistory
from hertzforge.core.run_record import RunRecord
from hertzforge.io import run_convert, run_folder

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GOLDEN = os.path.join(ROOT, 'schema', 'golden', 'runs', 'classical_shock')
G = 9.80665


def _json(*parts):
    with open(os.path.join(GOLDEN, *parts), encoding='utf-8') as f:
        return json.load(f)


def _lines(*parts):
    with open(os.path.join(GOLDEN, *parts), encoding='utf-8') as f:
        return [json.loads(line) for line in f]


@pytest.fixture(scope='module')
def converted(tmp_path_factory):
    folder = str(tmp_path_factory.mktemp('runs') / 'classical_shock')
    shutil.copytree(GOLDEN, folder)
    done = run_convert.convert(folder)
    return folder, done, Project.open(done.path)


def _pulses():
    return [s for s in _json('streams', 'inputs', 'stream.json')['shots']
            if s['what'] == 'pulse']


def test_the_folder_converts_and_opens_with_nothing_lacking(converted):
    _folder, done, project = converted
    assert done.lacked == []
    assert list(project) == done.objects
    assert project.project_type == 'Shock'
    assert project.name == 'Golden Classical Shock'


def test_the_channel_table_is_the_table_that_was_set(converted):
    _folder, _done, project = converted
    table = project['Channel Table']
    assert table.has_controller_settings
    assert list(table['channel']) == [1, 2, 3]
    assert list(table['comment']) == ['Control', 'Monitor', 'Drive']
    assert list(table['control']) == ['True', 'False', 'False']
    assert list(table['drive']) == ['False', 'False', 'True']
    rows = _json('table.json')['rows']
    back = run_convert.rows_from_channel_table(table)
    for given, read in zip(rows, back, strict=True):
        assert {k: v for k, v in read.items() if k != 'extra'} == given


@pytest.mark.parametrize('stream, width, unit, scale', [
    ('inputs', 2, 'g', G), ('drives', 1, 'V', 1.0)])
def test_each_shot_is_a_record_of_its_length_unit_and_source(converted, stream, width,
                                                             unit, scale):
    folder, _done, project = converted
    side = _json('streams', stream, 'stream.json')
    raw = np.fromfile(os.path.join(folder, 'streams', stream, 'telemetry.f32'),
                      dtype='<f4').reshape(-1, width)
    pulses = [s for s in side['shots'] if s['what'] == 'pulse']
    identification = [s for s in side['shots'] if s['what'] == 'identification']
    assert len(pulses) == len(_lines('results', 'pulses.jsonl')) == 2
    assert len(identification) == 1
    history = project[f'{stream} pulse']
    assert isinstance(history, TimeHistory)
    frames = pulses[0]['frames']
    assert history.ordinate.shape == (width * len(pulses), frames)
    assert history.ordinate_unit == [unit] * (width * len(pulses))
    assert history.source == 'SIM'
    assert history.block == [f'pulse {s["pulse"] + 1}' for _c in range(width) for s in pulses]
    # timed from the firing: the first sample is the main pulse's start before it
    assert history.abscissa[0] == pytest.approx(-pulses[0]['pre_frames'] / side['fs_hz'])
    assert history.abscissa[1] - history.abscissa[0] == pytest.approx(1 / side['fs_hz'])
    for k, shot in enumerate(pulses):
        span = raw[shot['first_frame']:shot['first_frame'] + shot['frames']]
        for c in range(width):
            want = (span[:, c].astype(np.float64) * scale).astype(np.float32)
            assert np.array_equal(history.ordinate[c * len(pulses) + k], want), (k, c)
    ident = project[f'{stream} identification']
    assert ident.ordinate.shape == (width, identification[0]['frames'])
    assert ident.abscissa[0] == 0.0


def test_the_srs_of_every_pulse_and_the_reference_band_are_there(converted):
    folder, _done, project = converted
    srs = project['pulse_srs']
    assert isinstance(srs, Srs) and srs.q == 10.0 and srs.kind == 'maximax'
    pulses = _lines('results', 'pulses.jsonl')
    assert srs.ordinate.shape[0] == len(pulses)
    assert srs.block == [f'pulse {p["index"] + 1}' for p in pulses]
    assert srs.response_dof == ['1'] * len(pulses)
    run = run_folder.read(folder)
    measured = run.results['pulse_srs'].arrays['values'][:, 0, :]
    assert np.allclose(srs.ordinate, measured * G)
    assert np.allclose(srs.abscissa, run.results['pulse_srs'].arrays['frequency'][0])
    spec = project['srs_reference']
    assert isinstance(spec, ShockSpecification)
    reference = run.results['srs_reference'].arrays
    assert np.allclose(spec.ordinate[0], reference['reference'] * G)
    assert np.allclose(spec.limits['warning_upper'][0], reference['band_high'] * G)
    assert np.allclose(spec.limits['warning_lower'][0], reference['band_low'] * G)
    assert isinstance(project['identification_frf'], Frf)
    assert isinstance(project['identification_coherence'], Coherence)
    assert project['identification_frf'].reference_dof == ['3']


def test_the_run_record_holds_the_setup_the_log_every_verdict_and_the_outcome(converted):
    _folder, _done, project = converted
    record = project['Run Record']
    assert isinstance(record, RunRecord)
    assert record.setup == _json('setup.json')
    assert record.resolved['pulse']['shape'] == 'half_sine'
    assert record.config['operator'] == 'Operator'
    log = _lines('log.jsonl')
    assert record.log == log
    assert [e['seq'] for e in record.log] == list(range(len(log)))
    fired = [e for e in record.log if e['kind'] == 'pulse']
    assert [e['data']['pulse'] for e in fired] == [0, 1]
    pulses = _lines('results', 'pulses.jsonl')
    held = record.arrays['pulses']
    assert held['passed'].shape == (len(pulses), 1)
    assert list(held['passed'][:, 0]) == [1.0 if p['passed'] else 0.0 for p in pulses]
    assert np.all(np.isfinite(held['error'])) and np.all(held['peak_velocity'] > 0)
    assert record.results['pulses']['records'] == pulses
    # the nominal pulse and its time-domain band have no analysis class: arrays
    reference = record.arrays['pulse_reference']
    assert set(reference) == {'time', 'reference', 'band_low', 'band_high'}
    assert reference['time'][0] < 0 < reference['time'][-1]
    assert record.complete and record.outcome == 'completed' == _json('end.json')['end'][
        'outcome']
    assert record.source == 'SIM' and record.test == 'classical_shock'
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
    section = text.split('## What a Classical Shock Run Saves', 1)[1].split('\n## ', 1)[0]
    return re.findall(r'^\| `([^`]+)` \|', section, flags=re.MULTILINE)


def test_the_guide_says_exactly_what_a_classical_shock_run_saves():
    named = _saved_by_the_guide()
    assert named, 'the guide lists nothing'
    run = run_folder.read(GOLDEN)
    on_disk = ([f'streams/{name}' for name in run.streams]
               + [f'results/{name}' for name in run.results] + ['end.json'])
    assert sorted(named) == sorted(on_disk)
