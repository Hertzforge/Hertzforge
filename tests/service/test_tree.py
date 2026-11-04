"""What the project tree opens an object into (`project.children`), as
Visual Dynamics' tree opens it: a geometry's parts, families and element
groups; a data object's grid of records with their quantity icons; a
shape set's modes; a channel table's channels."""

from __future__ import annotations

import asyncio
from pathlib import Path

import numpy as np
import pytest

from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.data import Frf, TimeHistory
from hertzforge.core.shapes import ShapeSet
from hertzforge.project import Project
from hertzforge.service import frames, projects, tree
from hertzforge.service.protocol import ProtocolError

from .test_server import _session

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'


@pytest.fixture
def plate():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    projects.import_file(state, {'path': str(PLATE / 'modal_spectra.nc4')})
    projects.import_file(state, {'path': str(PLATE / 'geometry.unv'),
                                 'units': {'length_unit': 'mm'}})
    return state


def named(state, kind):
    return next(name for name, obj in state.project.items() if projects.kind_of(obj) == kind)


def test_a_geometry_opens_into_its_parts_and_families(plate, validate):
    geometry = plate.project[named(plate, 'geometry')]
    held = tree.children(plate.project, named(plate, 'geometry'))
    validate(held, 'children')
    assert held['kind'] == 'geometry'
    parts = {p['id']: p['count'] for p in held['parts']}
    assert parts == {'nodes': len(geometry.node_id),
                     'coordinate_systems': len(geometry.cs_id)}
    families = {f['id']: f for f in held['families']}
    # the six always, and points too: a ground starts from one
    assert set(families) >= {'beams', 'triangles', 'quads', 'tetras', 'wedges', 'hexes', 'points'}
    assert sum(f['count'] for f in held['families']) == len(geometry.elem_id)
    plates = families['quads']
    assert plates['count'] == len(geometry.elem_id) and plates['groups']
    assert sum(g['count'] for g in plates['groups']) == plates['count']
    assert all(g['label'] for g in plates['groups'])
    assert families['beams']['groups'] == [] and families['beams']['count'] == 0
    assert held['solids'] == [] and held['mates'] == []


def test_an_frf_opens_into_a_grid_of_response_over_reference(plate, validate):
    name = named(plate, 'frf')
    frf = plate.project[name]
    held = tree.children(plate.project, name)
    validate(held, 'children')
    assert held['kind'] == 'grid'
    assert len(held['rows']) * len(held['columns']) == frf.num_records, 'a full matrix'
    assert held['columns'] == list(dict.fromkeys(frf.reference_dof))
    assert [r['dof'] for r in held['rows']] == list(dict.fromkeys(frf.response_dof))
    assert sorted(i for row in held['cells'] for i in row) == list(range(frf.num_records))
    assert set(held['icons']) == {'acceleration/force'}, 'each cell wears its fraction'
    assert held['undefined'] == [] and 'role' not in held['rows'][0]


def test_time_data_opens_with_a_role_per_row(plate, validate):
    name = named(plate, 'time_history')
    history = plate.project[name]
    held = tree.children(plate.project, name)
    validate(held, 'children')
    assert held['columns'] == [''] and len(held['rows']) == history.num_records
    roles = {r['dof']: r['role'] for r in held['rows']}
    quantities = {r['dof']: r['quantity'] for r in held['rows']}
    assert all(role in ('reference', 'response', 'monitor') for role in roles.values())
    forces = [dof for dof, q in quantities.items() if q == 'force']
    assert forces and all(roles[dof] == 'reference' for dof in forces)
    assert all(icon in ('acceleration', 'force') for icon in held['icons'])


def test_two_channels_at_one_point_are_two_rows_and_unknowns_are_told_apart(validate):
    history = TimeHistory(
        abscissa=np.linspace(0, 1, 8), ordinate=np.zeros((3, 8)),
        response_dof=['101Z+', '101Z+', '101Z+'], ordinate_unit=['m/s**2', 'N', None],
        ordinate_dim=['acceleration', 'force', 'unknown'])
    held = tree.grid(history)
    validate(held, 'children')
    assert [r['label'] for r in held['rows']] == ['101Z+', '101Z+', '101Z+']
    assert [r['quantity'] for r in held['rows']] == ['acceleration', 'force', 'unknown']
    assert held['icons'] == ['acceleration', 'force', ''] and held['undefined'] == [2]
    plain = TimeHistory(
        abscissa=np.linspace(0, 1, 8), ordinate=np.zeros((2, 8)),
        response_dof=['101Z+', '101Z+'], ordinate_unit=[None, None])
    labels = [r['label'] for r in tree.grid(plain)['rows']]
    assert labels == ['101Z+', '101Z+ #2'], 'the channel order breaks the tie, and says so'


def test_a_sparse_matrix_keeps_its_holes():
    frf = Frf(abscissa=np.linspace(1, 100, 5), ordinate=np.ones((3, 5), dtype=complex),
              response_dof=['1Z+', '2Z+', '1Z+'], reference_dof=['1Z+', '1Z+', '2Z+'],
              ordinate_unit=['m/s**2/N'] * 3)
    held = tree.grid(frf)
    assert held['columns'] == ['1Z+', '2Z+']
    assert held['cells'] == [[0, 2], [1, -1]]


def test_a_shape_set_opens_into_its_modes_and_a_table_into_its_channels(validate):
    project = Project()
    shapes = ShapeSet(frequency=[10.0, 25.5], damping=[0.01, 0.02],
                      shape_matrix=np.ones((2, 3)), coordinate=['1Z+', '2Z+', '3Z+'],
                      description=['bending', ''])
    project.add('Modes', shapes)
    held = tree.children(project, 'Modes')
    validate(held, 'children')
    assert held == {'kind': 'modes', 'modes': [
        {'index': 1, 'frequency': 10.0, 'damping': 0.01, 'description': 'bending'},
        {'index': 2, 'frequency': 25.5, 'damping': 0.02, 'description': ''}]}
    table = ChannelTable({'channel': [1, 2], 'node': [1, 2], 'direction': ['Z+', 'X-'],
                          'unit': ['g', 'lbf'], 'channel_type': ['acceleration', 'force']})
    project.add('Channels', table)
    held = tree.children(project, 'Channels')
    validate(held, 'children')
    assert held == {'kind': 'channels', 'channels': [
        {'dof': '1Z+', 'quantity': 'acceleration'}, {'dof': '2X-', 'quantity': 'force'}]}


def test_the_summary_says_what_each_object_holds(plate, validate):
    summary = projects.summary(plate)
    validate(summary, 'project')
    by_kind = {o['kind']: o for o in summary['objects']}
    assert 'nodes' in by_kind['geometry']['holds']
    assert 'banded' not in by_kind['frf']


def test_the_server_answers_children_and_refuses_an_unknown_name():
    async def check(client, socket):
        await socket.recv()        # hello
        # an import reports its progress as events before it answers
        await socket.send(frames.encode_text({'type': 'request', 'id': 99, 'method': 'project.import',
                                              'params': {'path': str(PLATE / 'modal_spectra.nc4')}}))
        while True:
            imported = frames.decode(await socket.recv())
            if imported.get('id') == 99:
                break
        frf = next(o['name'] for o in imported['result']['project']['objects']
                   if o['kind'] == 'frf')
        held = await client.call('project.children', {'name': frf})
        assert held['result']['kind'] == 'grid'
        missing = await client.call('project.children', {'name': 'nope'})
        assert missing['error']['code'] == 'not_found'

    asyncio.run(_session(check))


def test_children_needs_an_open_project():
    with pytest.raises(ProtocolError, match='none is open'):
        projects.children(None, {'name': 'x'})


def test_sine_objects_open_into_their_control_dofs(validate):
    """The rows under a sine specification or a level set are its control
    DOFs, as VD lays them (the tones are the bar's drop-down)."""
    from hertzforge.core.sine import SineSweepSpecification, SineTone

    project = Project()
    tone = SineTone('Up', 0.5, [100.0, 500.0], [[2.0, 1.0]] * 2, [0], [100.0])
    project.add('Profile', SineSweepSpecification([tone], ['101Z+', '102Z+'],
                                                  ordinate_unit='m/s**2'))
    held = tree.children(project, 'Profile')
    validate(held, 'children')
    assert held == {'kind': 'channels', 'channels': [
        {'dof': '101Z+', 'quantity': 'acceleration'}, {'dof': '102Z+', 'quantity': 'acceleration'}]}
