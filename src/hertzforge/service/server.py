"""The analysis service: sds/1 over WebSocket.

On connect the server sends `hello`. Each request is answered by a
response with its id; requests run concurrently, so a long `project.open`
does not hold up a `catalog.list` sent after it, and responses may come
back in a different order from the requests. Heavy work (opening a
project, running an operation, framing a large result) runs in a worker
thread so the event loop stays responsive.

Each connection has at most one project of its own (`project.*`), which
every operation runs on and which ends with it.
"""

from __future__ import annotations

import asyncio
import copy
import importlib
import itertools
import logging
import pkgutil
import time
from collections.abc import Callable, Mapping, Sequence
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

from websockets.asyncio.server import Server, ServerConnection, serve
from websockets.exceptions import ConnectionClosed

from . import display_units, fitting, frames, projects, reports, run_logs
from .descriptors import Operation, json_type
from .protocol import ProtocolError, bad_request, hello, not_found, refused

log = logging.getLogger('hertzforge.service')

#: the operation modules served first, in this order; every other `ops_*` module of this package
#: follows, by name, so a module of operations is served by being added
FIRST_MODULES = ('ops_project', 'ops_settings', 'ops_model', 'ops_specify', 'ops_readings')


def _operations() -> tuple[Operation, ...]:
    """Every `ops_*` module's `OPERATIONS`, the `FIRST_MODULES` first."""
    found = [m.name for m in pkgutil.iter_modules([str(Path(__file__).parent)])
             if m.name.startswith('ops_')]
    ordered = [n for n in FIRST_MODULES if n in found] + sorted(
        n for n in found if n not in FIRST_MODULES)
    return tuple(op for name in ordered
                 for op in importlib.import_module(f'{__package__}.{name}').OPERATIONS)


OPERATIONS: tuple[Operation, ...] = _operations()

#: the requests on a connection's own project
PROJECT_METHODS = ('project.new', 'project.open', 'project.import', 'project.save',
                   'project.rename', 'project.remove', 'project.delete_parts', 'project.activate',
                   'project.duplicate', 'project.refresh',
                   'project.units', 'project.define_units', 'project.children', 'project.journal',
                   'project.show', 'project.actions')

#: the largest frame a client may send; requests are small
MAX_INCOMING = 16 * 2**20

#: What the readers and operations reach, imported before the server
#: listens. Loading a compiled extension holds the interpreter lock until it
#: is done, so a first import inside a worker thread stalls the event loop
#: with it: 5.1 s once, opening the first file. Imported here, the cost is
#: paid at startup, before anyone is waiting on the loop.
#: tests/test_server.py fails if a session still imports one of these
#: packages for the first time, and names it.
#:
#: Listed module by module, as the readers and operations reach them, not
#: by walking whole packages, which imports every module whether anything
#: uses it or not.
PRELOAD = (
    'hertzforge',
    'hertzforge.drawing.plot',
    'hertzforge.drawing.viz.geometry',
    'hertzforge.drawing.viz.waterfall',
    'hertzforge.deform',
    # what showing a selection reaches, imported inside the core at first use
    'hertzforge.service.show',
    # the table a selection answers with
    'hertzforge.service.action_table',
    'hertzforge.core.merge',
    'hertzforge.core.classical_pulse',
    'hertzforge.core.filters',
    'hertzforge.core.rigid',
    'hertzforge.core.sensors',
    'hertzforge.core.shaker',
    'hertzforge.core.shock_synthesis',
    'hertzforge.core.signals',
    'hertzforge.core.tables',
    'hertzforge.core.transform',
    # imported inside hertzforge at first use: reading a random run and
    # forming its octave and coherence products
    'hertzforge.core.detect',
    'hertzforge.core.octave',
    # fitting a modal survey's FRFs
    'hertzforge.core.modal_fit',
    'hertzforge.core.partial_coherence',
    # solving a model's modes
    'hertzforge.core.sparse_solve',
    # imported inside the readers and solvers, at first use (pandas by the
    # channel table a random run carries)
    'netCDF4',
    'h5py',
    'pandas',
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


def preload() -> None:
    """Import everything in `PRELOAD`."""
    for name in PRELOAD:
        importlib.import_module(name)


#: The interpreter's switch interval while serving, in seconds. It hands its
#: lock between threads every 5 ms by default; a worker doing pure-Python
#: work (reading a file, fitting modes) then costs the event loop tens of
#: milliseconds per wait. At 1 ms the loop gets the lock back sooner, for a
#: small cost in the worker's speed. Process-wide, so the entry point sets
#: it, never `start`.
SWITCH_INTERVAL = 0.001



def _preview_copy(project: Any, name: str) -> Any:
    """A project holding only a copy of geometry `name`, for an operation to act on while
    its form is typed: nothing it does reaches the project or its journal."""
    from hertzforge import Project

    scratch = Project('preview')
    scratch.add(name, copy.deepcopy(project[name]))
    return scratch


def _said_as_preview(views: list[dict[str, Any]], units: Any) -> list[dict[str, Any]]:
    """A preview's views in the display units, saying what would be added."""
    for view in views:
        for row in view.get('rows', ()) if view.get('view') == 'reading' else ():
            if row.get('label') == 'Added':
                row['label'] = 'Would add'
    display_units.convert(views, units)
    return views


class Service:
    """Everything but the transport: the operations and the worker
    threads. One per process."""

    def __init__(self, operations: Sequence[Operation] = OPERATIONS,
                 workers: int = 2) -> None:
        self.operations: dict[str, Operation] = {op.id: op for op in operations}
        self.executor: ThreadPoolExecutor = ThreadPoolExecutor(
            workers, thread_name_prefix='hertzforge-service')

    def close(self) -> None:
        """Stop the worker threads without waiting; work not yet started is
        cancelled."""
        self.executor.shutdown(wait=False, cancel_futures=True)

    async def in_worker(self, call: Callable[..., Any], *args: Any) -> Any:
        """`call(*args)` run on a worker thread, awaited without blocking the
        event loop."""
        return await asyncio.get_running_loop().run_in_executor(
            self.executor, call, *args)

    # ---- methods --------------------------------------------------------------

    def catalog_list(self) -> dict[str, Any]:
        """The `catalog.list` result: every operation's descriptor."""
        return {'operations': [op.to_json() for op in self.operations.values()]}

    def operation(self, params: Any) -> Operation:
        """The operation an `op.run` names."""
        _only(params, ('op', 'params', 'preview'), 'op.run')
        op = self.operations.get(params.get('op'))
        if op is None:
            raise not_found(f'No operation named {params.get("op")!r}; '
                            'catalog.list names the ones that exist.')
        return op


def _only(params: Any, allowed: Sequence[str], method: str) -> None:
    if not isinstance(params, Mapping):
        raise bad_request(f'{method} "params" must be an object, not '
                          f'{json_type(params)}')
    extra = sorted(set(params) - set(allowed))
    if extra:
        raise bad_request(f'{method} takes {", ".join(map(repr, allowed)) or "no params"}; '
                          f'it has no {", ".join(map(repr, extra))}')


#: the shortest gap between two `project.progress` events of one file
PROGRESS_INTERVAL_S = 0.1


class Connection:
    """One client: its project, its requests in flight, and the lock that
    keeps its frames whole."""

    def __init__(self, service: Service, socket: ServerConnection) -> None:
        self.service: Service = service
        self.socket: ServerConnection = socket
        self.project: projects.OpenProject | None = None
        #: one project request at a time: two imports must not add to one
        #: project at once, and a save must not write one half-imported
        self.project_lock: asyncio.Lock = asyncio.Lock()
        #: the mode fit under way on the project (`fit.*`), or None
        self.fit: fitting.Fit | None = None
        #: the unit system views are drawn in (`units.system`); Visual
        #: Dynamics' own to start (in-slinch-lbf-s, accelerations in g)
        self.units: str = display_units.START
        #: the run log this connection keeps (`runlog.*`), its requests one
        #: at a time so entries are written in the order they were sent
        self.run_log: run_logs.Keeper = run_logs.Keeper()
        self.run_log_lock: asyncio.Lock = asyncio.Lock()
        self.tasks: set[asyncio.Task] = set()
        self.sending: asyncio.Lock = asyncio.Lock()
        self._seq = itertools.count(1)

    async def send(self, frame: str | bytes) -> None:
        """Send one frame, under the lock, so concurrent responses never
        interleave on the socket."""
        async with self.sending:
            await self.socket.send(frame)

    async def event(self, topic: str, data: dict[str, Any]) -> None:
        """Send one event on `topic`, numbered in this connection's order."""
        try:
            await self.send(frames.encode_text(
                {'type': 'event', 'topic': topic, 'seq': next(self._seq),
                 'data': data}))
        except ConnectionClosed:
            pass

    def progress(self, path: str) -> Callable[[int, int], None]:
        """A (done, total) callable for a reader on a worker thread that
        sends `project.progress` events, at most one per
        PROGRESS_INTERVAL_S and always the last."""
        loop = asyncio.get_running_loop()
        last = [-PROGRESS_INTERVAL_S]

        def report(done: int, total: int) -> None:
            now = time.monotonic()
            if now - last[0] < PROGRESS_INTERVAL_S and done < total:
                return
            last[0] = now
            data = {'done': int(done), 'total': int(total), 'path': path}
            asyncio.run_coroutine_threadsafe(
                self.event('project.progress', data), loop)
        return report

    async def project_request(self, method: str,
                              params: Any) -> dict[str, Any]:
        """Answer one `project.*` request on this connection's project, after
        any still running."""
        async with self.project_lock:
            return await self._project_request(method, params)

    async def _project_request(self, method: str,
                               params: Any) -> dict[str, Any]:
        call = self.service.in_worker
        if method == 'project.new':
            _only(params, ('title',), method)
            self.project = await call(projects.new, params)
            self.fit = None
            return {'project': projects.summary(self.project)}
        if method == 'project.open':
            _only(params, ('path',), method)
            self.project = await call(projects.open_file, params)
            self.fit = None
            return {'project': projects.summary(self.project)}
        if method == 'project.import':
            _only(params, ('path', 'units'), method)
            path = params.get('path')
            report = self.progress(path) if isinstance(path, str) else None
            self.project, added = await call(projects.import_file, self.project,
                                             params, report)
            return {'added': added, 'project': projects.summary(self.project)}
        edits = {'project.rename': (projects.rename, ('name', 'to', 'index')),
                 'project.remove': (projects.remove, ('names',)),
                 'project.delete_parts': (projects.delete_parts, ('parts',)),
                 'project.activate': (projects.activate, ('geometry',)),
                 'project.duplicate': (projects.duplicate, ('names',)),
                 'project.refresh': (projects.refresh, ('names',))}
        if method == 'project.journal':
            _only(params, (), method)
            return await call(projects.journal, self.project, params)
        if method == 'project.units':
            _only(params, ('name',), method)
            return await call(projects.units, self.project, params)
        if method == 'project.children':
            _only(params, ('name',), method)
            return await call(projects.children, self.project, params)
        if method == 'project.actions':
            _only(params, ('objects', 'records', 'part'), method)
            return await call(projects.actions, self.project, params)
        if method == 'project.show':
            _only(params, ('objects', 'records', 'part', 'prefs'), method)
            shown = await call(projects.show, self.project, params)
            display_units.convert(shown['views'], self.units)
            return shown
        if method == 'project.define_units':
            _only(params, ('name', 'units'), method)
            converted = await call(projects.define_units, self.project, params)
            assert self.project is not None
            return {'converted': converted, 'project': projects.summary(self.project)}
        if method in edits:
            edit, keys = edits[method]
            _only(params, keys, method)
            await call(edit, self.project, params)
            assert self.project is not None
            return {'project': projects.summary(self.project)}
        _only(params, ('path', 'compress'), method)
        written = await call(projects.save, self.project, params)
        assert self.project is not None
        return {'path': written, 'project': projects.summary(self.project)}

    async def project_op(self, params: Mapping[str, Any]) -> dict[str, Any]:
        """An operation on this connection's project: its views and the
        project's summary, which counts as modified when the operation
        added or removed an object."""
        op = self.service.operation(params)
        async with self.project_lock:
            state = self.project
            if state is None:
                raise refused(f'{op.title} works on a project, and none is open; '
                              'project.open or project.import opens one')
            project = state.project
            values = op.resolve(
                params.get('params', {}),
                objects={name: projects.kind_of(obj)
                         for name, obj in project.items()},
                verbs={name: projects.verbs_of(project, name) for name in project})
            if op.id == 'project.save_as' and values['units'] == 'display':
                values['units'] = self.units
            if params.get('preview') and not op.previews:
                raise bad_request(f'{op.id} does not preview')
            if params.get('preview'):
                scratch = _preview_copy(project, values['geometry'])
                return {'views': _said_as_preview(
                    await self.service.in_worker(op.run, scratch, values), self.units)}
            before, journaled = list(project), len(project.journal)
            views = await self.service.in_worker(op.run, project, values)
            if op.edits or list(project) != before or len(project.journal) != journaled:
                state.modified = True
            display_units.convert(views, self.units)
            return {'views': views, 'project': projects.summary(state)}

    async def report_request(self, method: str, params: Any) -> dict[str, Any]:
        """Answer one `report.*` request on this connection's project."""
        keys = {'report.generate': ('template', 'name'), 'report.render': ('name', 'theme'),
                'report.export': ('name', 'path', 'anyway'),
                'report.editor': ('name', 'selected', 'theme', 'page'),
                'report.operate': ('name', 'op', 'selected', 'theme', 'at', 'from', 'to',
                                   'kind', 'field', 'value'),
                'report.print': ('name',), 'report.issue': ('name', 'step', 'path', 'anyway')}
        _only(params, keys[method], method)
        call = self.service.in_worker
        async with self.project_lock:
            if self.project is None:
                raise refused(f'{method} works on a project, and none is open')
            project = self.project.project
            if method == 'report.generate':
                name = await call(reports.generate, project, params)
                self.project.modified = True
                return {'name': name, 'project': projects.summary(self.project)}
            if method == 'report.render':
                html = await call(reports.render, project, params, 'report.render', self.system)
                return {'name': params['name'], 'html': html}
            if method == 'report.print':
                return {'html': await call(reports.print_page, project, params, self.system)}
            if method == 'report.issue':
                done = await call(reports.issue, project, params, self.system)
                self.project.modified = True
                return {**done, 'project': projects.summary(self.project)}
            if method == 'report.editor':
                return await call(reports.edit_page, project, params, self.system)
            if method == 'report.operate':
                edited = await call(reports.operate, project, params, self.system)
                self.project.modified = True
                return {**edited, 'project': projects.summary(self.project)}
            return await call(reports.export, project, params, self.system)

    @property
    def system(self) -> Any:
        """The `UnitSystem` this connection draws in."""
        from hertzforge.units import SYSTEMS

        return SYSTEMS[self.units]

    def fit_state(self, fit: fitting.Fit) -> dict[str, Any]:
        """The fit's state with its CMIF drawn in this connection's units."""
        state = fitting.state(fit)
        display_units.convert([state['view']], self.units)
        return state

    def units_request(self, params: Any) -> dict[str, Any]:
        """`units.system`: the unit system this connection's views are drawn
        in, set when `name` is given; the systems there are."""
        _only(params, ('name',), 'units.system')
        name = params.get('name')
        if name is not None:
            if name not in display_units.systems():
                raise bad_request(f'units.system "name" is one of '
                                  f'{", ".join(display_units.systems())}')
            self.units = name
        return {'name': self.units, 'systems': display_units.systems()}

    async def fit_request(self, method: str, params: Any) -> dict[str, Any]:
        """Answer one `fit.*` request: a step of the mode fit on this
        connection's project, and the fit's state after it."""
        if not isinstance(params, Mapping):
            raise bad_request(f'{method} "params" must be an object, not '
                              f'{json_type(params)}')
        call = self.service.in_worker
        async with self.project_lock:
            if self.project is None:
                raise refused(f'{method} works on a project, and none is open')
            project = self.project.project
            if method == 'fit.start':
                _only(params, ('frf', 'shapes', 'coherence'), method)
                self.fit = await call(fitting.start, project, params)
                return {'fit': await call(self.fit_state, self.fit)}
            if method == 'fit.end':
                _only(params, (), method)
                self.fit = None
                return {}
            fit = self.fit
            if fit is None:
                raise refused(f'{method} needs a fit under way; fit.start begins one')
            if fit.frf not in project:
                self.fit = None
                raise refused(f'{fit.frf}, the FRF being fitted, is no longer in the '
                              'project; start the fit again')
            if method == 'fit.publish':
                _only(params, ('name',), method)
                name = await call(fitting.publish, project, fit, params)
                self.project.modified = True
                return {'name': name, 'project': projects.summary(self.project),
                        'fit': await call(self.fit_state, fit)}
            keys = {'fit.find': ('low', 'high'), 'fit.move': ('frequency',),
                    'fit.damping': ('damping',), 'fit.confirm': ('description',),
                    'fit.delete': ('modes',), 'fit.edit': ('mode', 'damping', 'description'),
                    'fit.residual': ('show',)}
            _only(params, keys.get(method, ()), method)
            await call(fitting.act, fit, method, params)
            return {'fit': await call(self.fit_state, fit)}

    async def serve(self) -> None:
        """Send `hello`, then answer each request in its own task until the
        client goes. A frame that will not decode closes the connection with
        1007, a message that is not a request with an integer id with 1008;
        requests still running when it ends are cancelled."""
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
            # the log's lock is released with the connection, so another
            # writer can take it
            self.run_log.close()

    async def answer(self, request: dict[str, Any]) -> None:
        """Run one request and send its response. A ProtocolError becomes an
        error response with its code; any other exception is logged and sent
        as `internal`, so one failing request never ends the connection."""
        ident, method = request['id'], request.get('method')
        params = request.get('params', {})
        started = time.perf_counter()
        try:
            if method == 'catalog.list':
                _only(params, (), 'catalog.list')
                frame = frames.encode_text(
                    {'type': 'response', 'id': ident,
                     'result': self.service.catalog_list()})
            elif method == 'op.run':
                result = await self.project_op(params)
                frame = await self.service.in_worker(
                    frames.pack, {'type': 'response', 'id': ident,
                                  'result': result})
            elif method in PROJECT_METHODS:
                result = await self.project_request(method, params)
                if method == 'project.show':
                    # its views carry arrays, as an operation's do
                    frame = await self.service.in_worker(
                        frames.pack, {'type': 'response', 'id': ident, 'result': result})
                else:
                    frame = frames.encode_text({'type': 'response', 'id': ident,
                                                'result': result})
            elif method in reports.REPORT_METHODS:
                result = await self.report_request(method, params)
                frame = frames.encode_text({'type': 'response', 'id': ident,
                                            'result': result})
            elif method == 'units.system':
                frame = frames.encode_text({'type': 'response', 'id': ident,
                                            'result': self.units_request(params)})
            elif method in run_logs.RUN_LOG_METHODS:
                async with self.run_log_lock:
                    project = self.project.project if self.project is not None else None
                    result = await self.service.in_worker(run_logs.request, self.run_log,
                                                          method, params, project, self.system)
                frame = frames.encode_text({'type': 'response', 'id': ident,
                                            'result': result})
            elif method in fitting.FIT_METHODS:
                result = await self.fit_request(method, params)
                frame = await self.service.in_worker(
                    frames.pack, {'type': 'response', 'id': ident,
                                  'result': result})
            else:
                raise ProtocolError(
                    'unknown_method',
                    f'The analysis service has no method {method!r}; it '
                    'answers catalog.list, op.run, '
                    f'{", ".join(PROJECT_METHODS)}, '
                    f'{", ".join(fitting.FIT_METHODS)}, '
                    f'{", ".join(run_logs.RUN_LOG_METHODS)} and '
                    f'{", ".join(reports.REPORT_METHODS)}.')
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
    which). It changes nothing beyond the server: the process that serves
    calls `preload` and sets `SWITCH_INTERVAL` first (`__main__._serve`)."""

    async def handler(socket: ServerConnection) -> None:
        log.info('connection from %s', socket.remote_address)
        await Connection(service, socket).serve()
        log.info('connection from %s closed', socket.remote_address)

    # no per-message compression: loopback, and deflating a dense model's
    # shapes would run on the event loop
    return await serve(handler, host, port, compression=None,
                       max_size=MAX_INCOMING)
