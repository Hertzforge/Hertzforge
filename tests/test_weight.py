"""The weight on the status line: a geometry whose element groups carry
a material, or the element groups picked in the tree, weighed as the
solve weighs them, in lbf for an imperial display system and kg for SI.

Two quads of 1 m square, aluminum at 2700 kg/m^3: the wing 10 mm thick
(27 kg), the tail 20 mm (54 kg).
"""

from __future__ import annotations

import pytest
from test_fem import ALUMINUM

from hertzforge.core.fem import GroupProperties
from hertzforge.core.geometry import Geometry

#: one kilogram's weight in pounds-force, 9.80665 / 4.4482216152605
LBF_PER_KG = 2.2046226218


@pytest.fixture
def panel():
    return Geometry(
        node_id=[1, 2, 3, 4, 5, 6],
        node_xyz=[[0, 0, 0], [1, 0, 0], [2, 0, 0],
                  [0, 1, 0], [1, 1, 0], [2, 1, 0]],
        elem_id=[10, 11],
        elem_conn=[[1, 2, 5, 4], [2, 3, 6, 5]],
        elem_type=[44, 44],
        elem_group=[7, 9],
        group_id=[7, 9],
        group_name=['wing', 'tail'],
        length_unit='m',
        group_properties={7: GroupProperties(ALUMINUM, 0.01),
                          9: GroupProperties(ALUMINUM, 0.02)})
