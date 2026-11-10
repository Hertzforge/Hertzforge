"""The service over a real socket: hello, its methods, the errors, and an
event loop that stays free while the work runs."""

from __future__ import annotations

import asyncio
import contextlib
import json
import re
import subprocess
import sys
import time
from pathlib import Path

import pytest
from websockets.asyncio.client import connect
from websockets.exceptions import ConnectionClosed

from hertzforge.service import __main__ as entry
from hertzforge.service import frames
from hertzforge.service.server import OPERATIONS, SWITCH_INTERVAL, Service, preload, start

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'


class Client:
    def __init__(self, socket):
        self.socket = socket
        self.ident = 0

    async def call(self, method, params=None, *, raw=False):
        self.ident += 1
        await self.socket.send(frames.encode_text(
            {'type': 'request', 'id': self.ident, 'method': method,
             'params': {} if params is None else params}))
        while True:
            frame = await self.socket.recv()
            message = frames.decode(frame)
            # a long import reports progress events before its response
            if message['type'] != 'event':
                break
        assert message['id'] == self.ident
        return (message, frame) if raw else message


def wire(message):
    return frames.to_wire(message)[0]


#: packages whose first import, if it happened while serving, would hold the
#: interpreter lock against the event loop
HEAVY = ('hertzforge', 'netCDF4', 'h5py', 'pandas', 'scipy', 'pint')


def _heavy(modules):
    # a None entry is a module refused, not loaded
    return {name for name, module in modules.items()
            if module is not None and name.split('.')[0] in HEAVY}


async def _session(check, imported=None):
    # the process prepared as the entry point prepares it (__main__._serve);
    # the switch interval is put back after
    preload()
    interval = sys.getswitchinterval()
    service = Service()
    server = await start(service, '127.0.0.1', 0)
    port = server.sockets[0].getsockname()[1]
    before = _heavy(dict(sys.modules))
    lag = {'max': 0.0, 'run': True}

    async def ticker():
        last = time.perf_counter()
        while lag['run']:
            await asyncio.sleep(0.005)
            now = time.perf_counter()
            lag['max'] = max(lag['max'], now - last - 0.005)
            last = now

    tick = asyncio.create_task(ticker())
    sys.setswitchinterval(SWITCH_INTERVAL)
    try:
        async with connect(f'ws://127.0.0.1:{port}/', max_size=None,
                           compression=None) as socket:
            await check(Client(socket), socket)
    finally:
        sys.setswitchinterval(interval)
        lag['run'] = False
        await tick
        server.close()
        await server.wait_closed()
        service.close()
    if imported is not None:
        imported.update(_heavy(dict(sys.modules)) - before)
    return lag['max']


# A timing test: CI runs it alone, after the rest (`-m timing`, without xdist), because any
# worker beside it can starve its event loop on a small runner. Beside the fresh-interpreter
# preload test below the loop was held 136 ms on ubuntu-latest with Python 3.12, against a
# 25-41 ms worst case run alone; the xdist group that kept those two apart still left 175 ms
# on windows-latest with Python 3.13 beside the other workers. The group stays for a local
# run of the whole suite under `--dist loadgroup`.
@pytest.mark.timing
@pytest.mark.xdist_group('serving')
def test_smoke(validate):
    seen = {}

    async def check(client, socket):
        hello = frames.decode(await socket.recv())
        validate(hello)
        assert hello == {'type': 'hello', 'protocol': 'sds/1',
                         'service': 'analysis', 'version': hello['version']}

        catalog, frame = await client.call('catalog.list', raw=True)
        assert isinstance(frame, str)
        validate(catalog)
        validate(catalog['result'], 'catalog')

        opened = await client.call('project.import', {'path': str(PLATE / 'random.nc4')})
        validate(opened)
        names = [o['name'] for o in opened['result']['project']['objects']]
        assert 'Time History' in names
        for op in ('project.plot', 'project.waterfall'):
            response, frame = await client.call('op.run', {
                'op': op, 'params': {'data': 'Time History'}}, raw=True)
            assert isinstance(frame, bytes)
            validate(wire(response))
            validate(wire(response['result'])['views'], 'views')
            seen[op] = response['result']['views']

        await client.call('project.new', {'title': 'Survey'})
        await client.call('project.import', {'path': str(PLATE / 'modal_spectra.nc4')})
        await client.call('project.import', {'path': str(PLATE / 'test_geometry.npz')})
        response, frame = await client.call('op.run', {
            'op': 'project.fit_modes',
            'params': {'frf': 'FRF', 'low_hz': 300, 'high_hz': 1300, 'count': 4}}, raw=True)
        validate(wire(response))
        shape = next(v for v in response['result']['views'] if v['view'] == 'shape3d')
        assert 1 <= len(shape['modes']) <= 4
        assert shape['nodes'].dtype.name == 'float32'
        seen['project.fit_modes'] = shape

    imported = set()
    lag = asyncio.run(_session(check, imported))
    assert seen['project.plot'][0]['view'] == 'spectrum'
    assert seen['project.waterfall'][0]['view'] == 'waterfall'
    # Nothing heavy was first imported while serving: that is what stalls the
    # loop on a cold machine (server.PRELOAD), however fast this one is.
    assert not imported, f'imported while serving: {sorted(imported)}'
    # heavy work ran off the loop: the ticker was never held up for long
    print(f'longest event-loop stall {lag * 1000:.1f} ms')
    assert lag < 0.1, f'event loop held for {lag:.3f} s'


#: Run in a fresh interpreter (argv: HEAVY and the plate folder, as JSON):
#: preload as the entry point does, then import the plate's runs into a
#: project, show every object and run the operations a study reaches
#: through the Service, framing each result as serving does, and print the
#: heavy modules imported for the first time after the preload.
_FIRST_IMPORTS = '''
import asyncio, json, sys
from hertzforge.service import frames, projects, server

heavy_packages, plate = (json.loads(arg) for arg in sys.argv[1:])

def heavy():
    return {name for name, module in sys.modules.items()
            if module is not None and name.split('.')[0] in heavy_packages}

RUNS = (('project.plot', {'data': 'Time History'}),
        ('project.waterfall', {'data': 'Time History'}),
        ('project.compute_psds', {'history': 'Time History'}),
        ('project.fit_modes', {'frf': 'FRF', 'low_hz': 300, 'high_hz': 1300, 'count': 4}),
        ('project.mac', {}), ('project.mode_shapes', {}))

async def serve():
    service = server.Service()
    call = service.in_worker
    try:
        state = None
        for name in ('random.nc4', 'modal_spectra.nc4', 'test_geometry.npz'):
            state, _added = await call(projects.import_file, state,
                                       {'path': f'{plate}/{name}'}, None)
        project = state.project
        for name in list(project):
            shown = await call(projects.show, state, {'objects': [name]})
            await call(frames.pack, {'type': 'response', 'id': 1, 'result': shown})
        for op_id, given in RUNS:
            op = service.operations[op_id]
            values = op.resolve(
                given, objects={n: projects.kind_of(o) for n, o in project.items()},
                verbs={n: projects.verbs_of(project, n) for n in project})
            views = await call(op.run, project, values)
            await call(frames.pack, {'type': 'response', 'id': 1, 'result': {'views': views}})
    finally:
        service.close()

server.preload()
before = heavy()
asyncio.run(serve())
print(json.dumps(sorted(heavy() - before)))
'''


@pytest.mark.xdist_group('serving')
def test_the_preload_leaves_nothing_heavy_to_import_while_serving():
    """`server.PRELOAD` covers reading the controller's files, showing what
    they hold and the operations a study reaches. In a fresh interpreter:
    in this one, other tests import modules first and would hide a module
    the list misses."""
    done = subprocess.run(
        [sys.executable, '-c', _FIRST_IMPORTS, json.dumps(HEAVY), json.dumps(str(PLATE))],
        check=False, capture_output=True, text=True, timeout=300)
    assert done.returncode == 0, done.stderr[-2000:]
    imported = json.loads(done.stdout.splitlines()[-1])
    assert not imported, f'imported while serving: {imported}'


def test_errors_name_the_cause(validate):
    async def check(client, socket):
        await socket.recv()                                     # hello
        cases = [
            (('nope', {}), 'unknown_method', 'no method'),
            (('catalog.list', {'x': 1}), 'bad_request', "no 'x'"),
            (('op.run', {'op': 'random.nope'}), 'not_found',
             "No operation named 'random.nope'; catalog.list names the ones that exist."),
            (('op.run', {'op': 'project.waterfall', 'handle': 'h1'}), 'bad_request',
             "no 'handle'"),
            (('op.run', {'op': 'project.waterfall'}), 'refused', 'the project has none'),
        ]
        for (method, params), code, words in cases:
            message = await client.call(method, params)
            validate(message)
            assert message['error']['code'] == code, message
            assert words in message['error']['message']

        await client.call('project.import', {'path': str(PLATE / 'modal_spectra.nc4')})
        message = await client.call('op.run', {'op': 'project.mac'})
        assert message['error']['code'] == 'refused'
        assert 'needs a shape set' in message['error']['message']
        message = await client.call('op.run', {
            'op': 'project.fit_modes', 'params': {'count': 99}})
        assert message['error']['code'] == 'bad_request'
        assert 'the most it takes is 60' in message['error']['message']

        # not a request: the connection is closed with the reason
        await socket.send(frames.encode_text({'type': 'hello'}))
        with pytest.raises(ConnectionClosed):
            await socket.recv()
        assert socket.close_code == 1008
        assert 'integer "id"' in socket.close_reason

    asyncio.run(_session(check))


def test_a_project_belongs_to_its_connection():
    async def check(client, socket):
        await socket.recv()
        opened = await client.call('project.import', {'path': str(PLATE / 'random.nc4')})
        assert 'Time History' in [o['name'] for o in opened['result']['project']['objects']]

    service = Service()

    async def both():
        server = await start(service, '127.0.0.1', 0)
        port = server.sockets[0].getsockname()[1]
        url = f'ws://127.0.0.1:{port}/'
        try:
            async with connect(url, max_size=None) as first:
                await check(Client(first), first)
            async with connect(url, max_size=None) as second:
                await second.recv()
                message = await Client(second).call('op.run', {
                    'op': 'project.plot', 'params': {'data': 'Time History'}})
                # the second connection has its own project, empty
                assert message['error']['code'] == 'bad_request'
                assert "'Time History', which is not one of the choices" in message['error']['message']
        finally:
            server.close()
            await server.wait_closed()
            service.close()

    asyncio.run(both())


def test_start_changes_nothing_beyond_the_server(monkeypatch):
    # a plain constructor: no imports, no interpreter-wide setting, so a
    # caller (these tests among them) keeps its own
    calls = []
    monkeypatch.setattr('hertzforge.service.server.preload', lambda: calls.append('preload'))
    interval = sys.getswitchinterval()
    service = Service()

    async def listen_and_close():
        listening = await start(service, '127.0.0.1', 0)
        listening.close()
        await listening.wait_closed()

    try:
        asyncio.run(listen_and_close())
        assert calls == [], 'start() ran the preload'
        assert sys.getswitchinterval() == interval, 'start() set the switch interval'
    finally:
        service.close()
        sys.setswitchinterval(interval)


def test_the_entry_point_preloads_and_sets_the_switch_interval(monkeypatch, capsys):
    calls = []
    monkeypatch.setattr('hertzforge.service.server.preload', lambda: calls.append('preload'))
    interval = sys.getswitchinterval()

    async def serve_until_listening():
        serving = asyncio.create_task(entry._serve('127.0.0.1', 0))
        said = ''
        while 'listening on' not in said:
            await asyncio.sleep(0.01)
            said += capsys.readouterr().err
            if serving.done():
                serving.result()                # raises what stopped it
        observed = sys.getswitchinterval()
        serving.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await serving
        return observed

    try:
        observed = asyncio.run(asyncio.wait_for(serve_until_listening(), 60))
    finally:
        sys.setswitchinterval(interval)
    assert calls == ['preload']
    assert observed == SWITCH_INTERVAL


def test_console_entry_point_starts_headless_and_answers():
    process = subprocess.Popen(
        [sys.executable, '-m', 'hertzforge.service', '--port', '0'],
        stderr=subprocess.PIPE, text=True)
    try:
        deadline = time.monotonic() + 60
        url = None
        while time.monotonic() < deadline:
            line = process.stderr.readline()
            if not line:
                break
            found = re.search(r'listening on (ws://\S+)', line)
            if found:
                url = found.group(1)
                break
        assert url, 'the service never said where it listens'

        async def ask():
            async with connect(url) as socket:
                hello = json.loads(await socket.recv())
                await socket.send(json.dumps({'type': 'request', 'id': 1,
                                              'method': 'catalog.list',
                                              'params': {}}))
                return hello, json.loads(await socket.recv())

        hello, catalog = asyncio.run(ask())
        assert hello['protocol'] == 'sds/1'
        assert len(catalog['result']['operations']) == len(OPERATIONS)
    finally:
        process.terminate()
        process.wait(timeout=20)


def test_with_watch_stdin_the_end_of_standard_input_ends_the_service():
    """The desktop app stops the service by closing the pipe on its standard
    input, and the pipe closes by itself if the app dies: the service must
    end either way, cleanly, and not outlive the app."""
    process = subprocess.Popen(
        [sys.executable, '-m', 'hertzforge', 'serve', '--port', '0', '--watch-stdin'],
        stdin=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        deadline = time.monotonic() + 60
        said = []
        while time.monotonic() < deadline:
            line = process.stderr.readline()
            if not line:
                break
            said.append(line)
            if 'listening on ws://' in line:
                break
        assert 'listening on ws://' in ''.join(said), said
        time.sleep(0.5)
        assert process.poll() is None, 'the service ended with its input still open'
        process.stdin.close()
        assert process.wait(timeout=30) == 0
        assert 'standard input closed' in process.stderr.read()
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=20)
