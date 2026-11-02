"""The partial coherence reading in the window: on a multiple coherence
computed from a time history with two or more drives, the multiple
coherence over each drive's partial coherence, the coherence between
the drives and the principal drives' powers, with the summary in the
pane.

A reading of a coherence that exists, so a toggle on the bar with no
act, offered on such a coherence and nowhere else.
"""

from __future__ import annotations

import numpy as np

import hertzforge
from hertzforge.core.averaging import Averaging

RATE = 1024.0


def _record(drives=2, coupled=False):
    rng = np.random.default_rng(31)
    n = 1 << 14
    x = rng.standard_normal((drives, n))
    if coupled and drives > 1:
        x[1] = x[0] + 0.1 * rng.standard_normal(n)
    y = np.vstack([x[0] + 0.3 * x[-1] + 0.2 * rng.standard_normal(n),
                   0.5 * x[0] - x[-1] + 0.2 * rng.standard_normal(n)])
    history = hertzforge.TimeHistory(
        np.arange(n) / RATE, np.vstack([x, y]),
        response_dof=[f'{k + 1}Z+' for k in range(drives)]
        + ['101Z+', '102Z+'],
        ordinate_dim=['force'] * drives + ['acceleration'] * 2)
    history.averaging = Averaging(frame_length=512, overlap=0.5,
                                  window='hann', frames=60)
    return history
