"""Coordinates are the user's to correct (Brandon, 2026-09-06).

The rows of every data object are coordinates, and the columns of a
matrix are coordinates too. A channel assigned to the wrong point at the
instrument is corrected here: double-click the label, type the
coordinate, and the object — and everything derived from it — carries the
correction. The channel moves, not the point: a force labeled at the
wrong node goes without the accelerometer at that node, which is changed
explicitly if it should be. `Project.rename_dof` is the verb; the grid
is its front.
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.data import Frf, Psd, TimeHistory


def _history():
    t = np.arange(0.0, 4.0, 0.01)
    return TimeHistory(t, np.random.default_rng(0).normal(size=(3, len(t))),
                       response_dof=['101Z+', '102Z+', '103Z+'],
                       ordinate_dim=['acceleration'] * 3)


def _frf():
    f = np.arange(0.0, 10.0)
    return Frf(f, np.ones((4, 10), dtype=complex),
               response_dof=['101Z+', '102Z+', '101Z+', '102Z+'],
               reference_dof=['101Z+', '101Z+', '105X+', '105X+'],
               ordinate_dim=['acceleration/force'] * 4)


def test_the_channel_moves_not_the_point():
    """Brandon, 2026-09-06: a force labeled at the wrong node moves
    without taking the accelerometer at that node with it. The drive
    point's accelerometer (a response) and its load cell (a reference)
    share 101Z+ and are two channels."""
    frf = _frf()
    assert frf.rename_dof('101Z+', '201Z+', 'acceleration') == 2, \
        'the two response records of the accelerometer'
    assert frf.response_dof == ['201Z+', '102Z+', '201Z+', '102Z+']
    assert frf.reference_dof == ['101Z+', '101Z+', '105X+', '105X+'], \
        'the load cell at 101Z+ is another channel and stays'
    assert frf.rename_dof('101Z+', '301Z+', 'force') == 2, \
        'the load cell, moved on its own'
    assert frf.reference_dof == ['301Z+', '301Z+', '105X+', '105X+']
    assert frf.response_dof == ['201Z+', '102Z+', '201Z+', '102Z+']
    with pytest.raises(ValueError, match="no force record at '102Z\\+'"):
        frf.rename_dof('102Z+', '1Z+', 'force')
    with pytest.raises(ValueError, match="no record at '999Z\\+'"):
        frf.rename_dof('999Z+', '1Z+')
    with pytest.raises(ValueError, match='does not start with a node id'):
        frf.rename_dof('102Z+', 'not a dof')
    assert frf.rename_dof('102Z+', '102Z+', 'acceleration') == 0, \
        'the same name changes nothing'


def test_the_point_moves_when_no_quantity_is_named():
    """The explicit way to move every channel at a coordinate: the API
    with no quantity, which the grid never sends."""
    frf = _frf()
    assert frf.rename_dof('101Z+', '201Z+') == 4, 'two responses, two references'
    assert frf.response_dof == ['201Z+', '102Z+', '201Z+', '102Z+']
    assert frf.reference_dof == ['201Z+', '201Z+', '105X+', '105X+']
    assert frf.rename_dof('105X+', '106X') == 2, \
        'an unsigned direction typed is the positive one'
    assert frf.reference_dof[2:] == ['106X+', '106X+']


def test_one_sensor_on_both_sides_of_a_cross_term_moves_together():
    f = np.arange(0.0, 10.0)
    cpsd = Psd(f, np.ones((4, 10), dtype=complex),
               response_dof=['101Z+', '101Z+', '102Z+', '102Z+'],
               reference_dof=['101Z+', '102Z+', '101Z+', '102Z+'],
               ordinate_dim=['acceleration**2/frequency'] * 4)
    assert cpsd.rename_dof('102Z+', '302Z+', 'acceleration') == 4
    assert cpsd.response_dof == ['101Z+', '101Z+', '302Z+', '302Z+']
    assert cpsd.reference_dof == ['101Z+', '302Z+', '101Z+', '302Z+'], \
        'the accelerometer is one channel whichever side of the record'


def test_two_channels_cannot_be_given_one_identity():
    """A channel is a coordinate and a quantity: a drive point's load
    cell and accelerometer share a DOF and stay two rows; two
    accelerometers at one DOF would be one row hiding a record."""
    t = np.arange(0.0, 1.0, 0.1)
    data = TimeHistory(t, np.ones((3, 10)),
                       response_dof=['101Z+', '102Z+', '101Z+'],
                       ordinate_dim=['acceleration', 'acceleration', 'force'])
    with pytest.raises(ValueError, match='already has a acceleration record'):
        data.rename_dof('102Z+', '101Z+', 'acceleration')
    assert data.response_dof == ['101Z+', '102Z+', '101Z+'], 'refused, unchanged'
    # the force may go where an accelerometer already is: two channels
    assert data.rename_dof('101Z+', '102Z+', 'force') == 1
    assert data.response_dof == ['101Z+', '102Z+', '102Z+']
    # moving the whole point is refused where any channel of it collides
    with pytest.raises(ValueError, match='already has a'):
        data.rename_dof('102Z+', '101Z+')


def test_a_channel_table_row_renames_its_node_and_direction():
    table = ChannelTable({'channel': [1, 2, 3], 'node': [101, 102, 101],
                         'direction': ['Z+', 'Z+', 'X+'],
                         'unit': ['g', 'g', 'lbf']})
    assert table.rename_dof('101Z+', '201Y', 'acceleration') == 1
    assert table.dof_strings() == ['201Y+', '102Z+', '101X+']
    assert int(table.frame['node'][0]) == 201 and table.frame['direction'][0] == 'Y+'
    with pytest.raises(ValueError, match='node'):
        table.rename_dof('102Z+', 'Z+')
    with pytest.raises(ValueError, match="no channel at '9Z\\+'"):
        table.rename_dof('9Z+', '10Z+')
    with pytest.raises(ValueError, match="no force channel at '102Z\\+'"):
        table.rename_dof('102Z+', '10Z+', 'force')
    assert table.rename_dof('102Z+', '102Z+') == 0
    # two channels at one point: the one named by its quantity moves
    both = ChannelTable({'channel': [1, 2], 'node': [101, 101],
                         'direction': ['Z+', 'Z+'], 'unit': ['g', 'lbf']})
    assert both.rename_dof('101Z+', '201Z+', 'force') == 1
    assert both.dof_strings() == ['101Z+', '201Z+']
    assert both.rename_dof('101Z+', '301Z+') == 1, 'and every channel when unnamed'


def test_the_verb_carries_the_correction_down_the_derivation_chain():
    project = hertzforge.Project()
    project.add('Run', _history())
    psds = project.compute_psds('Run')
    octave = project.compute_octave(psds)
    project.add('Other', _history())          # not derived: left alone
    changed = project.rename_dof('Run', '102Z+', '202Z+', 'acceleration')
    assert changed == ['Run', psds, octave]
    for name in changed:
        assert '202Z+' in project[name].response_dof, name
        assert '102Z+' not in project[name].response_dof, name
    assert '102Z+' in project['Other'].response_dof
    assert project.journal[-1] == \
        "project.rename_dof('Run', '102Z+', '202Z+', 'acceleration')", \
        'one line for the whole chain'
    # a derived object that never carried the coordinate is skipped, not
    # a refusal: a modal transformation has none of the source's
    geometry = hertzforge.Geometry(np.array([101, 102, 103]),
                                       np.eye(3), length_unit='m')
    project.add('Plate', geometry)
    with pytest.raises(TypeError, match='no coordinates to rename'):
        project.rename_dof('Plate', '101Z+', '1Z+')


def test_the_verb_takes_the_object_as_well_as_the_name():
    project = hertzforge.Project()
    project.add('Run', _history())
    assert project.rename_dof(project['Run'], '103Z+', '303Z+') == ['Run']
    assert project['Run'].response_dof[2] == '303Z+'


def test_a_runs_channel_table_and_data_are_linked_and_independent():
    """Brandon, 2026-09-06: imported together from one file they are
    linked; otherwise two independent objects — a coordinate corrected
    on the time history is not corrected on the table, or the reverse."""
    project = hertzforge.Project()
    added = project.import_file(fixture_path('plate', 'random.nc4'))
    assert set(added) == {'Channel Table', 'Time History', 'Specification'}
    assert set(project.object_group_of('Channel Table')) == set(added), 'one file, one group'
    project.rename_dof('Time History', '101Z+', '201Z+', 'acceleration')
    assert project['Time History'].response_dof[0] == '201Z+'
    assert project['Channel Table'].dof_strings()[0] == '101Z+', \
        'the table is its own object'
    assert project['Specification'].response_dof[0] == '101Z+'
    project.rename_dof('Channel Table', '104Z+', '204Z+', 'acceleration')
    assert project['Channel Table'].dof_strings()[1] == '204Z+'
    assert project['Time History'].response_dof[1] == '104Z+'
    # a file holding one object links nothing
    alone = hertzforge.Project()
    alone.import_file(fixture_path('plate', 'shapes.npy'))
    assert alone.object_groups == []
