"""`drive.inspect`, `drive.convert` and `drive.fix_ends`: Playback's drive files
by path, over the socket, their goldens, and the refusals a client sees."""

from __future__ import annotations

import asyncio
import importlib.util
import json

import numpy as np
import pytest
from websockets.asyncio.client import connect

from hertzforge.service import frames
from hertzforge.service.server import Service, start

from .conftest import GOLDEN, ROOT

GOLDENS = {'request_drive_inspect': ('params', 'driveInspectParams'),
           'response_drive_inspect': ('result', 'driveInspectResult'),
           'request_drive_convert': ('params', 'driveConvertParams'),
           'response_drive_convert': ('result', 'driveConvertResult'),
           'request_drive_fix_ends': ('params', 'driveFixEndsParams'),
           'response_drive_fix_ends': ('result', 'driveFixEndsResult'),
           'request_drive_convert_unequal': ('params', 'driveConvertParams'),
           'response_error_drive_refused': (None, None)}


@pytest.fixture(autouse=True)
def _data_root(tmp_path, monkeypatch):
    monkeypatch.setenv('HERTZFORGE_DATA_ROOT', str(tmp_path / 'data'))


def _golden(name):
    return json.loads((GOLDEN / f'{name}.json').read_text(encoding='utf-8'))


def _source_csv(tmp_path):
    spec = importlib.util.spec_from_file_location('make_golden_drive',
                                                  ROOT / 'schema' / 'make_golden_drive.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    path = tmp_path / 'drive.csv'
    # bytes, not text: text mode on Windows writes CRLF, and the golden pins the file's SHA-256
    path.write_bytes(module.source_text().encode('utf-8'))
    return path


async def _exchange(requests, project=None):
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
def test_the_drive_goldens_validate(name, validate):
    message = frames.decode((GOLDEN / f'{name}.json').read_text(encoding='utf-8'))
    validate(message)
    where, schema = GOLDENS[name]
    if where is not None:
        validate(message[where], schema)


def test_every_drive_golden_is_checked_here():
    names = {p.stem for p in GOLDEN.glob('*_drive_*.json')}
    assert names == set(GOLDENS)


def test_the_golden_file_inspects_converts_and_fixes_as_the_goldens_say(tmp_path, validate):
    path = str(_source_csv(tmp_path))
    inspected, converted = asyncio.run(_exchange([
        ('drive.inspect', {'path': path}),
        ('drive.convert', dict(_golden('request_drive_convert')['params'], path=path))]))
    validate(inspected['result'], 'driveInspectResult')
    expected = _golden('response_drive_inspect')['result']
    expected['source']['path'] = path
    assert inspected['result'] == expected
    validate(converted['result'], 'driveConvertResult')
    drive = converted['result']['drive']
    golden = json.loads((GOLDEN / 'drive' / 'basic' / 'drive.json').read_text(encoding='utf-8'))
    assert drive['source']['path'] == path
    for key in ('tool', 'created_utc'):
        assert isinstance(drive.pop(key), str)
        golden.pop(key)
    drive['source']['path'] = golden['source']['path']
    # the golden folder is this very conversion, byte for byte
    assert drive == golden
    folder = converted['result']['folder']
    assert (tmp_path / 'data' / 'drives' / 'drive').samefile(folder)
    (fixed,) = asyncio.run(_exchange([('drive.fix_ends', {'folder': folder})]))
    validate(fixed['result'], 'driveFixEndsResult')
    want = _golden('response_drive_fix_ends')['result']
    got = fixed['result']
    assert (got['taper_s'], got['taper_samples']) == (want['taper_s'], want['taper_samples'])
    assert got['from']['sha256'] == want['from']['sha256']
    for ours, theirs in zip(got['channels'], want['channels'], strict=True):
        assert ours['name'] == theirs['name']
        for key in ('a', 'b', 'rms_before', 'rms_after'):
            assert ours[key] == pytest.approx(theirs[key], rel=1e-4, abs=1e-9)
        for side in ('before', 'after'):
            for key, value in theirs[side].items():
                assert ours[side][key] == pytest.approx(value, rel=1e-4, abs=1e-9)


def test_the_refusal_golden_is_what_the_service_says(tmp_path, validate):
    values = np.linspace(-0.5, 0.5, 401)
    np.savez(tmp_path / 'drives.npz', a=values, b=values[:397])
    params = dict(_golden('request_drive_convert_unequal')['params'],
                  path=str(tmp_path / 'drives.npz'))
    (answer,) = asyncio.run(_exchange([('drive.convert', params)]))
    validate(answer)
    assert answer['error'] == _golden('response_error_drive_refused')['error']


def test_a_projects_time_histories_seed_a_drive(tmp_path, validate):
    from hertzforge.core.data import TimeHistory
    from hertzforge.project import Project

    history = TimeHistory(np.arange(300) / 500.0, np.vstack([np.sin(np.arange(300) / 7.0)]),
                          response_dof=['1X+'])
    history.define_units(['V'])
    project = Project('Seed')
    project.add('Drive', history)
    project_file = tmp_path / 'seed.hforge'
    project.save(project_file)

    async def run():
        service = Service()
        server = await start(service, port=0)
        port = server.sockets[0].getsockname()[1]
        try:
            async with connect(f'ws://127.0.0.1:{port}', max_size=None) as socket:
                frames.decode(await socket.recv())
                answers = []
                for ident, (method, params) in enumerate([
                        ('project.open', {'path': str(project_file)}),
                        ('drive.inspect', {'objects': ['Drive']}),
                        ('drive.convert', {'objects': ['Drive'],
                                           'selections': ['object:Drive:record:1']}),
                        ('drive.inspect', {'objects': ['Nope']})], 1):
                    await socket.send(frames.encode_text(
                        {'type': 'request', 'id': ident, 'method': method, 'params': params}))
                    while True:
                        message = frames.decode(await socket.recv())
                        if message.get('type') == 'response':
                            answers.append(message)
                            break
                return answers
        finally:
            server.close()
            await server.wait_closed()
            service.close()

    _opened, inspected, converted, missing = asyncio.run(run())
    validate(inspected['result'], 'driveInspectResult')
    assert [s['key'] for s in inspected['result']['signals']] == ['object:Drive:record:1']
    validate(converted['result'], 'driveConvertResult')
    source = converted['result']['drive']['source']
    assert (source['format'], source['path']) == ('project', str(project_file))
    assert converted['result']['drive']['rate_hz'] == 500.0
    assert missing['error']['code'] == 'not_found'


@pytest.mark.parametrize(('method', 'params', 'code', 'words'), [
    ('drive.inspect', {}, 'bad_request', 'needs "path"'),
    ('drive.inspect', {'path': 'a.csv', 'objects': ['x']}, 'bad_request', 'not both'),
    ('drive.inspect', {'path': 'a.csv', 'sheet': 1}, 'bad_request', "it has no 'sheet'"),
    ('drive.inspect', {'path': '/nowhere/a.csv'}, 'not_found', 'no file at'),
    ('drive.inspect', {'objects': ['x']}, 'not_found', "the project has no object 'x'"),
    ('drive.convert', {'path': 'a.csv'}, 'bad_request', 'needs "selections"'),
    ('drive.convert', {'path': 'a.csv', 'selections': ['column:2'], 'rate_hz': 'fast'},
     'bad_request', '"rate_hz" is a number'),
    ('drive.convert', {'path': 'a.csv', 'selections': ['column:2'], 'fix_ends': 1},
     'bad_request', '"fix_ends" is true or false'),
    ('drive.fix_ends', {}, 'bad_request', 'needs "folder"'),
    ('drive.fix_ends', {'folder': '/nowhere/drive'}, 'refused', 'no drive.json'),
])
def test_the_refusals_a_client_sees(method, params, code, words):
    (answer,) = asyncio.run(_exchange([(method, params)]))
    assert answer['error']['code'] == code
    assert words in answer['error']['message']
