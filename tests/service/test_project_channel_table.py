"""The project is the channel table's home (PROTOCOL.md, "The Channel
Table's Home"): `project.put_channel_table` saves the control service's rows
as a project table and `project.channel_table` answers them back, through
the one mapping. The rows are the golden table the control service's own
test reads (`request_tables_save_channels.json`, every optional member on
every row), so a table goes project to service to project losing nothing:
the service's leg is `crates/sds-control/tests/table_restored.rs`
(`project_table_rows_survive_restore_apply_and_a_restart`) on the same
rows."""

from __future__ import annotations

import copy
import json

import pytest

from hertzforge.service import projects
from hertzforge.service.protocol import ProtocolError

from .conftest import GOLDEN


def _golden():
    params = json.loads((GOLDEN / 'request_tables_save_channels.json').read_text('utf-8'))
    return params['params']['channels'], params['params']['environments']


def test_project_channel_table_round_trip_through_a_saved_project_loses_nothing(
        tmp_path, validate):
    rows, environments = _golden()
    state = projects.new({'title': 'Bench'})
    name = projects.put_channel_table(state, {'name': 'Bench table', 'channels': rows,
                                              'environments': environments})
    validate({'name': name, 'channels': rows}, 'projectPutChannelTableParams')
    path = projects.save(state, {'path': str(tmp_path / 'bench.hforge')})
    opened = projects.open_file({'path': path})
    got = projects.channel_table(opened, {})
    validate(got, 'projectChannelTableResult')
    assert got['name'] == 'Bench table'
    assert got['channels'] == rows
    assert got['environments'] == environments
    assert got['warnings'] == []
    # Sent back to the project as the service would return it, it replaces the
    # table and keeps its control loops when none are given.
    projects.put_channel_table(opened, {'name': 'Bench table', 'channels': got['channels'],
                                        'replace': True})
    again = projects.channel_table(opened, {'name': 'Bench table'})
    assert again['channels'] == rows
    assert again['environments'] == environments


def test_project_channel_table_refusals_say_what_and_why():
    rows, _ = _golden()
    with pytest.raises(ProtocolError, match='none is open'):
        projects.channel_table(None, {})
    state = projects.new({'title': 'Bench'})
    with pytest.raises(ProtocolError, match='no channel table'):
        projects.channel_table(state, {})
    projects.put_channel_table(state, {'name': 'A', 'channels': rows})
    with pytest.raises(ProtocolError, match='already holds'):
        projects.put_channel_table(state, {'name': 'A', 'channels': rows})
    projects.put_channel_table(state, {'name': 'B', 'channels': rows})
    with pytest.raises(ProtocolError, match='holds 2 channel tables'):
        projects.channel_table(state, {})
    broken = copy.deepcopy(rows)
    broken[1]['index'] = broken[0]['index']
    with pytest.raises(ProtocolError, match='twice'):
        projects.put_channel_table(state, {'name': 'C', 'channels': broken})
    broken = copy.deepcopy(rows)
    broken[0]['sensitivity_mv_per_eu'] = 'high'
    with pytest.raises(ProtocolError, match='high'):
        projects.put_channel_table(state, {'name': 'C', 'channels': broken})
    assert 'C' not in state.project
