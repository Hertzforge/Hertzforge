"""Sine sweep specifications: the sweep law as measured, held in tests.

The numbers here are the Phase A measurements, not invented: 700 Hz at
50 Hz/s is 14.00 s (matched the controller's own trajectory record to
0.0009 Hz), and one octave at log rate 10 is 6.00 s — oct/min, the
fact the fixture run settled.
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.sine import LOG, SineSweepSpecification, SineTone


def _tone(name='Up', start=1.0, frequency=(100.0, 800.0), level=2.0,
          types=(0,), rates=(50.0,), **limits):
    n = len(frequency)
    return SineTone(name, start, frequency, np.full((n, 3), level),
                    types, rates, **limits)


def _spec(*tones):
    return SineSweepSpecification(
        tones or [_tone()], ['101Z+', '104Z+', '110Z+'],
        ordinate_unit='m/s**2')


def test_the_linear_sweep_law_is_the_measured_one():
    tone = _tone()
    assert tone.duration() == pytest.approx(14.0), \
        '700 Hz at 50 Hz/s — the trajectory the controller record matched'
    assert tone.span() == (1.0, 15.0)
    t, f = tone.trajectory(1.0 / 4096.0)
    assert f[0] == 100.0 and f[-1] == 800.0
    assert np.all(np.diff(f) >= 0)
    mid = np.searchsorted(t, 7.0)
    assert f[mid] == pytest.approx(100.0 + 50.0 * 7.0, rel=1e-6)


def test_the_log_rate_is_octaves_per_minute():
    tone = _tone(frequency=(200.0, 400.0), types=(LOG,), rates=(10.0,))
    assert tone.duration() == pytest.approx(6.0), \
        'one octave at rate 10 transited in 5.87 s on the fixture: oct/min'
    _t, f = tone.trajectory(1e-3)
    assert f[len(f) // 2] == pytest.approx(200.0 * np.sqrt(2.0), rel=1e-3), \
        'a log sweep is halfway in octaves at half time'


def test_a_descending_segment_takes_a_negative_rate():
    tone = _tone(frequency=(800.0, 150.0), rates=(-65.0,))
    assert tone.duration() == pytest.approx(10.0)
    with pytest.raises(ValueError, match='descending'):
        _tone(frequency=(800.0, 150.0), rates=(65.0,))
    with pytest.raises(ValueError, match='ascending'):
        _tone(rates=(-50.0,))


def test_a_dwell_segment_refuses():
    with pytest.raises(ValueError, match='dwell'):
        _tone(frequency=(400.0, 400.0))


def test_target_interpolates_per_segment_reading():
    """Amplitude is linear in time: linear in f on a linear segment,
    linear in log-f on a log one — and NaN where the tone never goes."""
    tone = SineTone('Shaped', 0.0, [100.0, 200.0, 400.0],
                    [[1.0], [2.0], [4.0]], [0, LOG], [50.0, 10.0])
    target = tone.target([150.0, 200.0 * np.sqrt(2.0), 50.0, 900.0])
    assert target[0, 0] == pytest.approx(1.5), 'linear segment, linear in f'
    assert target[1, 0] == pytest.approx(3.0), \
        'log segment: halfway in octaves is halfway in amplitude'
    assert np.isnan(target[2, 0]) and np.isnan(target[3, 0]), \
        'the specification says nothing outside its sweeps'


def test_bands_travel_and_interpolate_like_the_amplitude():
    n = 2
    tone = _tone(warning_lower=np.full((n, 3), 2.0 * 10 ** (-3 / 20)),
                 warning_upper=np.full((n, 3), 2.0 * 10 ** (3 / 20)))
    band = tone.target([450.0], curve='warning_upper')
    assert band[0, 0] == pytest.approx(2.0 * 10 ** (3 / 20))
    with pytest.raises(ValueError, match='abort_lower'):
        tone.target([450.0], curve='abort_lower')


def test_tones_are_time_boxed_and_the_span_covers_them():
    spec = _spec(_tone('Up', 0.0),
                 _tone('Down', 2.0, (800.0, 150.0), rates=(-65.0,)),
                 _tone('Log', 1.0, (200.0, 400.0), types=(LOG,),
                       rates=(10.0,)))
    assert spec.tone('Down').span() == (2.0, 12.0)
    assert spec.span() == (0.0, 14.0), 'first start to last end'
    with pytest.raises(KeyError, match='Dwell'):
        spec.tone('Dwell')


def test_mismatched_channels_and_repeated_names_refuse():
    with pytest.raises(ValueError, match='columns'):
        SineSweepSpecification([_tone()], ['101Z+'])
    with pytest.raises(ValueError, match='repeat'):
        _spec(_tone('Up'), _tone('Up', 2.0))


def test_the_specification_round_trips_through_native(tmp_path):
    spec = _spec(_tone('Up', 0.0,
                       warning_lower=np.full((2, 3), 1.5),
                       abort_upper=np.full((2, 3), 3.0)),
                 _tone('Log', 1.0, (200.0, 400.0), types=(LOG,),
                       rates=(10.0,)))
    path = str(tmp_path / 'sine.vdyn')
    hertzforge.io.save(spec, path)
    back = hertzforge.io.load(path)
    assert back == spec
    assert list(back.tone('Up').limits) == ['warning_lower', 'abort_upper']
    assert back.ordinate_unit == 'm/s**2'


def test_a_project_carries_and_describes_it(tmp_path):
    from hertzforge.project import describe

    project = hertzforge.Project('Sine Test')
    project.add('Sine Specification', _spec())
    assert project.sine_sweep_specification is not None
    assert describe(project['Sine Specification']) == \
        '1 tone, 100-800 Hz, 3 control channels'
    path = str(tmp_path / 'sine_project.vdyn')
    project.save(path)
    back = hertzforge.Project.open(path)
    assert back.sine_sweep_specification == project.sine_sweep_specification
