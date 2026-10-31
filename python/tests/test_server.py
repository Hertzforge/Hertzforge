"""The service over a real socket: hello, the three methods, the errors,
and an event loop that stays free while the work runs."""

from __future__ import annotations

import asyncio
import json
import re
import subprocess
import sys
import time

import pytest
from websockets.asyncio.client import connect

from sds_analysis import frames
from sds_analysis.server import Service, start


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


async def _session(check):
    service = Service()
    server = await start(service, '127.0.0.1', 0)
    port = server.sockets[0].getsockname()[1]
    lag = {'max': 0.0, 'run': True}

    async def ticker():
        last = time.perf_counter()
        while lag['run']:
            await asyncio.sleep(0.005)
            now = time.perf_counter()
            lag['max'] = max(lag['max'], now - last - 0.005)
            last = now

    tick = asyncio.create_task(ticker())
    try:
        async with connect(f'ws://127.0.0.1:{port}/', max_size=None,
                           compression=None) as socket:
            await check(Client(socket), socket)
    finally:
        lag['run'] = False
        await tick
        server.close()
        await server.wait_closed()
        service.close()
    return lag['max']


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

    lag = asyncio.run(_session(check))
    assert seen['random.psd_vs_spec'][0]['series'][0]['name'].startswith('104Z+')
    # heavy work ran off the loop: the ticker was never held up for long
    print(f'longest event-loop stall {lag * 1000:.1f} ms')
    assert lag < 0.1, f'event loop held for {lag:.3f} s'


def test_errors_name_the_cause(validate):
    async def check(client, socket):
        await socket.recv()                                     # hello
        cases = [
            (('nope', {}), 'unknown_method', 'no method'),
            (('catalog.list', {'x': 1}), 'bad_request', "no 'x'"),
            (('dataset.open', {}), 'bad_request', 'needs "id"'),
            (('dataset.open', {'id': 'plate-x'}), 'not_found',
             'No dataset named "plate-x"; catalog.list names the ones that '
             'exist.'),
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
        with pytest.raises(Exception):
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
