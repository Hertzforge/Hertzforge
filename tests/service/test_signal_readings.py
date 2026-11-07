"""What the signal readings say on the wire: values and their units moved
together into the display system, the record asked for and not its
neighbor, and refusals where a reading cannot be made."""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core import compliance
from hertzforge.core.averaging import Averaging
from hertzforge.core.data import Psd, Specification, TimeHistory
from hertzforge.service import display_units, projects
from hertzforge.service.protocol import ProtocolError
from hertzforge.service.server import OPERATIONS

OPS = {op.id: op for op in OPERATIONS}
G = 9.80665
LBF = 4.4482216152605
DENSITY = 'acceleration**2/frequency'


def _values(project, op_id, given):
    return OPS[op_id].resolve(given, objects={name: projects.kind_of(obj)
                                              for name, obj in project.items()},
                              verbs={name: projects.verbs_of(project, name) for name in project})


def run(project, op_id, **given):
    return OPS[op_id].run(project, _values(project, op_id, given))


def rows(views):
    return {row['label']: row for view in views if view.get('view') == 'reading'
            for row in view['rows']}


def _force_and_response():
    """A response of about 0.7 g per lbf under a measured force."""
    rng = np.random.default_rng(5)
    rate, n = 2048.0, 2 ** 15
    force = rng.standard_normal(n) * LBF
    response = (np.convolve(force, [0.5, 0.2], mode='same') * G / LBF
                + 0.1 * G * rng.standard_normal(n))
    record = TimeHistory(np.arange(n) / rate, np.vstack([force, response]),
                         response_dof=['1Z+', '2Z+'],
                         ordinate_dim=['force', 'acceleration'],
                         ordinate_unit=['lbf', 'g'])
    record.averaging = Averaging(frame_length=1024, overlap=0.5, window='hann').filled(n, rate)
    project = hertzforge.Project()
    project.add('Run', record)
    return project


def test_the_transmissibility_band_converts_with_its_unit():
    project = _force_and_response()
    frf = project.compute_frfs('Run', 'H1')
    views = run(project, 'project.transmissibility', frf=frf, frequency=200.0)
    si = {label: dict(row) for label, row in rows(views).items()}
    assert si['Band, low']['unit'] == si['H1']['unit'] == '(m/s²)/N'
    display_units.convert(views, display_units.START)
    found = rows(views)
    assert found['H1']['unit'] == found['Band, low']['unit'] == found['Band, high']['unit'] \
        == 'g/lbf'
    assert found['Band, low']['value'] == pytest.approx(si['Band, low']['value'] * LBF / G)
    assert found['Band, low']['value'] < found['H1']['value'] < found['Band, high']['value']


def test_words_with_a_unit_keep_the_unit_they_are_in():
    views = [{'view': 'reading', 'title': 'T', 'rows': [
        {'label': 'Said', 'value': 'not read', 'unit': 'm/s²', 'status': 'info'}]}]
    display_units.convert(views, display_units.START)
    assert views[0]['rows'][0]['unit'] == 'm/s²', \
        'a value left in SI words cannot be labeled in another system'


def _two_quantities_at_one_dof():
    rng = np.random.default_rng(4)
    rate, n = 1024.0, 2 ** 15
    force = rng.standard_normal(n)
    acceleration = 2.0 * force + 0.1 * rng.standard_normal(n)
    velocity = 0.01 * rng.standard_normal(n)
    other = 0.3 * force + rng.standard_normal(n)
    record = TimeHistory(np.arange(n) / rate, np.vstack([force, acceleration, velocity, other]),
                         response_dof=['1Z+', '3Z+', '3Z+', '4Z+'],
                         ordinate_dim=['force', 'acceleration', 'velocity', 'acceleration'])
    record.averaging = Averaging(frame_length=1024, overlap=0.5, window='hann').filled(n, rate)
    project = hertzforge.Project()
    project.add('Run', record)
    return project


@pytest.mark.parametrize(('number', 'label'), [(1, '3Z+ acceleration'), (2, '3Z+ velocity'),
                                               (3, '4Z+')])
def test_partial_coherence_reads_the_channel_asked_for(number, label):
    project = _two_quantities_at_one_dof()
    name = project.compute_multiple_coherence('Run')
    coherence = project[name]
    found = rows(run(project, 'project.partial_coherence', coherence=name, record=number))
    assert found['Response']['value'] == label
    assert found['Multiple, median']['value'] == pytest.approx(
        float(np.median(coherence.ordinate[number - 1])), abs=1e-9)


def test_partial_coherence_refuses_a_channel_no_longer_a_response():
    project = _two_quantities_at_one_dof()
    name = project.compute_multiple_coherence('Run')
    project['Run'].roles = {('3Z+', 'velocity'): 'monitor', ('1Z+', 'force'): 'reference'}
    with pytest.raises(ProtocolError, match=r'3Z\+'):
        run(project, 'project.partial_coherence', coherence=name, record=2)


def _flights():
    rng = np.random.default_rng(2)
    f = np.arange(1.0, 1001.0)
    count = 10
    shape = 1.0 + 20.0 * np.exp(-((f - 300.0) / 60.0) ** 2)
    levels = shape * 10.0 ** (rng.normal(0.0, 3.0, (count, 1)) / 10.0)
    project = hertzforge.Project()
    project.add('Set', Psd(f, levels, response_dof=['1Z+'] * count,
                           ordinate_dim=[DENSITY] * count,
                           block=[str(k) for k in range(count)]))
    return project


@pytest.mark.parametrize('settings', [
    {'statistic': 'mean_k', 'k': 1.5},
    {'statistic': 'mean_k', 'k': 2.0, 'sigma_db': 4.0},
    {'statistic': 'tolerance', 'sigma_db': 2.5},
    {'statistic': 'tolerance', 'max_overshoot_db': 1.0},
    {'statistic': 'tolerance', 'rms': 'conserve'},
    {'statistic': 'envelope', 'max_overshoot_db': 1.0},
], ids=str)
def test_derive_writes_the_envelope_field_limit_shows(settings):
    project = _flights()
    run(project, 'project.derive_specification', source='Set', **settings)
    written = project['Set Derived Specification']
    fit = project.field_limit('Set', **{'max_breakpoints': 12, **settings})
    assert written.abscissa == pytest.approx(fit.frequencies)
    assert np.real(written.ordinate[0]) == pytest.approx(fit.levels)


def _drive_point_specification():
    """Two channels at one DOF and the cross term between them, which names
    that DOF twice and is not a channel."""
    levels = np.ones((3, 2))
    return Specification(np.array([20.0, 500.0]), levels,
                         response_dof=['1Z+', '1Z+', '1Z+'],
                         reference_dof=['1Z+', '1Z+', '1Z+'],
                         ordinate_dim=[DENSITY, 'acceleration*force/frequency',
                                       'force**2/frequency'])


def test_spec_rms_scores_channels_and_not_a_cross_term_at_one_dof():
    project = hertzforge.Project()
    project.add('Spec', _drive_point_specification())
    (reading,) = run(project, 'project.spec_rms', specification='Spec')
    assert reading['rows'][-1]['value'].startswith('2 channels')
    assert len(reading['rows']) == 3
    assert {row['unit'] for row in reading['rows'][:-1]} == {'m/s²', 'N'}


def _force_and_acceleration_at_one_dof():
    rng = np.random.default_rng(4)
    rate, n = 1024.0, 2 ** 14
    force = rng.standard_normal(n)
    record = TimeHistory(np.arange(n) / rate,
                         np.vstack([force, 2.0 * force + 0.1 * rng.standard_normal(n),
                                    rng.standard_normal(n)]),
                         response_dof=['1Z+', '1Z+', '4Z+'],
                         ordinate_dim=['force', 'acceleration', 'acceleration'])
    record.averaging = Averaging(frame_length=1024, overlap=0.5, window='hann').filled(n, rate)
    project = hertzforge.Project()
    project.add('Run', record)
    return project


def test_drive_points_of_a_density_are_its_autospectra():
    project = _force_and_acceleration_at_one_dof()
    name = project.compute_cpsds('Run')
    (view,) = run(project, 'project.drive_points', data=name)
    assert '3 of 9 records' in view['title']


def test_a_sheet_opens_the_channels_and_not_a_cross_term_at_one_dof():
    from hertzforge.core.author import SpecificationDraft

    spec = Specification(np.array([20.0, 500.0]), np.ones((3, 2)),
                         response_dof=['1Z+', '1Z+', '2Z+'],
                         reference_dof=['1Z+', '1Z+', '2Z+'],
                         ordinate_dim=[DENSITY, 'acceleration*force/frequency', DENSITY])
    assert list(SpecificationDraft.from_specification(spec).channels) == ['1Z+', '2Z+']


def test_control_channels_skip_a_cross_term_at_one_dof():
    from hertzforge.core.multi_point import control_channels

    spec = Specification(np.array([20.0, 500.0]), np.ones((2, 2)),
                         response_dof=['1Z+', '2Z+'], ordinate_dim=[DENSITY, DENSITY])
    f = np.arange(0.0, 513.0)
    measured = Psd(f, np.ones((3, 513)), response_dof=['1Z+', '1Z+', '2Z+'],
                   reference_dof=['1Z+', '1Z+', '2Z+'],
                   ordinate_dim=['acceleration*force/frequency', DENSITY, DENSITY])
    assert control_channels(spec, measured, channels=['1Z+', '2Z+']) == [1, 2]


def test_a_stage_stands_channels_and_not_a_cross_term_at_one_dof():
    spec = Specification(np.array([20.0, 500.0]), np.ones((3, 2)),
                         response_dof=['1Z+', '1Z+', '2Z+'],
                         reference_dof=['1Z+', '1Z+', '2Z+'],
                         ordinate_dim=[DENSITY, 'acceleration*force/frequency', DENSITY])
    project = hertzforge.Project()
    project.add('Spec', spec)
    (view,) = run(project, 'project.spec_stage', specification='Spec')
    assert len(view['stations']) == 2


def test_a_column_of_cross_terms_at_one_dof_is_a_reference_column():
    from hertzforge.service.show import _column

    f = np.arange(0.0, 8.0)
    data = Psd(f, np.ones((2, 8)) * (1.0 + 1.0j), response_dof=['1Z+', '1Z+'],
               reference_dof=['1Z+', '1Z+'],
               ordinate_dim=['acceleration*force/frequency', 'velocity*force/frequency'])
    assert _column(data, [0, 1]) is not None


def _flat_target(warning_db=3.0, abort_db=6.0):
    level = np.array([[1e-2, 1e-2]])
    return Specification(np.array([20.0, 2000.0]), level, response_dof=['1Z+'],
                         ordinate_dim=[DENSITY],
                         warning_lower=level * 10.0 ** (-warning_db / 10.0),
                         warning_upper=level * 10.0 ** (warning_db / 10.0),
                         abort_lower=level * 10.0 ** (-abort_db / 10.0),
                         abort_upper=level * 10.0 ** (abort_db / 10.0))


@pytest.mark.parametrize('offset_db', [6.0, -6.0, 3.0, -3.0])
def test_a_power_exactly_on_a_limit_is_inside(offset_db):
    f = np.arange(0.0, 2049.0)
    share = compliance.area_on_lines(np.array([20.0, 2000.0]), np.ones(2), f)
    measured = Psd(f, (np.where(np.isfinite(share), share, 1.0)
                       * 1e-2 * 10.0 ** (offset_db / 10.0))[None, :],
                   response_dof=['1Z+'], ordinate_dim=[DENSITY])
    project = hertzforge.Project()
    measured.scale_db = 0
    project.add('Spec', _flat_target())
    project.add('Run', measured)
    found = rows(run(project, 'project.spec_spectra', specification='Spec', spectrum='Run'))
    assert found['Lines outside abort (at most 10 %)']['value'] == 0.0
    if abs(offset_db) == 3.0:
        assert found['Lines outside warning']['value'] == 0.0


def test_a_flagged_line_the_spec_only_partly_reaches_says_so():
    f = np.arange(0.0, 2049.0)
    measured = Psd(f, np.full((1, f.size), 1e-2), response_dof=['1Z+'], ordinate_dim=[DENSITY])
    measured.scale_db = 0
    project = hertzforge.Project()
    project.add('Spec', _flat_target())
    project.add('Run', measured)
    found = rows(run(project, 'project.spec_spectra', specification='Spec', spectrum='Run'))
    assert found['Note']['value'] == (
        'Flagged lines the specification reaches only part of, judged over the '
        'whole bin: 20 Hz (50 %), 2000 Hz (50 %)')
    share = compliance.area_on_lines(np.array([20.0, 2000.0]), np.ones(2), f)
    measured.ordinate = (np.where(np.isfinite(share), share, 1.0) * 1e-2)[None, :]
    found = rows(run(project, 'project.spec_spectra', specification='Spec', spectrum='Run'))
    assert 'Note' not in found


def test_a_power_past_a_limit_by_a_little_is_outside():
    f = np.arange(0.0, 2049.0)
    measured = Psd(f, np.full((1, f.size), 1e-2 * 10.0 ** (6.001 / 10.0)),
                   response_dof=['1Z+'], ordinate_dim=[DENSITY])
    assert compliance.compare(_flat_target(), measured, scale_db=0)['abort_percent'] == 100.0


@pytest.mark.parametrize(('out', 'status'), [(20, 'pass'), (21, 'fail')])
def test_lines_out_at_the_ceiling_reads_the_same_both_ways(out, status):
    f = np.arange(0.0, 201.0)
    levels = np.full((1, 201), 1e-2)
    levels[0, 1:out + 1] = 1.0
    spec = Specification(np.array([0.5, 200.5]), np.array([[1e-2, 1e-2]]),
                         response_dof=['1Z+'], ordinate_dim=[DENSITY],
                         abort_lower=np.array([[2.5e-3] * 2]),
                         abort_upper=np.array([[4e-2] * 2]))
    project = hertzforge.Project()
    project.add('Spec', spec)
    measured = Psd(f, levels, response_dof=['1Z+'], ordinate_dim=[DENSITY])
    measured.scale_db = 0
    project.add('Run', measured)
    spectra = rows(run(project, 'project.spec_spectra', specification='Spec', spectrum='Run'))
    lines_out = rows(run(project, 'project.spec_lines_out', specification='Spec', spectrum='Run'))
    assert spectra['Lines outside abort (at most 10 %)']['status'] == status
    assert lines_out['1Z+']['status'] == status
    assert lines_out['Channels out']['status'] == status


def test_the_reports_lines_out_chart_says_the_ceiling_is_inside():
    from hertzforge.report import _bars_block

    f = np.arange(0.0, 201.0)
    measured = Psd(f, np.full((1, 201), 1e-2), response_dof=['1Z+'], ordinate_dim=[DENSITY])
    spec = Specification(np.array([0.5, 200.5]), np.array([[1e-2, 1e-2]]),
                         response_dof=['1Z+'], ordinate_dim=[DENSITY],
                         abort_lower=np.array([[2.5e-3] * 2]),
                         abort_upper=np.array([[4e-2] * 2]))
    built = _bars_block({'kind': 'bars', 'mode': 'lines', 'caption': 'Lines'}, spec, measured,
                        hertzforge.SI)
    assert built['high'] is None and built['inside_at_ceiling'] is True


def _fresh_record(seconds=4.0, rate=1024.0):
    rng = np.random.default_rng(1)
    n = int(seconds * rate)
    return TimeHistory(np.arange(n) / rate, rng.standard_normal((2, n)),
                       response_dof=['1Z+', '2Z+'], ordinate_dim=['force', 'acceleration'])


def test_compute_psds_on_a_fresh_record_is_averaged():
    project = hertzforge.Project()
    project.add('Run', _fresh_record())
    run(project, 'project.compute_psds', history='Run')
    psd = project['Run PSDs']
    assert project['Run'].averaging is not None
    assert project['Run'].averaging.window == 'hann'
    assert project['Run'].averaging.frames > 1
    assert psd.statistical_dof[0] > 2.0
    assert project.averaging_of('Run PSDs') == project['Run'].averaging


@pytest.mark.parametrize('first', ['compute_frfs', 'compute_multiple_coherence',
                                   'compute_cpsds'])
def test_compute_psds_does_not_depend_on_what_ran_first(first):
    alone = hertzforge.Project()
    alone.add('Run', _fresh_record())
    after = hertzforge.Project()
    after.add('Run', _fresh_record())
    getattr(after, first)('Run')
    a, b = alone[alone.compute_psds('Run')], after[after.compute_psds('Run')]
    assert a.abscissa == pytest.approx(b.abscissa)
    assert np.array_equal(a.ordinate, b.ordinate)
    assert not after.stale()


def test_several_captures_stay_their_own_frames():
    rng = np.random.default_rng(1)
    n, rate = 1024, 1024.0
    record = TimeHistory(np.arange(n) / rate, rng.standard_normal((8, n)),
                         response_dof=['1Z+'] * 8, ordinate_dim=['acceleration'] * 8,
                         block=[str(k) for k in range(8)])
    project = hertzforge.Project()
    project.add('Run', record)
    psd = project[project.compute_psds('Run')]
    assert project['Run'].averaging is None
    assert psd.abscissa[1] == pytest.approx(1.0)
    assert psd.statistical_dof[0] == pytest.approx(16.0)


def _with_a_nan(channel):
    record = _fresh_record(16.0)
    ordinate = np.array(record.ordinate)
    ordinate[channel, 1000] = np.nan
    record = TimeHistory(record.abscissa, ordinate, response_dof=['1Z+', '2Z+'],
                         ordinate_dim=['force', 'acceleration'])
    record.averaging = Averaging(frame_length=1024, overlap=0.5, window='hann').filled(
        ordinate.shape[1], 1024.0)
    project = hertzforge.Project()
    project.add('Run', record)
    return project


@pytest.mark.parametrize(('channel', 'role', 'dof'), [(0, 'reference', '1Z'),
                                                      (1, 'response', '2Z')])
@pytest.mark.parametrize('method', ['H1', 'H2', 'Hv'])
def test_frfs_refuse_a_nan_sample_by_channel(channel, role, dof, method):
    project = _with_a_nan(channel)
    with pytest.raises(ProtocolError, match=f'the {role} channel {dof}'):
        run(project, 'project.compute_frfs', history='Run', method=method)


@pytest.mark.parametrize(('channel', 'role', 'dof'), [(0, 'reference', '1Z'),
                                                      (1, 'response', '2Z')])
def test_multiple_coherence_refuses_a_nan_sample_by_channel(channel, role, dof):
    project = _with_a_nan(channel)
    with pytest.raises(ProtocolError, match=f'the {role} channel {dof}'):
        run(project, 'project.compute_multiple_coherence', source='Run')


def test_compute_spectra_says_it_is_raw_and_claims_no_averaging():
    n, rate = 16384, 1024.0
    t = np.arange(n) / rate
    record = TimeHistory(t, [3.0 * np.sin(2.0 * np.pi * 64.0 * t)], response_dof=['1Z+'],
                         ordinate_dim=['acceleration'])
    record.averaging = Averaging(frame_length=1024, overlap=0.5, window='hann').filled(n, rate)
    project = hertzforge.Project()
    project.add('Run', record)
    (view,) = run(project, 'project.compute_spectra', source='Run')
    assert 'raw FFT' in view['z']['label'] and 'A·N/2' in view['z']['label']
    name = 'Run Spectra'
    assert project[name].ordinate.shape == (1, n // 2 + 1), 'the whole record, not its frames'
    assert project.averaging_of(name) is None
    project['Run'].averaging = Averaging(frame_length=512, overlap=0.5, window='hann').filled(
        n, rate)
    assert name not in project.stale(), 'no change of averaging changes it'


def test_the_amplitude_axis_is_the_chosen_channels_quantity_and_unit():
    project = _force_and_response()
    views = run(project, 'project.amplitude_spectrum', history='Run', record=2)
    assert views[0]['y']['unit'] == 'm/s²'
    assert [line['name'] for line in views[0]['series']] == ['2Z+']
    assert '1 of another quantity left out' in views[0]['title']
    peak_si = float(np.max(views[0]['series'][0]['y']))
    tallest_si = rows(views)['Tallest line']['value']
    display_units.convert(views, display_units.START)
    assert views[0]['y']['unit'] == 'g'
    assert float(np.max(views[0]['series'][0]['y'])) == pytest.approx(peak_si / G)
    assert rows(views)['Tallest line']['value'] == pytest.approx(tallest_si / G)


def _sheet_specification():
    return Specification(np.array([20.0, 100.0, 1000.0]), np.array([[1.0, 1.0, 1.0]]),
                         response_dof=['1Z+'], ordinate_dim=[DENSITY])


def test_the_areas_table_converts_with_the_sheet():
    project = hertzforge.Project()
    project.add('Spec', _sheet_specification())
    (table,) = run(project, 'project.specification_areas', specification='Spec')
    assert table['columns'][2] == 'Area [(m/s²)²]'
    assert table['columns'][4] == 'Running rms [m/s²]'
    assert float(table['rows'][-1][4]) == pytest.approx(np.sqrt(980.0), rel=1e-3)
    display_units.convert([table], display_units.START)
    assert table['columns'][2] == 'Area [g²]'
    assert table['columns'][4] == 'Running rms [g]'
    assert float(table['rows'][-1][4]) == pytest.approx(np.sqrt(980.0) / G, rel=1e-3)
    assert float(table['rows'][0][2]) == pytest.approx(80.0 / G ** 2, rel=1e-3)


def shown(project, op_id, **given):
    """The views as the server sends them in the application's default
    system: words written in it while the operation runs, numbers after."""
    views = display_units.running_in(display_units.START, OPS[op_id].run, project,
                                     _values(project, op_id, given))
    return display_units.convert(views, display_units.START)


def test_a_tones_words_are_in_the_display_system():
    rate, n = 1024.0, 32768
    t = np.arange(n) / rate
    noise = 0.01 * np.random.default_rng(1).standard_normal(n)
    record = TimeHistory(t, [2.0 * np.sin(2.0 * np.pi * 101.0 * t) + noise],
                         response_dof=['1Z+'], ordinate_dim='acceleration')
    record.averaging = Averaging(frame_length=1024, frames=31)
    project = hertzforge.Project()
    project.add('Run', record)
    name = project.compute_psds('Run')
    views = shown(project, 'project.tone_readout', spectrum=name)
    found = rows(views)
    assert found['Tone']['unit'] == 'g'
    assert f"{found['Tone']['value']:.3g} g peak" in views[0]['title']
    assert 'm/s²' not in views[0]['title'] and 'g²/Hz' in views[0]['title']
    si = run(project, 'project.tone_readout', spectrum=name)
    assert 'm/s² peak' in si[0]['title'], 'with no display system, the words stay in SI'


def test_the_transmissibility_title_is_in_the_display_system():
    project = _force_and_response()
    frf = project.compute_frfs('Run', 'H1')
    views = shown(project, 'project.transmissibility', frf=frf, frequency=200.0)
    found = rows(views)
    assert f"H1 {found['H1']['value']:.4g} g/lbf" in views[0]['title']
    assert '(m/s²)/N' not in views[0]['title']


def test_spec_rms_says_its_highest_in_the_display_system():
    project = hertzforge.Project()
    project.add('Spec', _sheet_specification())
    (reading,) = shown(project, 'project.spec_rms', specification='Spec')
    first, last = reading['rows'][0], reading['rows'][-1]
    assert first['unit'] == 'g'
    assert last['value'].endswith(f"highest at {first['value']:.4g} g")


def _plant_and_target():
    f = np.arange(0.0, 1025.0)
    project = hertzforge.Project()
    project.add('Plant', hertzforge.Frf(f, np.full((1, 1025), 2.0 + 0.0j), response_dof=['1Z+'],
                                        reference_dof=['9Z+'],
                                        ordinate_dim=['acceleration/force']))
    project.add('Spec', Specification(np.array([20.0, 1000.0]), np.array([[1.0, 1.0]]),
                                      response_dof=['1Z+'], ordinate_dim=[DENSITY]))
    return project


def test_drive_prediction_says_its_demands_in_the_display_system():
    project = _plant_and_target()
    views = shown(project, 'project.drive_prediction', specification='Spec', frf='Plant')
    words = ' '.join(str(row['value']) for view in views if view.get('view') == 'reading'
                     for row in view['rows'])
    assert ' lbf' in words, words
    assert ' N' not in words, words


def test_a_connection_writes_its_words_in_the_system_it_chose():
    import asyncio
    from pathlib import Path

    from .test_projects import _exchange

    plate = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
    tone = ('op.run', {'op': 'project.tone_readout', 'params': {
        'spectrum': 'Time History PSDs'}})
    responses = asyncio.run(_exchange([
        ('project.import', {'path': str(plate / 'random.nc4')}),
        ('op.run', {'op': 'project.compute_psds', 'params': {}}),
        tone,
        ('units.system', {'name': display_units.DEFAULT}),
        tone,
    ]))
    title = [got[-1]['result']['views'][0]['title'] for got in (responses[2], responses[4])]
    assert ' g peak' in title[0] and 'm/s²' not in title[0]
    assert 'm/s² peak' in title[1]


def test_an_uneven_or_one_sample_history_is_refused_by_the_settings_and_readings():
    a = np.random.default_rng(3).standard_normal((1, 64))
    even = np.arange(64) / 64.0
    project = hertzforge.Project()
    project.add('Uneven', TimeHistory(np.cumsum(np.random.default_rng(1).random(64)), a,
                                      response_dof=['1Z+'], ordinate_dim=['acceleration']))
    project.add('One', TimeHistory(even[:1], a[:, :1], response_dof=['1Z+'],
                                   ordinate_dim=['acceleration']))
    for source in ('Uneven', 'One'):
        with pytest.raises(ProtocolError, match='sample'):
            run(project, 'project.set_averaging', source=source)
        with pytest.raises(ProtocolError, match='sample'):
            run(project, 'project.set_filter', source=source, high=100.0)
        with pytest.raises(ProtocolError, match='sample'):
            run(project, 'project.resample_reading', history=source)


def test_time_statistics_of_two_samples_says_what_it_could_not_read():
    t = np.arange(2) / 1024.0
    project = hertzforge.Project()
    project.add('Two', TimeHistory(t, [[1.0, -1.0]], response_dof=['1Z+'],
                                   ordinate_dim=['acceleration']))
    found = rows(run(project, 'project.time_statistics', history='Two'))
    assert found['Expected largest peak, in sigmas']['value'] == 'not read'
    assert found['Expected largest peak, in sigmas']['status'] == 'warn'
    assert isinstance(found['RMS']['value'], float)


def _one_line_psd():
    return Psd(np.zeros(1), np.ones((1, 1)), response_dof=['1Z+'], ordinate_dim=[DENSITY])


@pytest.mark.parametrize('op_id', ['project.spec_spectra', 'project.spec_rms_error',
                                   'project.spec_lines_out', 'project.spec_chance',
                                   'project.comparison_scale'])
def test_a_one_line_psd_is_not_a_failure_inside_the_comparison(op_id):
    project = hertzforge.Project()
    project.add('Spec', _flat_target())
    project.add('Run', _one_line_psd())
    try:
        run(project, op_id, specification='Spec', spectrum='Run')
    except ProtocolError:
        pass


def test_a_scale_cannot_be_read_from_a_spectrum_that_cannot_be_banded():
    from hertzforge.core import compliance

    assert compliance.read_scale_db(_flat_target(), _one_line_psd()) is None


@pytest.mark.parametrize('levels', [([20.0], [[1e-3]]), ([20.0, 500.0], [[0.0, 0.0]])],
                         ids=['one point', 'all zero'])
def test_a_matrix_comparison_against_nothing_written_is_refused(levels):
    at, level = levels
    project = _force_and_acceleration_at_one_dof()
    project.add('Spec', Specification(np.array(at), np.array(level), response_dof=['4Z+'],
                                      ordinate_dim=[DENSITY]))
    cpsd = project.compute_cpsds('Run')
    with pytest.raises(ProtocolError):
        run(project, 'project.matrix_comparison', specification='Spec', cpsd=cpsd)
