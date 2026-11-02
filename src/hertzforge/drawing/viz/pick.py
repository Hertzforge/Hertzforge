"""Drawing decisions from Visual Dynamics' `viz.pick`: what a figure needs
to know, with none of the code that draws it."""

from __future__ import annotations

import numpy as np


def segment_distances(point, starts, ends):
    """Distance from a 2-D point to each of many 2-D segments, given as
    arrays of `starts` and `ends`."""
    segment = ends - starts
    length2 = np.einsum('ij,ij->i', segment, segment)
    length2[length2 == 0] = 1e-12
    t = np.clip(np.einsum('ij,ij->i', point - starts, segment) / length2, 0, 1)
    closest = starts + t[:, np.newaxis] * segment
    return np.linalg.norm(point - closest, axis=1)
