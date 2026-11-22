"""A shape set picked with a geometry it does not belong on is not drawn there by DOF number:
the view shows the reduction onto that geometry's nodes, or why it cannot be made, and the
table offers DOF Mapping with Reduce under it and the objects' own acts, not the set's.
Expanding needs a model's modes, so it is offered with the model's set, not its geometry."""

from __future__ import annotations

import copy
import sys
from pathlib import Path

import hertzforge
from hertzforge.project import drawn_elsewhere
from hertzforge.service.action_table import action_table
from hertzforge.service.show import show

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from test_correlate import _fem, _test_model


def _project():
    """The cube's modes on their geometry and a uni-axial test's on another."""
    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    project = hertzforge.Project()
    project.add('FEM Geometry', fem_geometry)
    project.add('FEM Shapes', fem_shapes)
    project.add('Test Geometry', test_geometry)
    project.add('Test Shapes', test_shapes)
    project.link('FEM Geometry', 'FEM Shapes')
    project.link('Test Geometry', 'Test Shapes')
    return project


def _offered(project, names):
    return [row['id'] for row in action_table(project, names, {})['columns'][0]['rows']
            if row['ready'] and row['kind'] != 'group']


def test_a_set_that_can_go_onto_the_geometry_shows_what_reduce_would_add(validate):
    from .conftest import wire_views

    project = _project()
    before = list(project)
    for names in (['FEM Shapes', 'Test Geometry'], ['Test Geometry', 'FEM Shapes']):
        out = show(project, names, {}, None)
        wire_views(out['views'], validate)
        stage, reading = out['views']
        assert stage['title'] == 'Node matches: FEM Shapes onto Test Geometry'
        assert reading['title'] == 'FEM Shapes at the DOFs of Test Geometry: what Reduce would add'
        assert {row['label']: row['value'] for row in reading['rows']}['Nodes matched'] == '2 of 2'
        offered = _offered(project, names)
        assert offered[:2] == ['project.set_projection', 'project.project_onto_basis']
        rows = {row['id']: row for row in action_table(project, names, {})['columns'][0]['rows']}
        assert rows['project.project_onto_basis']['under'] == 'project.set_projection'
        assert 'project.expand_modes' not in offered
        assert 'show.animate' not in offered and 'project.synthesize_modal_frfs' not in offered
        assert 'self.link' in offered
    assert list(project) == before, 'nothing is added by looking'


def test_dof_mapping_offers_the_two_ways_the_set_goes_and_says_why_expansion_cannot():
    project = _project()
    rows = {row['id']: row for row in
            action_table(project, ['Test Geometry', 'FEM Shapes'], {})['columns'][0]['rows']}
    mapping = rows['project.set_projection']
    assert (mapping['fill']['first'], mapping['fill']['second']) == ('FEM Shapes',
                                                                     'Test Geometry')
    assert mapping['fill']['mapping'] == 'reduce_named'
    assert mapping['labels'] == {'mapping': {
        'reduce_named': 'Reduction: FEM Shapes at the nodes of Test Geometry',
        'expand_named': 'Expansion: FEM Shapes to the DOFs of Test Geometry (not possible: '
                        'Test Geometry holds no modes to expand through)'}}


def test_dof_mapping_on_a_geometry_draws_and_keeps_the_reduction(validate):
    from .conftest import wire_views
    from .test_projection_distance import _project_op

    project = _project()
    views = _project_op(project, op='project.set_projection', first='FEM Shapes',
                        second='Test Geometry', mapping='reduce_named', distance=0.5)
    wire_views(views, validate)
    assert [view['title'] for view in views] == [
        'Node matches: FEM Shapes onto Test Geometry',
        'FEM Shapes at the DOFs of Test Geometry']
    assert project.projection_of('FEM Shapes', 'Test Geometry')[2] == 0.5
    (_stage, reading) = show(project, ['FEM Shapes', 'Test Geometry'], {}, None)['views']
    assert {row['label']: row['value'] for row in reading['rows']}['Match limit'] == 0.5


def test_a_set_that_cannot_reduce_onto_it_says_why_and_what_to_pick_to_expand():
    """The test set holds one direction at each node: it cannot go onto the cube's nodes, and
    expanding it to them needs the cube's modes picked with it."""
    project = _project()
    (reading,) = show(project, ['Test Shapes', 'FEM Geometry'], {}, None)['views']
    assert reading['title'] == 'Test Shapes cannot be reduced onto FEM Geometry'
    said = {row['label']: row['value'] for row in reading['rows']}
    assert said['Not reduced'] == 'Test Shapes has 2 nodes measured in fewer than three directions'
    assert said['Not expanded'] == ('FEM Geometry holds no modes to expand through; pick Test '
                                    'Shapes with FEM Shapes to expand through its modes')
    assert _offered(project, ['Test Shapes', 'FEM Geometry']) == ['self.link', 'self.copy']


def test_with_no_set_to_expand_through_none_is_named():
    project = _project()
    project.add('Bare', copy.deepcopy(project['FEM Geometry']))
    (reading,) = show(project, ['Test Shapes', 'Bare'], {}, None)['views']
    said = {row['label']: row['value'] for row in reading['rows']}
    assert said['Not expanded'] == 'Bare holds no modes to expand through'
    assert _offered(project, ['Test Shapes', 'Bare']) == ['self.link', 'self.copy']


def test_nothing_matched_is_drawn_and_said_not_refused():
    project = _project()
    far = _test_model()[0]
    far.node_xyz = far.node_xyz + 10.0
    project['Test Geometry'] = far
    stage, reading = show(project, ['FEM Shapes', 'Test Geometry'], {}, None)['views']
    assert stage['title'] == 'Node matches: FEM Shapes onto Test Geometry'
    assert reading['title'] == ('No node of Test Geometry matched FEM Shapes within the default '
                                'distance')


def test_on_its_own_geometry_a_set_is_animated_as_before():
    project = _project()
    out = show(project, ['FEM Shapes', 'FEM Geometry'], {}, None)
    assert [view['view'] for view in out['views']] == ['shape3d']
    assert out['views'][0]['title'].startswith('FEM Shapes')
    assert 'show.animate' in _offered(project, ['FEM Shapes', 'FEM Geometry'])


def test_linked_to_the_geometry_the_set_is_drawn_on_it():
    """Link says the geometry goes with the set: it is the set's stage from then on."""
    project = _project()
    project.add('Unplaced', copy.deepcopy(project['FEM Shapes']))
    project.add('Bare', copy.deepcopy(project['FEM Geometry']))
    project.active_geometry = 'FEM Geometry'
    assert drawn_elsewhere(project, 'Unplaced', 'Bare')
    assert not drawn_elsewhere(project, 'Unplaced', 'FEM Geometry'), \
        'a set in no group is drawn on the active geometry'
    project.link('Bare', 'Unplaced')
    assert not drawn_elsewhere(project, 'Unplaced', 'Bare')
    assert 'show.animate' in _offered(project, ['Unplaced', 'Bare'])
