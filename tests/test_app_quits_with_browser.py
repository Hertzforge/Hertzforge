"""`hertzforge` with no command stops once its browser tab has been closed for
`CLOSED_S` seconds (`service.__main__._closed_for`); a reload reconnects in time
and keeps it, and `hertzforge serve`, which the desktop app runs, never stops so."""

from __future__ import annotations

import asyncio

import pytest
from websockets.asyncio.client import connect

from hertzforge.service import __main__ as service_main

GRACE = 0.4


@pytest.fixture(autouse=True)
def _light(monkeypatch):
    # the heavy imports and the process-wide switch interval belong to a real launch
    monkeypatch.setattr(service_main.sys, 'setswitchinterval', lambda _s: None)
    monkeypatch.setattr('hertzforge.service.server.preload', lambda: None)


async def _started(**kwargs):
    bound = asyncio.get_running_loop().create_future()
    task = asyncio.ensure_future(service_main._serve(
        '127.0.0.1', 0, ready=lambda _h, port: bound.set_result(port), **kwargs))
    return task, f'ws://127.0.0.1:{await bound}/'


async def _visit(url, hold=0.0):
    async with connect(url) as socket:
        await socket.recv()          # hello
        await asyncio.sleep(hold)


def test_it_stops_once_its_tab_is_closed_and_not_before():
    async def scenario():
        task, url = await _started(quit_when_closed=GRACE)
        await asyncio.sleep(GRACE * 2)
        assert not task.done(), 'no tab has opened yet'
        await _visit(url, hold=GRACE * 2)
        assert not task.done(), 'the tab was open the whole time'
        await asyncio.wait_for(task, GRACE * 6)

    asyncio.run(scenario())


def test_a_reload_keeps_it_running():
    # a grace long enough that a slow runner still reconnects inside it
    grace = 2.0

    async def scenario():
        task, url = await _started(quit_when_closed=grace)
        await _visit(url)
        await asyncio.sleep(0.1)
        await _visit(url, hold=grace * 1.5)
        assert not task.done(), 'the tab came back inside the grace'
        await asyncio.wait_for(task, grace * 6)

    asyncio.run(scenario())


def test_serve_keeps_running_after_its_clients_leave():
    async def scenario():
        task, url = await _started()
        await _visit(url)
        await asyncio.sleep(GRACE * 3)
        assert not task.done()
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task

    asyncio.run(scenario())


def test_the_app_command_says_how_it_stops(capsys):
    from hertzforge.__main__ import _announce

    _announce('http://127.0.0.1:5/', lambda url: True)
    assert 'It stops when its browser tab is closed, or press Ctrl-C.' in capsys.readouterr().out
