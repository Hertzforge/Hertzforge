"""New Geometry, the join that adds a part to it, and the add pane's
gizmo: a model built in the app.

Until 2026-09-26 the planes a plate model is made of could be meshed only
from a script (`mesh.plane`, `mesh.assemble`), and a geometry could only
be imported; the BARC's workflow page had to start in Python. The
project's **+** makes an empty geometry. Add Plane typed meshed planes
into it until 2026-10-26; a plate is now a solid on the Solids row,
meshed by Mesh Solids, and each part is tied to what is there where they
meet (`mesh.join`).
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge import mesh
from hertzforge.core.geometry import Geometry

INCH = 0.0254


def _empty(unit='m'):
    return Geometry(node_id=[], node_xyz=np.empty((0, 3)), length_unit=unit)


def test_joining_keeps_the_ids_already_there():
    """Data linked to a geometry names its nodes, so a plane joined to it
    leaves them alone: its own nodes that fall on them become them, the
    rest are numbered after the highest."""
    floor = mesh.plane((0, 0, 0), (4, 0, 0), (0, 2, 0), 1, 'floor')
    floor.node_id = floor.node_id + 100
    floor.elem_conn = [c + 100 for c in floor.elem_conn]
    before = floor.node_id.copy()
    wall = mesh.plane((0, 0, 0), (4, 0, 0), (0, 0, 2), 1, 'wall')
    found = mesh.join(floor, wall)
    assert found == {'added': 10, 'shared': 5, 'elements': 8, 'duplicates': 0,
                     'groups': [2]}
    assert np.array_equal(floor.node_id[:15], before)
    assert floor.node_id[15:].tolist() == list(range(116, 126))
    shared_line = [n for n in floor.elem_conn[8] if n < 116]  # its first
    assert shared_line == [101, 102], 'the wall is tied along the floor edge'


def test_a_block_of_the_same_name_is_joined_and_an_unnamed_one_is_not():
    geometry = mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0), 0.5, 'box')
    assert mesh.join(geometry, mesh.plane((1, 0, 0), (0, 1, 0), (0, 0, 1),
                                          0.5, 'box'))['groups'] == [1]
    assert mesh.join(geometry, mesh.plane((0, 0, 1), (1, 0, 0), (0, 1, 0),
                                          0.5, ''))['groups'] == [2]
    assert mesh.join(geometry, mesh.plane((0, 1, 0), (1, 0, 0), (0, 0, 1),
                                          0.5, ''))['groups'] == [3]
    assert list(geometry.group_name) == ['box', '', '']


def test_landing_says_what_joining_would_share_without_joining():
    floor = mesh.plane((0, 0, 0), (4, 0, 0), (0, 2, 0), 1, 'floor')
    wall = mesh.plane((0, 0, 0), (4, 0, 0), (0, 0, 2), 1, 'wall')
    on, rows = mesh.landing(floor, wall)
    assert on.sum() == 5 and floor.num_nodes == 15
    assert np.allclose(floor.node_xyz[rows[on]], wall.node_xyz[on])
    assert not mesh.landing(_empty(), wall)[0].any()


def test_a_part_kept_apart_joins_only_its_own_group():
    """Kept apart (2026-10-11), a plane lands only on the nodes of the
    element group it joins by name: a second floor plane still shares
    the first's edge, a foot laid on the floor keeps nodes of its own
    over it, coincident and unjoined, for a merge to join at the bolts."""
    floor = mesh.plane((0, 0, 0), (4, 0, 0), (0, 2, 0), 1, 'floor')
    foot = mesh.plane((1, 0, 0), (2, 0, 0), (0, 1, 0), 1, 'foot')
    on, _rows = mesh.landing(floor, foot, apart=True)
    assert not on.any() and mesh.landing(floor, foot)[0].sum() == 6
    found = mesh.join(floor, foot, apart=True)
    assert found['shared'] == 0 and found['added'] == 6
    assert len(floor.coincident_spots(0.1)) == 6, 'each over a floor node'
    more = mesh.plane((4, 0, 0), (1, 0, 0), (0, 2, 0), 1, 'floor')
    assert mesh.join(floor, more, apart=True)['shared'] == 3, \
        'the floor still joins the floor'
    assert mesh.landing(floor, mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0), 1),
                        apart=True)[0].sum() == 0, 'an unnamed group joins none'


def test_assembling_apart_keeps_each_group_whole_and_the_groups_unjoined():
    walls = [mesh.plane((0, 0, 0), (2, 0, 0), (0, 1, 0), 1, 'box'),
             mesh.plane((2, 0, 0), (2, 0, 0), (0, 1, 0), 1, 'box'),
             mesh.plane((1, 0, 0), (2, 0, 0), (0, 1, 0), 1, 'lid')]
    joined = mesh.assemble(*walls)
    apart = mesh.assemble(*walls, apart=True)
    assert joined.num_nodes == 10 and apart.num_nodes == 16
    lid = set(apart.elem_conn[4]) | set(apart.elem_conn[5])
    spots = apart.coincident_spots(0.1)
    assert len(spots) == 6 and all(b > 10 and b in lid and a <= 10
                                   for a, b in spots), spots


def test_coordinates_that_mean_different_things_do_not_join():
    raw = mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0), 0.5, unit=None)
    with pytest.raises(ValueError, match='do not mean the same thing'):
        mesh.join(raw, mesh.plane((0, 0, 0), (1, 0, 0), (0, 0, 1), 0.5))


def test_assembling_numbers_the_nodes_without_gaps():
    """Built on `join`, the assembly allocates no id to a node that lands
    on one already there, so its nodes run 1..n."""
    whole = mesh.assemble(
        mesh.plane((0, 0, 0), (4, 0, 0), (0, 2, 0), 1, 'floor'),
        mesh.plane((0, 0, 0), (4, 0, 0), (0, 0, 2), 1, 'wall'))
    assert whole.node_id.tolist() == list(range(1, 26))


def test_the_project_builds_a_model_from_nothing():
    """Plates added as solids and meshed, journaled as a script would
    write them; lengths in the unit named, held in SI. Two plates of one
    name are one element group, tied where they meet."""
    project = hertzforge.Project('p')
    name = project.new_geometry('Box', unit='in')
    assert project[name].num_nodes == 0 and project[name].length_unit == 'in'
    assert [verb for verb, _ in project.verbs(name)] == [
        'generate_rigid_body_modes',
        'add_solid_block', 'add_solid_cylinder', 'add_solid_cone', 'add_solid_sphere',
        'add_solid_section', 'add_solid_plate', 'add_solid_disk', 'set_view'], \
        'an empty geometry has no elements to merge or element groups to solve'
    floor = project.add_solid_plate(name, (2, 1, 0), (4, 2), name='box', unit='in')
    wall = project.add_solid_plate(name, (2, 0, 1), (4, 2), (90, 0, 0), 'box', unit='in')
    found = project.mesh_solids(name, [floor, wall], 1, unit='in', elements='quads')
    box = project[name]
    assert (found['added'], box.num_nodes, len(box.elem_conn)) == (25, 25, 16), \
        'fifteen and fifteen, five of them shared along the edge'
    assert np.allclose(box.node_xyz.max(axis=0), [4 * INCH, 2 * INCH,
                                                  2 * INCH])
    assert list(box.group_name) == ['box']
    assert project.journal[-3:] == [
        "project.add_solid_plate('Box', (2, 1, 0), (4, 2), name='box', unit='in')",
        "project.add_solid_plate('Box', (2, 0, 1), (4, 2), (90, 0, 0), 'box', unit='in')",
        "project.mesh_solids('Box', [1, 2], 1, unit='in', elements='quads')"]


def test_a_geometry_without_units_takes_the_numbers_as_given():
    project = hertzforge.Project('p')
    raw = mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0), 0.5, 'floor',
                     unit=None)
    project.add('Raw', raw)
    wall = project.add_solid_plate('Raw', (0.5, 0, 0.5), (1, 1), (90, 0, 0), 'wall',
                                   unit='in')
    project.mesh_solids('Raw', [wall], 0.5, unit='in', elements='quads')
    assert not raw.units_defined
    assert np.isclose(raw.node_xyz[:, 2].max(), 1.0)
    assert raw.num_nodes == 15, 'tied to the floor along their edge'
