"""The shock check: a record's raw velocity, read before its spectrum.

Pinned against arithmetic that does not depend on the implementation:
the area under a half-sine is 2AT/pi, a step of height s held for a
time t adds s*t to the velocity, and a pulse compensated symmetrically
before and after has no net area and no net first moment, so it ends
at rest in velocity and in displacement. The spectrum is pinned to the
half-sine's textbook hump, about 1.65 times the peak near 0.8 over the
duration, and to the shape of its two polarities.
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.core import srs
from hertzforge.core.data import TimeHistory
from hertzforge.core.filters import integrate
from hertzforge.core.shock_check import (
    RETURN_SHARE,
    ShockCheck,
    _percent,
    check_shock,
    summarize,
)
from hertzforge.core.shocks import Shock

RATE = 20480.0

G = 9.80665

PEAK = 50.0 * G

DURATION = 0.011

VELOCITY_CHANGE = 2.0 * PEAK * DURATION / np.pi


def times(span):
    return np.arange(round(span * RATE)) / RATE


def half_sine(t, start, peak=PEAK, duration=DURATION):
    inside = (t >= start) & (t <= start + duration)
    return np.where(inside, peak * np.sin(np.pi * (t - start) / duration),
                    0.0)


def compensated(t, center, compensation=0.11):
    """A half-sine with a half-sine of the opposite sign on either side,
    each carrying half its area and placed symmetrically about it: the
    net area and the net first moment are both zero."""
    depth = PEAK * DURATION / (2.0 * compensation)
    opens = center - DURATION / 2.0
    return (half_sine(t, opens)
            + half_sine(t, opens - compensation, -depth, compensation)
            + half_sine(t, opens + DURATION, -depth, compensation))


def history(*rows, dims=None):
    rows = np.atleast_2d(np.vstack(rows))
    n = len(rows)
    return TimeHistory(np.arange(rows.shape[1]) / RATE, rows,
                       response_dof=[f'{k + 1}Z+' for k in range(n)],
                       ordinate_dim=dims or ['acceleration'] * n)


def same_dof(span=0.4):
    """A velocity and an acceleration both at 1Z+, the velocity first."""
    t = times(span)
    return TimeHistory(t, np.vstack([np.full_like(t, 3.0),
                                     compensated(t, span / 2.0)]),
                       response_dof=['1Z+', '1Z+'],
                       ordinate_dim=['velocity', 'acceleration'])


def test_a_half_sine_reports_the_area_under_it():
    """The velocity change of a half-sine is 2AT/pi. Alone, nothing
    brings the velocity back, so it ends at its full change and holds
    there: flagged, because a shaker record ends at rest, and worded as
    a held change, not a drift, because a drop machine's pulse does
    exactly this."""
    t = times(0.25)
    check, = check_shock(history(half_sine(t, 0.05)))
    assert check.velocity_change == pytest.approx(VELOCITY_CHANGE, rel=1e-5)
    assert check.end_velocity == pytest.approx(VELOCITY_CHANGE, rel=1e-5)
    assert check.end_velocity_share == pytest.approx(1.0, abs=1e-9)
    assert check.flagged
    assert not check.drifting
    assert 'ends at 100 % of its peak and holds there' in check.describe()


def test_the_displacement_runs_on_from_the_pulses_middle():
    """After a half-sine the table coasts at its velocity change, so
    by the end it has traveled that velocity times the time since the
    pulse's middle, which is where a symmetric pulse's area acts."""
    t = times(0.25)
    check, = check_shock(history(half_sine(t, 0.05)))
    assert check.end_displacement == pytest.approx(
        VELOCITY_CHANGE * (t[-1] - 0.05 - DURATION / 2.0), rel=1e-4)
    assert check.end_displacement_share == pytest.approx(1.0, abs=1e-9)
    assert check.time[0] == 0.0
    assert check.time[1] == pytest.approx(1.0 / RATE)


def test_a_compensated_pulse_returns_to_rest():
    """Velocity and displacement both come back to where they started,
    and the velocity change is still the main pulse's: the span from
    the bottom of the pre-pulse to the top of the main pulse."""
    t = times(0.4)
    check, = check_shock(history(compensated(t, 0.2)))
    # zero to the sampling: the pulses' edges fall between samples
    assert check.end_velocity_share < 1e-4
    assert check.end_displacement_share < 1e-4
    assert check.velocity_change == pytest.approx(VELOCITY_CHANGE, rel=1e-5)
    assert not check.flagged
    assert check.describe().startswith('Velocity returns to rest')


def test_a_step_offset_is_flagged_and_ends_at_the_steps_integral():
    """A zero shift of 4 g from the moment the shock arrives. The pulse
    itself nets nothing, so the whole of the end velocity is the step:
    its height times how long it was held."""
    t = times(0.4)
    arrives = 0.2 - DURATION / 2.0 - 0.11
    step = np.where(t >= arrives, 4.0 * G, 0.0)
    check, = check_shock(history(compensated(t, 0.2) + step))
    held = t[-1] - t[np.flatnonzero(step)[0]]
    assert check.end_velocity == pytest.approx(4.0 * G * held, rel=1e-3)
    assert check.flagged
    assert check.drifting
    assert 'zero shift' in check.describe()


def test_the_default_integration_hides_what_the_check_finds():
    """Why the check integrates raw. The drift high-pass the package
    integrates with by default takes the step out, and the shifted
    record's velocity ends at rest as though nothing were wrong."""
    t = times(0.4)
    shifted = history(compensated(t, 0.2) + np.where(t >= 0.08, 4.0 * G, 0.0))
    smoothed = integrate(shifted).ordinate[0].real
    check, = check_shock(shifted)
    assert abs(smoothed[-1]) < 0.05 * check.peak_velocity
    assert check.flagged


def test_a_dc_offset_on_the_whole_record_is_flagged():
    """An offset in the chain is there before the shock as well as
    after it, and ramps the velocity from the first sample."""
    t = times(0.4)
    check, = check_shock(history(compensated(t, 0.2) + 0.5 * G))
    assert check.end_velocity == pytest.approx(
        0.5 * G * t[-1], rel=1e-4)
    assert check.flagged and check.drifting


def test_each_window_is_read_from_its_own_start():
    """An offset between two events walks the record's velocity away,
    but each window opens at rest: the second event reads exactly as
    it would cut out and integrated alone, displacement included."""
    t = times(1.0)
    gap = np.where((t > 0.42) & (t < 0.55), 2.0 * G, 0.0)
    record = history(compensated(t, 0.2) + compensated(t, 0.8) + gap)
    windows = [Shock(0.0, 0.4), Shock(0.6, 0.4)]
    first, second = check_shock(record, shocks=windows)
    assert (first.window, second.window) == (0, 1)
    assert second.start == pytest.approx(0.6)
    assert not first.flagged and not second.flagged

    cut = history(record.ordinate[0, round(0.6 * RATE):round(1.0 * RATE)].real)
    alone, = check_shock(cut)
    assert np.allclose(second.velocity, alone.velocity, rtol=0, atol=1e-12)
    assert np.allclose(second.displacement, alone.displacement,
                       rtol=0, atol=1e-12)


def test_a_sample_that_is_not_a_number_is_refused():
    """A NaN integrates to a velocity that is NaN from there on, whose
    share of its NaN peak would read as zero: refused, by channel and
    by where, rather than passed as having returned to rest."""
    t = times(0.4)
    clean = compensated(t, 0.2)
    broken = clean.copy()
    broken[round(0.3 * RATE)] = np.nan
    with pytest.raises(ValueError, match=r'2Z\+ holds a sample that is not '
                       r'a finite number between 0 s and 0\.4 s'):
        check_shock(history(clean, broken))


def test_a_window_reads_only_its_own_samples():
    """A NaN outside every window is no part of any of them: each
    window is integrated on its own, so the second event reads exactly
    as the record would without the bad sample."""
    t = times(1.0)
    clean = compensated(t, 0.2) + compensated(t, 0.8)
    broken = clean.copy()
    broken[round(0.5 * RATE)] = np.nan
    windows = [Shock(0.6, 0.4)]
    check, = check_shock(history(broken), shocks=windows)
    expected, = check_shock(history(clean), shocks=windows)
    assert np.array_equal(check.velocity, expected.velocity)
    assert check.end_velocity_share < 1e-4


def test_each_check_names_its_channel_by_index():
    """Two channels can share a DOF, an accelerometer's velocity beside
    its acceleration: the check carries which record it read."""
    check, = check_shock(same_dof())
    assert (check.dof, check.record) == ('1Z+', 1)


def test_a_window_that_never_moved_is_at_rest():
    check, = check_shock(history(np.zeros(1000)))
    assert check.end_velocity_share == 0.0
    assert check.end_displacement_share == 0.0
    assert not check.flagged and not check.drifting


@pytest.mark.parametrize('rate,drifting', [(0.00201, True), (0.00199, False)])
def test_drifting_is_the_tails_change_carried_across_the_window(
        rate, drifting):
    """101 samples at a peak of 2, the last 30 falling by `rate` a
    sample. The tail is the last 10, so the change across it is 10
    `rate`, and carried across the window's 100 intervals it is
    100 `rate`, against a tenth of the peak, 0.2: held within half a
    percent either side, so a tail a sample longer or shorter, or a
    window counted one sample differently, lands on the other side."""
    velocity = np.full(101, 2.0)
    velocity[-30:] -= rate * np.arange(1, 31)
    velocity[0] = 0.0
    check = ShockCheck(dof='1Z+', record=0, window=0, start=0.0,
                       sample_rate=100.0,
                       velocity=velocity, displacement=np.zeros(101))
    assert check.drifting is drifting


def test_the_shares_are_written_to_a_place_below_ten():
    assert _percent(0.0123) == '1.2 %'
    assert _percent(0.5) == '50 %'


def test_the_tolerance_is_the_callers():
    t = times(0.25)
    record = history(half_sine(t, 0.05))
    assert RETURN_SHARE == 0.1
    assert check_shock(record)[0].flagged
    assert not check_shock(record, tolerance=1.01)[0].flagged


def test_a_tolerance_of_nothing_is_refused():
    with pytest.raises(ValueError, match='above zero'):
        check_shock(history(half_sine(times(0.1), 0.01)), tolerance=0.0)


def test_only_acceleration_is_checked():
    """A velocity channel integrates to displacement, and read as a
    velocity it would be the mistake the check is there to catch."""
    t = times(0.4)
    record = history(compensated(t, 0.2), np.zeros_like(t),
                     compensated(t, 0.2),
                     dims=['acceleration', 'velocity', 'acceleration'])
    assert [check.dof for check in check_shock(record)] == ['1Z+', '3Z+']


def test_a_record_with_no_acceleration_is_refused():
    with pytest.raises(ValueError, match='holds none'):
        check_shock(history(np.zeros(100), dims=['velocity']))


def test_the_summary_names_the_worst_first():
    t = times(1.0)
    record = history(compensated(t, 0.2) + compensated(t, 0.8),
                     compensated(t, 0.2) + compensated(t, 0.8)
                     + np.where(t > 0.7, 4.0 * G, 0.0))
    checks = check_shock(record, shocks=[Shock(0.0, 0.4),
                                         Shock(0.6, 0.4)])
    lines = summarize(checks).splitlines()
    assert lines[0] == '1 of 4 do not return to rest.'
    assert lines[1].startswith('2Z+, shock 2: Velocity does not return')
    assert summarize(checks[:1]) == 'Velocity returns to rest on every channel.'


def test_the_summary_stops_at_three_and_counts_the_rest():
    t = times(0.25)
    checks = check_shock(history(*[half_sine(t, 0.05)] * 5))
    lines = summarize(checks).splitlines()
    assert lines[0] == '5 of 5 do not return to rest.'
    assert len(lines) == 5
    assert lines[-1] == 'and 2 more.'
    exactly = summarize(checks[:3]).splitlines()
    assert len(exactly) == 4 and 'more' not in exactly[-1]


def test_a_half_sine_spectrum_peaks_at_1_65_times_near_0_8_over_t():
    t = times(0.25)
    spectra = history(half_sine(t, 0.05)).compute_srs_polarities(
        low=10.0, high=2000.0, q=10.0)
    positive = spectra['positive'].ordinate[0].real
    at = np.argmax(positive)
    natural = spectra['positive'].abscissa[at]
    assert positive[at] / PEAK == pytest.approx(1.65, abs=0.02)
    assert natural * DURATION == pytest.approx(0.8, abs=0.06)


def test_the_negative_spectrum_falls_under_the_positive_at_high_frequency():
    """Above the pulse's content a half-sine pushes every oscillator
    one way only, so its negative spectrum falls away while the
    positive settles at the peak. Far below it, an oscillator rings
    after the pulse nearly as far one way as the other, and the two
    meet. The negative is a magnitude, so it is drawn on the same log
    axis."""
    t = times(0.25)
    spectra = history(half_sine(t, 0.05)).compute_srs_polarities(
        low=10.0, high=2000.0)
    frequencies = spectra['positive'].abscissa
    positive = spectra['positive'].ordinate[0].real
    negative = spectra['negative'].ordinate[0].real
    assert np.all(negative >= 0.0)
    high = frequencies > 10.0 / DURATION
    assert np.all(negative[high] < 0.1 * positive[high])
    assert positive[-1] / PEAK == pytest.approx(1.0, abs=0.01)
    low = frequencies < 0.2 / DURATION
    assert np.all(negative[low] > 0.5 * positive[low])


def test_the_three_polarities_come_from_one_call_with_their_settings():
    t = times(0.25)
    record = history(compensated(t, 0.12))
    spectra = record.compute_srs_polarities(per_octave=6, q=25.0)
    assert list(spectra) == list(srs.KINDS)
    for kind, curve in spectra.items():
        assert (curve.kind, curve.q) == (kind, 25.0)
        assert np.array_equal(
            curve.ordinate,
            record.compute_srs(per_octave=6, q=25.0, kind=kind).ordinate)
    assert np.array_equal(spectra['maximax'].ordinate,
                          np.maximum(spectra['positive'].ordinate,
                                     spectra['negative'].ordinate))
    steps = np.diff(np.log2(spectra['positive'].abscissa))
    assert np.allclose(steps, 1.0 / 6.0)


def test_the_spacing_defaults_to_a_twelfth_of_an_octave():
    spectra = history(half_sine(times(0.1), 0.01)).compute_srs_polarities()
    steps = np.diff(np.log2(spectra['negative'].abscissa))
    assert np.allclose(steps, 1.0 / 12.0)


def test_a_peak_that_is_no_polarity_is_refused():
    with pytest.raises(ValueError, match='maximax'):
        history(half_sine(times(0.1), 0.01)).compute_srs(kind='absolute')
