"""Fit Sensors: the preview of accelerometers fitted to measured points, and Apply."""

import sys
from pathlib import Path

import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from test_scan_fit import MM, _can, _cube

from hertzforge import Project
from hertzforge.io.point_files import points_geometry
from hertzforge.service import projects
from hertzforge.service.ops_scan import ON_SURFACE, OPERATIONS, UNFITTED
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

OP = OPERATIONS[0]


def _project(unit='mm'):
    """A 10.2 mm cube (sensor 31), a can (11), a 19 mm cube (40), one
    face alone (50) and a stray feature, as measured points."""
    _fa, center_a, _sa, faces_a = _cube(1, side=10.2 * MM)
    _fc, center_c, _sc, faces_c = _cube(3, side=19.0 * MM, count=30)
    _fd, center_d, _radius, _height, side, top = _can(4)
    features = {**{f'31_{k}': points for k, points in faces_a.items()},
                '11_c': side, '11_3': top,
                **{f'40_{k}': points for k, points in faces_c.items()},
                '50_1': faces_a['1'] + 1.0, 'datum': faces_a['2'] + 2.0}
    names = list(features)
    points = np.vstack([features[name] for name in names]) / MM
    owner = np.repeat(np.arange(len(names)), [len(features[name]) for name in names])
    project = Project()
    project.add('Probed', points_geometry(points, unit, names, owner))
    return project, {'31': center_a, '11': center_d, '40': center_c}


def run(project, **given):
    values = OP.resolve({'geometry': 'Probed', **given},
                        objects={name: projects.kind_of(obj) for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return OP.run(project, values)


def _rows(reading):
    return [(row['label'], row['value'], row['status']) for row in reading['rows']]


def test_the_preview_draws_the_fits_and_changes_nothing(validate):
    project, centers = _project()
    journal = list(project.journal)
    view, table, reading = run(project)
    wire_views([view, table, reading], validate)
    assert list(project) == ['Probed'] and project.journal == journal
    # every point drawn, colored by its distance from its surface: on it here
    (layer,) = view['points']
    assert len(layer['xyz']) == project['Probed'].num_nodes
    fitted = (layer['colors'] != np.uint8(UNFITTED)).any(axis=1)
    features = np.asarray(project['Probed'].elem_group)
    assert fitted[features <= 9].all()                        # every sensor's surfaces
    assert not fitted[features == 10].any()                   # the stray feature
    assert (layer['colors'][fitted] == ON_SURFACE).all()
    # a body for each of the three placed, and an arrow for each of their seven channels
    assert len(view['bodies']['triangles']) > 30
    assert view['arrows']['labels'] == ['31X+', '31Y+', '31Z+', '11Z+', '40X+', '40Y+', '40Z+']
    np.testing.assert_allclose(view['arrows']['tails'][0], centers['31'], atol=1e-6)
    # the can's height is not measured and is taken as its diameter, 8 mm for 6
    np.testing.assert_allclose(view['arrows']['tails'][3], centers['11'], atol=1.1e-3)
    assert table['columns'][0] == 'Sensor' and len(table['rows']) == 4
    by_sensor = {row[0]: row for row in table['rows']}
    assert by_sensor['31'][7:10] == ['356A33', '31', '1 2 3']
    assert by_sensor['11'][1] == 'uni-axial' and by_sensor['11'][7] == 'Cylinder 8 x 8 mm'
    assert by_sensor['40'][7].startswith('Block 17.')
    assert by_sensor['50'][7:10] == ['', '', ''] and 'a second face' in by_sensor['50'][10]
    read = _rows(reading)
    assert read[0] == ('Sensors', '3 placed of 4 measured', 'warn')
    assert any('datum is not named' in value for _l, value, _s in read)
    assert read[-1][0] == 'Not created'


def test_models_and_nodes_typed_change_the_preview():
    project, centers = _project()
    view, table, _reading = run(project, models='40=Block 19 mm; 11=cylinder 8x6 mm',
                                nodes='40=140')
    by_sensor = {row[0]: row for row in table['rows']}
    assert by_sensor['40'][7:9] == ['Block 19 mm', '140']
    assert by_sensor['11'][7] == 'cylinder 8x6 mm'
    assert '140X+' in view['arrows']['labels']
    # sized right, the 19 mm block's node is its center
    np.testing.assert_allclose(view['arrows']['tails'][4], centers['40'], atol=1e-6)
    for text, said in (('40', 'written sensor=value'), ('=356A02', 'written sensor=value')):
        with pytest.raises(ProtocolError, match=said):
            run(project, models=text)
    with pytest.raises(ProtocolError, match='whole number'):
        run(project, nodes='40=x')


def test_apply_creates_the_geometry_and_the_table():
    project, centers = _project()
    _view, _table, reading = run(project, keep=True, models='40=Block 19 mm', name='Frame')
    assert list(project) == ['Probed', 'Frame Geometry', 'Frame Channels']
    assert _rows(reading)[-1] == ('Created', 'Frame Geometry and Frame Channels', 'pass')
    assert project.journal[-1] == ("project.create_sensors('Probed', {'40': 'Block 19 mm'}, "
                                   "None, None, 'Frame')")
    geometry = project['Frame Geometry']
    assert geometry.node_id.tolist() == [31, 11, 40]
    np.testing.assert_allclose(geometry.node_xyz[2], centers['40'], atol=1e-8)
    assert project['Frame Channels'].num_channels == 7


def test_what_cannot_be_placed_is_said_in_the_preview_and_refused_on_apply():
    project, _centers = _project(unit=None)
    view, table, reading = run(project)
    assert 'bodies' not in view and 'arrows' not in view and len(table['rows']) == 4
    assert table['columns'][4] == 'Worst RMS [file units]'
    assert any(label == 'Not placed' and 'has no length unit' in value
               for label, value, _status in _rows(reading))
    with pytest.raises(ProtocolError, match='has no length unit'):
        run(project, keep=True)
    project, _centers = _project()
    _view, _table, reading = run(project, models='31=NOPE')
    assert any(label == 'Not placed' and "'NOPE' is not in the sensor catalog" in value
               for label, value, _status in _rows(reading))


def test_it_is_offered_on_a_geometry_of_measured_points_only():
    project, _centers = _project()
    project.add('Cloud', points_geometry(np.zeros((3, 3)) + [[0], [1], [2]]))
    assert 'create_sensors' in projects.verbs_of(project, 'Probed')
    assert 'create_sensors' not in projects.verbs_of(project, 'Cloud')
    with pytest.raises(ProtocolError):
        run(project, geometry='Cloud')
