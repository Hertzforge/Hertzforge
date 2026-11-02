"""The field limit in the window: a toggle on a plain PSD's or SRS's bar,
the set drawn with its limit and envelope, the settings and the reading
in the pane beside it, and Make Specification writing what it shows.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.data import Psd

G = 9.80665

DENSITY = 'acceleration**2/frequency'


def _flights(count=6, seed=12, lines=43):
    """`count` flights on sixth-octave lines from 20 to 2000 Hz: a
    profile with 2 dB of log-normal scatter, in m/s^2 squared per Hz."""
    rng = np.random.default_rng(seed)
    f = np.geomspace(20.0, 2000.0, lines)
    base = 0.04 * G * G * np.minimum(1.0, f / 150.0) * np.minimum(
        1.0, (800.0 / f) ** 2)
    levels = base * 10.0 ** (rng.normal(0.0, 2.0, (count, f.size)) / 10.0)
    return Psd(f, levels, '101Z+', ordinate_dim=DENSITY, ordinate_unit='g')
