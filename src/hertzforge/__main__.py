"""The ``hertzforge`` command: ``hertzforge --version`` prints the version,
``hertzforge serve`` starts the analysis service."""

import argparse

from hertzforge import __version__


def main(argv=None):
    parser = argparse.ArgumentParser(
        prog="hertzforge",
        description="Hertzforge: data acquisition and structural dynamics in one toolset.",
        epilog="https://github.com/hertzforge/hertzforge",
    )
    parser.add_argument("--version", action="version", version=f"hertzforge {__version__}")
    commands = parser.add_subparsers(dest="command")
    # the service parses its own options, so `hertzforge serve --help` is its help
    commands.add_parser("serve", add_help=False,
                        help="start the analysis service (sds/1 over WebSocket)")
    args, rest = parser.parse_known_args(argv)
    if args.command == "serve":
        from hertzforge.service.__main__ import main as serve

        return serve(rest)
    if rest:
        parser.error(f"unrecognized arguments: {' '.join(rest)}")
    parser.print_help()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
