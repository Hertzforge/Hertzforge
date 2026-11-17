"""The made-up probed-points example: what the guide walks through, end to end."""

import importlib.util
from pathlib import Path

import numpy as np

from hertzforge import Project

EXAMPLE = Path(__file__).resolve().parents[1] / 'examples' / 'measured_points' / 'make_points.py'


def _example():
    spec = importlib.util.spec_from_file_location('make_points', EXAMPLE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_the_example_file_is_read_fitted_corrected_and_made_into_sensors(tmp_path):
    example = _example()
    path = tmp_path / 'probed_points.csv'
    example.write(str(path))
    project = Project()
    (scan,) = project.import_file(str(path))
    geometry = project[scan]
    assert geometry.length_unit == 'mm' and geometry.num_nodes == 100
    assert len(geometry.group_name) == 10

    # as probed: the cubes are placed, the X- face read as one, and the two uni-axial
    # accelerometers are refused with the features to rename
    fits = {fit.sensor: fit for fit in project.fit_sensors(scan)[0]}
    assert [fits[s].placed for s in ('11', '21', '31', '32')] == [True, True, False, False]
    assert fits['21'].axes == (-1, 2, 3)
    assert any('31_3 and 32_3 look exchanged' in flag.text for flag in fits['31'].flags)

    # one correction, then the lengths the scan cannot give
    assert project.set_scan_set(scan, '31_3', sensor=32) == '32_3'
    for sensor, (kind, size, _center, _angles) in example.SENSORS.items():
        if kind == 'cylinder':
            project.set_sensor(scan, sensor, length=size[1] / 1000)
    made = project.create_sensors(scan, name='Part')
    assert made['notes'] == []

    placed = project[made['geometry']]
    assert placed.node_id.tolist() == [11, 21, 31, 32]
    for row, (sensor, (kind, size, center, angles)) in enumerate(example.SENSORS.items()):
        # each node is the body's center, to a hundredth of a millimeter
        np.testing.assert_allclose(placed.node_xyz[row] * 1000, center, atol=0.02)
        frame = example.frame_of(angles)
        found = placed.cs_matrix[row + 1, :3]
        if kind == 'cube':
            np.testing.assert_allclose(found, frame, atol=6e-3)       # 4 um of scatter on a 5 mm face
        else:
            np.testing.assert_allclose(found[2], frame[2], atol=6e-3)
    table = project[made['channels']].frame
    assert table['direction'].tolist() == ['X+', 'Y+', 'Z+'] * 2 + ['Z+', 'Z+']
    # the cubes took the catalog models their points fit in
    assert table['model'].tolist()[:6] == ['356A33'] * 3 + ['356A01'] * 3
    assert table['model'].tolist()[6:] == ['Cylinder 7 x 5 mm', 'Cylinder 16 x 9 mm']
