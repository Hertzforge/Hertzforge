"""The rms check in the window: a toggle on a plain autospectrum's bar,
the judgment and the gap in the plot's corner, the whole check in the
pane beside it.

A reading of a spectrum that exists, so a toggle on the bar with no act
(as the tones are), offered for a plain autospectrum with a time history
in the project to check it against, and nowhere else.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.averaging import Averaging
from hertzforge.core.data import TimeHistory

G = 9.80665

FS = 2048.0


def _record(seconds=8.0, seed=1502, grms=2.0):
    """2 grms of noise from 20 to 800 Hz, Hann frames at half overlap
    set on it, stored in m/s^2."""
    rng = np.random.default_rng(seed)
    n = int(seconds * FS)
    spectrum = np.fft.rfft(rng.standard_normal(n))
    f = np.fft.rfftfreq(n, 1.0 / FS)
    spectrum[(f < 20.0) | (f > 800.0)] = 0.0
    x = np.fft.irfft(spectrum, n)
    x = x * grms / np.sqrt(np.mean(x ** 2))
    history = TimeHistory(np.arange(n) / FS, x[None] * G,
                          response_dof=['1Z+'],
                          ordinate_dim=['acceleration'],
                          ordinate_unit=['g'])
    history.averaging = Averaging(frame_length=1024, overlap=0.5,
                                  window='hann').filled(n, FS)
    return history
