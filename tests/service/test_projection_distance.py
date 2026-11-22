"""A projection's node match tolerance is a distance typed in the display system, and its
row opens on the distance 2 % of the target's size comes to; a shape set beside another
group's geometry is offered Project, onto every node of that geometry."""

from __future__ import annotations

import asyncio
import copy
import sys
from pathlib import Path

import pytest

import hertzforge
from hertzforge.service import display_units
from hertzforge.service.action_table import action_table
from hertzforge.service.protocol import ProtocolError

from .test_projects import _exchange

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from test_correlate import _fem, _test_model

MM = 'mm-kg-N-s'


def _project(defined=True):
    """The cube's modes on their geometry and a test's on another, each node of the test
    1 mm from a corner of the cube, both in meters unless `defined` is False."""
    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    if defined:
        fem_geometry.define_units('m')
        test_geometry.define_units('m')
    project = hertzforge.Project()
    project.add('FEM Geometry', fem_geometry)
    project.add('FEM Shapes', fem_shapes)
    project.add('Test Geometry', test_geometry)
    project.add('Test Shapes', test_shapes)
    project.link('FEM Geometry', 'FEM Shapes')
    project.link('Test Geometry', 'Test Shapes')
    return project


def _row(project, names, system=display_units.DEFAULT, op='project.project_onto_basis'):
    table = display_units.running_in(system, action_table, project, names, {})
    return next(row for row in table['columns'][0]['rows'] if row['id'] == op)


def test_onto_a_geometry_the_row_opens_on_the_default_distance_in_the_display_units():
    project = _project()
    default = project.match_distance('FEM Shapes', 'Test Geometry')
    for system, scale in ((display_units.DEFAULT, 1.0), (MM, 1000.0)):
        row = _row(project, ['FEM Shapes', 'Test Geometry'], system)
        assert row['ready'], row
        assert float(row['fill']['distance']) == pytest.approx(default * scale, rel=1e-3)
    assert _row(project, ['FEM Shapes', 'Test Geometry'])['fill']['onto'] == 'Test Geometry'


def test_an_undefined_geometry_opens_on_its_own_numbers():
    project = _project(defined=False)
    default = project.match_distance('FEM Shapes', 'Test Geometry')
    row = _row(project, ['FEM Shapes', 'Test Geometry'], MM)
    assert float(row['fill']['distance']) == pytest.approx(default, rel=1e-3)


def test_project_is_greyed_out_with_why_it_cannot_project():
    project = _project()
    project.add('Other Modes', copy.deepcopy(project['FEM Shapes']))
    project.link('FEM Geometry', 'Other Modes')
    said = {
        ('FEM Shapes', 'Other Modes'): 'Both are on FEM Geometry, so there is nothing to project',
        ('FEM Shapes', 'FEM Geometry'): None,
        ('Test Shapes', 'FEM Geometry'):
            'Test Shapes has 2 nodes measured in fewer than three directions'}
    for names, why in said.items():
        table = action_table(project, list(names), {})
        row = next((r for r in table['columns'][0]['rows']
                    if r['id'] == 'project.project_onto_basis'), None)
        if why is None:
            # a set beside the geometry it is on: that geometry is only the stage
            assert row is None or not row['ready'], row
            continue
        assert row is not None and not row['ready']
        assert row['reason'] == why
        assert 'distance' not in (row.get('fill') or {})


def _save(project, tmp_path):
    path = str(tmp_path / 'two.hforge')
    project.save(path)
    return path


def _said_nothing_added(answer, onto):
    """No node matched: the two geometries drawn with every node unmatched, and nothing added."""
    stage, reading = answer['result']['views']
    assert reading['title'] == f'No node of {onto} matched, so nothing was added'
    assert stage['title'].startswith('Node matches')
    assert 'segments' not in stage or all(s['color'] != [255, 193, 7] for s in stage['segments'])
    assert 'project' not in answer['result'] or len(answer['result']['project']['objects']) == 4


def _run(path, op, params, system=MM):
    return [got[-1] for got in asyncio.run(_exchange([
        ('project.open', {'path': path}),
        ('units.system', {'name': system}),
        ('op.run', {'op': op, 'params': params})]))][-1]


@pytest.mark.parametrize('onto', ['Test Shapes', 'Test Geometry'])
def test_a_distance_typed_in_millimeters_is_taken_to_meters(tmp_path, onto):
    """Each test node is 1 mm from its corner: 1.1 mm matches them, 0.9 mm none. Taken as
    meters, 0.9 would match as well."""
    path = _save(_project(), tmp_path)
    params = {'source': 'FEM Shapes', 'onto': onto}
    near = _run(path, 'project.project_onto_basis', {**params, 'distance': 1.1})
    assert 'result' in near, near
    far = _run(path, 'project.project_onto_basis', {**params, 'distance': 0.9})
    _said_nothing_added(far, onto)


def test_the_projection_set_in_millimeters_is_what_the_overlay_projects_within(tmp_path):
    """Project kept at 1.1 mm, the animated projection matches both test nodes; kept at
    0.9 mm, it matches none and is refused. Taken as meters, 0.9 would match as well."""
    path = _save(_project(), tmp_path)
    pair = {'first': 'FEM Shapes', 'second': 'Test Shapes'}
    for typed, matched in ((1.1, True), (0.9, False)):
        set_it, overlay = (got[-1] for got in asyncio.run(_exchange([
            ('project.open', {'path': path}), ('units.system', {'name': MM}),
            ('op.run', {'op': 'project.set_projection',
                        'params': {**pair, 'direction': 'named', 'distance': typed}}),
            ('op.run', {'op': 'project.projection_overlay', 'params': pair})]))[2:])
        assert 'result' in set_it, set_it
        assert ('result' in overlay) is matched, overlay


def test_an_undefined_geometry_takes_the_distance_as_it_is_typed(tmp_path):
    path = _save(_project(defined=False), tmp_path)
    params = {'source': 'FEM Shapes', 'onto': 'Test Shapes'}
    assert 'result' in _run(path, 'project.project_onto_basis', {**params, 'distance': 0.0011})
    far = _run(path, 'project.project_onto_basis', {**params, 'distance': 0.0009})
    _said_nothing_added(far, 'Test Shapes')


def test_zero_projects_within_the_default(tmp_path):
    project = _project()
    path = _save(project, tmp_path)
    answer = _run(path, 'project.project_onto_basis',
                  {'source': 'FEM Shapes', 'onto': 'Test Shapes', 'distance': 0.0})
    assert 'result' in answer, answer


def _project_op(project, **given):
    from hertzforge.service import projects
    from hertzforge.service.ops_project import MODAL_VERB_OPERATIONS

    op = next(o for o in MODAL_VERB_OPERATIONS if o.id == 'project.project_onto_basis')
    values = op.resolve(given, objects={n: projects.kind_of(o) for n, o in project.items()},
                        verbs={n: projects.verbs_of(project, n) for n in project})
    return op.run(project, values)


def _layers(stage):
    """The stage's point layers by color, each node's place; and its segments by color."""
    import numpy as np

    points = {tuple(np.asarray(layer['colors'])[0]): np.asarray(layer['xyz']).reshape(-1, 3)
              for layer in stage['points']}
    segments = {tuple(layer['color']): np.asarray(layer['xyz']).reshape(-1, 2, 3)
                for layer in stage.get('segments', [])}
    return points, segments


def test_the_stage_links_each_matched_node_to_its_model_node_and_marks_the_rest(validate):
    """Nodes 101 and 102 sit a millimeter from cube corners 1 (0, 0, 0) and 8 (1, 1, 1); 103
    is far off. Matched, 101 and 102 are linked to those corners and 103 is marked unmatched;
    tightened below a millimeter, all three are unmatched and nothing links."""
    import numpy as np

    from hertzforge.service.ops_project import LINK_COLOR, MATCHED_COLOR, UNMATCHED_COLOR

    from .conftest import wire_views

    project = _project(defined=False)
    project['Test Geometry'] = _test_model(extra_node=True)[0]
    stage = _project_op(project, source='FEM Shapes', onto='Test Geometry', distance=0.0011)[0]
    wire_views([stage], validate)
    assert stage['title'] == 'Node matches: FEM Shapes onto Test Geometry'
    points, segments = _layers(stage)
    assert points[MATCHED_COLOR] == pytest.approx(np.array([[0.001, 0, 0], [1, 1, 0.999]]))
    assert points[UNMATCHED_COLOR] == pytest.approx(np.array([[5.0, 5.0, 5.0]]))
    assert segments[LINK_COLOR] == pytest.approx(np.array([
        [[0.001, 0, 0], [0, 0, 0]], [[1, 1, 0.999], [1, 1, 1]]]))
    assert points[LINK_COLOR] == pytest.approx(np.array([[0, 0, 0], [1, 1, 1]])), \
        'the model nodes they were matched to'
    assert len(stage['nodes']) == 8, "the model's own nodes are the mesh"

    tight = _project_op(project, source='FEM Shapes', onto='Test Geometry', distance=0.0009)
    stage, reading = tight
    wire_views([stage], validate)
    points, segments = _layers(stage)
    assert MATCHED_COLOR not in points and LINK_COLOR not in segments and LINK_COLOR not in points
    assert len(points[UNMATCHED_COLOR]) == 3
    assert reading['title'] == 'No node of Test Geometry matched, so nothing was added'
    assert not [name for name in project if name.startswith('FEM Shapes @ Test Geometry')
                and project[name].projection_report['limit'] == 0.0009]


def test_the_stage_is_drawn_in_the_display_units(tmp_path):
    """In millimeters a link a millimeter long is drawn a millimeter long."""
    import numpy as np

    from hertzforge.service.ops_project import LINK_COLOR

    path = _save(_project(), tmp_path)
    answer = _run(path, 'project.project_onto_basis',
                  {'source': 'FEM Shapes', 'onto': 'Test Geometry', 'distance': 1.1})
    stage = next(view for view in answer['result']['views']
                 if view['title'].startswith('Node matches'))
    assert stage['unit'] == 'mm'
    _points, segments = _layers(stage)
    lengths = np.linalg.norm(np.diff(segments[LINK_COLOR], axis=1), axis=2).ravel()
    assert lengths == pytest.approx([1.0, 1.0], rel=1e-4)


def test_the_preview_matches_against_the_geometry_apply_would(tmp_path):
    """A preview runs on a copy of what its form names; the copy keeps each object's linked
    geometry, so the set is still on the cube and the stage is the one Apply draws."""
    import numpy as np

    from hertzforge.service.ops_project import LINK_COLOR

    path = _save(_project(), tmp_path)
    params = {'source': 'FEM Shapes', 'onto': 'Test Geometry', 'distance': 1.1}
    previewed, applied = (got[-1] for got in asyncio.run(_exchange([
        ('project.open', {'path': path}), ('units.system', {'name': MM}),
        ('op.run', {'op': 'project.project_onto_basis', 'params': params, 'preview': True}),
        ('op.run', {'op': 'project.project_onto_basis', 'params': params})]))[2:])
    assert 'result' in previewed, previewed
    assert 'project' not in previewed['result'] or len(
        previewed['result']['project']['objects']) == 4, 'a preview adds nothing'

    def links(answer):
        stage = next(v for v in answer['result']['views'] if v['title'].startswith('Node matches'))
        return _layers(stage)[1][LINK_COLOR]

    assert np.allclose(links(previewed), links(applied))


def test_onto_a_table_a_node_with_no_channel_is_neither_matched_nor_unmatched():
    """The table's channels are at 101 and 102; its geometry's third node, 103, has none, so the
    projection never reads it: it is drawn in the gray of a node not read."""
    import numpy as np
    from test_correlate import _channels

    from hertzforge.service.ops_project import MATCHED_COLOR, UNMATCHED_COLOR, UNREAD_COLOR

    project = _project(defined=False)
    project['Test Geometry'] = _test_model(extra_node=True)[0]
    project.add('Channels', _channels())
    project.link('Test Geometry', 'Channels')
    stage = _project_op(project, source='FEM Shapes', onto='Channels', distance=0.0011)[0]
    points, _segments = _layers(stage)
    assert points[UNREAD_COLOR] == pytest.approx(np.array([[5.0, 5.0, 5.0]]))
    assert len(points[MATCHED_COLOR]) == 2 and UNMATCHED_COLOR not in points


def _row_for(project, names, op='project.project_onto_basis'):
    table = action_table(project, names, {})
    return next(row for row in table['columns'][0]['rows'] if row['id'] == op)


def _way(row):
    """(the set projected, the one onto) as Project's row fills them."""
    fill = row['fill']
    pair = (fill['first'], fill['second'])
    return pair[::-1] if fill['direction'] == 'reversed' else pair


def test_project_opens_on_the_way_the_pair_goes_whichever_set_is_picked_first():
    """The test set holds one direction at each node, so only the cube's modes go onto it. In
    either order of picking, Project opens on that way, words the other with why, and opens
    on 0, 2 % of the target's size, while it keeps no distance."""
    project = _project()
    for names in (['Test Shapes', 'FEM Shapes'], ['FEM Shapes', 'Test Shapes']):
        row = _row_for(project, names, 'project.set_projection')
        assert row['live'] and row['open']
        assert _way(row) == ('FEM Shapes', 'Test Shapes'), names
        words = row['labels']['direction']
        bad = 'named' if row['fill']['direction'] == 'reversed' else 'reversed'
        assert words[bad] == ('Test Shapes onto FEM Shapes (not possible: Test Shapes has 2 '
                              'nodes measured in fewer than three directions)')
        assert words[row['fill']['direction']] == 'FEM Shapes onto Test Shapes'
        assert row['fill']['distance'] == '0'


def test_what_reads_the_projection_is_listed_under_project():
    project = _project()
    rows = {row['id']: row for row in action_table(project, ['FEM Shapes', 'Test Shapes'],
                                                   {})['columns'][0]['rows']}
    under = sorted(name for name, row in rows.items() if row.get('under') == 'project.set_projection')
    assert under == sorted(['show.mac', 'project.projection_overlay', 'project.match_modes',
                            'project.add_matches', 'project.project_onto_basis'])
    assert rows['project.project_onto_basis']['verb'] == 'Add Projected Set'
    # on one geometry there is nothing to project: no Project, and MAC stands alone
    project.add('Other Modes', copy.deepcopy(project['FEM Shapes']))
    project.link('FEM Geometry', 'Other Modes')
    rows = {row['id']: row for row in action_table(project, ['FEM Shapes', 'Other Modes'],
                                                   {})['columns'][0]['rows']}
    assert not rows['project.set_projection']['ready']
    assert rows['project.set_projection']['reason'] == ('Both are on FEM Geometry, so there '
                                                        'is nothing to project')
    assert 'under' not in rows['show.mac']


def test_project_keeps_what_is_set_and_its_row_shows_it_in_the_display_units():
    project = _project()
    project.add('Model B', copy.deepcopy(project['FEM Geometry']))
    project.add('Modes B', copy.deepcopy(project['FEM Shapes']))
    project.link('Model B', 'Modes B')
    project.set_projection('FEM Shapes', 'Modes B', source='Modes B', distance=0.0011)
    for system, typed in ((display_units.DEFAULT, 0.0011), (MM, 1.1)):
        row = _row(project, ['FEM Shapes', 'Modes B'], system, 'project.set_projection')
        assert _way(row) == ('Modes B', 'FEM Shapes')
        assert float(row['fill']['distance']) == pytest.approx(typed)
        added = _row(project, ['FEM Shapes', 'Modes B'], system)
        assert (added['fill']['source'], added['fill']['onto']) == ('Modes B', 'FEM Shapes')
        assert float(added['fill']['distance']) == pytest.approx(typed)
        assert sorted(added['omit']) == ['direction', 'distance']


def _set_op(project, **given):
    from hertzforge.service import projects
    from hertzforge.service.ops_project import MODAL_VERB_OPERATIONS

    op = next(o for o in MODAL_VERB_OPERATIONS if o.id == 'project.set_projection')
    values = op.resolve(given, objects={n: projects.kind_of(o) for n, o in project.items()},
                        verbs={n: projects.verbs_of(project, n) for n in project})
    return op.run(project, values)


def test_setting_the_projection_keeps_it_and_draws_what_it_matches(validate):
    from .conftest import wire_views

    project = _project(defined=False)
    stage, reading = _set_op(project, first='Test Shapes', second='FEM Shapes',
                             direction='reversed', distance=0.0011)
    wire_views([stage, reading], validate)
    assert project.projection_of('Test Shapes', 'FEM Shapes') == ('FEM Shapes', 'Test Shapes',
                                                                  0.0011)
    assert stage['title'] == 'Node matches: FEM Shapes onto Test Shapes'
    assert {row['label']: row['value'] for row in reading['rows']}['Match limit'] == 0.0011
    # kept tighter than any node is close, it is still kept, and says nothing matched
    stage, reading = _set_op(project, first='Test Shapes', second='FEM Shapes',
                             direction='reversed', distance=0.0009)
    assert project.projection_of('Test Shapes', 'FEM Shapes')[2] == 0.0009
    assert {row['label']: row['value'] for row in reading['rows']}['Nodes matched'] == '0 of 2'
    with pytest.raises(ProtocolError, match='Test Shapes cannot be projected onto FEM Shapes: '
                                            'Test Shapes has 2 nodes measured'):
        _set_op(project, first='Test Shapes', second='FEM Shapes', direction='named',
                distance=0.0)
    assert project.projection_of('Test Shapes', 'FEM Shapes')[2] == 0.0009, 'refused, not kept'


def test_the_comparisons_read_the_projection_project_keeps():
    """Kept tighter than a millimeter, no node of the test is within reach: the MAC, Match
    Modes and Add Projected Set all say so, where the default matched both."""
    project = _project(defined=False)
    project.comparison_mac('FEM Shapes', 'Test Shapes')
    project.match_modes('FEM Shapes', 'Test Shapes')
    project.set_projection('FEM Shapes', 'Test Shapes', distance=0.0009)
    for call in (lambda: project.comparison_mac('FEM Shapes', 'Test Shapes'),
                 lambda: project.match_modes('FEM Shapes', 'Test Shapes', name='Again')):
        with pytest.raises(ValueError, match='within the match tolerance'):
            call()
    row = _row_for(project, ['FEM Shapes', 'Test Shapes'])
    views = _project_op(project, **{k: row['fill'][k] for k in ('source', 'onto', 'direction')},
                        distance=float(row['fill']['distance']))
    assert views[-1]['title'] == 'No node of Test Shapes matched, so nothing was added'


def test_a_table_or_a_geometry_goes_one_way_and_is_not_asked():
    from test_correlate import _channels

    project = _project()
    project.add('Channels', _channels())
    project.link('Test Geometry', 'Channels')
    for onto in ('Test Geometry', 'Channels'):
        row = _row_for(project, ['FEM Shapes', onto])
        assert 'direction' in row['omit'] and 'labels' not in row
        assert 'direction' not in row['fill']
        assert row['verb'] == 'Project' and 'under' not in row
    with pytest.raises(ProtocolError, match='only a shape set is projected the other way'):
        _project_op(project, source='FEM Shapes', onto='Channels', direction='reversed')


def test_a_preview_of_project_draws_the_matches_and_keeps_nothing(tmp_path):
    """Project is previewed as it opens: the matches are drawn, and the pair's projection is
    kept only when the form is changed, not by the preview."""
    path = _save(_project(), tmp_path)
    params = {'first': 'FEM Shapes', 'second': 'Test Shapes', 'direction': 'named',
              'distance': 1.1}
    previewed, actions = (got[-1] for got in asyncio.run(_exchange([
        ('project.open', {'path': path}), ('units.system', {'name': MM}),
        ('op.run', {'op': 'project.set_projection', 'params': params, 'preview': True}),
        ('project.actions', {'objects': ['FEM Shapes', 'Test Shapes'], 'records': {}})]))[2:])
    assert previewed['result']['views'][0]['title'] == 'Node matches: FEM Shapes onto Test Shapes'
    row = next(r for r in actions['result']['table']['columns'][0]['rows']
               if r['id'] == 'project.set_projection')
    assert row['fill']['distance'] == '0', 'the preview kept nothing'
