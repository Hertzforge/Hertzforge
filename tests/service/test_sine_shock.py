"""Sine and shock readings of a time history: the sine extraction's
settings, the levels against their specification, the distortion,
frequency, dwell and mixed-mode readouts, the pulse check, the stroke,
the replication fit, the scalogram and the shock windows.

The records are built here as the core's own tests build them: a 2 m/s²
sweep from 100 to 500 Hz with a third harmonic at a tenth of it
(`tests/test_sine_distortion.py`), a 1 g dwell at 20 Hz through a Q 30
resonator (`tests/test_dwell.py`), a 1 grms floor with two tones
(`tests/test_mixed_mode.py`) and a 50 g, 11 ms half-sine.
"""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

from hertzforge.core.data import TimeHistory
from hertzforge.core.sine import SineSweepSpecification, SineTone
from hertzforge.project import Project
from hertzforge.service import projects
from hertzforge.service.ops_sine_shock import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .test_ops import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in OPERATIONS}
G = 9.80665


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()})
    return op.run(project, values)


def rows(reading):
    return {row['label']: row for row in reading['rows']}


def history(t, rows_, dofs, dims=None):
    dims = dims or ['acceleration'] * len(dofs)
    return TimeHistory(t, np.atleast_2d(rows_), response_dof=dofs, ordinate_dim=dims,
                       ordinate_unit=['m/s**2' if d == 'acceleration' else 'N' for d in dims])


def test_the_descriptors_validate(validate):
    for op in OPERATIONS:
        validate(op.to_json(), 'operation')


# ---- the sweep ------------------------------------------------------------------------------

def _sweep(amplitude=2.0):
    """A 2 m/s² sweep at 101Z+ from 100 to 500 Hz at 100 Hz/s, its third
    harmonic at a tenth of it, and the specification it was run to, at
    `amplitude`."""
    rate, control = 4096.0, '101Z+'
    up = SineTone('Up', 0.5, [100.0, 500.0], [[2.0]] * 2, [0], [100.0])
    written = SineTone('Up', 0.5, [100.0, 500.0], [[amplitude]] * 2, [0], [100.0])
    n = int(6 * rate)
    x = np.zeros(n)
    argument = up.argument(1.0 / rate)
    start = int(0.5 * rate)
    m = min(len(argument), n - start)
    x[start:start + m] = 2.0 * (np.cos(argument[:m]) + 0.1 * np.cos(3.0 * argument[:m]))
    project = Project()
    project.add('Sweep', history(np.arange(n) / rate, x, [control]))
    project.add('Profile', SineSweepSpecification([written], [control],
                                                  ordinate_unit='m/s**2'))
    return project


@pytest.fixture(scope='module')
def swept():
    project = _sweep()
    project.extract_sine('Sweep')
    return project


def test_the_sine_settings_are_stored_and_read_at_their_smoothing(validate):
    project = _sweep()
    view, reading = run(project, 'project.sine_extraction')
    wire_views([view, reading], validate)
    found = rows(reading)
    chosen = project['Sweep'].sine_extraction.chosen
    assert found['Cycles used']['value'] == chosen == 10.0
    assert found['Predicted scatter']['value'] < 0.1, 'a clean sweep scatters nothing'
    _view, reading = run(project, 'project.sine_extraction', smoothing='set', cycles=40.0)
    assert rows(reading)['Window, start to end']['value'] == '0.40 s to 0.08 s'
    assert project['Sweep'].sine_extraction.cycles == 40.0


def test_a_specification_at_a_channel_the_record_lacks_is_refused():
    project = _sweep()
    tone = SineTone('Up', 0.5, [100.0, 500.0], [[2.0]] * 2, [0], [100.0])
    project.add('Elsewhere', SineSweepSpecification([tone], ['9Z+'], ordinate_unit='m/s**2'))
    with pytest.raises(ProtocolError) as error:
        run(project, 'project.sine_extraction', specification='Elsewhere')
    assert error.value.code == 'refused'


def test_levels_read_their_error_against_the_tone(swept, validate):
    levels, reading = run(swept, 'project.level_error')
    wire_views([levels, reading], validate)
    assert [s['role'] for s in levels['series']] == ['measured', 'reference']
    found = rows(reading)['101Z+']
    assert found['value'] == pytest.approx(0.0, abs=0.05) and found['status'] == 'pass'


def test_levels_run_at_half_their_tone_read_six_decibels_under():
    project = _sweep(amplitude=4.0)
    project.extract_sine('Sweep')
    _levels, reading = run(project, 'project.level_error')
    found = rows(reading)['101Z+']
    assert found['value'] == pytest.approx(-6.02, abs=0.1) and found['status'] == 'fail'
    with pytest.raises(ProtocolError, match='there is no tone 2'):
        run(project, 'project.level_error', tone=2)


def test_a_tenth_harmonic_reads_a_ten_percent_distortion(swept, validate):
    view, reading = run(swept, 'project.sine_distortion')
    wire_views([view, reading], validate)
    found = rows(reading)
    assert found['Worst']['value'].startswith('10.0 %')
    assert found['Verdict']['value'] == 'within the limit'
    assert view['series'][1]['y'][0] == pytest.approx(32.02, abs=0.01)
    _view, reading = run(swept, 'project.sine_distortion', excess=0.1)
    assert rows(reading)['Verdict']['status'] == 'fail'


def test_a_readout_before_the_levels_are_extracted_is_refused():
    with pytest.raises(ProtocolError, match='Extract Sine reads them'):
        run(_sweep(), 'project.sine_distortion')


def test_the_tone_was_at_its_frequency(swept, validate):
    view, reading = run(swept, 'project.sine_frequency')
    wire_views([view, reading], validate)
    found = rows(reading)
    assert found['Band']['value'] == '±2 %'
    assert found['Verdict']['value'] == 'within the band'
    assert np.max(np.abs(view['series'][0]['y'])) < 0.01
    with pytest.raises(ProtocolError, match='there is no control channel 2'):
        run(swept, 'project.sine_frequency', record=2)


# ---- the dwell ------------------------------------------------------------------------------

def test_a_dwell_builds_to_95_percent_in_about_q_cycles(validate):
    from scipy.signal import lfilter

    from hertzforge.core.srs import ramp_invariant

    rate, fn, q = 4000.0, 20.0, 30.0
    t = np.arange(150 * 200) / rate
    x = np.sin(2.0 * np.pi * fn * t) * (t < 90 / fn - 1e-12)
    b, a = ramp_invariant([fn], rate, 1.0 / (2.0 * q))
    project = Project()
    project.add('Run', history(t, np.vstack([x, lfilter(b[0], a[0], x)]) * G,
                               ['1Z+', '101Z+']))
    project.add('Profile', SineSweepSpecification([SineTone.dwell('Dwell', fn, [G], 90 / fn)],
                                                  ['1Z+'], ordinate_unit='m/s**2'))
    view, reading = run(project, 'project.dwell')
    wire_views([view, reading], validate)
    found = rows(reading)
    assert found['Channel']['value'] == '101Z+', 'the response, by default'
    assert found['Steady']['value'] / G == pytest.approx(30.0, abs=0.05)
    assert found['To 95 %']['value'].startswith('28.6 cycles')
    sweep = SineTone('Up', 0.0, [10.0, 50.0], [[G]] * 2, [0], [10.0])
    project.add('Swept', SineSweepSpecification([sweep], ['1Z+'], ordinate_unit='m/s**2'))
    with pytest.raises(ProtocolError, match='holds no dwell'):
        run(project, 'project.dwell', specification='Swept')


# ---- the mixed-mode data sheet ----------------------------------------------------------------

def test_a_mixed_record_reads_its_floor_and_tones(validate):
    from hertzforge.core.data import Specification
    from hertzforge.core.mixed_mode import Tone

    rate, seconds, tones = 1024.0, 10.0, ((19.44, 1.944), (38.88, 2.5))
    n = int(seconds * rate)
    spectrum = np.fft.rfft(np.random.default_rng(33).standard_normal(n))
    f = np.fft.rfftfreq(n, 1.0 / rate)
    spectrum[(f < 3.0) | (f > 480.0)] = 0.0
    x = np.fft.irfft(spectrum, n)
    x *= np.sqrt(477.0 / 395.0) / np.sqrt(np.mean(x ** 2))
    t = np.arange(n) / rate
    for hz, amplitude in tones:
        x = x + amplitude * np.sin(2.0 * np.pi * hz * t + hz)
    project = Project()
    project.add('Run', history(t, x * G, ['1Z+']))
    project.add('Floor', Specification(np.array([5.0, 400.0]),
                                       np.array([[1.0, 1.0]]) / 395.0 * G ** 2,
                                       response_dof=['1Z+'],
                                       ordinate_dim=['acceleration**2/frequency']))
    project.add('Tones', SineSweepSpecification(
        [Tone(hz, amplitude * G).sine_tone(seconds) for hz, amplitude in tones], ['1Z+'],
        ordinate_unit='m/s**2'))
    view, reading = run(project, 'project.mixed_mode')
    wire_views([view, reading], validate)
    found = rows(reading)
    assert found['Reading']['status'] == 'pass'
    assert [r['kind'] for r in view['regions']] == ['cut', 'cut'], 'one left out per tone'
    assert found['Floor, tone and band lines left out']['status'] == 'pass'
    assert 'written 24.5 m/s² peak' in found['Tone 38.88 Hz']['value']
    project.add('Elsewhere', Specification(np.array([5.0, 400.0]), np.ones((1, 2)),
                                           response_dof=['9Z+'],
                                           ordinate_dim=['acceleration**2/frequency']))
    with pytest.raises(ProtocolError, match='written at no channel Run holds'):
        run(project, 'project.mixed_mode', floor='Elsewhere')


# ---- a shock ------------------------------------------------------------------------------

RATE = 10240.0


def _shock():
    """A 50 g, 11 ms half-sine 50 ms into a 0.2 s record, and a force
    channel beside it."""
    t = np.arange(int(0.2 * RATE)) / RATE
    pulse = 50.0 * G * np.sin(np.pi * (t - 0.05) / 0.011) * ((t >= 0.05) & (t <= 0.061))
    project = Project()
    project.add('Shock', history(t, [pulse, pulse], ['1Z+', '1Z-'],
                                 ['acceleration', 'force']))
    project.add('Force', history(t, pulse, ['2Z+'], ['force']))
    return project


def test_a_half_sine_passes_its_pulse_check(validate):
    project = _shock()
    record, velocity, reading = run(project, 'project.pulse_check')
    wire_views([record, velocity, reading], validate)
    found = rows(reading)
    assert found['Pulse']['value'] == '490 m/s², 11 ms half-sine', 'the suggested pulse'
    assert found['Outside']['value'] == f'0 of {len(record["series"][3]["x"])}'
    assert found['Verdict']['status'] == 'pass'
    assert record['regions'][0]['label'] == 'velocity window'
    _record, _velocity, reading = run(project, 'project.pulse_check', peak=70.0 * G,
                                      duration=0.011)
    assert rows(reading)['Peak']['status'] == 'fail'
    with pytest.raises(ProtocolError, match='integrates acceleration'):
        run(project, 'project.pulse_check', history='Force')


def test_a_compensated_pulse_fits_its_shaker(validate):
    from hertzforge.core.stroke import compensated_half_sine

    project = _shock()
    project.add('Nominal', compensated_half_sine(50.0, 0.011, RATE, unit='g'))
    *views, reading = run(project, 'project.stroke', history='Nominal', stroke=0.0635,
                          velocity=1.778, moving_mass=45.4, body_mass=1361.0)
    wire_views([*views, reading], validate)
    found = rows(reading)
    assert found['Peak']['value'] / G == pytest.approx(50.0, rel=1e-3)
    assert found['Verdict']['value'] == 'fits'
    assert [s['name'] for s in views[2]['series']] == ['table', 'against the body', 'stop',
                                                       'stop, below']
    assert views[2]['series'][2]['y'][0] == pytest.approx(0.0635 / 2)
    *_views, reading = run(project, 'project.stroke', history='Nominal', stroke=0.01)
    assert rows(reading)['Verdict']['status'] == 'fail'
    with pytest.raises(ProtocolError, match='integrates acceleration'):
        run(project, 'project.stroke', history='Force')


def test_a_logged_pulse_fits_at_the_lowest_cutoff_that_takes_least(validate):
    project = _shock()
    travel, *sweep, reading = run(project, 'project.replication_fit')
    wire_views([travel, *sweep, reading], validate)
    found = rows(reading)
    fitting = [label for label in ('2 Hz', '3 Hz', '5 Hz', '8 Hz')
               if found[label]['status'] == 'pass']
    assert travel['title'].startswith(f'{fitting[0]}: fits')
    assert found['Logged']['status'] == 'fail', 'logged, the travel drifts off'
    assert found['Largest peak']['value'].startswith('490.3 m/s²')
    travel, *_rest = run(project, 'project.replication_fit', logged=True)
    assert travel['title'].startswith('As logged')
    with pytest.raises(ProtocolError, match='not a list of cutoffs'):
        run(project, 'project.replication_fit', cutoffs='lots')
    with pytest.raises(ProtocolError, match='not one of the candidate cutoffs'):
        run(project, 'project.replication_fit', cutoff=4.0)


def test_a_tone_rides_its_own_ridge(validate):
    rate = 1024.0
    t = np.arange(8192) / rate
    project = Project()
    project.add('Tone', history(t, 3.0 * np.sin(2.0 * np.pi * 64.0 * t), ['1Z+']))
    _surface, picture, ridge, reading = run(project, 'project.wavelet')
    wire_views([picture, ridge, reading], validate)
    assert picture['values'].shape == (len(picture['rows']['ticks']),
                                       len(picture['columns']['ticks']))
    assert len(picture['columns']['ticks']) <= 400, 'peak-held to a picture\'s width'
    assert float(picture['rows']['ticks'][0].split()[0]) > float(
        picture['rows']['ticks'][-1].split()[0]), 'the highest frequency on top'
    assert np.median(ridge['series'][0]['y']) == pytest.approx(64.0, rel=0.03)
    assert rows(reading)['Largest']['value'] == pytest.approx(3.0, rel=0.02)
    with pytest.raises(ProtocolError, match='there is no record 2'):
        run(project, 'project.wavelet', record=2)


# ---- the shock windows ----------------------------------------------------------------------

def test_a_shock_window_is_typed_added_and_removed():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'random.nc4')})
    project = state.project
    data = project['Time History']
    data.shocks = ()
    origin = float(data.abscissa[0])
    *_views, reading = run(project, 'project.set_shock', history='Time History',
                           start=origin + 0.5, stop=origin + 0.7)
    *_views, reading = run(project, 'project.set_shock', history='Time History', shock=2,
                           start=origin + 0.2, stop=origin + 0.6)
    assert [s.start for s in data.shocks] == pytest.approx([0.2, 0.5])
    assert data.shocks[0].stop == pytest.approx(0.5), 'held off its neighbor'
    assert rows(reading)['Shocks']['value'].startswith('2 shocks')
    with pytest.raises(ProtocolError, match='the next is shock 3'):
        run(project, 'project.set_shock', history='Time History', shock=4)
    with pytest.raises(ProtocolError, match='some length'):
        run(project, 'project.set_shock', history='Time History', start=1.0, stop=1.0)
    *_views, reading = run(project, 'project.remove_shock', history='Time History', shock=1)
    assert [s.start for s in data.shocks] == pytest.approx([0.5])
    with pytest.raises(ProtocolError, match='there is no shock 2'):
        run(project, 'project.remove_shock', history='Time History', shock=2)


def test_the_shock_windows_drag_on_the_record_and_on_its_stage(validate):
    """Visual Dynamics' shock windows: spans on the record and on the 3-D
    stage, each dragged to move it through Shock Window."""
    from .test_ops import wire_views

    state, _ = projects.import_file(None, {'path': str(PLATE / 'random.nc4')})
    project = state.project
    data = project['Time History']
    data.shocks = ()
    origin = float(data.abscissa[0])
    run(project, 'project.set_shock', history='Time History', start=origin + 0.2,
        stop=origin + 0.4)
    trace, stage, reading = run(project, 'project.shocks', history='Time History', record=2)
    wire_views([trace, stage, reading], validate)
    assert trace['view'] == 'spectrum' and stage['view'] == 'waterfall'
    (span,) = trace['regions']
    assert stage['regions'] == trace['regions'], 'the same window on the stage'
    assert (span['x0'], span['x1']) == (pytest.approx(origin + 0.2), pytest.approx(origin + 0.4))
    drag = span['drag']
    assert drag['op'] == 'project.set_shock' and (drag['low'], drag['high']) == ('start', 'stop')
    assert drag['params'] == {'history': 'Time History', 'shock': 1, 'record': 2}
    trace, stage, _reading = run(project, drag['op'], **drag['params'], start=origin + 0.3,
                                 stop=origin + 0.6)
    assert data.shocks[0].start == pytest.approx(0.3) and data.shocks[0].stop == pytest.approx(0.6)
    assert stage['regions'][0]['x1'] == pytest.approx(origin + 0.6)
    assert trace['title'].endswith(data.record_label(1)), 'the record dragged on stays shown'
