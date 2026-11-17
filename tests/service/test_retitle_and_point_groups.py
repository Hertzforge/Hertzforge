"""The project's own name is typed over (`project.retitle`), and the points of the element
groups picked in the tree are drawn over the rest in their own color."""

from __future__ import annotations

import asyncio

import numpy as np
import pytest

from hertzforge.core.geometry import Geometry
from hertzforge.service import projects, scans
from hertzforge.service.ops_geometry import part_entities
from hertzforge.service.protocol import ProtocolError

from .test_projects import _exchange


def test_the_project_is_renamed_and_the_name_is_saved(tmp_path):
    path = str(tmp_path / 'kept.hforge')
    new, named, same, _saved, opened = (got[-1] for got in asyncio.run(_exchange([
        ('project.new', {'title': 'First'}),
        ('project.retitle', {'title': '  Wing Test  '}),
        ('project.retitle', {'title': 'Wing Test'}),
        ('project.save', {'path': path}),
        ('project.open', {'path': path})])))
    assert new['result']['project']['title'] == 'First'
    assert named['result']['project']['title'] == 'Wing Test'
    assert named['result']['project']['modified'] is True
    assert same['result']['project']['title'] == 'Wing Test'
    assert opened['result']['project']['title'] == 'Wing Test'


def test_a_project_is_not_given_an_empty_name():
    held = projects.new({'title': 'First'})
    with pytest.raises(ProtocolError, match='not empty'):
        projects.retitle(held, {'title': '   '})
    assert held.project.name == 'First' and not held.modified


def _points():
    """Six nodes in two groups of point elements, and a beam on two more."""
    xyz = np.array([[float(k), 0.0, 0.0] for k in range(8)])
    geometry = Geometry(node_id=list(range(1, 9)), node_xyz=xyz, length_unit='m')
    first = geometry.add_group('Face A')
    second = geometry.add_group('Face B')
    beams = geometry.add_group('Frame')
    for node in (1, 2, 3):
        geometry.add_element([node], elem_type=161, group=first)
    for node in (4, 5, 6):
        geometry.add_element([node], elem_type=161, group=second)
    geometry.add_element([7, 8], elem_type=21, group=beams)
    return geometry, first, second, beams


def test_the_points_of_the_groups_picked_are_drawn_over_the_rest():
    geometry, first, second, beams = _points()
    (plain,) = part_entities('G', geometry, {'kind': 'part', 'id': 'nodes'})
    (one,) = part_entities('G', geometry, {'kind': 'group', 'id': first})
    assert len(one['points']) == len(plain['points']) + 1
    marked = one['points'][-1]
    assert np.asarray(marked['xyz'])[:, 0] == pytest.approx([0.0, 1.0, 2.0])
    assert {tuple(c) for c in np.asarray(marked['colors'])} == {scans.PICKED_COLOR}
    assert marked['size'] > one['points'][0]['size'], 'larger than the points under them'
    (both,) = part_entities('G', geometry, {'kind': 'group', 'id': first, 'ids': [first, second]})
    assert len(both['points'][-1]['xyz']) == 6
    # a group of beams has no points to mark
    (frame,) = part_entities('G', geometry, {'kind': 'group', 'id': beams})
    assert len(frame['points']) == len(plain['points'])


def test_an_element_group_is_renamed_from_the_tree():
    held = projects.new({'title': 'Points'})
    geometry, first, _second, _beams = _points()
    held.project.add('Probed', geometry)
    held.modified = False
    projects.rename(held, {'name': 'Probed', 'to': '  Top Face ', 'group': first})
    assert geometry.group_name[0] == 'Top Face' and held.modified
    assert held.project.journal[-1].endswith(f".rename_group({first}, 'Top Face')")
    kids = projects.children(held, {'name': 'Probed'})
    labels = [group['label'] for family in kids['families'] for group in family['groups']]
    assert 'Top Face' in labels and 'Face A' not in labels
    # the same name again changes nothing
    held.modified = False
    projects.rename(held, {'name': 'Probed', 'to': 'Top Face', 'group': first})
    assert not held.modified
    with pytest.raises(ProtocolError, match='no element group 99'):
        projects.rename(held, {'name': 'Probed', 'to': 'x', 'group': 99})
    with pytest.raises(ProtocolError, match='element group\'s id'):
        projects.rename(held, {'name': 'Probed', 'to': 'x', 'group': 'Face A'})
    assert held.project.names == ['Probed']
