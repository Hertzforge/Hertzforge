"""Each element's edges drawn on its surface (Brandon, 2026-11-11: a meshed element
group showed its nodes and one shaded surface, with no line between the elements; the
earlier toolset drew every element's edges)."""

from __future__ import annotations

import numpy as np

from hertzforge.core.geometry import Geometry
from hertzforge.service.mesh import drawn_mesh, outline


def _bricks():
    """Two unit hexes side by side along X: twelve nodes, all on the skin."""
    xyz = [[x, y, z] for z in (0.0, 1.0) for y in (0.0, 1.0) for x in (0.0, 1.0, 2.0)]
    geometry = Geometry(node_id=list(range(1, 13)), node_xyz=xyz)

    def node(x, y, z):
        return 1 + x + 3 * y + 6 * z

    for x in (0, 1):
        geometry.add_element([node(x, 0, 0), node(x + 1, 0, 0), node(x + 1, 1, 0), node(x, 1, 0),
                              node(x, 0, 1), node(x + 1, 0, 1), node(x + 1, 1, 1), node(x, 1, 1)],
                             elem_type=115)
    return geometry


def test_shared_edges_are_drawn_once_and_no_diagonal_is():
    edges = outline([[0, 1, 2, 3], [1, 4, 5, 2], [0, 1, 6]])
    assert edges.tolist() == [[0, 1], [0, 3], [0, 6], [1, 2], [1, 4], [1, 6], [2, 3], [2, 5],
                              [4, 5]]
    assert outline([]).shape == (0, 2)


def test_two_bricks_draw_their_twenty_edges():
    rows, triangles, lines = drawn_mesh(_bricks())
    assert len(rows) == 12 and len(triangles) == 2 * 10, 'ten outer quads, two triangles each'
    assert len(lines) == 20, 'every edge of the box once, the diagonals left out'
    xyz = _bricks().node_xyz[rows]
    lengths = np.linalg.norm(xyz[lines[:, 0]] - xyz[lines[:, 1]], axis=1)
    assert np.allclose(lengths, 1.0), 'each a cube edge, none across a face'


def test_a_line_element_stays_a_line():
    geometry = Geometry(node_id=[1, 2, 3], node_xyz=[[0, 0, 0], [1, 0, 0], [2, 0, 0]])
    geometry.add_beams([1, 2, 3])
    _rows, triangles, lines = drawn_mesh(geometry)
    assert len(triangles) == 0 and sorted(map(tuple, lines.tolist())) == [(0, 1), (1, 2)]
