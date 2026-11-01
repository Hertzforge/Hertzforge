"""Drawing decisions from Visual Dynamics' `viz.marks`: what a figure needs
to know, with none of the code that draws it."""

from __future__ import annotations

from collections.abc import Sequence
from typing import Any

import numpy as np

from ...core.averaging import Averaging, window_shape
from .waterfall import STAGE

#: the rail's proportions, in fractions of the stage height — the same
#: shape as the 2-D rail's view fractions, restated for a stage whose
#: ceiling is fixed at STAGE's z rather than wherever the view is
RAIL_GAP = 0.06


RAIL_SLOT = 0.10


RAIL_BAND = 0.34


GLYPH_SHARE = 0.78


#: how far behind the last station the back wall stands
WALL_SET_BACK = 1.04


#: where a frame band's fade tops out, at the window's full weight
FADE_OPACITY = 0.45


#: window glyphs are smooth; a handful of stops draws them faithfully
GLYPH_POINTS = 64


#: the tick under each end of every window, as a fraction of the
#: glyph height — the 2-D rail's own caps, telling a tapering
#: window's stop from its neighbor's start
CAP_SHARE = 0.35


def _mapper(extents):
    """Seconds -> stage x, from the extents the stage was drawn with.

    Time is seconds in every display system, so the averaging's and
    the shocks' own seconds land directly on the drawn axis.
    """
    x0, x1, _z0, _z1 = extents
    sx = STAGE[0]
    span = (x1 - x0) or 1.0
    return lambda t: (np.asarray(t, dtype=float) - x0) / span * sx


def averaging_stage_geometry(averaging: Averaging, sample_rate: float,
                             extents: Sequence[float],
                             origin: float = 0.0) -> dict[str, Any]:
    """The averaging as plain stage geometry, with no VTK and no Qt.

    These are the numbers that `add_averaging_marks` puts into a plotter
    and that the report's canvas figure draws for itself: the analyzed
    span, and for each frame the rail's baseline, the stage x of the
    window's sampled points, their weights, the glyph height at each,
    and the x of the two end caps. They are computed in one place so the
    two views cannot disagree about where a frame is.

    `extents` are the stage's real ranges (x0, x1, z0, z1), and
    `origin` is the record's start time in seconds, since frames are
    placed on the record's own clock. Everything returned is in stage
    coordinates: x across, ``wall`` for the depth of the back wall the
    rail is drawn on, z up. Returns ``span``, ``wall``, ``cap`` (the end
    caps' half height), ``fade`` (`FADE_OPACITY`) and ``frames``.
    """
    to_x = _mapper(extents)
    _sx, sy, sz = STAGE
    wall = sy * WALL_SET_BACK
    # on the record's clock, like everything the stage draws
    # (`Averaging.stop` on the origin)
    first = origin + averaging.start_sample(sample_rate) / sample_rate
    last = averaging.stop(sample_rate, origin)
    levels, count = averaging.levels(sample_rate)
    slot = min(RAIL_SLOT, RAIL_BAND / count) * sz
    glyph = slot * GLYPH_SHARE
    foot = sz * (1.0 + RAIL_GAP)
    n = averaging.frame_length
    shape = window_shape(averaging.window, n,
                         averaging.window_parameter)
    # the glyph is smooth: a fixed sampling draws it without carrying
    # a vertex per sample of a long frame onto the stage
    at = np.linspace(0, n - 1, min(GLYPH_POINTS, n)).astype(int)
    weights = shape[at]
    frames = []
    for (low, high), level in zip(
            averaging.frame_bounds(sample_rate, origin), levels):
        baseline = foot + level * slot
        frames.append({
            'baseline': float(baseline),
            'xs': [float(v) for v in to_x(low + at / sample_rate)],
            'weights': [float(w) for w in weights],
            'glyph': [float(baseline + w * glyph) for w in weights],
            'caps': [float(to_x(low)), float(to_x(high))]})
    return {'span': [float(to_x(first)), float(to_x(last))],
            'wall': float(wall), 'cap': float(glyph * CAP_SHARE),
            'fade': FADE_OPACITY, 'frames': frames}


def shock_stage_geometry(shocks: Sequence[Any],
                         extents: Sequence[float],
                         origin: float = 0.0) -> dict[str, Any]:
    """Each shock's analysis window as plain stage geometry: a dict whose
    ``windows`` each hold the slab's ``span`` in stage x and its
    ``label``, the event's number counted from 1. The 3-D counterpart of
    the regions in `plot/shocks.py`, used by both the app's stage and
    the report's canvas, as the averaging's geometry is. The shocks'
    times are measured from `origin`, the record's start."""
    to_x = _mapper(extents)
    return {'windows': [
        {'span': [float(to_x(origin + shock.start)),
                  float(to_x(origin + shock.stop))],
         'label': str(index + 1)}
        for index, shock in enumerate(shocks)]}
