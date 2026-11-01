"""Every table in hertzforge behaves the same way.

The conventions are written up under "How tables behave" in PLAN.md. This
checks them against every table model there is, so a new one cannot quietly
grow its own manners — the point is that the user learns a table once.
"""

from __future__ import annotations

from conftest import fixture_path

import hertzforge


def geometry():
    return hertzforge.import_file(fixture_path('plate', 'geometry.npz'))


def meshed_geometry():
    """One with elements in it."""
    return hertzforge.import_file(fixture_path('plate',
                                                   'geometry.npz'))


class _Grid:
    """Five rows of four numbers; C is read-only and D takes no negative."""

    def __init__(self):
        self.values = [[None] * 4 for _ in range(5)]


def _grid_setter(k):
    def setter(grid, row, text):
        value = float(text) if str(text).strip() else None
        if k == 3 and value is not None and value < 0:
            raise ValueError('D takes no negative value')
        grid.values[row][k] = value
    return setter


#: the Element Groups table's rows whose meaning depends on the group's
#: elements, and the groups below each one applies to
APPLIES = {
    'Thickness [m]': {'plate'},
    'Section': {'beam'}, 'Shape': {'beam'}, 'Dimensions [m]': {'beam'},
    'A [m²]': {'beam'}, 'Iy [m⁴]': {'beam'}, 'Iz [m⁴]': {'beam'},
    'J [m⁴]': {'beam'}, 'Orientation': {'beam'},
    'Mass [kg]': {'point'}, 'Ground': {'point'},
    'Stiffness [N/m, N·m/rad]': {'point', 'beam'},
}

#: the groups' columns in the table, in the order `_four_groups` makes them
GROUPS = ('beam', 'plate', 'solid', 'point')
