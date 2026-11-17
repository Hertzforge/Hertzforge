"""A geometry and a channel table from accelerometers fitted to measured points."""

import numpy as np
import pytest
from test_scan_fit import MM, _can, _cube

from hertzforge import Project
from hertzforge.core.scan_fit import fit_sensors
from hertzforge.core.scan_sensors import assemble, guess_model
from hertzforge.io.point_files import points_geometry


def _features():
    """A 10.2 mm cube (sensor 31), a 6.3 mm cube measured on its X- face
    (sensor 32), a 19 mm cube (sensor 40, larger than any model), a can
    (sensor 11) and a face with no partner (sensor 50)."""
    frame_a, center_a, _side_a, faces_a = _cube(1, side=10.2 * MM)
    frame_b, center_b, _side_b, faces_b = _cube(2, (-1, 2, 3), side=6.3 * MM)
    frame_c, center_c, _side_c, faces_c = _cube(3, side=19.0 * MM, count=30)
    frame_d, center_d, _radius, height, side, top = _can(4)
    features = {**{f'31_{k}': v for k, v in faces_a.items()},
                **{f'32_{k}': v for k, v in faces_b.items()},
                **{f'40_{k}': v for k, v in faces_c.items()},
                '11_c': side, '11_3': top, '50_1': faces_a['1'] + 1.0}
    truth = {'31': (frame_a, center_a), '32': (frame_b, center_b), '40': (frame_c, center_c),
             '11': (frame_d, center_d)}
    return features, truth, height


def _geometry_of(features, unit='mm'):
    names = list(features)
    points = np.vstack([features[name] for name in names]) / MM
    owner = np.repeat(np.arange(len(names)), [len(features[name]) for name in names])
    return points_geometry(points, unit, names, owner)


def test_the_catalog_model_a_block_fits_in_is_guessed():
    features, _truth, _height = _features()
    fits = {fit.sensor: fit for fit in fit_sensors(features)[0]}
    assert guess_model(fits['31'], 1.0).size == (10.2, 10.2, 10.2)
    assert guess_model(fits['32'], 1.0).size == (6.3, 6.3, 6.3)
    assert guess_model(fits['40'], 1.0) is None          # 19 mm: larger than any model
    assert guess_model(fits['11'], 1.0) is None          # no uni-axial is told by its faces
    # the points reach 0.9 of a 14 mm side: past a 10.2 mm cube, so the 14 mm one
    _frame, _center, _side, faces = _cube(5, side=14.0 * MM, count=40)
    assert guess_model(fit_sensors({f'1_{k}': v for k, v in faces.items()})[0][0], 1.0).model == '356A02'


def test_each_accelerometer_is_a_node_at_its_center_in_its_own_system():
    features, truth, height = _features()
    fits, _notes = fit_sensors(features, meters=1.0)
    geometry, table, placed, notes = assemble(fits, 'mm', sizes={11: height / MM, 40: 19.0})
    assert geometry.node_id.tolist() == [31, 32, 40, 11] and geometry.length_unit == 'mm'
    assert list(geometry.cs_name) == ['', 'Sensor 31', 'Sensor 32', 'Sensor 40', 'Sensor 11']
    assert geometry.node_def_cs.tolist() == [1, 1, 1, 1]
    assert geometry.node_disp_cs.tolist() == [2, 3, 4, 5]
    for row, sensor in enumerate(('31', '32', '40', '11')):
        frame, center = truth[sensor]
        np.testing.assert_allclose(geometry.node_xyz[row], center, atol=1e-8)
        np.testing.assert_allclose(geometry.cs_matrix[row + 1, 3], center, atol=1e-8)
        if sensor != '11':
            np.testing.assert_allclose(geometry.cs_matrix[row + 1, :3], frame, atol=1e-8)
    # the can's Z is its axis; its X and Y are laid into its top
    np.testing.assert_allclose(geometry.cs_matrix[4, 2], truth['11'][0][2], atol=1e-8)
    assert [(p.sensor, p.model, p.sized_by) for p in placed] == [
        ('31', '356A33', 'guess'), ('32', '356A01', 'guess'), ('40', 'Block 19 mm', 'given'),
        ('11', 'Cylinder 8 x 6 mm', 'given')]
    assert notes == [('sensor 50 is left out: one face gives one axis; a second face is '
                     'needed to orient the accelerometer')]
    frame_ = table.frame
    assert frame_['channel'].tolist() == list(range(1, 11))
    assert [int(node) for node in frame_['node']] == [31] * 3 + [32] * 3 + [40] * 3 + [11]
    # X+, Y+, Z+ though sensor 32 was measured on its X- face; the can is Z+
    assert frame_['direction'].tolist() == ['X+', 'Y+', 'Z+'] * 3 + ['Z+']
    assert frame_['triax_dof'].tolist() == ['X', 'Y', 'Z'] * 3 + ['']
    assert set(frame_['unit']) == {'g'} and set(frame_['channel_type']) == {'acceleration'}
    assert frame_['model'].tolist() == (['356A33'] * 3 + ['356A01'] * 3 + ['Block 19 mm'] * 3
                                        + ['Cylinder 8 x 6 mm'])
    assert frame_['make'].tolist()[:6] == ['PCB'] * 6
    assert placed[1].channels == (4, 5, 6) and placed[3].channels == (10,)


def test_sizes_come_from_the_model_named_then_given_then_measured():
    features, truth, _height = _features()
    fits, _notes = fit_sensors(features, meters=1.0)
    # a model named for sensor 31 that is larger than the one guessed moves its node
    geometry, _table, placed, notes = assemble(fits, 'mm', models={'31': 'pcb 356a02'})
    assert (placed[0].model, placed[0].sized_by, placed[0].size) == ('356A02', 'model', (14.0,) * 3)
    shift = geometry.node_xyz[0] - truth['31'][1]
    assert np.linalg.norm(shift) == pytest.approx(np.sqrt(3) * (14.0 - 10.2) / 2 * MM, rel=1e-6)
    # with nothing said, the 19 mm block and the can take what was measured, and say so
    assert placed[2].sized_by == 'measured' and placed[2].size[0] == pytest.approx(17.1, abs=0.2)
    assert placed[3].sized_by == 'measured' and placed[3].size[2] == pytest.approx(8.0, abs=1e-4)
    assert any('sensor 40: no catalog model fits a block of' in note for note in notes)
    assert any('sensor 11: its height is not measured' in note for note in notes)
    assert placed[2].model.startswith('Block 17.') and placed[3].model == 'Cylinder 8 x 8 mm'
    # a size written as the model is read as one
    _g, _t, placed, _n = assemble(fits, 'mm', models={'40': 'Block 20 mm', '11': 'cylinder 8x5 mm'})
    assert (placed[2].size, placed[2].sized_by) == ((20.0,) * 3, 'model')
    assert (placed[3].size, placed[3].model) == ((8.0, 8.0, 5.0), 'cylinder 8x5 mm')
    # a box that is not a cube
    _g, _t, placed, _n = assemble(fits, 'mm', sizes={'40': (19.0, 19.0, 19.0)}, models={'40': 'mine'})
    assert (placed[2].model, placed[2].sized_by) == ('mine', 'given')


def test_what_cannot_be_placed_is_refused():
    features, _truth, _height = _features()
    fits, _notes = fit_sensors(features, meters=1.0)
    with pytest.raises(ValueError, match="'XYZ99' is not in the sensor catalog; give its size"):
        assemble(fits, 'mm', models={31: 'XYZ99'})
    with pytest.raises(ValueError, match='sensor 31 is triax and 352A24 is uniaxial'):
        assemble(fits, 'mm', models={31: '352A24'})
    with pytest.raises(ValueError, match='models names sensor 99'):
        assemble(fits, 'mm', models={99: '356A02'})
    with pytest.raises(ValueError, match='sensors 31 and 32 would both be node 7'):
        assemble(fits, 'mm', nodes={31: 7, 32: 7})
    with pytest.raises(ValueError, match='no accelerometer is located'):
        assemble([fit for fit in fits if fit.sensor == '50'], 'mm')


def test_sensors_not_named_with_a_number_take_the_next_node_free():
    _frame, _center, _side, faces = _cube(1)
    features = {**{f'left_{k}': v for k, v in faces.items()},
                **{f'8_{k}': v + 0.1 for k, v in faces.items()},
                **{f'right_{k}': v + 0.2 for k, v in faces.items()}}
    geometry, _table, _placed, _notes = assemble(fit_sensors(features)[0], 'mm')
    assert geometry.node_id.tolist() == [9, 8, 10]
    geometry, _table, _placed, _notes = assemble(fit_sensors(features)[0], 'mm',
                                                 nodes={'left': 101})
    assert geometry.node_id.tolist() == [101, 8, 102]


def test_a_project_creates_them_linked_and_the_drawing_agrees():
    features, truth, height = _features()
    project = Project()
    project.add('Probed', _geometry_of(features))
    made = project.create_sensors('Probed', sizes={11: height / MM, 40: 19.0})
    assert (made['geometry'], made['channels']) == ('Probed Sensors Geometry',
                                                    'Probed Sensors Channels')
    assert any('sensor 50 is left out' in note for note in made['notes'])
    assert project.journal[-1] == "project.create_sensors('Probed', sizes={11: 6.0, 40: 19.0})"
    assert project.object_groups[-1]['members'] == [made['geometry'], made['channels']]
    geometry = project[made['geometry']]
    np.testing.assert_allclose(geometry.node_xyz[0], truth['31'][1], atol=1e-8)
    # the measured points are untouched
    assert project['Probed'].num_nodes == sum(len(points) for points in features.values())
    # the sensors the 3-D view draws sit on the nodes, turned as the fit found them
    sensors, notes = project.sensors(made['channels'])
    assert not notes
    by_node = {sensor.node: sensor for sensor in sensors}
    for sensor in ('31', '32', '40'):
        frame, center = truth[sensor]
        np.testing.assert_allclose(by_node[int(sensor)].center, center, atol=1e-8)
        np.testing.assert_allclose(by_node[int(sensor)].frame, frame.T, atol=1e-8)
    assert by_node[31].entry.model == '356A33' and not by_node[31].generic
    # a sensor with no catalog model is drawn at the size it was placed with
    assert (by_node[40].entry.shape, by_node[40].entry.size) == ('box', (19.0, 19.0, 19.0))
    assert (by_node[11].entry.shape, by_node[11].entry.size) == ('puck', (8.0, 8.0, 6.0))
    assert not by_node[40].generic and not by_node[11].generic
    np.testing.assert_allclose(by_node[11].frame[:, 2], truth['11'][0][2], atol=1e-8)
    # a DOF of the table points along the accelerometer's own axis
    np.testing.assert_allclose(geometry.dof_direction('32X+'), truth['32'][0][0], atol=1e-8)
    assert made['geometry'] == project.geometry_for(made['channels'])[0]


def test_a_geometry_with_no_unit_is_refused_and_names_are_chosen():
    features, _truth, _height = _features()
    project = Project()
    project.add('Raw', _geometry_of(features, unit=None))
    with pytest.raises(ValueError, match='has no length unit; declare it'):
        project.create_sensors('Raw')
    project['Raw'].define_units('mm')
    made = project.create_sensors('Raw', name='Frame')
    assert (made['geometry'], made['channels']) == ('Frame Geometry', 'Frame Channels')
