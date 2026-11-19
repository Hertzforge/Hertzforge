"""The nodes of a geometry that no line or face draws, as point layers.

A geometry of measured points (`io.point_files`) has nothing but nodes
and one-node elements, and a test geometry may be nodes alone. Neither
drew anything until its nodes were drawn as points. A node in a point
element takes the element's color, so the points of one feature read
together; a cloud of bare nodes is colored by height, so its shape
reads without shading; a few bare nodes are gray.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from hertzforge.core.geometry import ELEMENT_TYPES
from hertzforge.drawing.theme import VIRIDIS
from hertzforge.drawing.viz.geometry import color_rgb

#: the most points one layer draws; a larger one is drawn every nth point
DRAWN_POINTS = 500_000

#: this many points or fewer are drawn large: they were probed or placed
#: by hand, and each one matters
PROBED_POINTS = 5000

#: point sizes on screen, in pixels
CLOUD_SIZE, PROBED_SIZE = 2.0, 6.0

#: the color of a few bare nodes (sRGB)
BARE_COLOR = (139, 148, 158)

#: the color of the points of the element groups picked, which reads on a dark and a
#: light ground and is none of the element colors
PICKED_COLOR = (255, 214, 10)


def _by_height(points: np.ndarray) -> np.ndarray:
    """Each point's color from its height along the axis the points
    span least, the way a part lying on a table is read from above."""
    if not len(points):
        return np.empty((0, 3), np.uint8)
    height = points[:, int(np.argmin(np.ptp(points, axis=0)))]
    low, high = float(height.min()), float(height.max())
    at = (height - low) / (high - low) if high > low else np.zeros(len(height))
    stops = np.asarray(VIRIDIS, dtype=float)
    place = at * (len(stops) - 1)
    lower = np.minimum(place.astype(int), len(stops) - 2)
    part = (place - lower)[:, None]
    return np.rint(stops[lower] * (1 - part) + stops[lower + 1] * part).astype(np.uint8)


def _point_elements(geometry: Any) -> np.ndarray:
    """Which elements are one-node point elements, as a mask."""
    codes = np.array(sorted(ELEMENT_TYPES))
    is_point = np.array([ELEMENT_TYPES[int(code)][2] == 'point' for code in codes])
    kinds = np.asarray(geometry.elem_type, dtype=np.int64)
    return is_point[np.searchsorted(codes, kinds)] if len(kinds) else np.zeros(0, dtype=bool)


def _rows_of(geometry: Any, node_ids: np.ndarray) -> np.ndarray:
    """The node rows of `node_ids`."""
    ids = np.asarray(geometry.node_id, dtype=np.int64)
    order = np.argsort(ids, kind='stable')
    return order[np.searchsorted(ids[order], node_ids)]


def undrawn_rows(geometry: Any) -> np.ndarray:
    """The rows of the nodes no line, face or solid element uses. A
    solid's inside nodes are used by it, though only its skin is drawn."""
    points = _point_elements(geometry)
    conn = [c for c, point in zip(geometry.elem_conn, points, strict=True) if not point]
    used = np.zeros(geometry.num_nodes, dtype=bool)
    if conn:
        used[_rows_of(geometry, np.concatenate(conn).astype(np.int64))] = True
    return np.flatnonzero(~used)


def layers(geometry: Any) -> tuple[list[dict[str, Any]], str]:
    """The `points` layers of a geometry's `shape3d` view, and what to say
    when not every point is drawn ('' otherwise). No layers when every
    node is on a line, a face or a solid."""
    rows = undrawn_rows(geometry)
    if not len(rows):
        return [], ''
    xyz = np.asarray(geometry.node_xyz, dtype=float)[rows]
    cloud = len(rows) > PROBED_POINTS
    colors = _by_height(xyz) if cloud else np.tile(np.uint8(BARE_COLOR), (len(rows), 1))
    points = np.flatnonzero(_point_elements(geometry))
    if len(points):
        # a node in a point element takes that element's color
        place = np.full(geometry.num_nodes, -1, dtype=np.int64)
        place[rows] = np.arange(len(rows))
        first = np.array([int(geometry.elem_conn[k][0]) for k in points], dtype=np.int64)
        palette = np.rint(np.array([color_rgb(k) for k in range(10)]) * 255).astype(np.uint8)
        colors[place[_rows_of(geometry, first)]] = palette[
            np.asarray(geometry.elem_color, dtype=np.int64)[points] % len(palette)]
    step = -(-len(rows) // DRAWN_POINTS) if len(rows) > DRAWN_POINTS else 1
    note = f'every {step}th point of {len(rows):,} is drawn' if step > 1 else ''
    return [{'xyz': np.ascontiguousarray(xyz[::step], np.float32),
             'colors': np.ascontiguousarray(colors[::step], np.uint8),
             'size': CLOUD_SIZE if cloud else PROBED_SIZE}], note


def picked_layer(geometry: Any, groups: Any) -> dict[str, Any] | None:
    """A `points` layer of the nodes in the point elements of `groups`, larger than the
    points under them and in one color, so the element groups picked stand out among the
    rest (Brandon, 2026-11-17). None when the groups hold no point element."""
    wanted = np.isin(np.asarray(geometry.elem_group, dtype=np.int64),
                     np.asarray(list(groups), dtype=np.int64))
    picked = np.flatnonzero(_point_elements(geometry) & wanted)
    if not len(picked):
        return None
    first = np.array([int(geometry.elem_conn[k][0]) for k in picked], dtype=np.int64)
    rows = np.unique(_rows_of(geometry, first))
    step = -(-len(rows) // DRAWN_POINTS) if len(rows) > DRAWN_POINTS else 1
    xyz = np.asarray(geometry.node_xyz, dtype=float)[rows][::step]
    size = 2 * (CLOUD_SIZE if len(undrawn_rows(geometry)) > PROBED_POINTS else PROBED_SIZE)
    return {'xyz': np.ascontiguousarray(xyz, np.float32),
            'colors': np.tile(np.uint8(PICKED_COLOR), (len(xyz), 1)), 'size': size}
