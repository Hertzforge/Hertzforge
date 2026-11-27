"""Drawing decisions for `geometry`: what a figure needs
to know, with none of the code that draws it."""

from __future__ import annotations

import threading
from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from typing import TYPE_CHECKING, Any

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


#: what each palette color is called, as an element's Color cell names it
PALETTE_NAMES = ('gray', 'blue', 'orange', 'green', 'red', 'purple', 'brown', 'pink',
                 'olive', 'cyan')


def color_text(index: int) -> str:
    """A color index as a cell says it: the index and its color's name, '3 green'."""
    return f'{int(index)} {PALETTE_NAMES[int(index) % len(PALETTE)]}'


def color_index(text: str) -> int:
    """The color index a typed color names: a whole number from 0 (a name after it is
    ignored, so '3 green' reads as 3), or a palette color's name."""
    words = str(text).strip().lower().split()
    if not words:
        raise ValueError('name a color: a number from 0, or ' + ', '.join(PALETTE_NAMES))
    if words[0].isdigit():
        return int(words[0])
    if words[0] in PALETTE_NAMES:
        return PALETTE_NAMES.index(words[0])
    raise ValueError(f'"{text}" is not a color: a number from 0, or '
                     + ', '.join(PALETTE_NAMES))


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


def element_shapes(code: int, conn: Sequence[int]) -> tuple[list[list[int]], list[list[int]]]:
    """(polygons, lines) one element is drawn by on its own, as its nodes: a face
    element its corner nodes (`face_corners`), a solid every face of its cell
    (`solid_faces`), a line element its first two nodes, a point element nothing.
    `element_cells` draws every element from these, and the 3-D view highlights one
    element by them."""
    render = ELEMENT_TYPES[code][2]
    if render == 'face':
        return [list(conn[:face_corners(code)])], []
    if render == 'volume':
        return [[conn[k] for k in face] for face in solid_faces(len(conn))], []
    if render == 'line':
        return [], [list(conn[:2])]
    return [], []


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
    drawn = (drawn_once('cells', geometry, lambda: _cells(geometry, None)) if indices is None
             else _cells(geometry, indices))
    faces, lines, points, skins = ({color: list(entries) for color, entries in part.items()}
                                   for part in drawn)
    if separate_skins:
        return faces, lines, points, skins
    for color, skin in skins.items():
        faces.setdefault(color, []).extend(skin)
    return faces, lines, points


#: how many geometries' drawings are kept at once
KEPT = 4


class _Kept:
    """The drawings `drawn_once` keeps, by kind and geometry, with the
    generation `forget_drawings` was at when each was drawn."""

    def __init__(self) -> None:
        self.lock: threading.Lock = threading.Lock()
        self.generation: int = 0
        self.drawings: dict[tuple[str, int], tuple[Any, int, Any]] = {}
        self.reading: threading.local = threading.local()


_kept = _Kept()


@contextmanager
def drawing_once() -> Iterator[None]:
    """While it holds, `element_cells` (and the mesh drawn from it) draws a
    geometry once and hands out copies after, and keeps the drawing for the
    requests that follow until `forget_drawings`: one view of a large mesh asks
    for it several times, and every click on the mesh asks for a view. Only for
    work that changes no geometry; whatever may change one calls
    `forget_drawings` before and after."""
    before = getattr(_kept.reading, 'on', False)
    _kept.reading.on = True
    try:
        yield
    finally:
        _kept.reading.on = before


def forget_drawings() -> None:
    """Drop every drawing `drawing_once` kept, and any being drawn now, so a
    geometry edited from here on is drawn as it is."""
    with _kept.lock:
        _kept.generation += 1
        _kept.drawings.clear()


def drawn_once(kind: str, geometry: Geometry, draw: Any) -> Any:
    """`draw()`, kept for `geometry` under `kind` while `drawing_once` holds,
    drawn afresh otherwise. A drawing begun before `forget_drawings` was last
    called is used once: it carries the generation it began in, and is not
    handed out in a later one."""
    if not getattr(_kept.reading, 'on', False):
        return draw()
    key = (kind, id(geometry))
    with _kept.lock:
        generation = _kept.generation
        kept = _kept.drawings.get(key)
        if kept is not None and kept[0] is geometry and kept[1] == generation:
            _kept.drawings[key] = _kept.drawings.pop(key)
            return kept[2]
    drawn = draw()
    with _kept.lock:
        _kept.drawings.pop(key, None)
        _kept.drawings[key] = (geometry, generation, drawn)
        held = {k[1] for k in _kept.drawings}
        while len(held) > KEPT:
            oldest = next(iter(_kept.drawings))[1]
            for k in [k for k in _kept.drawings if k[1] == oldest]:
                del _kept.drawings[k]
            held.discard(oldest)
    return drawn


def _cells(geometry: Geometry, indices: Sequence[int] | None) -> tuple[dict, dict, dict, dict]:
    """`element_cells` with the skins apart: (faces, lines, points, skins)."""
    row_of = _node_rows(geometry)

    def rows(node_ids):
        return row_of(node_ids).tolist()

    order = (np.arange(len(geometry.elem_conn)) if indices is None
             else np.asarray(indices, dtype=np.int64).reshape(-1))
    codes = np.asarray(geometry.elem_type, dtype=np.int64)[order]
    volume_codes = [c for c in np.unique(codes).tolist() if ELEMENT_TYPES[c][2] == 'volume']
    volume = np.isin(codes, volume_codes)
    faces: dict[int, list[list[int]]] = {}
    lines: dict[int, list[list[int]]] = {}
    points: dict[int, list[int]] = {}
    for i in order[~volume].tolist():
        code, color = int(geometry.elem_type[i]), int(geometry.elem_color[i])
        conn = geometry.elem_conn[i]
        render = ELEMENT_TYPES[code][2]
        polygons, segments = element_shapes(code, conn)
        if render == 'face':
            faces.setdefault(color, []).extend(rows(face) for face in polygons)
        elif render == 'line':
            lines.setdefault(color, []).extend(rows(pair) for pair in segments)
        elif render == 'point':
            points.setdefault(color, []).extend(rows(conn))
    return faces, lines, points, _skins(geometry, order[volume], row_of)


def _node_rows(geometry: Geometry):
    """A function from node ids to their rows in `node_id`, the last row
    for an id listed twice, raising KeyError for an id that is not there."""
    ids = np.asarray(geometry.node_id, dtype=np.int64).reshape(-1)
    sorter = np.argsort(ids, kind='stable')

    def row_of(node_ids: ArrayLike) -> np.ndarray:
        wanted = np.asarray(node_ids, dtype=np.int64)
        at = np.searchsorted(ids, wanted, side='right', sorter=sorter) - 1
        found = (at >= 0) & (ids[sorter[at.clip(0)]] == wanted) if len(ids) else at >= len(ids)
        if not found.all():
            raise KeyError(int(wanted[~found].flat[0]))
        return sorter[at]

    return row_of


def _skins(geometry: Geometry, elements: np.ndarray, row_of) -> dict[int, list[list[int]]]:
    """The skin of each color's solids, as `element_cells` returns it: every
    face of their cells (`solid_faces`) that an odd number of the color's
    cells name, as the corner rows of the last cell that names it, in the
    order those cells come in `elements`. A solid draws as its skin because a
    polyline through every corner drew a mesh's insides as a tangle."""
    # The faces are matched as arrays: a loop over the cells one at a time
    # took seconds per view of a 750,000-hex mesh, and the views a few
    # clicks asked for queued behind each other for minutes.
    stride = 8                                # more than any cell's faces
    colors = np.asarray(geometry.elem_color, dtype=np.int64)[elements]
    _, first = np.unique(colors, return_index=True)
    out: dict[int, list[list[int]]] = {int(c): [] for c in colors[np.sort(first)]}
    sizes = np.fromiter((len(geometry.elem_conn[i]) for i in elements.tolist()),
                        dtype=np.int64, count=len(elements))
    by_length: dict[int, list[tuple[np.ndarray, np.ndarray, np.ndarray]]] = {}
    for size in np.unique(sizes).tolist():
        cell_faces = solid_faces(size)
        if not cell_faces:
            continue
        at = np.flatnonzero(sizes == size)
        cells = row_of(np.stack([geometry.elem_conn[i] for i in elements[at].tolist()]))
        for j, face in enumerate(cell_faces):
            by_length.setdefault(len(face), []).append(
                (at * stride + j, colors[at], cells[:, list(face)]))
    kept: list[tuple[np.ndarray, np.ndarray, np.ndarray]] = []
    for parts in by_length.values():
        where = np.concatenate([p[0] for p in parts])
        color = np.concatenate([p[1] for p in parts])
        corners = np.concatenate([p[2] for p in parts])
        sort = np.argsort(where, kind='stable')
        where, color, corners = where[sort], color[sort], corners[sort]
        words = _packed(np.column_stack([color - color.min(), np.sort(corners, axis=1)]))
        # lexsort is stable, so each face's last cell ends its run
        by_key = np.lexsort(words[::-1])
        ordered = [word[by_key] for word in words]
        differs = np.zeros(max(len(by_key) - 1, 0), dtype=bool)
        for word in ordered:
            differs |= word[1:] != word[:-1]
        starts = np.flatnonzero(np.r_[True, differs])
        ends = np.r_[starts[1:], len(by_key)]
        odd = (ends - starts) % 2 == 1
        keep = np.sort(by_key[ends[odd] - 1])
        kept.append((where[keep], color[keep], corners[keep]))
    if kept:
        where = np.concatenate([k[0] for k in kept])
        sort = np.argsort(where, kind='stable')
        color = np.concatenate([k[1] for k in kept])[sort]
        corners = [row for k in kept for row in k[2].tolist()]
        for c, row in zip(color.tolist(), (corners[s] for s in sort.tolist())):
            out[c].append(row)
    return out


def _packed(columns: np.ndarray) -> list[np.ndarray]:
    """The rows of a non-negative integer array as a few unsigned 64-bit
    words each, as many columns to a word as their widths allow, earlier
    columns in higher bits, so the words sort in the order the rows do."""
    words: list[np.ndarray] = []
    used = 64
    for column in columns.T:
        column = column.astype(np.uint64)
        width = max(int(column.max()).bit_length(), 1) if len(column) else 1
        if used + width > 64:
            words.append(column.copy())
            used = width
        else:
            words[-1] = (words[-1] << np.uint64(width)) | column
            used += width
    return words


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


def dof_arrows_on(geometry: Geometry, dofs: Sequence[str], measured_on: Geometry | None = None,
                  distance: float | None = None) -> list[tuple[int, np.ndarray, int]]:
    """For each DOF, (node row of `geometry`, global direction, axis) for its arrow, or
    ValueError naming the DOFs that have no place on `geometry`.

    `measured_on` is the geometry whose node ids the DOFs name. When it is None or
    `geometry` itself the ids are `geometry`'s own (`dof_arrow_vectors`). Otherwise each
    DOF's node is found on `measured_on`, its direction turned by that node's displacement
    system there, and the arrow is drawn at the node of `geometry` nearest it, as DOF
    Mapping matches a test geometry to a model (`core.correlate.project_shapes`): no
    further than `distance` (meters when the units are defined), or the match distance
    (`core.correlate.match_distance`) when none is given. A test's node 101 is
    usually some other node of a finite element mesh, so the ids are never read
    across two geometries.
    """
    from ...core.correlate import match_distance, si_points

    on = geometry if measured_on is None else measured_on
    found = dof_arrow_vectors(on, dofs)
    lost = [str(dof) for dof, hit in zip(dofs, found, strict=True) if hit is None]
    if lost:
        where = 'no node or direction on the geometry they were measured on' \
            if on is not geometry else 'no node or direction on the geometry'
        raise ValueError(f'{_counted(lost)} {"has" if len(lost) == 1 else "have"} {where}: '
                         f'{_listed(lost)}')
    if on is geometry:
        return list(found)
    if geometry.units_defined != on.units_defined:
        raise ValueError('the geometry the DOFs were measured on and the one they are drawn on '
                         'need units defined (or neither) to be matched by position')
    if distance is not None and not (np.isfinite(distance) and distance > 0):
        raise ValueError(f'the node match distance is {distance}; it must be above zero')
    from scipy.spatial import cKDTree

    limit = float(distance) if distance is not None else match_distance(geometry, on)
    gaps, nearest = cKDTree(si_points(geometry)).query(si_points(on)[[hit[0] for hit in found]])
    far = sorted({int(on.node_id[hit[0]]) for hit, gap in zip(found, gaps, strict=True)
                  if gap > limit})
    if far:
        unit = 'm' if geometry.units_defined else 'in model units'
        raise ValueError(
            f'{_counted([str(n) for n in far], "measured node")} ({_listed([str(n) for n in far])}) '
            f'{"has" if len(far) == 1 else "have"} no node of the geometry within '
            f'{limit:.4g} {unit} (the furthest is {float(gaps.max()):.4g} {unit} away); check '
            'that the geometries overlay, or give a larger distance')
    return [(int(row), vector, axis)
            for row, (_row, vector, axis) in zip(nearest, found, strict=True)]


def _counted(items: Sequence[str], what: str = 'DOF') -> str:
    return f'{len(items)} {what}{"s" * (len(items) != 1)}'


def _listed(items: Sequence[str], shown: int = 8) -> str:
    more = f' and {len(items) - shown} more' if len(items) > shown else ''
    return ', '.join(items[:shown]) + more


def dof_arrow_solids(xyz: np.ndarray,
                     drawn: Sequence[tuple[bool, tuple[int, np.ndarray, int]]],
                     length: float | Sequence[float], labels: Sequence[str]) -> dict[str, Any]:
    """The `arrows` of a `shape3d`: one solid arrow per DOF, `length` long, or each its own
    length when `length` holds one per arrow. `drawn` is (points in, (node row, direction,
    axis)) per arrow; an arrow pointing in ends on its node, the others start there. `tails`
    f32 [A, 3] is where each starts, `directions` f32 [A, 3] the unit vector it points along,
    and `axes` u8 [A] the DOF's own axis (0 X, 1 Y, 2 Z), which picks its color. With one length
    each they are `lengths` f32 [A], and `length` is the longest. `labels` names each arrow's DOF
    and `inward` u8 [A] is 1 for an arrow that points in, so a label can sit at the end away
    from the node. A caller whose arrows move with the model adds `follow` u32 [A], the row of
    the view's `nodes` whose motion each arrow takes."""
    each = np.ndim(length) > 0
    lengths = (np.asarray(length, dtype=float) if each
               else np.full(len(drawn), float(length)))
    if each and len(lengths) != len(drawn):
        raise ValueError(f'{len(lengths)} arrow lengths for {len(drawn)} arrows')
    tails = np.zeros((len(drawn), 3), dtype=np.float32)
    directions = np.zeros((len(drawn), 3), dtype=np.float32)
    axes = np.zeros(len(drawn), dtype=np.uint8)
    for k, (into, (row, vector, axis)) in enumerate(drawn):
        tails[k] = xyz[row] - lengths[k] * vector if into else xyz[row]
        directions[k] = vector
        axes[k] = axis
    out = {'tails': tails, 'directions': directions, 'axes': axes,
           'length': float(lengths.max(initial=0.0)) if each else float(length),
           'labels': [str(label) for label in labels],
           'inward': np.array([bool(into) for into, _hit in drawn], dtype=np.uint8)}
    if each:
        out['lengths'] = lengths.astype(np.float32)
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
