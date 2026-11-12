"""Planning a modal test and handing its model on: Plan Instrumentation,
Export State-Space Model, Project onto Picked Channels and Animate the
Projection, on the plate's modal run, its geometry and a model of it."""

from __future__ import annotations

import warnings
from pathlib import Path

import numpy as np
import pytest

from hertzforge.core.fem import GroupProperties, Material
from hertzforge.service import projects
from hertzforge.service.ops_modal_plan import OPERATIONS
from hertzforge.service.ops_project import OPERATIONS as PROJECT_OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in (*OPERATIONS, *PROJECT_OPERATIONS)}
MODEL = 'Geometry Modes'


@pytest.fixture
def plate():
    """The plate's modal run and geometry, as a person would import them,
    with the geometry made a plate model and its modes solved. The
    geometry's coordinates come in a thousandth of the plate's size, so
    its thickness is too."""
    state, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    projects.import_file(state, {'path': str(PLATE / 'geometry.unv'),
                                 'units': {'length_unit': 'mm'}})
    project = state.project
    geometry = project['Geometry']
    geometry.group_properties = {int(geometry.group_id[0]): GroupProperties(
        Material('aluminum', youngs_modulus=70e9, density=2700, poissons_ratio=0.3), 6.35e-6)}
    assert project.solve_modes('Geometry', num_modes=12) == MODEL
    return project


def fitted(project):
    frfs = project.compute_frfs('Time History', 'H1')
    return project.fit_modes(frfs, bounds=(100.0, 1300.0), limit=4)


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return op.run(project, values)


def test_a_flat_plate_opens_on_enough_sensors_to_tell_its_modes_apart():
    from hertzforge import Project
    from hertzforge.core import fem

    material = fem.Material('al', youngs_modulus=70e9, density=2700, poissons_ratio=0.3)
    columns, rows = 12, 8
    model = fem.Model('plate')
    node = lambda i, j: i * (rows + 1) + j + 1
    for i in range(columns + 1):
        for j in range(rows + 1):
            model.add_node(node(i, j), i * 0.6 / columns, j * 0.4 / rows, 0.0)
    for i in range(columns):
        for j in range(rows):
            model.add_plate((node(i, j), node(i + 1, j), node(i + 1, j + 1), node(i, j + 1)),
                            material, 0.005)
    project = Project()
    geometry = project.add('G', model.geometry())
    project.link(geometry, project.add('S', model.eigensolution(num_modes=12, damping=0.01)))
    views = run(project, 'project.plan_instrumentation', source='S', keep=False)
    reading = next(view for view in views if view['view'] == 'reading')
    said = {row['label']: row['value'] for row in reading['rows']}
    assert said['Sensors'].startswith('7 tri-axial')
    assert 'the 1 direction they move in' in said['Sensors']


def test_the_descriptors_validate(validate):
    for op in OPERATIONS:
        validate(op.to_json(), 'operation')
        assert op.workspace == 'modal'
        assert 'channel' not in {param.name for param in op.params}


# ---- Plan Instrumentation ---------------------------------------------------------

def test_a_plan_is_the_cores_choice_and_reads_as_the_pane_does(plate, validate):
    from hertzforge.core.instrumentation import elastic, plan

    views = run(plate, 'project.plan_instrumentation', uniaxial=8, edge_distance=0.0)
    wire_views(views, validate)
    shape, reading, matrix, sensors, surfaces = views
    made = plan(plate[MODEL], plate['Geometry'], 0, 8, edge_distance=0.0)
    rows = {row['label']: row['value'] for row in reading['rows']}
    assert rows['Condition number'] == pytest.approx(made.placement.condition)
    mac = made.shapes.auto_mac()
    assert rows['Worst off-diagonal MAC'] == pytest.approx((mac - np.diag(np.diag(mac))).max())
    assert rows['Channels'] == 8.0 and rows['Sensors'] == '0 tri-axial and 8 uni-axial'
    assert [row[1] for row in sensors['rows']] == [str(n) for n in made.geometry.node_id]
    assert np.allclose(np.asarray(matrix['values']), mac, atol=1e-6)
    assert len(shape['modes']) == len(elastic(plate[MODEL]))
    from hertzforge.service.mesh import drawn_mesh

    edges = len(drawn_mesh(plate['Geometry'])[2])
    assert shape['lines'].shape == (edges + 8, 2), 'the elements\' edges and a line per uni-axial sensor'
    assert surfaces['rows'][0][-1] == 'yes'
    assert 'Not kept' in rows and len(list(plate)) == 4


def test_a_click_on_a_surface_leaves_it_out_or_puts_it_back(plate, validate):
    shape, *_rest, surfaces = run(plate, 'project.plan_instrumentation', uniaxial=8,
                                  edge_distance=0.0)
    wire_views([shape], validate)
    pick = shape['pick']
    assert pick['target'] == 'faces' and pick['out'] == 'leave_out'
    assert len(pick['faces']) == len(shape['triangles']), 'a surface a triangle'
    kept = [int(row[0]) for row in surfaces['rows'] if row[-1] == 'yes']
    assert pick['kept'] == kept and set(kept) <= set(np.asarray(pick['faces']).tolist())
    assert pick['params']['keep'] is False, 'a click previews; it never keeps'
    assert kept == [1], 'the plate is one surface'
    with pytest.raises(ProtocolError, match='0 directions'):
        run(plate, pick['op'], **{**pick['params'], 'leave_out': '1'})
    shape, *_rest = run(plate, pick['op'], **{**pick['params'], 'put_back': '1'})
    assert shape['pick']['params']['put_back'] == '1' and shape['pick']['kept'] == [1]


def test_keep_adds_the_plans_three_objects(plate):
    (_shape, reading, *_rest) = run(plate, 'project.plan_instrumentation', triaxial=8,
                                    edge_distance=0.0, keep=True, name='Layout')
    assert {'Layout Geometry', 'Layout Channels', 'Layout Shapes'} <= set(plate)
    assert plate['Layout Channels'].num_channels == 24
    assert plate['Layout Geometry'].num_nodes == 8
    assert reading['rows'][-1]['label'] == 'Kept'


def test_left_out_surfaces_and_too_few_places_are_refused(plate):
    with pytest.raises(ProtocolError, match='tri-axial accelerometers but 0 places') as error:
        run(plate, 'project.plan_instrumentation')
    assert error.value.code == 'refused'
    with pytest.raises(ProtocolError, match='surfaces 1 to 1') as error:
        run(plate, 'project.plan_instrumentation', leave_out='3', edge_distance=0.0)
    with pytest.raises(ProtocolError, match='but 0 places'):
        run(plate, 'project.plan_instrumentation', leave_out='1', edge_distance=0.0)
    with pytest.raises(ProtocolError, match='both left out and put back'):
        run(plate, 'project.plan_instrumentation', leave_out='1', put_back='1')
    with pytest.raises(ProtocolError, match='modes run from 1 to 12'):
        run(plate, 'project.plan_instrumentation', modes='7-13', edge_distance=0.0)


def test_fewer_directions_than_modes_is_planned_for_the_lowest_and_said(plate):
    with warnings.catch_warnings():
        warnings.simplefilter('error')
        (_shape, reading, *_rest) = run(plate, 'project.plan_instrumentation', uniaxial=3,
                                        modes='7-10', edge_distance=0.0, keep=True)
    rows = {row['label']: row for row in reading['rows']}
    assert rows['Measuring']['value'].startswith('4 modes (7-10)')
    assert rows['Warning']['status'] == 'warn' and 'planned for the lowest 3' in (
        rows['Warning']['value'])
    assert rows['Kept']['status'] == 'pass'


# ---- Export State-Space Model -----------------------------------------------------

def test_the_model_previews_at_the_drive_points_and_writes_its_file(plate, validate, tmp_path):
    from hertzforge.core.state_space import state_space_model

    shapes = fitted(plate)
    path = tmp_path / 'plate'
    views = run(plate, 'project.export_state_space', shapes=shapes, table='Channel Table',
                maximum=1000.0, path=str(path))
    wire_views(views, validate)
    spectrum, reading = views
    rows = {row['label']: row['value'] for row in reading['rows']}
    assert rows['Modes'] == '3 of 4' and rows['Left out'] == 1.0 and rows['Inputs'] == 2.0
    held = np.load(tmp_path / 'plate.npz')
    assert held['A'].shape == (6, 6) and held['C'].shape == (13, 6)
    assert list(held['input_dof']) == ['101Z+', '1310Z+']
    model = state_space_model(plate[shapes], plate['Channel Table'], maximum_frequency=1000.0)
    line = spectrum['series'][0]
    assert line['name'] == '101Z+ / 101Z+, model'
    assert line['y'][100] == pytest.approx(abs(model.frf([line['x'][100]])[0, 0, 0]))
    assert spectrum['series'][1]['follows'] == 0
    assert spectrum['y']['unit'] == '(m/s²)/N'


def test_a_preview_writes_nothing_and_what_the_model_cannot_be_is_refused(plate, tmp_path):
    shapes = fitted(plate)
    (_spectrum, reading) = run(plate, 'project.export_state_space', shapes=shapes,
                               table='Channel Table')
    assert reading['rows'][-1]['value'].startswith('not written')
    assert reading['rows'][-2]['status'] == 'warn', 'every mode kept: says so'
    with pytest.raises(ProtocolError, match='drops every mode') as error:
        run(plate, 'project.export_state_space', shapes=shapes, table='Channel Table',
            maximum=100.0, path=str(tmp_path / 'never.npz'))
    assert error.value.code == 'refused' and not (tmp_path / 'never.npz').exists()
    with pytest.raises(ProtocolError, match='not a full path'):
        run(plate, 'project.export_state_space', shapes=shapes, table='Channel Table',
            path='relative.npz')


# ---- Projection --------------------------------------------------------------------

def test_a_set_projects_onto_picked_channels(plate, validate):
    views = run(plate, 'project.project_onto_channels', source=MODEL, table='Channel Table',
                rows='1-4, 11', tolerance=2.0)
    wire_views(views, validate)
    shape, reading = views
    added = f'{MODEL} @ Channel Table (5 channels)'
    projected = plate[added]
    assert list(projected.coordinate) == ['101Z+', '107Z+', '113Z+', '701Z+', '707X+']
    model = plate[MODEL]
    column = list(model.coordinate).index('707X+')
    assert np.allclose(projected.shape_matrix[:, 4], model.shape_matrix[:, column])
    rows = {row['label']: row['value'] for row in reading['rows']}
    assert rows['Nodes matched'] == '5 of 5' and rows['Worst distance'] == 0.0
    assert shape['view'] == 'shape3d'


def test_rows_outside_the_table_are_refused(plate):
    with pytest.raises(ProtocolError, match='rows run from 1 to 13') as error:
        run(plate, 'project.project_onto_channels', source=MODEL, table='Channel Table',
            rows='12-14')
    assert error.value.code == 'refused'


def test_project_onto_basis_takes_the_dialogs_tolerance(plate):
    fitted(plate)
    limits = []
    for tolerance in (0.5, 2.0):
        before = set(plate)
        (view,) = run(plate, 'project.project_onto_basis', source=MODEL, onto='Channel Table',
                      tolerance=tolerance)
        (added,) = set(plate) - before
        limits.append(plate[added].projection_report['limit'])
        assert view['view'] == 'shape3d'
    extent = limits[1] - limits[0]
    assert extent > 0 and limits[0] == pytest.approx(0.005 / 0.015 * extent + 0.5 * 6.35e-6)


def test_the_projection_overlays_a_set_from_another_geometry(plate, validate):
    shapes = fitted(plate)
    run(plate, 'project.plan_instrumentation', triaxial=8, edge_distance=0.0, keep=True)
    basis = f'{MODEL} Plan Shapes'
    views = run(plate, 'project.projection_overlay', first=basis, second=shapes)
    wire_views(views, validate)
    matrix, shape, reading = views
    assert matrix['selects'] == 'shape3d'
    assert np.allclose(np.asarray(matrix['values']), plate.comparison_mac(basis, shapes),
                       atol=1e-6)
    assert len(shape['overlays']) == plate[shapes].num_shapes
    assert len(shape['nodes']) == plate[f'{MODEL} Plan Geometry'].num_nodes
    assert reading['rows'][0]['value'] == '8 of 8'
    with pytest.raises(ProtocolError, match='both on Geometry') as error:
        run(plate, 'project.projection_overlay', first=MODEL, second=shapes)
    assert error.value.code == 'refused'
