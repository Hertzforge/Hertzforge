"""Plate meshes built from planes, tied where they meet (Brandon,
2026-09-26, for the BARC example).

Plates connect only through shared nodes, so planes meshed apart are
tied by merging the nodes they share along the lines where their
mid-surfaces meet. The proofs: an L of two planes shares its corner
line's nodes and solves as one structure; a box built from four planes
equals the same box meshed by hand; the merge refuses to fold an element
onto itself, keeps the lowest id, and refuses while linked data names a
node it would remove.
"""

from __future__ import annotations

import numpy as np
import pytest
from test_fem import ALUMINUM

from hertzforge.core import mesh
from hertzforge.core.fem import GroupProperties, Model
from hertzforge.core.geometry import Geometry
from hertzforge.core.merge import merge
from hertzforge.core.shapes import ShapeSet
from hertzforge.project import Project


def test_a_plane_is_a_block_of_rectangles():
    floor = mesh.plane((0, 0, 0), (0.4, 0, 0), (0, 0.2, 0), 0.1, 'floor')
    assert floor.num_nodes == 15 and len(floor.elem_conn) == 8
    assert list(floor.group_name) == ['floor'] and set(floor.elem_type) == {44}
    assert floor.length_unit == 'm'
    with pytest.raises(ValueError, match='not perpendicular'):
        mesh.plane((0, 0, 0), (1, 0, 0), (1, 1, 0), 0.5, 'skew')


def test_a_plane_takes_its_lengths_in_a_unit_and_holds_si():
    inch = mesh.plane((0, 0, 0), (6, 0, 0), (0, 3, 0), 0.5, 'wall', unit='in')
    assert inch.length_unit == 'in'
    assert inch.node_xyz[:, 0].max() == pytest.approx(6 * 0.0254)
    assert inch.num_nodes == 13 * 7


def test_elements_come_out_close_to_square_and_evenly_spaced():
    """Each edge divided evenly into the whole number of elements nearest
    the size (Brandon, 2026-09-26): a 0.13 by 1 strip at 0.125 is one
    element across, not two of 0.065 beside 0.125 lengthwise."""
    strip = mesh.plane((0, 0, 0), (1, 0, 0), (0, 0.13, 0), 0.125, 'strip')
    xs = np.unique(np.round(strip.node_xyz[:, 0], 9))
    ys = np.unique(np.round(strip.node_xyz[:, 1], 9))
    assert len(xs) == 9 and len(ys) == 2
    assert np.allclose(np.diff(xs), 0.125), 'even along its length'
    sides = (np.diff(xs)[0], np.diff(ys)[0])
    assert max(sides) / min(sides) < 1.1, 'close to square'
    # a size larger than an edge still gives that edge one element
    assert mesh.plane((0, 0, 0), (0.05, 0, 0), (0, 1, 0), 0.2, 'x').num_nodes == 12


def test_two_planes_meeting_share_their_corner_line():
    floor = mesh.plane((0, 0, 0), (0.4, 0, 0), (0, 0.2, 0), 0.1, 'floor')
    wall = mesh.plane((0, 0, 0), (0.4, 0, 0), (0, 0, 0.2), 0.1, 'wall')
    both = mesh.assemble(floor, wall)
    assert both.num_nodes == 15 + 15 - 5, 'the corner line shared'
    assert list(both.group_name) == ['floor', 'wall']
    both.group_properties = {1: GroupProperties(ALUMINUM, 0.003),
                             2: GroupProperties(ALUMINUM, 0.003)}
    model = Model.from_geometry(both)
    assert len(model.pieces()) == 1
    assert int(np.sum(model.eigensolution(num_modes=8).frequency == 0)) == 6


def test_a_box_from_planes_is_the_box_meshed_whole():
    """Four walls of a square tube as four planes, against the same tube
    meshed as one strip wrapped around it: the same model, to rounding."""
    s, h, size = 0.1, 0.05, 0.025
    walls = [mesh.plane((0, 0, 0), (s, 0, 0), (0, 0, h), size, 'bottom'),
             mesh.plane((s, 0, 0), (0, s, 0), (0, 0, h), size, 'right'),
             mesh.plane((s, s, 0), (-s, 0, 0), (0, 0, h), size, 'top'),
             mesh.plane((0, s, 0), (0, -s, 0), (0, 0, h), size, 'left')]
    box = mesh.assemble(*walls)
    box.group_properties = {b: GroupProperties(ALUMINUM, 0.002)
                            for b in range(1, 5)}
    built = Model.from_geometry(box)
    # the same tube by hand: nodes around the perimeter, up the height
    whole = Model()
    ring = [(x, 0) for x in np.arange(0, s, size)] + \
        [(s, y) for y in np.arange(0, s, size)] + \
        [(x, s) for x in np.arange(s, 0, -size)] + \
        [(0, y) for y in np.arange(s, 0, -size)]
    levels = np.arange(0, h + 1e-12, size)
    ids = {}
    for k, level in enumerate(levels):
        for i, (x, y) in enumerate(ring):
            ids[(i, k)] = whole.add_node(1000 * k + i + 1, x, y, level)
    for k in range(len(levels) - 1):
        for i in range(len(ring)):
            j = (i + 1) % len(ring)
            whole.add_plate([ids[(i, k)], ids[(j, k)], ids[(j, k + 1)],
                             ids[(i, k + 1)]], ALUMINUM, 0.002)
    a = built.eigensolution(num_modes=16).frequency
    b = whole.eigensolution(num_modes=16).frequency
    assert np.allclose(a, b, rtol=1e-9, atol=1e-6)


def test_the_merge_keeps_the_lowest_id_and_refuses_to_fold_an_element():
    geometry = Geometry(node_id=[7, 3, 5, 9],
                        node_xyz=[[0, 0, 0], [0, 0, 0], [1, 0, 0], [1, 1e-9, 0]],
                        elem_conn=[[7, 5], [3, 9]], elem_type=[21, 21],
                        length_unit='m')
    found = geometry.merge_coincident_nodes(1e-6)
    assert found == {'merged': 2, 'into': 2, 'duplicates': 0, 'links': 0}, (
        'two beams on one line can be two members: kept')
    assert sorted(geometry.node_id) == [3, 5]
    assert [list(c) for c in geometry.elem_conn] == [[3, 5], [3, 5]]
    square = mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0), 0.5, 'p')
    with pytest.raises(ValueError, match='folds element 1 onto itself'):
        square.merge_coincident_nodes(0.6)


def test_the_verb_refuses_while_linked_data_names_a_node_it_would_remove():
    project = Project()
    floor = mesh.plane((0, 0, 0), (0.4, 0, 0), (0, 0.2, 0), 0.1, 'floor')
    wall = mesh.plane((0, 0, 0), (0.4, 0, 0), (0, 0, 0.2), 0.1, 'wall')
    joined = merge(
        [floor, _renumbered(wall, 100)])
    project.add('Geometry', joined)
    shapes = ShapeSet([10.0], [0.01], ['101Z+'], [[1.0]])
    project.add('Modes', shapes)
    project.link('Geometry', 'Modes')
    with pytest.raises(ValueError, match='Modes names node 101, which the '
                                          'merge would remove'):
        project.merge_coincident_nodes('Geometry')
    project.unlink('Modes')
    assert project.merge_coincident_nodes('Geometry') == {'merged': 5,
                                                          'into': 5,
                                                          'duplicates': 0,
                                                          'links': 0}
    assert "project.merge_coincident_nodes('Geometry')" in project.journal[-1]


def _renumbered(geometry, offset):
    geometry.node_id = geometry.node_id + offset
    geometry.elem_conn = [c + offset for c in geometry.elem_conn]
    return geometry


def _three_planes(unit=None):
    """A floor, a wall on its y = 0 edge and a shelf on its y = 2 edge,
    each its own element group (1, 2, 3) and none yet sharing a node:
    two lines of five coincident pairs."""
    whole = mesh.plane((0, 0, 0), (4, 0, 0), (0, 2, 0), 1, 'floor', unit=unit)
    for corner, name in (((0, 0, 0), 'wall'), ((0, 2, 0), 'shelf')):
        # a negative tolerance lands nothing: kept apart to be merged
        mesh.join(whole, mesh.plane(corner, (4, 0, 0), (0, 0, 2), 1, name,
                                    unit=unit), -1.0)
    return whole


def _shared(geometry, a, b):
    return set(geometry.nodes_in([a])) & set(geometry.nodes_in([b]))


def test_a_merge_of_element_groups_ties_what_they_hold_and_nothing_else():
    """The floor and the wall picked: their line is tied, and the floor
    and the shelf, coincident just as much, stay apart (Brandon,
    2026-10-11: a whole geometry's merge reached too far)."""
    geometry = _three_planes()
    assert geometry.merge_coincident_nodes(1e-6, groups=[1, 2]) == {
        'merged': 5, 'into': 5, 'duplicates': 0, 'links': 0}
    assert len(_shared(geometry, 1, 2)) == 5
    assert not _shared(geometry, 1, 3)
    assert geometry.num_nodes == 40
    assert geometry.merge_coincident_nodes(1e-6, groups=[2]) == {
        'merged': 0, 'into': 0, 'duplicates': 0, 'links': 0}, (
        'one group touches nothing')
    with pytest.raises(ValueError, match='no element group 9'):
        geometry.merge_coincident_nodes(1e-6, groups=[1, 9])
    whole = _three_planes()
    assert whole.merge_coincident_nodes(1e-6)['merged'] == 10, (
        'left out, the whole geometry, as before')


def test_a_scoped_merge_makes_one_only_the_cells_of_its_own_groups():
    """Two bricks filling one cell in a third group are that group's
    business: a merge of two other groups leaves them, and makes one
    only the cell its own merge doubled."""
    whole = mesh.block((0, 0, 0), (1, 0, 0), (0, 1, 0), (0, 0, 1), 1, 'a', unit=None)
    mesh.join(whole, mesh.block((0, 0, 0), (1, 0, 0), (0, 1, 0), (0, 0, 1), 1, 'b',
                                unit=None), -1.0)
    corner = mesh.block((5, 0, 0), (1, 0, 0), (0, 1, 0), (0, 0, 1), 1, 'c', unit=None)
    mesh.join(whole, corner, -1.0)
    third = whole.elements_in('c')
    whole.add_elements([whole.elem_conn[list(whole.elem_id).index(third[0])]],
                       [115], [3])
    found = whole.merge_coincident_nodes(1e-6, groups=[1, 2])
    assert (found['merged'], found['duplicates']) == (8, 1)
    assert len(whole.elements_in('c')) == 2, "the third group's pair stays"


def test_the_verb_takes_element_groups_and_journals_them():
    """A node a linked object names is a refusal only when this merge
    would remove it: the shelf's nodes are safe from a floor-and-wall
    merge."""
    project = Project()
    geometry = _three_planes()
    project.add('Geometry', geometry)
    # the shelf's own node on the floor's far edge, which a merge of the
    # whole geometry folds into the floor's
    on_edge = [n for n in geometry.nodes_in([3])
               if np.allclose(geometry.node_xyz[geometry.node_index([n])[0]],
                              [4.0, 2.0, 0.0])]
    project.add('Modes', ShapeSet([10.0], [0.01], [f'{on_edge[0]}Z+'], [[1.0]]))
    project.link('Geometry', 'Modes')
    with pytest.raises(ValueError, match='which the merge would remove'):
        project.merge_coincident_nodes('Geometry')
    assert project.merge_coincident_nodes('Geometry', groups=[1, 2])['merged'] == 5
    assert project.journal[-1] == (
        "project.merge_coincident_nodes('Geometry', groups=[1, 2])")
    with pytest.raises(ValueError, match='no element group 9'):
        project.merge_coincident_nodes('Geometry', groups=[9])


def test_the_places_a_merge_would_make_one_are_listed_before_it():
    """Each place the merge would make one node, its nodes ascending so
    the first is the one kept; a node excluded is in no place, and a
    place left with one node is no place at all (2026-10-11)."""
    geometry = _three_planes()
    spots = geometry.coincident_spots(1e-6, groups=[1, 2])
    assert len(spots) == 5 and all(len(spot) == 2 for spot in spots)
    assert all(spot == sorted(spot) for spot in spots)
    assert [spot[0] for spot in spots] == sorted(spot[0] for spot in spots)
    assert len(geometry.coincident_spots(1e-6)) == 10, 'both lines, every group'
    fewer = geometry.coincident_spots(1e-6, groups=[1, 2], exclude=spots[0][1:])
    assert spots[0] not in fewer and len(fewer) == 4
    assert geometry.merge_folds(1e-6, groups=[1, 2]) is None
    assert 'folds element' in geometry.merge_folds(1.5, groups=[1, 2])


def test_the_mesh_spacing_is_the_shortest_gap_inside_an_element():
    """A brick's shortest edge, a quad8's half edge, a group's own
    elements only, and nothing to read without an element of two
    distinct nodes."""
    assert _three_planes().node_spacing() == pytest.approx(1.0)
    fine = mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0), 0.25, 'fine', unit=None)
    mesh.join(fine, mesh.plane((0, 0, 1), (4, 0, 0), (0, 4, 0), 1, 'coarse',
                               unit=None), -1.0)
    assert fine.node_spacing() == pytest.approx(0.25)
    assert fine.node_spacing(groups=[2]) == pytest.approx(1.0)
    brick = mesh.block((0, 0, 0), (2, 0, 0), (0, 3, 0), (0, 0, 0.5), 4, unit=None)
    assert brick.node_spacing() == pytest.approx(0.5)
    quad8 = Geometry(node_id=np.arange(1, 9),
                     node_xyz=[[0, 0, 0], [2, 0, 0], [2, 2, 0], [0, 2, 0],
                               [1, 0, 0], [2, 1, 0], [1, 2, 0], [0, 1, 0]])
    quad8.add_elements([list(range(1, 9))], [95], [1])
    assert quad8.node_spacing() == pytest.approx(1.0)
    assert Geometry(node_id=[1], node_xyz=[[0, 0, 0]]).node_spacing() is None


def test_a_merge_leaves_out_what_it_is_told_to():
    geometry = _three_planes()
    spots = geometry.coincident_spots(1e-6, groups=[1, 2])
    left = spots[:2]
    found = geometry.merge_coincident_nodes(
        1e-6, groups=[1, 2], exclude=[n for spot in left for n in spot])
    assert found == {'merged': 3, 'into': 3, 'duplicates': 0, 'links': 0}
    assert all(set(spot) <= set(geometry.node_id.tolist()) for spot in left), (
        'both nodes of each place left out are still there')
    project = Project()
    project.add('Geometry', _three_planes())
    project.merge_coincident_nodes('Geometry', 1e-6, groups=[1, 2],
                                   exclude=[n for spot in left for n in spot])
    assert project.journal[-1].endswith(
        f'groups=[1, 2], exclude={[n for spot in left for n in spot]})')


def _linked_planes(unit=None):
    """A floor, and a wall standing a thousandth above its y = 0 edge
    with a rigid link across the gap at each of the five pairs — the
    BARC's beam on its channels, as it was built (2026-10-11)."""
    whole = mesh.plane((0, 0, 0), (4, 0, 0), (0, 2, 0), 1, 'floor', unit=unit)
    mesh.join(whole, mesh.plane((0, 0, 0.001), (4, 0, 0), (0, 0, 2), 1, 'wall',
                                unit=unit), -1.0)
    scale = 0.0254 if unit == 'in' else 1.0
    pairs = [(a, b) for a in whole.nodes_in([1]) for b in whole.nodes_in([2])
             if np.linalg.norm(whole.node_xyz[whole.node_index([a])[0]]
                               - whole.node_xyz[whole.node_index([b])[0]])
             < 0.01 * scale]
    group = whole.add_group('ties')
    whole.add_elements([list(pair) for pair in pairs], [121] * len(pairs),
                       [group] * len(pairs))
    return whole


def test_a_link_across_a_gap_the_merge_closes_goes_with_it():
    """Merging the gap's pairs leaves each link joining a node to
    itself: it is removed, and said, rather than the merge refused for
    folding it (Brandon, 2026-10-11)."""
    geometry = _linked_planes()
    assert geometry.merge_folds(0.25, groups=[1, 2]) is None
    assert len(geometry.merge_collapses(0.25, groups=[1, 2])) == 5
    spots = geometry.coincident_spots(0.25, groups=[1, 2])
    assert len(geometry.merge_collapses(0.25, groups=[1, 2],
                                        exclude=spots[0])) == 4
    found = geometry.merge_coincident_nodes(0.25, groups=[1, 2])
    assert (found['merged'], found['links']) == (5, 5)
    assert geometry.elements_in('ties') == []


def test_planes_of_one_name_are_one_block():
    """Five walls named 'box' are the box — one block, given its material
    once; an unnamed plane stays a block of its own."""
    walls = [mesh.plane((0, 0, 0), (0.1, 0, 0), (0, 0, 0.05), 0.025, 'box'),
             mesh.plane((0.1, 0, 0), (0, 0.1, 0), (0, 0, 0.05), 0.025, 'box'),
             mesh.plane((0, 0, 0.05), (0.1, 0, 0), (0, 0.1, 0), 0.025, 'lid'),
             mesh.plane((0, 0, 0), (0, 0.1, 0), (0, 0, 0.05), 0.025, ''),
             mesh.plane((0, 0.1, 0), (0.1, 0, 0), (0, 0, 0.05), 0.025, '')]
    whole = mesh.assemble(*walls)
    assert list(whole.group_name) == ['box', 'lid', '', '']
    assert len(whole.elements_in('box')) == 2 * len(walls[0].elem_conn)
