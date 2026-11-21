"""Modal Test operations: a measurement's FRFs and coherence estimated again from its
frames (`Project.reprocess_modal_test`, `core.modal_test.reprocess`), with the estimator,
the windows, the block and the frames chosen. What it refuses, the core refuses, in its
own words."""

from __future__ import annotations

from typing import Any

from . import views as v
from .descriptors import Operation, Option, Param
from .parsing import parse_optional_picks
from .protocol import refused

FRAMES = Param('source', 'Measurement frames', 'enum', options_from='objects:time_history')
ESTIMATOR = Param('method', 'Estimator', 'enum', default='H1',
                  options=(Option('H1', 'H1'), Option('H2', 'H2'), Option('Hv', 'Hv')))
FORCE = Param('force', 'Force window', 'number', default=0.0,
              unit='of the block', min=0.0, max=1.0, step=0.01)
EXPONENTIAL = Param('exponential', 'Exponential window ends at', 'number', default=0.0,
                    unit='of its start', min=0.0, max=1.0, step=0.01)
BLOCK = Param('block', 'Block', 'integer', default=0, unit='samples', min=0, step=1024)
CAPTURES = Param('captures', 'Frames', 'string', default='')
SHAKER = Param('shaker_window', 'Shaker window', 'enum', default='',
               options=(Option('', 'None'), Option('hann', 'Hann'),
                        Option('flattop', 'Flat top'), Option('boxcar', 'Rectangular')))


def reprocess_modal_test(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.reprocess_modal_test`: the FRFs and coherence of one measurement
    estimated again from its frames, added beside them; a zero force width, a zero
    exponential end, a zero block and a blank list of frames each mean none chosen (no
    window, the whole frame, every frame)."""
    from hertzforge.core.modal_test import frf_tau

    name = params['source']
    history = project[name]
    count = max(history.capture_indices(), default=-1) + 1
    captures = parse_optional_picks(str(params['captures']), count, 'frame')
    extra: dict[str, Any] = {
        'force': float(params['force']) or None,
        'exponential': float(params['exponential']) or None,
        'block': int(params['block']) or None, 'captures': captures,
        'shaker_window': params['shaker_window'] or None}
    try:
        frf, coherence = project.reprocess_modal_test(name, params['method'], **extra)
    except ValueError as error:
        raise refused(str(error)) from None
    made = project[frf]
    rows = [v.row('Added', frf, None, 'info'), v.row('Added', coherence, None, 'info'),
            v.row('Records', float(made.num_records), '', 'info'),
            v.row('Frames', float(made.averaging.frames), '', 'info'),
            v.row('Block', float(made.averaging.frame_length), 'samples', 'info'),
            v.row('Window', made.averaging.window_text(), None, 'info')]
    tau = frf_tau(made)
    if tau is not None:
        rows.append(v.row('Exponential tau', tau, 's', 'info'))
    return [v.reading(f'{name}: {params["method"]} again', rows)]


OPERATIONS = (
    Operation(
        id='project.reprocess_modal_test', title='Estimate Again', workspace='modal',
        summary='One measurement\'s FRFs and coherence estimated again from its frames, '
                'with the estimator, windows, block and frames chosen, added beside them; '
                'the run\'s own FRFs are left as they are.',
        params=(FRAMES, ESTIMATOR, FORCE, EXPONENTIAL, BLOCK, CAPTURES, SHAKER),
        outputs=('reading',), run=reprocess_modal_test),
)
