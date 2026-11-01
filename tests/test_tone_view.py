"""The tone readout in the window: a cursor on a plain autospectrum,
the tone under it read and drawn, the reading in the pane beside it.

A reading of a spectrum that exists, so a toggle on the bar with no act
(as the octave preview is before its button), offered for a plain
autospectrum on even lines and nowhere else.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.averaging import Averaging
from hertzforge.core.data import TimeHistory

G = 9.80665

FS = 2048.0


def _record(seconds=16.0):
    """A 1 grms floor from 5 to 500 Hz with two tones: 1.944 g at
    19.44 Hz and 1.5 g at 58.32 Hz, stored in m/s^2."""
    rng = np.random.default_rng(3304)
    n = int(seconds * FS)
    spectrum = np.fft.rfft(rng.standard_normal(n))
    f = np.fft.rfftfreq(n, 1.0 / FS)
    spectrum[(f < 5.0) | (f > 500.0)] = 0.0
    x = np.fft.irfft(spectrum, n)
    x = x / np.sqrt(np.mean(x ** 2))
    t = np.arange(n) / FS
    for frequency, amplitude in ((19.44, 1.944), (58.32, 1.5)):
        x = x + amplitude * np.sin(2.0 * np.pi * frequency * t)
    history = TimeHistory(t, x[None] * G, response_dof=['1Z+'],
                          ordinate_dim=['acceleration'],
                          ordinate_unit=['g'])
    history.averaging = Averaging(frame_length=2048, overlap=0.5,
                                  window='hann').filled(n, FS)
    return history
