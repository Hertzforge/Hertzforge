"""A replicated record judged segment by segment.

`replication.compare` reads a run as a whole, and a short event's miss
hides in that figure because the event carries a small share of the
record's mean square. These pin the segmented judgment: each kind's
readings against hand values (a control that is its reference, one a
known number of decibels over, one that misses a known share of a
peak), the taper, the floors, the windows and the major peaks by
construction, a dead or unjudged reading never counted as a pass, and
then the case the judgment exists for, a rough-road record whose
whole-record rms error reads under five percent while its pothole
fails by its spectrum and its peaks.
"""

from __future__ import annotations

import numpy as np
import pytest
from scipy.signal import butter, sosfiltfilt

from hertzforge.core import replication_segments as rs
from hertzforge.core.replication_segments import (
    Segment,
    aligned_traces,
    judge_segments,
    major_peaks,
    replication_segments,
    tapered_cosine,
)

#: the road record's sample rate
FS = 2048.0

#: the road record's impacts: when, and how large, in g
IMPACTS = ((5.2, 9.0), (9.7, 7.5), (13.1, 10.0), (17.8, 8.0), (21.4, 11.0),
           (26.0, 7.0), (32.0, 14.0), (36.5, 9.5), (39.9, 8.5), (52.3, 10.5),
           (55.6, 7.5), (57.1, 9.0))

#: the two modes each impact rings: Hz, and their share of its peak
MODES = ((60.0, 0.4), (250.0, 0.6))

#: the three segments the record is judged by
ROAD = [(6.0, 30.0, 'stationary', 'rough road'),
        (31.9, 32.7, 'shock', 'pothole'),
        (41.0, 51.0, 'transient', 'braking')]

#: the road record's seed, the share of the pothole's 250 Hz ring the
#: run misses, and its error elsewhere as a share of the reference's
#: rms (see `road`). On this seed's random floor, missing 70 percent of
#: the ring leaves the pothole about 7 dB low at the top of its
#: spectrum in both polarities; how low depends on how the floor near
#: 250 Hz falls inside the pothole's 0.8 s, which is why a seed is named
SEED, MISSED, ERROR = 27, 0.7, 0.044


def ring(t, at, peak, frequency, zeta=0.06):
    """One mode ringing down from a hit at `at`: its acceleration, in g,
    starting at `peak` with the velocity at zero."""
    w = 2.0 * np.pi * frequency
    s = np.clip(t - at, 0.0, None)
    return np.where(t >= at, peak * np.exp(-zeta * w * s)
                    * (np.cos(w * s) - zeta * np.sin(w * s)), 0.0)


def _filtered(x, low, high=None):
    """Zero-phase second-order Butterworth, high-pass and optionally
    low-pass: the same filters on every trace they touch."""
    y = sosfiltfilt(butter(2, low, 'highpass', fs=FS, output='sos'), x)
    if high is not None:
        y = sosfiltfilt(butter(2, high, 'lowpass', fs=FS, output='sos'), y)
    return y


def road(seed=SEED, missed=MISSED, error=ERROR):
    """(reference, control): a minute of a vehicle crossing a rough
    course, in g, and a replication of it that converged.

    The reference is Gaussian ride motion and a broadband floor to
    400 Hz, entered and left over 2 s, swelling at 46 s as the vehicle
    brakes, with a dozen impacts each ringing a bracket's modes at 60
    and 250 Hz, the largest a 14 g pothole at 32 s; high-passed at 3 Hz.
    The control is the reference plus an error band-limited to the
    3 to 400 Hz test band at `error` of the reference's rms, less
    `missed` of the pothole's 250 Hz ring: the event a plant identified
    at low level under-drives.
    """
    rng = np.random.default_rng(seed)
    n = int(60.0 * FS)
    t = np.arange(n) / FS
    f = np.fft.rfftfreq(n, 1.0 / FS)
    corners = np.log([0.7, 3.5, 6.0, 150.0, 400.0])
    levels = np.log([0.02, 0.02, 0.015, 0.015, 0.0015])
    asd = np.zeros_like(f)
    inside = (f >= 0.7) & (f <= 400.0)
    asd[inside] = np.exp(np.interp(np.log(f[inside]), corners, levels))
    x = np.fft.irfft(np.fft.rfft(rng.standard_normal(n))
                     * np.sqrt(asd * FS / 2.0), n)
    x *= np.clip(np.minimum(t, 60.0 - t) / 2.0, 0.0, 1.0)
    x *= 1.0 + 0.8 * np.exp(-((t - 46.0) / 2.5) ** 2)
    for at, peak in IMPACTS:
        for frequency, share in MODES:
            x += ring(t, at, share * peak, frequency)
    reference = _filtered(x - x.mean(), 3.0)
    noise = _filtered(rng.standard_normal(n), 3.0, 400.0)
    control = (reference + error * reference.std() / noise.std() * noise
               - missed * ring(t, 32.0, MODES[1][1] * 14.0, MODES[1][0]))
    return reference, control


@pytest.fixture(scope='module')
def road_traces():
    return road()


@pytest.fixture(scope='module')
def judged_road(road_traces):
    reference, control = road_traces
    return judge_segments(reference, control, FS, ROAD, band=(5.0, 400.0),
                          window=0.5)


def noise(seconds=8.0, rate=1024.0, seed=5):
    """White Gaussian noise, a stationary reference."""
    return np.random.default_rng(seed).standard_normal(int(seconds * rate))


def _check(result, segment, reading):
    """One check of one segment, by the segment's place and the
    check's name."""
    (found,) = [c for c in result.segments[segment].checks
                if c.reading == reading]
    return found


def test_the_taper_rises_over_its_share_and_holds_one_between():
    """10 percent of 100 samples: five rising from zero at each end,
    one across the ninety between, and symmetric."""
    w = tapered_cosine(100, 10.0)
    assert w[0] == 0.0 and w[-1] == 0.0
    assert np.all(np.diff(w[:6]) > 0.0)
    assert np.all(w[5:95] == 1.0)
    assert np.allclose(w, w[::-1])
    assert np.all(tapered_cosine(50, 0.0) == 1.0)
    # all of it under the cosine is a symmetric Hann window, zero at
    # both ends
    n = 64
    hann = 0.5 - 0.5 * np.cos(2.0 * np.pi * np.arange(n) / (n - 1))
    assert np.allclose(tapered_cosine(n, 100.0), hann)


@pytest.mark.parametrize('samples, percent', [(0, 10.0), (10, -1.0),
                                              (10, 101.0)])
def test_a_taper_that_cannot_be_drawn_is_refused(samples, percent):
    with pytest.raises(ValueError):
        tapered_cosine(samples, percent)


def test_the_major_peaks_are_the_largest_each_way_and_few():
    """Crests 5, 2 and 1 and troughs -3 and -7: five peaks, so a tenth
    of them allows one each way, the largest; asked for half, two."""
    x = np.array([0.0, 5.0, 0.0, -3.0, 0.0, 2.0, 0.0, -7.0, 0.0, 1.0, 0.0])
    assert list(major_peaks(x)) == [1, 7]
    assert list(major_peaks(x, 2, share_percent=100.0)) == [1, 5, 7, 3]
    assert list(major_peaks(x, 1, share_percent=100.0)) == [1, 7]


@pytest.mark.parametrize('kind, readings', [
    ('stationary', ('ASD', 'variance')),
    ('transient', ('envelope', 'normalized ASD')),
    ('shock', ('SRS positive', 'SRS negative', 'major peaks'))])
def test_a_control_that_is_its_reference_passes_every_check_at_zero(
        kind, readings):
    x = noise()
    result = judge_segments(x, x.copy(), 1024.0, [(1.0, 7.0, kind)])
    (segment,) = result.segments
    assert [c.reading for c in segment.checks] == list(readings)
    assert segment.passed and result.passed
    for check in segment.checks:
        assert check.outside == 0 and check.judged > 0
        assert np.allclose(check.values, 0.0, atol=1e-9), check.reading
    assert result.rms_error == 0.0


def test_two_db_over_reads_two_db_in_every_level_reading():
    """The reference scaled by 2 dB: the ASD reads +2 dB at every line
    and passes ±3; the mean square reads 58.5 % over and fails +10;
    the envelope reads +2 dB and fails ±1; the normalized ASD divides
    the level out and reads 0 dB; the SRS reads +2 dB and passes
    -6/+3; every peak is 25.9 % high and fails ±20."""
    x = noise()
    gain = 10.0 ** (2.0 / 20.0)
    result = judge_segments(x, gain * x, 1024.0, [
        (0.5, 3.5, 'stationary'), (3.5, 6.0, 'transient'),
        (6.0, 7.5, 'shock')])
    asd = _check(result, 0, 'ASD')
    assert np.allclose(asd.values, 2.0) and asd.passed
    variance = _check(result, 0, 'variance')
    assert variance.values[0] == pytest.approx(100.0 * (gain ** 2 - 1.0))
    assert variance.passed is False
    envelope = _check(result, 1, 'envelope')
    assert np.allclose(envelope.values, 2.0) and envelope.passed is False
    normalized = _check(result, 1, 'normalized ASD')
    assert np.allclose(normalized.values, 0.0, atol=1e-9)
    assert normalized.passed
    for kind in ('positive', 'negative'):
        srs = _check(result, 2, f'SRS {kind}')
        assert np.allclose(srs.values, 2.0) and srs.passed
    peaks = _check(result, 2, 'major peaks')
    assert np.allclose(peaks.values, 100.0 * (gain - 1.0))
    assert peaks.passed is False
    assert [s.passed for s in result.segments] == [False, False, False]
    assert result.rms_error == pytest.approx(100.0 * (gain - 1.0))


def test_a_reading_on_a_tolerances_edge_is_inside_it():
    """Half the reference is 6.02 dB down, just past -6; 6.0 dB down is
    on the edge and inside."""
    x = noise()
    shock = [(2.0, 4.0, 'shock')]
    for db, passed in ((20.0 * np.log10(0.5), False), (-6.0, True)):
        result = judge_segments(x, x * 10.0 ** (db / 20.0), 1024.0, shock,
                                peak_percent=60.0)
        assert _check(result, 0, 'SRS positive').passed is passed, db


def test_the_asd_share_counts_lines_rather_than_requiring_every_one():
    """A tone on one line of a noise record lifts that line and its
    Hann neighbors far over the band: every line fails the default,
    which asks for all of them, and a 95 percent share passes."""
    rate = 1024.0
    x = noise(rate=rate)
    t = np.arange(x.size) / rate
    tone = x + 3.0 * np.sin(2.0 * np.pi * 100.0 * t)
    every = judge_segments(x, tone, rate, [(0.0, 8.0, 'stationary')])
    asd = _check(every, 0, 'ASD')
    assert 1 <= asd.outside <= 5
    assert asd.passed is False and asd.needed is None
    share = judge_segments(x, tone, rate, [(0.0, 8.0, 'stationary')],
                           asd_percent=95.0, variance_percent=(-10, 1000))
    asd = _check(share, 0, 'ASD')
    assert asd.passed and asd.needed == int(np.ceil(0.95 * asd.judged))


def test_the_asd_is_a_density_at_the_line_spacing_asked_for():
    """A 2-amplitude tone at 100 Hz in frames of 512 samples at
    1024 Hz: lines 2 Hz apart, the peak at 100 Hz, and the spectrum
    summing to the tone's mean square, 2. Judged with `lines=2`, the
    lines start at 2 Hz and step by 2."""
    rate = 1024.0
    t = np.arange(int(8 * rate)) / rate
    f, asd = rs._asd(2.0 * np.sin(2.0 * np.pi * 100.0 * t), rate, 512)
    df = f[1] - f[0]
    assert df == 2.0 and f[np.argmax(asd)] == 100.0
    assert np.sum(asd) * df == pytest.approx(2.0, rel=1e-3)
    x = noise(rate=rate)
    result = judge_segments(x, x, rate, [(0.0, 8.0, 'stationary')],
                            lines=2.0)
    asd_check = _check(result, 0, 'ASD')
    assert list(asd_check.where[:3]) == [2.0, 4.0, 6.0]
    assert asd_check.judged == 256


def test_lines_far_below_the_references_highest_are_not_judged():
    """A reference low-passed hard at 100 Hz and a control leaking a
    hundredth of white noise past it: unfloored, the lines past the
    skirt would read over 120 dB high. Only the lines within 40 dB of
    the reference's highest are judged, 1 to 126 Hz, and it passes."""
    rate = 1024.0
    x = noise(rate=rate)
    reference = sosfiltfilt(butter(8, 100.0, fs=rate, output='sos'), x)
    control = reference + 1e-2 * noise(rate=rate, seed=9)
    result = judge_segments(reference, control, rate,
                            [(1.0, 7.0, 'stationary')], taper_percent=0.0)
    asd = _check(result, 0, 'ASD')
    f, want = rs._asd(reference[1024:7168], rate, 1024)
    floor = want[f > 0].max() * 10.0 ** (-rs.ASD_FLOOR_DB / 10.0)
    assert np.array_equal(asd.where, f[(f > 0) & (want >= floor)])
    assert (asd.where[0], asd.where[-1], asd.judged) == (1.0, 126.0, 126)
    assert asd.passed


def test_srs_points_far_below_the_references_highest_are_not_judged():
    """A 150 Hz ring and a control with a 1 Hz drift under it: at the
    lowest natural frequencies the drift is all the control holds and
    would read 14 dB over. Points more than 40 dB under the reference's
    highest are not judged; the first judged is 10.68 Hz, and it
    passes. With no band the SRS runs from one cycle in the segment,
    0.5 Hz, to a fifth of the sample rate."""
    from hertzforge.core.srs import octave_frequencies, peaks

    rate = 1024.0
    t = np.arange(int(8 * rate)) / rate
    hit = ring(t, 1.5, 5.0, 150.0)
    drift = 0.02 * np.sin(2.0 * np.pi * t)
    result = judge_segments(hit, hit + drift, rate, [(1.0, 3.0, 'shock')])
    srs = _check(result, 0, 'SRS positive')
    every = octave_frequencies(0.5, 204.8)
    up, _down = peaks(hit[1024:3072], every, rate)
    kept = every[up >= up.max() * 10.0 ** (-40.0 / 20.0)]
    assert np.array_equal(srs.where, kept)
    assert srs.where[0] == pytest.approx(10.6787, abs=1e-4)
    assert srs.where[-1] == pytest.approx(every[-1]) and every[0] == 0.5
    assert srs.judged == 52 and srs.passed
    # the same with the reference's own content: every point judged
    x = noise(rate=rate)
    whole = _check(judge_segments(x, x, rate, [(2.0, 4.0, 'shock')]), 0,
                   'SRS positive')
    assert whole.where[0] == 0.5
    assert whole.where[-1] == pytest.approx(203.187, abs=1e-3)


def test_the_envelope_window_is_a_hundredth_of_the_record_by_default():
    """8 s at 1024 Hz: windows of 82 samples, 74 whole ones in a
    segment from 1 to 7 s, each placed at its start on the reference's
    clock."""
    x = noise()
    result = judge_segments(x, x, 1024.0, [(1.0, 7.0, 'transient')])
    envelope = _check(result, 0, 'envelope')
    assert envelope.judged == 74
    assert envelope.where[0] == 1.0
    assert np.allclose(np.diff(envelope.where), 82 / 1024.0)


def test_the_peaks_are_placed_on_the_references_clock():
    """One crest at sample 2100 and one trough at 2500, judged in a
    segment from 2 s: the peaks are read at 2.051 and 2.441 s."""
    y = np.zeros(8192)
    y[2100], y[2500] = 3.0, -2.0
    result = judge_segments(y, y, 1024.0, [(2.0, 3.0, 'shock')])
    peaks = _check(result, 0, 'major peaks')
    assert list(peaks.where) == [2100 / 1024.0, 2500 / 1024.0]


@pytest.mark.parametrize('kind', ['stationary', 'transient', 'shock'])
def test_a_dead_control_fails_every_kind(kind):
    """A control of zeros reads minus infinity wherever the reference
    asks for something, and every spectral and level check fails."""
    x = noise()
    result = judge_segments(x, np.zeros_like(x), 1024.0, [(1.0, 7.0, kind)])
    (segment,) = result.segments
    assert segment.passed is False and result.passed is False
    for check in segment.checks:
        if check.reading in ('variance', 'major peaks'):
            assert np.all(check.values == -100.0)
            continue
        assert check.judged > 0 and np.all(check.values == -np.inf)
        assert check.passed is False, check.reading


def test_samples_that_are_not_numbers_are_refused():
    """A control with a dead half recorded as NaN would pass on the
    half that holds numbers; it is refused instead."""
    x = noise()
    dead = x.copy()
    dead[x.size // 2:] = np.nan
    with pytest.raises(ValueError, match='control holds 4096 samples'):
        judge_segments(x, dead, 1024.0, [(0.0, 8.0, 'stationary')])
    with pytest.raises(ValueError, match='reference holds 1 samples'):
        judge_segments(np.r_[x[:-1], np.inf], x, 1024.0,
                       [(0.0, 8.0, 'stationary')])


def test_a_segment_not_judged_is_not_counted_as_passing():
    """A test band above a fifth of the sample rate leaves a shock's
    SRS with nothing to judge: the shock is not judged though its peaks
    pass, the record is not judged, and the line counts it apart. A
    failing check still fails its segment."""
    x = noise()
    marks = [(0.5, 3.5, 'stationary'), (4.0, 6.0, 'shock')]
    result = judge_segments(x, x, 1024.0, marks, band=(300.0, 400.0))
    stationary, shock = result.segments
    assert stationary.passed is True
    assert [c.passed for c in shock.checks] == [None, None, True]
    assert shock.passed is None and result.passed is None
    assert [s.label for s in result.not_judged] == ['shock 1']
    assert result.describe() == ('whole-record rms error 0.00 %; 1 of 2 '
                                 'segments pass, 1 not judged (shock 1)')
    assert shock.describe() == 'shock 1 (shock, 4 to 6 s): not judged'
    louder = judge_segments(x, 2.0 * x, 1024.0, marks, band=(300.0, 400.0))
    assert louder.segments[1].passed is False and louder.passed is False
    assert louder.describe().endswith('2 of 2 segments fail '
                                      '(stationary 1, shock 1)')


def test_an_unnamed_segment_is_called_by_its_kind_and_place():
    x = noise()
    result = judge_segments(x, x, 1024.0, [
        (0.0, 1.0, 'stationary'), Segment(1.0, 2.0, 'shock'),
        (2.0, 3.0, 'stationary', 'idle')])
    assert [s.label for s in result.segments] == ['stationary 1', 'shock 1',
                                                  'idle']


@pytest.mark.parametrize('segments, kwargs, message', [
    ([(1.0, 2.0, 'sine')], {}, 'not a kind'),
    ([(2.0, 2.0, 'shock')], {}, 'does not end after'),
    ([(7.0, 9.0, 'shock')], {}, 'outside the record'),
    ([(1.0, 1.001, 'shock')], {}, 'too few'),
    ([(1.0, 2.0, 'shock')], {'srs_db': (3.0, -6.0)}, 'not a band'),
    ([(1.0, 2.0, 'shock')], {'band': (0.0, 400.0)}, 'above 0 Hz'),
    ([(1.0, 2.0, 'stationary')], {'asd_percent': 0.0}, 'share of the lines'),
])
def test_marks_or_tolerances_that_cannot_be_judged_are_refused(
        segments, kwargs, message):
    x = noise()
    with pytest.raises(ValueError, match=message):
        judge_segments(x, x, 1024.0, segments, **kwargs)


def test_traces_of_different_lengths_are_refused():
    x = noise()
    with pytest.raises(ValueError, match='align them first'):
        judge_segments(x, x[:-1], 1024.0, [(1.0, 2.0, 'shock')])


def test_the_whole_record_figure_is_small_while_the_pothole_fails(
        judged_road):
    """The rough-road record: under five percent over the whole record,
    and still failing, because its pothole is about 7 dB low in both
    polarities at the top of its spectrum and two of its four major
    peaks are more than 20 percent short."""
    result = judged_road
    assert 4.6 < result.rms_error < 5.0
    assert result.passed is False
    assert [s.label for s in result.failed] == ['pothole']
    pothole = result.segments[1]
    # the pothole holds a percent and a half of the record's mean
    # square, which is why its own error of 17.53 percent hides
    assert pothole.share == pytest.approx(1.4905, abs=1e-4)
    assert pothole.rms_error == pytest.approx(17.530, abs=1e-3)
    for kind in ('positive', 'negative'):
        srs = _check(result, 1, f'SRS {kind}')
        assert srs.passed is False
        assert -8.0 < srs.low < -6.5, (kind, srs.low)
        # short at the top, where the 250 Hz ring was under-driven,
        # and near the reference below 60 Hz
        assert np.all((srs.where[srs.values < -6.0] > 200.0)
                      & (srs.where[srs.values < -6.0] < 340.0))
        assert np.all(np.abs(srs.values[srs.where < 60.0]) < 0.6)
    peaks = _check(result, 1, 'major peaks')
    assert peaks.judged == 4 and peaks.outside == 2
    assert np.sum(peaks.values < -20.0) == 2
    # the hit and its first rebounds, on the reference's clock
    assert np.round(peaks.where * FS).astype(int).tolist() == [
        65536, 65601, 65548, 66798]


def test_a_pothole_marked_tight_is_judged_on_its_whole_spectrum(
        road_traces):
    """The pothole marked from 31.99 s, a hundredth of a second before
    the hit: under a taper its first and largest cycles would be
    weighted down in both traces alike and the SRS would read about
    3 dB low and pass. The SRS is read untapered, so it is 7.38 and
    6.72 dB low and fails, and the taper does not move it."""
    reference, control = road_traces
    tight = [(31.99, 32.7, 'shock', 'pothole')]
    readings = []
    for taper in (rs.TAPER_PERCENT, 0.0):
        result = judge_segments(reference, control, FS, tight,
                                band=(5.0, 400.0), taper_percent=taper)
        readings.append([_check(result, 0, f'SRS {kind}').values
                         for kind in ('positive', 'negative')])
        positive = _check(result, 0, 'SRS positive')
        negative = _check(result, 0, 'SRS negative')
        assert positive.low == pytest.approx(-7.377, abs=1e-3)
        assert negative.low == pytest.approx(-6.720, abs=1e-3)
        assert positive.passed is False and negative.passed is False
    for tapered, untapered in zip(*readings):
        assert np.array_equal(tapered, untapered)


def test_the_stationary_and_transient_segments_pass_with_room(judged_road):
    road_segment, _pothole, braking = judged_road.segments
    assert road_segment.passed and braking.passed
    asd = _check(judged_road, 0, 'ASD')
    assert max(-asd.low, asd.high) < 0.2
    assert abs(_check(judged_road, 0, 'variance').values[0]) < 1.0
    envelope = _check(judged_road, 2, 'envelope')
    assert envelope.judged == 20 and max(-envelope.low, envelope.high) < 0.2
    # the windows are placed on the reference's clock, 0.5 s apart
    assert np.allclose(envelope.where, 41.0 + 0.5 * np.arange(20))


def test_the_whole_record_spectrum_cannot_see_the_pothole():
    """The same run read the controller's way, `replication.compare`
    over the whole record: the waveform error is the judgment's
    whole-record figure, and the maximax spectrum's worst deviation is
    nowhere near the pothole's."""
    from hertzforge.core.data import TimeHistory, TransientSpecification
    from hertzforge.core.replication import compare

    reference, control = road()
    t = np.arange(reference.size) / FS

    def record(x, kind):
        return kind(t, x[None] * 9.80665, response_dof=['1Z+'],
                    ordinate_dim=['acceleration'], ordinate_unit=['g'])

    run = record(control, TimeHistory)
    target = record(reference, TransientSpecification)
    (row,) = compare(run, target)
    result = replication_segments(run, target, ROAD, band=(5.0, 400.0),
                                  window=0.5)
    assert row['waveform'] == pytest.approx(result.rms_error, rel=1e-9)
    assert row['srs'] > -3.0
    assert result.channel == '1Z+' and result.lag == 0
    assert result.passed is False


def test_a_run_late_on_its_reference_is_aligned_before_it_is_judged():
    """The control three samples late: `lag_of` finds the shift, and
    the judgment is the aligned one's."""
    from hertzforge.core.data import TimeHistory, TransientSpecification

    rate = 1024.0
    x = noise(rate=rate)
    late = np.r_[np.zeros(3), x]
    reference = TransientSpecification(
        np.arange(x.size) / rate, x[None], response_dof=['1Z+'],
        ordinate_dim=['acceleration'])
    run = TimeHistory(np.arange(late.size) / rate, late[None],
                      response_dof=['1Z+'], ordinate_dim=['acceleration'])
    time, ref, ctl, dof, lag = aligned_traces(run, reference)
    assert lag == 3 and dof == '1Z+'
    assert np.array_equal(ref, ctl) and time[1] == pytest.approx(1 / rate)
    result = replication_segments(run, reference, [(1.0, 3.0, 'shock')])
    assert result.lag == 3 and result.rms_error == 0.0 and result.passed


def test_a_channel_the_reference_does_not_name_is_refused():
    from hertzforge.core.data import TimeHistory, TransientSpecification

    x = noise()
    t = np.arange(x.size) / 1024.0
    reference = TransientSpecification(t, x[None], response_dof=['1Z+'],
                                       ordinate_dim=['acceleration'])
    run = TimeHistory(t, x[None], response_dof=['2X+'],
                      ordinate_dim=['acceleration'])
    with pytest.raises(ValueError, match='no channel 3Y'):
        aligned_traces(run, reference, channel='3Y+')
    with pytest.raises(ValueError, match='no acceleration record at 1Z'):
        aligned_traces(run, reference)


def test_the_readout_says_which_segment_failed_and_on_what(judged_road):
    assert judged_road.describe() == (
        'whole-record rms error '
        f'{judged_road.rms_error:.2f} %; 1 of 3 segments fail (pothole)')
    pothole = judged_road.segments[1]
    assert pothole.describe() == (
        'pothole (shock, 31.9 to 32.7 s): fails on SRS positive, '
        'SRS negative, major peaks')
    peaks = _check(judged_road, 1, 'major peaks')
    assert peaks.describe().endswith('(±20 %), 2 of 4 outside: fails')
    variance = _check(judged_road, 0, 'variance')
    assert '(up to +10 %)' in variance.describe()
    assert variance.describe().endswith(': passes')


def _small_pair(missed=0.5):
    """A short run and its reference as objects: noise with one ring a
    third of the way in that the run plays at `1 - missed` of its size."""
    from hertzforge.core.data import TimeHistory, TransientSpecification

    rate = 1024.0
    x = 0.2 * noise(rate=rate)
    t = np.arange(x.size) / rate
    hit = ring(t, 3.0, 5.0, 100.0)
    reference = TransientSpecification(
        t, (x + hit)[None] * 9.80665, response_dof=['1Z+'],
        ordinate_dim=['acceleration'], ordinate_unit=['g'])
    run = TimeHistory(t, (x + (1.0 - missed) * hit)[None] * 9.80665,
                      response_dof=['1Z+'], ordinate_dim=['acceleration'],
                      ordinate_unit=['g'])
    return run, reference


SMALL = [(0.5, 2.5, 'stationary', 'idle'), (2.9, 3.4, 'shock', 'hit'),
         (4.0, 7.5, 'transient', 'run-up')]
