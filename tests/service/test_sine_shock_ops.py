"""The sine and shock operations through the service, as `server.py` runs
them: `op.resolve`, then `op.run`, with the verbs the shell is offered."""

from __future__ import annotations

import warnings

import numpy as np
import pytest

from hertzforge.core import classical_pulse
from hertzforge.core.averaging import Averaging
from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.data import ShockSpecification, TimeHistory, TransientSpecification
from hertzforge.core.mixed_mode import tone_mean_square
from hertzforge.core.shocks import Shock, find
from hertzforge.core.sine import LINEAR, LOG, SineSweepSpecification, SineTone
from hertzforge.project import Project
from hertzforge.service import action_table, actions, projects, server
from hertzforge.service.protocol import ProtocolError
from hertzforge.service.show import show

from .conftest import wire_views

OPS = {op.id: op for op in server.OPERATIONS}
G = 9.80665
RATE = 4096.0


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return op.run(project, values)


def reading(views):
    return next(view for view in views if view['view'] == 'reading')


def rows_of(view):
    out = {}
    for row in view['rows']:
        out.setdefault(row['label'], row)
    return out


def history(t, rows, dofs=None, unit='m/s**2', dim='acceleration'):
    rows = np.atleast_2d(rows)
    dofs = dofs or [f'{k + 1}X+' for k in range(len(rows))]
    return TimeHistory(t, rows, response_dof=dofs, ordinate_dim=[dim] * len(rows),
                       ordinate_unit=[unit] * len(rows))


def stations(stage):
    """{label: (x, time, amplitude)} of a stage's records."""
    out = {}
    offsets = stage['offsets']
    for k, label in enumerate(stage['records']):
        a, b = offsets[k], offsets[k + 1]
        out[label] = (np.asarray(stage['x_values'][a:b]), np.asarray(stage['y_values'][a:b]),
                      np.asarray(stage['z_values'][a:b]))
    return out


def _tone_record(tone, row, lead=1.0, tail=1.0):
    """`row(argument)` with `lead` and `tail` seconds of quiet about it."""
    argument = tone.argument(1.0 / RATE)
    start = int(lead * RATE)
    rows = np.atleast_2d(row(argument))
    x = np.zeros((len(rows), start + rows.shape[1] + int(tail * RATE)))
    x[:, start:start + rows.shape[1]] = rows
    return np.arange(x.shape[1]) / RATE, x


def offered(project, name):
    """The ids of the rows the action table offers on `name`."""
    table = action_table.action_table(project, [name], {})
    return {row['id'] for row in table['columns'][0]['rows']}


# ---- the stage ------------------------------------------------------------------------------

def test_the_stage_draws_a_channel_in_quadrature_at_its_magnitude():
    amplitude = np.array([[10.0, 10.0]] * 2)
    tone = SineTone('T', 0.0, [100.0, 300.0], amplitude, [LINEAR], [50.0],
                    abort_lower=amplitude * 0.7, abort_upper=amplitude * 1.4)
    spec = SineSweepSpecification([tone], ['1Z+', '2Z+'], ordinate_unit='m/s**2')
    t, x = _tone_record(tone, lambda a: 10.0 * np.vstack([np.cos(a), np.cos(a + np.pi / 2)]))
    project = Project()
    project.add('R', history(t, x, ['1Z+', '2Z+']))
    project.add('S', spec)
    project.extract_sine('R')
    with warnings.catch_warnings():
        warnings.simplefilter('error', np.exceptions.ComplexWarning)
        (stage,) = show(project, ['S', 'Sine Levels'], {}, {})['views']
    drawn = stations(stage)
    assert np.median(drawn['T level, 1Z+'][2]) == pytest.approx(10.0, rel=1e-3)
    assert np.median(drawn['T level, 2Z+'][2]) == pytest.approx(10.0, rel=1e-3)
    assert not stage.get('exceed')


def _up_and_back():
    amplitude = np.array([[10.0], [10.0], [2.5]])
    tone = SineTone('UD', 0.0, [100.0, 300.0, 100.0], amplitude, [LINEAR, LINEAR],
                    [50.0, -50.0], abort_lower=amplitude * 0.7, abort_upper=amplitude * 1.4)
    spec = SineSweepSpecification([tone], ['1Z+'], ordinate_unit='m/s**2')
    corners = np.array([0.0, 4.0, 8.0])

    def row(argument):
        seconds = np.arange(argument.size) / RATE
        return np.interp(seconds, corners, amplitude[:, 0]) * np.cos(argument)

    t, x = _tone_record(tone, row)
    project = Project()
    project.add('R', history(t, x, ['1Z+']))
    project.add('S', spec)
    project.extract_sine('R')
    return project, tone


def test_an_up_and_back_sweep_with_two_levels_passes_level_error_against_itself():
    project, _tone = _up_and_back()
    judged = rows_of(reading(run(project, 'project.level_error', specification='S',
                                 levels='Sine Levels')))['1Z+']
    assert judged['status'] == 'pass'
    assert abs(judged['value']) < 0.1


def test_the_stage_marks_no_line_of_an_up_and_back_sweep_that_met_its_requirement():
    project, _tone = _up_and_back()
    (stage,) = show(project, ['S', 'Sine Levels'], {}, {})['views']
    assert not stage.get('exceed')


def test_the_stage_draws_each_pass_of_the_requirement_at_its_own_level():
    project, _tone = _up_and_back()
    (stage,) = show(project, ['S'], {}, {})['views']
    x, t, z = stations(stage)['UD requirement']
    assert z[np.argmin(np.abs(t - 1.0))] == pytest.approx(10.0)
    assert z[np.argmin(np.abs(t - 7.0))] == pytest.approx(4.375, rel=1e-2)
    assert x[np.argmin(np.abs(t - 1.0))] == pytest.approx(150.0, rel=1e-2)


def test_the_stage_draws_a_level_along_its_path_in_time():
    project, _tone = _up_and_back()
    (stage,) = show(project, ['S', 'Sine Levels'], {}, {})['views']
    x, t, _z = stations(stage)['UD level']
    assert np.all(np.diff(t) > 0.0)
    assert x.max() > 290.0 and x[0] < 110.0 and x[-1] < 110.0


def test_a_tones_mean_square_counts_each_pass_at_its_own_level():
    _project, tone = _up_and_back()
    # the up pass holds 10, the way back runs 10 to 2.5: the time average of
    # a squared ramp is (a² + ab + b²)/3
    down = (10.0 ** 2 + 10.0 * 2.5 + 2.5 ** 2) / 3.0
    assert tone_mean_square(tone) == pytest.approx(0.5 * (100.0 + down) / 2.0, rel=1e-2)


# ---- Extract Sine, Generate -----------------------------------------------------------------

def _one_tone_project():
    tone = SineTone('T', 0.0, [100.0, 300.0], [[10.0]] * 2, [LINEAR], [50.0])
    t, x = _tone_record(tone, lambda a: 10.0 * np.cos(a))
    project = Project()
    project.add('R', history(t, x, ['1Z+']))
    project.add('S', SineSweepSpecification([tone], ['1Z+'], ordinate_unit='m/s**2'))
    return project


def test_extract_sine_draws_the_levels_it_added(validate):
    project = _one_tone_project()
    views = run(project, 'project.extract_sine', source='R')
    assert sorted(project.names) == ['R', 'S', 'Sine Levels']
    wire_views(views, validate)
    (stage,) = views
    assert stage['view'] == 'waterfall'
    assert np.median(stations(stage)['T level'][2]) == pytest.approx(10.0, rel=1e-3)


def test_a_sine_specification_of_undeclared_units_generates_nothing():
    project = Project()
    tone = SineTone('T', 0.0, [100.0, 300.0], [[2.0]] * 2, [LINEAR], [50.0])
    project.add('Un', SineSweepSpecification([tone], ['1Z+']))
    with pytest.raises(ProtocolError, match='declare its units'):
        run(project, 'project.generate_signal', source='Un', kind='sine', sample_rate=4096.0)
    assert project.names == ['Un']


def _requirement():
    return ShockSpecification.from_breakpoints([20.0, 500.0], [5.0, 10.0], '1X+',
                                               ordinate_unit='g', warning=(-1.5, 3.0),
                                               abort=(-3.0, 6.0))


def test_generate_is_not_offered_on_a_shock_specification():
    project = Project()
    project.add('Req', _requirement())
    assert 'generate_signal' not in projects.verbs_of(project, 'Req')
    assert 'project.generate_signal' not in offered(project, 'Req')
    assert 'project.srs_synthesis' in offered(project, 'Req')
    with pytest.raises(ProtocolError):
        run(project, 'project.generate_signal', source='Req')


@pytest.mark.parametrize('frequency,law,rate', [
    ([0.0, 100.0], LINEAR, 10.0), ([0.0, 100.0], LOG, 1.0), ([-10.0, 100.0], LINEAR, 10.0),
    ([100.0, 0.0], LINEAR, -10.0), ([float('nan'), 100.0], LINEAR, 10.0)])
def test_a_sweep_from_zero_hertz_or_below_is_refused(frequency, law, rate):
    with pytest.raises(ValueError, match='above 0 Hz'):
        SineTone('Z', 0.0, frequency, [[1.0]] * 2, [law], [rate])


def test_a_file_holding_a_sweep_from_zero_hertz_still_reads():
    tone = SineTone('Z', 0.0, [0.0, 100.0], [[1.0]] * 2, [LINEAR], [10.0], from_file=True)
    assert tone.duration() == pytest.approx(10.0)


@pytest.mark.parametrize('op_id,given', [
    ('project.sine_extraction', {'history': 'R', 'specification': 'E'}),
    ('project.sine_profile', {'specification': 'E'}),
    ('project.sweep_rate', {'specification': 'E', 'modes': '100 10'}),
    ('project.extract_sine', {'source': 'R'}),
    ('project.generate_signal', {'source': 'E', 'kind': 'sine', 'sample_rate': 4096.0}),
])
def test_a_specification_with_no_tone_is_refused_in_words(op_id, given):
    empty = SineSweepSpecification([], ['1Z+'], ordinate_unit='m/s**2')
    assert '0 tones' in repr(empty)
    project = Project()
    t = np.arange(8192) / RATE
    project.add('R', history(t, np.cos(t), ['1Z+']))
    project.add('E', empty)
    with pytest.raises(ProtocolError, match='no tone|0 tones') as refusal:
        run(project, op_id, **given)
    assert 'iterable' not in str(refusal.value)


# ---- showing a requirement ------------------------------------------------------------------

def _shock_record():
    t = np.arange(int(RATE)) / RATE
    y = np.where((t > 0.2) & (t < 0.211), 100.0 * np.sin(np.pi * (t - 0.2) / 0.011), 0.0)
    return t, y


def test_a_shock_or_transient_specification_shows_alone(validate):
    t, y = _shock_record()
    project = Project()
    project.add('H', history(t, y))
    project.add('Req', _requirement())
    project.add('T', TransientSpecification(t, y[None], response_dof=['1X+'],
                                            ordinate_dim=['acceleration'],
                                            ordinate_unit=['m/s**2']))
    made = project.compute_srs('T')
    for name in ('Req', 'T', made):
        (view,) = show(project, [name], {}, {})['views']
        assert view['view'] == 'waterfall', name
        wire_views([view], validate)
    run(project, 'project.plot', data='Req')
    assert show(project, ['Req', made], {}, {})['views']


# ---- the velocity check ---------------------------------------------------------------------

def _pulse(offset=0.0):
    """A half-sine and its opposite, so the velocity comes back to rest; a
    zero shift of `offset` from the first on."""
    t = np.arange(int(2 * RATE)) / RATE
    up = np.where((t > 0.5) & (t < 0.511), 100.0 * np.sin(np.pi * (t - 0.5) / 0.011), 0.0)
    down = np.where((t > 0.52) & (t < 0.531), -100.0 * np.sin(np.pi * (t - 0.52) / 0.011), 0.0)
    return t, up + down + np.where(t > 0.5, offset, 0.0)


def _check(views):
    return [row for row in reading(views)['rows'] if row['label'] == 'Velocity check']


def test_shocks_read_a_velocity_that_returns_to_rest():
    t, y = _pulse()
    project = Project()
    project.add('H', history(t, y))
    project['H'].shocks = (Shock(0.4, 0.5),)
    (row,) = _check(run(project, 'project.shocks', history='H'))
    assert row['status'] == 'pass'
    assert row['value'] == 'Velocity returns to rest on every channel.'


def test_shocks_say_when_a_zero_shift_keeps_the_velocity_moving():
    t, y = _pulse(offset=4.0 * G)
    project = Project()
    project.add('H', history(t, y))
    project['H'].shocks = (Shock(0.4, 0.5),)
    first, *rest = _check(run(project, 'project.shocks', history='H'))
    assert first['status'] == 'warn'
    assert first['value'] == '1 of 1 do not return to rest.'
    assert 'still changing' in rest[0]['value']


def test_the_velocity_check_follows_a_window_as_it_is_set():
    t, y = _pulse(offset=4.0 * G)
    project = Project()
    project.add('H', history(t, y))
    run(project, 'project.set_shock', history='H', shock=1, start=0.4, stop=0.9)
    first, *_rest = _check(run(project, 'project.shocks', history='H'))
    assert first['status'] == 'warn'
    (shifted,) = _check(run(project, 'project.set_shock', history='H', shock=1, start=0.1,
                            stop=0.4))
    assert shifted['status'] == 'pass'


def test_the_velocity_check_waits_for_the_windows_it_speaks_for():
    t, y = _pulse()
    project = Project()
    project.add('H', history(t, y))
    (row,) = _check(run(project, 'project.shocks', history='H'))
    assert 'waits' in row['value'] and row['status'] == 'info'


def test_a_record_with_no_acceleration_has_no_velocity_check():
    t, y = _pulse()
    project = Project()
    project.add('H', history(t, y, unit='N', dim='force'))
    assert not _check(run(project, 'project.shocks', history='H'))


def test_the_velocity_check_says_when_a_window_cannot_be_read():
    t, y = _pulse()
    y[int(0.45 * RATE)] = np.nan
    project = Project()
    project.add('H', history(t, y))
    project['H'].shocks = (Shock(0.4, 0.5),)
    (row,) = _check(run(project, 'project.shocks', history='H'))
    assert row['status'] == 'warn' and 'cannot be read' in row['value']


# ---- a sample that is not a number ----------------------------------------------------------

def _three_shocks():
    fs = 20480.0
    t = np.arange(int(4 * fs)) / fs
    x = sum(np.where((t >= t0) & (t <= t0 + 0.011), a * np.sin(np.pi * (t - t0) / 0.011), 0.0)
            for t0, a in ((0.5, 50 * G), (1.5, 25 * G), (2.5, 100 * G)))
    x = x + 0.05 * np.random.default_rng(0).standard_normal(t.size)
    return t, x


def test_one_nan_stops_detection_with_its_sample_named():
    t, x = _three_shocks()
    x[100] = np.nan
    project = Project()
    project.add('R', history(t, x))
    with pytest.raises(ValueError, match=r'1X\+ holds 1 sample .*index 100'):
        find(project['R'])
    with pytest.raises(ProtocolError, match='index 100'):
        run(project, 'project.detect_shocks', source='R')


def test_compute_srs_refuses_a_nan_in_a_window_and_adds_nothing():
    t, x = _three_shocks()
    x[100] = np.nan
    project = Project()
    project.add('R', history(t, x))
    with pytest.raises(ProtocolError, match='index 100'):
        run(project, 'project.compute_srs', source='R')
    assert project.names == ['R']
    project['R'].shocks = (Shock(0.0, 0.1),)
    with pytest.raises(ProtocolError, match='index 100'):
        run(project, 'project.compute_srs', source='R')


def test_a_nan_outside_the_shock_windows_does_not_stop_the_spectrum():
    t, x = _three_shocks()
    x[100] = np.nan
    project = Project()
    project.add('R', history(t, x))
    project['R'].shocks = (Shock(0.4, 0.5),)
    run(project, 'project.compute_srs', source='R')
    assert np.isfinite(project['R SRS'].ordinate).all()


# ---- a window past the record ---------------------------------------------------------------

def test_a_shock_window_past_the_records_end_is_refused():
    t, y = _pulse()
    project = Project()
    project.add('H', history(t, y))
    with pytest.raises(ProtocolError, match='ends at 2 s'):
        run(project, 'project.set_shock', history='H', shock=1, start=50.0, stop=60.0)
    assert not project['H'].shocks


def test_a_window_running_off_the_end_is_held_to_the_record():
    t, y = _pulse()
    project = Project()
    project.add('H', history(t, y))
    run(project, 'project.set_shock', history='H', shock=1, start=1.5, stop=60.0)
    assert project['H'].shocks[0].stop == pytest.approx(2.0)


def test_a_window_too_short_for_a_spectrum_is_refused_by_its_length():
    t, y = _pulse()
    project = Project()
    project.add('H', history(t, y))
    project['H'].shocks = (Shock(1.0, 2.0 / RATE),)
    with pytest.raises(ProtocolError, match='shortest window is .* s long') as refusal:
        run(project, 'project.compute_srs', source='H')
    assert 'octave band' not in str(refusal.value)


def test_detect_is_refused_on_a_record_a_file_cut_into_captures():
    t, y = _pulse()
    captured = history(t, np.vstack([y, 2.0 * y, y]), ['1X+', '1X+', '2X+'])
    captured.averaging = Averaging(frame_length=len(t), overlap=0.0, window='boxcar', frames=1)
    assert captured.split_into_frames
    project = Project()
    project.add('H', captured)
    with pytest.raises(ProtocolError, match='settled'):
        run(project, 'project.detect_shocks', source='H')
    assert not project['H'].shocks


# ---- Pulse Check, Stroke, SRS Synthesis ------------------------------------------------------

@pytest.mark.parametrize('shape,edge', [('sawtooth', 0.0005), ('trapezoid', 0.001)])
def test_pulse_check_opens_on_the_shape_it_was_asked_for(shape, edge):
    fs = 50000.0
    t = np.arange(int(0.3 * fs)) / fs
    x = classical_pulse.ClassicalPulse(shape, 50 * G, 0.011, edge).at(t - 0.1)
    project = Project()
    project.add('R', history(t, x))
    found = rows_of(reading(run(project, 'project.pulse_check', history='R', shape=shape)))
    assert found['Pulse']['value'].split(', ')[1] == '11 ms'


def _half_sine_project():
    fs = 50000.0
    t = np.arange(int(0.3 * fs)) / fs
    project = Project()
    project.add('R', history(t, classical_pulse.ClassicalPulse('half-sine', 50 * G, 0.011)
                             .at(t - 0.1)))
    return project


@pytest.mark.parametrize('shape', ['trapezoid', 'sawtooth'])
def test_an_edge_of_zero_means_one_thing_in_pulse_check_and_generate(shape):
    table = ChannelTable({'channel': [1], 'node': ['1'], 'direction': ['X+'], 'unit': ['g'],
                          'role': ['reference'], 'channel_type': ['acceleration']})
    project = Project()
    project.add('Table', table)
    run(project, 'project.generate_signal', source='Table', kind='pulse', shape=shape, peak=50.0,
        pulse_duration=0.01, sample_rate=20480.0, duration=0.1, at=0.01)
    made = project.provenance[project.names[-1]]['params']
    assert made['edge'] == pytest.approx(0.0005)
    labels = {next(p for p in OPS[op_id].params if p.name == 'edge').label
              for op_id in ('project.generate_signal', 'project.pulse_check')}
    assert labels == {'Edge, 0 for a twentieth of the duration'}


def test_a_force_rating_with_no_moving_mass_says_so():
    project = _half_sine_project()
    found = rows_of(reading(run(project, 'project.stroke', history='R', force=30000.0)))
    assert 'moving mass' in found['Against the rating']['value']
    assert found['Against the rating']['status'] == 'warn'
    assert 'moving mass' in found['Verdict']['value']


def test_a_force_rating_with_no_moving_mass_is_said_beside_a_judged_rating():
    project = _half_sine_project()
    views = run(project, 'project.stroke', history='R', force=30000.0, velocity=100.0)
    said = [row['value'] for row in reading(views)['rows'] if row['label'] == 'Against the rating']
    assert any('moving mass' in text for text in said)
    assert any(' against ' in text for text in said)


def test_no_rating_at_all_still_says_no_ratings():
    project = _half_sine_project()
    found = rows_of(reading(run(project, 'project.stroke', history='R', moving_mass=45.0)))
    assert found['Verdict']['value'] == 'no ratings given'


def test_a_synthesis_refusal_speaks_in_the_units_of_the_rows():
    spec = ShockSpecification.from_breakpoints([50.0, 400.0, 2000.0], [20.0, 100.0, 100.0], '1Z+',
                                               ordinate_unit='g', per_octave=6,
                                               warning=(-1.5, 3.0), abort=(-3.0, 6.0))
    project = Project()
    project.add('Req', spec)
    with pytest.raises(ProtocolError, match=r'against 100 N') as refusal:
        run(project, 'project.srs_synthesis', requirement='Req', force=100.0, moving_mass=45.0)
    assert 'lbf' not in str(refusal.value)
    with pytest.raises(ProtocolError, match=r'against 0\.01 m/s') as refusal:
        run(project, 'project.srs_synthesis', requirement='Req', velocity=0.01)
    assert 'in/s' not in str(refusal.value)


# ---- counts and stale reasons ----------------------------------------------------------------

def _srs_against_requirement(q=25.0):
    t, y = _shock_record()
    project = Project()
    project.add('H', history(t, y))
    project.add('Req', _requirement())
    return project, project.compute_srs('H', q=q)


def test_a_count_with_no_channel_scored_is_no_pass():
    project, made = _srs_against_requirement()
    found = rows_of(reading(run(project, 'project.srs_error', srs=made, requirement='Req')))
    assert found['Channels out']['status'] == 'warn'
    assert found['Channels out']['value'].startswith('no channels scored; 1 not scored')


def test_a_scored_count_still_judges():
    project, made = _srs_against_requirement(q=10.0)
    found = rows_of(reading(run(project, 'project.srs_error', srs=made, requirement='Req')))
    assert found['Channels out']['status'] in ('pass', 'fail')
    assert ' of 1 channels ' in found['Channels out']['value']


def test_waveform_error_counts_readings_when_there_are_many_events():
    fs, n, playings = 4096.0, 4096, 6
    t = np.arange(n) / fs
    y = np.exp(-(((t - 0.5) / 0.1) ** 2)) * (10.0 * np.sin(2 * np.pi * 50.0 * t)
                                             + 5.0 * np.sin(2 * np.pi * 200.0 * t))
    project = Project()
    project.add('Run', history(np.arange(n * playings) / fs, np.tile(y, playings)))
    project.add('Ref', TransientSpecification(t, y[None], response_dof=['1X+'],
                                              ordinate_dim=['acceleration'],
                                              ordinate_unit=['m/s**2']))
    found = rows_of(reading(run(project, 'project.waveform_error', history='Run',
                                reference='Ref')))
    assert 'Channels out' not in found
    assert found['Readings out']['value'].startswith('0 of 6 readings ')


def test_a_spectrum_stale_because_a_window_moved_says_so():
    t, y = _pulse()
    project = Project()
    project.add('H', history(t, y))
    run(project, 'project.set_shock', history='H', shock=1, start=0.4, stop=0.9)
    made = project.compute_srs('H')
    run(project, 'project.set_shock', history='H', shock=1, start=0.45, stop=0.9)
    assert project.stale()[made] == 'computed before its shock windows moved'


def test_a_spectrum_stale_because_a_window_was_added_counts_them():
    t, y = _pulse()
    project = Project()
    project.add('H', history(t, y))
    run(project, 'project.set_shock', history='H', shock=1, start=0.4, stop=0.9)
    made = project.compute_srs('H')
    run(project, 'project.set_shock', history='H', shock=2, start=1.0, stop=1.5)
    assert project.stale()[made] == 'computed with 1 window; the recording now says 2 windows'


# ---- what is listed ------------------------------------------------------------------------

def test_every_action_named_is_an_operation():
    assert actions.ADDS <= set(OPS), sorted(actions.ADDS - set(OPS))


def test_no_operation_shares_a_kind_of_data_twice():
    from hertzforge.service.ops_geometry import RENAMED

    assert len(set(RENAMED)) == len(RENAMED)
