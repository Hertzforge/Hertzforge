"""A geometry as the viz kit draws it: the nodes a triangle or a line
uses, and the triangles and lines indexing them."""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v


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


def outline(faces: list[list[int]]) -> np.ndarray:
    """The edges of polygons (as `element_cells` draws faces and solid
    skins), each pair of corners once whichever face or way round it came:
    the outline of every element face, without the diagonals the triangles
    add."""
    out = []
    by_count: dict[int, list[list[int]]] = {}
    for face in faces:
        by_count.setdefault(len(face), []).append(face)
    for count, group in by_count.items():
        if count < 2:
            continue
        polygons = np.asarray(group, dtype=np.int64)
        out.append(np.stack([polygons, np.roll(polygons, -1, axis=1)], axis=-1).reshape(-1, 2))
    if not out:
        return np.zeros((0, 2), dtype=np.int64)
    return np.unique(np.sort(np.concatenate(out), axis=1), axis=0)


def drawn_mesh(geometry: Any) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """(node rows drawn, triangles, lines), the last two indexing the drawn
    nodes. Faces and lines are the drawing of the elements
    (`viz.geometry.element_cells`: a face element by its corners, a solid by
    its skin, a line by its two ends). The lines are the line elements and
    the outline of every face drawn (`outline`), so each element's edges show
    on the surface as the earlier toolset drew them (Brandon, 2026-11-11). Only
    nodes a triangle or a line uses are sent, so a brick model's interior
    nodes do not travel; a geometry with no faces or lines sends every
    node."""
    from hertzforge.drawing.viz.geometry import element_cells

    faces, lines, _points = element_cells(geometry)
    polygons = [face for group in faces.values() for face in group]
    triangles = triangulate(polygons)
    segments = np.asarray([pair for group in lines.values() for pair in group],
                          dtype=np.int64).reshape(-1, 2)
    if polygons:
        segments = np.unique(np.sort(np.vstack([segments, outline(polygons)]), axis=1), axis=0)
    used = np.unique(np.concatenate([triangles.ravel(), segments.ravel()]))
    if not used.size:
        used = np.arange(geometry.num_nodes)
    remap = np.full(geometry.num_nodes, -1, dtype=np.int64)
    remap[used] = np.arange(used.size)
    return used, remap[triangles], remap[segments]


def places_view(geometry: Any, name: str, op: str, params: dict[str, Any], places: list[int],
                out: set[int], hint: str) -> dict:
    """A geometry with `places` (node ids) marked through it, as its first rows and those in
    `out` last among them and dim, so a click on one reruns `op` with it in `toggle`."""
    rows, triangles, lines = drawn_mesh(geometry)
    index = {int(node): i for i, node in enumerate(geometry.node_id)}
    ordered = [index[n] for n in places if n not in out] + [index[n] for n in places if n in out]
    first = set(ordered)
    final = np.asarray(ordered + [int(r) for r in rows if int(r) not in first], dtype=np.int64)
    position = np.full(geometry.num_nodes, -1, dtype=np.int64)
    position[final] = np.arange(final.size)
    return v.shape3d(
        name, 'm' if geometry.units_defined else 'undefined',
        np.asarray(geometry.node_xyz, dtype=float)[final], [],
        triangles=position[rows[triangles]] if len(triangles) else triangles,
        lines=position[rows[lines]] if len(lines) else lines,
        node_ids=np.asarray(geometry.node_id)[final],
        pick=v.picking(op, params, 'nodes', hint,
                       ids=[int(geometry.node_id[i]) for i in ordered], count=1, param='toggle',
                       off=sum(n in out for n in places)))


def sides_view(geometry: Any, name: str, op: str, params: dict[str, Any],
               picking: tuple[tuple[int, ...], set[int]], other: tuple[tuple[int, ...], set[int]],
               hint: str, pickable: bool = True) -> dict:
    """A geometry drawn see-through with two sets of places marked through it, each as
    (the nodes offered, those held): `picking`, whose nodes a click toggles in `op`'s
    `toggle`, as the first rows, those held first and the rest dim; then `other`, drawn in
    a second colour and not clicked (`pick.others`: how many held, how many not). A mate's
    two sides: coincident nodes of the two are told apart by which side is picked."""
    rows, triangles, lines = drawn_mesh(geometry)
    index = {int(node): i for i, node in enumerate(geometry.node_id)}

    def split(offered: tuple[int, ...], held: set[int]) -> tuple[list[int], list[int]]:
        return ([index[n] for n in offered if n in held],
                [index[n] for n in offered if n not in held])

    on, off = split(*picking)
    other_on, other_off = split(*other)
    if not pickable:
        on, off, other_on, other_off = [], [], on + other_on, off + other_off
    marked = on + off + other_on + other_off
    first = set(marked)
    final = np.asarray(marked + [int(r) for r in rows if int(r) not in first], dtype=np.int64)
    position = np.full(geometry.num_nodes, -1, dtype=np.int64)
    position[final] = np.arange(final.size)
    return v.shape3d(
        name, 'm' if geometry.units_defined else 'undefined',
        np.asarray(geometry.node_xyz, dtype=float)[final], [],
        triangles=position[rows[triangles]] if len(triangles) else triangles,
        lines=position[rows[lines]] if len(lines) else lines,
        node_ids=np.asarray(geometry.node_id)[final], opacity=SEE_THROUGH,
        pick=v.picking(op, params, 'nodes', hint,
                       ids=[int(geometry.node_id[i]) for i in on + off], count=1,
                       param='toggle', off=len(off), others=[len(other_on), len(other_off)]))


#: how solid a model is drawn when what matters is on its surfaces inside
SEE_THROUGH = 0.3
