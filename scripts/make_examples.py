"""Write the example projects hertzforge.com offers for download.

Nine `.hforge` files, each built by this package alone: the free plate
of `hertzforge.demo` with its 30 lowest normal modes (six of them
rigid), the Frame-Wing as `frame_wing.project`
builds it (the frame, each wing alone and the frame with each wing,
every one solved to 1500 Hz, a 170 MB file), the BARC of bricks as
`barc.solid_project` builds it (the band-average study's BARC: solved to
10 kHz, with its drives, accelerometers, force CPSD and the modes at
them), the BARC's removable
component on a fixture plate as `rc_plate.project` builds it (bricks,
solved to 10 kHz, with its test geometry, channels and modes at the test
DOFs, the system the band-average control study drives), and the plate's
random and modal controller runs read from `testdata/plate`
(`scripts/fetch_testdata.sh`). They were the analysis service's built-in
datasets until 2026-11-05; now a person downloads the ones they want and
opens them like any other project. The BARC and the RC plate were added on
2026-11-10.

Three more carry the band-average study's workflow through (2026-11-15):
the BARC with the random specification made from it
(`barc-random-specification`), the RC plate controlled to that
specification on the MIMO Random engine (`rc-plate-random-test`), and a
burst random modal test of the RC plate with an accelerometer at every
drive (`rc-plate-modal-test`). No other program made any of their numbers.

    python scripts/make_examples.py OUT_DIR [--only NAME ...]

The frame-wing project takes a minute or two to build and solve and
needs the CAD kernel (OCP); the RC plate takes a few minutes and about
6 GB, solved once for the three that use it; the random test runs the
study's simulator (built with `cargo build --release -p
hertzforge-mimo-random --example mimo_study`) for a few minutes; the
others take seconds.
"""

from __future__ import annotations

import argparse
import functools
import sys
import tempfile
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
PLATE = ROOT / 'testdata' / 'plate'
STUDY = ROOT / 'examples' / 'rc_plate_band_average'
#: the band-average study's run the random test demo plays: band-average,
#: 70 % drive reduction, FRF updates, on the plant that softens 8 % at full
#: level with white response noise (`run_study.py`)
RANDOM_RUN = 'nl8n_ba_r70_tfu'
RANDOM_TITLE = 'BA r70 + FRF Update'
G = 9.80665                # m/s^2
LBF = 4.4482216152605      # N
#: the modal test: burst random at the twelve drives at once, uncorrelated,
#: over the specification's band, each at MODAL_LEVEL lbf RMS while the burst
#: is on, half of each one-second frame, 30 frames
MODAL_BAND = (20.0, 4000.0)
MODAL_LEVEL = 10.0
MODAL_RATE = 16384.0
MODAL_FRAME = 16384
MODAL_BURSTS = 30
MODAL_SEED = 2026
#: white noise on every accelerometer, g^2/Hz, the study's (`softening.json`)
MODAL_NOISE = 1e-5
#: where the fit looks for poles, Hz: nothing elastic below the 418 Hz mode,
#: and nothing driven above the burst's 4000 Hz
FIT_BAND = (300.0, 3990.0)
FIT_LIMIT = 10
#: refined twice after the confirms: the poles of modes whose bands overlap are
#: searched together, and once left the 1125 Hz mode's damping an eighth low
FIT_REFINE = 2


def _model(title: str, build: Callable[[], Any]) -> Any:
    import hertzforge as hz

    model = build()
    project = hz.Project(title)
    project.add('Geometry', model.geometry())
    project.add('Modes', model.eigensolution(num_modes=30))
    return project


def free_plate() -> Any:
    from hertzforge.demo import plate
    return _model('Free Plate', plate.build)


def frame_wing() -> Any:
    from hertzforge.demo import frame_wing
    return frame_wing.project(solved=True)


def barc() -> Any:
    from hertzforge.demo import barc
    return barc.solid_project(solved=True)


@functools.cache
def _rc_plate_solved() -> Any:
    from hertzforge.demo import rc_plate
    return rc_plate.project(solved=True)


def rc_plate() -> Any:
    return _rc_plate_solved()


@functools.cache
def _barc_specified() -> Any:
    from hertzforge.demo import barc

    project = barc.solid_project(solved=True)
    barc.specification(project)
    return project


def _rc_plate_start(title: str, table: Any) -> Any:
    """A project holding the RC plate, its modes, its test geometry, `table`
    and the modes at the test DOFs, with the test objects as the Basis."""
    import hertzforge as hz

    rc = _rc_plate_solved()
    project = hz.Project(title)
    for name in ('RC Plate', 'RC Plate Modes', 'RC Plate Test Geometry',
                 'RC Plate Modes @ Test DOFs'):
        project.add(name, rc[name])
    project.add('RC Plate Channels', table)
    project.link('RC Plate', 'RC Plate Modes')
    project.set_basis('RC Plate Test Geometry', 'RC Plate Channels',
                      'RC Plate Modes @ Test DOFs')
    return project


def barc_random_specification() -> Any:
    """The BARC example with the band-average study's specification made from
    it (`barc.specification`). The modal response CPSD between the steps is
    left out: 59 modes against 59 at every line, it would add 230 MB."""
    project = _barc_specified()
    project.remove('BARC Force CPSD Modal Responses')
    project.name = 'BARC Random Specification'
    return project


def _random_history(stream: Path, controls: list[str], drives: list[str],
                    keep: int, rate: float) -> Any:
    """The last `keep` samples of a study stream (`run_study.read_stream`: the
    controls in g, then the drives in lbf) as a time history in SI, its
    values the stream's single precision ones."""
    import numpy as np

    from hertzforge.core.data import TimeHistory

    width = len(controls) + len(drives)
    samples = np.memmap(stream, dtype='<f4', mode='r').reshape(-1, width)
    values = np.array(samples[-keep:].T, dtype=np.float32)
    values[:len(controls)] *= np.float32(G)
    values[len(controls):] *= np.float32(LBF)
    history = TimeHistory(
        abscissa=np.arange(keep) / rate, ordinate=values,
        response_dof=controls + drives,
        ordinate_dim=['acceleration'] * len(controls) + ['force'] * len(drives),
        ordinate_unit=['m/s^2'] * len(controls) + ['N'] * len(drives),
        comment=['control'] * len(controls) + ['drive'] * len(drives))
    history.roles = {**{(d, 'acceleration'): 'response' for d in controls},
                     **{(d, 'force'): 'reference' for d in drives}}
    history.source = 'SIM'
    return history


def rc_plate_random_test() -> Any:
    """The band-average study's random test: the RC plate controlled to the
    BARC's specification at the sixteen accelerometers through the twelve
    corner drives, run on Hertzforge's MIMO Random engine with the band-average
    law, the plant simulated from the RC plate's modes (`run_study.py`). Kept:
    the last 60 s at full level, the 117 Hann frames the study scores, their
    PSDs and multiple coherence against the specification, and the random
    report. Needs the study driver built:

        cargo build --release -p hertzforge-mimo-random --example mimo_study
    """
    sys.path.insert(0, str(ROOT))
    from examples.rc_plate_band_average import make_rc_plate, run_study

    from hertzforge.core.averaging import Averaging
    from hertzforge.units import SYSTEMS

    if not Path(run_study.simulator()).is_file():
        raise SystemExit(f'make_examples: {run_study.simulator()} is missing; build it with '
                         'cargo build --release -p hertzforge-mimo-random --example mimo_study')
    rc = _rc_plate_solved()
    make_rc_plate.export(rc)
    specified = _barc_specified()
    specified.export('BARC Response Specification', STUDY / 'barc_response_spec.npz',
                     unit_system=SYSTEMS['in-slinch-lbf-s (g)'],
                     format='rattlesnake_specification')
    plant, _spec, drive_rows, control_rows = run_study.load_models()
    names = [str(d) for d in plant['channel_dof']]
    controls = [names[k] for k in control_rows]
    drives = [names[k] for k in drive_rows]
    span = (run_study.SCORED_FRAMES + 1) * run_study.HOP
    with tempfile.TemporaryDirectory() as out:
        run_study.main([out, RANDOM_RUN, '--keep'])
        history = _random_history(Path(out) / f'{RANDOM_RUN}.f32', controls, drives,
                                  span, run_study.SAMPLE_RATE)
    history.averaging = Averaging(frame_length=run_study.FRAME, frames=run_study.SCORED_FRAMES,
                                  overlap=0.5, window='hann')
    project = _rc_plate_start('RC Plate Random Test', rc['RC Plate Channels'])
    project.add('BARC Response Specification', specified['BARC Response Specification'])
    run = project.add(f'{RANDOM_TITLE} Time History', history)
    project.link('RC Plate Test Geometry', 'RC Plate Channels', 'RC Plate Modes @ Test DOFs',
                 'BARC Response Specification', run)
    project.compute_psds(run)
    project.compute_multiple_coherence(run)
    project.generate_report('random', name=f'{RANDOM_TITLE} Report')
    return project


def rc_plate_modal_test() -> Any:
    """A burst random modal test of the RC plate, simulated in Hertzforge from
    its modes, with an accelerometer at every drive so the fitted shapes come
    out scaled to unit modal mass: Generate Signal (burst random at the drives),
    Compute Modal Responses and Expand through the modes at the test DOFs,
    white noise on every accelerometer (`MODAL_NOISE`), then H1 FRFs, multiple
    coherence, the automatic fit with its poles refined, and the fit against the
    FE modes."""
    import numpy as np

    from hertzforge.core.averaging import Averaging
    from hertzforge.demo import rc_plate

    table = rc_plate.with_drive_points(_rc_plate_solved()['RC Plate Channels'])
    project = _rc_plate_start('RC Plate Modal Test', table)
    shapes = 'RC Plate Modes @ Test DOFs'
    forces = project.generate_signal(
        'RC Plate Channels', 'burst', sample_rate=MODAL_RATE, frame_length=MODAL_FRAME,
        bursts=MODAL_BURSTS, burst=0.5, band=MODAL_BAND, correlated=False,
        level=MODAL_LEVEL ** 2 / (MODAL_BAND[1] - MODAL_BAND[0]), seed=MODAL_SEED,
        name='Burst Random Forces')
    modal = project.compute_modal_responses(forces, shapes)
    responses = project.expand(modal, shapes, name='Responses')
    project.remove(modal)
    noisy = project[responses]
    rng = np.random.default_rng(MODAL_SEED)
    sigma = np.sqrt(MODAL_NOISE * MODAL_RATE / 2) * G
    noisy.ordinate = noisy.ordinate + rng.normal(0.0, sigma, noisy.ordinate.shape)
    history = project.merge(forces, responses, name='Modal Test Time History')
    # one rectangular frame per burst, as the forces were generated to be read. The
    # samples are rounded to single precision, as an acquisition's are: the file's
    # compression then takes them to near half, and the FRFs over the band move by a
    # few parts in a million
    measured = project[history]
    measured.averaging = Averaging(frame_length=MODAL_FRAME, frames=MODAL_BURSTS,
                                   window='boxcar')
    measured.ordinate = np.asarray(measured.ordinate, dtype=np.float32).astype(np.float64)
    frfs = project.compute_frfs(history, 'H1')
    project.compute_multiple_coherence(history)
    modes = project.fit_modes(frfs, bounds=FIT_BAND, limit=FIT_LIMIT, refine=FIT_REFINE,
                              name='Modal Test Modes')
    project.comparison_mac(shapes, modes)
    return project


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
    'free-plate': free_plate,
    'frame-wing': frame_wing,
    'barc': barc,
    'rc-plate': rc_plate,
    'barc-random-specification': barc_random_specification,
    'rc-plate-random-test': rc_plate_random_test,
    'rc-plate-modal-test': rc_plate_modal_test,
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
