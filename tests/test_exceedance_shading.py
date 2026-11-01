"""Marking the lines that went outside an abort limit.

A percentage says how much of a run was out of tolerance. This says
which lines, and which way: over its own bin, from the limit to where
the line actually got to — red where it went over, blue where it fell
under.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.compliance import log_interpolate
from hertzforge.core.data import Psd, Specification

_ALIVE = []

BREAKS = np.array([20.0, 80.0, 350.0, 2000.0])

LEVELS = np.array([0.01, 0.16, 0.16, 0.005])


def spec(**limits):
    return Specification(
        abscissa=BREAKS, ordinate=np.atleast_2d(LEVELS),
        response_dof=['101Z+'], ordinate_dim=['acceleration**2/frequency'],
        **{name: np.atleast_2d(LEVELS * factor)
           for name, factor in limits.items()})


def measured(lines, tweak=None):
    values = log_interpolate(lines, BREAKS, LEVELS)
    if tweak is not None:
        values = tweak(values)
    out = Psd(abscissa=lines, ordinate=np.atleast_2d(values),
              response_dof=['101Z+'],
              ordinate_dim=['acceleration**2/frequency'])
    # held unscaled: these tests aim responses far outside the limits
    # on purpose, and comparison scaling would first lay them back on
    out.scale_db = 0
    return out


def color_of(fill):
    return fill.brush().color().name()


def span(fill):
    """(low, high) in x over which the fill has any height at all."""
    lower, upper = (np.asarray(curve.getData()[1]) for curve in fill.curves)
    edges = np.asarray(fill.curves[0].getData()[0])
    open_at = np.flatnonzero(~np.isclose(lower, upper))
    if not len(open_at):
        return None
    return float(edges[open_at.min()]), float(edges[open_at.max() + 1])


def legend_names(plot):
    return [label.text for _sample, label in plot.legend.items]
