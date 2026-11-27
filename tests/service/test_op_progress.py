"""Progress through an operation, and its cancel (Brandon, 2026-11-11: a long solve
showed no progress and could not be stopped, which the earlier toolset could)."""

from __future__ import annotations

import asyncio
import time

from websockets.asyncio.client import connect

from hertzforge.core.progress import BUSY
from hertzforge.service import frames, op_progress
from hertzforge.service import server as server_module
from hertzforge.service.descriptors import Operation
from hertzforge.service.server import OPERATIONS, Service, start


def _slow(project, params):
    """Counts to 50 with the progress hook, a stage it cannot count in the middle, and
    stops only when its progress says so (or after ten seconds)."""
    report = op_progress.hook()
    report(0, 50)
    report(0, BUSY)
    started = time.monotonic()
    k = 0
    while time.monotonic() - started < 10:
        report(k % 50, 50)
        k += 1
        time.sleep(0.01)
    return []


SLOW = Operation(id='project.test_slow', title='Slow Thing', workspace='signals',
                 summary='A test operation that runs until cancelled.', params=(), outputs=(),
                 run=_slow)


async def _run_and_cancel(cancel_after: float | None):
    service = Service((*OPERATIONS, SLOW))
    server = await start(service, port=0)
    port = server.sockets[0].getsockname()[1]
    messages = []
    try:
        async with connect(f'ws://127.0.0.1:{port}', max_size=None) as socket:
            frames.decode(await socket.recv())
            await socket.send(frames.encode_text({'type': 'request', 'id': 1, 'method': 'op.run',
                                                  'params': {'op': SLOW.id, 'params': {}}}))
            if cancel_after is not None:
                await asyncio.sleep(cancel_after)
                await socket.send(frames.encode_text({'type': 'request', 'id': 2,
                                                      'method': 'op.cancel', 'params': {}}))
            while True:
                message = frames.decode(await asyncio.wait_for(socket.recv(), 15))
                messages.append(message)
                if message['type'] == 'response' and message['id'] == 1:
                    break
    finally:
        server.close()
        await server.wait_closed()
        service.close()
    return messages


def test_an_operation_reports_its_progress_and_stops_when_cancelled(validate):
    messages = asyncio.run(_run_and_cancel(0.6))
    for message in messages:
        validate(message)
    events = [m['data'] for m in messages if m['type'] == 'event' and m['topic'] == 'op.progress']
    for data in events:
        validate(data, 'opProgress')
        assert data['op'] == SLOW.id
    assert {'done': 0, 'total': BUSY} in [{k: e[k] for k in ('done', 'total')} for e in events], \
        'a stage that cannot count itself is said as such'
    assert 2 <= len(events) <= 20, 'about ten a second, not one a step'
    cancelled = next(m for m in messages if m['type'] == 'response' and m['id'] == 2)
    validate(cancelled['result'], 'opCancelResult')
    assert cancelled['result'] == {'op': SLOW.id}
    answer = messages[-1]
    assert answer['error']['code'] == 'refused'
    assert 'Slow Thing was cancelled' in answer['error']['message']


def test_with_nothing_running_a_cancel_says_so():
    async def ask():
        service = Service()
        server = await start(service, port=0)
        port = server.sockets[0].getsockname()[1]
        try:
            async with connect(f'ws://127.0.0.1:{port}', max_size=None) as socket:
                frames.decode(await socket.recv())
                await socket.send(frames.encode_text({'type': 'request', 'id': 1,
                                                      'method': 'op.cancel', 'params': {}}))
                return frames.decode(await socket.recv())
        finally:
            server.close()
            await server.wait_closed()
            service.close()

    assert asyncio.run(ask())['result'] == {'op': None}


def test_solve_modes_reports_to_the_running_operation():
    from test_fem import square_plate

    import hertzforge as hz
    from hertzforge.core.fem import GroupProperties, Material
    from hertzforge.service.ops_project import OPERATIONS as PROJECT

    geometry = square_plate(4).geometry()
    geometry.group_properties = {int(geometry.group_id[0]): GroupProperties(
        Material('aluminum', youngs_modulus=70e9, density=2700, poissons_ratio=0.3), 0.01)}
    project = hz.Project('model')
    project.add('Skin', geometry)
    op = next(o for o in PROJECT if o.id == 'project.solve_modes')
    seen = []
    values = op.resolve({'source': 'Skin', 'num_modes': 10}, objects={'Skin': 'geometry'},
                        verbs={'Skin': ['solve_modes']})
    op_progress.running_with(lambda done, total: seen.append((done, total)), op.run, project,
                             values)
    assert seen, 'the solve reported its progress'
    assert op_progress.hook() is None, 'and only while it ran'


def _quiet(project, params):
    """Works for 0.4 s in one stage that reports nothing, as one long library call does."""
    time.sleep(0.4)
    return []


def _once(project, params):
    """Reports 3 of 10 once, then works 0.4 s without another word."""
    op_progress.hook()(3, 10)
    time.sleep(0.4)
    return []


QUIET = Operation(id='project.test_quiet', title='Quiet Thing', workspace='signals',
                  summary='A test operation that reports nothing.', params=(), outputs=(),
                  run=_quiet)
ONCE = Operation(id='project.test_once', title='Once Thing', workspace='signals',
                 summary='A test operation that reports once.', params=(), outputs=(),
                 run=_once)


async def _heard(op):
    """The op.progress events while `op` runs and for 0.3 s after its response."""
    service = Service((*OPERATIONS, op))
    server = await start(service, port=0)
    port = server.sockets[0].getsockname()[1]
    before, after = [], []
    try:
        async with connect(f'ws://127.0.0.1:{port}', max_size=None) as socket:
            frames.decode(await socket.recv())
            await socket.send(frames.encode_text({'type': 'request', 'id': 1, 'method': 'op.run',
                                                  'params': {'op': op.id, 'params': {}}}))
            heard, ends = before, asyncio.get_running_loop().time() + 15
            while (left := ends - asyncio.get_running_loop().time()) > 0:
                try:
                    message = frames.decode(await asyncio.wait_for(socket.recv(), left))
                except TimeoutError:
                    break
                if message['type'] == 'response' and message['id'] == 1:
                    heard, ends = after, asyncio.get_running_loop().time() + 0.3
                elif message.get('topic') == 'op.progress':
                    heard.append(message['data'])
    finally:
        server.close()
        await server.wait_closed()
        service.close()
    return before, after


def test_an_operation_that_reports_nothing_still_says_it_is_running(monkeypatch):
    """A long stage with no report of its own, a factorization say, is not a silent service:
    the shell waits on an operation for as long as `op.progress` keeps coming."""
    monkeypatch.setattr(server_module, 'HEARTBEAT_S', 0.05)
    before, after = asyncio.run(_heard(QUIET))
    assert len(before) >= 3
    assert all(d == {'op': QUIET.id, 'done': 0, 'total': -1} for d in before)
    assert after == [], 'the heartbeat stops with the operation'


def test_the_heartbeat_repeats_the_last_progress_reported(monkeypatch):
    monkeypatch.setattr(server_module, 'HEARTBEAT_S', 0.05)
    before, after = asyncio.run(_heard(ONCE))
    assert before[0] == {'op': ONCE.id, 'done': 3, 'total': 10}
    assert len(before) >= 3 and all(d == before[0] for d in before)
    assert after == []
