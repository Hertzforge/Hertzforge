"""The transmissibility reading in the window: on an FRF computed from a
time history, the ASD ratio drawn dashed beside H1 with the band H1's
error gives, a cursor, and the reading at its line in the pane.

A reading of an FRF that exists, so a toggle on the bar with no act (as
the tones are), offered for an FRF drawn as its magnitude whose history
is in the project, against one reference, and nowhere else.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.averaging import Averaging
from hertzforge.core.data import TimeHistory

G = 9.80665

FS = 5120.0


def _record(seconds=8.0):
    """Random from 20 to 2000 Hz on 1Z+, through a 300 Hz resonance at
    Q 20 to 101Z+ with 0.6 g rms of noise the input did not cause,
    stored in m/s², 1Z+ the reference."""
    n = int(seconds * FS)
    f = np.fft.rfftfreq(n, 1.0 / FS)
    spectrum = np.fft.rfft(np.random.default_rng(1505).standard_normal(n))
    spectrum[(f < 20.0) | (f > 2000.0)] = 0.0
    x = np.fft.irfft(spectrum, n)
    x *= np.sqrt(0.04 * 1980.0) / np.std(x)
    r, z = f / 300.0, 1.0 / 40.0
    y = np.fft.irfft(np.fft.rfft(x) * (1 + 2j * z * r)
                     / (1 - r ** 2 + 2j * z * r), n)
    y += np.random.default_rng(1506).standard_normal(n) * 0.6
    history = TimeHistory(np.arange(n) / FS, np.vstack([x, y]) * G,
                          response_dof=['1Z+', '101Z+'],
                          ordinate_dim=['acceleration'] * 2,
                          ordinate_unit=['g'] * 2)
    history.roles = {('1Z+', 'acceleration'): 'reference'}
    history.averaging = Averaging(frame_length=2048, overlap=0.5,
                                  window='hann').filled(n, FS)
    return history
