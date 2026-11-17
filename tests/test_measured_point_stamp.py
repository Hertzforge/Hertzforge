"""A file whose geometry holds measured points is stamped with the schema
that added them, so an earlier build says to update, and a geometry with an
element type this build does not know is refused as a newer build's."""

from __future__ import annotations

import h5py
import numpy as np
import pytest

import hertzforge
from hertzforge.core.geometry import MEASURED_POINT
from hertzforge.io import native


def _geometry(measured: bool):
    geometry = hertzforge.Geometry([1, 2, 3], [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]])
    geometry.add_element([1, 2], elem_type=11)
    if measured:
        geometry.add_element([3], elem_type=MEASURED_POINT)
    return geometry


@pytest.mark.parametrize('measured', [False, True])
def test_measured_points_decide_the_stamp_of_a_project(tmp_path, measured):
    project = hertzforge.Project('Frame')
    project.add('Geometry', _geometry(measured))
    path = tmp_path / 'frame.hforge'
    project.save(path)
    with h5py.File(path) as f:
        assert f.attrs[native.STAMP] == (native.MEASURED_POINT_SCHEMA if measured
                                         else native.BASE_SCHEMA)
    again = hertzforge.Project.open(path)['Geometry']
    assert int(np.sum(again.elem_type == MEASURED_POINT)) == int(measured)


def test_measured_points_decide_the_stamp_of_a_geometry_file(tmp_path):
    native.save(_geometry(True), tmp_path / 'points.hforge')
    with h5py.File(tmp_path / 'points.hforge') as f:
        assert f.attrs[native.STAMP] == native.MEASURED_POINT_SCHEMA


def test_an_element_type_this_build_does_not_know_is_a_newer_builds(tmp_path):
    project = hertzforge.Project('Frame')
    project.add('Geometry', _geometry(True))
    path = tmp_path / 'frame.hforge'
    project.save(path)
    with h5py.File(path, 'r+') as f:
        group = next(iter(f['objects'].values()))
        types = group['elem_type'][()]
        types[types == MEASURED_POINT] = 987
        group['elem_type'][...] = types
    with pytest.raises(ValueError, match='elements of type 987.*newer Hertzforge. Update'):
        hertzforge.Project.open(path)
