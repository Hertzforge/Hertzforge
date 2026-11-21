"""The ``hertzforge`` command: ``hertzforge --version`` prints the version,
``hertzforge serve`` starts the analysis service, ``hertzforge check-solver``
says which sparse solver this installation factors with, ``hertzforge
convert-run FOLDER [-o OUTPUT]`` converts a run folder into a project file,
``hertzforge export-animation PROJECT -o PAGE --geometry NAME ...`` writes a geometry's
animation as one page that plays offline (`hertzforge.animation`), and ``hertzforge record
PROJECT -o VIDEO --geometry NAME ...`` records it to an MP4 with no window open
(`hertzforge.record`)."""

import argparse
from collections.abc import Callable

from hertzforge import __version__


def main(argv: list[str] | None = None) -> int:
    """Run the ``hertzforge`` command line on `argv` (``sys.argv[1:]`` when
    None) and return the exit status.

    ``serve`` hands everything after it to the service's own parser, so
    the service's options are not repeated here. With no command it opens
    the interface in a browser (`_open_app`).
    """
    parser = argparse.ArgumentParser(
        prog="hertzforge",
        description="Hertzforge: data acquisition and structural dynamics in one toolset. "
                    "With no command, it opens the interface in a browser.",
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
    export.add_argument("--opacity", type=float, default=1.0,
                        help="how solid the surface opens, above 0 to 1 (default 1, solid)")
    export.add_argument("--view", default="iso",
                        help="the view the page opens looking from: iso (the geometry's own), "
                             "front, back, left, right, top or bottom (default iso)")
    export.add_argument("--eye", help="X,Y,Z: the direction from the model to the camera the "
                                      "page opens with, in place of --view")
    export.add_argument("--up", help="X,Y,Z: which way is up with --eye (default 0,0,1)")
    export.add_argument("--theme", choices=("dark", "light"),
                        help="the app's dark or light look (default: the reader's own)")
    export.add_argument("--title", help="the page's title (default: the view's)")
    record = commands.add_parser(
        "record",
        help="record a geometry's animation, as the app's 3-D view plays it, to an MP4 "
             "with no window open (needs Chrome and ffmpeg)")
    record.add_argument("project", help="the project file (.hforge)")
    record.add_argument("-o", "--output", required=True, help="the video to write (.mp4)")
    record.add_argument("--geometry", required=True, help="the geometry drawn")
    record.add_argument("--data", help="data played on it: measured motion, or modal "
                                       "coordinates through --shapes")
    record.add_argument("--shapes", help="the shape set: its modes alone, or the one --data's "
                                         "modal coordinates are of")
    record.add_argument("--mode", type=int, default=1,
                        help="the mode played, from 1, when there is no --data (default 1)")
    record.add_argument("--size", default="1920x1080", help="WIDTHxHEIGHT in pixels "
                                                            "(default 1920x1080)")
    record.add_argument("--fps", type=float, default=30.0, help="frames per second (default 30)")
    record.add_argument("--speed", type=float,
                        help="the fraction of real time --data plays at (default 1: real time)")
    record.add_argument("--seconds", type=float,
                        help="the video's length in seconds, in place of --speed or --cycles")
    record.add_argument("--cycles", type=float, default=3.0,
                        help="cycles a mode swings (default 3)")
    record.add_argument("--scale", type=float, default=1.0,
                        help="times the app's own amplitude (default 1)")
    record.add_argument("--opacity", type=float, default=1.0,
                        help="how solid the surface is drawn, above 0 to 1 (default 1, solid)")
    record.add_argument("--view", default="iso",
                        help="iso (the geometry's own view), front, back, left, right, top or "
                             "bottom (default iso)")
    record.add_argument("--eye", help="X,Y,Z: the direction from the model to the camera, in "
                                      "place of --view")
    record.add_argument("--up", help="X,Y,Z: which way is up with --eye (default 0,0,1)")
    record.add_argument("--theme", default="dark", choices=("dark", "light"),
                        help="the app's dark or light look (default dark)")
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
    if args.command == "record":
        if rest:
            parser.error(f"unrecognized arguments: {' '.join(rest)}")
        return _record(args)
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
    return _open_app()


#: the analysis service's usual port; another is picked when it is taken,
#: as when the desktop app is running
APP_PORT = 8791

#: how long `hertzforge` with no command waits after its last browser tab
#: closes before it stops; a reload reconnects within half a second
CLOSED_S = 5.0


def _free(port: int) -> bool:
    """Whether `port` on the loopback address can be listened on."""
    import socket

    with socket.socket() as probe:
        try:
            probe.bind(("127.0.0.1", port))
        except OSError:
            return False
    return True


def _app_url(host: str, port: int) -> str:
    """The page that opens the interface on the service at `host`:`port`.
    The shell is told the service's address, since it assumes 8791
    otherwise, and that no control service runs beside it, so it leaves out
    the workspaces that need one."""
    return f"http://{host}:{port}/?analysis=ws://{host}:{port}/&control=none"


def _announce(url: str, opener: Callable[[str], bool]) -> None:
    """Say where the interface is and open it with `opener`
    (`webbrowser.open`); where that opens nothing, say to open it by hand."""
    print(f"Hertzforge is open at {url}\nIt stops when its browser tab is closed, "
          "or press Ctrl-C.", flush=True)
    if not opener(url):
        print("No browser could be opened here; open that address in one.", flush=True)


def _open_app() -> int:
    """`hertzforge` with no command: the analysis service started with the
    shell's built pages, and the interface opened in the default browser. It
    stops `CLOSED_S` seconds after its last browser tab closes; the project
    lives on the tab's connection, so nothing is kept for a tab opened later.
    Where no browser opens (over SSH, on a server) the address is printed to
    open by hand. The control service is not part of the Python package, so
    running a test needs the desktop app."""
    import asyncio
    import logging
    import sys
    import webbrowser

    from hertzforge.service.__main__ import _serve
    from hertzforge.service.server import shell_folder

    if shell_folder() is None:
        print("This install of hertzforge carries no interface. In a source checkout, build it "
              "with `npm --prefix ui run build`; otherwise get the desktop app from "
              "https://hertzforge.com. `hertzforge --help` lists the commands.")
        return 1

    print(f"Starting Hertzforge {__version__}...", flush=True)
    logging.basicConfig(level=logging.WARNING, stream=sys.stderr,
                        format="%(asctime)s %(name)s %(levelname)s %(message)s")
    port = APP_PORT if _free(APP_PORT) else 0
    try:
        asyncio.run(_serve("127.0.0.1", port, ready=lambda host, bound: _announce(
            _app_url(host, bound), webbrowser.open), quit_when_closed=CLOSED_S))
    except KeyboardInterrupt:
        return 0
    print("The browser tab was closed; Hertzforge stopped.", flush=True)
    return 0


def _xyz(text: str | None) -> tuple[float, ...] | None:
    """`X,Y,Z` as numbers; ValueError, in words, when it is not."""
    if text is None:
        return None
    try:
        return tuple(float(part) for part in text.split(","))
    except ValueError:
        raise ValueError(f"a direction is three numbers, as 1,0,0, not {text!r}") from None


def _export_animation(args: argparse.Namespace) -> int:
    """`hertzforge export-animation`: the project opened and the animation's page written."""
    from hertzforge.animation import AnimationError, export_animation
    from hertzforge.project import Project

    try:
        project = Project.open(args.project)
        out = export_animation(project, args.output, args.geometry, args.data, args.shapes,
                               mode=args.mode, scale=args.scale, opacity=args.opacity,
                               view=args.view, eye=_xyz(args.eye), up=_xyz(args.up),
                               theme=args.theme, title=args.title)
    except (AnimationError, OSError, ValueError) as error:
        print(f"hertzforge export-animation: {error}")
        return 1
    print(f"wrote {out} ({out.stat().st_size / 1e6:.1f} MB)")
    return 0


def _record(args: argparse.Namespace) -> int:
    """`hertzforge record`: the project opened, the animation drawn frame by frame in headless
    Chrome and written by ffmpeg."""
    import sys

    from hertzforge.project import Project
    from hertzforge.record import RecordError, Recording, record

    try:
        width, height = (int(part) for part in args.size.lower().split("x"))
    except ValueError:
        print(f"hertzforge record: the size is WIDTHxHEIGHT, as 1920x1080, not {args.size!r}")
        return 1
    try:
        eye, up = _xyz(args.eye), _xyz(args.up)
    except ValueError as error:
        print(f"hertzforge record: {error}")
        return 1
    recording = Recording(
        geometry=args.geometry, data=args.data, shapes=args.shapes, mode=args.mode,
        width=width, height=height, fps=args.fps, speed=args.speed, seconds=args.seconds,
        cycles=args.cycles, scale=args.scale, opacity=args.opacity, view=args.view,
        eye=eye, up=up, theme=args.theme)
    try:
        project = Project.open(args.project)
    except (OSError, ValueError) as error:
        print(f"hertzforge record: {error}")
        return 1

    def said(done: int, every: int) -> None:
        if done == every or done % 30 == 0:
            print(f"\rframe {done} of {every}", end="", file=sys.stderr, flush=True)

    try:
        out = record(project, recording, args.output, progress=said)
    except RecordError as error:
        print(f"\nhertzforge record: {error}")
        return 1
    print(f"\nwrote {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
