"""Start the analysis service.

    hertzforge serve --port 8791
    python -m hertzforge.service --port 8791

With `--watch-stdin` it serves until its standard input reaches its end:
a parent that starts the service (the desktop app) closes the pipe to stop
it, and the pipe closes by itself if the parent dies, so the service never
outlives it.
"""

import argparse
import asyncio
import contextlib
import logging
import os
import sys
import threading


def main(argv: list[str] | None = None) -> int:
    """Parse the `hertzforge serve` arguments, set up logging to stderr and
    serve until interrupted. Ctrl-C ends it quietly; the return value is
    the exit status, always 0."""
    parser = argparse.ArgumentParser(
        prog='hertzforge serve',
        description='The analysis service of the structural dynamics suite '
                    '(protocol sds/1 over WebSocket).')
    parser.add_argument('--host', default='127.0.0.1',
                        help='address to listen on (default 127.0.0.1)')
    parser.add_argument('--port', type=int, default=8791,
                        help='port to listen on (default 8791; 0 picks one)')
    parser.add_argument('--log-level', default='INFO',
                        choices=('DEBUG', 'INFO', 'WARNING', 'ERROR'))
    parser.add_argument('--watch-stdin', action='store_true',
                        help='serve until standard input closes (for a parent '
                             'process that starts the service)')
    args = parser.parse_args(argv)
    logging.basicConfig(level=args.log_level, stream=sys.stderr,
                        format='%(asctime)s %(name)s %(levelname)s %(message)s')
    try:
        asyncio.run(_serve(args.host, args.port, watch_stdin=args.watch_stdin))
    except KeyboardInterrupt:
        pass
    return 0


def _stdin_closed(loop: asyncio.AbstractEventLoop) -> asyncio.Future:
    """A future that completes when standard input reaches its end, read on
    a thread of its own. A read that fails (a broken pipe, or no standard
    input at all) counts as the end."""
    closed = loop.create_future()

    def done() -> None:
        if not closed.done():
            closed.set_result(None)

    def watch() -> None:
        try:
            fd = sys.stdin.fileno()
            while os.read(fd, 256):
                pass
        except (AttributeError, OSError, ValueError):
            pass
        loop.call_soon_threadsafe(done)

    threading.Thread(target=watch, name='stdin-watch', daemon=True).start()
    return closed


async def _serve(host: str, port: int, *, watch_stdin: bool = False) -> None:
    from .server import SWITCH_INTERVAL, Service, preload, start

    # Process-wide setup, done here and not in start(), so a caller that
    # starts a server in its own process keeps its own settings. The heavy
    # imports (the core, numpy and scipy, what the operations reach)
    # load before the port opens: a launcher that waits for the line below
    # gets a service whose first project.open does not pay for them.
    preload()
    sys.setswitchinterval(SWITCH_INTERVAL)

    service = Service()
    server = await start(service, host, port)
    bound = server.sockets[0].getsockname()
    # one line a launcher can wait for
    print(f'hertzforge analysis service listening on ws://{bound[0]}:{bound[1]}/',
          file=sys.stderr, flush=True)
    try:
        serving = asyncio.ensure_future(server.serve_forever())
        if watch_stdin:
            closed = _stdin_closed(asyncio.get_running_loop())
            closed.add_done_callback(lambda _: serving.cancel())
            try:
                await serving
            except asyncio.CancelledError:
                if not closed.done():
                    raise
            # the parent may have gone, and its end of a pipe with it
            with contextlib.suppress(OSError):
                print('hertzforge analysis service: standard input closed; stopping',
                      file=sys.stderr, flush=True)
        else:
            await serving
    finally:
        server.close()
        await server.wait_closed()
        service.close()


if __name__ == '__main__':
    sys.exit(main())
