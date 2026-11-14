"""The ``hertzforge`` command: ``hertzforge --version`` prints the version,
``hertzforge serve`` starts the analysis service, ``hertzforge check-solver``
says which sparse solver this installation factors with, and ``hertzforge
convert-run FOLDER [-o OUTPUT]`` converts a run folder into a project file."""

import argparse

from hertzforge import __version__


def main(argv: list[str] | None = None) -> int:
    """Run the ``hertzforge`` command line on `argv` (``sys.argv[1:]`` when
    None) and return the exit status.

    ``serve`` hands everything after it to the service's own parser, so
    the service's options are not repeated here.
    """
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
    # the packaged builds are tested with it: a bundle that leaves oneMKL out
    # still solves, through SuperLU, many times slower on a large model
    commands.add_parser("check-solver",
                        help="check that the sparse modal solve uses the solver this "
                             "machine should: Accelerate on a Mac, PARDISO on x86-64 "
                             "Windows and Linux; exit 1 when it does not")
    # a run folder into a project file, as a process of its own, so a
    # large run converts without holding up anything else
    convert = commands.add_parser(
        "convert-run",
        help="convert a run folder into a .hforge project file "
             "(<run name>.hforge inside the folder unless -o names another)")
    convert.add_argument("folder", help="the run folder, or the run.json inside it")
    convert.add_argument("-o", "--output", help="the project file to write")
    args, rest = parser.parse_known_args(argv)
    if args.command == "convert-run":
        if rest:
            parser.error(f"unrecognized arguments: {' '.join(rest)}")
        from hertzforge.io.run_convert import convert as convert_run
        from hertzforge.io.run_folder import RunFolderError

        try:
            done = convert_run(args.folder, args.output)
        except (RunFolderError, OSError) as error:
            print(f"hertzforge convert-run: {error}")
            return 1
        print(f"wrote {done.path}: {len(done.objects)} objects "
              f"({', '.join(done.objects)})")
        for lack in done.lacked:
            print(f"the run lacked: {lack}")
        return 0
    if args.command == "serve":
        from hertzforge.service.__main__ import main as serve

        return serve(rest)
    if args.command == "check-solver":
        from hertzforge.core.sparse_solve import check

        code, said = check()
        print(said)
        return code
    if rest:
        parser.error(f"unrecognized arguments: {' '.join(rest)}")
    parser.print_help()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
