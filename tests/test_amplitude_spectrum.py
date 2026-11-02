"""The amplitude spectrum: a record's lines scaled so a sine reads its
amplitude in peak units, the window's correction applied and stated,
the frames' magnitudes averaged.

Pinned against numbers that do not depend on the implementation: the
coherent gain of a periodic Hann window is its mean, one half, so its
amplitude correction is 2; scipy's flat top's is its first coefficient,
0.21557895 (a correction of 4.639); a sine midway between two lines
reads 0.849 of itself under Hann (1.42 dB low, Harris's scalloping
loss), 2/pi under a rectangle (3.92 dB) and within 0.01 dB under the
flat top. The worked case is a 1 g peak tone at 500 Hz sampled at 5,120
a second in 1,024-sample frames (5 Hz lines), which reads 1.000 g peak
on its line under every window, and the same tone moved to 502.5 Hz,
halfway between lines. A tone given a fresh phase in each of 20
captures reads its amplitude here and averages away in
`compute_spectra`'s complex mean, which is left as it was. A random
floor's height goes as one over the root of the frame length, which is
why it is not a level.
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.core.amplitude_spectrum import (
    amplitude_correction,
    amplitude_lines,
    coherent_gain,
    scalloping,
)
from hertzforge.core.averaging import Averaging
from hertzforge.core.data import TimeHistory

G = 9.80665

RATE = 5120.0

FRAME = 1024


def history(*rows, rate=RATE, dofs=None):
    """A time history in g, stored in SI as every record is."""
    rows = np.vstack(rows) * G
    return TimeHistory(np.arange(rows.shape[1]) / rate, rows,
                       response_dof=dofs or [f'{k + 1}Z+'
                                             for k in range(len(rows))],
                       ordinate_dim=['acceleration'] * len(rows),
                       ordinate_unit=['g'] * len(rows))


def tone(frequency, amplitude=1.0, seconds=12.0, phase=0.3, rate=RATE):
    t = np.arange(round(seconds * rate)) / rate
    return amplitude * np.sin(2.0 * np.pi * frequency * t + phase)


def test_the_coherent_gain_is_the_windows_mean():
    assert coherent_gain('hann') == pytest.approx(0.5, abs=1e-15)
    assert coherent_gain('boxcar') == 1.0
    assert coherent_gain('flattop') == pytest.approx(0.21557895, abs=1e-12)
    assert coherent_gain('hamming') == pytest.approx(0.54, abs=1e-15)


def test_the_amplitude_correction_is_its_reciprocal():
    assert amplitude_correction('hann') == pytest.approx(2.0, abs=1e-14)
    assert amplitude_correction('flattop') == pytest.approx(4.6387, abs=1e-4)
    assert amplitude_correction('rectangle') == 1.0


def test_the_arithmetic_takes_an_averaging():
    averaging = Averaging(frame_length=512, window='flattop')
    assert coherent_gain(averaging) == coherent_gain('flattop', 512)
    assert amplitude_correction(averaging) == pytest.approx(
        1.0 / 0.21557895, rel=1e-12)
    # the averaging's own frame length, which a Kaiser's mean depends on
    short = Averaging(frame_length=16, window='kaiser')
    assert coherent_gain(short) == coherent_gain('kaiser', 16, 14.0)
    assert coherent_gain(short) != coherent_gain('kaiser')


def test_the_scalloping_is_harriss():
    assert scalloping('hann') == pytest.approx(0.8488, abs=1e-4)
    assert -20.0 * np.log10(scalloping('hann')) == pytest.approx(1.42,
                                                                  abs=0.005)
    assert scalloping('boxcar') == pytest.approx(2.0 / np.pi, abs=1e-6)
    assert -20.0 * np.log10(scalloping('flattop')) < 0.01


def test_the_lines_are_a_sine_amplitude_not_its_transform():
    """The raw transform reads A*N/2 under a rectangle and A*sum(w)/2
    under a window; the scaling takes both to A."""
    x = tone(500.0, 2.5)[:FRAME]
    shape = np.hanning(FRAME + 1)[:-1]
    lines = amplitude_lines(x * shape, shape)
    raw = np.abs(np.fft.rfft(x * shape))
    assert raw[100] == pytest.approx(2.5 * shape.sum() / 2.0, rel=1e-12)
    assert lines[100] == pytest.approx(2.5, rel=1e-12)


def test_a_window_that_sums_to_nothing_is_refused():
    with pytest.raises(ValueError, match='sums to nothing'):
        amplitude_lines(np.ones((2, 8)), np.zeros(8))


def test_a_tone_with_a_fresh_phase_each_capture_keeps_its_amplitude():
    """20 captures of one channel, each its own frame, the tone's phase
    new in each: the magnitudes average to the tone, and
    `compute_spectra`'s complex mean, unchanged, shrinks it."""
    phases = np.random.default_rng(15).uniform(0.0, 2.0 * np.pi, 20)
    rows = [tone(500.0, 1.0, seconds=FRAME / RATE, phase=p) for p in phases]
    record = history(*rows, dofs=['1Z+'] * 20)
    spectrum = record.amplitude_spectrum(window='boxcar')
    assert spectrum.frames == (20,)
    assert not spectrum.from_averaging
    assert spectrum.amplitude.shape[0] == 1
    assert spectrum.tallest(0) == (500.0, pytest.approx(G, rel=1e-12))
    raw = record.compute_spectra()
    line = int(np.argmin(np.abs(raw.abscissa - 500.0)))
    complex_mean = abs(raw.ordinate[0, line])
    # the raw spectrum is still unscaled and a complex mean: one
    # capture alone reads A*N/2 there, and twenty at random phases far
    # less
    assert abs(history(rows[0]).compute_spectra().ordinate[0, line]) == \
        pytest.approx(G * FRAME / 2.0, rel=1e-12)
    assert complex_mean < 0.5 * G * FRAME / 2.0
    expected = abs(np.mean(np.exp(1j * (phases - np.pi / 2.0))))
    assert complex_mean == pytest.approx(G * FRAME / 2.0 * expected,
                                         rel=1e-9)


def test_a_random_floor_is_not_a_level():
    """The mean magnitude of a white floor goes as one over the root of
    the frame length: four times the lines, half the floor, while the
    tone on it reads the same."""
    noise = np.random.default_rng(8).standard_normal(round(RATE * 48.0))
    x = noise + tone(500.0, 2.0, seconds=48.0)
    floors, tones = [], []
    for block in (FRAME, 4 * FRAME):
        record = history(x)
        record.averaging = Averaging(frame_length=block, overlap=0.5,
                                     window='hann').filled(len(x), RATE)
        spectrum = record.amplitude_spectrum()
        f = spectrum.frequencies
        away = (f > 1000.0) & (f < 2000.0)
        floors.append(float(np.mean(spectrum.amplitude[0, away])) / G)
        tones.append(spectrum.tallest(0)[1] / G)
        # a Rayleigh magnitude's mean, through the window's sums:
        # 2*sqrt(pi/4)*sqrt(sum w^2)/sum w for unit variance
        w = np.hanning(block + 1)[:-1]
        expected = np.sqrt(np.pi) * np.sqrt(np.sum(w ** 2)) / np.sum(w)
        assert floors[-1] == pytest.approx(expected, rel=0.03)
    assert floors[0] / floors[1] == pytest.approx(2.0, rel=0.03)
    assert tones == [pytest.approx(2.0, abs=0.03)] * 2


def test_without_an_averaging_each_record_is_one_frame():
    record = history(tone(500.0, seconds=2.0))
    spectrum = record.amplitude_spectrum()
    assert spectrum.frames == (1,)
    assert not spectrum.from_averaging
    assert spectrum.spacing == pytest.approx(0.5)
    assert spectrum.describe().endswith('1 frame averaged as magnitudes')
    assert spectrum.tallest() == spectrum.tallest(0)
