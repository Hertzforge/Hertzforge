"""Solid modeling: a new geometry, its solids added, changed and combined,
Mesh Solids, Mate Surfaces and the default view."""

from __future__ import annotations

import math

import numpy as np
import pytest

from hertzforge.core.fem import GroupProperties, material
from hertzforge.project import Project
from hertzforge.service import projects
from hertzforge.service.ops_geometry import OPERATIONS as GEOMETRY_OPERATIONS
from hertzforge.service.ops_solids import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

OPS = {op.id: op for op in (*OPERATIONS, *GEOMETRY_OPERATIONS)}


@pytest.fixture
def project():
    project = Project()
    project.new_geometry('Part', unit='mm')
    return project


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()})
    return op.run(project, values)


def rows(reading):
    return {row['label']: row for row in reading['rows']}


def refused(project, op_id, match, **given):
    with pytest.raises(ProtocolError, match=match) as error:
        run(project, op_id, **given)
    assert error.value.code == 'refused'


def block(project, **given):
    sizes = {'width_x': 0.1, 'width_y': 0.05, 'width_z': 0.01}
    return run(project, 'project.add_solid_block', **{**sizes, **given})


def test_the_descriptors_validate(validate):
    for op in OPERATIONS:
        validate(op.to_json(), 'operation')
        assert op.workspace in ('geometry', 'model')


def test_a_new_geometry_is_empty_and_records_its_unit(project, validate):
    reading, table = run(project, 'project.new_geometry', unit='in')
    wire_views([reading, table], validate)
    assert rows(reading)['Added']['value'] == 'Geometry'
    assert project['Geometry'].length_unit == 'in' and table['rows'] == []
    with pytest.raises(ProtocolError, match='not one of the choices'):
        run(project, 'project.new_geometry', unit='furlong')


def test_a_block_is_added_drawn_and_listed(project, validate):
    reading, view, table = block(project, x=0.02, name='base')
    wire_views([reading, view, table], validate)
    assert rows(reading)['Volume']['value'] == pytest.approx(0.1 * 0.05 * 0.01)
    assert view['view'] == 'shape3d' and len(view['triangles']) >= 12
    assert np.ptp(view['nodes'][:, 0]) == pytest.approx(0.1, rel=1e-5)
    assert table['rows'][0][:3] == ['1', 'base', 'Block']
    assert table['rows'][0][3] == '0.1 × 0.05 × 0.01 m'
    assert table['rows'][0][4] == '(0.02, 0, 0) m'
    refused(project, 'project.add_solid_block', 'width', width_x=0.0)


@pytest.mark.parametrize(('op_id', 'given', 'label', 'expected'), [
    ('project.add_solid_cylinder', {'radius': 0.01, 'length': 0.02}, 'Volume',
     math.pi * 0.01 ** 2 * 0.02),
    ('project.add_solid_cone', {'radius': 0.02, 'top_radius': 0.01, 'height': 0.03},
     'Volume', math.pi * 0.03 * (0.02 ** 2 + 0.02 * 0.01 + 0.01 ** 2) / 3),
    ('project.add_solid_sphere', {'radius': 0.02}, 'Volume', 4 / 3 * math.pi * 0.02 ** 3),
    ('project.add_solid_plate', {'width_x': 0.2, 'width_y': 0.1}, 'Area', 0.02),
    ('project.add_solid_disk', {'radius': 0.05}, 'Area', math.pi * 0.05 ** 2),
    ('project.add_solid_section', {'shape': 'I-beam', 'dimensions': '0.1, 0.05, 0.01, 0.005',
                                   'length': 0.5}, 'Volume',
     (2 * 0.05 * 0.01 + 0.08 * 0.005) * 0.5),
    ('project.add_solid_hole', {'diameter': 0.01, 'depth': 0.02}, 'Volume',
     math.pi * 0.005 ** 2 * 0.02),
])
def test_each_solid_reads_its_size(project, validate, op_id, given, label, expected):
    reading, view, table = run(project, op_id, **given)
    wire_views([reading, view, table], validate)
    assert rows(reading)[label]['value'] == pytest.approx(expected, rel=2e-3)
    assert len(table['rows']) == 1


@pytest.mark.parametrize(('op_id', 'given', 'match'), [
    ('project.add_solid_cylinder', {'radius': 0.0}, 'radius'),
    ('project.add_solid_cone', {'radius': 0.1, 'top_radius': 0.1}, 'is a cylinder'),
    ('project.add_solid_sphere', {'radius': 0.0}, 'radius'),
    ('project.add_solid_plate', {'width_y': 0.0}, 'width'),
    ('project.add_solid_disk', {'radius': 0.0}, 'radius'),
    ('project.add_solid_section', {'shape': 'angle', 'dimensions': '1, 2'}, 'takes long leg'),
    ('project.add_solid_hole', {'diameter': 0.01, 'counterbore_diameter': 0.005,
                                'counterbore_depth': 0.002}, 'counterbore wider'),
])
def test_what_a_solid_cannot_be_is_refused(project, op_id, given, match):
    refused(project, op_id, match, **given)
    assert project['Part'].solids == []


def test_a_section_opens_on_its_shape_s_dimensions(project):
    reading, _view, table = run(project, 'project.add_solid_section', shape='round tube')
    outer, wall = 1.0, 0.1
    assert rows(reading)['Volume']['value'] == pytest.approx(
        math.pi / 4 * (outer ** 2 - (outer - 2 * wall) ** 2) * 4.0, rel=2e-3)
    assert table['rows'][0][3] == 'outer diameter 1, wall 0.1, length 4 m'


def test_a_cylinder_subtracted_from_a_block_leaves_a_hole(project, validate):
    block(project)
    run(project, 'project.add_solid_cylinder', radius=0.01, length=0.02)
    reading, view, table = run(project, 'project.combine_solids', operation='subtract',
                               solids='1, 2', name='plate')
    wire_views([reading, view, table], validate)
    assert rows(reading)['Volume']['value'] == pytest.approx(
        0.1 * 0.05 * 0.01 - math.pi * 0.01 ** 2 * 0.01, rel=1e-6)
    assert rows(reading)['Parts']['value'] == 1.0
    assert [r[0] for r in table['rows']] == ['3', '1', '2']
    assert table['rows'][0][6] == 'subtract of 1, 2'
    assert table['rows'][2][6] == 'part, in 3'
    refused(project, 'project.combine_solids', 'part of solid 3', solids='1, 2')
    refused(project, 'project.combine_solids', 'not 9', solids='3, 9')


def test_a_solid_is_edited_by_the_fields_typed(project):
    block(project)
    reading, _view, table = run(project, 'project.edit_solid', solid=1,
                                widths='0.2, 0.05, 0.01', name='wide')
    assert table['rows'][0][1] == 'wide' and table['rows'][0][3] == '0.2 × 0.05 × 0.01 m'
    assert 'widths' in rows(reading)['Changed']['value']
    refused(project, 'project.edit_solid', 'type what changes', solid=1)
    refused(project, 'project.edit_solid', 'has no radius', solid=1, radius='0.1')
    refused(project, 'project.edit_solid', 'no solid 4', solid=4, name='x')


def test_parts_are_moved_and_turned_whole(project):
    block(project)
    run(project, 'project.add_solid_cylinder', radius=0.01, length=0.02)
    run(project, 'project.combine_solids', operation='subtract', solids='1, 2')
    reading, _view, _table = run(project, 'project.move_solids', solids='3', offset_z=0.5)
    from hertzforge.core import solids
    part = project['Part']
    assert solids.solid(part, 1).center[2] == pytest.approx(0.5)
    assert solids.solid(part, 2).center[2] == pytest.approx(0.5)
    assert rows(reading)['Moved']['value'].startswith('3: 2 solids')


def test_solids_are_copied_into_another_geometry(project):
    block(project)
    project.new_geometry('Other', unit='m')
    reading, _view, table = run(project, 'project.copy_solids', geometry='Part', solids='1',
                                into='Other')
    assert rows(reading)['Copied']['value'] == '1 as 1'
    assert len(project['Other'].solids) == 1 and table['title'] == 'Solids of Other'
    refused(project, 'project.copy_solids', 'not 5', geometry='Part', solids='5',
            into='Other')


def test_the_parts_are_reordered(project):
    block(project)
    run(project, 'project.add_solid_sphere', radius=0.01)
    _reading, _view, table = run(project, 'project.reorder_solids', solids='2, 1')
    assert [r[0] for r in table['rows']] == ['2', '1']
    refused(project, 'project.reorder_solids', 'give each of them', solids='2')


def test_a_boolean_is_taken_apart_and_deleted(project):
    block(project)
    run(project, 'project.add_solid_sphere', radius=0.01)
    run(project, 'project.combine_solids', operation='union', solids='1, 2')
    reading, _view, table = run(project, 'project.separate_solids', solids='3')
    assert rows(reading)['Parts']['value'] == 2.0 and len(table['rows']) == 2
    refused(project, 'project.separate_solids', 'is a block', solids='1')
    reading, _view, table = run(project, 'project.delete_solids', solids='1, 2')
    assert rows(reading)['Deleted']['value'] == '2 solids' and table['rows'] == []
    refused(project, 'project.delete_solids', 'no solids yet', solids='')


def test_a_pattern_copies_its_solids(project, validate):
    run(project, 'project.add_solid_cylinder', radius=0.01, length=0.02)
    reading, view, table = run(project, 'project.pattern_solids', solids='1', count_x=3,
                               count_y=2, spacing_x=0.05, spacing_y=0.04)
    wire_views([reading, view, table], validate)
    assert rows(reading)['Copies']['value'] == 5.0
    assert table['rows'][0][3] == '3 × 2 × 1, spacing 0.05, 0.04, 0 m'
    assert np.ptp(view['nodes'][:, 0]) == pytest.approx(0.12, rel=1e-3)
    refused(project, 'project.pattern_solids', 'needs a spacing', solids='2', count_y=2,
            spacing_y=0.0)
    refused(project, 'project.pattern_solids', 'no copies yet', solids='2', count_x=1)


def test_mesh_solids_previews_and_then_meshes(project, validate):
    block(project)
    reading, view = run(project, 'project.mesh_solids', size=0.005)
    wire_views([reading, view], validate)
    found = rows(reading)
    assert found['Bricks']['value'] == 20 * 10 * 2
    assert found['Method']['value'] == 'extruded along Z'
    assert project['Part'].num_nodes == 0
    reading, view, table = run(project, 'project.mesh_solids', size=0.005, keep=True)
    wire_views([reading, view, table], validate)
    assert rows(reading)['Nodes in all']['value'] == 21 * 11 * 3
    assert len(project['Part'].elem_id) == 400
    reading, _view = run(project, 'project.mesh_solids', size=0.01)
    assert 'the 400 elements' in rows(reading)['Replaces']['value']


def test_a_plate_meshes_into_shells_only(project):
    run(project, 'project.add_solid_plate', width_x=0.1, width_y=0.05)
    reading, _view = run(project, 'project.mesh_solids', elements='quads', size=0.01)
    assert rows(reading)['Shells']['value'] == 50
    refused(project, 'project.mesh_solids', 'is flat', elements='hexes', size=0.01)


def test_a_fit_likely_to_fold_is_meshed_only_when_asked(project):
    run(project, 'project.add_solid_sphere', radius=0.05)
    reading, _view = run(project, 'project.mesh_solids', size=0.05)
    fit = rows(reading)['Fit']
    assert fit['status'] == 'warn' and '1.0 bricks across its radius' in fit['value']
    assert '0.01 m is likely to hold' in fit['value']
    with pytest.raises(ProtocolError, match='likely to fold') as error:
        run(project, 'project.mesh_solids', size=0.05, keep=True)
    assert error.value.code == 'refused' and project['Part'].num_nodes == 0


def stacked(project):
    block(project, name='base')
    block(project, z=0.01, name='lid')
    run(project, 'project.mesh_solids', size=0.025, parts_apart=True, keep=True)


def test_mated_surfaces_are_found_kept_edited_and_deleted(project, validate):
    stacked(project)
    reading, patches, view, mates = run(project, 'project.mate_surfaces')
    wire_views([reading, patches, view, mates], validate)
    assert rows(reading)['Patches found']['value'] == 1.0
    assert patches['rows'][0][3] == '15 on 15' and mates['rows'] == []
    reading, _patches, _view, mates = run(project, 'project.mate_surfaces', keep=True,
                                          normal='stiffness', normal_stiffness=2e6)
    assert mates['rows'][0][2] == 'base on lid'
    held = project['Part'].mates[0]
    assert held.normal_stiffness == 2e6 and held.shear_mode == 'rigid'
    reading, mates = run(project, 'project.edit_mated_surfaces', mate=1, shear='free',
                         name='seat')
    assert rows(reading)['Saved']['value'] == 'seat (normal rigid, shear free)'
    refused(project, 'project.edit_mated_surfaces', 'holds nothing', mate=1, normal='free',
            shear='free')
    first = project['Part'].mates[0]
    reading, view = run(project, 'project.edit_mate_sides', mate=1)
    held = view['pick']
    assert held['ids'][:len(first.nodes_a)] != [] and held['count'] == 1
    on = len(held['ids']) - held['off']
    assert on == len(first.nodes_a), 'the side held is what shows as in'
    node = held['ids'][0]
    reading, view = run(project, 'project.edit_mate_sides', mate=1, toggle=str(node))
    assert node not in project['Part'].mates[0].nodes_a
    assert view['pick']['off'] == held['off'] + 1
    run(project, 'project.edit_mate_sides', mate=1, toggle=str(node))
    assert node in project['Part'].mates[0].nodes_a, 'a second click puts it back'
    refused(project, 'project.edit_mate_sides', 'not 9', mate=9)
    refused(project, 'project.mate_surfaces', 'not 9', groups='9')
    refused(project, 'project.delete_mated_surfaces', 'not 4', mates='4')
    reading, mates = run(project, 'project.delete_mated_surfaces', mates='1')
    assert rows(reading)['Deleted']['value'] == 1.0 and mates['rows'] == []


def test_the_default_view_is_set_and_reset(project, validate):
    block(project)
    reading, view = run(project, 'project.set_view', eye_x=1.0, eye_y=1.0, eye_z=-1.0,
                        up_y=1.0, up_z=0.0)
    wire_views([reading, view], validate)
    assert tuple(project['Part'].view.eye) == pytest.approx((1.0, 1.0, -1.0))
    run(project, 'project.set_view')
    assert project['Part'].view is None
    refused(project, 'project.set_view', 'line of sight', eye_z=1.0, up_z=1.0)


def test_the_solids_are_listed(project, validate):
    wire_views(run(project, 'project.solids'), validate)
    block(project)
    reading, view, table = run(project, 'project.solids')
    wire_views([reading, view, table], validate)
    assert rows(reading)['Parts']['value'] == 1.0 and table['rows'][0][0] == '1'


def test_move_part_offers_arrows_and_rings(project, validate):
    moved = run(project, 'project.add_solid_block', x=0.5)
    wire_views(moved, validate)
    ids = str(project['Part'].solids[-1].id)
    views = run(project, 'project.move_solids', solids=ids)
    wire_views(views, validate)
    gizmo = next(view for view in views if view['view'] == 'shape3d')['gizmo']
    assert gizmo['origin'] == pytest.approx([0.5, 0.0, 0.0])
    assert gizmo['slide']['values']['x'] == 'offset_x'
    assert gizmo['turn']['values']['z'] == 'turn_z'
    run(project, gizmo['slide']['op'], **gizmo['slide']['params'], offset_x=0.25)
    views = run(project, 'project.move_solids', solids=ids)
    gizmo = next(view for view in views if view['view'] == 'shape3d')['gizmo']
    assert gizmo['origin'] == pytest.approx([0.75, 0.0, 0.0]), 'the handles follow'


def test_a_form_previews_on_a_copy(validate):
    """VD's live preview of a solid: nothing reaches the project or its journal."""
    import hertzforge
    from hertzforge.service import display_units, server

    project = hertzforge.Project('p')
    name = project.new_geometry('Part', unit='m')
    project.add_solid_block(name, (0, 0, 0), (1, 1, 1))
    held = len(project[name].solids)
    journal = list(project.journal)
    scratch = server._preview_copy(project, name)
    assert next(o for o in server.OPERATIONS if o.id == 'project.add_solid_block').previews
    views = server._said_as_preview(run(scratch, 'project.add_solid_block', x=2.0),
                                    display_units.DEFAULT)
    wire_views(views, validate)
    assert views[0]['rows'][0]['label'] == 'Would add'
    assert len(scratch[name].solids) == held + 1
    assert len(project[name].solids) == held and project.journal == journal


def test_a_preview_over_the_socket_leaves_the_project_unmodified():
    import asyncio

    from .test_projects import _exchange

    block = {'geometry': 'Part', 'x': 2.0, 'y': 0.0, 'z': 0.0, 'width_x': 1.0, 'width_y': 1.0,
             'width_z': 1.0, 'angle_x': 0.0, 'angle_y': 0.0, 'angle_z': 0.0, 'corner': 0.0,
             'name': ''}
    responses = asyncio.run(_exchange([
        ('project.new', {}),
        ('op.run', {'op': 'project.new_geometry', 'params': {'name': 'Part', 'unit': 'm'}}),
        ('op.run', {'op': 'project.add_solid_block', 'params': block, 'preview': True}),
        ('op.run', {'op': 'project.new_channel_table', 'params': {'name': 'x'}, 'preview': True}),
        ('op.run', {'op': 'project.add_solid_block', 'params': block}),
    ]))
    results = [got[-1] for got in responses]
    previewed = results[2]['result']
    assert 'project' not in previewed
    assert previewed['views'][0]['rows'][0]['label'] == 'Would add'
    assert 'Would add' not in str(results[4]['result']['views'][0])
    assert 'x' in [o['name'] for o in results[3]['result']['project']['objects']], (
        'an operation with no preview runs for real')


def _union_in_pieces():
    """A slab with a tilted hole, which is fitted, and a bar swept on top of it: two pieces."""
    import hertzforge

    project = hertzforge.Project('p')
    name = project.new_geometry('Part', unit='in')
    block = project.add_solid_block(name, (0, 0, 0), (4, 2, 1), name='slab', unit='in')
    tilted = project.add_solid_cylinder(name, (0, 0, 0), 0.6, 3.0, angles=(30, 0, 0), unit='in')
    cut = project.combine_solids(name, 'subtract', [block, tilted], 'slab')
    bar = project.add_solid_block(name, (1.5, 0, 1.0), (1, 2, 1), name='bar', unit='in')
    return project, project.combine_solids(name, 'union', [cut, bar], 'both'), cut, bar


def test_mesh_solids_takes_a_size_per_piece(monkeypatch):
    """VD's table: a union meshed in pieces lists a row a piece, each takes its own size,
    and Detect fills every row with the size that piece is likely to hold at. The fit
    itself is the core's test, stood in for here."""
    from hertzforge.service import ops_solids

    monkeypatch.setattr(ops_solids, '_preview', lambda *_a, **_k: (None, []))
    project, both, cut, bar = _union_in_pieces()
    params = {'solids': str(both), 'size': 0.01}
    views = run(project, 'project.mesh_solids', **params)
    pieces = next(view for view in views if view.get('title') == 'Pieces')
    assert {int(row[0]) for row in pieces['rows']} == {cut, bar}
    assert all(float(row[3]) == 0.01 for row in pieces['rows'])
    views = run(project, 'project.mesh_solids', sizes=f'{bar}: 0.005', **params)
    pieces = next(view for view in views if view.get('title') == 'Pieces')
    assert {int(row[0]): float(row[3]) for row in pieces['rows']} == {cut: 0.01, bar: 0.005}
    refused(project, 'project.mesh_solids', 'a piece and', sizes='3', **params)
    refused(project, 'project.mesh_solids', 'a piece and', sizes='3: 0', **params)


def test_detect_fills_each_piece_and_the_sizes_reach_the_mesh(monkeypatch):
    project, both, cut, bar = _union_in_pieces()
    asked = {}
    monkeypatch.setattr(project, 'mesh_solids', lambda *_a, **kw: asked.update(kw) or {
        'nodes': 0, 'elements': 0, 'replaced': 0, 'groups': []})
    with pytest.raises((KeyError, TypeError)):
        run(project, 'project.mesh_solids', solids=str(both), size=0.01, detect=True, keep=True,
            anyway=True)
    likely = asked['sizes']
    assert set(likely) == {cut, bar} and all(size > 0 for size in likely.values())
    with pytest.raises((KeyError, TypeError)):
        run(project, 'project.mesh_solids', solids=str(both), size=0.01, sizes=f'{bar}: 0.005',
            keep=True, anyway=True)
    assert asked['sizes'] == {bar: 0.005}


def _meshed_block(project, size=0.5, **given):
    run(project, 'project.add_solid_block', width_x=1, width_y=1, width_z=1, **given)
    run(project, 'project.mesh_solids', solids='1', size=size, keep=True)
    return project['Part']


@pytest.mark.parametrize('rename', ['group', 'solid'])
def test_meshing_again_after_a_rename_replaces_the_mesh(project, rename):
    geometry = _meshed_block(project)
    assert len(geometry.elem_id) == 8
    if rename == 'group':
        run(project, 'project.set_entity', kind='groups', entity=1, field='name', value='Housing')
    else:
        run(project, 'project.edit_solid', solid=1, name='Housing')
    (reading, *_rest) = run(project, 'project.mesh_solids', size=0.25)
    assert 'the 8 elements' in rows(reading)['Replaces']['value']
    (reading, *_rest) = run(project, 'project.mesh_solids', size=0.25, keep=True)
    assert len(geometry.elem_id) == 64, 'the old mesh does not stay beside the new one'
    assert len(geometry.group_id) == 1
    assert geometry.group_solids == {1: (1,)}
    assert rows(reading)['Replaced']['value'] == 'the 8 elements meshed before'


def test_a_group_renamed_keeps_its_name_and_material_when_meshed_again(project):
    geometry = _meshed_block(project)
    run(project, 'project.set_entity', kind='groups', entity=1, field='name', value='Housing')
    held = GroupProperties(material('6061-T6'))
    geometry.group_properties[1] = held
    run(project, 'project.mesh_solids', size=0.25, keep=True)
    assert list(geometry.group_name) == ['Housing']
    assert geometry.group_properties[1] == held


def test_two_parts_of_one_name_meshed_in_turn_keep_their_own_meshes(project):
    geometry = project['Part']
    for x in (0.0, 5.0):
        run(project, 'project.add_solid_block', x=x, width_x=1, width_y=1, width_z=1, name='bar')
    run(project, 'project.mesh_solids', solids='1', size=0.5, keep=True)
    (reading, *_rest) = run(project, 'project.mesh_solids', solids='2', size=0.5, keep=True)
    assert 'Replaced' not in rows(reading)
    assert len(geometry.elem_id) == 16
    assert geometry.node_xyz[:, 0].min() == -0.5 and geometry.node_xyz[:, 0].max() == 5.5
    assert geometry.group_solids == {1: (1,), 2: (2,)}
    (reading, *_rest) = run(project, 'project.mesh_solids', solids='1', size=0.25, keep=True)
    assert len(geometry.elem_id) == 64 + 8, 'the other part is left alone'


def test_two_parts_of_one_name_meshed_together_are_one_group_meshed_together(project):
    geometry = project['Part']
    for x in (0.0, 1.0):
        run(project, 'project.add_solid_block', x=x, width_x=1, width_y=1, width_z=1, name='bar')
    run(project, 'project.mesh_solids', size=0.5, keep=True)
    assert list(geometry.group_name) == ['bar'] and geometry.group_solids == {1: (1, 2)}
    refused(project, 'project.mesh_solids', 'also holds the mesh of bar', solids='1', size=0.25,
            keep=True)
    refused(project, 'project.mesh_solids', 'also holds the mesh of bar', solids='1', size=0.25)
    assert len(geometry.elem_id) == 16
    run(project, 'project.mesh_solids', size=0.25, keep=True)
    assert len(geometry.elem_id) == 128


def test_a_solid_deleted_leaves_its_mesh_for_a_new_solid_of_its_id(project):
    geometry = _meshed_block(project)
    run(project, 'project.delete_solids', solids='1')
    assert geometry.group_solids == {1: ()}
    new = run(project, 'project.add_solid_block', x=5.0, width_x=1, width_y=1, width_z=1)
    assert new and geometry.solids[0].id == 1, 'the id is taken again'
    (reading, *_rest) = run(project, 'project.mesh_solids', size=0.5, keep=True)
    assert 'Replaced' not in rows(reading)
    assert len(geometry.elem_id) == 16 and len(geometry.group_id) == 2


def test_a_mesh_stays_tied_to_its_solid_through_a_save(project, tmp_path):
    import h5py

    from hertzforge.io import native

    geometry = _meshed_block(project)
    path = tmp_path / 'part.hforge'
    native.save(geometry, path)
    assert native.load(path).group_solids == {1: (1,)}
    with h5py.File(path, 'r+') as f:
        holder = next(g for g in f.values() if 'group_solids' in g.attrs)
        del holder.attrs['group_solids']
    older = native.load(path)
    assert older.group_solids == {}, 'a file from before the tie reads back with none'
    project.add('Older', older)
    (reading, *_rest) = run(project, 'project.mesh_solids', geometry='Older', size=0.25, keep=True)
    assert len(older.elem_id) == 64 and rows(reading)['Replaced']
    assert older.group_solids == {1: (1,)}, 'found by its name, and tied from then on'


def test_a_merged_or_renumbered_group_keeps_what_it_was_meshed_from(project):
    geometry = project['Part']
    for x in (0.0, 5.0):
        run(project, 'project.add_solid_block', x=x, width_x=1, width_y=1, width_z=1)
    run(project, 'project.mesh_solids', size=0.5, keep=True)
    run(project, 'project.set_entity', kind='groups', entity=2, field='id', value='7')
    assert geometry.group_solids == {1: (1,), 7: (2,)}
    geometry.merge_groups([1, 7])
    assert geometry.group_solids == {1: (1, 2)}
    geometry.delete_groups([1])
    assert geometry.group_solids == {}


def test_renumbering_a_group_takes_its_properties_with_it(project):
    geometry = _meshed_block(project)
    held = GroupProperties(material('6061-T6'))
    geometry.group_properties[1] = held
    run(project, 'project.set_entity', kind='groups', entity=1, field='id', value='7')
    assert geometry.group_properties == {7: held}
    assert geometry.add_group('new', 1) == 1
    assert 1 not in geometry.group_properties, 'a later group of the old id inherits nothing'


@pytest.fixture
def undefined():
    from hertzforge.core.geometry import Geometry

    project = Project()
    project.add('Raw', Geometry(node_id=[], node_xyz=np.empty((0, 3))))
    assert not project['Raw'].units_defined
    return project


@pytest.mark.parametrize('kind', ['block', 'cylinder', 'section'])
@pytest.mark.parametrize('keep', [False, True])
def test_a_single_turned_solid_meshes_swept_along_its_own_axis(project, kind, keep):
    given = {'angle_x': 30.0, 'angle_z': 20.0}
    if kind == 'block':
        block(project, width_x=1.0, width_y=0.5, width_z=0.25, **given)
    elif kind == 'cylinder':
        run(project, 'project.add_solid_cylinder', radius=0.25, length=1.0, **given)
    else:
        run(project, 'project.add_solid_section', shape='rectangular tube',
            dimensions='1 0.5 0.1', **given)
    (reading, *_rest) = run(project, 'project.mesh_solids', size=0.1, keep=keep)
    assert rows(reading)['Method']['value'] == 'swept along its own axis'
    assert bool(project['Part'].num_nodes) == keep


@pytest.mark.parametrize('add', [
    {'op': 'project.add_solid_plate', 'width_x': 2.0, 'width_y': 1.0},
    {'op': 'project.add_solid_disk', 'radius': 0.5}], ids=['plate', 'disk'])
def test_move_part_moves_a_flat_part_and_gives_it_a_gizmo(project, add):
    given = dict(add)
    run(project, given.pop('op'), **given)
    (_reading, view, _table) = run(project, 'project.move_solids', solids='1')
    assert view['gizmo']['origin'] == [0.0, 0.0, 0.0], 'the middle of its extent'
    (_reading, view, _table) = run(project, 'project.move_solids', solids='1', offset_z=1.0)
    assert project['Part'].solids[0].center == (0.0, 0.0, 1.0)
    assert view['gizmo']['origin'] == [0.0, 0.0, 1.0]
    run(project, 'project.move_solids', solids='1', turn_x=90.0)
    assert project['Part'].solids[0].center == pytest.approx((0.0, 0.0, 1.0))


def test_a_flat_part_turned_reaches_as_far_as_its_edges(project):
    from hertzforge.core import solids

    run(project, 'project.add_solid_plate', width_x=2.0, width_y=1.0, angle_x=90.0)
    run(project, 'project.add_solid_disk', radius=0.5, x=5.0, angle_y=90.0)
    low, high = solids._box(project['Part'], 1)
    assert low == pytest.approx([-1.0, 0.0, -0.5]) and high == pytest.approx([1.0, 0.0, 0.5])
    low, high = solids._box(project['Part'], 2)
    assert low == pytest.approx([5.0, -0.5, -0.5]) and high == pytest.approx([5.0, 0.5, 0.5])


def test_detect_on_parts_that_mesh_as_one_says_so(project):
    for x in (0.0, 5.0):
        run(project, 'project.add_solid_block', x=x, width_x=1, width_y=1, width_z=1)
    (reading, *_rest) = run(project, 'project.mesh_solids', detect=True, size=0.5)
    assert 'mesh as one' in rows(reading)['Detect']['value']
    assert not project['Part'].num_nodes
    (reading, *_rest) = run(project, 'project.mesh_solids', detect=True, size=0.5, keep=True)
    assert len(project['Part'].elem_id) == 16


def test_a_size_for_a_piece_that_is_not_there_is_refused(monkeypatch, project):
    from hertzforge.service import ops_solids

    monkeypatch.setattr(ops_solids, '_preview', lambda *_a, **_k: (None, []))
    run(project, 'project.add_solid_block', width_x=1, width_y=1, width_z=1)
    refused(project, 'project.mesh_solids', 'mesh as one', sizes='1: 0.5', size=0.5)
    in_pieces, both, _cut, _bar = _union_in_pieces()
    refused(in_pieces, 'project.mesh_solids', 'no piece 99', solids=str(both), size=0.01,
            sizes='99: 0.01')


def _spheres(project):
    for x in (0.0, 5.0):
        run(project, 'project.add_solid_sphere', x=x, radius=0.5)


def test_the_fold_check_reads_each_piece_at_its_own_size(monkeypatch, project):
    from hertzforge.service import ops_solids

    _spheres(project)
    monkeypatch.setattr(ops_solids, '_preview', lambda *_a, **_k: (None, []))
    (reading, *_rest) = run(project, 'project.mesh_solids', size=0.2, sizes='1: 0.1, 2: 0.1')
    assert 'Fit' not in rows(reading), 'both pieces are given a size they hold at'
    (reading, *_rest) = run(project, 'project.mesh_solids', size=0.1, sizes='1: 0.3')
    assert 'at 0.3 m the fit is likely to fold' in rows(reading)['Fit']['value']


def test_sizes_detected_can_be_kept_without_asking_to_mesh_anyway(monkeypatch, project):
    _spheres(project)
    asked = {}
    monkeypatch.setattr(project, 'mesh_solids', lambda *_a, **kw: asked.update(kw) or {
        'bricks': 0, 'nodes': 0, 'axis': 0, 'added': 0, 'shared': 0, 'replaced': 0, 'groups': []})
    run(project, 'project.mesh_solids', size=0.2, detect=True, keep=True)
    assert set(asked['sizes']) == {1, 2}


def test_a_piece_with_a_hole_reads_its_likely_size_with_the_hole(monkeypatch, project):
    from hertzforge.core import solids
    from hertzforge.service import ops_solids

    monkeypatch.setattr(ops_solids, '_preview', lambda *_a, **_k: (None, []))
    block(project, width_x=0.4, width_y=0.2, width_z=0.1)
    block(project, x=0.1, z=0.1, width_x=0.1, width_y=0.2, width_z=0.1)
    run(project, 'project.combine_solids', solids='1, 2', operation='union')
    run(project, 'project.add_solid_cylinder', x=-0.1, radius=0.005, length=0.3, angle_x=30.0)
    run(project, 'project.combine_solids', solids='3, 4', operation='subtract')
    geometry = project['Part']
    views = run(project, 'project.mesh_solids', solids='5', size=0.02)
    table = next(view for view in views if view.get('title') == 'Pieces')
    likely = {int(row[0]): float(row[4]) for row in table['rows']}
    for entry in solids.piece_plan(geometry, [5], 0.02):
        assert entry['cuts'] and likely[entry['id']] < ops_solids._likely_size(
            geometry, [entry['id']], 'hexes')
    assert table['columns'][3:5] == ['Size [m]', 'Likely size [m]']
    views = run(project, 'project.mesh_solids', solids='5', size=0.02, detect=True)
    table = next(view for view in views if view.get('title') == 'Pieces')
    assert {int(row[0]): float(row[3]) for row in table['rows']} == likely


def test_an_undefined_geometry_reports_its_numbers_without_a_unit(undefined):
    from hertzforge.service import display_units

    project = undefined
    assert not project['Raw'].units_defined
    run(project, 'project.add_solid_block', geometry='Raw', width_x=100.0, width_y=100.0,
        width_z=10.0)
    views = run(project, 'project.mesh_solids', geometry='Raw', size=10.0)
    assert views[-1]['unit'] == 'undefined'
    said = rows(views[0])
    assert said['Element size']['unit'] is None and said['Shortest edge']['unit'] is None
    display_units.convert(views, display_units.START)
    said = rows(views[0])
    assert said['Element size']['value'] == 10.0 and said['Element size']['unit'] is None
    assert said['Shortest edge']['value'] == 10.0


def test_an_undefined_geometry_says_a_refusal_and_a_fold_without_a_unit(monkeypatch, undefined):
    from hertzforge.service import ops_solids

    project = undefined
    monkeypatch.setattr(ops_solids, '_preview', lambda *_a, **_k: (None, []))
    for x in (0.0, 5.0):
        run(project, 'project.add_solid_sphere', geometry='Raw', x=x, radius=0.5)
    (reading, *_rest) = run(project, 'project.mesh_solids', geometry='Raw', size=0.3)
    assert ' m ' not in rows(reading)['Fit']['value']
    views = run(project, 'project.mesh_solids', geometry='Raw', size=0.3)
    table = next(view for view in views if view.get('title') == 'Pieces')
    assert table['columns'][3:5] == ['Size', 'Likely size']
    run(project, 'project.mesh_solids', geometry='Raw', size=0.1, anyway=True, keep=True)
    with pytest.raises(ProtocolError) as error:
        run(project, 'project.mate_surfaces', geometry='Raw', tolerance=0.001)
    assert not str(error.value).endswith(' m')


def test_meshing_again_says_the_mated_surfaces_it_deleted(project):
    for z in (0.0, 1.0):
        run(project, 'project.add_solid_block', z=z, width_x=1, width_y=1, width_z=1)
    run(project, 'project.mesh_solids', size=0.5, parts_apart=True, keep=True)
    run(project, 'project.mate_surfaces', keep=True)
    assert len(project['Part'].mates) == 1
    (reading, *_rest) = run(project, 'project.mesh_solids', size=0.5, parts_apart=True, keep=True)
    assert not project['Part'].mates
    said = rows(reading)['Mated surfaces']
    assert '1 on the old mesh' in said['value'] and said['status'] == 'warn'
