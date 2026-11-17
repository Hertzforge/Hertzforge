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

OP, EDIT = OPERATIONS


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


def edit(project, row, column, value):
    values = EDIT.resolve({'geometry': 'Probed', 'row': row, 'column': column, 'value': value},
                          objects={name: projects.kind_of(obj) for name, obj in project.items()},
                          verbs={name: projects.verbs_of(project, name) for name in project})
    return EDIT.run(project, values)


def _sensors(table):
    return {row[0]: row for row in table['rows']}


def test_the_preview_draws_the_fits_and_changes_nothing(validate):
    project, centers = _project()
    journal = list(project.journal)
    view, sensors, fits, reading = run(project)
    wire_views([view, sensors, fits, reading], validate)
    assert list(project) == ['Probed'] and project.journal == journal
    assert [param.name for param in OP.params] == ['geometry', 'keep']
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
    # the can's length is not measured and is taken as its diameter, 8 mm for 6
    np.testing.assert_allclose(view['arrows']['tails'][3], centers['11'], atol=1.1e-3)
    # the sensors, one a row: node, model, shape, size, length, fit
    assert sensors['columns'] == ['Sensor', 'Node', 'Model', 'Shape', 'Size [m]', 'Length [m]',
                                  'Fit']
    found = _sensors(sensors)
    assert found['31'] == ['31', '31', '356A33', 'Cube', '0.0102', '', 'placed']
    assert found['11'][:4] == ['11', '11', '', 'Cylinder']
    assert float(found['11'][4]) == pytest.approx(0.008, rel=1e-4) == float(found['11'][5])
    assert found['40'][2:4] == ['', 'Cube'] and float(found['40'][4]) == pytest.approx(0.0171, abs=2e-4)
    assert found['50'][4:] == ['', '', ('one face gives one axis; a second face is needed to '
                                       'orient the accelerometer')]
    # typed into: a node, a model from the catalog or any name, a size and a length
    assert sensors['edit']['op'] == 'project.set_sensor'
    assert sensors['edit']['columns'] == [None, 'node', 'model', None, 'size', 'length', None]
    assert {'356A02', '352A24', '208C01'} <= set(sensors['edit']['choices']['model'])
    assert fits['columns'][0] == 'Sensor' and len(fits['rows']) == 4
    read = _rows(reading)
    assert read[0] == ('Sensors', '3 placed of 4 measured', 'warn')
    assert any('datum is not named' in value for _l, value, _s in read)
    assert read[-1][0] == 'Not created'


def test_the_sizes_are_shown_and_typed_in_the_display_length():
    from hertzforge.service import display_units

    project, _centers = _project()
    views = run(project)
    display_units.convert(views, 'in-slinch-lbf-s')
    table = views[1]
    assert table['columns'][4:6] == ['Size [in]', 'Length [in]']
    assert float(_sensors(table)['31'][4]) == pytest.approx(10.2 / 25.4, rel=1e-5)
    # what is typed there comes back in meters by these
    assert table['edit']['to_si'] == {'size': [0.0254, 0.0], 'length': [0.0254, 0.0]}


def test_a_cell_typed_changes_the_sensor_and_the_preview():
    project, centers = _project()
    # a size for the block no model fits: its node moves to its center
    view, sensors, _fits, _reading = edit(project, 3, 'size', '0.019')
    assert project['Probed'].sensor_choices == {'40': {'size': 0.019}}
    assert project.journal[-1] == "project.set_sensor('Probed', '40', size=0.019)"
    assert _sensors(sensors)['40'][4] == '0.019'
    np.testing.assert_allclose(view['arrows']['tails'][4], centers['40'], atol=1e-6)
    # a length for the can
    view, sensors, _fits, _reading = edit(project, 2, 'length', '0.006')
    assert _sensors(sensors)['11'][5] == '0.006'
    np.testing.assert_allclose(view['arrows']['tails'][3], centers['11'], atol=1e-6)
    # a model fills the size from the catalog, and takes back the size typed before
    edit(project, 1, 'size', '0.012')
    _view, sensors, _fits, _reading = edit(project, 1, 'model', '356A02')
    assert _sensors(sensors)['31'][2:5] == ['356A02', 'Cube', '0.014']
    # a force transducer on the can: its catalog size, and a force channel on Apply
    _view, sensors, _fits, _reading = edit(project, 2, 'model', '208C01')
    assert _sensors(sensors)['11'][2:6] == ['208C01', 'Hex', '0.01588', '0.01588']
    # a node
    view, sensors, _fits, _reading = edit(project, 3, 'node', '140')
    assert _sensors(sensors)['40'][1] == '140' and '140X+' in view['arrows']['labels']
    # an empty cell takes the choice back
    _view, sensors, _fits, _reading = edit(project, 1, 'model', '')
    assert _sensors(sensors)['31'][2] == '356A33'
    assert '31' not in project['Probed'].sensor_choices


def test_a_cell_that_cannot_hold_what_was_typed_is_refused_or_said():
    project, _centers = _project()
    for row, column, value, said in ((1, 'size', 'big', 'a size is a number'),
                                     (1, 'size', '-1', 'a length above 0'),
                                     (1, 'node', '2.5', 'whole number'),
                                     (9, 'node', '5', 'there is no row 9')):
        with pytest.raises(ProtocolError, match=said):
            edit(project, row, column, value)
    assert project['Probed'].sensor_choices == {}
    # a model of the other kind is kept and said in its row; the sensor is left out
    _view, sensors, _fits, reading = edit(project, 1, 'model', '352A24')
    assert 'is a uniaxial model' in _sensors(sensors)['31'][6]
    assert _rows(reading)[0] == ('Sensors', '2 placed of 4 measured', 'warn')


def test_apply_creates_the_geometry_and_the_table():
    project, centers = _project()
    edit(project, 3, 'size', '0.019')
    _view, _sensors_table, _fits, reading = run(project, keep=True)
    assert list(project) == ['Probed', 'Probed Sensors Geometry', 'Probed Sensors Channels']
    assert _rows(reading)[-1] == ('Created', ('Probed Sensors Geometry and Probed Sensors '
                                             'Channels'), 'pass')
    assert project.journal[-1] == "project.create_sensors('Probed')"
    geometry = project['Probed Sensors Geometry']
    assert geometry.node_id.tolist() == [31, 11, 40]
    np.testing.assert_allclose(geometry.node_xyz[2], centers['40'], atol=1e-8)
    assert project['Probed Sensors Channels'].num_channels == 7


def test_what_cannot_be_placed_is_said_in_the_preview_and_refused_on_apply():
    project, _centers = _project(unit=None)
    view, sensors, fits, reading = run(project)
    assert 'bodies' not in view and 'arrows' not in view and len(sensors['rows']) == 4
    assert sensors['columns'][4:6] == ['Size', 'Length']
    assert fits['columns'][3] == 'Worst RMS [file units]'
    assert any(label == 'Not placed' and 'has no length unit' in value
               for label, value, _status in _rows(reading))
    with pytest.raises(ProtocolError, match='has no length unit'):
        run(project, keep=True)


def test_it_is_offered_on_a_geometry_of_measured_points_only():
    project, _centers = _project()
    project.add('Cloud', points_geometry(np.zeros((3, 3)) + [[0], [1], [2]]))
    assert 'create_sensors' in projects.verbs_of(project, 'Probed')
    assert 'create_sensors' not in projects.verbs_of(project, 'Cloud')
    with pytest.raises(ProtocolError):
        run(project, geometry='Cloud')
