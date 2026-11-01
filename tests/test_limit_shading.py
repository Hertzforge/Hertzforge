"""A specification says where the response may not go, so the plot shades
it rather than leaving four lines to be read against each other.

Yellow between the warning limit and the abort limit, red beyond abort,
above the upper pair and below the lower pair alike. A specification may
carry any of the four limits, so each side is worked out on its own: with
no abort limit the yellow runs to the edge of the plot, and with no
warning limit the red starts at abort.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.data import Specification


def spec(**limits):
    frequencies = np.logspace(1, 3, 48)
    level = np.full((1, 48), 0.01)
    return Specification(abscissa=frequencies, ordinate=level,
                         response_dof=['101X+'],
                         ordinate_dim=['acceleration**2/frequency'],
                         **{name: level * factor
                            for name, factor in limits.items()})


# the widgets are kept alive for the run: a GraphicsLayoutWidget that
# goes out of scope takes its items' C++ objects with it, and touching a
# fill afterwards raises "already deleted" rather than failing an assert
_ALIVE = []


def color_of(fill):
    return fill.brush().color().name()


def bounds_of(fill):
    """(lower, upper) curves a fill spans, as arrays."""
    return tuple(np.asarray(curve.getData()[1]) for curve in fill.curves)


def shown(data, bound, at=None):
    """A limit as the plot's curves hold it, at the x they are drawn on.

    Two transforms sit between a limit and the numbers in a curve: the
    conversion to display units, and the log the ordinate axis is in —
    a PSD plots on a log axis, and pyqtgraph keeps its items' data in
    that space, so 31 (in/s^2)^2/Hz reads back as 1.491.

    And a third, when the specification is written at breakpoints: its
    zone is filled between power laws rather than straight lines, so the
    fill carries more points than the specification does and the limit
    has to be evaluated where they fall.
    """
    from hertzforge.core.compliance import log_interpolate
    from hertzforge.units import DEFAULT_SYSTEM

    values = np.asarray(data.display_limit(bound, DEFAULT_SYSTEM))[0].real
    if at is not None:
        values = log_interpolate(at, data.abscissa, values)
    return np.log10(values)


def edge_x(fill):
    return np.asarray(fill.curves[0].getData()[0])
