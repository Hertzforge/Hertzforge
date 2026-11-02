"""How a scene draws a meshed part by default: a solid's skin
see-through from both sides, its nodes drawn only where they mean
something, and the skin's faces wound outward, which a renderer that
lights or culls by the winding relies on.
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.core.geometry import Geometry, solid_faces
from hertzforge.drawing.viz.geometry import element_cells

INTERIOR = 14          # the one node inside two bricks each way

BEAM_END = 28          # a beam from a brick corner (27) out to here

FLOATING = 29          # a node no element names

QUAD = [30, 31, 32, 33]


def part() -> Geometry:
    """Eight bricks (27 nodes, one inside), a beam leaving a corner, a
    floating node and a plate off to the side."""
    xyz = [(x, y, z) for z in range(3) for y in range(3) for x in range(3)]
    xyz += [(3, 0, 0), (5, 5, 5), (0, 5, 0), (1, 5, 0), (1, 6, 0), (0, 6, 0)]

    def node(x, y, z):
        return 1 + x + 3 * y + 9 * z

    bricks = [[node(x, y, z), node(x + 1, y, z), node(x + 1, y + 1, z),
               node(x, y + 1, z), node(x, y, z + 1), node(x + 1, y, z + 1),
               node(x + 1, y + 1, z + 1), node(x, y + 1, z + 1)]
              for z in range(2) for y in range(2) for x in range(2)]
    conn = bricks + [[27, BEAM_END], QUAD]
    return Geometry(node_id=np.arange(1, 34), node_xyz=np.array(xyz, dtype=float),
                    elem_conn=conn, elem_type=[115] * 8 + [21, 44],
                    length_unit='m')


def actors_of(plotter):
    """(dataset, actor) for every actor drawn, the dataset being what
    the mapper holds."""
    return [(actor.mapper.dataset, actor)
            for actor in plotter.renderer.actors.values()
            if getattr(actor, 'mapper', None) is not None]


def skin_actor(plotter):
    for dataset, actor in actors_of(plotter):
        if dataset.n_faces == 24:
            return actor
    raise AssertionError('no skin of 24 faces drawn')


def verts_drawn(plotter) -> set[int]:
    """The node rows drawn as points."""
    rows = set()
    for dataset, _actor in actors_of(plotter):
        if dataset.n_verts:
            rows.update(int(v) for v in np.asarray(dataset.verts).reshape(-1, 2)[:, 1])
    return rows


def test_element_cells_still_hands_the_html_export_one_set_of_faces():
    faces, _lines, _points = element_cells(part())
    assert sum(len(polys) for polys in faces.values()) == 25
    faces, _lines, _points, skins = element_cells(part(), separate_skins=True)
    assert sum(len(polys) for polys in faces.values()) == 1
    assert sum(len(polys) for polys in skins.values()) == 24


@pytest.mark.parametrize('xyz', [
    # a tetrahedron, a pyramid, a wedge and a brick, each in the node
    # order the readers and the meshers use
    [(0, 0, 0), (1, 0, 0), (0, 1, 0), (0, 0, 1)],
    [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0), (0.5, 0.5, 1)],
    [(0, 0, 0), (1, 0, 0), (0, 1, 0), (0, 0, 1), (1, 0, 1), (0, 1, 1)],
    [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0),
     (0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)],
])
def test_every_solids_faces_wind_outward(xyz):
    """A face's winding says which side is outside, which is what a
    renderer lights by, so a face of a right-way-out cell must point
    away from its middle."""
    xyz = np.array(xyz, dtype=float)
    middle = xyz.mean(axis=0)
    for face in solid_faces(len(xyz)):
        corners = xyz[list(face)]
        normal = np.cross(corners[1] - corners[0], corners[2] - corners[0])
        assert normal @ (corners.mean(axis=0) - middle) > 0, face
