"""The confidence band in the window: the chi-square band shaded on an
averaged autospectrum, the confidence and the count in the pane beside
it.

A statement about a spectrum that exists, so a toggle on the bar with
no act (as the tone readout is), offered for an autospectrum and
nowhere else. The count is the one the spectrum was stamped with when
it was computed, or one typed in the pane for a spectrum that carries
none.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.averaging import (
    Averaging,
)
from hertzforge.core.data import TimeHistory

FS = 1024.0


def _record(seconds=8.0, channels=2):
    """White noise in g on two channels, averaged over Hann frames at
    half overlap."""
    rng = np.random.default_rng(1506)
    n = int(seconds * FS)
    history = TimeHistory(np.arange(n) / FS,
                          rng.standard_normal((channels, n)),
                          response_dof=[f'{i + 1}Z+' for i in
                                        range(channels)],
                          ordinate_dim=['acceleration'] * channels,
                          ordinate_unit=['g'] * channels)
    history.averaging = Averaging(frame_length=512, overlap=0.5,
                                  window='hann').filled(n, FS)
    return history
