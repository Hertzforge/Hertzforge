"""A shape set picked with a geometry it does not belong on is not drawn there by DOF number:
the view shows the projection Project would make onto that geometry, or why it cannot be
made, and the table offers Project and the objects' own acts, not the set's."""

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
        assert 'project.project_onto_basis' in offered
        assert 'show.animate' not in offered and 'project.synthesize_modal_frfs' not in offered
        assert 'self.link' in offered
    assert list(project) == before, 'nothing is added by looking'


def test_a_set_that_cannot_reduce_onto_it_shows_what_expand_would_add(validate):
    """The test set holds one direction at each node: it cannot go onto the cube's nodes, but
    it expands to them through the cube's modes, linked to that geometry."""
    from .conftest import wire_views

    project = _project()
    out = show(project, ['Test Shapes', 'FEM Geometry'], {}, None)
    wire_views(out['views'], validate)
    stage, reading = out['views']
    assert stage['title'] == 'Node matches: FEM Shapes onto Test Shapes'
    assert reading['title'] == ('Test Shapes expanded to the DOFs of FEM Geometry through FEM '
                                'Shapes: what Expand would add')
    said = {row['label']: row['value'] for row in reading['rows']}
    assert said['Not reduced'] == 'Test Shapes has 2 nodes measured in fewer than three directions'
    assert said['Model modes used'] == 2.0
    offered = _offered(project, ['Test Shapes', 'FEM Geometry'])
    assert 'project.expand_modes' in offered and 'project.project_onto_basis' not in offered
    assert 'show.animate' not in offered and 'self.link' in offered


def test_neither_way_says_why():
    project = _project()
    project.add('Bare', copy.deepcopy(project['FEM Geometry']))
    (reading,) = show(project, ['Test Shapes', 'Bare'], {}, None)['views']
    assert reading['title'] == 'Test Shapes cannot be reduced or expanded onto Bare'
    said = {row['label']: row['value'] for row in reading['rows']}
    assert said['Not reduced'] == 'Test Shapes has 2 nodes measured in fewer than three directions'
    assert said['Not expanded'] == ('no shape set linked to Bare holds the whole motion to '
                                    'expand through')
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
