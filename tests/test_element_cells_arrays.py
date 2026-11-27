"""`element_cells` draws a solid mesh's skin with array operations, the
views draw each mesh once and keep it until a request that may edit it
(`drawing_once`, `forget_drawings`), and `unique_pairs` finds a mesh's edges
as `np.unique` over rows would. The skins are checked against the rule
applied one cell at a time, on meshes that mix every solid with faces, lines
and points, so a face two cells share, a face three cells name and a cell
with no known faces all come up.
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.core.geometry import ELEMENT_TYPES, Geometry, solid_faces
from hertzforge.drawing.viz import geometry as viz
from hertzforge.drawing.viz.geometry import (
    drawing_once,
    drawn_once,
    element_cells,
    element_shapes,
    forget_drawings,
)
from hertzforge.service import frames, projects
from hertzforge.service.mesh import drawn_mesh, unique_pairs
from hertzforge.service.server import OPERATIONS, Service, start


@pytest.fixture(autouse=True)
def nothing_kept():
    forget_drawings()
    yield
    forget_drawings()


def one_cell_at_a_time(geometry, indices=None, separate_skins=False):
    """The drawing rule applied cell by cell: a solid's face is dropped each
    time another cell of its color names it again and kept when named once
    more, by the corners of the cell that named it last."""
    row = {int(i): r for r, i in enumerate(geometry.node_id)}

    def rows(ids):
        return [row[int(i)] for i in ids]

    faces, lines, points, skins = {}, {}, {}, {}
    for i in (range(len(geometry.elem_conn)) if indices is None else indices):
        code, color = int(geometry.elem_type[i]), int(geometry.elem_color[i])
        conn = geometry.elem_conn[i]
        render = ELEMENT_TYPES[code][2]
        polygons, segments = element_shapes(code, conn)
        if render == 'face':
            faces.setdefault(color, []).extend(rows(f) for f in polygons)
        elif render == 'volume':
            skin = skins.setdefault(color, {})
            for face in polygons:
                corners = rows(face)
                key = tuple(sorted(corners))
                if key in skin:
                    del skin[key]
                else:
                    skin[key] = corners
        elif render == 'line':
            lines.setdefault(color, []).extend(rows(p) for p in segments)
        elif render == 'point':
            points.setdefault(color, []).extend(rows(conn))
    if separate_skins:
        return faces, lines, points, {c: list(s.values()) for c, s in skins.items()}
    for color, skin in skins.items():
        faces.setdefault(color, []).extend(skin.values())
    return faces, lines, points


def mixed(rng, n):
    """An n-cube grid of cells, each a brick, tetrahedron, wedge, pyramid,
    20-node brick, 32-node brick (no known faces), quad, beam or mass, some
    bricks named two or three times, three colors, node ids out of order."""
    ids = rng.permutation(np.arange(1, (n + 1) ** 3 + 1) * 7)
    xyz = np.array([(i, j, k) for k in range(n + 1) for j in range(n + 1)
                    for i in range(n + 1)], dtype=float)

    def node(i, j, k):
        return ids[i + (n + 1) * (j + (n + 1) * k)]

    types, conns = [], []
    for i in range(n):
        for j in range(n):
            for k in range(n):
                c = [node(i, j, k), node(i + 1, j, k), node(i + 1, j + 1, k), node(i, j + 1, k),
                     node(i, j, k + 1), node(i + 1, j, k + 1), node(i + 1, j + 1, k + 1),
                     node(i, j + 1, k + 1)]
                pick = rng.random()
                if pick < 0.45:
                    cells = [(115, c)]
                elif pick < 0.55:
                    cells = [(111, [c[0], c[1], c[3], c[4]])]
                elif pick < 0.65:
                    cells = [(112, [c[0], c[1], c[3], c[4], c[5], c[7]])]
                elif pick < 0.7:
                    cells = [(201, c[:4] + [c[6]])]
                elif pick < 0.75:
                    cells = [(116, c + list(rng.choice(ids, 12)))]
                elif pick < 0.78:
                    cells = [(106, list(rng.choice(ids, 32)))]
                elif pick < 0.84:
                    cells = [(44, c[:4])]
                elif pick < 0.9:
                    cells = [(21, c[:2])]
                elif pick < 0.94:
                    cells = [(161, c[:1])]
                else:
                    cells = [(115, c)] * int(rng.integers(2, 4))
                for code, conn in cells:
                    types.append(code)
                    conns.append(conn)
    return Geometry(node_id=ids, node_xyz=xyz, elem_id=np.arange(1, len(types) + 1),
                    elem_type=types, elem_color=rng.integers(1, 4, len(types)),
                    elem_conn=conns, length_unit='m')


def as_lists(drawn):
    return [list(part.items()) for part in drawn]


@pytest.mark.parametrize('seed', range(40))
def test_the_skins_match_the_rule_one_cell_at_a_time(seed):
    rng = np.random.default_rng(seed)
    geometry = mixed(rng, int(rng.integers(1, 5)))
    count = len(geometry.elem_conn)
    picks = rng.integers(0, count, int(rng.integers(1, 2 * count))).tolist()
    for indices in (None, picks, []):
        for separate in (False, True):
            assert (as_lists(element_cells(geometry, indices, separate))
                    == as_lists(one_cell_at_a_time(geometry, indices, separate))), (indices, separate)


def test_a_face_three_cells_name_is_drawn_by_the_last():
    brick = [1, 2, 3, 4, 5, 6, 7, 8]
    turned = [5, 6, 7, 8, 1, 2, 3, 4]
    geometry = Geometry(node_id=np.arange(1, 9), node_xyz=np.zeros((8, 3)),
                        elem_type=[115] * 3, elem_conn=[brick, brick, turned], length_unit='m')
    (skin,) = element_cells(geometry, separate_skins=True)[3].values()
    assert len(skin) == 6
    assert skin == one_cell_at_a_time(geometry, separate_skins=True)[3][1]
    assert [r + 1 for r in skin[0]] == [turned[k] for k in solid_faces(8)[0]]


def test_a_node_the_mesh_does_not_hold_is_refused_by_its_id():
    geometry = Geometry(node_id=[1, 2, 3, 4], node_xyz=np.zeros((4, 3)), elem_type=[111],
                        elem_conn=[[1, 2, 3, 4]], length_unit='m')
    geometry.elem_conn[0] = np.array([1, 2, 3, 9])
    with pytest.raises(KeyError, match='9'):
        element_cells(geometry)


def test_a_geometry_with_no_elements_draws_nothing():
    geometry = Geometry(node_id=[1, 2], node_xyz=np.zeros((2, 3)), length_unit='m')
    assert element_cells(geometry, separate_skins=True) == ({}, {}, {}, {})


@pytest.mark.parametrize('seed', range(5))
def test_unique_pairs_is_unique_over_rows(seed):
    rng = np.random.default_rng(seed)
    pairs = rng.integers(0, 50, (400, 2))
    pairs[::7] = pairs[::7, ::-1]
    expected = np.unique(np.sort(pairs, axis=1), axis=0)
    found = unique_pairs(pairs)
    assert found.dtype == np.int64 and np.array_equal(found, expected)
    assert unique_pairs(np.zeros((0, 2), dtype=np.int64)).shape == (0, 2)


def counting(monkeypatch):
    calls = []
    cells = viz._cells

    def counted(geometry, indices):
        calls.append(indices)
        return cells(geometry, indices)

    monkeypatch.setattr(viz, '_cells', counted)
    return calls


def test_a_mesh_is_drawn_once_while_drawing_once_holds_and_handed_out_as_copies(monkeypatch):
    calls = counting(monkeypatch)
    geometry = mixed(np.random.default_rng(3), 3)
    with drawing_once():
        first = element_cells(geometry)
        first[0].clear()
        first[1].setdefault(99, []).append([0, 1])
        again = element_cells(geometry, separate_skins=True)
        mesh = drawn_mesh(geometry)
        mesh[1][:] = 0
        assert not np.array_equal(drawn_mesh(geometry)[1], mesh[1])
    assert len(calls) == 1
    assert as_lists(again) == as_lists(one_cell_at_a_time(geometry, separate_skins=True))
    element_cells(geometry)
    element_cells(geometry, [0, 1])
    assert len(calls) == 3, 'outside it, and for a selection, the mesh is drawn each time'


def test_a_geometry_edited_after_a_view_is_drawn_as_it_is_now():
    geometry = mixed(np.random.default_rng(4), 2)
    with drawing_once():
        before = element_cells(geometry)
    assert before[0] and 7 not in before[0]
    geometry.elem_color[:] = 7
    assert set(element_cells(geometry)[0]) == {7}


def test_a_view_of_a_meshed_part_draws_its_elements_once(monkeypatch):
    ops = {op.id: op for op in OPERATIONS}
    state = projects.new({'title': 'x'})
    project = state.project
    project.new_geometry('Part', unit='m')

    def run(op_id, **given):
        op = ops[op_id]
        op.run(project, op.resolve(given, objects={n: projects.kind_of(o)
                                                   for n, o in project.items()}))

    run('project.add_solid_block', geometry='Part')
    run('project.mesh_solids', geometry='Part', solids='1', elements='hexes', size=0.25)
    group = int(project['Part'].group_id[-1])
    calls = counting(monkeypatch)
    view = projects.show(state, {'objects': ['Part'], 'records': {},
                                 'part': {'name': 'Part', 'kind': 'group', 'id': group}})
    assert view['views'], 'the view drew something'
    assert calls == [None], 'every part of the view drew from one drawing of the mesh'


def test_a_drawing_is_kept_from_one_view_to_the_next_until_forgotten(monkeypatch):
    calls = counting(monkeypatch)
    geometry = mixed(np.random.default_rng(5), 2)
    for _ in range(3):
        with drawing_once():
            element_cells(geometry)
    assert len(calls) == 1
    forget_drawings()
    with drawing_once():
        element_cells(geometry)
    assert len(calls) == 2


def test_a_drawing_begun_before_a_forget_is_used_once_and_not_kept():
    geometry = mixed(np.random.default_rng(6), 1)
    drawn = []

    def draw():
        drawn.append(1)
        forget_drawings()          # an edit lands while the view draws
        return len(drawn)

    with drawing_once():
        assert drawn_once('test', geometry, draw) == 1
        assert drawn_once('test', geometry, draw) == 2


def test_the_drawings_of_only_the_last_few_geometries_are_kept():
    shapes = [mixed(np.random.default_rng(10 + k), 1) for k in range(viz.KEPT + 1)]
    drawn = []
    with drawing_once():
        for geometry in shapes:
            drawn_once('test', geometry, lambda: drawn.append(1))
        drawn_once('test', shapes[-1], lambda: drawn.append(1))
        assert len(drawn) == viz.KEPT + 1, 'the newest is still kept'
        drawn_once('test', shapes[0], lambda: drawn.append(1))
        assert len(drawn) == viz.KEPT + 2, 'the oldest was let go'


async def _over_the_wire(requests, after):
    """Each request in turn over a real connection, `after()` called once its answer came."""
    from websockets.asyncio.client import connect

    service = Service()
    server = await start(service, port=0)
    port = server.sockets[0].getsockname()[1]
    try:
        async with connect(f'ws://127.0.0.1:{port}', max_size=None) as socket:
            frames.decode(await socket.recv())
            for ident, (method, params) in enumerate(requests, 1):
                await socket.send(frames.encode_text({'type': 'request', 'id': ident,
                                                      'method': method, 'params': params}))
                while True:
                    message = frames.decode(await socket.recv())
                    if message['type'] == 'response' and message['id'] == ident:
                        assert 'error' not in message, message
                        after()
                        break
    finally:
        server.close()
        await server.wait_closed()
        service.close()


def test_the_service_keeps_a_mesh_across_views_and_draws_it_again_after_an_edit(monkeypatch):
    import asyncio

    drawn: list[int] = []
    cells = viz._cells
    monkeypatch.setattr(viz, '_cells', lambda g, i: (drawn.append(len(g.elem_conn)), cells(g, i))[1])
    picked = {'objects': ['Part'], 'records': {}}

    def op(op_id, **params):
        return ('op.run', {'op': op_id, 'params': params})

    requests = [
        ('project.new', {}),
        op('project.new_geometry', name='Part', unit='m'),
        op('project.add_solid_block', geometry='Part'),
        op('project.mesh_solids', geometry='Part', solids='1', elements='hexes', size=0.5),
        ('project.show', picked),
        ('project.show', picked),
        ('project.actions', {**picked, 'part': None}),
        ('project.show', picked),
        op('project.mesh_solids', geometry='Part', solids='1', elements='hexes', size=0.25),
        ('project.show', picked),
    ]
    after: list[list[int]] = []
    asyncio.run(_over_the_wire(requests, lambda: after.append(list(drawn))))
    new = [after[k][len(after[k - 1]):] if k else after[0] for k in range(len(after))]
    assert new[4] == [8], 'the first view of the 8 hexes draws them'
    assert new[5] == new[6] == new[7] == [], 'the views after it use that drawing'
    assert new[9] and new[9][-1] > 8, 'the view after the second mesh draws it as it is now'
