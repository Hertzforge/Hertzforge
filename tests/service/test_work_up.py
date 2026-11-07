"""Work Up in each module, through the service, on the plate's own files, with
and without a geometry, and the shock readings it leaves behind."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

from hertzforge.core.compliance import srs_errors, srs_tolerance
from hertzforge.core.data import ShockSpecification, TimeHistory, TransientSpecification
from hertzforge.core.report import Report
from hertzforge.core.report_caption import srs_caption
from hertzforge.core.signals import sine_history
from hertzforge.core.sine import SineSweepSpecification, SineTone
from hertzforge.project import Project
from hertzforge.report.check_blocks import _read_srs_points, _show_srs_points
from hertzforge.report.runs import read_record
from hertzforge.service import projects, verbs
from hertzforge.service.protocol import ProtocolError
from hertzforge.service.server import OPERATIONS
from hertzforge.units import DEFAULT_SYSTEM

from .conftest import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in OPERATIONS}
G = 9.80665
GEOMETRY = {'path': str(PLATE / 'geometry.unv'), 'units': {'length_unit': 'mm'}}


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return op.run(project, values)


def load(*files):
    state = None
    for file in files:
        state, _ = projects.import_file(state, GEOMETRY if file == 'geometry'
                                        else {'path': str(PLATE / file)})
    return state.project


def values(reading, label):
    return [row['value'] for row in reading['rows'] if row['label'] == label]


def row_of(reading, label):
    return next(row for row in reading['rows'] if row['label'] == label)


def history(t, rows, dofs=None):
    rows = np.atleast_2d(rows)
    return TimeHistory(t, rows, response_dof=dofs or [f'{i + 1}X+' for i in range(len(rows))],
                       ordinate_dim=['acceleration'] * len(rows),
                       ordinate_unit=['m/s**2'] * len(rows))


def half_sine(fs=20480.0, duration=1.0, width=0.003, peak=50 * G, at=0.2):
    t = np.arange(int(duration * fs)) / fs
    return t, np.where((t >= at) & (t <= at + width), peak * np.sin(np.pi * (t - at) / width), 0.0)


# ---- a geometry loaded ---------------------------------------------------------

@pytest.mark.parametrize('module, file', [('random', 'random.nc4'), ('modal', 'modal.nc4'),
                                          ('shock', 'shock.nc4'), ('transient', 'shock.nc4'),
                                          ('sysid', 'random.nc4')])
def test_a_geometry_does_not_stop_work_up(module, file, validate):
    """A run's import group holds what the Basis slots read, so Work Up does
    with a geometry loaded, in either order, what it does without one."""
    (alone,) = run(load(file), f'project.work_up_{module}')
    added = values(alone, 'Added')
    assert len(added) > 1 and added[-1] == 'Report'
    for files in ((file, 'geometry'), ('geometry', file)):
        project = load(*files)
        (reading,) = run(project, f'project.work_up_{module}')
        wire_views([reading], validate)
        assert values(reading, 'Added') == added
        assert 'Geometry' in project.placed()['Basis']
        assert 'Time History' in project.placed()['Basis']
        missing = values(reading, 'Missing')
        assert not {'Channel Table', 'Time History', 'PSD', 'FRF', 'SRS',
                    'Multiple Coherence'} & set(missing), missing
        (again,) = run(project, f'project.work_up_{module}')
        assert values(again, 'Added') == ['nothing to compute']


def test_a_sine_work_up_with_a_geometry_reads_the_levels():
    tone = SineTone('Up', 0.5, [100.0, 500.0], [[2.0]] * 2, [0], [100.0])
    spec = SineSweepSpecification([tone], ['1Z+'], ordinate_unit='m/s**2')
    for geometry in (False, True):
        state, _ = projects.import_file(None, GEOMETRY) if geometry else (None, None)
        project = state.project if geometry else Project()
        project.add('Time History', sine_history(spec, sample_rate=4096.0))
        project.add('Profile', spec)
        (reading,) = run(project, 'project.work_up_sine')
        assert 'Sine Levels' in values(reading, 'Added')


def test_a_model_is_not_taken_for_the_run():
    """A group of a geometry and its shapes is the other side, not the test."""
    worked = load('modal.nc4', 'geometry')
    run(worked, 'project.work_up_modal')
    project = Project()
    project.add('Model Geometry', worked['Geometry'])
    project.add('Model Shapes', worked['Time History Hv FRFs Modes'])
    project.link('Model Geometry', 'Model Shapes')
    project.add('Time History', worked['Time History'])
    run(project, 'project.work_up_random')
    assert 'Model Shapes' not in project.placed().get('Basis', [])
    assert 'Model Geometry' not in project.placed().get('Basis', [])


def test_a_second_run_stays_out_of_a_basis_that_holds_a_run():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'random.nc4')})
    state, _ = projects.import_file(state, GEOMETRY)
    project = state.project
    run(project, 'project.work_up_random')
    basis = list(project.placed()['Basis'])
    state, added = projects.import_file(state, {'path': str(PLATE / 'modal.nc4')})
    run(project, 'project.work_up_random')
    assert not set(added) & set(project.placed()['Basis'])
    assert set(basis) <= set(project.placed()['Basis'])


# ---- Work Up Sine with no specification, or two --------------------------------

def test_a_sine_work_up_with_no_specification_leaves_its_slots_gray(validate):
    project = load('random.nc4')
    (reading,) = run(project, 'project.work_up_sine')
    wire_views([reading], validate)
    assert values(reading, 'Added') == ['Report']
    assert {'Sine Sweep Specification', 'Sine Level Set'} <= set(values(reading, 'Missing'))
    assert any('no sine sweep specification' in value for value in values(reading, 'Not computed'))


def test_a_sine_work_up_with_two_specifications_says_it_cannot_pick():
    project = load('random.nc4')
    tone = SineTone('Up', 0.5, [100.0, 500.0], [[2.0]] * 2, [0], [100.0])
    for name in ('A', 'B'):
        project.add(name, SineSweepSpecification([tone], ['101Z+'], ordinate_unit='m/s**2'))
    (reading,) = run(project, 'project.work_up_sine')
    assert values(reading, 'Added') == ['Report']
    assert 'Sine Level Set' in values(reading, 'Missing')
    said = ' '.join(values(reading, 'Not computed'))
    assert "'A'" in said and "'B'" in said and 'Use .' not in said


def test_extract_sine_refuses_in_words_and_can_be_told_which(monkeypatch):
    project = load('random.nc4')
    with pytest.raises(ProtocolError, match='applies to none'):
        run(project, 'project.extract_sine')
    with pytest.raises(ProtocolError, match='no sine sweep specification'):
        OPS['project.extract_sine'].run(project, {'source': 'Time History', 'specification': ''})
    tone = SineTone('Up', 0.5, [100.0, 500.0], [[2.0]] * 2, [0], [100.0])
    for name in ('A', 'B'):
        project.add(name, SineSweepSpecification([tone], ['101Z+'], ordinate_unit='m/s**2'))
    before = list(project)
    with pytest.raises(ProtocolError, match="2 sine sweep specifications"):
        run(project, 'project.extract_sine', source='Time History')
    assert list(project) == before
    monkeypatch.setattr('hertzforge.service.ops_project.waterfall_view',
                        lambda name, obj: {'title': name}, raising=False)
    run(project, 'project.extract_sine', source='Time History', specification='B')
    assert project.provenance['Sine Levels']['params']['specification'] == 'B'


def test_a_failed_slot_says_why_on_any_module(validate):
    """A record with no references has no coherence to compute: the slot stays
    gray with its reason, and the slots after it fill."""
    t = np.arange(4096) / 4096.0
    noise = np.random.default_rng(1).standard_normal((1, t.size))
    project = Project()
    project.add('Time History', history(t, noise))
    (reading,) = run(project, 'project.work_up_random')
    wire_views([reading], validate)
    assert values(reading, 'Added')[-1] == 'Report'
    assert any('Multiple Coherence' in value for value in values(reading, 'Not computed'))


# ---- Work Up Shock: where the events are found, and how far the SRS runs -------

def test_work_up_shock_finds_the_events_on_the_recording():
    project = load('shock.nc4')
    run(project, 'project.work_up_shock')
    raw, filtered = project['Time History'], project['Time History Filtered']
    assert len(raw.shocks) == 4
    assert filtered.shocks == raw.shocks
    srs = project['Time History Filtered SRS']
    assert srs.num_records == 7 * 4
    assert project.stale() == {}


def test_apply_filter_then_compute_srs_finds_the_events_on_the_recording():
    project = load('shock.nc4')
    run(project, 'project.filter_data', source='Time History')
    run(project, 'project.compute_srs', source='Time History Filtered')
    assert len(project['Time History'].shocks) == 4
    assert project['Time History Filtered'].shocks == project['Time History'].shocks
    assert project['Time History Filtered SRS'].num_records == 7 * 4


def test_compute_srs_takes_marks_set_on_the_recording_after_the_filter():
    project = load('shock.nc4')
    run(project, 'project.filter_data', source='Time History')
    project.detect_shocks('Time History')
    project.compute_srs('Time History Filtered')
    assert project['Time History Filtered'].shocks == project['Time History'].shocks


def ringdown():
    fs = 10240.0
    t = np.arange(int(fs)) / fs
    tail = np.clip(t - 0.2, 0.0, None)
    x = np.where(t > 0.2, 100.0 * np.exp(-tail * 200.0) * np.sin(2 * np.pi * 1500.0 * tail), 0.0)
    return fs, t, x


def test_a_filtered_records_srs_stops_at_the_filters_corner(validate):
    """A ring at 1500 Hz is gone from a record low-passed at 1024 Hz, so its
    SRS has no business reading a line there 22 dB low."""
    fs, t, x = ringdown()
    project = Project()
    project.add('Time Data', history(t, x))
    (reading,) = run(project, 'project.work_up_shock')
    wire_views([reading], validate)
    corner = fs / 10.0
    srs = project['Time Data Filtered SRS']
    assert srs.abscissa[-1] <= corner * 2.0 ** (1 / 24)
    assert any('1024 Hz' in value for value in values(reading, 'SRS band'))
    # a refresh recomputes the same band
    before = np.array(srs.abscissa)
    project.refresh('Time Data Filtered SRS')
    assert np.array_equal(project['Time Data Filtered SRS'].abscissa, before)


def test_the_plates_srs_agrees_with_the_recording_well_below_the_corner():
    project = load('shock.nc4')
    run(project, 'project.work_up_shock')
    srs = project['Time History Filtered SRS']
    raw = project['Time History'].compute_srs()
    assert srs.abscissa[-1] == pytest.approx(project['Time History'].sample_rate / 10.0, rel=0.03)
    assert srs.num_records == raw.num_records
    below = srs.abscissa <= srs.abscissa[-1] / 2.0
    on_raw = np.array([np.interp(srs.abscissa, raw.abscissa, row) for row in raw.ordinate])
    assert np.abs(20 * np.log10(srs.ordinate / on_raw)[:, below]).max() < 1.0


def test_the_recordings_own_srs_still_runs_to_a_fifth_of_the_rate():
    fs, t, x = ringdown()
    project = Project()
    project.add('Time Data', history(t, x))
    project.compute_srs('Time Data')
    assert project['Time Data SRS'].abscissa[-1] == pytest.approx(fs / 5.0, rel=0.03)


def test_a_high_pass_does_not_cap_the_srs():
    fs, t, x = ringdown()
    project = Project()
    project.add('Time Data', history(t, x))
    run(project, 'project.set_filter', source='Time Data', kind='high-pass', low=20.0)
    run(project, 'project.filter_data', source='Time Data')
    project.compute_srs('Time Data Filtered')
    assert project['Time Data Filtered SRS'].abscissa[-1] == pytest.approx(fs / 5.0, rel=0.03)


# ---- an SRS over part of its requirement's band --------------------------------

def requirement():
    return ShockSpecification.from_breakpoints(
        [20.0, 2000.0], [50.0, 50.0], '1X+', ordinate_unit='g',
        warning=(-1.5, 3.0), abort=(-3.0, 6.0))


def partial_project():
    t, x = half_sine()
    project = Project()
    project.add('R', history(t, x))
    project.add('Req', requirement())
    project.add('Partial', project['R'].compute_srs(low=800.0, high=2000.0, per_octave=12))
    return project


def test_points_rule_does_not_pass_a_spectrum_that_misses_the_band(validate):
    project = partial_project()
    plot, reading = run(project, 'project.srs_points', srs='Partial', requirement='Req')
    wire_views([plot, reading], validate)
    verdict = row_of(reading, 'Verdict')
    assert verdict['value'] == 'not judged' and verdict['status'] == 'warn'
    said = row_of(reading, 'Not read')
    assert said['status'] == 'warn' and '20' in said['value'] and '800' in said['value']


def test_a_failing_spectrum_still_fails_when_it_misses_the_band():
    project = partial_project()
    project['Partial'].ordinate *= 0.1
    _plot, reading = run(project, 'project.srs_points', srs='Partial', requirement='Req')
    assert row_of(reading, 'Verdict')['value'] == 'fails'
    assert 'Not read' in [row['label'] for row in reading['rows']]


def test_the_error_reading_says_what_it_did_not_read(validate):
    project = partial_project()
    (reading,) = run(project, 'project.srs_error', srs='Partial', requirement='Req')
    wire_views([reading], validate)
    channel = row_of(reading, '1X+')
    assert channel['status'] == 'warn', 'inside the band, but only over part of it'
    assert row_of(reading, 'Channels out')['status'] == 'warn'
    assert '800' in row_of(reading, 'Not read')['value']


def test_a_spectrum_over_the_whole_band_is_judged_as_before():
    t, x = half_sine(peak=1.0)
    requirement_ = requirement()
    whole = history(t, x).compute_srs(low=20.0, high=2000.0, per_octave=12)
    whole.ordinate[:] = 50 * G
    result = srs_tolerance(requirement_, whole)
    assert result['passed'] is True and 'unread' not in result
    assert srs_errors(requirement_, whole)[0].unread is None
    # a line short of each end, as a grid lands, is the whole band
    near = history(t, x).compute_srs(low=21.0, high=1900.0, per_octave=12)
    near.ordinate[:] = 50 * G
    assert srs_tolerance(requirement_, near)['passed'] is True


def test_the_report_says_a_partial_spectrum_was_not_judged():
    project = partial_project()
    found = {'source': project['Req'], 'measured': project['Partial']}
    shown = _show_srs_points(_read_srs_points(found, {}), found, DEFAULT_SYSTEM)
    assert 'not judged' in shown.rows[0][-1]
    assert '800' in shown.words
    caption = srs_caption(project['Partial'], specification=project['Req'])
    text = caption.points[0].text()
    assert 'not judged' in text and '800' in text


# ---- modules ------------------------------------------------------------------

def test_every_module_with_set_filter_can_apply_it():
    applied = verbs.modules_of(OPS['project.filter_data'])
    assert set(verbs.modules_of(OPS['project.set_filter'])) <= set(applied)
    assert 'shock' in verbs.modules_of(OPS['project.integrate'])


# ---- Work Up Transient: the run on the reference's frames ----------------------

def transient_project():
    fs = 4096.0
    t = np.arange(4096) / fs
    wave = np.exp(-((t - 0.5) / 0.1) ** 2) * (10 * np.sin(2 * np.pi * 50 * t)
                                              + 5 * np.sin(2 * np.pi * 200 * t))
    reference = TransientSpecification(t, wave[None], response_dof=['1X+'],
                                       ordinate_dim=['acceleration'],
                                       ordinate_unit=['m/s**2'])
    played = np.tile(wave, 6)
    project = Project()
    project.add('Run', history(np.arange(played.size) / fs, played))
    project.add('Ref', reference)
    return project


@pytest.mark.parametrize('with_geometry', [False, True])
def test_work_up_transient_reads_the_run_on_the_references_frames(with_geometry):
    project = transient_project()
    if with_geometry:
        project.import_file(str(PLATE / 'geometry.unv'), length_unit='mm')
    run(project, 'project.work_up_transient')
    assert project['Run'].averaging.frame_length == 4096
    assert project['Run'].averaging.frames == 6
    ref, measured = project['Ref PSDs'], project['Run PSDs']
    assert np.array_equal(ref.abscissa, measured.abscissa)
    i = int(np.argmax(np.real(ref.ordinate[0])))
    assert np.real(measured.ordinate[0][i]) == pytest.approx(np.real(ref.ordinate[0][i]), rel=1e-6)


def test_work_up_transient_keeps_an_averaging_already_set():
    project = transient_project()
    project['Run'].averaging = project['Run'].suggest_averaging(frame_length=1024)
    held = project['Run'].averaging
    run(project, 'project.work_up_transient')
    assert project['Run'].averaging == held
