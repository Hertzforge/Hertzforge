"""One figure as one interactive HTML file (`hertzforge.export_html`).

For an interactive slide deck beside a paper's printed figures (Brandon,
2026-09-27): the report's own figure — zoom, pan and readout on a plot,
turning and animation on a scene — alone in a file that opens offline
and, with `fill`, takes whatever frame holds it.
"""

from __future__ import annotations

import json
import re

import numpy as np
import pytest

import hertzforge
from hertzforge.core.data import Psd, Specification


def _pair():
    f = np.linspace(20.0, 2000.0, 400)
    level = np.full((1, f.size), 1e-3)
    spec = Specification(f, level, response_dof=['101Z+'],
                         ordinate_dim=['acceleration**2/frequency'],
                         warning_upper=level * 2, warning_lower=level / 2,
                         abort_upper=level * 4, abort_lower=level / 4)
    measured = Psd(f, level * 1.2, response_dof=['101Z+'],
                   reference_dof=['101Z+'],
                   ordinate_dim=['acceleration**2/frequency'])
    return measured, spec


def _payload(path):
    with open(path, encoding='utf-8') as page:
        text = page.read()
    found = re.search(r'<script id="data" type="application/json">(.*?)</script>',
                      text, re.DOTALL)
    return text, json.loads(found.group(1))


def test_a_comparison_is_one_self_contained_file(tmp_path):
    measured, spec = _pair()
    path = hertzforge.export_html(tmp_path / 'spec.html', measured,
                                      specification=spec, channel='101Z+',
                                      theme='light')
    text, data = _payload(path)
    assert not re.search(r'(src|href)="https?://', text), 'opens offline'
    assert data['fill'] is True and data['theme'] == 'light'
    assert data['marking'] == '' and data['title'] == ''
    [block] = data['blocks']
    assert block['kind'] == 'plot' and block['curves'], 'the one figure, drawn'


def test_a_scene_takes_its_shapes(tmp_path):
    from hertzforge import mesh
    from hertzforge.core.shapes import ShapeSet

    plate = mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0), 0.5, unit=None)
    dofs = [f'{n}Z+' for n in plate.node_id]
    shapes = ShapeSet([10.0], [0.01], dofs, np.ones((1, len(dofs))))
    path = hertzforge.export_html(tmp_path / 'm.html', geometry=plate,
                                      shapes=shapes)
    _text, data = _payload(path)
    [block] = data['blocks']
    assert block['kind'] == 'scene' and len(block['modes']) == 1
    assert data['theme'] is None, 'unset, it follows the reader'


def test_a_brick_is_drawn_as_its_six_faces_and_a_block_as_its_skin(tmp_path):
    """The HTML scene draws an element as the 3-D view does. It drew
    every element of three nodes or more as one polygon in node order,
    and a brick's eight nodes in that order fold into a bow tie: the
    website's brick examples drew no cubes at all (Brandon, 2026-10-11).
    One brick is six square faces of a unit cube; two bricks sharing a
    face are the ten faces of their skin, the shared one gone."""
    from hertzforge import mesh

    brick = mesh.block((0, 0, 0), (1, 0, 0), (0, 1, 0), (0, 0, 1), 1, unit=None)
    _text, data = _payload(hertzforge.export_html(tmp_path / 'b.html',
                                                      geometry=brick))
    [block] = data['blocks']
    points = np.asarray(block['points'], dtype=float)
    faces = [points[face['nodes']] for face in block['faces']]
    assert len(faces) == 6 and all(len(face) == 4 for face in faces)
    for face in faces:
        # four corners of the cube, all on one side of it, going round
        flat = [axis for axis in range(3) if np.ptp(face[:, axis]) == 0]
        assert len(flat) == 1 and face[0, flat[0]] in (0.0, 1.0)
        sides = np.linalg.norm(np.roll(face, -1, axis=0) - face, axis=1)
        assert np.allclose(sides, 1.0), 'edges, not diagonals'
    two = mesh.block((0, 0, 0), (2, 0, 0), (0, 1, 0), (0, 0, 1), 1, unit=None)
    _text, data = _payload(hertzforge.export_html(tmp_path / 't.html',
                                                      geometry=two))
    assert len(data['blocks'][0]['faces']) == 10


def test_a_higher_order_element_is_drawn_by_its_corners(tmp_path):
    """A quad8 is its four corners, not an eight-sided polygon through its
    midside nodes; a three-node beam is a line, not a triangle."""
    geometry = hertzforge.Geometry(
        np.arange(1, 12),
        [[0, 0, 0], [2, 0, 0], [2, 2, 0], [0, 2, 0], [1, 0, 0], [2, 1, 0],
         [1, 2, 0], [0, 1, 0], [5, 0, 0], [7, 0, 0], [6, 0, 0]])
    geometry.add_elements([list(range(1, 9)), [9, 10, 11]], [95, 23], [1, 2])
    _text, data = _payload(hertzforge.export_html(tmp_path / 'q.html',
                                                      geometry=geometry))
    [block] = data['blocks']
    assert [face['nodes'] for face in block['faces']] == [[0, 1, 2, 3]]
    assert [line[:2] for line in block['lines']] == [[8, 9]]


def test_nothing_to_draw_is_refused(tmp_path):
    with pytest.raises(ValueError, match='data to plot or a geometry'):
        hertzforge.export_html(tmp_path / 'x.html')


def test_a_report_page_does_not_fill(tmp_path):
    """The option is the export's; a report keeps its page."""
    from hertzforge.core.report import Report
    from hertzforge.report import render_html

    text = render_html(Report('R', []), {})
    data = json.loads(re.search(
        r'<script id="data" type="application/json">(.*?)</script>',
        text, re.DOTALL).group(1))
    assert data['fill'] is False and data['theme'] is None


def test_a_solids_insides_are_left_out_of_its_scene(tmp_path):
    """A block of bricks draws as its skin, so the nodes inside it are
    never drawn: the scene leaves them out with their share of each mode
    (2026-10-11 — the Frame-Wing's figure was 12.8 MB of them). Every
    node kept is on the skin, the faces point at the kept nodes, and a
    mode's motion is still each node's own."""
    from hertzforge import mesh
    from hertzforge.core.shapes import ShapeSet

    block = mesh.block((0, 0, 0), (2, 0, 0), (0, 2, 0), (0, 0, 2), 0.5, 'b', unit=None)
    assert block.num_nodes == 125
    xyz = block.node_xyz
    dofs = [f'{int(n)}Z+' for n in block.node_id]
    shapes = ShapeSet([10.0], [0.01], dofs, [xyz[:, 0] + 1.0])
    _text, data = _payload(hertzforge.export_html(tmp_path / 'b.html', geometry=block,
                                                      shapes=shapes))
    [scene] = data['blocks']
    points = np.array(scene['points'], dtype=float)
    assert len(points) == 125 - 27, 'the 3 x 3 x 3 inside left out'
    on_skin = np.any((points == points.min(axis=0)) | (points == points.max(axis=0)), axis=1)
    assert on_skin.all()
    used = {n for face in scene['faces'] for n in face['nodes']}
    assert used == set(range(len(points)))
    motion = np.array(scene['modes'][0]['real'], dtype=float)[:, 2]
    assert np.allclose(motion / motion.max(), (points[:, 0] + 1) / (points[:, 0].max() + 1))


def _workmanship():
    """The workmanship profile written at its four breakpoints (20 Hz
    0.01, 80 to 500 Hz 0.04, 2000 Hz 0.01 g²/Hz, stored in SI) with the
    default bands: a specification on its own, nothing measured."""
    from hertzforge.core.author import SpecificationDraft

    levels = [0.01, 0.04, 0.04, 0.01]
    draft = SpecificationDraft(['1Z+'], ['acceleration'],
                               [20.0, 80.0, 500.0, 2000.0],
                               [[g * 9.80665 ** 2 for g in levels]])
    return draft.make('Workmanship')


def test_a_specification_alone_is_drawn_on_the_grid_its_channels_are(
        tmp_path):
    """A specification with limits and nothing measured beside it drew
    an empty plot. Its target was filled in as a power law on a grid of
    its own (the curve's `x`, 800 points), while the channels the page
    draws from, the target and its zones, stayed on the four
    breakpoints; the page puts the picked channel's values on the first
    curve and draws them against that curve's grid, so four values were
    drawn at the first four of 800 frequencies, 20 to 20.3 Hz, and the
    zones with them. Every array the page draws for a channel now has
    the length of the grid it is drawn on, and spans the band."""
    from hertzforge.units import SYSTEMS

    _text, data = _payload(hertzforge.export_html(
        tmp_path / 'spec.html', _workmanship(),
        unit_system=SYSTEMS['in-slinch-lbf-s (g)']))
    [block] = data['blocks']
    target = block['curves'][0]
    drawn = target['x'] or block['x']
    [channel] = block['channels']
    assert len(channel['y']) == len(drawn)
    assert channel['zones']
    for zone in channel['zones']:
        for edge in (zone['lower'], zone['upper']):
            assert edge is None or len(edge) == len(drawn)
    written = [f for f, v in zip(drawn, channel['y']) if v is not None]
    assert min(written) == pytest.approx(20.0)
    assert max(written) == pytest.approx(2000.0)
    # the power law, not a straight line between breakpoints: at 1 kHz,
    # halfway down the falling segment in decades, -3 dB/oct from 0.04
    k = int(np.argmin([abs(f - 1000.0) for f in drawn]))
    expected = np.log10(0.04 * (drawn[k] / 500.0) ** (np.log10(0.25) / np.log10(4.0)))
    assert channel['y'][k] == pytest.approx(expected, abs=2e-3)


def _cornered():
    """A 2 x 2 x 2 block of 0.5 bricks, the eight bricks within 1 of the
    origin in an element group 'corner' and the rest in 'b'."""
    from hertzforge import mesh

    block = mesh.block((0, 0, 0), (2, 0, 0), (0, 2, 0), (0, 0, 2), 0.5, 'b', unit=None)
    corner = block.add_group('corner')
    xyz = dict(zip(block.node_id.tolist(), block.node_xyz))
    inside = [all(max(xyz[n]) <= 1 for n in conn) for conn in block.elem_conn]
    block.elem_group[np.asarray(inside)] = corner
    return block


def test_a_scene_opens_framed_on_element_groups_with_the_rest_drawn(tmp_path):
    """The page fits its view to the box around the groups' drawn nodes
    and still draws the whole model around them. The node at (1, 1, 1)
    is inside the block and never drawn; the box still reaches it,
    since the corner's skin does."""
    from hertzforge.core.shapes import ShapeSet

    block = _cornered()
    dofs = [f'{int(n)}Z+' for n in block.node_id]
    shapes = ShapeSet([10.0], [0.01], dofs, [block.node_xyz[:, 0] + 1.0])
    _text, framed = _payload(hertzforge.export_html(
        tmp_path / 'f.html', geometry=block, shapes=shapes, frame=['corner']))
    _text, whole = _payload(hertzforge.export_html(
        tmp_path / 'w.html', geometry=block, shapes=shapes))
    [scene], [plain] = framed['blocks'], whole['blocks']
    assert scene['frame'] == {'lo': [0.0, 0.0, 0.0], 'hi': [1.0, 1.0, 1.0]}
    assert 'frame' not in plain
    assert scene['points'] == plain['points'], 'the whole model is still drawn'
    assert scene['modes'] == plain['modes'], 'and swings as it did'


def test_a_frame_the_scene_cannot_have_is_refused(tmp_path):
    with pytest.raises(ValueError, match="no element group named 'wing'; it has 'b', 'corner'"):
        hertzforge.export_html(tmp_path / 'x.html', geometry=_cornered(), frame=['wing'])
    measured, _spec = _pair()
    with pytest.raises(ValueError, match='a plot has none'):
        hertzforge.export_html(tmp_path / 'x.html', measured, frame=['b'])


def test_node_dots_are_left_off_a_large_model_unless_asked_for(tmp_path):
    """On a finite element mesh the canvas's node squares tile the surface and read as
    speckle; a test geometry of a few hundred points keeps them."""
    from hertzforge import mesh
    from hertzforge.report import NODE_DOT_LIMIT

    def dots(geometry, **options):
        path = hertzforge.export_html(tmp_path / 'g.html', geometry=geometry, **options)
        [block] = _payload(path)[1]['blocks']
        return len(block['points']), block['node_dots']

    small = mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0), 0.1, unit=None)
    large = mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0), 0.025, unit=None)
    assert dots(small) == (121, True)
    count, drawn = dots(large)
    assert count > NODE_DOT_LIMIT and drawn is False
    assert dots(large, node_dots=True) == (count, True)
    assert dots(small, node_dots=False) == (121, False)
    with pytest.raises(ValueError, match="node_dots draws a scene's nodes, and a plot has none"):
        hertzforge.export_html(tmp_path / 'p.html', _pair()[0], node_dots=False)


def _test_on(plate, seats, ids):
    """A test geometry: nodes `ids` at the places of `plate`'s nodes `seats`."""
    from hertzforge import Geometry

    rows = [int(np.flatnonzero(plate.node_id == seat)[0]) for seat in seats]
    return Geometry(node_id=np.asarray(ids, dtype=np.int64),
                    node_xyz=np.asarray(plate.node_xyz, dtype=float)[rows],
                    node_def_cs=np.ones(len(ids), dtype=np.int64),
                    node_disp_cs=np.ones(len(ids), dtype=np.int64), cs_id=np.array([1]),
                    cs_name=[''], cs_type=np.zeros(1, dtype=np.int64),
                    cs_matrix=np.array([np.vstack([np.eye(3), np.zeros(3)])]),
                    length_unit=plate.length_unit)


def test_a_tests_dofs_are_drawn_where_they_were_measured(tmp_path):
    """Node 1 of a test is some other node of a model: given the geometry the data was
    measured on, each arrow is drawn at the model's node nearest the sensor; without it, a DOF
    the model has no node for is refused rather than left out."""
    from hertzforge import mesh
    from hertzforge.core.data import TimeHistory

    plate = mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0), 0.1, unit=None)
    seats = [int(plate.node_id[-1]), int(plate.node_id[60])]
    test = _test_on(plate, seats, [1, 2])
    history = TimeHistory(np.arange(4) / 4.0, np.ones((2, 4)), ['1Z+', '2X+'],
                          ordinate_dim='acceleration')
    path = hertzforge.export_html(tmp_path / 'd.html', geometry=plate,
                                  dofs=('acceleration', history, test))
    [block] = _payload(path)[1]['blocks']
    points = np.asarray(block['points'])
    xyz = np.asarray(plate.node_xyz)
    drawn = [points[arrow['node']] for arrow in block['arrows']]
    assert drawn == [pytest.approx(xyz[np.flatnonzero(plate.node_id == seat)[0]])
                     for seat in seats]
    assert [arrow['label'] for arrow in block['arrows']] == ['1Z+', '2X+']
    # by id, node 1 of the plate is its corner, not where the sensor was
    corner = hertzforge.export_html(tmp_path / 'c.html', geometry=plate,
                                    dofs=('acceleration', history))
    [by_id] = _payload(corner)[1]['blocks']
    assert np.asarray(by_id['points'])[by_id['arrows'][0]['node']] == pytest.approx(xyz[0])
    far = TimeHistory(np.arange(4) / 4.0, np.ones((2, 4)), ['1Z+', '9999X+'],
                      ordinate_dim='acceleration')
    with pytest.raises(ValueError, match=r'1 DOF has no node or direction on the geometry: '
                                         r'9999X\+'):
        hertzforge.export_html(tmp_path / 'f.html', geometry=plate, dofs=('acceleration', far))
    with pytest.raises(ValueError, match=r'1 DOF has no node or direction on the geometry '
                                         r'they were measured on: 9999X\+'):
        hertzforge.export_html(tmp_path / 'f.html', geometry=plate,
                               dofs=('acceleration', far, test))
    with pytest.raises(ValueError, match=r'dofs is \(quantity, data\) or'):
        hertzforge.export_html(tmp_path / 'f.html', geometry=plate,
                               dofs=('acceleration', history, test, test))


@pytest.mark.slow
def test_the_page_draws_no_node_squares_when_told_not_to(tmp_path):
    """Measured on the drawn canvas, in headless Chrome: the squares in the nodes' own color
    are there with node_dots and gone without, while the plate itself is drawn both times."""
    import time

    from hertzforge import mesh
    from hertzforge.drawing.viz.geometry import PALETTE
    from hertzforge.record import RecordError, _Chrome, find_chrome

    try:
        chrome = find_chrome()
    except RecordError as error:
        pytest.skip(str(error))
    plate = mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0), 0.1, unit=None)
    plate.node_color[:] = 4
    red = [round(255 * c) for c in PALETTE[4]]
    count = f"""(() => {{
        const c = document.querySelector('canvas');
        if (!c || !c.width) return null;
        const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data;
        let node = 0, drawn = 0;
        for (let i = 0; i < d.length; i += 4) {{
            if (d[i + 3]) drawn++;
            if (d[i] === {red[0]} && d[i + 1] === {red[1]} && d[i + 2] === {red[2]}) node++;
        }}
        return [node, drawn];
    }})()"""
    seen = {}
    for dots in (True, False):
        path = tmp_path / f'dots-{dots}.html'
        hertzforge.export_html(path, geometry=plate, node_dots=dots, theme='light')
        browser = _Chrome(chrome, 800, 600)
        try:
            browser.call('Page.enable')
            browser.call('Runtime.enable')
            browser.call('Page.navigate', {'url': path.as_uri()})
            deadline = time.monotonic() + 30
            while not (found := browser.evaluate(count)) or not found[1]:
                assert time.monotonic() < deadline, f'the page drew nothing: {browser.errors}'
                time.sleep(0.1)
            seen[dots] = found
        finally:
            browser.close()
    assert seen[True][0] > 100, '121 nodes, a 4 by 4 square each, some behind others'
    assert seen[False][0] == 0 and seen[False][1] > 10_000
