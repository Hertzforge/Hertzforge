"""Write the example projects hertzforge.com offers for download.

Five `.hforge` files, each built by this package alone: the three finite
element models of `hertzforge.demo` with their lowest normal modes (30,
six of them rigid; 12 for the frame-wing, whose 393,510 DOFs make each
mode 3 MB), and the plate's random and modal controller runs read from
`testdata/plate` (`scripts/fetch_testdata.sh`). They were the analysis
service's built-in datasets until 2026-11-05; now a person downloads the
ones they want and opens them like any other project.

    python scripts/make_examples.py OUT_DIR [--only NAME ...]

The frame-wing model takes about 30 s to build and solve and needs the
CAD kernel (OCP); the others take seconds.
"""

from __future__ import annotations

import argparse
import sys
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
PLATE = ROOT / 'testdata' / 'plate'


def _model(title: str, build: Callable[[], Any], modes: int = 30) -> Any:
    import hertzforge as hz

    model = build()
    project = hz.Project(title)
    project.add('Geometry', model.geometry())
    project.add('Modes', model.eigensolution(num_modes=modes))
    return project


def quadcopter() -> Any:
    from hertzforge.demo import drone
    return _model('Quadcopter', drone.build)


def free_plate() -> Any:
    from hertzforge.demo import plate
    return _model('Free Plate', plate.build)


def frame_wing() -> Any:
    from hertzforge.demo import frame_wing
    return _model('Frame With Thick Wing', lambda: frame_wing.build('thick wing'),
                  modes=12)


def plate_random_run() -> Any:
    import hertzforge as hz

    project = hz.random_vibration_run(PLATE / 'random.nc4')
    project.name = 'Plate Random Run'
    return project


def plate_modal_run() -> Any:
    import hertzforge as hz

    project = hz.Project('Plate Modal Run')
    project.import_file(str(PLATE / 'modal_spectra.nc4'))
    survey = hz.import_file(str(PLATE / 'test_geometry.npz'))
    survey.define_units('m')
    project.add('Geometry', survey)
    return project


EXAMPLES: dict[str, Callable[[], Any]] = {
    'quadcopter': quadcopter,
    'free-plate': free_plate,
    'frame-wing': frame_wing,
    'plate-random-run': plate_random_run,
    'plate-modal-run': plate_modal_run,
}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('out', type=Path, help='the folder the files are written to')
    parser.add_argument('--only', nargs='+', choices=sorted(EXAMPLES), metavar='NAME',
                        help='write these examples only')
    args = parser.parse_args(argv)
    names = args.only or list(EXAMPLES)
    if any(n.startswith('plate-') for n in names) and not (PLATE / 'random.nc4').is_file():
        print(f'make_examples: {PLATE} is missing; run scripts/fetch_testdata.sh first',
              file=sys.stderr)
        return 1
    args.out.mkdir(parents=True, exist_ok=True)
    for name in names:
        started = time.perf_counter()
        path = args.out / f'{name}.hforge'
        EXAMPLES[name]().save(path, compress=True)
        print(f'{path}  {path.stat().st_size / 2**20:.1f} MB  '
              f'{time.perf_counter() - started:.1f} s')
    return 0


if __name__ == '__main__':
    sys.exit(main())
