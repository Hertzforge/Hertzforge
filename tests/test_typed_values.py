"""A number counts when it is finished, not while it is being typed.

Typing `50` into the overlap field applied 5 first: Qt reports every
keystroke unless told not to, and here every report is a real edit — it
landed on the object, moved the shading, and restated the clamped value,
which took the selection away mid-number. 50% overlap could not be
entered at all, and frame length behaved the same way (Brandon,
2026-08-27).

The panel tests below pin the two fields that were reported. The sweep
at the end is the one that matters afterwards: it walks every spin box
the application builds, so a tenth one added next year cannot quietly
arrive with the old behavior.
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.core.data import TimeHistory


@pytest.fixture
def history():
    t = np.arange(4096) / 512.0
    return TimeHistory(t, np.atleast_2d(np.sin(2 * np.pi * 20.0 * t)),
                       response_dof=['101Z+'])
