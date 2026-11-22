"""Points measured with a coordinate measuring arm, read as a geometry."""

import warnings

import numpy as np
import pytest

from hertzforge import Project
from hertzforge import io as hio
from hertzforge.core.geometry import Geometry
from hertzforge.io import point_files
from hertzforge.io.notes import ImportNote

TABLE = """Feature Measured Points Table,,,,,
Length Units,Millimeters,,,,
Coordinate System,world,,,,
Data Alignments,original,,,,
,,,,,
Name,31_1,,,,
No. Pts,3,,,,
Min Dev,-0.002,,,,
Max Dev,0.004,,,,
,Index,Dev,X,Y,Z
Feature,3,0.004,10.000,2.000,9.000
Feature,1,0.001,10.000,0.000,5.000
Feature,2,-0.002,10.000,4.000,5.000
,,,,,
Name,7_C,,,,
No. Pts,2,,,,
Min Dev,0.000,,,,
Max Dev,0.000,,,,
,Index,Dev,X,Y,Z
Feature,1,0.000,-1.500,12.000,5.000
Feature,2,0.000,4.000,12.000,5.000


""".replace('\n', '\r\n')


def _write(tmp_path, name, text):
    path = tmp_path / name
    path.write_text(text, newline='')
    return str(path)


def _sniffed(path):
    return [imp.name for imp in hio._IMPORTERS if imp.sniff(path)]


def test_feature_table_reads_as_nodes_in_named_element_groups(tmp_path):
    path = _write(tmp_path, 'probed.csv', TABLE)
    assert _sniffed(path) == ['measured_points']
    geometry = hio.import_file(path)
    assert isinstance(geometry, Geometry) and geometry.length_unit == 'mm'
    assert list(geometry.group_name) == ['31_1', '7_C']
    assert geometry.node_id.tolist() == [1, 2, 3, 4, 5]
    # the file lists a feature's points by deviation; they are held by index
    np.testing.assert_allclose(geometry.node_xyz[:3],
                               [[0.010, 0.0, 0.005], [0.010, 0.004, 0.005], [0.010, 0.002, 0.009]])
    assert [int(conn[0]) for conn in geometry.elem_conn] == [1, 2, 3, 4, 5]
    assert geometry.elem_group.tolist() == [1, 1, 1, 2, 2]
    assert set(geometry.elem_type.tolist()) == {point_files.MEASURED_POINT}
    # one color to a feature, another to the next
    assert len(set(geometry.elem_color[:3].tolist())) == 1
    assert geometry.elem_color[0] != geometry.elem_color[3]


def test_feature_table_says_what_it_read_differently(tmp_path):
    text = (TABLE.replace('Length Units,Millimeters', 'Length Units,Cubits')
            .replace('No. Pts,3', 'No. Pts,4')
            .replace('Feature,2,0.000,4.000,12.000,5.000', 'Feature,2,0.000,4.000,twelve,5.000'))
    path = _write(tmp_path, 'probed.csv', text)
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter('always')
        geometry = point_files.load_table(path)
    said = [str(w.message) for w in caught if issubclass(w.category, ImportNote)]
    assert len(said) == 4
    assert any('31_1 says 4 points and holds 3' in note for note in said)
    assert any("'Cubits'" in note for note in said)
    assert any('a row of 7_C' in note for note in said)
    assert any('7_C says 2 points and holds 1' in note for note in said)
    assert geometry.length_unit is None and geometry.num_nodes == 4
    np.testing.assert_allclose(geometry.node_xyz[0], [10.0, 0.0, 5.0])
    # the unit given by hand is used whatever the file says
    with warnings.catch_warnings():
        warnings.simplefilter('ignore', ImportNote)
        assert point_files.load_table(path, length_unit='in').length_unit == 'in'


def test_feature_table_with_no_points_is_refused(tmp_path):
    path = _write(tmp_path, 'empty.csv',
                  'Feature Measured Points Table,,,,,\r\nLength Units,Millimeters,,,,\r\n')
    with pytest.raises(ValueError, match='holds no measured points'):
        point_files.load_table(path)


@pytest.mark.parametrize('mark', [',', ' ', '\t', ';'])
def test_bare_points_read_as_nodes(tmp_path, mark):
    rows = [[489.3, 170.5, -45.2], [488.7, 169.6, -45.2], [488.1, 170.9, -45.1], [488.9, 171.0, -45.3]]
    text = '\r\n'.join(mark.join(f'{v:.6f}' for v in row) for row in rows) + '\r\n'
    path = _write(tmp_path, 'cloud.txt', text)
    assert _sniffed(path)[0] == 'points'
    geometry = hio.import_file(path)
    assert isinstance(geometry, Geometry) and geometry.length_unit is None
    assert not len(geometry.elem_conn) and geometry.node_id.tolist() == [1, 2, 3, 4]
    np.testing.assert_allclose(geometry.node_xyz, rows)
    assert hio.import_file(path, length_unit='mm').node_xyz[0] == pytest.approx(
        [0.4893, 0.1705, -0.0452])


def test_columns_of_data_are_not_points(tmp_path):
    """Three columns whose first rises are a time and two channels, a
    header makes it a table, and four columns are not points: each stays
    the delimited reader's."""
    rising = _write(tmp_path, 'history.txt', ''.join(
        f'{k * 0.001:.3f},{np.sin(k):.5f},{np.cos(k):.5f}\n' for k in range(50)))
    headed = _write(tmp_path, 'headed.csv', 'x,y,z\n' + '3,2,1\n1,2,3\n2,3,1\n')
    four = _write(tmp_path, 'four.txt', '3,2,1,0\n1,2,3,4\n2,3,1,9\n')
    for path in (rising, headed, four):
        assert 'points' not in _sniffed(path)
    # asked for by name, the reader takes a file the rule would not
    assert hio.import_file(rising, format='points').num_nodes == 50
    with pytest.raises(ValueError, match='4 columns; a point file has three'):
        point_files.load_points(four)


def test_the_coordinate_system_moves_and_turns_the_points(tmp_path):
    """The nodes are placed in the geometry's one coordinate system, so
    moving or turning it carries the whole scan, as it does any geometry."""
    geometry = hio.import_file(_write(tmp_path, 'probed.csv', TABLE))
    was = geometry.node_xyz.copy()
    geometry.place_coordinate_system(1, origin=(1.0, 0.0, 0.0), angles=(0.0, 0.0, 90.0))
    np.testing.assert_allclose(geometry.node_xyz[:, 0], 1.0 - was[:, 1], atol=1e-12)
    np.testing.assert_allclose(geometry.node_xyz[:, 1], was[:, 0], atol=1e-12)
    np.testing.assert_allclose(geometry.node_xyz[:, 2], was[:, 2], atol=1e-12)


def test_a_project_holds_the_points_as_geometries_and_saves_them(tmp_path):
    project = Project()
    names = project.import_file(_write(tmp_path, 'probed.csv', TABLE))
    cloud = _write(tmp_path, 'cloud.txt', '3,2,1\n1,2,3\n2,3,1\n')
    names += project.import_file(cloud)
    assert names == ['Geometry', 'Geometry (2)']
    assert project.journal[-1] == f'project.import_file({cloud!r})'
    path = tmp_path / 'points.hforge'
    project.save(path)
    again = Project.open(path)
    for name in names:
        was, now = project[name], again[name]
        np.testing.assert_array_equal(was.node_xyz, now.node_xyz)
        np.testing.assert_array_equal(was.elem_group, now.elem_group)
        assert (list(was.group_name), was.length_unit) == (list(now.group_name), now.length_unit)


def test_a_universal_file_takes_the_points_as_nodes(tmp_path):
    """The universal file has no element that only marks a node, so the
    measured points go out as their nodes and come back as nodes."""
    geometry = hio.import_file(_write(tmp_path, 'probed.csv', TABLE))
    path = tmp_path / 'probed.unv'
    hio.export_file(geometry, str(path), format='unv')
    back = hio.import_file(str(path))
    back = back if isinstance(back, Geometry) else next(
        obj for obj in back.values() if isinstance(obj, Geometry))
    assert back.num_nodes == 5 and not len(back.elem_conn)


def test_a_table_that_names_no_unit_says_so_and_is_left_undefined(tmp_path):
    """A file may be written in another unit system: the unit is taken
    only when the file states it."""
    path = _write(tmp_path, 'probed.csv', TABLE.replace('Length Units,Millimeters,,,,\r\n', ''))
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter('always')
        geometry = point_files.load_table(path)
    said = [str(w.message) for w in caught if issubclass(w.category, ImportNote)]
    assert said == [('probed.csv: the file names no length unit, so the coordinates are left '
                    'as they are until a unit is declared')]
    assert geometry.length_unit is None
    np.testing.assert_allclose(geometry.node_xyz[0], [10.0, 0.0, 5.0])
