"""Which geometry is active, said where it can be seen.

The active geometry is what an object *not* linked to a geometry of its
own is drawn on, checked against, and projected onto — so in a project
with two of them it decides what a great deal of the window is showing.
Bold was the only mark it wore, and in a tree where the real objects are
already darker than the gray slots around them, a bolder weight among
them is not a mark anyone can find.

The bullet is painted rather than put in the row: the row's text is the
object's name, and anything prepended to it would be carried into a
rename.
"""

from __future__ import annotations

import numpy as np

import hertzforge
from hertzforge.core.shapes import ShapeSet


def _geometry():
    return hertzforge.Geometry(
        node_id=[1, 2, 3], node_xyz=[[0, 0, 0], [1, 0, 0], [2, 0, 0]],
        length_unit='m')


def _shapes():
    return ShapeSet(np.array([10.0, 20.0]), np.array([0.01, 0.01]),
                    ['1Z+', '2Z+', '3Z+'], np.ones((2, 3)))
