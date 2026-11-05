"""A geometry as the viz kit draws it: the nodes a triangle or a line
uses, and the triangles and lines indexing them."""

from __future__ import annotations

from typing import Any

import numpy as np


def triangulate(faces: list[list[int]]) -> np.ndarray:
    """Polygons (3 or 4 corners, as `element_cells` draws faces and solid
    skins) as triangles, fanned from each polygon's first corner."""
    out = []
    by_count: dict[int, list[list[int]]] = {}
    for face in faces:
        by_count.setdefault(len(face), []).append(face)
    for count, group in by_count.items():
        if count < 3:
            continue
        polygons = np.asarray(group, dtype=np.int64)
        for k in range(1, count - 1):
            out.append(polygons[:, [0, k, k + 1]])
    return (np.concatenate(out) if out
            else np.zeros((0, 3), dtype=np.int64))


def drawn_mesh(geometry: Any) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """(node rows drawn, triangles, lines), the last two indexing the drawn
    nodes. Faces and lines are VD's own drawing of the elements
    (`viz.geometry.element_cells`: a face element by its corners, a solid by
    its skin, a line by its two ends). Only nodes a triangle or a line uses
    are sent, so a brick model's interior nodes do not travel; a geometry
    with no faces or lines sends every node."""
    from hertzforge.drawing.viz.geometry import element_cells

    faces, lines, _points = element_cells(geometry)
    triangles = triangulate([face for group in faces.values()
                             for face in group])
    segments = np.asarray([pair for group in lines.values() for pair in group],
                          dtype=np.int64).reshape(-1, 2)
    used = np.unique(np.concatenate([triangles.ravel(), segments.ravel()]))
    if not used.size:
        used = np.arange(geometry.num_nodes)
    remap = np.full(geometry.num_nodes, -1, dtype=np.int64)
    remap[used] = np.arange(used.size)
    return used, remap[triangles], remap[segments]
