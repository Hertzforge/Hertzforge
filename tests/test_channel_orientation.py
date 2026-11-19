"""The channel table's direction columns, derived from a geometry.

Each channel's measured direction as a unit vector in the geometry's
global system, the global axis it is nearest and the angle to it
(Brandon, 2026-09-20) — read from the geometry, never stored, so they
follow an edit to the node or the direction at once and can never
disagree with the geometry. In the window's table, in the report's,
and in a script's.
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.channel_table import DERIVED_COLUMNS, ChannelTable, title_of


def _framed_geometry():
    """Node 101 measured in a frame turned 30° about Z, 102 global."""
    c, s = np.cos(np.radians(30.0)), np.sin(np.radians(30.0))
    turned = [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]]
    identity = [[1.0, 0, 0], [0, 1.0, 0], [0, 0, 1.0], [0, 0, 0]]
    return hertzforge.Geometry(
        node_id=[101, 102], node_xyz=np.zeros((2, 3)), node_disp_cs=[2, 1],
        cs_id=[1, 2], cs_name=['', ''], cs_type=[0, 0],
        cs_matrix=[identity, turned], length_unit='m')


def _table():
    return ChannelTable({
        'channel': [1, 2, 3, 4],
        'node': [101, 101, 102, 999],
        'direction': ['X+', 'Y-', 'Z+', 'X+'],
        'channel_type': ['acceleration'] * 4,
        'unit': ['g'] * 4,
    })


def test_a_channel_reads_its_direction_through_its_nodes_frame():
    table, geometry = _table(), _framed_geometry()
    c, s = np.cos(np.radians(30.0)), np.sin(np.radians(30.0))
    vector, axis, angle = table.orientation(0, geometry)
    assert np.allclose(vector, [c, s, 0.0])
    assert axis == 'X+' and angle == pytest.approx(30.0)
    vector, axis, angle = table.orientation(1, geometry)
    assert np.allclose(vector, [s, -c, 0.0])
    assert axis == 'Y-' and angle == pytest.approx(30.0)
    vector, axis, angle = table.orientation(2, geometry)
    assert np.allclose(vector, [0.0, 0.0, 1.0])
    assert axis == 'Z+' and angle == pytest.approx(0.0)
    assert table.orientation(3, geometry) == (None, None, None), 'no such node'
    assert table.orientation(0, None) == (None, None, None), 'no geometry'
    assert table.derived_cells(0, geometry) == ['+0.866', '+0.500', '+0.000', 'X+', '30.0']
    assert table.derived_cells(3, geometry) == [''] * 5
    assert [title_of(n) for n in DERIVED_COLUMNS] == [
        'Unit X', 'Unit Y', 'Unit Z', 'Nearest Axis', 'Angle to Axis (deg)']


def test_a_script_reads_the_same_table():
    from hertzforge.core.tables import table_of

    table, geometry = _table(), _framed_geometry()
    headers, rows = table_of(table)
    assert headers == [title_of(n) for n in table.SCHEMA]
    headers, rows = table_of(table, geometry)
    assert headers[-5:] == [title_of(n) for n in DERIVED_COLUMNS]
    assert rows[0][-5:] == ['+0.866', '+0.500', '+0.000', 'X+', '30.0']
    assert rows[3][-5:] == [''] * 5
