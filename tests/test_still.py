"""A still picture of a geometry with DOFs as arrows (`hertzforge still`): the view it is drawn
from, the DOFs placed by position when they were measured on another geometry, the refusals,
the command line, and the figure `export_html` draws. One test draws a real picture when Chrome
is on the machine."""

from __future__ import annotations

import struct
from pathlib import Path

import numpy as np
import pytest

import hertzforge as hz
from hertzforge import Geometry
from hertzforge.__main__ import main
from hertzforge.core.data import TimeHistory
from hertzforge.drawing.viz.geometry import (
    dof_arrow_length,
    dof_arrow_solids,
    dof_arrow_vectors,
    dof_arrows_on,
)
from hertzforge.record import RecordError, find_chrome
from hertzforge.report.viewer import read_viewer
from hertzforge.still import (
    LABEL_LIMIT,
    Still,
    StillError,
    still_page,
    still_view,
    write_still,
)

PLATE = Path(__file__).resolve().parent.parent / 'testdata' / 'plate'

#: the plate nodes a test's nodes 101 to 104 sit on: other nodes than the plate's own 101 to 104
SEATS = (707, 709, 1105, 403)


def _test_geometry(model: Geometry, seats=SEATS, ids=(101, 102, 103, 104),
                   offset=(0.0, 0.0, 0.0)) -> Geometry:
    """A test geometry of nodes `ids` at the places of the model's nodes `seats`, moved by
    `offset`. The last displaces in a system turned 90 degrees about Z, so its X is global Y."""
    rows = [int(np.flatnonzero(model.node_id == seat)[0]) for seat in seats]
    xyz = np.asarray(model.node_xyz, dtype=float)[rows] + np.asarray(offset)
    turned = np.array([[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], xyz[-1]])
    return Geometry(
        node_id=np.asarray(ids, dtype=np.int64), node_xyz=xyz,
        node_def_cs=np.ones(len(ids), dtype=np.int64),
        node_disp_cs=np.array([1] * (len(ids) - 1) + [2], dtype=np.int64),
        cs_id=np.array([1, 2]), cs_name=['', 'turned'], cs_type=np.zeros(2, dtype=np.int64),
        cs_matrix=np.array([np.vstack([np.eye(3), np.zeros(3)]), turned]),
        length_unit=model.length_unit)


def _measured(dofs=('101X+', '102Y+', '103Z+', '104X+'), forces=('104X+',)) -> TimeHistory:
    time = np.arange(8) / 8.0
    return TimeHistory(time, np.ones((len(dofs), len(time))), list(dofs),
                       ordinate_dim=['force' if dof in forces else 'acceleration'
                                     for dof in dofs])


@pytest.fixture
def plate():
    """The plate model, a test geometry numbered as a test is (101 to 104, which are other
    nodes of the plate), and a time history measured on it, linked with it."""
    project = hz.Project('plate')
    project.import_file(str(PLATE / 'geometry.unv'))
    project.add('Test Geometry', _test_geometry(project['Geometry']))
    project.add('Kick', _measured())
    project.link('Test Geometry', 'Kick')
    return project


def _row(geometry: Geometry, node: int) -> int:
    return int(np.flatnonzero(geometry.node_id == node)[0])


def test_dofs_measured_on_another_geometry_are_placed_by_position():
    """A test's node 101 is some other node of a model: matched by id the arrows land there
    with no word; matched by position they land where the sensors are."""
    project = hz.Project('plate')
    project.import_file(str(PLATE / 'geometry.unv'))
    model = project['Geometry']
    test = _test_geometry(model)
    dofs = ['101X+', '102Y+', '103Z+', '104X+']
    placed = dof_arrows_on(model, dofs, test)
    assert [row for row, _v, _a in placed] == [_row(model, seat) for seat in SEATS]
    by_id = [hit[0] for hit in dof_arrow_vectors(model, dofs)]
    assert by_id == [_row(model, n) for n in (101, 102, 103, 104)]
    assert set(by_id).isdisjoint(row for row, _v, _a in placed)
    # the direction is the test node's own system's, turned into the global frame
    assert placed[3][1] == pytest.approx([0.0, 1.0, 0.0])
    assert placed[3][2] == 0, "the color is the DOF's own letter, X"
    assert placed[1][1] == pytest.approx([0.0, 1.0, 0.0]) and placed[1][2] == 1
    # on its own geometry, or with none named, the ids are read as they are
    assert [hit[0] for hit in dof_arrows_on(test, dofs, test)] == [0, 1, 2, 3]


def test_a_dof_with_no_place_on_the_geometry_is_refused_by_name():
    project = hz.Project('plate')
    project.import_file(str(PLATE / 'geometry.unv'))
    model = project['Geometry']
    # one sensor a whole plate width off the model
    off = _test_geometry(model, seats=(707, 709), ids=(101, 102))
    off.node_xyz[1] += np.array([0.0, 0.0, 0.3048])
    from hertzforge.core.correlate import match_distance

    # DOF Mapping's own limit: 2 % of the test geometry's extent (half the span of its two
    # nodes here), plus half the thickest plate of a model with units
    limit = match_distance(model, off)
    assert limit == pytest.approx(0.02 * np.linalg.norm(off.node_xyz[1] - off.node_xyz[0]) / 2)
    with pytest.raises(ValueError, match=rf'1 measured node \(102\) has no node of the geometry '
                                         rf'within {limit:.4g} in model units \(the furthest '
                                         r'is 0.3048 in model units away\)'):
        dof_arrows_on(model, ['101X+', '102Z+'], off)
    # a distance given in place of the match distance
    assert len(dof_arrows_on(model, ['101X+', '102Z+'], off, distance=0.31)) == 2
    with pytest.raises(ValueError, match='above zero'):
        dof_arrows_on(model, ['101X+'], off, distance=0.0)
    with pytest.raises(ValueError, match=r'2 DOFs have no node or direction on the geometry '
                                         r'they were measured on: 105X\+, 101Q\+'):
        dof_arrows_on(model, ['101X+', '105X+', '101Q+'], off)
    with pytest.raises(ValueError, match=r'1 DOF has no node or direction on the geometry: 9999Z\+'):
        dof_arrows_on(model, ['101X+', '9999Z+'])
    meters = _test_geometry(model)
    meters.length_unit = 'm'
    assert meters.units_defined and not model.units_defined
    with pytest.raises(ValueError, match='need units defined'):
        dof_arrows_on(model, ['101X+'], meters)


def test_the_still_draws_no_node_markers(plate):
    """The app's 3-D view draws a node no element passes through as a dot; on a finite element
    model those are masses and spider centers, specks on a slide, so the still leaves them off
    and draws the surface and mesh lines alone."""
    from hertzforge.service import show

    model = plate['Geometry']
    model.add_nodes([[0.15, 0.15, 0.05]])
    shown = show.show(plate, ['Geometry'], {}, None, {**show.PREFS, 'sensors': False})
    app = next(v for v in shown['views'] if v.get('view') == 'shape3d')
    assert sum(len(layer['xyz']) for layer in app['points']) == 1, 'the app draws the free node'
    view = still_view(plate, Still('Geometry', dofs=['Kick']))
    assert 'points' not in view and 'bodies' not in view
    assert len(view['triangles']) and len(view['lines'])


def test_the_still_marks_each_dof_where_it_was_measured(plate):
    model = plate['Geometry']
    view = still_view(plate, Still('Geometry', dofs=['Kick'], opacity=0.4, view='top'))
    arrows = view['arrows']
    assert arrows['labels'] == ['101X+', '102Y+', '103Z+', '104X+']
    assert arrows['inward'].tolist() == [0, 0, 0, 1], 'the force points in'
    xyz = np.asarray(model.node_xyz, dtype=float)
    seats = xyz[[_row(model, seat) for seat in SEATS]]
    length = arrows['lengths'][3]
    assert 0 < length
    assert arrows['tails'][:3] == pytest.approx(seats[:3].astype(np.float32))
    # the force ends on its node: it starts its own length back along global Y
    assert arrows['tails'][3] == pytest.approx(seats[3] - length * np.array([0, 1, 0]), abs=1e-6)
    assert arrows['axes'].tolist() == [0, 1, 2, 0]
    assert view['opacity'] == 0.4 and view['home']['eye'] == [0.0, 0.0, 1.0]


def test_the_still_refuses_what_it_cannot_place(plate):
    plate.add('Loose', _measured())
    with pytest.raises(StillError, match='Loose is linked with no geometry, so its node ids '
                                         'name no place on Geometry'):
        still_view(plate, Still('Geometry', dofs=['Loose']))
    # named, the geometry its ids belong to places it
    assert len(still_view(plate, Still('Geometry', dofs=['Loose'], on='Test Geometry'))
               ['arrows']['labels']) == 4
    with pytest.raises(StillError, match='Far on Geometry: 1 DOF has no node or direction on '
                                         r'the geometry: 9999Z\+'):
        plate.add('Far', _measured(dofs=('101X+', '9999Z+'), forces=()))
        still_view(plate, Still('Geometry', dofs=['Far'], on='Geometry'))
    with pytest.raises(StillError, match='Test Geometry is not data or a channel table'):
        still_view(plate, Still('Geometry', dofs=['Test Geometry'], on='Test Geometry'))
    with pytest.raises(StillError, match="no object named 'Nothing'"):
        still_view(plate, Still('Geometry', dofs=['Nothing']))
    with pytest.raises(StillError, match='Kick is not a geometry'):
        still_view(plate, Still('Kick'))
    with pytest.raises(StillError, match='at least 16 by 16'):
        still_page(plate, Still('Geometry', width=8))
    with pytest.raises(RecordError, match='light or dark'):
        still_page(plate, Still('Geometry', theme='sepia'))


def test_a_channel_table_marks_its_channels(plate):
    from hertzforge.core.channel_table import ChannelTable

    plate.add('Channels', ChannelTable({
        'channel': [1, 2, 3], 'node': ['101', '104', ''], 'direction': ['X+', 'X+', ''],
        'unit': ['g', 'lbf', 'V'], 'channel_type': ['acceleration', 'force', 'voltage']}))
    plate.link('Test Geometry', 'Channels')
    arrows = still_view(plate, Still('Geometry', dofs=['Channels']))['arrows']
    # a channel with no node or direction marks nothing
    assert arrows['labels'] == ['101X+', '104X+'] and arrows['inward'].tolist() == [0, 1]


def test_the_command_writes_or_says_why_not(plate, tmp_path, capsys):
    plate.add('Loose', _measured())
    path = tmp_path / 'plate.hforge'
    plate.save(path)
    out = tmp_path / 'still.png'
    assert main(['still', str(path), '-o', str(out), '--geometry', 'Geometry',
                 '--dofs', 'Loose']) == 1
    assert 'hertzforge still: Loose is linked with no geometry' in capsys.readouterr().out
    assert main(['still', str(path), '-o', str(out), '--geometry', 'Geometry',
                 '--size', 'big']) == 1
    assert 'WIDTHxHEIGHT' in capsys.readouterr().out
    assert main(['still', str(path), '-o', str(out), '--geometry', 'Geometry',
                 '--eye', 'up']) == 1
    assert 'three numbers' in capsys.readouterr().out
    assert not out.exists()


def _png_size(path: Path) -> tuple[int, int]:
    data = path.read_bytes()
    assert data[:8] == b'\x89PNG\r\n\x1a\n'
    return struct.unpack('>II', data[16:24])


@pytest.mark.slow
def test_a_still_is_drawn_to_a_png_with_no_window(plate, tmp_path):
    try:
        find_chrome()
    except RecordError as error:
        pytest.skip(str(error))
    out = write_still(plate, Still('Geometry', dofs=['Kick'], width=320, height=200,
                                   theme='light'), tmp_path / 'still.png')
    assert _png_size(out) == (320, 200)


def test_each_objects_forces_and_responses_are_sized_to_their_own_spacing(plate):
    """One length for every arrow held the RC plate's corner forces, 0.7 in. apart, to what its
    triaxes 0.5 in. apart allowed. Each set (an object's DOFs pointing in, or out) now takes the
    longest its own spacing allows."""
    model = plate['Geometry']
    # two responses a node apart, one force alone
    plate.add('Close', _test_geometry(model, seats=(707, 708, 1313), ids=(1, 2, 3)))
    plate.add('Hits', _measured(dofs=('1Z+', '2Z+', '3X+'), forces=('3X+',)))
    plate.link('Close', 'Hits')
    arrows = still_view(plate, Still('Geometry', dofs=['Hits', 'Kick']))['arrows']
    xyz = np.asarray(model.node_xyz, dtype=float)
    from hertzforge.service.ops_geometry import _extent

    extent = _extent(xyz)
    out = dof_arrow_length(xyz[[_row(model, 707), _row(model, 708)]], extent)
    alone = dof_arrow_length(xyz[[_row(model, 1313)]], extent)
    kick_out = dof_arrow_length(xyz[[_row(model, seat) for seat in SEATS[:3]]], extent)
    assert out == pytest.approx(0.9 * 0.0254), 'the pair: nine tenths of their spacing'
    assert alone == pytest.approx(0.12 * extent), 'the lone force: 12 % of the model'
    assert arrows['labels'] == ['1Z+', '2Z+', '3X+', '101X+', '102Y+', '103Z+', '104X+']
    assert arrows['lengths'].tolist() == pytest.approx(
        [out, out, alone, kick_out, kick_out, kick_out, alone])
    assert arrows['length'] == pytest.approx(alone), 'the longest'


def test_one_length_each_is_held_to_one_per_arrow():
    xyz = np.zeros((2, 3))
    hits = [(False, (0, np.array([1.0, 0, 0]), 0)), (True, (1, np.array([0, 0, 1.0]), 2))]
    one = dof_arrow_solids(xyz, hits, 0.5, ['a', 'b'])
    assert one['length'] == 0.5 and 'lengths' not in one
    each = dof_arrow_solids(xyz, hits, [0.5, 2.0], ['a', 'b'])
    assert each['lengths'].tolist() == [0.5, 2.0] and each['length'] == 2.0
    assert each['tails'][1].tolist() == [0.0, 0.0, -2.0], 'a force starts its own length back'
    with pytest.raises(ValueError, match='3 arrow lengths for 2 arrows'):
        dof_arrow_solids(xyz, hits, [0.5, 2.0, 1.0], ['a', 'b'])


def test_labels_are_written_for_a_few_arrows_unless_told(plate):
    def labeled(**options):
        return read_viewer(still_page(plate, Still('Geometry', **options)))['record']['setup']['labels']

    assert labeled(dofs=['Kick']) is True, 'four arrows'
    assert labeled(dofs=['Kick'], labels=False) is False
    assert labeled() is False, 'no arrows, nothing to write'
    many = [f'{n}Z+' for n in plate['Geometry'].node_id[:LABEL_LIMIT + 1]]
    plate.add('Many', _measured(dofs=many, forces=()))
    assert labeled(dofs=['Many'], on='Geometry') is False
    assert labeled(dofs=['Many'], on='Geometry', labels=True) is True
    with pytest.raises(StillError, match='labels name the DOF arrows, and the picture has none'):
        still_page(plate, Still('Geometry', labels=True))


def _frame_counts(project, still, count_js):
    """`count_js`, a JavaScript function of the frame's pixels (an ImageData), run on the still's
    one frame in headless Chrome."""
    import tempfile

    from hertzforge.record import _Chrome, opened

    try:
        chrome = find_chrome()
    except RecordError as error:
        pytest.skip(str(error))
    with tempfile.TemporaryDirectory() as folder:
        path = Path(folder) / 'still.html'
        path.write_text(still_page(project, still), encoding='utf-8')
        browser = _Chrome(chrome, still.width, still.height)
        try:
            opened(browser, path)
            result = browser.call('Runtime.evaluate', {
                'expression': f"""(async () => {{
                    const image = new Image();
                    image.src = window.__hertzforgeRecorder.frame(0);
                    await image.decode();
                    const c = document.createElement('canvas');
                    c.width = image.width; c.height = image.height;
                    const g = c.getContext('2d');
                    g.drawImage(image, 0, 0);
                    const text = getComputedStyle(document.documentElement).getPropertyValue('--text-1');
                    g.fillStyle = text; g.fillRect(0, 0, 1, 1);
                    const ink = Array.from(g.getImageData(0, 0, 1, 1).data.slice(0, 3));
                    g.drawImage(image, 0, 0);
                    return ({count_js})(g.getImageData(0, 0, c.width, c.height), ink);
                }})()""", 'awaitPromise': True, 'returnByValue': True})
        finally:
            browser.close()
    return result['result']['value']


@pytest.mark.slow
def test_the_labels_are_drawn_into_the_picture(plate):
    """Counted on the frame itself: pixels in the labels' own color, there with labels and gone
    without, since the pane writes them over its canvas and not in it."""
    ink = """(image, ink) => { let n = 0; const d = image.data;
        for (let i = 0; i < d.length; i += 4)
            if (d[i] === ink[0] && d[i + 1] === ink[1] && d[i + 2] === ink[2]) n++;
        return n; }"""
    shown = _frame_counts(plate, Still('Geometry', dofs=['Kick'], width=640, height=400,
                                       theme='light', labels=True), ink)
    hidden = _frame_counts(plate, Still('Geometry', dofs=['Kick'], width=640, height=400,
                                        theme='light', labels=False), ink)
    assert hidden < 20 and shown > hidden + 100, (shown, hidden)


@pytest.mark.slow
def test_a_see_through_picture_keeps_the_element_colors(plate):
    """The RC drawn see-through to show the forces behind the plate turned the whole model gray,
    so its parts could not be told apart."""
    plate['Geometry'].elem_color[:] = 3
    green = """(image) => { let n = 0; const d = image.data;
        for (let i = 0; i < d.length; i += 4)
            if (d[i + 1] > d[i] + 40 && d[i + 1] > d[i + 2] + 40) n++;
        return n; }"""
    solid = _frame_counts(plate, Still('Geometry', width=640, height=400, theme='light'), green)
    through = _frame_counts(plate, Still('Geometry', width=640, height=400, theme='light',
                                         opacity=0.4), green)
    assert solid > 10_000 and through > 10_000, (solid, through)



def test_a_view_of_arrows_of_their_own_lengths_is_one_the_protocol_allows(plate):
    import json

    import jsonschema

    from hertzforge.service import frames

    schema = json.loads((Path(__file__).resolve().parent.parent / 'schema'
                         / 'protocol.schema.json').read_text(encoding='utf-8'))
    check = jsonschema.Draft202012Validator(
        {'$schema': schema['$schema'], '$defs': schema['$defs'], '$ref': '#/$defs/views'})
    view = still_view(plate, Still('Geometry', dofs=['Kick']))
    assert len(view['arrows']['lengths']) == 4
    wire, _arrays = frames.to_wire({'views': [view]})
    check.validate(wire['views'])
    wire['views'][0]['arrows']['lengths'] = 0.5
    with pytest.raises(jsonschema.ValidationError):
        check.validate(wire['views'])
