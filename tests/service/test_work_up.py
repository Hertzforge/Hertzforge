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
