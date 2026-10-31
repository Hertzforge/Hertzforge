"""The analysis service: sds/1 over WebSocket.

On connect the server sends `hello`. Each request is answered by a
response with its id; requests run concurrently, so a long `dataset.open`
does not hold up a `catalog.list` sent after it, and responses may come
back in a different order from the requests. Heavy work (opening a
dataset, running an operation, framing a large result) runs in a worker
thread so the event loop stays responsive.

Datasets are opened once per process and shared; a handle belongs to the
connection that opened it and ends with it.
"""

from __future__ import annotations

import asyncio
import importlib
import itertools
import logging
import pkgutil
import sys
import time
from collections.abc import Callable, Mapping, Sequence
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from typing import Any

from websockets.asyncio.server import Server, ServerConnection, serve
from websockets.exceptions import ConnectionClosed

from . import frames
from .datasets import CATALOG, Entry
from .descriptors import Operation, json_type
from .ops_modal import OPERATIONS as MODAL_OPERATIONS
from .ops_random import OPERATIONS as RANDOM_OPERATIONS
from .protocol import ProtocolError, bad_request, hello, not_found, refused

log = logging.getLogger('sds_analysis')

OPERATIONS: tuple[Operation, ...] = RANDOM_OPERATIONS + MODAL_OPERATIONS

#: the largest frame a client may send; requests are small
MAX_INCOMING = 16 * 2**20

#: What the operations reach, imported before the server listens. Loading a
#: compiled extension holds the interpreter lock until it is done, so a first
#: import inside a worker thread stalls the event loop with it: 5.1 s once,
#: opening the first dataset with nothing on disk cached yet. Imported here,
#: the cost is paid at startup, before anyone is waiting on the loop.
#: tests/test_server.py fails if a session still imports one of these
#: packages for the first time, and names it.
PRELOAD = (
    'visualdynamics',
    'visualdynamics.plot',
    'visualdynamics.viz.geometry',
    'visualdynamics.deform',
    'visualdynamics.demo.drone',
    'visualdynamics.demo.plate',
    'visualdynamics.demo.frame_wing',
    # imported inside the readers and solvers, at first use
    'netCDF4',
    'h5py',
    'scipy.constants',
    'scipy.integrate',
    'scipy.interpolate',
    'scipy.linalg',
    'scipy.ndimage',
    'scipy.optimize',
    'scipy.signal',
    'scipy.sparse',
    'scipy.sparse.linalg',
    'scipy.spatial',
    'scipy.special',
    'scipy.stats',
)

#: Packages imported whole: their modules import each other at first use.
PRELOAD_PACKAGES = ('visualdynamics.core', 'visualdynamics.io')


def preload() -> None:
    """Import everything in `PRELOAD`, and every module of `PRELOAD_PACKAGES`.
    A module whose optional dependency is not installed is skipped, as the
    package itself skips it."""
    for name in PRELOAD:
        importlib.import_module(name)
    for package in PRELOAD_PACKAGES:
        for module in pkgutil.walk_packages(importlib.import_module(package).__path__,
                                            f'{package}.'):
            try:
                importlib.import_module(module.name)
            except ImportError as error:
                if (error.name or '').startswith('visualdynamics'):
                    raise
                log.debug('preload: %s needs %s, not installed', module.name, error.name)


@dataclass
class Handle:
    name: str
    entry: Entry
    data: Any


class Service:
    """Everything but the transport: the catalog, the datasets opened so
    far, and the worker threads. One per process."""

    def __init__(self, operations: Sequence[Operation] = OPERATIONS,
                 catalog: Sequence[Entry] = CATALOG,
                 workers: int = 2) -> None:
        self.operations = {op.id: op for op in operations}
        self.catalog = tuple(catalog)
        self.executor = ThreadPoolExecutor(workers,
                                           thread_name_prefix='sds-analysis')
        self._opening: dict[str, asyncio.Future] = {}
        self._handles = itertools.count(1)

    def close(self) -> None:
        self.executor.shutdown(wait=False, cancel_futures=True)

    async def in_worker(self, call: Callable[..., Any], *args: Any) -> Any:
        return await asyncio.get_running_loop().run_in_executor(
            self.executor, call, *args)

    # ---- methods --------------------------------------------------------------

    def catalog_list(self) -> dict[str, Any]:
        return {'operations': [op.to_json() for op in self.operations.values()],
                'datasets': [item.to_json() for item in self.catalog]}

    async def load(self, item: Entry) -> Any:
        """The dataset, opened at most once however many ask at the same
        time; a failed open is forgotten so it can be tried again."""
        future = self._opening.get(item.id)
        if future is None:
            future = asyncio.ensure_future(self.in_worker(item.open))
            self._opening[item.id] = future
        try:
            return await asyncio.shield(future)
        except BaseException:
            if future.done() and self._opening.get(item.id) is future:
                del self._opening[item.id]
            raise

    async def dataset_open(self, params: Mapping[str, Any],
                           handles: dict[str, Handle]) -> dict[str, Any]:
        _only(params, ('id',), 'dataset.open')
        if 'id' not in params:
            raise bad_request('dataset.open needs "id", a dataset id from '
                              'catalog.list')
        item = next((d for d in self.catalog if d.id == params['id']), None)
        if item is None:
            raise not_found(f'No dataset named "{params["id"]}"; catalog.list '
                            'names the ones that exist.')
        data = await self.load(item)
        name = f'h{next(self._handles)}'
        handles[name] = Handle(name, item, data)
        return {'handle': name, 'kind': item.kind, 'title': item.title,
                'summary': data.summary(),
                'channels': [c.to_json() for c in data.channels]}

    async def op_run(self, params: Mapping[str, Any],
                     handles: dict[str, Handle]) -> dict[str, Any]:
        """The result of one operation; the caller frames it in a worker,
        since a dense model's shapes are megabytes."""
        _only(params, ('op', 'handle', 'params'), 'op.run')
        op = self.operations.get(params.get('op'))
        if op is None:
            raise not_found(f'No operation named {params.get("op")!r}; '
                            'catalog.list names the ones that exist.')
        handle = handles.get(params.get('handle'))
        if handle is None:
            raise not_found(f'No open handle {params.get("handle")!r} on this '
                            'connection; dataset.open returns one, and a '
                            'handle ends with its connection.')
        if handle.entry.kind != op.accepts:
            raise refused(f'{op.id} works on a {op.accepts} dataset; handle '
                          f'{handle.name} is {handle.entry.title}, a '
                          f'{handle.entry.kind}.')
        values = op.resolve(params.get('params', {}),
                            [c.value for c in handle.data.channels])
        views = await self.in_worker(op.run, handle.data, values)
        return {'views': views}


def _only(params: Any, allowed: Sequence[str], method: str) -> None:
    if not isinstance(params, Mapping):
        raise bad_request(f'{method} "params" must be an object, not '
                          f'{json_type(params)}')
    extra = sorted(set(params) - set(allowed))
    if extra:
        raise bad_request(f'{method} takes {", ".join(map(repr, allowed)) or "no params"}; '
                          f'it has no {", ".join(map(repr, extra))}')


class Connection:
    """One client: its handles, its requests in flight, and the lock that
    keeps its frames whole."""

    def __init__(self, service: Service, socket: ServerConnection) -> None:
        self.service = service
        self.socket = socket
        self.handles: dict[str, Handle] = {}
        self.tasks: set[asyncio.Task] = set()
        self.sending = asyncio.Lock()

    async def send(self, frame: str | bytes) -> None:
        async with self.sending:
            await self.socket.send(frame)

    async def serve(self) -> None:
        await self.send(frames.encode_text(hello()))
        try:
            async for frame in self.socket:
                try:
                    message = frames.decode(frame)
                except frames.FrameError as error:
                    await self.socket.close(1007, _reason(
                        f'frame not readable as sds/1: {error}'))
                    return
                ident = message.get('id')
                if (message.get('type') != 'request' or isinstance(ident, bool)
                        or not isinstance(ident, int)):
                    await self.socket.close(1008, _reason(
                        'expected a request with an integer "id"; the '
                        'analysis service takes requests only'))
                    return
                task = asyncio.create_task(self.answer(message))
                self.tasks.add(task)
                task.add_done_callback(self.tasks.discard)
        except ConnectionClosed:
            pass
        finally:
            for task in self.tasks:
                task.cancel()

    async def answer(self, request: dict[str, Any]) -> None:
        ident, method = request['id'], request.get('method')
        params = request.get('params', {})
        started = time.perf_counter()
        try:
            if method == 'catalog.list':
                _only(params, (), 'catalog.list')
                frame = frames.encode_text(
                    {'type': 'response', 'id': ident,
                     'result': self.service.catalog_list()})
            elif method == 'dataset.open':
                result = await self.service.dataset_open(params, self.handles)
                frame = frames.pack({'type': 'response', 'id': ident,
                                     'result': result})
            elif method == 'op.run':
                result = await self.service.op_run(params, self.handles)
                frame = await self.service.in_worker(
                    frames.pack, {'type': 'response', 'id': ident,
                                  'result': result})
            else:
                raise ProtocolError(
                    'unknown_method',
                    f'The analysis service has no method {method!r}; it '
                    'answers catalog.list, dataset.open and op.run.')
        except ProtocolError as error:
            frame = _error(ident, error.code, error.message)
        except asyncio.CancelledError:
            raise
        except Exception as error:
            log.exception('request %s %s failed', ident, method)
            frame = _error(ident, 'internal',
                           f'{method} failed inside the service: '
                           f'{type(error).__name__}: {error}')
        log.info('%s #%s %.3f s', method, ident, time.perf_counter() - started)
        try:
            await self.send(frame)
        except ConnectionClosed:
            pass


def _error(ident: int, code: str, message: str) -> str:
    return frames.encode_text({'type': 'response', 'id': ident,
                               'error': {'code': code, 'message': message}})


def _reason(text: str) -> str:
    """A close reason fits in 123 bytes of UTF-8."""
    data = text.encode('utf-8')
    return text if len(data) <= 123 else data[:120].decode('utf-8', 'ignore') + '...'


async def start(service: Service, host: str = '127.0.0.1',
                port: int = 8791) -> Server:
    """A listening server (port 0 picks a free one; `server.sockets` says
    which). The heavy imports are done first (`preload`)."""
    preload()
    # The interpreter hands its lock between threads every 5 ms by default;
    # a worker doing pure-Python work (building a demonstration model) then
    # costs the event loop tens of milliseconds per wait. At 1 ms the loop
    # gets the lock back sooner, for a small cost in the worker's speed.
    sys.setswitchinterval(0.001)

    async def handler(socket: ServerConnection) -> None:
        log.info('connection from %s', socket.remote_address)
        await Connection(service, socket).serve()
        log.info('connection from %s closed', socket.remote_address)

    # no per-message compression: loopback, and deflating a dense model's
    # shapes would run on the event loop
    return await serve(handler, host, port, compression=None,
                       max_size=MAX_INCOMING)
