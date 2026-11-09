"""Sensor bodies drawn at their nodes: one triangle mesh for every sensor a
channel table places (`core.sensors.place`), so a model with hundreds of
sensors is one mesh rather than hundreds.

Each body is built about its own origin from its catalog size, turned into
its frame and moved to its node, which is therefore the body's center. A
triax's X, Y and Z faces carry their letters in black, as the sensors do; a
uniaxial accelerometer or a force transducer carries a black arrow on each
side along its sensing axis, pointing the positive way. The body is gray,
the connector and the cable darker.

Bodies are true size where the geometry's units are defined. Where they are
not, a millimeter cannot be placed, and the bodies are drawn at a size
proportional to the model's instead. `size` multiplies either, about the
node, so the node stays at the center.

Every point of a body names the node it moves with (`follow`), so an
animation carries the bodies with their nodes: the shell gives each point
its node's motion.
"""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

import numpy as np

#: the body, its connector and cable, and the letters and arrows on it
BODY_COLOR = '#a7abb1'
CONNECTOR_COLOR = '#5d6166'
CABLE_COLOR = '#3b3f44'
MARK_COLOR = '#1a1a1a'

#: a teardrop's narrow end, as a fraction of its round end's diameter
TAIL = 0.42

#: a mark stands this fraction of the body's smallest side off its face,
#: so the face does not draw through it
LIFT = 0.02

#: with the geometry's units undefined, a millimeter is drawn as this
#: fraction of the model's size (a 10 mm cube is 1.5 % of it)
NOMINAL_MM = 0.0015

#: sides of the circle a puck and a connector are drawn with
ROUND = 24

#: `follow` for a point whose node is not among the view's nodes: it stays put
STILL = np.iinfo(np.uint32).max

NOMINAL_NOTE = ("the geometry's units are not defined, so the sensors are "
                'drawn at a size proportional to the model, not true size')


def _rgb(color: str) -> tuple[int, int, int]:
    color = color.lstrip('#')
    return tuple(int(color[i:i + 2], 16) for i in (0, 2, 4))


def _prism(outline: np.ndarray, bottom: float, top: float):
    """A prism along z over a convex outline (counterclockwise): its points
    and its triangles, wound outward."""
    n = len(outline)
    low = np.column_stack([outline, np.full(n, bottom)])
    high = np.column_stack([outline, np.full(n, top)])
    points = np.vstack([low, high])
    triangles = []
    for k in range(1, n - 1):
        triangles.append((0, k + 1, k))
        triangles.append((n, n + k, n + k + 1))
    for k in range(n):
        j = (k + 1) % n
        triangles += [(k, j, n + j), (k, n + j, n + k)]
    return points, triangles


def _circle(radius: float, sides: int, turn: float = 0.0) -> np.ndarray:
    angles = turn + 2 * np.pi * np.arange(sides) / sides
    return radius * np.column_stack([np.cos(angles), np.sin(angles)])


def _teardrop(length: float, width: float) -> np.ndarray:
    """A teardrop along x, counterclockwise: a round end `width` across at
    -length/2 narrowing to a flat tail at +length/2."""
    radius, tail = width / 2, TAIL * width / 2
    cx = -length / 2 + radius
    # the flanks run from the tail's corners to where they touch the circle
    dx, dy = length / 2 - cx, tail
    touch = np.arctan2(dy, dx) + np.arccos(radius / np.hypot(dx, dy))
    # an odd count puts a point on the nose itself, so the length is exact
    arc = np.linspace(touch, 2 * np.pi - touch, ROUND + 1)
    nose = np.column_stack([cx + radius * np.cos(arc), radius * np.sin(arc)])
    return np.vstack([[[length / 2, tail]], nose, [[length / 2, -tail]]])


def _rounded(length: float, width: float) -> np.ndarray:
    """A box along x with its -x end a half circle, counterclockwise: the
    nose is round and the cable leaves the flat end."""
    radius = width / 2
    cx = -length / 2 + radius
    arc = np.linspace(np.pi / 2, 3 * np.pi / 2, ROUND // 2 + 1)
    nose = np.column_stack([cx + radius * np.cos(arc), radius * np.sin(arc)])
    return np.vstack([[[length / 2, radius]], nose, [[length / 2, -radius]]])


def _outline(entry: Any) -> np.ndarray:
    sx, sy, _sz = entry.size
    if entry.shape == 'box':
        return np.array([[-sx, -sy], [sx, -sy], [sx, sy], [-sx, sy]]) / 2
    if entry.shape == 'hex':
        # flats facing +-x, so the connector sits on one
        return _circle(sx / np.sqrt(3.0), 6, np.pi / 6)
    if entry.shape == 'teardrop':
        return _teardrop(sx, sy)
    if entry.shape == 'rounded':
        return _rounded(sx, sy)
    return _circle(sx / 2, ROUND)


#: each letter as strokes in a box one unit high, (right, up) from the
#: letter's center: the strokes of X, Y and Z as the sensors etch them
_LETTERS = {
    'X': [((-0.35, 0.5), (0.35, -0.5)), ((-0.35, -0.5), (0.35, 0.5))],
    'Y': [((-0.35, 0.5), (0.0, 0.0)), ((0.35, 0.5), (0.0, 0.0)),
          ((0.0, 0.0), (0.0, -0.5))],
    'Z': [((-0.35, 0.5), (0.35, 0.5)), ((0.35, 0.5), (-0.35, -0.5)),
          ((-0.35, -0.5), (0.35, -0.5))],
}

#: a stroke's width as a fraction of the letter's height
STROKE = 0.16


def _letter(letter: str, height: float):
    """A letter's strokes as flat polygons in the (right, up) plane,
    `height` high and centered on the origin."""
    polygons = []
    for (r0, u0), (r1, u1) in _LETTERS[letter]:
        a, b = np.array([r0, u0]) * height, np.array([r1, u1]) * height
        along = b - a
        across = np.array([-along[1], along[0]]) / np.linalg.norm(along)
        half = STROKE * height / 2 * across
        polygons.append(np.array([a - half, b - half, b + half, a + half]))
    return polygons


def _arrow(height: float):
    """An arrow up the (right, up) plane, `height` high: its shaft and its
    head, the shaft reaching lower than the head stands tall so the arrow
    reads from either side."""
    shaft, head = 0.07 * height, 0.25 * height
    tail, neck, tip = -0.4 * height, 0.1 * height, 0.5 * height
    return ([np.array([[-shaft, tail], [shaft, tail], [shaft, neck],
                       [-shaft, neck]])],
            [np.array([[-head, neck], [head, neck], [0.0, tip]])])


def _on_face(polygons, origin, normal, up, lift):
    """Flat polygons placed on a face: `origin` is the mark's center on the
    face, `normal` the face's outward normal, `up` the mark's up, both unit
    vectors in the body's frame."""
    right = np.cross(up, normal)
    base = np.asarray(origin, dtype=float) + lift * normal
    placed, triangles, start = [], [], 0
    for polygon in polygons:
        points = base + np.outer(polygon[:, 0], right) + np.outer(polygon[:, 1], up)
        placed.append(points)
        triangles += [(start, start + k, start + k + 1)
                      for k in range(1, len(points) - 1)]
        start += len(points)
    return np.vstack(placed), triangles


def _arrow_sides(outline: np.ndarray) -> list[tuple[np.ndarray, np.ndarray]]:
    """Where a uniaxial's arrows go: on each side of the body, the longest
    side facing that way; among equals the one facing it most squarely (a
    puck's facets), then the one farther from the connector (a hex's two
    flats either side of its back)."""
    chosen = []
    n = len(outline)
    for sign in (1.0, -1.0):
        candidates = []
        for k in range(n):
            a, b = outline[k], outline[(k + 1) % n]
            edge = b - a
            normal = np.array([edge[1], -edge[0]]) / np.linalg.norm(edge)
            if normal[1] * sign > 0.5:
                candidates.append((-round(float(np.linalg.norm(edge)), 6),
                                   -round(abs(float(normal[1])), 6),
                                   normal[0], (a + b) / 2, normal))
        candidates.sort(key=lambda c: c[:3])
        chosen.append((candidates[0][3], candidates[0][4]))
    return chosen


def _cylinder(radius: float, start: float, length: float, sides: int,
              axis: str = 'x', sign: float = 1.0):
    """A cylinder of `length` along +-x or +-z from `start` along it."""
    points, triangles = _prism(_circle(radius, sides), 0.0, length)
    if axis == 'x':
        points = np.column_stack([sign * (start + points[:, 2]),
                                  points[:, 0], points[:, 1]])
    else:
        points = np.column_stack([points[:, 0], points[:, 1],
                                  sign * (start + points[:, 2])])
    if sign < 0:
        triangles = [(a, c, b) for a, b, c in triangles]
    return points, triangles


def body(entry: Any, marks: bool = True) -> tuple[np.ndarray, np.ndarray, list[str]]:
    """A sensor's body in its own frame, in millimeters, centered on the
    origin: (points, triangles, parts), where a triangle's part is 'body',
    'connector' (the plug and a force transducer's studs), 'cable', or a
    mark on the body: 'mark:X', 'mark:Y' and 'mark:Z' for a triax's letters,
    'mark:shaft' and 'mark:head' for the arrow of a uniaxial accelerometer
    or a force transducer, left out when `marks` is False."""
    sx, sy, sz = entry.size
    outline = _outline(entry)
    pieces = [(*_prism(outline, -sz / 2, sz / 2), 'body')]
    # the connector: a plug out of the middle of its face, stepping down to
    # its thread, and the cable beyond it
    sign = 1.0 if entry.connector == '+x' else -1.0
    reach = outline[:, 0].max() if sign > 0 else -outline[:, 0].min()
    diameter, length = entry.plug
    if length > 0:
        base = 0.45 * length
        pieces.append((*_cylinder(diameter / 2, reach, base, 12, 'x', sign),
                       'connector'))
        pieces.append((*_cylinder(0.6 * diameter / 2, reach + base,
                                  length - base, 12, 'x', sign), 'connector'))
        reach += length
    diameter, length = entry.cable
    if length > 0:
        pieces.append((*_cylinder(diameter / 2, reach, length, 8, 'x', sign),
                       'cable'))
    diameter, length = entry.stud
    if length > 0:
        for end in (1.0, -1.0):
            pieces.append((*_cylinder(diameter / 2, sz / 2, length, 12, 'z',
                                      end), 'connector'))
    if marks:
        lift = LIFT * min(entry.size)
        if entry.kind == 'triax':
            # the letter on each positive face, upright as the sensors etch
            # them: Z read with the connector below it
            faces = (('X', (sx / 2, 0, 0), (1, 0, 0), (0, 0, 1), min(sy, sz)),
                     ('Y', (0, sy / 2, 0), (0, 1, 0), (0, 0, 1), min(sx, sz)),
                     ('Z', (0, 0, sz / 2), (0, 0, 1), (1, 0, 0), min(sx, sy)))
            for letter, origin, normal, up, side in faces:
                placed = _on_face(_letter(letter, 0.6 * side), origin,
                                  np.array(normal, float), np.array(up, float),
                                  lift)
                pieces.append((*placed, f'mark:{letter}'))
        else:
            shafts_and_heads = _arrow(0.7 * sz)
            for center, normal in _arrow_sides(outline):
                origin = np.array([center[0], center[1], 0.0])
                normal = np.array([normal[0], normal[1], 0.0])
                for polygons, part in zip(shafts_and_heads, ('shaft', 'head'),
                                          strict=True):
                    placed = _on_face(polygons, origin, normal,
                                      np.array([0.0, 0.0, 1.0]), lift)
                    pieces.append((*placed, f'mark:{part}'))
    all_points, all_triangles, parts = [], [], []
    offset = 0
    for points, triangles, part in pieces:
        all_points.append(points)
        all_triangles += [(a + offset, b + offset, c + offset)
                          for a, b, c in triangles]
        parts += [part] * len(triangles)
        offset += len(points)
    return (np.vstack(all_points), np.asarray(all_triangles, dtype=np.int64),
            parts)


def millimeter(geometry: Any) -> tuple[float, list[str]]:
    """A millimeter in the geometry's coordinates, and the note when it is
    only proportional to the model's size."""
    if geometry.units_defined:
        return 1e-3, []
    xyz = np.asarray(geometry.node_xyz, dtype=float).reshape(-1, 3)
    span = float(np.linalg.norm(xyz.max(axis=0) - xyz.min(axis=0))) if len(xyz) else 0.0
    return NOMINAL_MM * (span or 1.0), [NOMINAL_NOTE]


def sensor_bodies(sensors: list, geometry: Any, index: Callable[[int], int | None],
                  size: float = 1.0) -> tuple[dict[str, np.ndarray] | None, list[str]]:
    """One mesh of every sensor's body at its node, in the geometry's
    coordinates, and the notes.

    Parameters
    ----------
    sensors : list of Sensor
        From `core.sensors.place`.
    geometry : Geometry
        The geometry they are placed on.
    index : callable
        A node id's row among the view's nodes, or None when the view does
        not draw it (its sensor's points are then `STILL`).
    size : float, default 1.0
        A multiple of true size (or of the proportional size, when the
        geometry's units are not defined), about each node.

    Returns
    -------
    bodies : dict or None
        `nodes` f32 [P, 3], `triangles` u32 [T, 3], `colors` u8 [P, 3] and
        `follow` u32 [P], each point's node row; None with no sensors.
    notes : list of str
    """
    if not sensors:
        return None, []
    per_mm, notes = millimeter(geometry)
    colors = {'body': _rgb(BODY_COLOR), 'connector': _rgb(CONNECTOR_COLOR),
              'cable': _rgb(CABLE_COLOR), 'mark': _rgb(MARK_COLOR)}
    all_points, all_triangles, all_colors, all_follow = [], [], [], []
    count = 0
    for sensor in sensors:
        points, triangles, parts = body(sensor.entry, marks=sensor.marked)
        all_points.append(sensor.center + (points * per_mm * size) @ sensor.frame.T)
        all_triangles.append(triangles + count)
        # a piece's points are its own, so a point takes its triangles' part
        point_colors = np.empty((len(points), 3), dtype=np.uint8)
        point_colors[triangles.ravel()] = np.repeat(
            [colors[part.split(':')[0]] for part in parts], 3, axis=0)
        all_colors.append(point_colors)
        row = index(int(sensor.node))
        all_follow.append(np.full(len(points), STILL if row is None else row, dtype=np.uint32))
        count += len(points)
    return ({'nodes': np.vstack(all_points).astype(np.float32),
             'triangles': np.vstack(all_triangles).astype(np.uint32),
             'colors': np.vstack(all_colors),
             'follow': np.concatenate(all_follow)}, notes)


def rows_of(node_ids: Any) -> Callable[[int], int | None]:
    """`index` for `sensor_bodies` over a view's drawn node ids."""
    rows = {int(node): row for row, node in enumerate(np.asarray(node_ids).ravel())}
    return rows.get
