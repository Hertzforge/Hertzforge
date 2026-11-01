"""The service over a real socket: hello, the three methods, the errors,
and an event loop that stays free while the work runs."""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
import re
import subprocess
import sys
import time

import pytest
from websockets.asyncio.client import connect
from websockets.exceptions import ConnectionClosed

from sds_analysis import __main__ as entry
from sds_analysis import frames
from sds_analysis.server import SWITCH_INTERVAL, Service, preload, start


class Client:
    def __init__(self, socket):
        self.socket = socket
        self.ident = 0

    async def call(self, method, params=None, *, raw=False):
        self.ident += 1
        await self.socket.send(frames.encode_text(
            {'type': 'request', 'id': self.ident, 'method': method,
             'params': {} if params is None else params}))
        frame = await self.socket.recv()
        message = frames.decode(frame)
        assert message['id'] == self.ident
        return (message, frame) if raw else message


def wire(message):
    return frames.to_wire(message)[0]


#: packages whose first import, if it happened while serving, would hold the
#: interpreter lock against the event loop
HEAVY = ('visualdynamics', 'vtkmodules', 'pyvista', 'PySide6', 'pyqtgraph',
         'netCDF4', 'h5py', 'pandas', 'scipy', 'pint')


def _heavy(modules):
    # a None entry is a module refused, not loaded (Visual Dynamics blocks
    # the Qt modules that are not LGPL that way)
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


# The timing test and the fresh-interpreter preload test share one xdist group (CI runs
# `--dist loadgroup`), so they never run at the same time: the second one starts a whole
# interpreter and runs every dataset and operation, and on a four-core runner beside
# three other workers that starves the first one's event loop. It held the loop 136 ms
# on ubuntu-latest with Python 3.12, against a 25-41 ms worst case run alone.
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

        opened = await client.call('dataset.open', {'id': 'plate-random'})
        validate(opened)
        result = opened['result']
        assert result['kind'] == 'random_run'
        assert len(result['channels']) == 12
        channel = result['channels'][1]['value']
        for op in ('random.psd_vs_spec', 'random.error_db'):
            response, frame = await client.call('op.run', {
                'op': op, 'handle': result['handle'],
                'params': {'channel': channel}}, raw=True)
            assert isinstance(frame, bytes)
            validate(wire(response))
            validate(wire(response['result'])['views'], 'views')
            seen[op] = response['result']['views']

        model = (await client.call('dataset.open', {'id': 'drone-fem'}))['result']
        assert model['kind'] == 'modal_model' and model['channels'] == []
        response, frame = await client.call('op.run', {
            'op': 'modal.modes', 'handle': model['handle'],
            'params': {'count': 4}}, raw=True)
        validate(wire(response))
        (view,) = response['result']['views']
        assert len(view['modes']) == 4
        assert view['nodes'].dtype.name == 'float32'
        seen['modal.modes'] = view

    imported = set()
    lag = asyncio.run(_session(check, imported))
    assert seen['random.psd_vs_spec'][0]['series'][0]['name'].startswith('104Z+')
    # Nothing heavy was first imported while serving: that is what stalls the
    # loop on a cold machine (server.PRELOAD), however fast this one is.
    assert not imported, f'imported while serving: {sorted(imported)}'
    # heavy work ran off the loop: the ticker was never held up for long
    print(f'longest event-loop stall {lag * 1000:.1f} ms')
    assert lag < 0.1, f'event loop held for {lag:.3f} s'


#: Run in a fresh interpreter (argv: HEAVY and the dataset ids, as JSON):
#: preload as the entry point does, then open each dataset and run every
#: operation it accepts through the Service, framing each result as serving
#: does, and print the heavy modules imported for the first time after the
#: preload.
_FIRST_IMPORTS = '''
import asyncio, json, sys
from sds_analysis import frames, server

heavy_packages, datasets = (json.loads(arg) for arg in sys.argv[1:])

def heavy():
    return {name for name, module in sys.modules.items()
            if module is not None and name.split('.')[0] in heavy_packages}

async def serve():
    service = server.Service()
    handles = {}
    try:
        for dataset in datasets:
            opened = await service.dataset_open({'id': dataset}, handles)
            kind = handles[opened['handle']].entry.kind
            for op in service.operations.values():
                if op.accepts != kind:
                    continue
                params = ({'channel': opened['channels'][1]['value']}
                          if opened['channels'] else {})
                result = await service.op_run({'op': op.id, 'handle': opened['handle'],
                                               'params': params}, handles)
                await service.in_worker(frames.pack, {'type': 'response', 'id': 1,
                                                      'result': result})
    finally:
        service.close()

server.preload()
before = heavy()
asyncio.run(serve())
print(json.dumps(sorted(heavy() - before)))
'''


@pytest.mark.xdist_group('serving')
def test_the_preload_leaves_nothing_heavy_to_import_while_serving(tmp_path):
    """`server.PRELOAD` covers every dataset open and operation, from an empty
    cache and from a full one. In a fresh interpreter: in this one, other
    tests import modules first and would hide a module the list misses.
    The frame-wing model is left out, as `PRELOAD` leaves out its first
    build (about 30 s)."""
    datasets = ['plate-random', 'drone-fem', 'plate-fem']
    env = {**os.environ, 'SDS_CACHE_DIR': str(tmp_path / 'cache')}
    for cache in ('empty', 'full'):
        done = subprocess.run(
            [sys.executable, '-c', _FIRST_IMPORTS, json.dumps(HEAVY), json.dumps(datasets)],
            check=False, env=env, capture_output=True, text=True, timeout=300)
        assert done.returncode == 0, done.stderr[-2000:]
        imported = json.loads(done.stdout.splitlines()[-1])
        assert not imported, f'cache {cache}: imported while serving: {imported}'


def test_errors_name_the_cause(validate):
    async def check(client, socket):
        await socket.recv()                                     # hello
        cases = [
            (('nope', {}), 'unknown_method', 'no method'),
            (('catalog.list', {'x': 1}), 'bad_request', "no 'x'"),
            (('dataset.open', {}), 'bad_request', 'needs "id"'),
            (('dataset.open', {'id': 'plate-x'}), 'not_found',
             ('No dataset named "plate-x"; catalog.list names the ones that '
              'exist.')),
            (('op.run', {'op': 'random.nope', 'handle': 'h1'}), 'not_found',
             "No operation named 'random.nope'"),
            (('op.run', {'op': 'modal.modes', 'handle': 'h999'}), 'not_found',
             "No open handle 'h999'"),
        ]
        for (method, params), code, words in cases:
            message = await client.call(method, params)
            validate(message)
            assert message['error']['code'] == code, message
            assert words in message['error']['message']

        drone = (await client.call('dataset.open', {'id': 'drone-fem'}))['result']
        message = await client.call('op.run', {'op': 'random.error_db',
                                               'handle': drone['handle']})
        assert message['error']['code'] == 'refused'
        assert 'random_run' in message['error']['message']
        message = await client.call('op.run', {
            'op': 'modal.modes', 'handle': drone['handle'],
            'params': {'count': 99}})
        assert message['error']['code'] == 'bad_request'
        assert 'the most it takes is 24' in message['error']['message']

        # not a request: the connection is closed with the reason
        await socket.send(frames.encode_text({'type': 'hello'}))
        with pytest.raises(ConnectionClosed):
            await socket.recv()
        assert socket.close_code == 1008
        assert 'integer "id"' in socket.close_reason

    asyncio.run(_session(check))


def test_a_handle_belongs_to_its_connection():
    async def check(client, socket):
        await socket.recv()
        handle = (await client.call('dataset.open',
                                    {'id': 'plate-fem'}))['result']['handle']
        return handle

    service = Service()

    async def both():
        server = await start(service, '127.0.0.1', 0)
        port = server.sockets[0].getsockname()[1]
        url = f'ws://127.0.0.1:{port}/'
        try:
            async with connect(url, max_size=None) as first:
                handle = await check(Client(first), first)
            async with connect(url, max_size=None) as second:
                await second.recv()
                message = await Client(second).call('op.run', {
                    'op': 'modal.modes', 'handle': handle})
                assert message['error']['code'] == 'not_found'
        finally:
            server.close()
            await server.wait_closed()
            service.close()

    asyncio.run(both())


def test_start_changes_nothing_beyond_the_server(monkeypatch):
    # a plain constructor: no imports, no interpreter-wide setting, so a
    # caller (these tests among them) keeps its own
    calls = []
    monkeypatch.setattr('sds_analysis.server.preload', lambda: calls.append('preload'))
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
    monkeypatch.setattr('sds_analysis.server.preload', lambda: calls.append('preload'))
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
        [sys.executable, '-m', 'sds_analysis', '--port', '0'],
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
        assert len(catalog['result']['operations']) == 3
    finally:
        process.terminate()
        process.wait(timeout=20)
