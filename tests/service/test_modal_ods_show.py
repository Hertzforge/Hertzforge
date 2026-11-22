"""Modal FRFs, a channel table with a force reference and a geometry selected together
show the operating deflection shape of a force at that reference (`show._driven`)."""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.shapes import ShapeSet
from hertzforge.deform import modal_ods_deflection
from hertzforge.demo import plate
from hertzforge.service import projects
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views


def _table(nodes=('101',), roles=('reference',), kinds=('force',)):
    count = len(nodes)
    return hertzforge.ChannelTable({
        'channel': list(range(1, count + 1)), 'node': list(nodes),
        'direction': ['Z+'] * count, 'role': list(roles),
        'unit': ['N' if kind == 'force' else 'g' for kind in kinds],
        'channel_type': list(kinds)})


@pytest.fixture(scope='module')
def state():
    """A free plate with its first twenty modes, six of them rigid, its modal FRFs in
    displacement, and three channel tables: one force, two forces, no force."""
    built = plate.build(4)
    solved = built.eigensolution(num_modes=20)
    shapes = ShapeSet(solved.frequency, np.full(solved.num_shapes, 0.02), solved.coordinate,
                      solved.shape_matrix, modal_mass=solved.modal_mass, mass_unit='kg')
    held = projects.new({'title': 'Plate'})
    project = held.project
    project.add('Plate', built.geometry())
    project.add('Modes', shapes)
    project.link('Plate', 'Modes')
    project.synthesize_modal_frfs('Modes', sample_rate=4096.0, frame_length=64,
                                  quantity='displacement', name='Modal FRFs')
    project.add('Force', _table())
    project.add('Two', _table(('101', '505'), ('reference',) * 2, ('force',) * 2))
    project.add('Quiet', _table(roles=('response',), kinds=('acceleration',)))
    project.add('Other Modes', ShapeSet(shapes.frequency, shapes.damping, shapes.coordinate,
                                        shapes.shape_matrix))
    return held


def shown(state, *objects, records=None, prefs=None, validate=None):
    out = projects.show(state, {'objects': list(objects), 'records': records or {},
                                'part': None, **({'prefs': prefs} if prefs else {})})
    if validate:
        wire_views(out['views'], validate)
        validate({**out, 'views': []}, 'shown')
    return out


def _elastic(state):
    frequency = state.project['Modes'].frequency
    return [k for k in range(len(frequency)) if frequency[k] > 1.0]


def test_modal_frfs_a_force_and_a_geometry_show_the_response_to_that_force(state, validate):
    out = shown(state, 'Modal FRFs', 'Force', 'Plate', validate=validate)
    stage, *curves = out['views']
    assert stage['view'] == 'shape3d' and stage['cursor'] == 'frequency'
    assert all(curve['view'] == 'spectrum' and curve['link'] == 'frequency' for curve in curves)
    assert 'the response to a force at 101Z+: 20 mode shapes of Modes' in out['note']
    # the one arrow is the force, pointing in at its node
    arrows = stage['arrows']
    assert arrows['labels'] == ['101Z+'] and list(arrows['inward']) == [1]
    assert list(arrows['axes']) == [2]
    # nothing else is offered to be marked, and one force leaves no choice of drive
    assert out['offers']['drives'] == ['101Z+']
    assert not out['offers']['arrows'] and out['offers']['arrow_roles'] == []
    # what is drawn is the deflection the project's own calls make, at its strongest line
    project = state.project
    basis, driven = project.driven_modes('Modal FRFs', table='Force')
    deflection = modal_ods_deflection(project['Plate'], project[basis], driven)
    deflection.line = deflection.strongest_line
    frequency = float(np.asarray(project['Modal FRFs'].abscissa)[deflection.line])
    mode = stage['modes'][0]
    assert mode['frequency_hz'] == pytest.approx(frequency)
    assert mode['label'] == f'ODS at {frequency:.4g} Hz'
    drawn = np.asarray(mode['shape']) + 1j * np.asarray(mode.get('shape_imag', 0.0))
    at_zero = deflection.offsets(0.0).copy()
    expected = at_zero - 1j * deflection.offsets(np.pi / 2)
    assert np.abs(drawn.real).max() > np.abs(drawn.imag).max(), 'fullest at phase zero'
    assert np.abs(drawn).max() == pytest.approx(np.abs(expected).max())
    assert np.abs(drawn).max() == pytest.approx(1.0, abs=1e-6), 'each line at full scale'


def test_the_elastic_records_show_the_static_shape_at_zero_hertz(state, validate):
    elastic = _elastic(state)
    whole = shown(state, 'Modal FRFs', 'Force', 'Plate', prefs={'line': 0.0}, validate=validate)
    label = whole['views'][0]['modes'][0]['label']
    assert label == 'ODS at 0 Hz: no shape, a mode is unbounded here'
    assert not np.any(whole['views'][0]['modes'][0]['shape'])
    assert 'no shape at 0 Hz' in whole['note']
    out = shown(state, 'Modal FRFs', 'Force', 'Plate', records={'Modal FRFs': elastic},
                prefs={'line': 0.0}, validate=validate)
    mode = out['views'][0]['modes'][0]
    assert mode['label'] == 'ODS at 0 Hz'
    assert np.abs(np.asarray(mode['shape'])).max() == pytest.approx(1.0, abs=1e-6)
    assert f'{len(elastic)} mode shapes of Modes' in out['note']
    # only the modes kept are plotted beside it
    assert sum(len(curve['series']) for curve in out['views'][1:]) == len(elastic)


def test_several_force_references_offer_the_drive(state, validate):
    first = shown(state, 'Modal FRFs', 'Two', 'Plate', validate=validate)
    assert first['offers']['drives'] == ['101Z+', '505Z+']
    assert first['views'][0]['arrows']['labels'] == ['101Z+']
    other = shown(state, 'Modal FRFs', 'Two', 'Plate', prefs={'drive': '505Z+'},
                  validate=validate)
    assert other['views'][0]['arrows']['labels'] == ['505Z+']
    assert 'a force at 505Z+' in other['note']
    assert not np.allclose(first['views'][0]['modes'][0]['shape'],
                           other['views'][0]['modes'][0]['shape'])
    # a drive the table does not hold is the first
    lost = shown(state, 'Modal FRFs', 'Two', 'Plate', prefs={'drive': '999Z+'})
    assert lost['views'][0]['arrows']['labels'] == ['101Z+']
    with pytest.raises(ProtocolError, match='"drive"'):
        shown(state, 'Modal FRFs', 'Two', 'Plate', prefs={'drive': 5})


def test_without_a_force_it_says_what_is_missing(state):
    alone = shown(state, 'Modal FRFs', 'Plate')
    assert 'select a channel table with a force reference too' in alone['note']
    assert alone['offers']['drives'] == []
    quiet = shown(state, 'Modal FRFs', 'Quiet', 'Plate')
    assert 'Quiet has no reference channel measuring force' in quiet['note']
    assert not any(view['view'] == 'shape3d' and view.get('modes') for view in quiet['views'])


def test_a_shape_set_selected_with_them_has_to_be_theirs(state, validate):
    theirs = shown(state, 'Modal FRFs', 'Force', 'Plate', 'Modes', validate=validate)
    assert theirs['offers']['drives'] == ['101Z+']
    assert 'the response to a force at 101Z+' in theirs['note']
    other = shown(state, 'Modal FRFs', 'Force', 'Plate', 'Other Modes')
    assert other['offers']['drives'] == []
    assert 'the response to a force' not in other['note']


def _drawn_row(stage, xyz):
    """The row of the stage's nodes at a place."""
    nodes = np.asarray(stage['nodes'], dtype=float).reshape(-1, 3)
    distance = np.linalg.norm(nodes - np.asarray(xyz, dtype=float), axis=1)
    assert distance.min() < 1e-6
    return int(np.argmin(distance))


def test_the_force_arrow_rides_the_node_it_pushes_on(state, validate):
    """The arrow ends on the drive's node and is told to follow that row of the nodes
    drawn, so the shell carries it and its label through the sweep."""
    out = shown(state, 'Modal FRFs', 'Two', 'Plate', prefs={'drive': '505Z+'},
                validate=validate)
    stage = out['views'][0]
    plate_geometry = state.project['Plate']
    place = np.asarray(plate_geometry.node_xyz)[list(plate_geometry.node_id).index(505)]
    arrows = stage['arrows']
    assert arrows['follow'].dtype == np.uint32
    assert list(arrows['follow']) == [_drawn_row(stage, place)]
    head = np.asarray(arrows['tails'])[0] + arrows['length'] * np.asarray(arrows['directions'])[0]
    assert head == pytest.approx(place, abs=1e-6)
    # that node moves, so an arrow left where it was would come off it
    moved = np.asarray(stage['modes'][0]['shape']).reshape(-1, 3)[arrows['follow'][0]]
    assert np.linalg.norm(moved) > 0.0


def test_only_the_force_arrow_follows(state):
    """A channel table's DOF arrows on a geometry stay put, as they did."""
    out = shown(state, 'Two', 'Plate')
    arrows = next(view for view in out['views'] if view['view'] == 'shape3d')['arrows']
    assert 'follow' not in arrows


def test_the_force_arrow_is_placed_from_the_tables_own_geometry(state, validate):
    """A table linked to a sparse test geometry, over the model drawn: the drive is a node
    of the test geometry, so the arrow goes at the drawn node nearest that node's place and
    points along that node's coordinate system, whatever the model numbers its nodes."""
    project = state.project
    plate_geometry = project['Plate']
    ids = list(plate_geometry.node_id)
    xyz = np.asarray(plate_geometry.node_xyz, dtype=float)
    # the test geometry's node 101 sits where the model's 505 is, in a system turned about Z
    sparse = hertzforge.Geometry(
        node_id=[101, 7], node_xyz=xyz[[ids.index(505), ids.index(303)]],
        node_disp_cs=[5, 0], cs_id=[5], cs_type=[0], cs_name=['turned'],
        cs_matrix=[[[0, 1, 0], [-1, 0, 0], [0, 0, 1], [0, 0, 0]]])
    if plate_geometry.units_defined:
        sparse.define_units('m')
    project.add('Sparse', sparse)
    project.add('Linked', _table(('101', '7'), ('reference',) * 2, ('force',) * 2))
    project.link('Sparse', 'Linked')
    try:
        for drive, node in (('101Z+', 505), ('7Z+', 303)):
            out = shown(state, 'Modal FRFs', 'Linked', 'Plate', prefs={'drive': drive},
                        validate=validate)
            stage = out['views'][0]
            arrows = stage['arrows']
            assert arrows['labels'] == [drive]
            place = xyz[ids.index(node)]
            assert list(arrows['follow']) == [_drawn_row(stage, place)]
            head = (np.asarray(arrows['tails'])[0]
                    + arrows['length'] * np.asarray(arrows['directions'])[0])
            assert head == pytest.approx(place, abs=1e-6)
            assert 'the force is not drawn' not in out['note']
        # the same shape as the force put straight on the model's own node
        direct = shown(state, 'Modal FRFs', 'Two', 'Plate', prefs={'drive': '505Z+'})
        linked = shown(state, 'Modal FRFs', 'Linked', 'Plate', prefs={'drive': '101Z+'})
        assert np.allclose(direct['views'][0]['modes'][0]['shape'],
                           linked['views'][0]['modes'][0]['shape'], atol=1e-6)
    finally:
        project.remove('Linked')
        project.remove('Sparse')


def test_the_arrow_points_along_the_tables_nodes_own_system(state):
    """The direction is the table's node's, read in its displacement system."""
    project = state.project
    plate_geometry = project['Plate']
    ids = list(plate_geometry.node_id)
    xyz = np.asarray(plate_geometry.node_xyz, dtype=float)
    # local X is the model's Z
    sparse = hertzforge.Geometry(
        node_id=[9], node_xyz=xyz[[ids.index(505)]], node_disp_cs=[5], cs_id=[5],
        cs_type=[0], cs_name=['on end'],
        cs_matrix=[[[0, 0, 1], [0, 1, 0], [-1, 0, 0], [0, 0, 0]]])
    if plate_geometry.units_defined:
        sparse.define_units('m')
    table = hertzforge.ChannelTable({
        'channel': [1], 'node': ['9'], 'direction': ['X+'], 'role': ['reference'],
        'unit': ['N'], 'channel_type': ['force']})
    project.add('On End', sparse)
    project.add('Pushed', table)
    project.link('On End', 'Pushed')
    try:
        out = shown(state, 'Modal FRFs', 'Pushed', 'Plate')
        arrows = out['views'][0]['arrows']
        assert arrows['labels'] == ['9X+'] and list(arrows['axes']) == [0]
        assert np.asarray(arrows['directions'])[0] == pytest.approx([0, 0, 1], abs=1e-6)
    finally:
        project.remove('Pushed')
        project.remove('On End')


def test_the_line_asked_for_is_kept_when_the_force_is_moved(state):
    """Comparing drive points is one frequency asked of each: with the line given, both
    forces are read there; with none, each starts at its own strongest line."""
    abscissa = np.asarray(state.project['Modal FRFs'].abscissa, dtype=float)
    first = shown(state, 'Modal FRFs', 'Two', 'Plate')
    held = first['views'][0]['modes'][0]['frequency_hz']
    line = float(abscissa[5])
    assert held != line
    for drive in ('505Z+', '101Z+'):
        out = shown(state, 'Modal FRFs', 'Two', 'Plate', prefs={'drive': drive, 'line': line})
        assert out['views'][0]['modes'][0]['frequency_hz'] == pytest.approx(line)
