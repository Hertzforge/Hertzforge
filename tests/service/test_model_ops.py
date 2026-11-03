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
