"""The application's own copy buttons: the plot bar, the 3-D bar, and
every table (Brandon, 2026-09-24: everything should copy the way a
chat's code blocks do). The report's are in `test_report_copy.py`.
"""

from __future__ import annotations

import numpy as np

import hertzforge

T = np.arange(1024) / 1024.0


def _time_history():
    return hertzforge.TimeHistory(
        T, np.random.default_rng(4).standard_normal((2, len(T))),
        response_dof=['101Z+', '104Z+'], ordinate_dim='acceleration')
