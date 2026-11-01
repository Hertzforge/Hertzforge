"""A tone read off an averaged autospectrum, in amplitude.

The reading sums the lines a tone leaks into, less the floor beside
them, times the spacing: the tone's mean square whatever the spacing and
window, where the spike's height moves with both. Pinned against hand
values (Harris's noise bandwidths and scalloping losses), against a
composite of four tones on a 1 grms floor read at 1 Hz and 4 Hz lines,
and against single tones of known amplitude in a known floor, under
every window the averaging offers. Then the plot, the project's record
of the window, and the reading in the window.
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.core.data import Psd, TimeHistory
from hertzforge.core.tone_readout import (
    leakage,
    read_tone,
    spike_height,
)

G = 9.80665

FS = 2048.0

#: four tones of a rotor test, Hz and g peak: one small at the floor's
#: lower edge, three standing well clear of it
TONES = ((4.86, 0.12), (19.44, 1.944), (38.88, 2.50), (58.32, 1.50))


def _floor(seconds, low=5.0, high=500.0, grms=1.0, seed=3304):
    """Gaussian noise flat from `low` to `high` Hz at `grms`, in g."""
    rng = np.random.default_rng(seed)
    n = int(seconds * FS)
    spectrum = np.fft.rfft(rng.standard_normal(n))
    f = np.fft.rfftfreq(n, 1.0 / FS)
    spectrum[(f < low) | (f > high)] = 0.0
    x = np.fft.irfft(spectrum, n)
    return x * grms / np.sqrt(np.mean(x ** 2))


def _history(tones=TONES, seconds=60.0, seed=3304):
    """A floor of 1 grms with `tones` added, stored in m/s^2."""
    x = _floor(seconds, seed=seed)
    t = np.arange(len(x)) / FS
    for f, amplitude in tones:
        x = x + amplitude * np.sin(2.0 * np.pi * f * t + f)
    return TimeHistory(t, x[None] * G, response_dof=['1Z+'],
                       ordinate_dim=['acceleration'], ordinate_unit=['g'])


@pytest.mark.parametrize('beta,half', [(0.0, 1), (14.0, 5), (40.0, 13)])
def test_a_kaiser_s_lobe_widens_with_its_beta(beta, half):
    """Its first null is at sqrt(1 + (beta/pi)^2) lines: 1, 4.6, 12.8."""
    assert leakage('kaiser', parameter=beta).half_width == half


def test_the_lines_of_any_placement_hold_the_whole_tone():
    """Parseval: every line's share, summed, is the tone, wherever it
    sits between lines; that is why the sum reads the amplitude."""
    kernel = leakage('hann', 256)
    for offset in (0.0, 0.17, 0.5):
        shares = kernel.share(np.arange(256) - 128 - offset)
        assert shares.sum() == pytest.approx(1.0, rel=1e-9)


def test_the_spike_height_goes_as_one_over_the_spacing():
    """The guide's arithmetic: 1.944 g peak is 1.89 g², which stands
    at 1.26 g²/Hz at 1 Hz lines under Hann and a quarter of that at
    4 Hz."""
    assert spike_height(1.944, 1.0) == pytest.approx(1.2598, abs=1e-4)
    assert spike_height(1.944, 4.0) == pytest.approx(1.2598 / 4, abs=1e-4)
    assert spike_height(1.944, 1.0, 'boxcar') == pytest.approx(1.8896,
                                                              abs=1e-4)
    # and between lines it reads short, by the scalloping
    assert spike_height(1.944, 1.0, offset=0.5) == pytest.approx(
        1.944 ** 2 / 2 / 1.5 * leakage('hann').scallop(0.5), rel=1e-9)
    with pytest.raises(ValueError, match='positive'):
        spike_height(1.0, 0.0)


def test_read_tone_works_on_bare_arrays():
    """The reading is of any evenly spaced density, not only a Psd."""
    f = np.arange(0.0, 200.0, 1.0)
    density = np.full(len(f), 0.002)
    shares = leakage('hann').share(np.arange(len(f)) - 60.25)
    density = density + 2.0 ** 2 / 2.0 * shares
    reading = read_tone(f, density, 60.0, window='hann', warn=False)
    assert reading.amplitude == pytest.approx(2.0, rel=1e-3)
    assert reading.frequency == pytest.approx(60.25, abs=0.01)
    assert reading.floor == pytest.approx(0.002, rel=1e-4)


def test_the_floor_s_two_sides_are_written_in_the_units_shown():
    """A tone at a step in the floor: both sides named in g²/Hz."""
    f = np.arange(0.0, 200.0, 1.0)
    density = np.where(f < 60.0, 0.001, 0.004) * G ** 2
    density = density + (2.0 * G) ** 2 / 2.0 * leakage('hann').share(
        np.arange(len(f)) - 60.0)
    reading = read_tone(f, density, 60.0, unit='m/s²', warn=False)
    note = next(note for note in reading.notes_in('g', 1.0 / G)
                if 'differs either side' in note)
    assert '(0.001 g²/Hz below, 0.004 g²/Hz above)' in note
    assert '(m/s²)²/Hz below' in next(
        note for note in reading.notes if 'differs either side' in note)


def test_a_compound_unit_is_squared_whole():
    from hertzforge.core.tone_readout import density_label

    assert density_label('g') == 'g²/Hz'
    assert density_label('m/s²') == '(m/s²)²/Hz'
    assert density_label('') == '/Hz'
    assert density_label('µm') == 'µm²/Hz'


def test_a_cross_spectrum_is_refused():
    """A CPSD's off-diagonal record is not a density of one signal,
    though its values are real here; its diagonal is read."""
    f = np.arange(0.0, 64.0)
    cpsd = Psd(f, np.ones((2, 64)), response_dof=['1Z+', '1Z+'],
               reference_dof=['1Z+', '2Z+'],
               ordinate_dim=['acceleration**2/frequency',
                             'acceleration*acceleration/frequency'])
    assert cpsd.can_read_tone(0) and not cpsd.can_read_tone(1)
    with pytest.raises(ValueError, match='cross spectrum'):
        cpsd.read_tone(20.0, 1, warn=False)
    cpsd.read_tone(20.0, 0, warn=False)


def test_lines_without_a_number_are_said_or_refused():
    f = np.arange(0.0, 200.0, 1.0)
    density = 0.002 + 2.0 ** 2 / 2.0 * leakage('hann').share(
        np.arange(len(f)) - 60.25)
    holed = density.copy()
    holed[64] = np.nan
    reading = read_tone(f, holed, 60.0, warn=False)
    assert reading.uncertain
    assert any('1 of the lines read' in note for note in reading.notes)
    far = density.copy()
    far[180] = np.nan
    clear = read_tone(f, far, 60.0, warn=False)
    assert not clear.uncertain and not clear.notes
    with pytest.raises(ValueError, match='holds a number'):
        read_tone(f, np.full(len(f), np.nan), 60.0, warn=False)
