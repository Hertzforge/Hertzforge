"""The conversions between a control test's tables and the analysis
objects they describe (`hertzforge.io.setup_convert`), each tested both
ways where both ways make sense, with the units made explicit: a Random
profile and a `Specification`, a Sine profile and a
`SineSweepSpecification`, an SRS target and a `ShockSpecification`, a
classical pulse and a `ClassicalPulse`, a TWR reference and a time
history. Each refusal names its reason; nothing is adapted."""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.core.classical_pulse import ClassicalPulse
from hertzforge.core.data import ShockSpecification, Specification, TimeHistory
from hertzforge.core.sine import LINEAR, LOG, SineSweepSpecification, SineTone
from hertzforge.io import setup_convert as sc

G = 9.80665


def _rel(a, b):
    a, b = np.asarray(a, dtype=float), np.asarray(b, dtype=float)
    return float(np.max(np.abs(a - b) / np.abs(b)))


def test_the_unit_factors_come_from_the_unit_code():
    assert sc.G == pytest.approx(G, rel=1e-12)
    assert sc.IPS == pytest.approx(0.0254, rel=1e-12)
    assert sc.INCH == pytest.approx(0.0254, rel=1e-12)


# ---- Random ----------------------------------------------------------------------------------

PROFILE = {'breakpoints': [{'freq_hz': 20.0, 'psd_g2_hz': 0.01},
                           {'freq_hz': 80.0, 'psd_g2_hz': 0.04},
                           {'freq_hz': 350.0, 'psd_g2_hz': 0.04},
                           {'freq_hz': 2000.0, 'psd_g2_hz': 0.007}],
           'alarm_db': [3.0, 3.0], 'abort_db': [6.0, 6.0]}


def test_a_random_profile_goes_to_a_specification_in_si_and_back():
    spec = sc.specification_from_random_profile(PROFILE, ['1X+', '2X+'])
    assert spec.ordinate_dim[0] == 'acceleration**2/frequency'
    # SI: (m/s²)²/Hz, so g²/Hz times g squared
    assert np.real(spec.ordinate[0, 0]) == pytest.approx(0.01 * G ** 2, rel=1e-12)
    back = sc.random_profile(spec)
    assert back['limits'] == ['warning', 'abort']
    assert back['alarm_db'] == [3.0, 3.0] and back['abort_db'] == [6.0, 6.0]
    got = [(r['freq_hz'], r['psd_g2_hz']) for r in back['breakpoints']]
    want = [(r['freq_hz'], r['psd_g2_hz']) for r in PROFILE['breakpoints']]
    assert [f for f, _ in got] == pytest.approx([f for f, _ in want], rel=sc.TOLERANCE)
    assert _rel([v for _, v in got], [v for _, v in want]) <= sc.TOLERANCE


def test_a_random_profile_keeps_bands_that_vary_on_their_rows():
    profile = {**PROFILE, 'breakpoints': [dict(r) for r in PROFILE['breakpoints']]}
    profile['breakpoints'][1]['abort_db'] = [4.0, 5.0]
    back = sc.random_profile(sc.specification_from_random_profile(profile, ['1X+']))
    assert back['abort_db'] is None
    assert [r.get('abort_db') for r in back['breakpoints']] == [
        [6.0, 6.0], [4.0, 5.0], [6.0, 6.0], None]
    assert back['alarm_db'] == [3.0, 3.0]


def test_a_specification_without_limits_fills_no_band():
    spec = sc.specification_from_random_profile(PROFILE, ['1X+'])
    spec.limits.clear()
    back = sc.random_profile(spec)
    assert back['limits'] == [] and back['alarm_db'] is None and back['abort_db'] is None


def test_random_refuses_what_it_cannot_control_to():
    two = sc.specification_from_random_profile(PROFILE, ['1X+', '2X+'])
    two.ordinate[1] *= 2
    with pytest.raises(sc.ConversionRefused, match='choose one channel'):
        sc.random_profile(two)
    assert sc.random_profile(two, channel='2X+')['breakpoints'][0]['psd_g2_hz'] == \
        pytest.approx(0.02, rel=1e-9)
    f = np.array([20.0, 2000.0])
    cross = Specification(abscissa=f, ordinate=np.ones((3, 2)) * G ** 2,
                          response_dof=['1X+', '1X+', '2X+'],
                          reference_dof=['1X+', '2X+', '2X+'],
                          ordinate_dim=['acceleration**2/frequency'] * 3)
    with pytest.raises(sc.ConversionRefused, match='MIMO Random'):
        sc.random_profile(cross)
    force = Specification(abscissa=f, ordinate=np.ones((1, 2)),
                          response_dof=['1X+'], ordinate_dim=['force**2/frequency'])
    with pytest.raises(sc.ConversionRefused, match='force'):
        sc.random_profile(force)
    with pytest.raises(sc.ConversionRefused, match='not a random specification'):
        sc.random_profile(object())


# ---- Sine ------------------------------------------------------------------------------------

SINE_PROFILE = {'rows': [{'freq_hz': 300.0, 'quantity': 'accel_g', 'value': 0.4},
                         {'freq_hz': 320.0, 'quantity': 'accel_g', 'value': 0.4}],
                'alarm_db': [1.5, 1.5], 'abort_db': [3.0, 3.0]}
SWEEP = {'kind': 'sweep', 'band_hz': [300.0, 320.0], 'rate': {'law': 'log', 'oct_per_min': 3.0},
         'start_hz': 300.0, 'direction': 'up', 'end': {'sweeps': 1}}


def test_a_sine_profile_goes_to_a_tone_in_si_and_back():
    spec = sc.sine_specification_from_profile(SINE_PROFILE, SWEEP, ['1Z+'])
    tone = spec.tones[0]
    assert tone.amplitude[0, 0] == pytest.approx(0.4 * G, rel=1e-12)
    assert tone.limits['abort_upper'][0, 0] == pytest.approx(0.4 * G * 10 ** (3 / 20), rel=1e-12)
    assert int(tone.segment_type[0]) == LOG and tone.segment_rate[0] == 3.0
    back = sc.sine_profile(spec)
    assert back['test'] == SWEEP
    assert back['alarm_db'] == [1.5, 1.5] and back['abort_db'] == [3.0, 3.0]
    assert _rel([r['value'] for r in back['rows']], [0.4, 0.4]) <= sc.TOLERANCE


def test_a_downward_linear_sweep_and_a_dwell_come_back():
    down = {**SWEEP, 'rate': {'law': 'linear', 'hz_per_s': 2.0}, 'direction': 'down',
            'start_hz': 320.0}
    spec = sc.sine_specification_from_profile(SINE_PROFILE, down, ['1Z+'])
    assert int(spec.tones[0].segment_type[0]) == LINEAR and spec.tones[0].segment_rate[0] == -2.0
    assert sc.sine_profile(spec)['test'] == down
    dwell = {'kind': 'dwell', 'freq_hz': 310.0, 'end': {'seconds': 4.0}}
    spec = sc.sine_specification_from_profile(SINE_PROFILE, dwell, ['1Z+'])
    back = sc.sine_profile(spec)
    assert back['test'] == dwell
    assert back['rows'][0]['value'] == pytest.approx(0.4, rel=sc.TOLERANCE)


def test_velocity_and_peak_to_peak_displacement_rows():
    velocity = {'rows': [{'freq_hz': 10.0, 'quantity': 'velocity_ips', 'value': 2.0},
                         {'freq_hz': 20.0, 'quantity': 'velocity_ips', 'value': 2.0}]}
    spec = sc.sine_specification_from_profile(velocity, {**SWEEP, 'band_hz': [10, 20]}, ['1Z+'])
    assert spec.ordinate_dim == 'velocity'
    assert spec.tones[0].amplitude[0, 0] == pytest.approx(2.0 * 0.0254, rel=1e-12)
    assert sc.sine_profile(spec)['rows'][0]['value'] == pytest.approx(2.0, rel=1e-12)
    pp = {'rows': [{'freq_hz': 5.0, 'quantity': 'disp_pp_in', 'value': 1.0},
                   {'freq_hz': 9.0, 'quantity': 'disp_pp_in', 'value': 1.0}]}
    spec = sc.sine_specification_from_profile(pp, {**SWEEP, 'band_hz': [5, 9]}, ['1Z+'])
    # one inch peak to peak is half an inch peak
    assert spec.tones[0].amplitude[0, 0] == pytest.approx(0.5 * 0.0254, rel=1e-12)
    assert sc.sine_profile(spec)['rows'][0]['quantity'] == 'disp_pp_in'


def test_sine_refuses_what_it_cannot_play():
    one = sc.sine_specification_from_profile(SINE_PROFILE, SWEEP, ['1Z+'])
    other = sc.sine_specification_from_profile(SINE_PROFILE, SWEEP, ['1Z+']).tones[0]
    other.name = 'Tone 2'
    two = SineSweepSpecification([one.tones[0], other], ['1Z+'],
                                 ordinate_dim='acceleration')
    with pytest.raises(sc.ConversionRefused, match='2 tones'):
        sc.sine_profile(two)
    tone = SineTone('t', 0.0, [10.0, 20.0, 40.0], np.ones((3, 1)), [LOG, LINEAR], [1.0, 1.0])
    with pytest.raises(sc.ConversionRefused, match='law or rate'):
        sc.sine_profile(SineSweepSpecification([tone], ['1Z+'], ordinate_dim='acceleration'))
    back = SineTone('t', 0.0, [10.0, 20.0, 15.0], np.ones((3, 1)), [LOG, LOG], [1.0, -1.0])
    with pytest.raises(sc.ConversionRefused, match='turns back'):
        sc.sine_profile(SineSweepSpecification([back], ['1Z+'], ordinate_dim='acceleration'))
    split = SineTone('t', 0.0, [10.0, 20.0], np.array([[1.0, 2.0], [1.0, 2.0]]), [LOG], [1.0])
    with pytest.raises(sc.ConversionRefused, match='different levels'):
        sc.sine_profile(SineSweepSpecification([split], ['1Z+', '2Z+'],
                                               ordinate_dim='acceleration'))
    force = SineSweepSpecification([one.tones[0]], ['1Z+'], ordinate_dim='force')
    with pytest.raises(sc.ConversionRefused, match='force'):
        sc.sine_profile(force)


# ---- SRS -------------------------------------------------------------------------------------

TARGET = {'kind': 'breakpoints', 'points': [[100.0, 10.0], [1000.0, 100.0], [2000.0, 100.0]]}


def test_an_srs_target_goes_to_a_shock_specification_in_si_and_back():
    spec = sc.shock_specification_from_target(TARGET, {'q': 10.0}, {'alarm_db': [3, 3],
                                                                    'abort_db': [6, 6]}, '1Z+')
    assert isinstance(spec, ShockSpecification) and spec.q == 10.0
    assert np.real(spec.ordinate[0, 1]) == pytest.approx(100.0 * G, rel=1e-12)
    back = sc.srs_target(spec)
    assert back['bands'] == {'f_lo_hz': 100.0, 'f_hi_hz': 2000.0, 'q': 10.0}
    assert back['alarm_db'] == [3.0, 3.0] and back['abort_db'] == [6.0, 6.0]
    assert _rel(np.array(back['target']['points']), np.array(TARGET['points'])) <= sc.TOLERANCE


def test_srs_refuses_another_kind_or_a_band_that_varies():
    spec = ShockSpecification.from_breakpoints([100.0, 1000.0], [10.0, 100.0], '1Z+',
                                               per_octave=3, q=10.0, ordinate_unit='g',
                                               warning=(-3.0, 3.0), abort=(-6.0, 6.0))
    back = sc.srs_target(spec)
    assert back['abort_db'] == [6.0, 6.0]
    spec.kind = 'maximum_absolute'
    with pytest.raises(sc.ConversionRefused, match='maximax'):
        sc.srs_target(spec)
    spec.kind = 'maximax'
    spec.limits['abort_upper'][0, 0] *= 1.5
    with pytest.raises(sc.ConversionRefused, match='varies from point to point'):
        sc.srs_target(spec)


# ---- Classical Shock -------------------------------------------------------------------------

@pytest.mark.parametrize('wire', [
    {'shape': 'half_sine', 'amplitude_g': 50.0, 'duration_ms': 11.0, 'polarity': 'positive',
     'rise_frac': None, 'fall_frac': None},
    {'shape': 'terminal_peak_sawtooth', 'amplitude_g': 30.0, 'duration_ms': 6.0,
     'polarity': 'negative', 'rise_frac': None, 'fall_frac': None},
    {'shape': 'trapezoid', 'amplitude_g': 20.0, 'duration_ms': 10.0, 'polarity': 'positive',
     'rise_frac': 0.1, 'fall_frac': 0.1}])
def test_a_pulse_goes_to_a_classical_pulse_in_si_and_back(wire):
    pulse = sc.classical_pulse(wire)
    assert abs(pulse.peak) == pytest.approx(wire['amplitude_g'] * G, rel=1e-12)
    assert pulse.duration == pytest.approx(wire['duration_ms'] * 1e-3, rel=1e-12)
    back = sc.shock_pulse(pulse)
    for key, value in wire.items():
        assert back[key] == (pytest.approx(value, rel=1e-12) if isinstance(value, float)
                             else value)


def test_a_pulse_with_no_analysis_shape_is_refused():
    with pytest.raises(sc.ConversionRefused, match='haversine'):
        sc.classical_pulse({'shape': 'haversine', 'amplitude_g': 1, 'duration_ms': 1})
    with pytest.raises(sc.ConversionRefused, match='falls at once'):
        sc.shock_pulse(ClassicalPulse('sawtooth', 9.8, 0.01, 0.001))


@pytest.mark.parametrize('pulse', [ClassicalPulse('half-sine', 50 * G, 0.011),
                                   ClassicalPulse('sawtooth', -30 * G, 0.006, 0.0),
                                   ClassicalPulse('trapezoid', 20 * G, 0.010, 0.001)])
def test_a_waveform_that_is_a_classical_pulse_is_read_as_one(pulse):
    time, values = sc.pulse_waveform(pulse, 51200.0)
    found, off = sc.pulse_of_waveform(time, values)
    assert found.shape == pulse.shape and off <= sc.PULSE_TOLERANCE
    assert found.peak == pytest.approx(pulse.peak, rel=0.01)
    assert found.duration == pytest.approx(pulse.duration, rel=0.01)


def test_a_waveform_that_is_no_pulse_is_refused():
    time = np.arange(2000) / 20000.0
    with pytest.raises(sc.ConversionRefused, match='no classical pulse'):
        sc.pulse_of_waveform(time, np.sin(2 * np.pi * 300 * time) * np.exp(-time * 20))


# ---- TWR -------------------------------------------------------------------------------------

def test_a_time_history_goes_to_a_reference_in_the_control_unit_and_back():
    time = np.arange(400) / 2048.0
    values = np.sin(2 * np.pi * 50 * time) * 3 * G
    history = TimeHistory(time, values[np.newaxis], response_dof=['1Z+'],
                          ordinate_dim=['acceleration'], ordinate_unit=['m/s**2'])
    ref = sc.twr_reference(history, units='g')
    assert ref['fs_hz'] == pytest.approx(2048.0, rel=1e-12) and ref['samples'] == 400
    samples = np.array([float(v) for v in ref['text'].split()])
    assert np.max(np.abs(samples - values / G)) <= 3 * sc.TOLERANCE
    back = sc.transient_from_reference(samples, ref['fs_hz'], 'g', '1Z+')
    assert np.max(np.abs(np.real(back.ordinate[0]) - values)) <= 3 * G * sc.TOLERANCE
    in_si = sc.twr_reference(history, units='m/s^2')
    assert float(in_si['text'].split()[10]) == pytest.approx(values[10], rel=1e-9)


def test_a_reference_of_another_quantity_or_uneven_time_is_refused():
    time = np.arange(10) / 100.0
    force = TimeHistory(time, np.ones((1, 10)), response_dof=['1Z+'], ordinate_dim=['force'])
    with pytest.raises(sc.ConversionRefused, match='force'):
        sc.twr_reference(force, units='g')
    uneven = TimeHistory(time ** 2, np.ones((1, 10)), response_dof=['1Z+'],
                         ordinate_dim=['acceleration'])
    with pytest.raises(sc.ConversionRefused, match='evenly spaced'):
        sc.twr_reference(uneven, units='g')


# ---- Mixed Mode ------------------------------------------------------------------------------

def test_mixed_parts_come_from_a_floor_its_narrowbands_and_tones_and_back():
    from hertzforge.core.mixed_mode import Narrowband, Tone

    floor = sc.specification_from_random_profile(PROFILE, ['1Z+'])
    floor.narrowbands = (Narrowband(100.0, 0.05 * G ** 2, width=10.0, tolerance_db=3.0,
                                    name='Blade'),
                         Narrowband(200.0, 0.02 * G ** 2, share=0.1, sweep_to=400.0, rate=1.0,
                                    mode=LOG))
    floor.tone_exclusion = 3.0
    tone = Tone(150.0, 2.0 * G, sweep_to=300.0, rate=2.0, mode=LOG, name='Rotor')
    fixed = Tone(60.0, 1.0 * G, name='Line')
    tones = SineSweepSpecification([tone.sine_tone(None, 1), fixed.sine_tone(10.0, 1)], ['1Z+'],
                                   ordinate_dim='acceleration')
    out = sc.mixed_parts(floor, tones)
    parts = {p['id']: p for p in out['parts']}
    assert out['exclusion_hz'] == 3.0
    assert parts['band1']['level_g2_hz'] == pytest.approx(0.05, rel=1e-12)
    assert parts['band1']['alarm_db'] == [3.0, 3.0] and parts['band1']['width_hz'] == 10.0
    assert parts['band2']['schedule'] == {
        'kind': 'sweep', 'band_hz': [200.0, 400.0], 'rate': {'law': 'log', 'oct_per_min': 1.0},
        'start_hz': 200.0, 'direction': 'up'}
    assert parts['tone1']['amplitude_g'] == pytest.approx(2.0, rel=1e-12)
    assert parts['tone1']['schedule']['band_hz'] == [150.0, 300.0]
    assert parts['tone2']['schedule'] == {'kind': 'fixed', 'freq_hz': 60.0,
                                          'end': {'seconds': 10.0}}
    with pytest.raises(sc.ConversionRefused, match='Mixed Mode'):
        sc.random_profile(floor)


# ---- limits and a ceiling --------------------------------------------------------------------

def test_a_ceiling_goes_to_a_specification_in_si_and_back():
    limit = {'channel': 3, 'kind': 'response',
             'rows': [{'freq_hz': 20.0, 'ceiling': 0.1}, {'freq_hz': 2000.0, 'ceiling': 0.5}]}
    spec = sc.specification_from_limit(limit, '3X+')
    assert np.real(spec.ordinate[0, 0]) == pytest.approx(0.1 * G ** 2, rel=1e-12)
    assert sc.limit_profile(spec, 3) == {**limit, 'rows': [
        {'freq_hz': 20.0, 'ceiling': pytest.approx(0.1, rel=1e-12)},
        {'freq_hz': 2000.0, 'ceiling': pytest.approx(0.5, rel=1e-12)}]}
