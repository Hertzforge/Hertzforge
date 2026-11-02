"""Drawing decisions from Visual Dynamics' `viz.geometry`: what a figure needs
to know, with none of the code that draws it."""

from __future__ import annotations

from collections.abc import Sequence
from typing import TYPE_CHECKING

import numpy as np
from numpy.typing import ArrayLike

from ...core.geometry import ELEMENT_TYPES, face_corners, solid_faces

if TYPE_CHECKING:
    from ...core.geometry import Geometry
    from ...units import UnitSystem


# UFF-style color indices -> RGB
PALETTE = [
    (0.55, 0.55, 0.55),  # 0 gray
    (0.12, 0.47, 0.71),  # 1 blue
    (1.00, 0.50, 0.05),  # 2 orange
    (0.17, 0.63, 0.17),  # 3 green
    (0.84, 0.15, 0.16),  # 4 red
    (0.58, 0.40, 0.74),  # 5 purple
    (0.55, 0.34, 0.29),  # 6 brown
    (0.89, 0.47, 0.76),  # 7 pink
    (0.74, 0.74, 0.13),  # 8 olive
    (0.09, 0.75, 0.81),  # 9 cyan
]


def color_rgb(index: int) -> tuple[float, float, float]:
    """The RGB triple (0 to 1) of a color index, wrapping past 9."""
    return PALETTE[int(index) % len(PALETTE)]


def axis_unit_text(geometry: Geometry, unit_system: UnitSystem) -> str:
    """The unit text the labeled axes show, such as ``'[mm]'``. A
    geometry whose units are undefined shows ``'[units undefined]'``
    instead of a unit."""
    if not geometry.units_defined:
        return '[units undefined]'
    return f"[{unit_system.label_text('length')}]"


def display_points(geometry: Geometry,
                   unit_system: UnitSystem) -> tuple[np.ndarray, str]:
    """The coordinates to draw for a geometry, and the axis unit text.

    Coordinates are converted from SI to `unit_system`. A geometry whose
    units are undefined is drawn with the file's raw coordinates, because
    converting them would scale values whose unit is unknown, and its
    axis text says the units are undefined.
    """
    if not geometry.units_defined:
        return geometry.node_xyz, axis_unit_text(geometry, unit_system)
    return (unit_system.from_si(geometry.node_xyz, 'length'),
            axis_unit_text(geometry, unit_system))


def element_cells(geometry: Geometry,
                  indices: Sequence[int] | None = None,
                  separate_skins: bool = False) -> tuple:
    """How the elements are drawn: (faces, lines, points), each keyed by
    element color, with every entry given as node rows (positions in
    `node_id`), not node ids; with `separate_skins`, (faces, lines,
    points, skins), the solids' skins kept apart from the face elements
    so a scene can draw them see-through.

    A face element is drawn as its corner nodes (`face_corners`), since
    a quad8's midside nodes are not corners of the outline. A solid is
    drawn as the outer surface of its color: each cell's faces
    (`solid_faces`), less every face that two cells share. A line
    element is its first two nodes, and a point element is its nodes.
    The 3-D view, the report's scenes and the HTML export all use this
    one function.

    Parameters
    ----------
    geometry : Geometry
        The geometry.
    indices : sequence of int, optional
        Which elements, by row; every element when omitted.
    separate_skins : bool, default False
        Return the solids' skins as a fourth dict instead of among the
        faces.

    Returns
    -------
    tuple of dict
        Faces and lines as lists of row lists, points as a row list,
        each by element color; and the skins, when asked for.
    """
    # One rule for every scene because the HTML export once drew a brick
    # as one eight-sided polygon in node order, which folds it into a bow
    # tie (Brandon, 2026-10-11, the website's brick examples).
    id_to_row = {int(i): r for r, i in enumerate(geometry.node_id)}

    def rows(node_ids):
        return [id_to_row[int(i)] for i in node_ids]

    faces: dict[int, list[list[int]]] = {}
    lines: dict[int, list[list[int]]] = {}
    points: dict[int, list[int]] = {}
    skins: dict[int, dict[tuple[int, ...], list[int]]] = {}
    for i in (range(len(geometry.elem_conn)) if indices is None else indices):
        code, color = int(geometry.elem_type[i]), int(geometry.elem_color[i])
        conn = geometry.elem_conn[i]
        _name, _nnodes, render = ELEMENT_TYPES[code]
        if render == 'face':
            faces.setdefault(color, []).append(rows(conn[:face_corners(code)]))
        elif render == 'volume':
            # a solid draws as its skin: the faces of its cells that no
            # other cell of the color shares (2026-09-30; a polyline
            # through every corner drew the mesh's insides as a tangle)
            skin = skins.setdefault(color, {})
            for face in solid_faces(len(conn)):
                corners = rows([conn[k] for k in face])
                key = tuple(sorted(corners))
                if key in skin:
                    del skin[key]
                else:
                    skin[key] = corners
        elif render == 'line':
            lines.setdefault(color, []).append(rows(conn[:2]))
        elif render == 'point':
            points.setdefault(color, []).extend(rows(conn))
    if separate_skins:
        return faces, lines, points, {color: list(skin.values())
                                      for color, skin in skins.items()}
    for color, skin in skins.items():
        faces.setdefault(color, []).extend(skin.values())
    return faces, lines, points


AXIS_COLORS = ('#e5534b', '#3fb950', '#4c92d9')


# DOF arrow direction per direction string; a rotational DOF points
# along its axis and keeps its R label to say what it is
DOF_DIRECTIONS = {
    'X+': (1, 0, 0), 'X-': (-1, 0, 0), 'Y+': (0, 1, 0), 'Y-': (0, -1, 0),
    'Z+': (0, 0, 1), 'Z-': (0, 0, -1),
    'RX+': (1, 0, 0), 'RX-': (-1, 0, 0), 'RY+': (0, 1, 0),
    'RY-': (0, -1, 0), 'RZ+': (0, 0, 1), 'RZ-': (0, 0, -1),
}


def dof_axis(direction: str) -> int | None:
    """0, 1 or 2 for the axis of a DOF direction, or None if it has none.

    A rotation counts as its axis: 'RZ-' is 2, and draws blue like 'Z+'.
    """
    letter = str(direction).upper().lstrip('R')[:1]
    return {'X': 0, 'Y': 1, 'Z': 2}.get(letter)


def dof_arrow_vectors(geometry: Geometry, dofs: Sequence[str]
                      ) -> list[tuple[int, np.ndarray, int] | None]:
    """For each DOF, (node row, global direction, axis) for its arrow, or
    None for a DOF at a node the geometry lacks or with no axis.

    The direction is turned by the node's displacement coordinate
    system the same way the animation turns the motion
    (`deform.dof_directions`), so '101X+' on a node measured in a turned
    system points along that system's X. A rotational DOF points along
    its axis, turned the same way. The axis, which picks the color, is
    the DOF's own letter, not the nearest global axis.
    """
    # The arrows were drawn along global X, Y and Z whatever the node's
    # displacement system said, so a turned accelerometer was marked
    # pointing the wrong way while the animation moved it the right one
    # (Brandon, 2026-10-16).
    from ...core.data import parse_dof
    from ...deform import _cs_rotations

    rows = {int(node): i for i, node in enumerate(geometry.node_id)}
    rotations = _cs_rotations(geometry)
    out: list[tuple[int, np.ndarray, int] | None] = []
    for dof in dofs:
        node, direction = parse_dof(dof)
        vector = DOF_DIRECTIONS.get(str(direction).upper())
        axis = dof_axis(direction)
        if node is None or vector is None or axis is None \
                or int(node) not in rows:
            out.append(None)
            continue
        row = rows[int(node)]
        vector = np.asarray(vector, dtype=float)
        rotation = rotations.get(int(geometry.node_disp_cs[row]))
        if rotation is not None:
            vector = vector @ rotation
        out.append((row, vector, axis))
    return out


def dof_arrow_length(points: ArrayLike, extent: float) -> float:
    """One length for every arrow on a plot: 12 % of `extent`, reduced to
    0.9 times the closest spacing of the arrowed `points` so that arrows
    on closely spaced nodes do not overlap."""
    length = 0.12 * extent
    unique = np.unique(np.asarray(points, dtype=float), axis=0)
    if len(unique) > 1:
        from scipy.spatial import cKDTree

        # each node's nearest neighbor, by tree: the all-pairs table this
        # replaced was 600 MB at five thousand nodes (2026-09-27)
        distances, _index = cKDTree(unique).query(unique, k=2)
        nearest = float(distances[:, 1].min())
        if np.isfinite(nearest) and nearest > 0:
            length = min(length, 0.9 * nearest)
    return length
