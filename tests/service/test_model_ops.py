"""The Model workspace's tools: a geometry on its own, what its element
groups are made of, and its coincident nodes merged."""

from __future__ import annotations

from pathlib import Path

import pytest

from hertzforge.service import projects
from hertzforge.service.ops_model import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .test_ops import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in OPERATIONS}


@pytest.fixture
def project():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'geometry.unv'),
                                           'units': {'length_unit': 'mm'}})
    return state.project


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()})
    return op.run(project, values)


def test_the_descriptors_validate(validate):
    for op in OPERATIONS:
        validate(op.to_json(), 'operation')
        assert op.workspace == 'model' and op.accepts == 'project'


def test_a_geometry_is_drawn_on_its_own(project, validate):
    (view,) = run(project, 'project.view_geometry')
    wire_views([view], validate)
    assert view['view'] == 'shape3d' and view['modes'] == []
    assert len(view['nodes']) == len(project['Geometry'].node_id)


def test_plates_take_a_material_and_thickness_and_then_solve(project, validate):
    assert 'solve_modes' not in projects.verbs_of(project, 'Geometry')
    (reading,) = run(project, 'project.group_properties', material='7075-T6', thickness=6.35)
    wire_views([reading], validate)
    assert reading['rows'][0]['value'] == '7075-T6, 6.35 mm'
    held = project['Geometry'].group_properties[1]
    assert held.material.name == '7075-T6' and held.thickness == pytest.approx(0.00635)
    assert 'solve_modes' in projects.verbs_of(project, 'Geometry')
    shapes = project[project.solve_modes('Geometry', num_modes=8)]
    assert shapes.num_shapes == 8 and shapes.frequency[7] > 0
    assert OPS['project.group_properties'].edits


@pytest.mark.parametrize(('given', 'match'), [
    ({'groups': '7'}, 'has element groups 1; not 7'),
    ({'groups': 'one'}, 'not an element group number'),
    ({'thickness': 0.0}, 'thickness above 0'),
])
def test_what_the_groups_cannot_take_is_refused(project, given, match):
    with pytest.raises(ProtocolError, match=match) as error:
        run(project, 'project.group_properties', **given)
    assert error.value.code == 'refused'
    assert project['Geometry'].group_properties == {}


def test_coincident_nodes_are_merged_and_counted(project):
    (reading,) = run(project, 'project.merge_coincident_nodes')
    rows = {row['label']: row['value'] for row in reading['rows']}
    assert rows['Nodes before'] == rows['Nodes now'] + rows['Nodes merged away']


def _parts():
    """A floor plate and, 0.25 above it, a smaller foot plate, two element
    groups with nothing shared (the core's tie test), in a project."""
    import hertzforge
    from hertzforge import mesh
    from hertzforge.core import fem

    geometry = mesh.assemble(
        mesh.plane((0, 0, 0), (4, 0, 0), (0, 4, 0), 1, 'floor'),
        mesh.plane((1, 1, 0.25), (2, 0, 0), (0, 2, 0), 1, 'foot'))
    aluminum = fem.material('6061-T6')
    geometry.group_properties = {1: fem.GroupProperties(aluminum, 0.01)}
    project = hertzforge.Project('p')
    project.add('Plate', geometry)
    return project


def test_the_element_groups_table_lists_each_group(validate):
    project = _parts()
    (view,) = run(project, 'project.element_groups', geometry='Plate')
    wire_views([view], validate)
    named = [dict(zip(view['columns'], row, strict=True)) for row in view['rows']]
    assert [(g['Element group'], g['Name'], g['Elements are'], g['Count']) for g in named] == [
        ('1', 'floor', 'face', '16'), ('2', 'foot', 'face', '4')]
    assert named[0]['Material'] == '6061-T6' and float(named[0]['Thickness [m]']) == 0.01
    assert named[1]['Material'] == '' and named[1]['Thickness [m]'] == ''


def test_merging_element_groups_keeps_the_first(validate):
    project = _parts()
    with pytest.raises(ProtocolError, match='differ in material, thickness or section'):
        run(project, 'project.merge_element_groups', geometry='Plate', groups='1, 2')
    run(project, 'project.group_properties', geometry='Plate', groups='2',
        material='6061-T6', thickness=10.0)
    (view,) = run(project, 'project.merge_element_groups', geometry='Plate', groups='1, 2')
    assert [[row[0], row[1], row[3], row[4]] for row in view['rows']] == [['1', 'floor', 'face', '20']]
    with pytest.raises(ProtocolError, match='two or more'):
        run(project, 'project.merge_element_groups', geometry='Plate', groups='1')


def test_deleting_an_element_group_takes_its_elements():
    project = _parts()
    (view,) = run(project, 'project.delete_element_groups', geometry='Plate', groups='2')
    assert [row[1] for row in view['rows']] == ['floor']
    with pytest.raises(ProtocolError, match='delete the geometry instead'):
        run(project, 'project.delete_element_groups', geometry='Plate', groups='1')


def test_a_patch_ties_to_a_group_by_name():
    project = _parts()
    patch = project['Plate'].elements_in('foot')[:1]
    (reading,) = run(project, 'project.tie_elements', geometry='Plate',
                     elements=str(int(patch[0])), to='floor')
    found = {row['label']: row['value'] for row in reading['rows']}
    assert found['Links added'] == 4 and found['Links in group'] == '3'
    with pytest.raises(ProtocolError, match='name what the patch ties to'):
        run(project, 'project.tie_elements', geometry='Plate', elements='1')


def test_align_meshes_moves_the_coarser_mesh_onto_the_finer():
    import hertzforge
    from hertzforge import mesh

    fine = mesh.block((0, 0, -0.5), (1, 0, 0), (0, 1, 0), (0, 0, 0.5), 0.25, 'frame',
                      unit='in')
    coarse = mesh.block((0.02, 0.01, 0.05), (1, 0, 0), (0, 1, 0), (0, 0, 1 / 3), 1 / 3,
                        'wing', unit='in')
    project = hertzforge.Project('p')
    project.add('Wing', mesh.assemble(fine, coarse, apart=True))
    (reading,) = run(project, 'project.align_meshes', geometry='Wing')
    found = {row['label']: row['value'] for row in reading['rows']}
    assert found['Nodes moved'] == 16
    assert 0 < found['Farthest move'] <= 0.5 / 3 * 25.4
