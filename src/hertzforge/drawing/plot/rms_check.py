"""Drawing decisions from Visual Dynamics' `plot.rms_check`: what a figure needs
to know, with none of the code that draws it."""

from __future__ import annotations

from typing import Any

import numpy as np


def heading(check: Any) -> str:
    """The judgment in a word or two, the label's first line: 'agree'
    within the threshold, 'the window's gap' outside it when the window
    is the whole of the gap, 'flagged' outside it otherwise, and 'not
    judged' when there is no gap to judge (`RmsCheck.unjudged`)."""
    if not np.isfinite(check.gap_db):
        return 'rms check: not judged'
    if check.passed:
        return 'rms check: agree'
    return 'rms check: ' + ("the window's gap" if check.explained
                            else 'flagged')
