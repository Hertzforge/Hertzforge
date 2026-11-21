"""Two shape sets on two geometries are drawn each on its own, in one scene: a model's
modes on the model and a test's on the test geometry laid over it (`mode_pairs._stage`)."""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
import pytest

from hertzforge.service import projects
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


@pytest.fixture(scope='module')
def state():
    from test_correlate import _fem, _test_model

    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    held = projects.new({'title': 'Two'})
    project = held.project
    project.add('FEM Geometry', fem_geometry)
    project.add('FEM Shapes', fem_shapes)
    project.add('Test Geometry', test_geometry)
    project.add('Test Shapes', test_shapes)
    project.link('FEM Geometry', 'FEM Shapes')
    project.link('Test Geometry', 'Test Shapes')
    return held


def shown(state, *objects, prefs=None, validate=None):
    out = projects.show(state, {'objects': list(objects), 'records': {}, 'part': None,
                                **({'prefs': prefs} if prefs else {})})
    if validate:
        wire_views(out['views'], validate)
        validate({**out, 'views': []}, 'shown')
    return out


def _moving(mode):
    shape = np.asarray(mode['shape'])
    if 'shape_imag' in mode:
        shape = np.hypot(shape, np.asarray(mode['shape_imag']))
    return np.flatnonzero(np.abs(shape).max(axis=1) > 0)


@pytest.mark.parametrize('order', [('FEM Shapes', 'Test Shapes'), ('Test Shapes', 'FEM Shapes')])
def test_each_set_moves_its_own_geometry(state, validate, order):
    from hertzforge.service.mesh import drawn_mesh

    project = state.project
    first, second = order
    out = shown(state, first, second, validate=validate)
    matrix, stage = out['views'][:2]
    assert matrix['selects'] == 'shape3d' and stage['view'] == 'shape3d'
    home = {name: project.geometry_for(name) for name in order}
    count = {name: len(drawn_mesh(home[name][1])[0]) for name in order}
    # the scene is the first set's geometry, then the second's
    assert len(stage['nodes']) == count[first] + count[second]
    assert np.allclose(stage['nodes'][:count[first]],
                       np.asarray(home[first][1].node_xyz)[drawn_mesh(home[first][1])[0]])
    assert np.allclose(stage['nodes'][count[first]:],
                       np.asarray(home[second][1].node_xyz)[drawn_mesh(home[second][1])[0]])
    # the first's mesh is the view's own, the second's the overlays', and neither reaches
    # into the other's nodes
    own = [stage[key] for key in ('triangles', 'lines') if stage.get(key) is not None]
    assert all(mesh.max() < count[first] for mesh in own)
    assert 'overlay_triangles' in stage and 'overlay_lines' in stage
    over = [stage[key] for key in ('overlay_triangles', 'overlay_lines') if len(stage[key])]
    assert all(mesh.min() >= count[first] for mesh in over)
    assert all(mesh.max() < len(stage['nodes']) for mesh in over)
    # a mode moves only the first geometry, an overlay only the second
    assert len(stage['modes']) == project[first].num_shapes
    assert len(stage['overlays']) == project[second].num_shapes
    for mode in stage['modes']:
        moving = _moving(mode)
        assert len(moving) and moving.max() < count[first]
    for overlay in stage['overlays']:
        moving = _moving(overlay)
        assert len(moving) and moving.min() >= count[first]
    assert f'{first} drawn on {home[first][0]} and {second} on {home[second][0]}' in out['note']
    assert out['offers']['projection'] is True
    # the projection behind the MAC is made whether or not it is animated: what it matched
    # is shown beside it
    reading = out['views'][2]
    assert reading['view'] == 'reading'
    assert reading['title'].startswith('FEM Shapes projected onto Test Shapes: it has 2 nodes')
    assert [row['label'] for row in reading['rows']] == ['Nodes matched', 'Worst distance',
                                                         'Match limit', 'Dropped']


def test_sets_on_one_geometry_share_its_nodes_as_before(state):
    project = state.project
    out = shown(state, 'FEM Shapes', 'FEM Shapes')
    stage = out['views'][1]
    assert 'overlay_triangles' not in stage and 'overlay_lines' not in stage
    from hertzforge.service.mesh import drawn_mesh
    assert len(stage['nodes']) == len(drawn_mesh(project['FEM Geometry'])[0])


def test_matched_modes_on_two_geometries_draw_each_on_its_own(state, validate):
    from hertzforge.service.mesh import drawn_mesh
    from hertzforge.service.ops_tables import OPERATIONS

    project = state.project
    add = next(op for op in OPERATIONS if op.id == 'project.add_matches')
    add.run(project, {'first': 'FEM Shapes', 'second': 'Test Shapes', 'pairs': '1,1'})
    name = next(n for n, obj in project.items() if projects.kind_of(obj) == 'matched_modes')
    try:
        out = shown(state, name, validate=validate)
        stage = next(view for view in out['views'] if view['view'] == 'shape3d')
        first = len(drawn_mesh(project['FEM Geometry'])[0])
        assert stage['paired'] and len(stage['nodes']) > first
        assert _moving(stage['modes'][0]).max() < first
        assert _moving(stage['overlays'][0]).min() >= first
        assert 'overlay_lines' in stage and 'overlay_triangles' in stage
    finally:
        project.remove(name)


def test_a_pair_on_two_geometries_is_aligned_by_the_projected_shapes(state):
    """Sets on two geometries share no DOF, so each pair's angle is read from the second
    set projected onto the first's DOFs: with it the two move together, and a mode turned
    over is turned back (Brandon, 2026-11-16: the pairs were not aligned)."""
    from hertzforge.core.shapes import ShapeSet, shared_columns

    project = state.project
    first, second = 'FEM Shapes', 'Test Shapes'

    def phases():
        out = shown(state, first, second)
        matrix, stage = out['views'][:2]
        rows, columns = len(stage['modes']), len(stage['overlays'])
        return (np.asarray(matrix['values']).reshape(rows, columns),
                np.asarray(stage['overlay_phase']).reshape(rows, columns))

    values, phase = phases()
    ours_set, compared, _said, _report = project.compared_sets(first, second)
    ours, theirs, signs = shared_columns(ours_set, compared)
    assert ours, 'the projection is on the first set\'s DOFs'
    # every pair whose shapes have something in common is turned to move together
    inner = (np.conj(np.asarray(ours_set.shape_matrix)[:, ours])
             @ (np.asarray(compared.shape_matrix)[:, theirs] * signs).T)
    related = np.abs(inner) > 1e-9 * np.abs(inner).max()
    assert related.any() and values.shape == inner.shape
    assert np.all((inner * np.exp(1j * phase)).real[related] > 0)
    # the test's modes turned over: every angle moves half a turn
    held = project[second]
    project[second] = ShapeSet(held.frequency, held.damping, held.coordinate,
                               -np.asarray(held.shape_matrix))
    try:
        _values, turned = phases()
    finally:
        project[second] = held
    apart = np.abs(np.angle(np.exp(1j * (turned - phase))))
    assert np.allclose(apart[related], np.pi, atol=1e-5)


def test_the_projection_goes_onto_the_uniaxial_set_whichever_is_picked_first(state, validate):
    """The test set has nodes measured along one axis, so the model is projected onto it
    in either selection order: the MAC is the same, turned, and the note says which way."""
    from hertzforge.service.mesh import drawn_mesh

    forward = shown(state, 'FEM Shapes', 'Test Shapes', validate=validate)
    back = shown(state, 'Test Shapes', 'FEM Shapes', validate=validate)
    for out in (forward, back):
        assert 'FEM Shapes projected onto Test Shapes: it has 2 nodes measured' in out['note']
    rows, columns = len(forward['views'][1]['modes']), len(forward['views'][1]['overlays'])
    assert np.allclose(np.asarray(forward['views'][0]['values']).reshape(rows, columns),
                       np.asarray(back['views'][0]['values']).reshape(columns, rows).T,
                       atol=1e-6)
    # Projection animates it on the geometry projected onto: the test's, both ways
    project = state.project
    test_nodes = len(drawn_mesh(project['Test Geometry'])[0])
    first = shown(state, 'FEM Shapes', 'Test Shapes', prefs={'projection': True},
                  validate=validate)
    matrix, stage = first['views'][:2]
    assert matrix['rows']['label'] == 'FEM Shapes, projected'
    assert matrix['columns']['label'] == 'Test Shapes'
    assert len(stage['nodes']) == test_nodes and 'on Test Geometry' in matrix['title']
    second = shown(state, 'Test Shapes', 'FEM Shapes', prefs={'projection': True},
                   validate=validate)
    matrix, stage = second['views'][:2]
    assert matrix['rows']['label'] == 'Test Shapes'
    assert matrix['columns']['label'] == 'FEM Shapes, projected'
    assert len(stage['nodes']) == test_nodes


@pytest.fixture
def apart():
    """The two sets with the test geometry moved well off the model."""
    from test_correlate import _fem, _test_model

    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    reach = float(np.ptp(np.asarray(fem_geometry.node_xyz), axis=0).max())
    test_geometry.node_xyz = np.asarray(test_geometry.node_xyz) + [0.0, 0.0, 0.3 * reach]
    held = projects.new({'title': 'Apart'})
    project = held.project
    project.add('FEM Geometry', fem_geometry)
    project.add('FEM Shapes', fem_shapes)
    project.add('Test Geometry', test_geometry)
    project.add('Test Shapes', test_shapes)
    project.link('FEM Geometry', 'FEM Shapes')
    project.link('Test Geometry', 'Test Shapes')
    return held


def test_the_match_distance_is_the_comparisons_own(apart, validate):
    # by default nothing matches: said beside the distance, with how far off the nearest is
    out = shown(apart, 'FEM Shapes', 'Test Shapes', validate=validate)
    (reading,) = out['views']
    assert reading['view'] == 'reading' and 'No node' in reading['title']
    rows = {row['label']: row for row in reading['rows']}
    assert rows['Nodes matched']['value'].startswith('0 of ')
    assert rows['Worst distance']['value'] > rows['Match limit']['value'] > 0
    assert 'no test node has a FEM node within the match tolerance: the nearest is' in out['note']
    assert out['offers']['projection'] is True, 'the distance is offered with it'
    # the distance offered is the one used: 2 % of the size of the geometry projected onto
    default = apart.project.match_distance('FEM Shapes', 'Test Shapes')
    assert out['offers']['match_distance'] == {'value': default, 'defined': False}
    assert rows['Match limit']['value'] == pytest.approx(default)
    # loosened past the 0.3 the test geometry was moved, the MAC is made within it
    loose = shown(apart, 'FEM Shapes', 'Test Shapes', prefs={'match_distance': 0.5},
                  validate=validate)
    assert loose['views'][0]['view'] == 'matrix'
    limit = {row['label']: row for row in loose['views'][2]['rows']}['Match limit']['value']
    assert limit == pytest.approx(0.5)
    assert loose['offers']['match_distance'] == {'value': 0.5, 'defined': False}
    # and the animated projection takes the same distance
    moving = shown(apart, 'FEM Shapes', 'Test Shapes',
                   prefs={'match_distance': 0.5, 'projection': True}, validate=validate)
    assert any(view['view'] == 'shape3d' for view in moving['views'])
    still = shown(apart, 'FEM Shapes', 'Test Shapes', prefs={'projection': True})
    assert 'No node' in still['views'][0]['title']
    # a pair on one geometry has nothing to match: no distance is offered
    alone = shown(apart, 'FEM Shapes', 'FEM Shapes', validate=validate)
    assert alone['offers']['match_distance'] is None
    for bad in (-1, float('nan'), float('inf'), '2', True):
        with pytest.raises(ProtocolError, match='"match_distance"'):
            shown(apart, 'FEM Shapes', 'Test Shapes', prefs={'match_distance': bad})


def test_the_match_distance_offered_says_when_it_is_in_meters(apart, validate):
    apart.project['FEM Geometry'].define_units('m')
    apart.project['Test Geometry'].define_units('m')
    out = shown(apart, 'FEM Shapes', 'Test Shapes', prefs={'match_distance': 0.5},
                validate=validate)
    assert out['offers']['match_distance'] == {'value': 0.5, 'defined': True}
