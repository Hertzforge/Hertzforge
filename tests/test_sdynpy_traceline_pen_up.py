"""A 0 in an sdynpy traceline lifts the pen; it does not name a node."""

import numpy as np
import pytest

from hertzforge.io import sdynpy_npz
from hertzforge.project import Project


def _file(path, *connectivities):
    node = np.zeros(4, dtype=sdynpy_npz.NODE_DTYPE)
    node['id'] = [1, 2, 3, 4]
    node['coordinate'] = np.arange(12.0).reshape(4, 3)
    node['color'] = 1
    node['def_cs'] = 1
    node['disp_cs'] = 1
    cs = np.zeros(1, dtype=sdynpy_npz.CS_DTYPE)
    cs['id'] = 1
    cs['matrix'] = np.concatenate((np.eye(3), np.zeros((1, 3))))
    lines = np.zeros(len(connectivities), dtype=sdynpy_npz.TRACELINE_DTYPE)
    lines['id'] = np.arange(len(connectivities)) + 1
    lines['color'] = 1
    lines['description'] = 'line'
    for i, connectivity in enumerate(connectivities):
        lines['connectivity'][i] = np.asarray(connectivity, dtype=np.uint32)
    np.savez(path, node=node, coordinate_system=cs, traceline=lines)
    return path


def _chains(geometry):
    return [[list(map(int, c)) for c in line['chains']]
            for line in geometry.drawn_lines()]


def test_a_trailing_zero_ends_the_line(tmp_path):
    geometry = sdynpy_npz.load(_file(tmp_path / 'g.npz', [1, 2, 3], [3, 4, 1, 0]))
    assert _chains(geometry) == [[[1, 2, 3]], [[3, 4, 1]]]


def test_a_zero_inside_a_line_splits_it_under_one_id(tmp_path):
    geometry = sdynpy_npz.load(_file(tmp_path / 'g.npz', [1, 2, 0, 3, 4]))
    assert _chains(geometry) == [[[1, 2], [3, 4]]]


def test_a_lone_node_between_zeros_draws_nothing(tmp_path):
    geometry = sdynpy_npz.load(_file(tmp_path / 'g.npz', [0, 1, 0, 3, 4, 0], [0, 2, 0]))
    assert _chains(geometry) == [[[3, 4]]]


def test_a_node_the_geometry_lacks_is_still_refused(tmp_path):
    with pytest.raises(ValueError, match='unknown nodes'):
        sdynpy_npz.load(_file(tmp_path / 'g.npz', [1, 9]))


def test_the_project_imports_it_and_it_comes_back_the_same(tmp_path):
    path = _file(tmp_path / 'g.npz', [1, 2, 0, 3, 4, 0])
    project = Project()
    project.import_file(str(path))
    geometry = next(iter(project.values()))
    sdynpy_npz.save(geometry, tmp_path / 'back.npz')
    assert _chains(sdynpy_npz.load(tmp_path / 'back.npz')) == [[[1, 2], [3, 4]]]


def test_the_writer_joins_a_lines_runs_with_zeros_one_row_per_line(tmp_path):
    """One row per run gave sdynpy several tracelines sharing an id; the
    layout stores one row per line with 0 between its runs."""
    geometry = sdynpy_npz.load(_file(tmp_path / 'g.npz', [0, 1, 2, 0, 0, 3, 4, 0], [2, 3]))
    sdynpy_npz.save(geometry, tmp_path / 'back.npz')
    with np.load(tmp_path / 'back.npz', allow_pickle=True) as raw:
        rows = [[int(n) for n in c] for c in raw['traceline']['connectivity']]
        ids = raw['traceline']['id'].tolist()
    assert rows == [[1, 2, 0, 3, 4], [2, 3]]
    assert len(set(ids)) == len(ids) == 2
    assert _chains(sdynpy_npz.load(tmp_path / 'back.npz')) == [[[1, 2], [3, 4]], [[2, 3]]]


def test_a_drawn_line_through_node_zero_is_not_written(tmp_path):
    """The layout reads a 0 as a break, so the line would come back cut."""
    from hertzforge.core.geometry import Geometry

    geometry = Geometry(np.array([0, 1, 2]), np.eye(3))
    geometry.attach_drawn_lines([('line', 1, [[0, 1, 2]])])
    with pytest.raises(ValueError, match='node 0'):
        sdynpy_npz.save(geometry, tmp_path / 'zero.npz')
