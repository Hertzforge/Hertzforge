"""A geometry and a channel table from accelerometers fitted to measured points."""

import numpy as np
import pytest
from test_scan_fit import MM, _can, _cube

from hertzforge import Project
from hertzforge.core.scan_fit import fit_sensors
from hertzforge.core.scan_sensors import assemble, guess_model, resolve
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
    geometry, table, placed, notes = assemble(
        fits, 'mm', choices={11: {'length': height}, 40: {'size': 19.0 * MM}})
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
    assert [(p.sensor, p.table_model, p.shape, p.sized_by, p.placed) for p in placed] == [
        ('31', '356A33', 'Cube', 'guess', True), ('32', '356A01', 'Cube', 'guess', True),
        ('40', 'Block 19 mm', 'Cube', 'given', True),
        ('11', 'Cylinder 8 x 6 mm', 'Cylinder', 'given', True),
        ('50', '', 'Cube', '', False)]
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
    by_sensor = {p.sensor: p for p in placed}
    assert by_sensor['32'].channels == (4, 5, 6) and by_sensor['11'].channels == (10,)
    assert by_sensor['50'].channels == ()


def test_a_sensors_size_is_typed_then_its_models_then_guessed_then_measured():
    features, truth, _height = _features()
    fits, _notes = fit_sensors(features, meters=1.0)
    by = lambda rows: {row.sensor: row for row in rows}
    # nothing chosen: the cubes that fit a model are guessed, the rest measured, and said
    rows, notes = resolve(fits)
    found = by(rows)
    assert (found['31'].model, found['31'].sized_by, found['31'].size) == (
        '356A33', 'guess', pytest.approx(10.2 * MM))
    assert found['40'].sized_by == 'measured' and found['40'].size == pytest.approx(17.1 * MM, abs=2e-4)
    assert found['40'].table_model.startswith('Block 17.') and found['40'].length is None
    assert (found['11'].shape, found['11'].sized_by) == ('Cylinder', 'measured')
    assert found['11'].size == pytest.approx(8 * MM, rel=1e-5) == found['11'].length
    assert any('sensor 40: its side is taken as the least it can be' in note for note in notes)
    assert any('sensor 11: its length is not measured' in note for note in notes)
    # a model named: its size, and a larger model moves the node
    geometry, _table, placed, _notes = assemble(fits, 'mm', choices={'31': {'model': 'pcb 356a02'}})
    one = by(placed)['31']
    assert (one.model, one.sized_by, one.size) == ('pcb 356a02', 'model', pytest.approx(14 * MM))
    shift = geometry.node_xyz[0] - truth['31'][1]
    assert np.linalg.norm(shift) == pytest.approx(np.sqrt(3) * (14.0 - 10.2) / 2 * MM, rel=1e-6)
    # a size written as the model is read as one; a size typed beside a model wins
    found = by(resolve(fits, {'40': {'model': 'Block 20 mm'},
                              '11': {'model': 'cylinder 8x5 mm'},
                              '31': {'model': '356A02', 'size': 12 * MM}})[0])
    assert found['40'].size == pytest.approx(20 * MM) and found['40'].sized_by == 'model'
    assert found['11'].length == pytest.approx(5 * MM) and found['11'].table_model == 'cylinder 8x5 mm'
    assert (found['31'].size, found['31'].sized_by, found['31'].table_model) == (
        pytest.approx(12 * MM), 'given', '356A02')
    # a name the catalog does not hold: kept as the model, sized as measured
    found = by(resolve(fits, {'40': {'model': 'mine'}})[0])
    assert (found['40'].table_model, found['40'].sized_by) == ('mine', 'measured')
    # a catalog uni-axial that is not round, and a force transducer on a cylinder
    found = by(resolve(fits, {'11': {'model': '352A24'}})[0])
    assert (found['11'].shape, found['11'].length) == ('Teardrop', pytest.approx(4.8 * MM))
    found = by(resolve(fits, {'11': {'model': '208C01'}})[0])
    assert (found['11'].shape, found['11'].channel_type) == ('Hex', 'force')
    assert found['11'].size == pytest.approx(15.88 * MM) == found['11'].length
    _g, table, _p, _n = assemble(fits, 'mm', choices={'11': {'model': '208C01'}})
    assert table.frame[['channel_type', 'unit', 'model']].iloc[-1].tolist() == ['force', 'N', '208C01']


def test_what_cannot_be_placed_is_said_and_left_out():
    features, _truth, _height = _features()
    fits, _notes = fit_sensors(features, meters=1.0)
    by = lambda rows: {row.sensor: row for row in rows}
    # a model of the other kind
    found = by(resolve(fits, {31: {'model': '352A24'}, 11: {'model': '356A02'}})[0])
    assert found['31'].problem == ('352A24 is a uniaxial model, and this sensor was measured '
                                   'as a cube')
    assert found['11'].problem == ('356A02 is a triax model, and this sensor was measured as '
                                   'a cylinder')
    # a node chosen twice goes to the first
    found = by(resolve(fits, {31: {'node': 7}, 32: {'node': 7}})[0])
    assert found['31'].placed and found['32'].problem == "node 7 is already sensor 31's"
    assert found['50'].problem.startswith('one face gives one axis')
    with pytest.raises(ValueError, match='sensor 99 is not among the measured points'):
        resolve(fits, {99: {'model': '356A02'}})
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
                                                 choices={'left': {'node': 101}})
    assert geometry.node_id.tolist() == [101, 8, 102]


def test_a_project_creates_them_linked_and_the_drawing_agrees():
    features, truth, height = _features()
    project = Project()
    project.add('Probed', _geometry_of(features))
    assert project.set_sensor('Probed', 11, length=height) == {'length': height}
    project.set_sensor('Probed', '40', size=19.0 * MM)
    made = project.create_sensors('Probed')
    assert (made['geometry'], made['channels']) == ('Probed Sensors Geometry',
                                                    'Probed Sensors Channels')
    assert any('sensor 50 is left out' in note for note in made['notes'])
    assert project.journal[-3:] == ["project.set_sensor('Probed', 11, length=0.006)",
                                    "project.set_sensor('Probed', '40', size=0.019)",
                                    "project.create_sensors('Probed')"]
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


def test_a_sensors_choices_are_set_taken_back_and_saved(tmp_path):
    features, _truth, _height = _features()
    project = Project()
    project.add('Probed', _geometry_of(features))
    assert project.set_sensor('Probed', 31, node=131, size=0.012) == {'node': 131, 'size': 0.012}
    # naming a model takes back the size typed before it, so the model's own is used
    assert project.set_sensor('Probed', 31, model='356A02') == {'node': 131, 'model': '356A02'}
    assert project.set_sensor('Probed', 31, model='356A02', size=0.013)['size'] == 0.013
    assert project.set_sensor('Probed', 31, node=None, model='', size=None) == {}
    assert project['Probed'].sensor_choices == {}
    project.set_sensor('Probed', 40, size=0.019)
    for given, said in (({'node': 0}, 'whole number above 0'), ({'node': 2.5}, 'whole number'),
                        ({'size': -1.0}, 'a size is a length above 0'),
                        ({'color': 'red'}, "a sensor's color is not chosen")):
        with pytest.raises(ValueError, match=said):
            project.set_sensor('Probed', 31, **given)
    with pytest.raises(ValueError, match='has no sensor 99'):
        project.set_sensor('Probed', 99, node=1)
    # saved with the project, and scaled with the geometry's unit
    path = tmp_path / 'probed.hforge'
    project.save(path)
    again = Project.open(path)
    assert again['Probed'].sensor_choices == {'40': {'size': 0.019}}
    again['Probed'].undefine_units()
    assert again['Probed'].sensor_choices['40']['size'] == pytest.approx(19.0)
    # a choice for a sensor whose features have gone is passed over
    again['Probed'].define_units('mm')
    again['Probed'].sensor_choices['77'] = {'node': 5}
    assert again.create_sensors('Probed')['geometry'] == 'Probed Sensors Geometry'
