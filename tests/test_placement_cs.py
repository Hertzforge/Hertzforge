"""A node's coordinates are relative to its placement coordinate system.

Brandon, 2026-11-11: "If the placement coordinate system is moved or
rotated, all nodes should retain the same coordinates relative to that
placement coordinate system and should therefore move." And for a node
given another placement system: "I want to keep the numbers." The node
table showed global positions and a turned system left its nodes where
they were; the Placement CS column named a system the numbers were not
in. Positions are still stored in global, so everything that reads them
is unchanged.
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.geometry import Geometry
from hertzforge.rotate import frame_from_angles


def _geometry():
    """Three nodes placed in system 2 (origin (1, 0, 0)), one in the
    global system 1, and a cylindrical system 3."""
    geometry = Geometry(node_id=[1, 2, 3, 4],
                        node_xyz=[[1.0, 0.0, 0.0], [1.0, 1.0, 0.0],
                                  [1.0, 0.0, 2.0], [5.0, 5.0, 5.0]],
                        node_def_cs=[2, 2, 2, 1], node_disp_cs=[2, 2, 2, 1])
    geometry.add_coordinate_system(origin=(1.0, 0.0, 0.0), cs_id=2)
    geometry.add_coordinate_system(origin=(0.0, 0.0, 0.0), cs_id=3, cs_type=1)
    return geometry


def test_the_local_coordinates_are_in_the_placement_system():
    geometry = _geometry()
    assert np.allclose(geometry.node_local_xyz(), [[0, 0, 0], [0, 1, 0],
                                                   [0, 0, 2], [5, 5, 5]])
    geometry.set_node_local_xyz(1, (0.0, 3.0, 0.0))
    assert np.allclose(geometry.node_xyz[1], [1.0, 3.0, 0.0])


def test_a_turned_system_carries_its_nodes():
    geometry = _geometry()
    before = geometry.node_local_xyz()
    geometry.place_coordinate_system(2, angles=(90.0, 0.0, 0.0))
    assert np.allclose(geometry.node_local_xyz(), before), \
        'the same coordinates in the system'
    # a quarter turn about X takes local Y to global Z and local Z to -Y
    assert np.allclose(geometry.node_xyz[1], [1.0, 0.0, 1.0])
    assert np.allclose(geometry.node_xyz[2], [1.0, -2.0, 0.0])
    assert np.allclose(geometry.node_xyz[3], [5.0, 5.0, 5.0]), \
        'a node in another system stays put'


def test_a_moved_system_carries_its_nodes():
    geometry = _geometry()
    geometry.place_coordinate_system(2, origin=(1.0, 0.0, 10.0))
    assert np.allclose(geometry.node_xyz[:3, 2], [10.0, 10.0, 12.0])
    assert np.allclose(geometry.node_xyz[3], [5.0, 5.0, 5.0])
    geometry.place_coordinate_system(
        2, rotation=frame_from_angles((0.0, 0.0, 90.0))[:3])
    assert np.allclose(geometry.node_xyz[1], [0.0, 0.0, 10.0]), \
        'local Y turned to global -X about the moved origin'


def test_a_cylindrical_system_keeps_r_theta_and_z():
    geometry = _geometry()
    geometry.set_node_placement(0, 3)
    geometry.set_node_local_xyz(0, (2.0, 90.0, 1.0))
    assert np.allclose(geometry.node_xyz[0], [0.0, 2.0, 1.0])
    geometry.place_coordinate_system(3, origin=(0.0, 0.0, 4.0))
    assert np.allclose(geometry.node_local_xyz([0])[0], [2.0, 90.0, 1.0])
    assert np.allclose(geometry.node_xyz[0], [0.0, 2.0, 5.0])


def test_another_placement_system_keeps_the_numbers():
    geometry = _geometry()
    geometry.set_node_placement([1], 1)
    assert geometry.node_def_cs[1] == 1
    assert np.allclose(geometry.node_xyz[1], [0.0, 1.0, 0.0]), \
        '(0, 1, 0) now read in the global system'
    with pytest.raises(ValueError, match='no coordinate system 9'):
        geometry.set_node_placement([1], 9)


def test_another_type_keeps_the_numbers():
    geometry = _geometry()
    geometry.set_node_placement(1, 3)
    geometry.set_node_local_xyz(1, (2.0, 30.0, 1.0))
    geometry.set_coordinate_system_type(3, 0)
    assert np.allclose(geometry.node_xyz[1], [2.0, 30.0, 1.0]), \
        'r, theta, z read again as x, y, z'
    geometry.set_node_local_xyz(1, (1.0, 0.0, 0.0))
    geometry.set_coordinate_system_type(3, 1)
    assert np.allclose(geometry.node_xyz[1], [1.0, 0.0, 0.0]), \
        '(1, 0, 0) as r, theta, z is the same point'
    geometry.set_node_local_xyz(1, (1.0, 90.0, 0.0))
    geometry.set_coordinate_system_type(3, 0)
    assert np.allclose(geometry.node_xyz[1], [1.0, 90.0, 0.0])
    with pytest.raises(ValueError, match='type is one of'):
        geometry.set_coordinate_system_type(3, 7)
    with pytest.raises(KeyError):
        geometry.set_coordinate_system_type(8, 0)


def test_a_file_written_after_a_turn_keeps_the_numbers(tmp_path):
    """The universal file gives each node in its placement system, so a
    turned system writes the same numbers as before it turned."""
    from hertzforge.io import unv

    geometry = _geometry()
    geometry.define_units('m')
    geometry.place_coordinate_system(2, angles=(30.0, 40.0, 50.0))
    path = tmp_path / 'turned.unv'
    unv.save(geometry, path)
    again = hertzforge.import_file(path)
    if isinstance(again, dict):
        again = next(v for v in again.values() if isinstance(v, Geometry))
    assert np.allclose(again.node_xyz, geometry.node_xyz, atol=1e-9)
    assert np.allclose(again.node_local_xyz()[:3], [[0, 0, 0], [0, 1, 0],
                                                    [0, 0, 2]], atol=1e-9)


# ---- the entity tables, as the shell types into them --------------------------


@pytest.fixture
def project():
    made = hertzforge.Project()
    made.add('Geometry', _geometry())
    return made


def _cell(project, kind, row, column, value):
    from hertzforge.service.ops_geometry import set_entity_cell

    return set_entity_cell(project, {'geometry': 'Geometry', 'kind': kind, 'row': row,
                                     'column': column, 'value': value})


def _shown_rows(project, kind):
    from hertzforge.service.ops_geometry import entity_rows

    geometry = project['Geometry']
    count = len(geometry.node_id if kind == 'nodes' else geometry.cs_id)
    return entity_rows(geometry, kind, range(count))


def test_the_node_table_shows_and_takes_placement_coordinates(project):
    geometry = project['Geometry']
    geometry.place_coordinate_system(2, angles=(0.0, 0.0, 90.0))
    rows = _shown_rows(project, 'nodes')
    assert [float(c) for c in rows[1][1:4]] == pytest.approx([0.0, 1.0, 0.0], abs=1e-9), \
        'node 2 in system 2, not its global place'
    _cell(project, 'nodes', 2, 'y', '3')
    assert geometry.node_local_xyz([1])[0] == pytest.approx([0.0, 3.0, 0.0], abs=1e-12)
    assert geometry.node_xyz[1] == pytest.approx([-2.0, 0.0, 0.0], abs=1e-12)
    before = geometry.node_local_xyz([1])[0]
    _cell(project, 'nodes', 2, 'placement_cs', '1')
    assert int(geometry.node_def_cs[1]) == 1
    assert geometry.node_local_xyz([1])[0] == pytest.approx(before, abs=1e-12), \
        'the same numbers in the other system'
    assert geometry.node_xyz[1] == pytest.approx(before, abs=1e-12), 'so it moved'


def test_the_system_table_carries_the_nodes(project):
    geometry = project['Geometry']
    local = geometry.node_local_xyz([0, 1, 2])
    other = np.array(geometry.node_xyz[3])
    _cell(project, 'coordinate_systems', 2, 'origin_z', '1')
    assert np.allclose(geometry.node_local_xyz([0, 1, 2]), local)
    assert np.allclose(geometry.node_xyz[0], [1.0, 0.0, 1.0])
    _cell(project, 'coordinate_systems', 2, 'rotation_x', '90')
    assert np.allclose(geometry.node_local_xyz([0, 1, 2]), local)
    assert np.allclose(geometry.node_xyz[3], other), 'not placed in it'
    # away from r = 0, where theta means nothing
    _cell(project, 'nodes', 2, 'x', '2')
    _cell(project, 'nodes', 2, 'y', '30')
    _cell(project, 'nodes', 2, 'z', '1')
    _cell(project, 'coordinate_systems', 2, 'type', 'cylindrical')
    assert int(geometry.cs_type[1]) == 1
    assert geometry.node_local_xyz([1])[0] == pytest.approx([2.0, 30.0, 1.0]), \
        'the numbers kept, now as r, theta, z'


def test_a_typed_turn_and_a_reset_carry_the_nodes(project):
    from hertzforge.service.ops_geometry import (
        reset_rotation,
        rotate_coordinate_system,
        slide_coordinate_system,
    )

    geometry = project['Geometry']
    local = geometry.node_local_xyz([0, 1, 2])
    start = np.array(geometry.node_xyz[:3])
    rotate_coordinate_system(project, {'geometry': 'Geometry', 'cs': 2, 'axis': 'z',
                                       'angle': 90.0})
    assert np.allclose(geometry.node_local_xyz([0, 1, 2]), local)
    assert not np.allclose(geometry.node_xyz[:3], start), 'they moved'
    reset_rotation(project, {'geometry': 'Geometry', 'cs': 2})
    assert np.allclose(geometry.node_xyz[:3], start), 'and came back'
    slide_coordinate_system(project, {'geometry': 'Geometry', 'cs': 2, 'axis': 'x',
                                      'distance': 0.5})
    assert np.allclose(geometry.node_xyz[:3], start + [0.5, 0.0, 0.0])
    assert project.journal[-1].startswith("project['Geometry'].place_coordinate_system(2")


def test_a_deleted_system_leaves_its_nodes_their_numbers():
    """Its nodes go to the first remaining system with the same numbers,
    so they move (Brandon: "Keep the numbers on delete too, nodes should
    move")."""
    geometry = _geometry()
    geometry.place_coordinate_system(2, angles=(0.0, 0.0, 90.0))
    local = geometry.node_local_xyz([0, 1, 2])
    said = geometry.delete_coordinate_systems([2])
    assert said == {'coordinate_systems': 1, 'nodes_reassigned': 6}
    assert list(geometry.node_def_cs[:3]) == [1, 1, 1]
    assert np.allclose(geometry.node_local_xyz([0, 1, 2]), local)
    assert np.allclose(geometry.node_xyz[:3], local), \
        'system 1 is the global system here, so the numbers are the place'
    assert np.allclose(geometry.node_xyz[3], [5.0, 5.0, 5.0])


def test_an_angle_in_the_table_is_degrees_in_any_units(project):
    """r and z are lengths in the stored unit; theta is degrees, never
    scaled as a length."""
    geometry = project['Geometry']
    geometry.define_units('in')
    geometry.set_node_placement(0, 3)
    geometry.set_node_local_xyz(0, (0.0254, 90.0, 0.0508))
    shown = [float(c) for c in _shown_rows(project, 'nodes')[0][1:4]]
    assert shown == pytest.approx([0.0254, 90.0, 0.0508])
    _cell(project, 'nodes', 1, 'y', '45')
    assert geometry.node_local_xyz([0])[0] == pytest.approx([0.0254, 45.0, 0.0508])
