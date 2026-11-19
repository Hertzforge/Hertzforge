"""A geometry's node, coordinate system and slide forms are typed in the display system, and a
click or a drag in the view sends what it reads there: the service takes each to meters once."""

from __future__ import annotations

import asyncio

import numpy as np
import pytest

from hertzforge.core.geometry import Geometry
from hertzforge.service import display_units
from hertzforge.service.server import OPERATIONS

from .test_projects import _exchange

INCH = 0.0254
OPS = {op.id: op for op in OPERATIONS}


def answers(unit, *steps):
    start = [('project.new', {}),
             ('units.system', {'name': display_units.START}),
             ('op.run', {'op': 'project.new_geometry', 'params': {'name': 'Part', 'unit': unit}})]
    return [got[-1] for got in asyncio.run(_exchange([*start, *steps]))][len(start):]


def run(op, **params):
    return ('op.run', {'op': f'project.{op}', 'params': {'geometry': 'Part', **params}})


def stage(answer):
    return next(view for view in answer['result']['views'] if view['view'] == 'shape3d')


def last_node(answer):
    view = stage(answer)
    return np.asarray(view['nodes'], dtype=float).reshape(-1, 3)[len(view['pick']['ids']) - 1]


def test_a_node_typed_at_two_inches_is_two_inches_from_the_origin():
    (added,) = answers('m', run('add_node', x=2.0, y=1.0, z=0.5))
    view = stage(added)
    assert view['unit'] == 'in'
    assert last_node(added) == pytest.approx([2.0, 1.0, 0.5])
    # a click sends the place as the view shows it, which the form's fields take as typed
    assert view['pick']['op'] == 'project.add_node' and 'to_si' not in view['pick']


def test_a_slide_typed_as_one_inch_moves_the_system_one_inch():
    (_node, slid, again) = answers(
        'm', run('add_node', x=2.0),
        run('slide_coordinate_system', cs=1, axis='x', distance=1.0),
        run('slide_coordinate_system', cs=1, axis='x', distance=0.5))
    gizmo = stage(slid)['gizmo']
    assert gizmo['origin'] == pytest.approx([1.0, 0.0, 0.0])
    assert gizmo['slide']['in_display'] and 'to_si' not in gizmo, 'a drag is sent as it is read'
    assert stage(again)['gizmo']['origin'] == pytest.approx([1.5, 0.0, 0.0])
    said = ' '.join(str(view.get('note', '')) + str(view.get('title', ''))
                    for view in slid['result']['views'])
    assert '+1 in along its X' in said


def test_a_geometry_without_units_takes_the_numbers_as_typed():
    geometry = Geometry(node_id=[], node_xyz=np.empty((0, 3)))
    assert not geometry.units_defined
    for op_id, typed in (('project.add_node', {'x': 2.0}),
                         ('project.slide_coordinate_system', {'distance': 1.0}),
                         ('project.merge_coincident_nodes', {'tolerance': 0.5})):
        op = OPS[op_id]
        given = op.resolve({'geometry': 'Raw', **typed}, objects={'Raw': 'geometry'})
        assert display_units.typed_lengths(op.params, given, display_units.START, geometry) == given


def test_a_merge_tolerance_typed_in_inches_reaches_the_merge_in_meters():
    op = OPS['project.merge_coincident_nodes']
    defined = Geometry(node_id=[], node_xyz=np.empty((0, 3)), length_unit='m')
    given = op.resolve({'geometry': 'P', 'tolerance': 0.5}, objects={'P': 'geometry'})
    out = display_units.typed_lengths(op.params, given, display_units.START, defined)
    assert out['tolerance'] == pytest.approx(0.5 * INCH)
