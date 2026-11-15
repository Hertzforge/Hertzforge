"""The ``hertzforge`` command: ``hertzforge --version`` prints the version,
``hertzforge serve`` starts the analysis service, ``hertzforge check-solver``
says which sparse solver this installation factors with, ``hertzforge
convert-run FOLDER [-o OUTPUT]`` converts a run folder into a project file, and
``hertzforge export-animation PROJECT -o PAGE --geometry NAME ...`` writes a geometry's
animation as one page that plays offline (`hertzforge.animation`)."""

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
    export = commands.add_parser(
        "export-animation",
        help="write a geometry's animation, drawn by the app's own 3-D view with its "
             "controls, as one HTML page that plays offline (for an iframe in a presentation)")
    export.add_argument("project", help="the project file (.hforge)")
    export.add_argument("-o", "--output", required=True, help="the page to write (.html)")
    export.add_argument("--geometry", required=True, help="the geometry drawn")
    export.add_argument("--data", help="data played on it: measured motion, or modal "
                                       "coordinates through --shapes")
    export.add_argument("--shapes", help="the shape set: its modes alone, or the one --data's "
                                         "modal coordinates are of")
    export.add_argument("--mode", type=int, default=1,
                        help="the mode the page opens at, from 1, when there is no --data")
    export.add_argument("--scale", type=float, default=1.0,
                        help="the amplitude the page opens at, times the app's own (default 1)")
    export.add_argument("--theme", choices=("dark", "light"),
                        help="the app's dark or light look (default: the reader's own)")
    export.add_argument("--title", help="the page's title (default: the view's)")
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
    if args.command == "export-animation":
        if rest:
            parser.error(f"unrecognized arguments: {' '.join(rest)}")
        return _export_animation(args)
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


def _export_animation(args: argparse.Namespace) -> int:
    """`hertzforge export-animation`: the project opened and the animation's page written."""
    from hertzforge.animation import AnimationError, export_animation
    from hertzforge.project import Project

    try:
        project = Project.open(args.project)
        out = export_animation(project, args.output, args.geometry, args.data, args.shapes,
                               mode=args.mode, scale=args.scale, theme=args.theme,
                               title=args.title)
    except (AnimationError, OSError, ValueError) as error:
        print(f"hertzforge export-animation: {error}")
        return 1
    print(f"wrote {out} ({out.stat().st_size / 1e6:.1f} MB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
