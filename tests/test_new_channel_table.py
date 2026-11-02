"""An empty channel table, filled in by hand.

A geometry could be started empty and built in the app; a channel table
could only be imported (Brandon, 2026-10-22: the user should be able to
create an empty channel table just as they can an empty geometry). New
Channel Table on the project's bar makes one, and Add Channel on the
table's bar gives it rows to type into.
"""

from __future__ import annotations

import pytest

import hertzforge
from hertzforge import io
from hertzforge.core.channel_table import ChannelTable


def _empty():
    return ChannelTable({name: [] for name in ChannelTable.CORE})


def test_channels_are_numbered_on_from_the_highest():
    table = _empty()
    assert table.add_channels() == [1], 'an empty table starts at 1'
    assert table.add_channels(2) == [2, 3]
    table = ChannelTable({'channel': [3, 7], 'node': [101, 102],
                          'direction': ['X+', 'Y+'], 'unit': ['g', 'g']})
    assert table.add_channels() == [8], 'after the highest, not the count'
    assert table.dof_strings()[:2] == ['101X+', '102Y+'], 'the rows kept'


def test_a_new_channel_is_blank_but_its_number():
    table = _empty()
    table.add_channels()
    for name, spec in ChannelTable.COLUMNS.items():
        if name == 'channel':
            continue
        assert str(table[name][0]) == spec.default, name
    assert table['daq_range'][0] == '' and table['sensor_range'][0] == '', \
        'unstated until someone says'


def test_no_channels_cannot_be_added():
    with pytest.raises(ValueError, match='at least one'):
        _empty().add_channels(0)


def test_an_empty_table_has_a_filled_tables_column_types():
    """The type column was a bare list, which pandas read as floats when
    there were no channels in it, and a typed value would not go in."""
    empty, filled = _empty(), ChannelTable(
        {'channel': [1], 'node': [1], 'direction': ['X+'], 'unit': ['g']})
    assert dict(empty.frame.dtypes) == dict(filled.frame.dtypes)
    empty.add_channels()
    empty.set_cell('channel_type', 0, 'acceleration')
    assert empty.types() == ['acceleration']


def test_the_project_makes_one_and_adds_to_it():
    project = hertzforge.Project('p')
    name = project.new_channel_table()
    assert name == 'Channel Table' and project[name].num_channels == 0
    assert 'add_channels' in dict(project.verbs(name))
    assert project.add_channels(name, 2) == [1, 2]
    assert project[name].num_channels == 2
    with pytest.raises(TypeError, match='not a channel table'):
        project.add_channels(project.new_geometry())


@pytest.mark.parametrize('suffix, fmt', [('xlsx', 'excel'), ('h5', 'escdf')])
def test_an_empty_table_reads_back(tmp_path, suffix, fmt):
    """What can be written can be read: Excel used to refuse a header
    with nothing under it."""
    path = tmp_path / f'empty.{suffix}'
    io.export_file(_empty(), str(path), format=fmt)
    back = io.import_file(str(path))
    back = back if isinstance(back, ChannelTable) else next(
        obj for obj in back.values() if isinstance(obj, ChannelTable))
    assert back.num_channels == 0


def test_an_empty_table_saves_and_opens(tmp_path):
    project = hertzforge.Project('p')
    project.new_channel_table()
    project.save(tmp_path / 'p.vdyn')
    opened = hertzforge.Project.open(tmp_path / 'p.vdyn')
    assert opened['Channel Table'] == project['Channel Table']
    assert opened['Channel Table'].num_channels == 0


def test_a_channel_not_yet_placed_is_named_by_its_number():
    """A blank channel listed as '', and the message ended at its colon."""
    from hertzforge.compatibility import check_object

    geometry = hertzforge.Geometry(node_id=[1, 2],
                                       node_xyz=[[0, 0, 0], [1, 0, 0]])
    table = ChannelTable({'channel': [1, 2, 3, 4, 5],
                          'node': ['', 2, '', 9, 9],
                          'direction': ['', '', 'X+', 'X+', ''],
                          'unit': [''] * 5})
    issue = check_object('Channels', table, geometry, 'Plate')
    # channel 2 is at node 2, on the geometry: no direction yet is not
    # off the geometry
    assert issue.message == (
        "4 of 5 channels are not in geometry 'Plate': "
        'channel 1 (no node or direction yet), channel 3 (no node yet), '
        '9X+, channel 5 at node 9 (no direction yet)')
    assert issue.missing_dofs == ['9X+'], 'only what is a DOF'
    assert issue.sub_items == [0, 2, 3, 4]
