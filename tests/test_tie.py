"""Tie: a patch of elements tied rigidly to the part under it.

A bolted joint had been two clicks per rigid link in add mode — 148 for
the BARC. Brandon (2026-09-26): select the elements under a washer, one
at a time or several, and tie them to the part below in one step.
`mesh.tie` is the rule; `Project.tie_elements` records it; the Elements
edit bar's Tie button offers it for a selection.
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge import fem, mesh
from hertzforge.core.fem import RIGID


def _floor_and_foot():
    """A floor plate and, 0.25 above it, a smaller foot plate: nothing
    shared, as a bolted foot's mid-surface is not the floor's."""
    geometry = mesh.assemble(
        mesh.plane((0, 0, 0), (4, 0, 0), (0, 4, 0), 1, 'floor'),
        mesh.plane((1, 1, 0.25), (2, 0, 0), (0, 2, 0), 1, 'foot'))
    aluminum = fem.material('6061-T6')
    geometry.group_properties = {1: fem.GroupProperties(aluminum, 0.01),
                                 2: fem.GroupProperties(aluminum, 0.01)}
    return geometry


def test_a_patch_ties_to_the_nearest_nodes_of_a_block():
    geometry = _floor_and_foot()
    patch = geometry.elements_in('foot')[:1]             # one element
    found = mesh.tie(geometry, patch, 'floor')
    assert found['links'] == 4 and found['shared'] == 0
    assert geometry.group_name[-1] == 'ties'
    assert geometry.group_properties[found['group']].material is RIGID
    links = geometry.elements_in('ties')
    rows = np.flatnonzero(np.isin(geometry.elem_id, links))
    for row in rows:
        leader, follower = geometry.elem_conn[row]
        a, b = geometry.node_xyz[geometry.node_index([leader, follower])]
        assert np.allclose(a[:2], b[:2]) and np.isclose(b[2] - a[2], 0.25), \
            'each foot node to the floor node straight under it'


def test_a_second_tie_joins_the_rigid_block_and_shared_nodes_are_left():
    geometry = _floor_and_foot()
    foot = geometry.elements_in('foot')
    first = mesh.tie(geometry, foot[:1], 'floor')
    second = mesh.tie(geometry, foot[1:2], 'floor')
    assert second['group'] == first['group'], 'one element group of ties'
    assert second['links'] == 4
    floor = geometry.elements_in('floor')
    with pytest.raises(ValueError, match='every node is shared'):
        mesh.tie(geometry, floor[:1], floor[:2])


def test_ties_join_the_rigid_block_already_there_whatever_its_name():
    """The demo's links are in 'bolts'; a tie made in the app on its
    project joins them rather than starting an element group of its own."""
    geometry = _floor_and_foot()
    foot = geometry.elements_in('foot')
    mesh.tie(geometry, foot[:1], 'floor', group='bolts')
    found = mesh.tie(geometry, foot[1:2], 'floor')
    assert geometry.group_name[list(geometry.group_id).index(
        found['group'])] == 'bolts'
    assert 'ties' not in geometry.group_name


def test_a_patch_ties_to_a_second_patch():
    geometry = _floor_and_foot()
    floor = geometry.elements_in('floor')
    found = mesh.tie(geometry, geometry.elements_in('foot')[:1], floor[5:6])
    assert found['links'] == 4
    targets = {int(geometry.elem_conn[row][0]) for row in np.flatnonzero(
        np.isin(geometry.elem_id, geometry.elements_in('ties')))}
    corners = set(geometry.elem_conn[int(np.flatnonzero(
        geometry.elem_id == floor[5])[0])].tolist())
    assert targets <= corners, 'only the second patch leads'


def test_ties_refuse_a_block_that_is_not_rigid():
    geometry = _floor_and_foot()
    with pytest.raises(ValueError, match='not rigid'):
        mesh.tie(geometry, geometry.elements_in('foot')[:1], 'floor',
                 group='floor')


def test_the_tied_model_solves_as_one_piece():
    geometry = _floor_and_foot()
    with pytest.raises(ValueError, match='2 disconnected pieces'):
        fem.Model.from_geometry(geometry)
    mesh.tie(geometry, geometry.elements_in('foot'), 'floor')
    assert len(fem.Model.from_geometry(geometry).pieces()) == 1


def test_the_project_records_a_tie():
    project = hertzforge.Project('p')
    project.add('Plate', _floor_and_foot())
    patch = project['Plate'].elements_in('foot')[:2]
    found = project.tie_elements('Plate', patch, 'floor')
    assert found['links'] == 6
    assert project.journal[-1] == (
        f"project.tie_elements('Plate', {patch}, 'floor')")


def test_a_tie_between_conforming_bricks_solves():
    """Two brick element groups, one resting a thousandth of an inch over the
    other on the same grid, tied over a patch: each tied node below
    leads only the nodes straight above it, so its rotation about that
    vertical line moves nothing, and solids give a rotation nothing to
    act on — a degree of freedom with neither mass nor stiffness, and
    the model would not factor (2026-10-07, the BARC in hexes). That
    rotation is grounded now (`Model.idle_lead_rotations`), and the
    tied pair solves free-free: six rigid-body modes, then elastic."""
    x, y, z = np.eye(3)
    base = mesh.block((0, 0, 0), 2 * x, 2 * y, 0.125 * z, 0.25, 'base',
                      unit='in')
    lid = mesh.block((0, 0, 0.126), 2 * x, 2 * y, 0.125 * z, 0.25, 'lid',
                     unit='in')
    whole = mesh.assemble(base, lid)
    whole.group_properties = {
        int(b): fem.GroupProperties(fem.material('6061-T6'))
        for b in whole.group_id}
    xyz = whole.node_xyz / 0.0254
    patch = []
    for element in whole.elements_in('lid'):
        row = int(np.flatnonzero(whole.elem_id == element)[0])
        center = xyz[whole.node_index(whole.elem_conn[row])].mean(axis=0)
        if abs(center[0] - 1.0) <= 0.25 and abs(center[1] - 1.0) <= 0.25:
            patch.append(element)
    mesh.tie(whole, patch, 'base')
    model = fem.Model.from_geometry(whole)
    idle = model.idle_lead_rotations()
    assert idle and {axis for _node, axis in idle} == {2}, \
        'the rotation about the vertical line, and only that one'
    frequencies = model.eigensolution(num_modes=8).frequency
    assert np.sum(np.abs(frequencies) < 1.0) == 6
    assert frequencies[6] > 1.0
