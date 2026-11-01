"""Sensors drawn on the geometry: each body centered on its node, turned by
the channel table alone, and moving with its node in an animation.

Brandon, 2026-10-26: a sensor whose make and model are listed is drawn
in the geometry window, aligned with the node's local coordinates, with
nothing added to the model. Brandon, 2026-10-27: the node is the body's
center, so the lines through the centers of a triax's X, Y and Z faces
cross at the node and a uniaxial's axis runs through it; the mounting
surface is never read. Brandon, 2026-10-27, later: no colored faces; a
triax's X, Y and Z faces carry their letters, as the sensors do, and a
uniaxial's or a force transducer's sensing axis is an arrow on its body,
so a sign wrong in the table still shows; the bodies follow PCB's
outline drawings, the 352A24 a teardrop.
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core import sensors as placing
from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.sensor_catalog import GENERIC, lookup, models_for

C, S = np.cos(np.radians(30.0)), np.sin(np.radians(30.0))

HERE = np.array([0.3, -0.2, 0.5])


def _geometry(units=True):
    """Node 101 at HERE measured in a frame turned 30 degrees about Z,
    node 102 in the global frame, node 103 in a cylindrical one."""
    turned = [[C, S, 0.0], [-S, C, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]]
    identity = [[1.0, 0, 0], [0, 1.0, 0], [0, 0, 1.0], [0, 0, 0]]
    geometry = hertzforge.Geometry(
        node_id=[101, 102, 103],
        node_xyz=[HERE, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        node_disp_cs=[2, 1, 3], cs_id=[1, 2, 3], cs_name=['', '', ''],
        cs_type=[0, 0, 1], cs_matrix=[identity, turned, identity])
    if units:
        geometry.define_units('m')
    return geometry


def _table(rows):
    """A table from (node, direction, type, model, triax DOF) rows."""
    columns = list(zip(*rows))
    count = len(rows)
    return ChannelTable({
        'channel': list(range(1, count + 1)), 'node': list(columns[0]),
        'direction': list(columns[1]), 'channel_type': list(columns[2]),
        'unit': ['g' if t == 'acceleration' else 'N' if t == 'force' else 'V'
                 for t in columns[2]],
        'model': list(columns[3]), 'triax_dof': list(columns[4])})


def _triax(node=101, model='356A02', directions=('X+', 'Y+', 'Z+'),
           legs=('X', 'Y', 'Z')):
    return [(node, d, 'acceleration', model, leg)
            for d, leg in zip(directions, legs)]


def test_two_legs_give_the_third_and_one_leg_stands_on_the_nodes_axes():
    geometry = _geometry()
    two = _triax(directions=('Y+', 'Z+'), legs=('Y', 'Z'))
    (sensor,), _notes = placing.place(_table(two), geometry)
    assert np.allclose(sensor.frame[:, 0], geometry.dof_direction('101X+'))
    for wired, legs, third in ((('X+', 'Z+'), ('X', 'Z'), 1),
                               (('Y+', 'X+'), ('Y', 'X'), 2)):
        (sensor,), _notes = placing.place(
            _table(_triax(directions=wired, legs=legs)), geometry)
        letter = 'XYZ'[third]
        assert np.allclose(sensor.frame[:, third],
                           geometry.dof_direction(f'101{letter}+')), \
            f'the {letter} leg from the other two'
    one = _triax(node=102, directions=('Z-',), legs=('Z',))
    (sensor,), _notes = placing.place(_table(one), geometry)
    assert np.allclose(sensor.frame[:, 2], [0, 0, -1])
    assert np.allclose(sensor.frame[:, 0], [1, 0, 0])
    assert np.linalg.det(sensor.frame) == pytest.approx(1.0)


def test_a_blank_triax_dof_reads_the_rows_own_letter():
    geometry = _geometry()
    rows = _triax(node=102, model='356A01', directions=('Y-', 'X+', 'Z-'),
                  legs=('', '', ''))
    (sensor,), notes = placing.place(_table(rows), geometry)
    assert notes == []
    assert np.allclose(sensor.frame, [[1, 0, 0], [0, -1, 0], [0, 0, -1]])


def test_a_left_handed_or_skewed_triax_is_drawn_generic_and_named():
    geometry = _geometry()
    flipped = _triax(node=102, directions=('X+', 'Y+', 'Z-'))
    (sensor,), notes = placing.place(_table(flipped), geometry)
    assert sensor.generic and not sensor.marked
    assert np.allclose(sensor.frame, np.eye(3)), "on the node's own axes"
    assert notes == [('node 102: the triax legs make a left-handed frame, so '
                     "one sign in the table is wrong; drawn on the node's "
                     'own axes')]
    skewed = _triax(node=102, directions=('X+', 'X+', 'Z+'))
    (sensor,), notes = placing.place(_table(skewed), geometry)
    assert sensor.generic
    assert notes == [("node 102: the triax legs are not perpendicular; drawn "
                     "on the node's own axes")]


def test_what_counts_as_one_sensor():
    geometry = _geometry()
    rows = (_triax(node=102, model='356a02')
            + [(102, 'Z+', 'acceleration', '352A24', ''),
               (102, 'X+', 'acceleration', '352A24', ''),
               (101, 'Z+', 'acceleration', 'ACME-9', 'Z'),
               (101, 'X+', 'acceleration', 'ACME-9', 'X'),
               (101, 'Z+', 'acceleration', 'BETA-2', ''),
               (101, 'Y+', 'acceleration', '', ''),
               (102, 'X+', 'force', '', ''),
               (102, 'Z+', 'voltage', 'V-1', '')])
    found, notes = placing.place(_table(rows), geometry)
    kinds = [(s.node, s.kind, s.rows) for s in found]
    assert kinds == [(102, 'triax', (0, 1, 2)), (102, 'uniaxial', (3,)),
                     (102, 'uniaxial', (4,)), (101, 'triax', (5, 6)),
                     (101, 'uniaxial', (7,))], \
        'a case-blind model; two uniaxials apart; a triax of an unknown ' \
        'model by its Triax DOF; nothing for a row with no model, nor for ' \
        'a voltage'
    assert found[3].entry is GENERIC['triax'] and found[3].generic
    assert found[4].entry is GENERIC['uniaxial']
    assert notes == [('not in the catalog, drawn as a generic triaxial '
                     'accelerometer: ACME-9'),
                     ('not in the catalog, drawn as a generic uniaxial '
                     'accelerometer: BETA-2')]


def test_rows_that_cannot_be_placed_are_counted():
    geometry = _geometry()
    rows = [('', 'Z+', 'acceleration', '352A24', ''),
            (999, 'Z+', 'acceleration', '352A24', ''),
            (101, '', 'force', '208C01', ''),
            (103, 'Z+', 'acceleration', '352A24', ''),
            (101, 'Z+', 'force', '352A24', '')]
    found, notes = placing.place(_table(rows), geometry)
    assert [s.entry for s in found] == [GENERIC['force']], \
        'an accelerometer model on a force row draws a generic force gauge'
    assert notes == [('352A24 is a uniaxial accelerometer, on a force row at '
                     'node 101: drawn as a generic force transducer'),
                     '1 sensor row not drawn: no node',
                     '1 sensor row not drawn: node not in the geometry',
                     '1 sensor row not drawn: no direction',
                     ('1 sensor row not drawn: node in a cylindrical or '
                     'spherical system')]


def test_the_catalog():
    assert lookup(' 356 a02 ') is lookup('356A02')
    assert lookup('nothing') is None
    assert {m.model for m in models_for('force')} == {
        f'208C0{k}' for k in range(1, 6)}
    assert 'force' not in {m.kind for m in models_for('acceleration')}
    assert models_for('voltage') == []
    assert lookup('352A24').shape == 'teardrop'
    assert lookup('352A73').shape == 'rounded'
    assert lookup('208C03').stud == (4.8, 3.5)


def test_the_project_places_a_linked_tables_sensors():
    project = hertzforge.Project()
    project.add('Plate', _geometry())
    project.add('Channels', _table(_triax()))
    project.link('Plate', 'Channels')
    (sensor,), notes = project.sensors('Channels')
    assert sensor.node == 101 and notes == []
    with pytest.raises(TypeError, match='not a channel table'):
        project.sensors('Plate')


def _colors(mesh):
    return {tuple(int(v) for v in rgb) for rgb in mesh.mesh.cell_data['colors']}


def _rgb(color):
    return tuple(int(color.lstrip('#')[i:i + 2], 16) for i in (0, 2, 4))


def test_a_catalog_model_fills_a_blank_make():
    table = _table([(101, 'Z+', 'acceleration', '', ''),
                    (102, 'Z+', 'acceleration', '', ''),
                    (103, 'Z+', 'acceleration', '', '')])
    table.set_cell('model', 0, '356A02')
    assert table['make'][0] == 'PCB'
    table.set_cell('make', 1, 'Lab stock')
    table.set_cell('model', 1, '352A24')
    assert table['make'][1] == 'Lab stock', 'a make already written is kept'
    table.set_cell('model', 2, 'ACME-9')
    assert table['make'][2] == '', 'nothing to fill from'


def test_a_triaxs_identity_is_written_once_for_its_legs():
    """The other acceleration rows at the node get the triax's make,
    model and serial number where theirs are blank; a force gauge there
    is never filled, nor a leg already holding another sensor's model."""
    table = _table([(101, 'X+', 'acceleration', '', 'X'),
                    (101, 'Y+', 'acceleration', '', 'Y'),
                    (101, 'Z+', 'acceleration', '', 'Z'),
                    (101, 'Z+', 'force', '', ''),
                    (101, 'Z-', 'acceleration', '352A24', ''),
                    (102, 'Z+', 'acceleration', '', '')])
    table.set_cell('model', 1, '356A02')
    assert list(table['model']) == ['356A02', '356A02', '356A02', '',
                                    '352A24', '']
    assert list(table['make']) == ['PCB', 'PCB', 'PCB', '', '', '']
    table.set_cell('serial_number', 2, 'LW1234')
    assert list(table['serial_number']) == ['LW1234'] * 3 + ['', '', '']
    table.set_cell('serial_number', 0, 'LW9999')
    assert list(table['serial_number'][:3]) == ['LW9999', 'LW1234',
                                                'LW1234'], \
        'a serial already written is kept'
    table.set_cell('model', 3, '208C02')
    assert table['model'][0] == '356A02', 'a force model fills no leg'


def test_one_or_two_legs_wired_fill_only_what_is_there():
    table = _table([(101, 'X+', 'acceleration', '', 'X'),
                    (101, 'Z+', 'acceleration', '', 'Z')])
    table.set_cell('model', 0, '356A43')
    assert list(table['model']) == ['356A43', '356A43']
    assert table.num_channels == 2, 'no leg is added'
    uniaxial = _table([(101, 'X+', 'acceleration', '', ''),
                       (101, 'Y+', 'acceleration', '', '')])
    uniaxial.set_cell('model', 0, '352A24')
    assert list(uniaxial['model']) == ['352A24', ''], \
        'only a triax is spread'
