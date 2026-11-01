"""The ASD-ratio transmissibility beside H1, and the random error of H1.

The ratio is the root of the response's autospectrum over the input's;
H1 is the cross-spectrum over the input's autospectrum. Pinned against
hand values of the random-error expression, against the identity that
ties the two estimates to the coherence, against a synthetic survey with
a known answer (a 300 Hz resonance at Q 20 driven by band-limited random,
with uncorrelated noise on the response channel), and against the
package's own FRF and coherence from the same frames. Then the project's
reading behind an FRF and the plot.
"""

from __future__ import annotations

import re

import numpy as np
import pytest

from hertzforge.core.averaging import Averaging
from hertzforge.core.data import TimeHistory
from hertzforge.core.transmissibility import (
    SMALL_ERROR,
    TransmissibilityPoint,
    frf_random_error,
    read_transmissibility,
)

G = 9.80665

FS = 5120.0

#: the resonance the survey passes through
FN, Q = 300.0, 20.0


def truth(f):
    """The base-excited resonator's transmissibility at `f`, the answer
    the survey is read against."""
    r = np.asarray(f, dtype=float) / FN
    z = 1.0 / (2.0 * Q)
    return (1.0 + 2j * z * r) / (1.0 - r ** 2 + 2j * z * r)


def survey(seconds=24.0, noise=0.6, overlap=0.5, force=False,
           input_noise=0.0):
    """Band-limited random from 20 to 2000 Hz at 0.04 g²/Hz on the input,
    the response through the resonance plus `noise` g rms of noise the
    input did not cause, stored in m/s². The response is the input's
    spectrum times the resonance, on a record that repeats, so every
    frame holds the steady answer and `truth` is exact at every line.
    `input_noise` g rms of noise is added to the input channel after the
    response is made, as an input sensor's own noise would be. With
    `force` the input is a force in newtons, the response an
    acceleration over it."""
    n = int(seconds * FS)
    f = np.fft.rfftfreq(n, 1.0 / FS)
    spectrum = np.fft.rfft(np.random.default_rng(1505).standard_normal(n))
    spectrum[(f < 20.0) | (f > 2000.0)] = 0.0
    x = np.fft.irfft(spectrum, n)
    x *= np.sqrt(0.04 * 1980.0) / np.std(x)
    y = np.fft.irfft(np.fft.rfft(x) * truth(f), n)
    y += np.random.default_rng(1506).standard_normal(n) * noise
    x = x + np.random.default_rng(1507).standard_normal(n) * input_noise
    t = np.arange(n) / FS
    history = TimeHistory(
        t, np.vstack([x, y]) * G, response_dof=['1Z+', '101Z+'],
        ordinate_dim=['force' if force else 'acceleration', 'acceleration'],
        ordinate_unit=['N' if force else 'g', 'g'])
    history.averaging = Averaging(frame_length=2048, overlap=overlap,
                                  window='hann').filled(n, FS)
    return history


@pytest.fixture(scope='module')
def history():
    return survey()


@pytest.fixture(scope='module')
def reading(history):
    return history.read_transmissibility('101Z+', '1Z+')


@pytest.mark.parametrize('coherence,averages,expected', [
    (0.9, 60, 0.0304), (0.5, 60, 0.0913), (0.99, 60, 0.00917),
    (0.9, 10, 0.0745)])
def test_the_random_error_matches_the_hand_values(coherence, averages,
                                                  expected):
    assert float(frf_random_error(coherence, averages)) == pytest.approx(
        expected, rel=2e-3)


def test_the_error_goes_as_one_over_the_root_of_the_averages():
    four = frf_random_error(0.8, 25.0)
    assert float(frf_random_error(0.8, 100.0)) == pytest.approx(four / 2.0)


def test_the_error_s_two_ends_are_zero_and_unbounded():
    error = frf_random_error(np.array([1.0, 0.0, np.nan]), 10)
    assert error[0] == 0.0
    assert np.isinf(error[1])
    assert np.isnan(error[2])


def test_the_error_refuses_what_is_not_a_coherence_or_an_average():
    with pytest.raises(ValueError, match='between 0 and 1'):
        frf_random_error(1.2, 10)
    with pytest.raises(ValueError, match='positive number of averages'):
        frf_random_error(0.5, 0)
    # an equivalent count discounted for overlap may fall below one
    assert np.isfinite(frf_random_error(0.5, 0.5))


def test_the_ratio_is_h1_over_the_root_of_the_coherence(reading):
    good = np.isfinite(reading.coherence) & (reading.coherence > 0.0)
    expected = reading.magnitude[good] / np.sqrt(reading.coherence[good])
    np.testing.assert_allclose(reading.asd_ratio[good], expected,
                               rtol=1e-12)
    over = reading.ratio_to_h1_db[good]
    np.testing.assert_allclose(over, -10.0 * np.log10(
        reading.coherence[good]), rtol=1e-9, atol=1e-9)


def test_the_ratio_never_reads_under_h1(reading):
    good = np.isfinite(reading.coherence) & (reading.coherence > 0.0)
    assert np.all(reading.asd_ratio[good]
                  >= reading.magnitude[good] * (1.0 - 1e-12))


def test_h1_is_the_package_s_h1_and_the_coherence_its_coherence(history,
                                                                reading):
    frf = history.compute_frfs(references=['1Z+'], method='H1')
    coherence = history.compute_multiple_coherence(references=['1Z+'])
    np.testing.assert_allclose(reading.h1, frf.ordinate[0], rtol=1e-12)
    np.testing.assert_allclose(reading.coherence, coherence.ordinate[0],
                               rtol=1e-12, atol=1e-15)


def test_the_ratio_is_the_root_of_the_two_psds(history, reading):
    psd = history.compute_psds()
    expected = np.sqrt(psd.ordinate[1] / psd.ordinate[0])
    np.testing.assert_allclose(reading.asd_ratio, expected, rtol=1e-12)


def test_where_the_coherence_is_high_the_two_agree_with_the_truth(reading):
    point = reading.at(100.0)
    assert point.coherence > 0.99
    assert point.asd_ratio == pytest.approx(point.magnitude, rel=0.005)
    assert point.magnitude == pytest.approx(abs(truth(point.frequency)),
                                            rel=0.02)


def test_where_the_coherence_falls_the_ratio_reads_high(reading):
    f = reading.frequency
    low = (f > 1200.0) & (f < 1900.0) & (reading.coherence < 0.5)
    assert low.sum() > 20
    over_truth = reading.asd_ratio[low] / np.abs(truth(f[low]))
    h1_over_truth = reading.magnitude[low] / np.abs(truth(f[low]))
    # the noise counted as response lifts the ratio by over 40 percent
    # there; H1 scatters about the truth
    assert np.median(over_truth) > 1.4
    assert np.median(h1_over_truth) == pytest.approx(1.0, abs=0.1)


def test_h1_lies_inside_its_band_about_as_often_as_one_error_says(reading):
    # away from the resonance, where the 2.5 Hz lines are too coarse for
    # the peak and the bias is the resolution's, not the noise's; and
    # where the error is small enough for its expression to hold
    f = reading.frequency
    lower, upper = reading.band(1.0)
    real = np.abs(truth(f))
    clean = ((f > 500.0) & (f < 1900.0)
             & (reading.error <= SMALL_ERROR))
    inside = (real >= lower) & (real <= upper)
    share = inside[clean].mean()
    assert clean.sum() > 300
    # one standard deviation holds about 68 percent of a normal error
    assert 0.55 < share < 0.80
    lower2, upper2 = reading.band(2.0)
    share2 = ((real >= lower2) & (real <= upper2))[clean].mean()
    assert 0.88 < share2 <= 1.0


def test_the_error_is_read_at_the_averages_the_frames_are_worth(history,
                                                                reading):
    worth = history.averaging.statistical_dof()
    assert reading.frames == history.averaging.frames == 119
    assert reading.averages == pytest.approx(worth.equivalent / 2.0)
    assert reading.averages < reading.frames
    np.testing.assert_allclose(
        reading.error, frf_random_error(reading.coherence, worth.equivalent
                                        / 2.0))
    assert any('worth' in note for note in reading.notes)


def test_frames_that_do_not_overlap_are_their_own_count():
    reading = survey(seconds=12.0, overlap=0.0).read_transmissibility(
        '101Z+', '1Z+')
    assert reading.averages == reading.frames
    assert not any('worth' in note for note in reading.notes)


def test_the_point_reads_the_nearest_line(reading):
    point = reading.at(1001.0)
    k = reading.line(1001.0)
    assert point.frequency == reading.frequency[k] == 1000.0
    assert point.h1 == reading.h1[k]
    assert point.asd_ratio == reading.asd_ratio[k]
    assert point.error == pytest.approx(reading.error[k])
    lower, upper = reading.band()
    assert point.lower == pytest.approx(lower[k])
    assert point.upper == pytest.approx(upper[k])


def test_h1_s_phase_swings_through_a_quarter_cycle_at_the_mode(reading):
    # the base-excited resonator's phase at its natural frequency is
    # minus the angle whose tangent is 1 / (2 zeta), just short of 90
    expected = np.degrees(np.angle(truth(FN)))
    assert reading.at(FN).phase == pytest.approx(expected, abs=4.0)
    assert reading.at(100.0).phase == pytest.approx(0.0, abs=2.0)


def test_the_band_has_no_lower_edge_where_it_reaches_zero():
    f = np.array([10.0, 20.0, 30.0])
    gxx = np.ones(3)
    gyy = np.ones(3)
    # coherences of 0.81, 0.01 and 1/9: the second's error is past one
    gxy = np.array([0.9, 0.1, 1.0 / 3.0]) + 0j
    reading = read_transmissibility(f, gxx, gyy, gxy, frames=4)
    lower, upper = reading.band(1.0)
    assert np.isfinite(lower[0]) and np.isnan(lower[1])
    assert np.isfinite(upper[1])
    assert reading.at(20.0).lower is None
    assert reading.at(10.0).lower == pytest.approx(lower[0])
    assert not reading.at(20.0).small
    assert any('poor rather than how poor' in note for note in reading.notes)


def test_a_line_with_no_input_has_no_reading():
    f = np.array([0.0, 10.0])
    reading = read_transmissibility(f, [0.0, 1.0], [1.0, 1.0],
                                    [0.0, 0.5 + 0j], frames=8)
    assert np.isnan(reading.h1[0]) and np.isnan(reading.asd_ratio[0])
    assert np.isnan(reading.coherence[0])
    assert reading.coherence[1] == pytest.approx(0.25)
    assert any('no input power' in note for note in reading.notes)


def test_bare_arrays_read_like_a_record(history, reading):
    psd = history.compute_psds()
    frequency = reading.frequency
    gxy = reading.h1 * psd.ordinate[0]
    again = read_transmissibility(frequency, psd.ordinate[0],
                                  psd.ordinate[1], gxy, frames=119,
                                  averages=reading.averages)
    np.testing.assert_allclose(again.asd_ratio, reading.asd_ratio,
                               rtol=1e-12)
    np.testing.assert_allclose(again.coherence, reading.coherence,
                               rtol=1e-9)


def test_what_is_not_an_estimate_is_refused():
    with pytest.raises(ValueError, match='two frames'):
        read_transmissibility([1.0], [1.0], [1.0], [1.0 + 0j], frames=1)
    with pytest.raises(ValueError, match='same length'):
        read_transmissibility([1.0, 2.0], [1.0], [1.0], [1.0 + 0j],
                              frames=4)
    with pytest.raises(ValueError, match='more than zero'):
        read_transmissibility([1.0], [1.0], [1.0], [1.0 + 0j], frames=4,
                              averages=0.0)


def test_a_reading_is_one_response_over_one_reference(history):
    with pytest.raises(ValueError, match='one reference'):
        history.read_transmissibility('101Z+')     # no channel's role is
    with pytest.raises(ValueError, match='not a response'):
        history.read_transmissibility('1Z+', '1Z+')
    with pytest.raises(ValueError, match='no channel matches'):
        history.read_transmissibility('101Z+', '7Z+')


def test_a_dof_holding_two_quantities_is_named_with_its_quantity():
    """An accelerometer and a velocity probe at one DOF: a bare DOF is
    refused whichever row comes first, and the pair picks the one."""
    n, fs = 8192, 2048.0
    x = np.random.default_rng(0).standard_normal(n)
    a, v = np.convolve(x, [0.5, 0.3], 'same'), np.convolve(x, [2.0, 1.0], 'same')
    for rows, dims, units in (((x, a, v), ['acceleration', 'velocity'], ['m/s**2', 'm/s']),
                              ((x, v, a), ['velocity', 'acceleration'], ['m/s', 'm/s**2'])):
        history = TimeHistory(np.arange(n) / fs, np.vstack(rows),
                              response_dof=['1Z+', '2Z+', '2Z+'],
                              ordinate_dim=['acceleration', *dims],
                              ordinate_unit=['m/s**2', *units])
        history.averaging = Averaging(frame_length=512, overlap=0.5,
                                      window='hann').filled(n, fs)
        with pytest.raises(ValueError, match='more than one quantity'):
            history.read_transmissibility('2Z+', '1Z+')
        reading = history.read_transmissibility(('2Z+', 'velocity'), '1Z+')
        assert reading.dimension == 'velocity/acceleration'


def test_a_force_reference_gives_a_ratio_in_acceleration_over_force():
    reading = survey(seconds=6.0, force=True).read_transmissibility('101Z+')
    assert reading.reference == '1Z+'
    assert reading.dimension == 'acceleration/force'


def test_the_point_describes_itself_in_display_units(reading):
    text = reading.at(1000.0).describe('g/g', 1.0)
    assert text.startswith('1000 Hz: H1 ')
    assert 'coherence 0.' in text and 'dB from H1' in text
    assert TransmissibilityPoint(1.0, 0j, 0.0, 0.0, np.inf, 1.0,
                                 4.0).describe().count('unbounded') == 1


def test_with_noise_on_the_input_the_ratio_is_the_nearer_and_not_called_high():
    # the same survey with its noise moved to the input channel: H1 reads
    # low by the coherence, the ratio low by its root, so the ratio is
    # the nearer of the two and the gap between them is not the
    # response's noise
    reading = survey(noise=0.0, input_noise=6.0).read_transmissibility(
        '101Z+', '1Z+')
    point = reading.at(1000.0)
    real = abs(truth(point.frequency))
    assert real == pytest.approx(0.1003, abs=1e-4)
    assert point.magnitude == pytest.approx(0.0729, abs=0.001)
    assert point.asd_ratio == pytest.approx(0.0840, abs=0.001)
    assert abs(point.asd_ratio - real) < abs(point.magnitude - real)
    assert point.magnitude / real == pytest.approx(point.coherence, rel=0.05)
    assert point.ratio_to_h1_db == pytest.approx(1.23, abs=0.05)
    text = point.describe()
    assert 'dB from H1' in text and 'over H1' not in text
    # and over the band where the coherence is down, the same holds on
    # the whole: the ratio sits between H1 and the truth
    f = reading.frequency
    low = (f > 500.0) & (f < 1900.0) & (reading.coherence < 0.8)
    assert low.sum() > 100
    h1_error = np.abs(reading.magnitude[low] / np.abs(truth(f[low])) - 1)
    ratio_error = np.abs(reading.asd_ratio[low] / np.abs(truth(f[low])) - 1)
    assert np.median(ratio_error) < np.median(h1_error)


def _with(history, row=None, sample=None, value=np.nan, zero=None):
    ordinate = np.array(history.ordinate, copy=True)
    if row is not None:
        ordinate[row, sample] = value
    if zero is not None:
        ordinate[zero] = 0.0
    copy = TimeHistory(history.abscissa, ordinate,
                       response_dof=list(history.response_dof),
                       ordinate_dim=list(history.ordinate_dim),
                       ordinate_unit=list(history.ordinate_unit))
    copy.averaging = history.averaging
    return copy


@pytest.mark.parametrize('row,channel,role', [(0, '1Z+', 'reference'),
                                               (1, '101Z+', 'response')])
def test_a_sample_that_is_not_a_number_is_refused_naming_its_channel(
        row, channel, role):
    history = _with(survey(seconds=6.0), row, 1000)
    with pytest.raises(ValueError, match=re.escape(
            f'{role} channel {channel} holds samples that are not finite')):
        history.read_transmissibility('101Z+', '1Z+')
    # an infinite sample the same
    with pytest.raises(ValueError, match='not finite'):
        _with(survey(seconds=6.0), row, 1000, np.inf).read_transmissibility(
            '101Z+', '1Z+')


def test_spectra_that_are_not_numbers_are_noted_as_such_and_nothing_else():
    f = np.array([10.0, 20.0, 30.0, 40.0])
    gxx = np.array([1.0, np.nan, 1.0, 1.0])
    gyy = np.array([1.0, 1.0, np.nan, 1.0])
    gxy = np.array([0.9, 0.9, 0.9, 0.9]) + 0j
    reading = read_transmissibility(f, gxx, gyy, gxy, frames=60)
    assert np.isnan(reading.h1[1:3]).all()
    assert np.isnan(reading.coherence[1:3]).all()
    assert reading.coherence[0] == pytest.approx(0.81)
    notes = ' '.join(reading.notes)
    assert '2 lines are not finite numbers in the input and response' in notes
    assert 'no input power' not in notes
    assert 'poor rather than how poor' not in notes


def test_a_dead_response_is_noted_and_not_read_as_a_poor_estimate():
    history = _with(survey(seconds=6.0), zero=1)
    reading = history.read_transmissibility('101Z+', '1Z+')
    driven = np.isfinite(reading.h1)
    assert driven.any()
    assert np.all(reading.h1[driven] == 0.0)
    assert np.all(reading.asd_ratio[driven] == 0.0)
    assert np.isnan(reading.coherence).all()
    assert np.isnan(reading.error).all()
    notes = ' '.join(reading.notes)
    assert 'a channel that recorded nothing' in notes
    assert 'poor rather than how poor' not in notes
    # and a response dead at some lines only says how many
    f = np.array([10.0, 20.0, 30.0])
    part = read_transmissibility(f, np.ones(3), np.array([1.0, 0.0, 1.0]),
                                 np.array([0.9, 0.0, 0.9]) + 0j, frames=60)
    assert any(note.startswith('1 lines carry no response power')
               for note in part.notes)


def test_a_coherence_a_hair_over_one_is_held_at_one():
    # |Gxy|² a few parts in 1e13 over Gxx Gyy is the arithmetic; unclipped
    # it would be a coherence over one, which the error expression refuses
    f = np.array([10.0, 20.0])
    gxx, gyy = np.array([2.0, 3.0]), np.array([5.0, 7.0])
    gxy = np.sqrt(gxx * gyy) * (1.0 + 3e-13) + 0j
    reading = read_transmissibility(f, gxx, gyy, gxy, frames=8)
    assert np.all(reading.coherence == 1.0)
    assert np.all(reading.error == 0.0)


def test_the_records_of_a_history_are_pooled_and_counted():
    # the survey cut into two captures of 12 s: each capture is framed
    # the same way and every frame is pooled, so the frames double and
    # the error is read at what two records of frames are worth
    whole = survey(seconds=24.0)
    n = len(whole.abscissa) // 2
    x, y = whole.ordinate
    history = TimeHistory(whole.abscissa[:n],
                          np.vstack([x[:n], y[:n], x[n:], y[n:]]),
                          response_dof=['1Z+', '101Z+', '1Z+', '101Z+'],
                          ordinate_dim=['acceleration'] * 4,
                          ordinate_unit=['g'] * 4)
    history.averaging = Averaging(frame_length=2048, overlap=0.5,
                                  window='hann').filled(n, FS)
    reading = history.read_transmissibility('101Z+', '1Z+')
    per = history.averaging.frames
    assert reading.frames == 2 * per
    two = history.averaging.statistical_dof(records=2).equivalent / 2.0
    one = history.averaging.statistical_dof(records=1).equivalent / 2.0
    assert reading.averages == pytest.approx(two)
    assert reading.averages == pytest.approx(2.0 * one, rel=0.02)
    frf = history.compute_frfs(references=['1Z+'], method='H1')
    np.testing.assert_allclose(reading.h1, frf.ordinate[0], rtol=1e-12)
    psd = history.compute_psds()
    np.testing.assert_allclose(reading.asd_ratio,
                               np.sqrt(psd.ordinate[1] / psd.ordinate[0]),
                               rtol=1e-12)


def test_a_force_reference_is_titled_and_noted_as_an_frf():
    force = survey(seconds=6.0, force=True).read_transmissibility('101Z+')
    assert not force.is_transmissibility
    assert force.title == 'ASD ratio and H1'
    assert any('not a transmissibility' in note for note in force.notes)
    motion = survey(seconds=6.0).read_transmissibility('101Z+', '1Z+')
    assert motion.is_transmissibility
    assert motion.title == 'Transmissibility'
    assert not any('not a transmissibility' in note for note in motion.notes)
    bare = read_transmissibility([1.0, 2.0], [1.0, 1.0], [1.0, 1.0],
                                 [0.5 + 0j, 0.5 + 0j], frames=4)
    assert bare.title == 'ASD ratio and H1'


def _project(method='H1'):
    from hertzforge import Project

    project = Project()
    record = survey(seconds=8.0)
    record.roles = {('1Z+', 'acceleration'): 'reference'}
    project.add('Run', record)
    return project, project.compute_frfs('Run', method)


@pytest.mark.parametrize('method', ['H1', 'Hv'])
def test_the_project_reads_behind_an_frf_from_its_frames(method):
    project, name = _project(method)
    assert project.averaging_of(name) == project['Run'].averaging
    reading = project.read_transmissibility(name)
    direct = project['Run'].read_transmissibility('101Z+', '1Z+')
    np.testing.assert_allclose(reading.h1, direct.h1, rtol=1e-12)
    assert (reading.response, reading.reference) == ('101Z+', '1Z+')
    if method == 'H1':
        np.testing.assert_allclose(reading.h1, project[name].ordinate[0],
                                   rtol=1e-12)


def test_the_project_reads_with_the_averaging_the_frf_was_made_with():
    project, name = _project()
    made = project['Run'].averaging
    # moved since: the FRF is stale, and its reading is still of its own
    # frames
    project['Run'].averaging = Averaging(frame_length=512, overlap=0.5,
                                         window='hann').filled(
        len(project['Run'].abscissa), FS)
    reading = project.read_transmissibility(name)
    assert reading.frames == made.frames
    assert reading.frequency[1] == pytest.approx(FS / 2048)


def test_the_project_refuses_what_has_no_autospectra_behind_it():
    from hertzforge.core.data import Frf

    project, name = _project()
    frf = project[name]
    # the same numbers brought in from a file: no history behind them
    project.add('Copy', Frf(frf.abscissa, frf.ordinate,
                            response_dof=list(frf.response_dof),
                            reference_dof=list(frf.reference_dof),
                            ordinate_dim=list(frf.ordinate_dim)))
    with pytest.raises(ValueError, match='not an FRF computed'):
        project.read_transmissibility('Copy')
    with pytest.raises(ValueError, match='not an FRF computed'):
        project.read_transmissibility('Run')


def test_the_project_refuses_an_frf_of_several_references():
    from hertzforge import Project

    record = survey(seconds=8.0)
    third = TimeHistory(record.abscissa,
                        np.vstack([record.ordinate,
                                   np.roll(record.ordinate[0], 977)]),
                        response_dof=['1Z+', '101Z+', '2Z+'],
                        ordinate_dim=['acceleration'] * 3,
                        ordinate_unit=['g'] * 3)
    third.averaging = record.averaging
    third.roles = {('1Z+', 'acceleration'): 'reference',
                   ('2Z+', 'acceleration'): 'reference'}
    project = Project()
    project.add('Run', third)
    name = project.compute_frfs('Run', 'H1')
    with pytest.raises(ValueError, match='2 references'):
        project.read_transmissibility(name)
