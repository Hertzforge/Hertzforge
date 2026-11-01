"""The fatigue reading in the window: a toggle on an acceleration record's
bar, the extreme response and fatigue damage spectra drawn with the
time-compression rule, and the settings and the comparison with a
candidate test in the pane beside them.

Offered for one record of an acceleration PSD or specification, or one
channel of an acceleration time history, and nowhere else; a refusal is
said in the pane with nothing drawn; heavy work goes to the window's
worker with only records and numbers handed to it, and is kept so a
redraw with nothing changed does not count again.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core import fatigue
from hertzforge.core.data import Specification, TimeHistory

G = 9.80665

LEVEL = 0.04 * G ** 2


def _spec(level=LEVEL, low=20.0, high=500.0):
    return Specification([low, high], [[level, level]], response_dof=['1X+'],
                         ordinate_dim=[fatigue.DENSITY])


def _history(seconds=4.0, rate=4096.0, seed=2):
    rng = np.random.default_rng(seed)
    n = int(seconds * rate)
    spectrum = np.fft.rfft(rng.standard_normal(n))
    f = np.fft.rfftfreq(n, 1.0 / rate)
    spectrum[(f < 20.0) | (f > 150.0)] = 0.0
    x = np.fft.irfft(spectrum, n)
    x *= 2.0 * G / np.sqrt(np.mean(x ** 2))
    return TimeHistory(np.arange(n) / rate, x[None], response_dof=['1X+'],
                       ordinate_dim=['acceleration'], ordinate_unit=['g'])
