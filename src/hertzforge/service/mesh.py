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
    return unique_pairs(np.concatenate(out))


def unique_pairs(pairs: np.ndarray) -> np.ndarray:
    """Pairs of node rows, each once whichever way round it came, smaller row first, in
    order: what `np.unique(np.sort(pairs, axis=1), axis=0)` gives, found as one integer a
    pair, since unique over rows sorted a large mesh's edges for seconds."""
    pairs = np.sort(np.asarray(pairs, dtype=np.int64).reshape(-1, 2), axis=1)
    if not len(pairs):
        return pairs
    span = int(pairs.max()) + 1
    joined = np.unique(pairs[:, 0] * span + pairs[:, 1])
    return np.column_stack([joined // span, joined % span])


def element_highlights(geometry: Any, elements: Any) -> tuple[list[list[int]], list[list[int]]]:
    """Each element of `elements` (rows of the geometry's elements) as the 3-D view
    highlights it alone: (triangles, edges), a flat list of node rows per element, the
    triangles fanned as `triangulate` fans them and the edges each polygon's sides, once,
    and a line element's two ends. Built for every element at once, since an element
    table's view carries one for each of its rows."""
    from hertzforge.drawing.viz.geometry import element_shapes

    index = {int(node): row for row, node in enumerate(geometry.node_id)}
    polygons: dict[int, tuple[list[list[int]], list[int]]] = {}
    pairs: list[list[int]] = []
    owners: list[int] = []
    for k, element in enumerate(elements):
        shapes, lines = element_shapes(int(geometry.elem_type[element]), geometry.elem_conn[element])
        for shape in shapes:
            if all(int(node) in index for node in shape):
                held = polygons.setdefault(len(shape), ([], []))
                held[0].append([index[int(node)] for node in shape])
                held[1].append(k)
        for line in lines:
            if all(int(node) in index for node in line):
                pairs.append([index[int(node)] for node in line])
                owners.append(k)
    tri_rows, tri_owners = [np.zeros((0, 3), dtype=np.int64)], [np.zeros(0, dtype=np.int64)]
    edge_rows = [np.asarray(pairs, dtype=np.int64).reshape(-1, 2)]
    edge_owners = [np.asarray(owners, dtype=np.int64)]
    for count, (group, held) in polygons.items():
        shapes, owner = np.asarray(group, dtype=np.int64), np.asarray(held, dtype=np.int64)
        if count >= 3:
            # `triangulate` gives the (count - 2) triangles of every polygon in turn
            tri_rows.append(triangulate(group))
            tri_owners.append(np.tile(owner, count - 2))
        edge_rows.append(np.stack([shapes, np.roll(shapes, -1, axis=1)], axis=-1).reshape(-1, 2))
        edge_owners.append(np.repeat(owner, count))
    triangles, edges = np.concatenate(tri_rows), np.sort(np.concatenate(edge_rows), axis=1)
    tri_owner, edge_owner = np.concatenate(tri_owners), np.concatenate(edge_owners)
    # a solid's faces share their sides: each side once per element
    keyed = np.unique(np.column_stack([edge_owner, edges]), axis=0)
    edge_owner, edges = keyed[:, 0], keyed[:, 1:]

    def split(rows: np.ndarray, owner: np.ndarray) -> list[list[int]]:
        order = np.argsort(owner, kind='stable')
        cuts = np.searchsorted(owner[order], np.arange(1, len(elements)))
        return [part.ravel().tolist() for part in np.split(rows[order], cuts)]

    return split(triangles, tri_owner), split(edges, edge_owner)


def element_colors(geometry: Any, drawn: Any = None) -> dict[str, np.ndarray]:
    """The elements in their own colors (`elem_color`, the palette `color_rgb` reads), as
    `element_cells` draws them: `triangles` and `lines` by node row, and each one's color,
    `triangle_colors` and `line_colors` (u8 RGB). With `drawn`, the geometry rows a view's
    nodes are (`drawn_mesh`'s first), the rows are places among those."""
    from hertzforge.drawing.viz.geometry import color_rgb, element_cells

    faces, lines, _points = element_cells(geometry)
    tri_rows, tri_colors, line_rows, line_colors = [], [], [], []
    for color, group in faces.items():
        triangles = triangulate(group)
        tri_rows.append(triangles)
        tri_colors.append(np.tile(np.round(np.multiply(color_rgb(color), 255)), (len(triangles), 1)))
    for color, group in lines.items():
        pairs = np.asarray(group, dtype=np.int64).reshape(-1, 2)
        line_rows.append(pairs)
        line_colors.append(np.tile(np.round(np.multiply(color_rgb(color), 255)), (len(pairs), 1)))

    def joined(parts: list, width: int, kind: Any) -> np.ndarray:
        return (np.concatenate(parts) if parts else np.zeros((0, width))).astype(kind)

    triangles, pairs = joined(tri_rows, 3, np.int64), joined(line_rows, 2, np.int64)
    if drawn is not None:
        place = np.full(geometry.num_nodes, -1, dtype=np.int64)
        place[np.asarray(drawn, dtype=np.int64)] = np.arange(len(drawn))
        triangles, pairs = place[triangles], place[pairs]
    return {'triangles': triangles.astype(np.uint32),
            'triangle_colors': joined(tri_colors, 3, np.uint8),
            'lines': pairs.astype(np.uint32),
            'line_colors': joined(line_colors, 3, np.uint8)}


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
    from hertzforge.drawing.viz.geometry import drawn_once

    used, triangles, segments = drawn_once('mesh', geometry, lambda: _drawn_mesh(geometry))
    return used.copy(), triangles.copy(), segments.copy()


def _drawn_mesh(geometry: Any) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    from hertzforge.drawing.viz.geometry import element_cells

    faces, lines, _points = element_cells(geometry)
    polygons = [face for group in faces.values() for face in group]
    triangles = triangulate(polygons)
    segments = np.asarray([pair for group in lines.values() for pair in group],
                          dtype=np.int64).reshape(-1, 2)
    if polygons:
        segments = unique_pairs(np.vstack([segments, outline(polygons)]))
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
