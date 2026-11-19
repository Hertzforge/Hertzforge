"""Never hand pyqtgraph a curve with holes in it.

pyqtgraph builds a `QPainterPath` two ways. With no gaps it fills a
`QPolygonF` and lets Qt build the path — Qt's own API. With gaps, and
`arrayToQPath` draws the line at 2% of the points being non-finite, it
packs a byte buffer by hand and has Qt deserialize it, against a
private binary layout its own docstring warns "may change in future
versions of Qt".

That second route is where the intermittent Bus error was raised from,
and on a plot of a specification against its response exactly one curve
was taking it: the specification, 404 non-finite points in 1083,
because a controller writes it to zero outside its band and zero on a
log axis is -inf.

These tests pin the invariant rather than the crash. A Bus error at one
run in eight cannot be tested for directly — a green run proves
nothing — but "no curve is drawn with gaps in it" is exactly checkable,
and it is the condition that keeps the process off that code path.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.data import Psd, Specification
from hertzforge.drawing.plot import as_power_law

#: the fraction of non-finite points at which pyqtgraph stops using
#: QPolygonF and starts packing the path's bytes itself
HAND_PACKED = 0.02

BREAKS = np.array([20.0, 80.0, 350.0, 2000.0])

LEVELS = np.array([0.01, 0.16, 0.16, 0.005])

_ALIVE: list = []


def spec(**limits):
    """A specification written to zero outside its band, as a controller
    writes one: the shape that put 404 holes in a 1083-point curve."""
    axis = np.linspace(0.0, 4096.0, 1025)
    values = np.zeros((1, axis.size))
    inside = (axis >= BREAKS[0]) & (axis <= BREAKS[-1])
    values[0, inside] = np.interp(np.log10(axis[inside]),
                                  np.log10(BREAKS), np.log10(LEVELS))
    values[0, inside] = 10.0 ** values[0, inside]
    return Specification(
        abscissa=axis, ordinate=values, response_dof=['101Z+'],
        ordinate_dim=['acceleration**2/frequency'],
        **{name: values * factor for name, factor in limits.items()})


def response(target):
    return Psd(abscissa=target.abscissa,
               ordinate=np.where(target.ordinate > 0, target.ordinate, 1e-9),
               response_dof=['101Z+'],
               ordinate_dim=['acceleration**2/frequency'])


def test_a_specification_is_filled_in_only_over_its_own_band():
    """`as_power_law` used to densify across the whole axis, so every
    point outside the band came back NaN. That single curve was the one
    taking the hand-packed route."""
    axis = np.linspace(0.0, 4096.0, 1025)
    values = np.zeros(axis.size)
    inside = (axis >= 20.0) & (axis <= 2000.0)
    values[inside] = 0.1
    x, y = as_power_law(axis, values)
    assert np.isfinite(y).all(), 'no holes'
    assert x.min() >= 20.0 and x.max() <= 2000.0, 'and only the band'
