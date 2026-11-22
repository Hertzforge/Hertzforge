"""Files a browser sends by their contents: `project.upload` in pieces, then `project.import` or
`project.open` naming the upload in place of a path (`uploads`). The project indexes the file by
its name, size and SHA-256, with no location, and holds nothing by reference to the copy."""

from __future__ import annotations

import asyncio
import os
from pathlib import Path

import numpy as np
import pytest
from websockets.asyncio.client import connect

from hertzforge.core.data_index import digest
from hertzforge.service import frames, projects
from hertzforge.service.server import Service, start
from hertzforge.service.uploads import CHUNK_LIMIT, Uploads

from .test_server import Client

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
PIECE = 3 * 2**20


async def _send(client, path, piece=PIECE, name=None):
    """`path` sent as a browser sends a dropped file: its name, its size and its bytes in
    pieces. Answers the upload's id."""
    data = np.frombuffer(Path(path).read_bytes(), dtype=np.uint8)
    ident = None
    for offset in range(0, max(data.size, 1), piece):
        params = {'name': name or os.path.basename(path), 'size': int(data.size),
                  'offset': offset, 'data': {'$array': 'd'}}
        if ident is not None:
            params['upload'] = ident
        message = await _binary(client, 'project.upload', params,
                                {'d': data[offset:offset + piece]})
        assert 'result' in message, message
        ident = message['result']['upload']
        assert message['result']['received'] == min(offset + piece, data.size)
    return ident


async def _binary(client, method, params, arrays):
    client.ident += 1
    await client.socket.send(frames.encode(
        {'type': 'request', 'id': client.ident, 'method': method, 'params': params}, arrays))
    while True:
        message = frames.decode(await client.socket.recv())
        if message['type'] != 'event':
            return message


def _serve(check):
    service = Service()

    async def run():
        server = await start(service, '127.0.0.1', 0)
        port = server.sockets[0].getsockname()[1]
        try:
            async with connect(f'ws://127.0.0.1:{port}/', max_size=None) as socket:
                await socket.recv()
                await check(Client(socket))
        finally:
            server.close()
            await server.wait_closed()
            service.close()

    asyncio.run(run())


@pytest.mark.xdist_group('serving')
def test_a_file_sent_by_its_contents_is_imported_and_indexed_with_no_location():
    async def check(client):
        ident = await _send(client, PLATE / 'random.nc4')
        message = await client.call('project.import', {'upload': ident})
        assert 'result' in message, message
        names = [o['name'] for o in message['result']['project']['objects']]
        assert 'Time History' in names
        journal = (await client.call('project.journal', {}))['result']['lines']
        # the line a script replays beside the file, not the copy's path
        assert "project.import_file('random.nc4')" in journal

    _serve(check)


def test_the_index_names_a_received_file_with_no_location(tmp_path):
    import hertzforge as hz

    copy = tmp_path / 'random.nc4'
    copy.write_bytes((PLATE / 'random.nc4').read_bytes())
    project = hz.Project()
    project.import_file(copy, received=True)
    (entry,) = project.data_index.entries
    assert (entry.name, entry.path) == ('random.nc4', '')
    assert (entry.size, entry.sha256) == digest(PLATE / 'random.nc4')
    # found again by its name where the original is kept
    (checked,) = project.data_index.verify(search=[PLATE])
    assert checked.status == 'matches'
    # the same file received twice is one entry
    project.import_file(copy, received=True)
    assert len(project.data_index.entries) == 1


def test_nothing_is_held_by_reference_to_a_received_project(tmp_path, monkeypatch):
    import hertzforge as hz
    from hertzforge.io import native

    source = hz.Project('Run')
    source.import_file(PLATE / 'random.nc4')
    saved = tmp_path / 'run.hforge'
    source.save(saved)
    # every object counts as large, as a long run's would
    monkeypatch.setattr(native, 'REFERENCE_FROM_BYTES', 1)
    held = hz.Project()
    held.add('Other', source['Channel Table'])
    held.import_file(saved)
    assert held.references, 'a file imported by its path is held by reference'
    received = hz.Project()
    received.add('Other', source['Channel Table'])
    received.import_file(saved, received=True)
    assert not received.references


@pytest.mark.xdist_group('serving')
def test_a_project_sent_by_its_contents_opens_with_no_file_of_its_own(tmp_path):
    import hertzforge as hz

    source = hz.Project('Sent')
    source.import_file(PLATE / 'test_geometry.npz')
    saved = tmp_path / 'sent.hforge'
    source.save(saved)

    async def check(client):
        ident = await _send(client, saved)
        message = await client.call('project.open', {'upload': ident})
        assert 'result' in message, message
        project = message['result']['project']
        assert project['title'] == 'Sent' and project['file'] is None
        saving = await client.call('project.save', {})
        assert 'has no file of its own' in saving['error']['message']

    _serve(check)


@pytest.mark.xdist_group('serving')
def test_what_cannot_be_read_one_way_is_refused():
    async def check(client):
        piece = {'d': np.zeros(4, dtype=np.uint8)}
        start = {'name': 'a.unv', 'size': 8, 'offset': 0, 'data': {'$array': 'd'}}
        first = (await _binary(client, 'project.upload', start, piece))['result']['upload']
        # the next piece comes where the last ended
        wrong = await _binary(client, 'project.upload',
                              {'upload': first, 'offset': 2, 'data': {'$array': 'd'}}, piece)
        assert '4 bytes have come, so the next piece starts there' in wrong['error']['message']
        # not all of it has come
        early = await client.call('project.import', {'upload': first})
        assert early['error']['code'] == 'refused'
        assert '4 of 8 bytes have come' in early['error']['message']
        past = await _binary(client, 'project.upload',
                             {'upload': first, 'offset': 4, 'data': {'$array': 'd'}},
                             {'d': np.zeros(5, dtype=np.uint8)})
        assert 'runs past its end' in past['error']['message']
        for params, said in (
                ({**start, 'name': '../a.unv'}, 'a file\'s name, not a path'),
                ({**start, 'offset': 4}, 'the first piece starts at offset 0'),
                ({**start, 'upload': 'nope'}, 'no upload \'nope\' is under way')):
            refused = await _binary(client, 'project.upload', params, piece)
            assert said in refused['error']['message'], refused
        both = await client.call('project.import', {'upload': first, 'path': '/x.unv'})
        assert 'by its "path" or by its "upload", not both' in both['error']['message']
        unknown = await client.call('project.open', {'upload': 'nope'})
        assert 'no upload \'nope\' was received' in unknown['error']['message']

    _serve(check)


def test_the_files_go_with_the_connection():
    uploads = Uploads()
    data = np.arange(10, dtype=np.uint8)
    got = uploads.receive({'name': 'a.unv', 'size': 10, 'offset': 0, 'data': data})
    path = uploads.taken(got['upload']).path
    assert Path(path).read_bytes() == data.tobytes() and Path(path).name == 'a.unv'
    uploads.close()
    assert not Path(path).exists() and not uploads.held
    with pytest.raises(Exception, match='pieces of at most'):
        uploads.receive({'name': 'b.unv', 'size': CHUNK_LIMIT + 1, 'offset': 0,
                         'data': np.zeros(CHUNK_LIMIT + 1, dtype=np.uint8)})


def test_a_received_run_manifest_is_refused(tmp_path):
    from hertzforge.io.run_folder import SCHEMA_NAME

    # a run's manifest alone: the run's streams are files of the folder it names
    (tmp_path / 'run.json').write_text(f'{{"schema": "{SCHEMA_NAME}/1"}}')
    with pytest.raises(Exception, match='opened by its path'):
        projects.open_file({'path': str(tmp_path / 'run.json')}, received=True)


def test_folders_a_stopped_service_left_are_swept(tmp_path):
    """A service stopped before its connections closed leaves its received files; the next one
    to receive a file removes them, and never a running service's."""
    import subprocess
    import sys

    from hertzforge.service.uploads import PREFIX, sweep

    ended = subprocess.run([sys.executable, '-c', 'import os; print(os.getpid())'],
                           capture_output=True, text=True, check=True)
    dead = int(ended.stdout)
    left = tmp_path / f'{PREFIX}{dead}-abc'
    (left / 'x').mkdir(parents=True)
    (left / 'x' / 'run.nc4').write_bytes(b'0' * 16)
    mine = tmp_path / f'{PREFIX}{os.getpid()}-def'
    mine.mkdir()
    other = tmp_path / 'unrelated-123'
    other.mkdir()
    assert sweep(str(tmp_path)) == [left.name]
    assert not left.exists() and mine.exists() and other.exists()
