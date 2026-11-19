"""One playing at a time, in both views.

Where the averaging view meets a multi-capture record it shows a
single playing — channels overlaid on the flat plot, receding on the
stage — and the pager steps playings (Brandon, 2026-08-28, after
driving two concatenated layouts and loving neither). The marks sit
once, on the record's own clock; the panel's '20 × 3 = 60' row is
what tells the pooling.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.data import TimeHistory

RATE = 1024.0

SAMPLES = 512


def _multi(captures=4, channels=('101Z+', '104Z+')):
    """A history the way imports build them: capture-major, one record
    per channel per capture."""
    t = np.arange(SAMPLES) / RATE
    rng = np.random.default_rng(8)
    rows, dofs = [], []
    for _capture in range(captures):
        for dof in channels:
            rows.append(rng.standard_normal(SAMPLES))
            dofs.append(dof)
    return TimeHistory(t, np.asarray(rows), response_dof=dofs,
                       ordinate_dim='acceleration')


def test_capture_indices_count_each_channels_playings():
    history = _multi(captures=3)
    assert history.capture_indices() == [0, 0, 1, 1, 2, 2], \
        'per channel, in pooling order — never a global record count'
