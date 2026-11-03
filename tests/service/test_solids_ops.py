"""Solid modeling: a new geometry, its solids added, changed and combined,
Mesh Solids, Mate Surfaces and the default view."""

from __future__ import annotations

import math

import numpy as np
import pytest

from hertzforge.project import Project
from hertzforge.service import projects
from hertzforge.service.ops_solids import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .test_ops import wire_views

OPS = {op.id: op for op in OPERATIONS}


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
        assert op.workspace == 'model' and op.accepts == 'project'


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
    refused(project, 'project.move_solids', 'how far', solids='3')


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
