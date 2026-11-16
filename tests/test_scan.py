"""A scan: points measured in space, read from a measuring arm's files."""

import warnings

import h5py
import numpy as np
import pytest

from hertzforge import Project
from hertzforge import io as hio
from hertzforge.core.scan import UNASSIGNED, Scan
from hertzforge.io import native, point_files
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


def test_feature_table_reads_as_features_in_measured_order(tmp_path):
    path = _write(tmp_path, 'probed.csv', TABLE)
    assert _sniffed(path) == ['measured_points']
    scan = hio.import_file(path)
    assert isinstance(scan, Scan)
    assert scan.features == ['31_1', '7_C'] and scan.length_unit == 'mm'
    assert scan.feature_of.tolist() == [0, 0, 0, 1, 1]
    # the file lists a feature's points by deviation; they are held by index
    np.testing.assert_allclose(scan.feature_points('31_1'),
                               [[0.010, 0.0, 0.005], [0.010, 0.004, 0.005], [0.010, 0.002, 0.009]])
    np.testing.assert_allclose(scan.deviation, [1e-6, -2e-6, 4e-6, 0, 0])
    assert scan.properties == {'Coordinate System': 'world', 'Data Alignments': 'original'}
    assert scan.describe() == '5 points in 2 features'


def test_feature_table_says_what_it_read_differently(tmp_path):
    text = (TABLE.replace('Length Units,Millimeters', 'Length Units,Cubits')
            .replace('No. Pts,3', 'No. Pts,4')
            .replace('Feature,2,0.000,4.000,12.000,5.000', 'Feature,2,0.000,4.000,twelve,5.000'))
    path = _write(tmp_path, 'probed.csv', text)
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter('always')
        scan = point_files.load_table(path)
    said = [str(w.message) for w in caught if issubclass(w.category, ImportNote)]
    assert len(said) == 4
    assert any('31_1 says 4 points and holds 3' in note for note in said)
    assert any("'Cubits'" in note for note in said)
    assert any('a row of 7_C' in note for note in said)
    assert any('7_C says 2 points and holds 1' in note for note in said)
    assert scan.length_unit is None and scan.num_points == 4
    np.testing.assert_allclose(scan.points[0], [10.0, 0.0, 5.0])
    # the unit given by hand is used whatever the file says
    with warnings.catch_warnings():
        warnings.simplefilter('ignore', ImportNote)
        assert point_files.load_table(path, length_unit='in').length_unit == 'in'


def test_feature_table_with_no_points_is_refused(tmp_path):
    path = _write(tmp_path, 'empty.csv', 'Feature Measured Points Table,,,,,\r\nLength Units,Millimeters,,,,\r\n')
    with pytest.raises(ValueError, match='holds no measured points'):
        point_files.load_table(path)


@pytest.mark.parametrize('mark', [',', ' ', '\t', ';'])
def test_bare_points_read_as_a_cloud(tmp_path, mark):
    rows = [[489.3, 170.5, -45.2], [488.7, 169.6, -45.2], [488.1, 170.9, -45.1], [488.9, 171.0, -45.3]]
    path = _write(tmp_path, 'cloud.txt', '\r\n'.join(mark.join(f'{v:.6f}' for v in row) for row in rows) + '\r\n')
    assert _sniffed(path)[0] == 'points'
    scan = hio.import_file(path)
    assert isinstance(scan, Scan) and not scan.features and scan.length_unit is None
    assert (scan.feature_of == UNASSIGNED).all() and scan.deviation is None
    np.testing.assert_allclose(scan.points, rows)
    assert hio.import_file(path, length_unit='mm').points[0] == pytest.approx([0.4893, 0.1705, -0.0452])


def test_columns_of_data_are_not_points(tmp_path):
    """Three columns whose first rises are a time and two channels, a
    header makes it a table, and four columns are not points: each stays
    the delimited reader's."""
    rising = _write(tmp_path, 'history.txt', ''.join(f'{k * 0.001:.3f},{np.sin(k):.5f},{np.cos(k):.5f}\n'
                                                     for k in range(50)))
    headed = _write(tmp_path, 'headed.csv', 'x,y,z\n' + '3,2,1\n1,2,3\n2,3,1\n')
    four = _write(tmp_path, 'four.txt', '3,2,1,0\n1,2,3,4\n2,3,1,9\n')
    for path in (rising, headed, four):
        assert 'points' not in _sniffed(path)
    # asked for by name, the reader takes a file the rule would not
    assert hio.import_file(rising, format='points').num_points == 50
    with pytest.raises(ValueError, match='4 columns; a point file has three'):
        point_files.load_points(four)


def test_units_are_declared_corrected_and_removed():
    scan = Scan([[1.0, 2.0, 3.0]], deviation=[0.5])
    assert not scan.units_defined
    scan.define_units('mm')
    np.testing.assert_allclose(scan.points, [[0.001, 0.002, 0.003]])
    scan.define_units('in')                     # the file's numbers, read again
    np.testing.assert_allclose(scan.points, [[0.0254, 0.0508, 0.0762]])
    np.testing.assert_allclose(scan.deviation, [0.0127])
    scan.undefine_units()
    np.testing.assert_allclose(scan.points, [[1.0, 2.0, 3.0]])
    assert scan.length_unit is None


def test_a_scan_is_checked_as_it_is_made():
    with pytest.raises(ValueError, match='one feature per point'):
        Scan(np.zeros((3, 3)), features=['a'], feature_of=[0, 0])
    with pytest.raises(ValueError, match='a feature the scan does not have'):
        Scan(np.zeros((2, 3)), features=['a'], feature_of=[0, 1])
    with pytest.raises(ValueError, match='one value per point'):
        Scan(np.zeros((2, 3)), deviation=[0.0])
    with pytest.raises(ValueError, match="no feature named 'b'"):
        Scan(np.zeros((1, 3)), features=['a'], feature_of=[0]).feature_points('b')


def test_a_project_holds_scans_and_saves_them(tmp_path):
    project = Project()
    names = project.import_file(_write(tmp_path, 'probed.csv', TABLE))
    cloud = _write(tmp_path, 'cloud.txt', '3,2,1\n1,2,3\n2,3,1\n')
    names += project.import_file(cloud)
    assert names == ['Scan', 'Scan (2)']
    assert project.scans == [project['Scan'], project['Scan (2)']]
    assert project.journal[-1] == f'project.import_file({cloud!r})'
    path = tmp_path / 'scans.hforge'
    project.save(path)
    with h5py.File(path) as f:
        assert f.attrs[native.STAMP] == native.SCAN_SCHEMA == 5
    again = Project.open(path)
    for name in names:
        was, now = project[name], again[name]
        np.testing.assert_array_equal(was.points, now.points)
        np.testing.assert_array_equal(was.feature_of, now.feature_of)
        assert (was.features, was.length_unit, was.properties) == (now.features, now.length_unit, now.properties)
    assert again['Scan (2)'].deviation is None
    np.testing.assert_array_equal(again['Scan'].deviation, project['Scan'].deviation)


def test_an_older_build_refuses_a_file_holding_a_scan(tmp_path, monkeypatch):
    path = tmp_path / 'scan.hforge'
    native.save(Scan(np.zeros((2, 3))), path)
    monkeypatch.setattr(native, 'SCHEMA_VERSION', native.TEST_SETUP_SCHEMA)
    with pytest.raises(ValueError, match='newer Hertzforge'):
        native.load(path)
