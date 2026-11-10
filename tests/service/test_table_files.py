"""`tables.save` and `tables.load`: table files by path, over the socket, their
goldens, and the refusals a client sees."""

from __future__ import annotations

import asyncio
import json

import pytest
from websockets.asyncio.client import connect

from hertzforge.service import frames, table_files
from hertzforge.service.server import Service, start

from .conftest import GOLDEN

GOLDENS = {'request_tables_save': ('params', 'tablesSaveParams'),
           'response_tables_save': ('result', 'tablesSaveResult'),
           'request_tables_load': ('params', 'tablesLoadParams'),
           'response_tables_load': ('result', 'tablesLoadResult'),
           'request_tables_save_bound': ('params', 'tablesSaveParams'),
           'request_tables_load_gain': ('params', 'tablesLoadParams'),
           'response_tables_load_gain': ('result', 'tablesLoadResult')}


def _golden(name):
    return json.loads((GOLDEN / f'{name}.json').read_text(encoding='utf-8'))


async def _exchange(requests):
    """Each request's response, decoded, sent in turn on one connection."""
    service = Service()
    server = await start(service, port=0)
    port = server.sockets[0].getsockname()[1]
    out = []
    try:
        async with connect(f'ws://127.0.0.1:{port}', max_size=None) as socket:
            frames.decode(await socket.recv())             # hello
            for ident, (method, params) in enumerate(requests, 1):
                await socket.send(frames.encode_text(
                    {'type': 'request', 'id': ident, 'method': method, 'params': params}))
                out.append(frames.decode(await socket.recv()))
    finally:
        server.close()
        await server.wait_closed()
        service.close()
    return out


@pytest.mark.parametrize('name', GOLDENS)
def test_the_table_goldens_validate_and_round_trip(name, validate):
    text = (GOLDEN / f'{name}.json').read_text(encoding='utf-8')
    message = frames.decode(text)
    validate(message)
    where, schema = GOLDENS[name]
    validate(message[where], schema)
    assert frames.pack(message) == json.dumps(message, separators=(',', ':'),
                                              ensure_ascii=False)


def test_every_table_golden_is_checked_here():
    names = {p.stem for p in GOLDEN.glob('*_tables_*.json')}
    assert names == set(GOLDENS)


@pytest.mark.parametrize('suffix', ('.hforge', '.csv', '.tsv', '.xlsx'))
def test_the_golden_table_saves_and_opens_in_every_form(tmp_path, suffix, validate):
    from hertzforge.core.channel_table import ChannelTable

    params = dict(_golden('request_tables_save')['params'], path=str(tmp_path / f'ch{suffix}'))
    saved, opened = asyncio.run(_exchange([
        ('tables.save', params),
        ('tables.load', {'path': str(tmp_path / f'ch{suffix}'), 'kind': 'channels'})]))
    validate(saved)
    validate(saved['result'], 'tablesSaveResult')
    assert saved['result'] == {'path': str(tmp_path / f'ch{suffix}')}
    validate(opened)
    validate(opened['result'], 'tablesLoadResult')
    result = opened['result']
    assert (result['kind'], result['table_kind'], result['warnings']) == ('channels', None, [])
    # the whole table comes back; every value sent is as it was
    assert result['columns'] == list(ChannelTable.COLUMNS)
    at = [result['columns'].index(c) for c in params['columns']]
    assert [[row[k] for k in at] for row in result['rows']] == params['rows']
    assert 'x_unit' not in result


def test_a_breakpoint_table_opens_with_its_units_and_what_was_left_out(tmp_path, validate):
    path = tmp_path / 'profile.csv'
    path.write_text('Frequency (Hz),PSD (g²/Hz),Slope,Note\n20,,3,\n80,0.04,,flat\n'
                    '350,0.04,,\n2000,,-3,roll off\n', encoding='utf-8')
    (opened,) = asyncio.run(_exchange([('tables.load', {
        'path': str(path), 'kind': 'breakpoints', 'table_kind': 'random_profile'})]))
    validate(opened['result'], 'tablesLoadResult')
    expected = _golden('response_tables_load')['result']
    assert opened['result'] == expected


@pytest.mark.parametrize('suffix', ('.hforge', '.csv', '.tsv', '.xlsx'))
def test_the_golden_bound_saves_and_opens_in_every_form(tmp_path, suffix, validate):
    params = dict(_golden('request_tables_save_bound')['params'],
                  path=str(tmp_path / f'bound{suffix}'))
    saved, opened = asyncio.run(_exchange([
        ('tables.save', params),
        ('tables.load', {'path': params['path'], 'kind': 'breakpoints',
                         'table_kind': 'sine_limit', 'x_unit': 'Hz', 'y_unit': ''})]))
    assert saved['result'] == {'path': params['path']}
    validate(opened['result'], 'tablesLoadResult')
    assert opened['result'] == {
        'kind': 'breakpoints', 'table_kind': 'sine_limit', 'columns': params['columns'],
        'rows': params['rows'], 'warnings': [], 'x_unit': 'Hz', 'y_unit': ''}


def test_a_gain_opens_in_the_units_it_is_opened_into(tmp_path, validate):
    path = tmp_path / 'gain.csv'
    path.write_text('Frequency,Magnitude\n50,0.8\n400,4\n2000,1.5\n', encoding='utf-8')
    params = dict(_golden('request_tables_load_gain')['params'], path=str(path))
    (opened,) = asyncio.run(_exchange([('tables.load', params)]))
    validate(opened['result'], 'tablesLoadResult')
    assert opened['result'] == _golden('response_tables_load_gain')['result']
    (lbf,) = asyncio.run(_exchange([('tables.load', dict(params, y_unit='lbf/V'))]))
    assert lbf['result']['y_unit'] == 'lbf/V'


@pytest.mark.parametrize(('method', 'params', 'code', 'words'), [
    ('tables.load', {'kind': 'channels'}, 'bad_request', 'needs "path"'),
    ('tables.load', {'path': 'x.csv', 'kind': 'shapes'}, 'bad_request', '"kind" is one of'),
    ('tables.load', {'path': 'x.csv', 'kind': 'channels', 'sheet': 1}, 'bad_request',
     "it has no 'sheet'"),
    ('tables.load', {'path': '/nowhere/x.csv', 'kind': 'channels'}, 'not_found', 'no file at'),
    ('tables.load', {'path': 'x.json', 'kind': 'channels'}, 'refused', 'a table file is'),
    ('tables.save', {'kind': 'channels', 'path': 'x.csv', 'columns': ['channel'], 'rows': [1]},
     'bad_request', '"rows" is a list of rows'),
    ('tables.save', {'kind': 'channels', 'path': 'x.csv', 'columns': ['channel', 'serial'],
                     'rows': [[1, 'A']]}, 'bad_request', "'serial' is not a column"),
    ('tables.save', {'kind': 'channels', 'path': 'x.csv', 'columns': ['channel', 'enabled'],
                     'rows': [[1, 'on']]}, 'bad_request', "row 1, Enabled: 'on' is not true"),
    ('tables.save', {'kind': 'breakpoints', 'path': 'x.csv', 'columns': ['freq_hz'],
                     'rows': []}, 'bad_request', 'needs "table_kind"'),
    ('tables.save', {'kind': 'channels', 'path': 'x.csv', 'columns': ['channel'], 'rows': [],
                     'x_unit': 'Hz'}, 'bad_request', 'no x_unit or y_unit'),
    ('tables.load', {'kind': 'channels', 'path': 'x.csv', 'y_unit': 'g'}, 'bad_request',
     'no x_unit or y_unit'),
    ('tables.load', {'kind': 'breakpoints', 'path': 'x.csv', 'y_unit': 2}, 'bad_request',
     '"y_unit" is text'),
])
def test_a_request_that_cannot_be_answered_says_why(tmp_path, monkeypatch, method, params,
                                                    code, words, validate):
    monkeypatch.chdir(tmp_path)
    (answer,) = asyncio.run(_exchange([(method, params)]))
    validate(answer)
    assert answer['error']['code'] == code, answer
    assert words in answer['error']['message']
    assert not list(tmp_path.iterdir())


def test_a_file_of_the_other_kind_is_refused(tmp_path):
    saved = dict(_golden('request_tables_save')['params'], path=str(tmp_path / 'ch.csv'))
    _, wrong = asyncio.run(_exchange([
        ('tables.save', saved),
        ('tables.load', {'path': str(tmp_path / 'ch.csv'), 'kind': 'breakpoints',
                         'table_kind': 'random_profile'})]))
    assert wrong['error'] == {'code': 'refused', 'message': 'ch.csv holds a channel table, '
                                                            'not a breakpoint table'}


def test_a_channel_table_file_opens_as_a_project_and_a_breakpoint_file_is_refused(tmp_path):
    saved = dict(_golden('request_tables_save')['params'], path=str(tmp_path / 'ch.hforge'))
    profile = {'kind': 'breakpoints', 'table_kind': 'random_profile',
               'path': str(tmp_path / 'profile.hforge'), 'columns': ['freq_hz', 'psd_g2_hz'],
               'rows': [[20, 0.01], [2000, 0.01]]}
    _, _, opened, refused_open, refused_import, project = asyncio.run(_exchange([
        ('tables.save', saved),
        ('tables.save', profile),
        ('project.open', {'path': str(tmp_path / 'ch.hforge')}),
        ('project.open', {'path': str(tmp_path / 'profile.hforge')}),
        ('project.import', {'path': str(tmp_path / 'profile.hforge')}),
        ('project.save', {'path': str(tmp_path / 'project.hforge')})]))
    # one table: the test's channel table is the analysis's channel table
    assert [o['kind'] for o in opened['result']['project']['objects']] == ['channel_table']
    for answer in (refused_open, refused_import):
        assert answer['error']['code'] == 'refused'
        assert answer['error']['message'] == (
            'profile.hforge holds a breakpoint table. A table file is opened from the table '
            'it belongs to, not opened or imported as a project.')
    # the refusals left the project the open made
    assert [o['kind'] for o in project['result']['project']['objects']] == ['channel_table']


def test_the_methods_are_the_ones_the_server_answers():
    assert table_files.TABLE_METHODS == ('tables.save', 'tables.load')


def test_a_channel_table_converts_to_carry_controller_settings_and_back(validate):
    from hertzforge.core import ChannelTable
    from hertzforge.service import projects
    from hertzforge.service.ops_sheets import OPERATIONS

    ops = {op.id: op for op in OPERATIONS}
    state = projects.new({})
    project = state.project
    project.add('Channels', ChannelTable({'channel': [1, 2], 'node': ['1', '2'],
                                          'direction': ['Z+', 'Z+'], 'unit': ['g', 'g']}))

    def run(op_id):
        op = ops[op_id]
        values = op.resolve({'table': 'Channels'},
                            objects={n: projects.kind_of(o) for n, o in project.items()})
        return op.run(project, values)

    table, reading = run('project.add_controller_settings')
    validate(table, 'tableView')
    assert table['columns'][-11] == 'Enabled' and reading['title'].endswith('added')
    assert project['Channels'].has_controller_settings
    # the converted columns edit as the others do
    assert 'enabled' in table['edit']['columns']
    assert table['edit']['kinds']['enabled'] == 'flag'
    _table, again = run('project.add_controller_settings')
    assert again['rows'][0]['label'] == 'Unchanged'
    table, _reading = run('project.remove_controller_settings')
    assert 'Enabled' not in table['columns']
    assert project.journal[-2:] == ["project['Channels'].add_controller_settings()",
                                    "project['Channels'].remove_controller_settings()"]
