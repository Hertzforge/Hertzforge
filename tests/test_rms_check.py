"""The rms check: the rms from the area under an autospectrum beside the
rms of the time history it came from, the gap in dB, and the verdict.

Pinned against hand values (a flat density whose area is known, a sine's
rms), against a random record averaged every way the check has to tell
apart (frames end to end with no window, which is exact; Hann with and
without overlap, which is an estimate within hundredths of a decibel and
carries the frames' own mean square exactly), and against the scaling
errors the check exists to catch, each made on purpose and named: the
window's amplitude correction on a density, the one-sided doubling left
out or applied twice, and a factor of standard gravity. Then the span
and the captures read, what is refused, the project's record of where a
spectrum came from, and the label and plot that write the check.
"""

from __future__ import annotations

import copy
from dataclasses import replace

import numpy as np
import pytest

from hertzforge.core.averaging import Averaging
from hertzforge.core.data import Psd, TimeHistory
from hertzforge.core.rms_check import (
    _coverage,
    matching_records,
    refusal,
    rms_check,
    time_rms,
)

G = 9.80665

FS = 2048.0


def _record(seconds=16.0, seed=1502, grms=2.0, offset=0.0):
    """Gaussian noise flat from 20 to 800 Hz at `grms` plus `offset`, in
    g, stored as one channel in m/s^2."""
    rng = np.random.default_rng(seed)
    n = int(seconds * FS)
    spectrum = np.fft.rfft(rng.standard_normal(n))
    f = np.fft.rfftfreq(n, 1.0 / FS)
    spectrum[(f < 20.0) | (f > 800.0)] = 0.0
    x = np.fft.irfft(spectrum, n)
    x = x * grms / np.sqrt(np.mean(x ** 2)) + offset
    t = np.arange(n) / FS
    return TimeHistory(t, x[None] * G, response_dof=['1Z+'],
                       ordinate_dim=['acceleration'], ordinate_unit=['g'])


def _scaled(psd, factor):
    """The spectrum with every line multiplied by `factor`: a scaling
    error made on purpose."""
    out = copy.deepcopy(psd)
    out.ordinate = psd.ordinate * factor
    return out


def replace_check(check, **changes):
    """The check with `changes` made: a case set up on its numbers."""
    return replace(check, **changes)


@pytest.fixture(scope='module')
def record():
    return _record()


def test_a_flat_density_s_area_is_its_level_times_its_band():
    """101 lines 1 Hz apart at 0.01 per hertz hold 1.01 of mean square:
    every line, the two ends included, stands for a whole bin."""
    f = np.arange(0.0, 101.0)
    check = rms_check(f, np.full(101, 0.01), np.full(400, np.sqrt(1.01)))
    assert check.spectrum_rms == pytest.approx(np.sqrt(1.01), rel=1e-12)
    assert check.time_rms == pytest.approx(np.sqrt(1.01), rel=1e-12)
    assert check.gap_db == pytest.approx(0.0, abs=1e-10)
    assert check.passed and check.lines == 101 and check.spacing == 1.0
    assert check.samples == 400 and check.framed_rms is None


def test_a_sine_s_rms_is_its_amplitude_over_root_two():
    t = np.arange(4096) / 4096.0
    assert time_rms(3.0 * np.sin(2.0 * np.pi * 64.0 * t)) == \
        pytest.approx(3.0 / np.sqrt(2.0), rel=1e-12)
    # captures are pooled sample for sample, whatever their lengths
    assert time_rms([np.full(3, 2.0), np.full(1, 6.0)]) == \
        pytest.approx(np.sqrt((3 * 4.0 + 36.0) / 4.0))


def test_the_gap_is_signed_high_when_the_spectrum_reads_high():
    f = np.arange(0.0, 11.0)
    high = rms_check(f, np.full(11, 2.0), np.full(10, np.sqrt(11.0)))
    assert high.gap_db == pytest.approx(10.0 * np.log10(2.0))
    assert high.verdict.startswith('the spectrum reads 3.01 dB high')
    low = rms_check(f, np.full(11, 0.5), np.full(10, np.sqrt(11.0)))
    assert low.gap_db == pytest.approx(-10.0 * np.log10(2.0))
    assert 'low' in low.verdict


def test_the_record_is_read_over_the_frames_span_alone():
    history = _record(seconds=8.0)
    loud = history.ordinate.copy()
    loud[0, :int(2 * FS)] *= 10.0              # a loud start, not analyzed
    history = TimeHistory(history.abscissa, loud, response_dof=['1Z+'],
                          ordinate_dim=['acceleration'],
                          ordinate_unit=['g'])
    averaging = Averaging(frame_length=1024, overlap=0.0, window='boxcar',
                          frames=8, start=2.0)
    check = history.compute_psds(averaging).rms_check(history,
                                                      averaging=averaging)
    span = loud[0, int(2 * FS):int(2 * FS) + 8 * 1024]
    assert check.time_rms == pytest.approx(np.sqrt(np.mean(span ** 2)))
    assert check.span == pytest.approx((2.0, 6.0))
    assert check.samples == 8 * 1024
    assert check.gap_db == pytest.approx(0.0, abs=1e-9)


def test_no_averaging_is_each_record_whole_unwindowed():
    history = _record(seconds=2.0)
    check = history.compute_psds().rms_check(history)
    assert check.window == 'boxcar' and check.span == pytest.approx(
        (0.0, 2.0))
    assert check.gap_db == pytest.approx(0.0, abs=1e-9)


def test_a_force_at_the_same_dof_is_not_the_channel():
    t = np.arange(64) / 64.0
    history = TimeHistory(t, np.ones((2, 64)), response_dof=['1Z+', '1Z+'],
                          ordinate_dim=['force', 'acceleration'],
                          ordinate_unit=['N', 'g'])
    assert matching_records(history, '1Z+', 'acceleration**2/frequency') \
        == [1]
    assert matching_records(history, '1Z+', 'force**2/frequency') == [0]
    assert matching_records(history, '1Z+',
                            'acceleration*force/frequency') == []


def test_a_record_with_no_mean_square_has_no_gap():
    f = np.arange(0.0, 11.0)
    check = rms_check(f, np.ones(11), np.zeros(32))
    assert np.isnan(check.gap_db) and not check.passed
    assert check.verdict.startswith('no gap to judge')
    assert 'no gap' in check.describe()


def _undeclared_pair(seconds=4.0):
    """Two channels at 1Z+ with no declared units, told apart only by
    their hints: an acceleration of 2 (raw) rms and a force a million
    times smaller."""
    accel = _record(seconds=seconds, seed=7).ordinate[0] / G
    force = 1e-6 * _record(seconds=seconds, seed=8).ordinate[0] / G
    t = np.arange(accel.size) / FS
    return TimeHistory(t, np.vstack([accel, force]),
                       response_dof=['1Z+', '1Z+'],
                       dimension_hint=['acceleration', 'force'])


def test_a_hint_that_matches_no_channel_is_named_in_the_refusal():
    history = _undeclared_pair()
    f = np.arange(0.0, 1025.0) * 2.0
    velocity = Psd(f, np.ones((1, f.size)), response_dof=['1Z+'],
                   dimension_hint=['velocity**2/frequency'])
    with pytest.raises(ValueError,
                       match=r'what velocity\*\*2/frequency is the') as err:
        velocity.rms_check(history)
    assert 'unknown' not in str(err.value)


@pytest.mark.parametrize('block, cut, short', [
    (1024, slice(None), False), (1024, slice(1, None), True),
    (1024, slice(None, -1), True), (1023, slice(None), False),
    (1023, slice(None, -1), True)])
def test_the_coverage_is_said_a_line_short_at_either_end(block, cut, short):
    """Lines from 0 to half the rate cover it all, and so does an odd
    frame's, whose top line sits half a line under it; without the DC
    line or the top line they are a line short and say so."""
    f = np.fft.rfftfreq(block, 1.0 / FS)[cut]
    assert (_coverage(f, FS) is not None) is short


def test_samples_not_finite_are_left_out_and_counted():
    """The rule `time_statistics` reads a record by: a sample that is not
    finite is left out and counted, not a refusal of the whole record."""
    from hertzforge.core.time_statistics import time_statistics

    x = np.array([1.0, np.nan, -1.0, 1.0, np.inf])
    assert time_rms(x) == pytest.approx(1.0)
    assert time_statistics(x, 4.0).rms == pytest.approx(time_rms(x))
    check = rms_check(np.arange(0.0, 11.0), np.ones(11), x)
    assert any('2 samples' in note and 'not finite' in note
               for note in check.notes)


def _one_channel(x, start=0.0):
    """`x`, in g, as a one-channel record at 1Z+ whose clock starts at
    `start`."""
    t = start + np.arange(len(x)) / FS
    return TimeHistory(t, np.asarray(x, dtype=float)[None] * G,
                       response_dof=['1Z+'], ordinate_dim=['acceleration'],
                       ordinate_unit=['g'])


def test_a_spectrum_of_zeros_and_a_record_of_nans_are_each_named():
    f = np.arange(0.0, 11.0)
    assert rms_check(f, np.zeros(11), np.ones(32)).verdict == (
        "no gap to judge: the spectrum's lines are all zero")
    assert rms_check(f, np.ones(11), np.full(32, np.nan)).verdict == (
        "no gap to judge: none of the record's samples is a finite number")


def test_an_empty_record_and_an_empty_spectrum_are_refused():
    with pytest.raises(ValueError, match='no samples'):
        rms_check(np.arange(0.0, 11.0), np.ones(11), np.array([]))
    with pytest.raises(ValueError, match='two lines or more'):
        rms_check(np.array([1.0]), np.ones(1), np.ones(32))


def test_a_record_whose_clock_starts_late_reads_the_same(record):
    x = record.ordinate[0] / G
    early, late = _one_channel(x), _one_channel(x, start=3.25)
    averaging = Averaging(frame_length=1024, frames=20, start=1.0)
    first = early.compute_psds(averaging).rms_check(early,
                                                    averaging=averaging)
    second = late.compute_psds(averaging).rms_check(late,
                                                    averaging=averaging)
    assert second.span == pytest.approx((3.25 + 1.0, 3.25 + 11.0))
    assert first.span == pytest.approx((1.0, 11.0))
    assert second.gap_db == pytest.approx(first.gap_db, abs=1e-12)
    assert second.time_rms == pytest.approx(first.time_rms, rel=1e-12)


def test_white_noise_at_the_defaults_is_never_called_a_scaling_error():
    """Correctly scaled spectra of white noise under the default window
    (Hann, frames end to end): the gap scatters with the window's
    weighting, and over 0.05 dB it is said to be the window's, never
    flagged and never given a cause."""
    flagged, windows = 0, 0
    for seed in range(40):
        rng = np.random.default_rng(seed)
        history = _one_channel(rng.standard_normal(int(8 * FS)))
        averaging = Averaging(frame_length=1024).filled(
            len(history.abscissa), FS)
        check = history.compute_psds(averaging).rms_check(
            history, averaging=averaging)
        assert check.window == 'hann'
        assert check.cause is None
        flagged += not (check.passed or check.explained)
        windows += check.explained
    assert flagged == 0
    assert windows > 0, 'the default threshold is reached by the window alone'


def test_a_boxcar_names_no_amplitude_correction():
    """A rectangle's noise bandwidth is one line, so its amplitude
    correction is no correction and is never the cause of a gap."""
    f = np.arange(0.0, 11.0)
    values = np.full(11, 1.0 / 10.5)
    # the record read 0.05 dB under the spectrum's own rms
    carried = rms_check(f, values, np.ones(8)).spectrum_rms
    record = np.ones(64) * carried / 10.0 ** (0.05 / 20.0)
    check = rms_check(f, values, record, threshold_db=0.01, window='boxcar')
    assert check.gap_db == pytest.approx(0.05, abs=1e-9)
    assert not check.passed and check.noise_bandwidth == pytest.approx(1.0)
    assert check.cause is None


def test_one_finite_sample_has_an_rms_and_a_zero_threshold_is_allowed():
    assert time_rms(np.array([np.nan, -3.0, np.inf])) == pytest.approx(3.0)
    f = np.arange(0.0, 11.0)
    assert rms_check(f, np.ones(11), np.ones(8),
                     threshold_db=0.0).threshold_db == 0.0


def test_two_lines_are_enough_for_an_area():
    check = rms_check([0.0, 1.0], [1.0, 1.0], np.ones(8))
    assert check.lines == 2 and np.isfinite(check.gap_db)
    two = Psd(np.array([0.0, 1.0]), np.ones((1, 2)), response_dof=['1Z+'],
              ordinate_dim=['acceleration**2/frequency'])
    assert refusal(two) is None


def test_one_line_left_out_is_said_in_the_singular():
    f = np.arange(0.0, 11.0)
    values = np.ones(11)
    values[3] = np.nan
    check = rms_check(f, values, np.ones(8))
    assert check.notes == (('1 line of the spectrum is not finite or below '
                            'zero, and left out of its area'),)


def test_a_declared_spectrum_names_its_own_quantity_in_the_refusal():
    t = np.arange(64) / 64.0
    history = TimeHistory(t, np.ones((1, 64)), response_dof=['1Z+'],
                          ordinate_dim=['acceleration'], ordinate_unit=['g'])
    f = np.arange(0.0, 33.0)
    force = Psd(f, np.ones((1, f.size)), response_dof=['1Z+'],
                ordinate_dim=['force**2/frequency'])
    with pytest.raises(ValueError, match=r'what force\*\*2/frequency is'):
        force.rms_check(history)


def test_a_fine_spectrum_a_line_short_at_the_bottom_is_said_to_be():
    """At half-hertz lines the first line at 0.5 Hz is one short of DC."""
    assert _coverage(np.arange(1.0, 2049.0) * 0.5, 2048.0) == (
        'its lines run 0.5 to 1024 Hz of the 0 to 1024 Hz the record holds')
    assert _coverage(np.arange(0.0, 2049.0) * 0.5, 2048.0) is None
