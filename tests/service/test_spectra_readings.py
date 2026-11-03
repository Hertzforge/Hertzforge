"""Readings of spectra, FRFs and cross-spectral densities: a tone, the
transmissibility, the confidence band, the partial coherence, the
rigid-body residual, the field limit, the octave preview, a sine
profile, the CMIF and the drive points."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

import hertzforge
from hertzforge.core.averaging import Averaging
from hertzforge.service import projects
from hertzforge.service.ops_spectra import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .test_ops import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in OPERATIONS}


@pytest.fixture(scope='module')
def random_project():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'random.nc4')})
    state.project.compute_psds('Time History')
    return state.project


@pytest.fixture(scope='module')
def modal_project():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    project = state.project
    for verb in ('compute_psds', 'compute_frfs', 'compute_cpsds', 'compute_multiple_coherence'):
        getattr(project, verb)('Time History')
    return project


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()})
    return op.run(project, values)


def rows(reading):
    return {row['label']: row for row in reading['rows']}


def series(view, name):
    return next(s for s in view['series'] if s['name'] == name)


def test_the_descriptors_validate(validate):
    for op in OPERATIONS:
        validate(op.to_json(), 'operation')


def test_a_sine_reads_its_amplitude_as_a_tone(validate):
    rate, n = 1024.0, 32768
    t = np.arange(n) / rate
    noise = 0.01 * np.random.default_rng(1).standard_normal(n)
    project = hertzforge.Project()
    record = hertzforge.TimeHistory(t, [2.0 * np.sin(2 * np.pi * 101.3 * t) + noise],
                                    response_dof=['1Z+'], ordinate_dim='acceleration')
    record.averaging = Averaging(frame_length=1024, frames=31)
    project.add('Run', record)
    name = project.compute_psds('Run')
    view, reading = run(project, 'project.tone_readout', spectrum=name)
    wire_views([view, reading], validate)
    found = rows(reading)
    assert found['Tone']['value'] == pytest.approx(2.0, rel=0.02)
    assert found['Tone at']['value'] == pytest.approx(101.3, abs=0.2)
    assert found['Tone']['unit'] == 'm/s²'
    assert found['Window from']['value'].startswith('the averaging')
    with pytest.raises(ProtocolError, match='there is no record 2'):
        run(project, 'project.tone_readout', spectrum=name, record=2)


def test_a_tone_is_read_at_the_frequency_given(random_project):
    _view, reading = run(random_project, 'project.tone_readout',
                         spectrum='Time History PSDs', frequency=200.0, window='flattop')
    found = rows(reading)
    assert found['Tone at']['value'] == pytest.approx(200.0, abs=10.0)
    assert found['Window from']['value'] == 'chosen (flattop)'


def test_a_cross_term_has_no_tone(modal_project):
    with pytest.raises(ProtocolError, match='cross'):
        run(modal_project, 'project.tone_readout', spectrum='Time History CPSDs', record=2)


def _one_input_project():
    """A response at a 100 Hz, 2 % damped mode under a measured input."""
    rng = np.random.default_rng(3)
    rate, n = 1024.0, 16384
    t = np.arange(n) / rate
    x = rng.standard_normal(n)
    f = np.fft.rfftfreq(n, 1 / rate)
    h = 1.0 / (1.0 - (f / 100.0) ** 2 + 2j * 0.02 * f / 100.0)
    y = np.fft.irfft(np.fft.rfft(x) * h, n) + 0.01 * rng.standard_normal(n)
    record = hertzforge.TimeHistory(t, [x, y], response_dof=['1Z+', '2Z+'],
                                    ordinate_dim='acceleration')
    record.roles = {('1Z+', 'acceleration'): 'reference'}
    record.averaging = Averaging(frame_length=1024, frames=15)
    project = hertzforge.Project()
    project.add('Run', record)
    return project, project.compute_frfs('Run', 'H1')


def test_transmissibility_reads_the_mode_s_magnification_off_resonance(validate):
    project, name = _one_input_project()
    view, reading = run(project, 'project.transmissibility', frf=name, frequency=50.0)
    wire_views([view, reading], validate)
    found = rows(reading)
    assert found['H1']['value'] == pytest.approx(1.0 / (1.0 - 0.25), rel=0.01)
    assert found['ASD ratio']['value'] == pytest.approx(found['H1']['value'], rel=0.01)
    assert found['H1']['unit'] is None
    _view, start = run(project, 'project.transmissibility', frf=name)
    assert rows(start)['At']['value'] == pytest.approx(100.0, abs=1.0)


def test_transmissibility_refuses_an_frf_against_two_references(modal_project):
    with pytest.raises(ProtocolError, match='2 references'):
        run(modal_project, 'project.transmissibility', frf='Time History Hv FRFs')


def test_the_confidence_band_is_the_chi_square_band(random_project, validate):
    from scipy.stats import chi2

    psd = random_project['Time History PSDs']
    view, reading = run(random_project, 'project.confidence_band',
                        spectrum='Time History PSDs', dof=10.0)
    wire_views([view, reading], validate)
    assert rows(reading)['DOF from']['value'] == 'typed'
    lower = series(view, '90 % lower bound')
    k = len(lower['y']) // 2
    line = np.flatnonzero(np.isclose(psd.abscissa, series(view, '101Z+')['x'][k]))[0]
    estimate = np.real(psd.ordinate[0, line])
    assert lower['y'][k] == pytest.approx(estimate * 10.0 / chi2.ppf(0.95, 10), rel=1e-6)
    upper = series(view, '90 % upper bound')
    assert upper['y'][k] == pytest.approx(estimate * 10.0 / chi2.ppf(0.05, 10), rel=1e-6)
    _view, own = run(random_project, 'project.confidence_band',
                     spectrum='Time History PSDs', level=0.95)
    assert rows(own)['DOF from']['value'] == 'the averaging'


def test_a_cross_spectral_matrix_has_no_confidence_band(modal_project):
    with pytest.raises(ProtocolError, match='cross'):
        run(modal_project, 'project.confidence_band', spectrum='Time History CPSDs')


def test_partial_coherence_reads_the_multiple_coherence_it_is_behind(modal_project,
                                                                     validate):
    *views, reading = run(modal_project, 'project.partial_coherence',
                          coherence='Time History Multiple Coherence')
    wire_views([*views, reading], validate)
    assert len(views) == 3
    multiple = np.real(modal_project['Time History Multiple Coherence'].ordinate[0])
    found = rows(reading)
    assert found['Multiple, median']['value'] == pytest.approx(
        np.median(multiple[np.isfinite(multiple)]), rel=1e-6)
    assert found['References']['value'] == '101Z+ force, 1310Z+ force'


def test_partial_coherence_refuses_a_coherence_not_computed_here(modal_project):
    project = hertzforge.Project()
    project.add('Copy', modal_project['Time History Multiple Coherence'])
    with pytest.raises(ProtocolError, match='Copy'):
        run(project, 'project.partial_coherence', coherence='Copy')


def _rigid_project():
    """Five triaxial points moving as a rigid body, with independent
    noise above 300 Hz that no rigid motion explains."""
    from hertzforge.core.rigid import MassProperties, rigid_body_shapes

    rng = np.random.default_rng(5)
    rate, n = 1024.0, 16384
    t = np.arange(n) / rate
    geometry = hertzforge.Geometry(np.arange(101, 106), rng.uniform(-1.0, 1.0, (5, 3)),
                                   length_unit='m')
    rigid = rigid_body_shapes(geometry, MassProperties((0.0, 0.0, 0.0)))
    motion = rigid.shape_matrix.T @ rng.standard_normal((6, n))
    noise = np.fft.rfft(rng.standard_normal(motion.shape), axis=1)
    noise[:, np.fft.rfftfreq(n, 1 / rate) < 300.0] = 0.0
    record = hertzforge.TimeHistory(t, motion + 2.0 * np.fft.irfft(noise, n, axis=1),
                                    response_dof=list(rigid.coordinate),
                                    ordinate_dim='acceleration')
    record.averaging = Averaging(frame_length=1024, frames=15)
    project = hertzforge.Project()
    project.add('Run', record)
    project.add('Rigid', rigid)
    cpsds = project.compute_cpsds('Run')
    return project, cpsds, project.transform(cpsds, 'Rigid')


def test_the_rigid_body_holds_up_to_where_the_flexing_starts(validate):
    project, cpsds, modal = _rigid_project()
    *views, reading = run(project, 'project.rigid_body_residual', density=modal)
    wire_views([*views, reading], validate)
    found = rows(reading)
    assert 280.0 <= found['Holds to']['value'] <= 300.0
    assert found['Channels']['value'] == '15 to 6 motions'
    with pytest.raises(ProtocolError, match='not transformed'):
        run(project, 'project.rigid_body_residual', density=cpsds)


def test_an_envelope_limit_is_the_largest_record(random_project, validate):
    view, reading = run(random_project, 'project.field_limit', source='Time History PSDs',
                        statistic='envelope', margin_db=3.0)
    wire_views([view, reading], validate)
    psd = random_project['Time History PSDs']
    accelerations = [i for i in range(psd.num_records)
                     if psd.ordinate_dim[i].startswith('acceleration')]
    found = rows(reading)
    assert found['Records']['value'] == len(accelerations) == 8
    limit = next(s for s in view['series'] if s['role'] == 'reference')
    k = len(limit['x']) // 2
    line = np.flatnonzero(np.isclose(psd.abscissa, limit['x'][k]))[0]
    assert limit['y'][k] == pytest.approx(np.max(np.real(psd.ordinate[accelerations, line])), rel=1e-9)
    assert view['series'][-1]['name'] == 'with +3 dB margin'
    assert len(list(random_project.items())) == 4, 'a reading adds nothing'


def test_a_tolerance_limit_of_one_record_needs_a_stated_deviation(random_project):
    with pytest.raises(ProtocolError, match='standard deviation'):
        run(random_project, 'project.field_limit', source='Time History PSDs', records='1')
    _view, reading = run(random_project, 'project.field_limit', source='Time History PSDs',
                         records='1', sigma_db=3.0)
    assert rows(reading)['Scatter (median)']['value'] == '3 dB stated'


def test_octave_bands_keep_the_mean_square(random_project, validate):
    psd = random_project['Time History PSDs']
    view, reading = run(random_project, 'project.octave_bands',
                        spectrum='Time History PSDs', per_octave=3)
    wire_views([view, reading], validate)
    steps = series(view, '1/3 octave bands')
    assert steps['shape'] == 'steps'
    assert rows(reading)['Bands']['value'] == len(steps['y'])
    banded = np.sum(steps['y'] * np.diff(steps['edges']))
    low, high = steps['edges'][0], steps['edges'][-1]
    f = np.asarray(psd.abscissa)
    inside = (f >= low) & (f <= high)
    narrow = np.sum(np.real(psd.ordinate[0, inside])) * (f[1] - f[0])
    assert banded == pytest.approx(narrow, rel=0.02)


def test_a_spectrum_of_one_line_is_not_banded():
    project = hertzforge.Project()
    project.add('Narrow', hertzforge.Psd([100.0], [[1.0]],
                                         response_dof=['1Z+']))
    with pytest.raises(ProtocolError, match='octave bands need a spectrum with frequency lines'):
        run(project, 'project.octave_bands', spectrum='Narrow', per_octave=1)


def test_a_sine_profile_reads_back_its_segments(validate):
    from hertzforge.core.sine_profile import SineProfile, acceleration, displacement

    segments = [displacement(0.20, 'in', 'peak to peak', start=5.0, end=14.0),
                acceleration(2.0, 'g', start=14.0, end=33.0),
                displacement(0.036, 'in', 'peak to peak', start=33.0, end=52.1),
                acceleration(5.0, 'g', start=52.1, end=500.0)]
    project = hertzforge.Project()
    project.add('Sine', SineProfile(segments).specification(['1Z+'], 2.0))
    *views, reading = run(project, 'project.sine_profile', specification='Sine')
    wire_views([*views, reading], validate)
    assert len(views) == 3
    found = rows(reading)
    assert found['Segments']['value'] == 4
    assert found['Crossovers']['status'] == 'pass'
    assert found['Most displacement peak to peak']['value'] == pytest.approx(0.20 * 0.0254,
                                                                              rel=1e-3)
    assert found['Most acceleration']['value'] == pytest.approx(5.0 * 9.80665, rel=1e-3)
    with pytest.raises(ProtocolError, match='no tone'):
        run(project, 'project.sine_profile', specification='Sine', tone='Nope')


def test_one_frf_s_cmif_is_its_magnitude(modal_project, validate):
    frf = modal_project['Time History Hv FRFs']
    (view,) = run(modal_project, 'project.cmif', frf='Time History Hv FRFs', records='3')
    wire_views([view], validate)
    (curve,) = view['series']
    k = len(curve['x']) // 2
    line = np.flatnonzero(np.isclose(frf.abscissa, curve['x'][k]))[0]
    assert curve['y'][k] == pytest.approx(abs(frf.ordinate[2, line]), rel=1e-6)
    (whole,) = run(modal_project, 'project.cmif', frf='Time History Hv FRFs')
    assert [s['name'] for s in whole['series']] == ['CMIF 1', 'CMIF 2']
    with pytest.raises(ProtocolError, match='not a record number'):
        run(modal_project, 'project.cmif', frf='Time History Hv FRFs', records='x')


def test_the_drive_points_are_the_frfs_at_their_own_reference(modal_project, validate):
    (view,) = run(modal_project, 'project.drive_points', data='Time History Hv FRFs')
    wire_views([view], validate)
    assert [s['name'] for s in view['series']] == ['101Z+/101Z+', '1310Z+/1310Z+']
    with pytest.raises(ProtocolError, match='no references'):
        run(modal_project, 'project.drive_points', data='Time History PSDs')
