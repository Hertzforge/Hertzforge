"""A time history's analysis settings, set from the shell: its averaging,
its filter and its truncation span, which Compute Spectra and Compute
PSDs, Apply Filter and Apply Truncation then read. Visual Dynamics sets
them in its averaging, filter and truncate views; here each is an
operation whose params open on the history's own settings (`setting`)
and whose result reads back what was stored.
"""

from __future__ import annotations

from typing import Any

from . import views as v
from .descriptors import Operation, Option, Param
from .protocol import refused

HISTORY = Param('source', 'Time history', 'enum', options_from='objects:time_history')

FRAME = Param('frame_length', 'Frame', 'integer', default=4096, unit='samples',
              min=2, step=1024, setting='averaging.frame_length')
OVERLAP = Param('overlap', 'Overlap', 'number', default=0.5, min=0.0, max=0.95,
                step=0.05, setting='averaging.overlap')
WINDOW = Param('window', 'Window', 'enum', default='hann', setting='averaging.window',
               options=tuple(Option(name, label) for name, label in (
                   ('hann', 'Hann'), ('boxcar', 'Rectangular'), ('hamming', 'Hamming'),
                   ('blackman', 'Blackman'), ('blackmanharris', 'Blackman-Harris'),
                   ('flattop', 'Flat top'), ('tukey', 'Tukey'), ('kaiser', 'Kaiser'))))
FRAMES = Param('frames', 'Frames', 'integer', default=1, min=1, step=1,
               setting='averaging.frames')
START = Param('start', 'Start', 'number', default=0.0, unit='s', min=0.0, step=0.1,
              setting='averaging.start')
DETREND = Param('detrend', 'Detrend', 'enum', default='none', setting='averaging.detrend',
                options=(Option('none', 'None'), Option('mean', 'Mean'),
                         Option('linear', 'Linear')))

PASS = Param('kind', 'Filter', 'enum', default='low-pass', setting='filtering.kind',
             options=(Option('low-pass', 'Low-pass'), Option('high-pass', 'High-pass'),
                      Option('band-pass', 'Band-pass')))
LOW = Param('low', 'Low edge', 'number', default=10.0, unit='Hz', min=0.0, step=10.0,
            setting='filtering.low')
HIGH = Param('high', 'High edge', 'number', default=1000.0, unit='Hz', min=0.0,
             step=10.0, setting='filtering.high')
ORDER = Param('order', 'Order', 'integer', default=4, min=1, max=12, step=1,
              setting='filtering.order')

CUT_START = Param('start', 'Start', 'number', default=0.0, unit='s', step=0.1,
                  setting='truncation.start')
CUT_STOP = Param('stop', 'Stop', 'number', default=1.0, unit='s', step=0.1,
                 setting='truncation.stop')


def settings_of(obj: Any) -> dict[str, dict[str, Any]]:
    """The analysis settings an object carries, as the project summary
    lists them: {'averaging': {...}, 'filtering': {...}, 'truncation':
    {...}}, each present only when set. What a `setting` param opens on."""
    out: dict[str, dict[str, Any]] = {}
    averaging = getattr(obj, 'averaging', None)
    if averaging is not None:
        out['averaging'] = {
            'frame_length': averaging.frame_length, 'overlap': averaging.overlap,
            'window': averaging.window, 'frames': averaging.frames,
            'start': averaging.start, 'detrend': averaging.detrend}
    filtering = getattr(obj, 'filtering', None)
    if filtering is not None:
        out['filtering'] = {'kind': filtering.kind, 'order': filtering.order,
                            **{edge: getattr(filtering, edge) for edge in ('low', 'high')
                               if getattr(filtering, edge) is not None}}
    truncation = getattr(obj, 'truncation', None)
    if truncation is not None:
        out['truncation'] = {'start': truncation.start, 'stop': truncation.stop}
    return out


def _rate(history: Any) -> float:
    return float(history.sample_rate)


def _averaging_reading(name: str, history: Any) -> dict:
    averaging = history.averaging
    rate = _rate(history)
    samples = len(history.abscissa)
    most = averaging.most_frames(samples, rate)
    rows = [
        v.row('Frame', averaging.frame_length / rate, 's', 'info'),
        v.row('Line spacing', rate / averaging.frame_length, 'Hz', 'info'),
        v.row('Frames averaged', float(averaging.frames), '', 'info'),
        v.row('Frames that fit', float(most), '',
              'pass' if averaging.frames <= most else 'fail'),
        v.row('From', averaging.start, 's', 'info'),
        v.row('To', averaging.stop(rate), 's', 'info'),
        v.row('Window', averaging.window, None, 'info'),
    ]
    return v.reading(f'Averaging of {name}', rows)


def set_averaging(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_averaging`: the frames a history's spectra are averaged
    from. Refused when the record cannot hold them."""
    from hertzforge.core.averaging import Averaging

    name = params['source']
    history = project[name]
    try:
        averaging = Averaging(params['frame_length'], params['overlap'],
                              params['window'], params['frames'], params['start'],
                              params['detrend'], start_set=True)
    except ValueError as error:
        raise refused(str(error)) from None
    if not averaging.fits(len(history.abscissa), _rate(history)):
        most = averaging.most_frames(len(history.abscissa), _rate(history))
        raise refused(f'{name} holds {most} such frames from {averaging.start:g} s, '
                      f'not {averaging.frames}')
    history.averaging = averaging
    return [_averaging_reading(name, history)]


def detect_averaging(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.detect_averaging`: the steady stretch of a history found
    and as many frames as fit in it (`TimeHistory.suggest_averaging`),
    keeping the frame, window and overlap it already has."""
    name = params['source']
    history = project[name]
    try:
        history.averaging = history.suggest_averaging()
    except ValueError as error:
        raise refused(str(error)) from None
    return [_averaging_reading(name, history)]


def set_filter(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_filter`: the pass band Apply Filter filters a history
    to. A low-pass takes the high edge, a high-pass the low edge and a
    band-pass both."""
    from hertzforge.core.filters import Filtering

    name, kind = params['source'], params['kind']
    history = project[name]
    edges = {'low-pass': {'high': params['high']},
             'high-pass': {'low': params['low']},
             'band-pass': {'low': params['low'], 'high': params['high']}}[kind]
    nyquist = _rate(history) / 2
    for edge, hz in edges.items():
        if not hz < nyquist:
            raise refused(f'the {edge} edge ({hz:g} Hz) must be below half the '
                          f'sample rate ({nyquist:g} Hz)')
    try:
        history.filtering = Filtering(order=params['order'], **edges)
    except ValueError as error:
        raise refused(str(error)) from None
    return [v.reading(f'Filter of {name}', [
        v.row('Filter', history.filtering.describe(), None, 'info'),
        v.row('Order', float(history.filtering.order), '', 'info')])]


def set_truncation(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_truncation`: the span Apply Truncation keeps, on the
    record's own clock and inside it."""
    from hertzforge.core.truncate import Truncation

    name = params['source']
    history = project[name]
    first, last = float(history.abscissa[0]), float(history.abscissa[-1])
    start, stop = params['start'], params['stop']
    if start < first or stop > last:
        raise refused(f'{name} runs from {first:g} s to {last:g} s; the span '
                      f'{start:g} to {stop:g} s is not inside it')
    try:
        history.truncation = Truncation(start, stop)
    except ValueError as error:
        raise refused(str(error)) from None
    return [v.reading(f'Truncation of {name}', [
        v.row('From', start, 's', 'info'), v.row('To', stop, 's', 'info'),
        v.row('Kept', stop - start, 's', 'info')])]


OPERATIONS = (
    Operation(
        id='project.set_averaging', title='Averaging', workspace='analyze',
        accepts='project',
        summary='The frames a time history\'s spectra are averaged from: their '
                'length, overlap, window, number and start.',
        params=(HISTORY, FRAME, OVERLAP, WINDOW, FRAMES, START, DETREND),
        outputs=('reading',), run=set_averaging, edits=True),
    Operation(
        id='project.detect_averaging', title='Detect Averaging', workspace='analyze',
        accepts='project',
        summary='The steady stretch of a time history found, and as many frames '
                'as fit in it, keeping its frame length, window and overlap.',
        params=(HISTORY,), outputs=('reading',), run=detect_averaging, edits=True),
    Operation(
        id='project.set_filter', title='Filter', workspace='analyze',
        accepts='project',
        summary='The pass band Apply Filter filters a time history to, at zero '
                'phase.',
        params=(HISTORY, PASS, LOW, HIGH, ORDER), outputs=('reading',),
        run=set_filter, edits=True),
    Operation(
        id='project.set_truncation', title='Truncation', workspace='analyze',
        accepts='project',
        summary='The span of a time history Apply Truncation keeps.',
        params=(HISTORY, CUT_START, CUT_STOP), outputs=('reading',),
        run=set_truncation, edits=True),
)
