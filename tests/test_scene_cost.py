"""A frame of animation must not cost the size of the model.

Measured on a 202 500-node, 201 601-quad plate: a frame is 2.2 ms, which
is seven percent of the 33 ms a 30 fps timer has to spend. That headroom
is the *size ceiling* — it is what makes a real FE model animate at all —
and it comes from three structural properties rather than from anything
being fast:

- a frame is **one write** into a shared point buffer and one into a
  shared scalar buffer, seen by every mesh at once;
- meshes are grouped by **color**, so a scene has a dozen of them
  whatever the model size, and a dozen draw calls;
- nothing derived is rebuilt per frame — no actor added, no mesh
  replaced, no array reallocated.

None of that is asserted here in milliseconds. A wall-clock ceiling
measures the machine it runs on, and this suite has been taught that
lesson twice; what is pinned is the *shape* of the work, which is what
actually decays. Tubes and sphere glyphs were measured at 120 ms and
1 258 ms a frame on this model and rejected on these grounds, and the
way that decision gets quietly undone is a per-entity actor creeping
into the draw path.
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.core.geometry import Geometry


def plate(side: int) -> Geometry:
    """A meshed square of quads, `side` nodes each way."""
    xs, ys = np.meshgrid(np.arange(side), np.arange(side))
    nodes = np.arange(1, side * side + 1)
    xyz = np.column_stack([xs.ravel() * 0.01, ys.ravel() * 0.01,
                           np.zeros(side * side)])
    index = nodes.reshape(side, side)
    quads = [np.column_stack([index[r, :-1], index[r, 1:],
                              index[r + 1, 1:], index[r + 1, :-1]])
             for r in range(side - 1)]
    conn = np.vstack(quads)
    return Geometry(node_id=nodes, node_xyz=xyz, elem_conn=list(conn),
                    elem_type=[44] * len(conn), length_unit='m')


@pytest.fixture(scope='module')
def small():
    return plate(20)          # 400 nodes


@pytest.fixture(scope='module')
def larger():
    return plate(60)          # 3 600 nodes, nine times the first
