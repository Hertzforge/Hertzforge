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

OP, EDIT, EDIT_SET = OPERATIONS


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


def edit_set(project, row, column, value):
    values = EDIT_SET.resolve(
        {'geometry': 'Probed', 'row': row, 'column': column, 'value': value},
        objects={name: projects.kind_of(obj) for name, obj in project.items()},
        verbs={name: projects.verbs_of(project, name) for name in project})
    return EDIT_SET.run(project, values)


def _sensors(table):
    return {row[0]: row for row in table['rows']}


def test_the_preview_draws_the_fits_and_changes_nothing(validate):
    project, centers = _project()
    journal = list(project.journal)
    view, sensors, stray, fits, reading = run(project)
    wire_views([view, sensors, stray, fits, reading], validate)
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
    # the can's length is twice its top from the middle of its side's points, not its true 6 mm
    np.testing.assert_allclose(view['arrows']['tails'][3], centers['11'], atol=1.1e-3)
    # the sensors, one a row: node, model, shape, length, diameter, fit
    assert sensors['columns'] == ['Sensor', 'Node', 'Model', 'Shape', 'Length [m]',
                                  'Diameter [m]', 'Fit']
    found = _sensors(sensors)
    assert found['31'] == ['31', '31', '356A33', 'Cube', '0.0102', '', 'placed']
    assert found['11'][:4] == ['11', '11', '', 'Cylinder']
    # a cube's side is its length and it has no diameter; a can has both
    can = next(fit for fit in project.fit_sensors('Probed')[0] if fit.sensor == '11')
    assert float(found['11'][5]) == pytest.approx(0.008, rel=1e-4)
    assert float(found['11'][4]) == pytest.approx(can.length, rel=1e-4)
    assert float(found['11'][4]) != pytest.approx(0.008, rel=1e-2)
    assert found['40'][2:4] == ['', 'Cube'] and float(found['40'][4]) == pytest.approx(0.0171, abs=2e-4)
    assert found['50'][4:] == ['', '', ('one face gives one axis; a second face is needed to '
                                       'orient the accelerometer')]
    # typed into: a node, a model from the catalog or any name, a length and a diameter
    assert sensors['edit']['op'] == 'project.set_sensor'
    assert sensors['edit']['columns'] == [None, 'node', 'model', None, 'length', 'diameter', None]
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
    assert table['columns'][4:6] == ['Length [in]', 'Diameter [in]']
    assert float(_sensors(table)['31'][4]) == pytest.approx(10.2 / 25.4, rel=1e-5)
    # what is typed there comes back in meters by these
    assert table['edit']['to_si'] == {'length': [0.0254, 0.0], 'diameter': [0.0254, 0.0]}


def test_a_cell_typed_changes_the_sensor_and_the_preview():
    project, centers = _project()
    # a length for the block no model fits, its side: its node moves to its center
    view, sensors, *_rest = edit(project, 3, 'length', '0.019')
    assert project['Probed'].sensor_choices == {'40': {'size': 0.019}}
    assert project.journal[-1] == "project.set_sensor('Probed', '40', length=0.019)"
    assert _sensors(sensors)['40'][4] == '0.019'
    np.testing.assert_allclose(view['arrows']['tails'][4], centers['40'], atol=1e-6)
    # a length for the can
    view, sensors, *_rest = edit(project, 2, 'length', '0.006')
    assert _sensors(sensors)['11'][4] == '0.006'
    assert project['Probed'].sensor_choices['11'] == {'length': 0.006}
    np.testing.assert_allclose(view['arrows']['tails'][3], centers['11'], atol=1e-6)
    # a diameter for the can is its size
    edit(project, 2, 'diameter', '0.009')
    assert project['Probed'].sensor_choices['11'] == {'length': 0.006, 'size': 0.009}
    # a model fills the size from the catalog, and takes back the length typed before
    edit(project, 1, 'length', '0.012')
    _view, sensors, *_rest = edit(project, 1, 'model', '356A02')
    assert _sensors(sensors)['31'][2:5] == ['356A02', 'Cube', '0.014']
    # a force transducer on the can: its catalog size, and a force channel on Apply
    _view, sensors, *_rest = edit(project, 2, 'model', '208C01')
    assert _sensors(sensors)['11'][2:6] == ['208C01', 'Hex', '0.01588', '0.01588']
    # a node
    view, sensors, *_rest = edit(project, 3, 'node', '140')
    assert _sensors(sensors)['40'][1] == '140' and '140X+' in view['arrows']['labels']
    # an empty cell takes the choice back
    _view, sensors, *_rest = edit(project, 1, 'model', '')
    assert _sensors(sensors)['31'][2] == '356A33'
    assert '31' not in project['Probed'].sensor_choices


def test_a_cell_that_cannot_hold_what_was_typed_is_refused_or_said():
    project, _centers = _project()
    for row, column, value, said in ((1, 'length', 'big', 'a length is a number'),
                                     (1, 'length', '-1', 'a length is a length above 0'),
                                     (2, 'diameter', '-1', 'a diameter is a length above 0'),
                                     (1, 'diameter', '0.01', 'a cube has no diameter'),
                                     (1, 'node', '2.5', 'whole number'),
                                     (9, 'node', '5', 'there is no row 9')):
        with pytest.raises(ProtocolError, match=said):
            edit(project, row, column, value)
    assert project['Probed'].sensor_choices == {}
    # a model of the other kind is kept and said in its row; the sensor is left out
    _view, sensors, *_rest, reading = edit(project, 1, 'model', '352A24')
    assert 'is a uniaxial model' in _sensors(sensors)['31'][6]
    assert _rows(reading)[0] == ('Sensors', '2 placed of 4 measured', 'warn')


def test_apply_creates_the_geometry_and_the_table():
    project, centers = _project()
    edit(project, 3, 'length', '0.019')
    *_views, reading = run(project, keep=True)
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
    view, sensors, _stray, fits, reading = run(project)
    assert 'bodies' not in view and 'arrows' not in view and len(sensors['rows']) == 4
    assert sensors['columns'][4:6] == ['Length', 'Diameter']
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


def test_each_sensors_scan_sets_are_under_its_row_and_typed_into(validate):
    project, _centers = _project()
    _view, sensors, stray, _fits, _reading = run(project)
    wire_views([sensors, stray], validate)
    # a table under each sensor's row: its sets, the sensor and the surface each is taken for
    assert [under['title'] for under in sensors['nested']] == [
        f'Scan sets of sensor {sensor}' for sensor in ('31', '11', '40', '50')]
    under = sensors['nested'][1]
    assert under['columns'] == ['Scan set', 'Sensor', 'Surface', 'Points', 'RMS (mm)']
    assert [row[:4] for row in under['rows']] == [['11_c', '11', 'Cylinder', '40'],
                                                  ['11_3', '11', 'Uni-axial Top', '6']]
    assert [row[2] for row in sensors['nested'][0]['rows']] == ['X+', 'Y+', 'Z+']
    assert under['edit']['op'] == 'project.set_scan_set'
    assert under['edit']['columns'] == [None, 'sensor', 'surface', None, None]
    # a row's place is among all the geometry's sets, whichever table it is in
    assert under['edit']['rows'] == [4, 5]
    assert under['edit']['choices']['surface'] == [
        'X+', 'X-', 'Y+', 'Y-', 'Z+', 'Z-', 'Cylinder', 'Uni-axial Top', 'Uni-axial Bottom']
    # the sensor that is not placed starts opened; the placed ones closed
    assert sensors['open'] == [4]
    # a set whose name says no sensor has a table of its own
    assert stray['title'] == 'Scan sets of Probed with no sensor'
    assert stray['rows'] == [['datum', '', '', '6', '']] and stray['edit']['rows'] == [10]


def test_a_scan_set_is_given_to_another_sensor_or_surface():
    project, _centers = _project()
    # the stray set made sensor 50's second face: sensor 50 is then located
    with pytest.raises(ProtocolError, match='give its sensor first'):
        edit_set(project, 10, 'surface', 'Y+')
    # given a sensor, it takes the first surface that sensor lacks, here Y+ beside its X+
    _view, sensors, *_rest = edit_set(project, 10, 'sensor', '50')
    assert list(project['Probed'].group_name)[9] == '50_2'
    assert project.journal[-1] == "project.set_scan_set('Probed', 'datum', sensor='50')"
    assert len(_rest) == 2                                    # no table of stray sets is left
    assert sensors['nested'][3]['rows'][1][:3] == ['50_2', '50', 'Y+']
    # a face of sensor 31 said to be its X- face: the labelling is then left-handed
    _view, sensors, *_rest = edit_set(project, 1, 'surface', 'X-')
    assert next(iter(project['Probed'].group_name)) == '31_-1'
    assert 'left-handed' in _sensors(sensors)['31'][6]
    assert sensors['open'][0] == 1
    # and back
    _view, sensors, *_rest = edit_set(project, 1, 'surface', 'x+')
    assert _sensors(sensors)['31'][6] == 'placed'
    # the can's flat face said to be its bottom: Z+ turns over, and the node moves through
    view, sensors, *_rest = edit_set(project, 5, 'surface', 'Uni-axial Bottom')
    assert list(project['Probed'].group_name)[4] == '11_b'
    assert sensors['nested'][1]['rows'][1][2] == 'Uni-axial Bottom'
    row = view['arrows']['labels'].index('11Z+')
    turned = view['arrows']['directions'][row]
    edit_set(project, 5, 'surface', 'Uni-axial Top')
    view, *_rest = run(project)
    np.testing.assert_allclose(view['arrows']['directions'][row], -turned, atol=1e-6)
    for row, column, value, said in ((1, 'surface', 'sideways', 'is not a surface'),
                                     (1, 'sensor', '', 'a scan set needs a sensor'),
                                     (99, 'sensor', '5', 'there is no row 99')):
        with pytest.raises(ProtocolError, match=said):
            edit_set(project, row, column, value)


def test_two_sets_named_for_each_others_sensor_exchange_in_one_edit():
    """The correction for a top measured under another sensor's name: give
    it to the sensor it is on, and the top that was there comes back."""
    from test_scan_fit import _can

    _f, _c, _r, _h, side_a, top_a = _can(51)
    _f, _c, _r, _h, side_b, top_b = _can(52)
    features = {'11_c': side_a, '11_3': top_b, '12_c': side_b, '12_3': top_a}
    names = list(features)
    points = np.vstack([features[name] for name in names]) / MM
    owner = np.repeat(np.arange(4), [len(features[name]) for name in names])
    project = Project()
    project.add('Probed', points_geometry(points, 'mm', names, owner))
    _view, sensors, *_rest = run(project)
    assert sensors['open'] == [1, 2]
    assert all('look exchanged' in row[6] for row in sensors['rows'])
    _view, sensors, *_rest = edit_set(project, 2, 'sensor', '12')
    assert list(project['Probed'].group_name) == ['11_c', '12_3', '12_c', '11_3']
    assert [row[6] for row in sensors['rows']] == ['placed', 'placed'] and sensors['open'] == []
