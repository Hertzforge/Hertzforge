"""`sds-analysis`: start the analysis service.

    uv run --project python sds-analysis --port 8791
"""

import os

# Headless, before visualdynamics is imported (the package __init__ does the
# same; repeated here so the entry module holds it whatever imports it).
os.environ.setdefault('QT_QPA_PLATFORM', 'offscreen')
os.environ.setdefault('PYVISTA_OFF_SCREEN', 'true')

import argparse
import asyncio
import logging
import sys


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog='sds-analysis',
        description='The analysis service of the structural dynamics suite '
                    '(protocol sds/1 over WebSocket).')
    parser.add_argument('--host', default='127.0.0.1',
                        help='address to listen on (default 127.0.0.1)')
    parser.add_argument('--port', type=int, default=8791,
                        help='port to listen on (default 8791; 0 picks one)')
    parser.add_argument('--log-level', default='INFO',
                        choices=('DEBUG', 'INFO', 'WARNING', 'ERROR'))
    args = parser.parse_args(argv)
    logging.basicConfig(level=args.log_level, stream=sys.stderr,
                        format='%(asctime)s %(name)s %(levelname)s %(message)s')
    try:
        asyncio.run(_serve(args.host, args.port))
    except KeyboardInterrupt:
        pass
    return 0


async def _serve(host: str, port: int) -> None:
    from .server import SWITCH_INTERVAL, Service, preload, start

    # Process-wide setup, done here and not in start(), so a caller that
    # starts a server in its own process keeps its own settings. The heavy
    # imports (visualdynamics, its Qt and VTK, what the operations reach)
    # load before the port opens: a launcher that waits for the line below
    # gets a service whose first dataset.open does not pay for them.
    preload()
    sys.setswitchinterval(SWITCH_INTERVAL)

    service = Service()
    server = await start(service, host, port)
    bound = server.sockets[0].getsockname()
    # one line a launcher can wait for
    print(f'sds-analysis listening on ws://{bound[0]}:{bound[1]}/',
          file=sys.stderr, flush=True)
    try:
        await server.serve_forever()
    finally:
        server.close()
        await server.wait_closed()
        service.close()


if __name__ == '__main__':
    sys.exit(main())
