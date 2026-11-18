"""A time history's analysis settings, set from the shell: its averaging,
its filter and its truncation span, which Compute Spectra and Compute
PSDs, Apply Filter and Apply Truncation then read. Each is an
operation whose params open on the history's own settings (`setting`)
and whose result draws the setting on one record, and reads back what was stored.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .descriptors import Operation, Option, Param
from .protocol import refused
from .waterfalls import axes

HISTORY = Param('source', 'Time history', 'enum', options_from='objects:time_history')
#: the channel the setting is drawn on; not a setting itself
SHOWN = Param('record', 'Drawn on record', 'integer', default=1, min=1, step=1)
#: more frames than this are shaded as one span: a frame a few pixels wide says nothing
MOST_FRAMES_SHADED = 1000

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

#: the dragged span of every frame, seconds on the record's clock
SPAN_START = Param('start', 'Span from', 'number', default=0.0, unit='s', step=0.1)
SPAN_STOP = Param('stop', 'Span to', 'number', default=1.0, unit='s', step=0.1)
#: where the span ended before the drag, which tells a left edge dragged from a slide;
#: negative when not known
SPAN_WAS = Param('was_stop', 'Span ended at', 'number', default=-1.0, unit='s', step=0.1)

PASS = Param('kind', 'Filter', 'enum', default='low-pass', setting='filtering.kind',
             options=(Option('low-pass', 'Low-pass'), Option('high-pass', 'High-pass'),
                      Option('band-pass', 'Band-pass')))
LOW = Param('low', 'Low edge', 'number', default=10.0, unit='Hz', min=0.0, step=10.0,
            setting='filtering.low')
HIGH = Param('high', 'High edge', 'number', default=1000.0, unit='Hz', min=0.0,
             step=10.0, setting='filtering.high')
ORDER = Param('order', 'Order', 'integer', default=4, min=1, max=12, step=1,
              setting='filtering.order')
FILTER_KEEP = Param('keep', 'Add the filtered record (else the preview)', 'boolean',
                    default=False)

CUT_START = Param('start', 'Start', 'number', default=0.0, unit='s', step=0.1,
                  setting='truncation.start')
CUT_STOP = Param('stop', 'Stop', 'number', default=1.0, unit='s', step=0.1,
                 setting='truncation.stop')


def settings_of(obj: Any) -> dict[str, dict[str, Any]]:
    """The settings an object carries, as the project summary lists them: a
    time history's {'averaging': {...}, 'filtering': {...}, 'truncation':
    {...}}, each present only when set, a geometry's {'rigid': {...}}, and a
    report's {'features': {key:
    on}} for every report feature and its identification fields
    {'identity': {key: text}}. What a `setting` param opens on."""
    from hertzforge.core.report import Report

    if isinstance(obj, Report):
        from hertzforge.report.features import FEATURES
        from hertzforge.report.identity import FIELDS, value

        held = set(obj.features)
        return {'features': {key: key in held for key in FEATURES},
                'identity': {field.key: _text(value(obj, field.key)) for field in FIELDS}}
    out: dict[str, dict[str, Any]] = {}
    if hasattr(obj, 'suggest_mass_properties') and obj.num_nodes:
        out['rigid'] = _rigid_settings(obj.mass_properties or obj.suggest_mass_properties())
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


def _rigid_settings(properties: Any) -> dict[str, Any]:
    """A geometry's reference point, mass and inertia, as the Rigid Modes form opens on."""
    # a centroid's last digits are rounding, and the form would show them
    x, y, z = (float(f'{value:.12g}') for value in properties.point)
    out: dict[str, Any] = {'x': x, 'y': y, 'z': z, 'normalize': properties.mass is not None}
    if properties.mass is not None:
        out['mass'] = properties.mass
        out.update(zip(('ixx', 'iyy', 'izz', 'ixy', 'ixz', 'iyz'), properties.inertia))
    return out


def _text(held: Any) -> str:
    """An identification field as the form shows it: a list of people
    comma-separated."""
    return ', '.join(map(str, held)) if isinstance(held, list) else str(held or '')


def _rate(history: Any) -> float:
    try:
        return float(history.sample_rate)
    except ValueError as error:
        raise refused(str(error)) from None


def _shown(history: Any, name: str, params: dict[str, Any]) -> int:
    number = int(params.get('record', 1))
    if not 1 <= number <= history.num_records:
        raise refused(f'{name} has {history.num_records} records; there is no record {number}')
    return number - 1


def _trace(history: Any, name: str, record: int, regions: list[dict],
           filtered: Any = None) -> dict:
    """One record of a history, peak-decimated, with `regions` shaded
    behind it. With `filtered` (the record through a filter) that is
    drawn over the record, the unfiltered record thin behind it."""
    import hertzforge as hz
    from hertzforge.decimate import peak_decimate
    from hertzforge.drawing.viz.waterfall import POINT_BUDGET, waterfall_arrays

    arrays = waterfall_arrays(history, [record], unit_system=hz.SI, exponents=False,
                              component='real')
    x, y = arrays['curves'][0]
    label = history.record_label(record)
    if filtered is None:
        lines = [v.series(label, 'measured', x, y)]
    else:
        fx, fy = peak_decimate(np.asarray(history.abscissa, dtype=float), filtered,
                               POINT_BUDGET)
        lines = [v.series(f'{label}, filtered', 'measured', fx, fy),
                 v.series(f'{label}, as recorded', 'drive', x, y)]
    return v.spectrum(f'{name}, {label}', *axes(history, record, 'real', arrays), lines,
                      regions=regions)


#: how many points a frame's window is drawn with: it is smooth, so this only has to be
#: finer than the eye at the width of a frame
WINDOW_POINTS = 33


def span_region(name: str, averaging: Any, rate: float, origin: float,
                samples: int | None = None) -> dict:
    """The span of every frame, dragged on the plot: its middle slides the
    frames along the record and an edge adds or removes frames
    (`project.drag_averaging`, which keeps whole frames). The drag carries the
    frame, overlap, window and detrend drawn, so frames typed into the form and
    not yet stored are the ones dragged."""
    bounds = averaging.frame_bounds(rate, origin)
    held = {'source': name, 'frame_length': int(averaging.frame_length),
            'overlap': float(averaging.overlap), 'window': averaging.window,
            'detrend': averaging.detrend, 'was_stop': float(bounds[-1][1])}
    snap = None if samples is None else {
        'frame': averaging.frame_length / rate, 'hop': averaging.hop / rate,
        'low': origin, 'high': origin + samples / rate}
    return v.region(bounds[0][0], bounds[-1][1], 'span',
                    'Drag to slide the frames; drag an edge for more or fewer',
                    v.dragging('project.drag_averaging', held, 'start', 'stop'), snap=snap)


def frame_regions(averaging: Any, rate: float, origin: float) -> list[dict]:
    """The frames spectra are averaged from, each a region carrying its
    window's weights and its row among the frames it overlaps
    (`Averaging.levels`); one span over all of them when there are too
    many to see."""
    from hertzforge.core.averaging import window_shape

    bounds = averaging.frame_bounds(rate, origin)
    if len(bounds) > MOST_FRAMES_SHADED:
        return [v.region(bounds[0][0], bounds[-1][1], 'frame',
                         f'{len(bounds)} frames, {bounds[0][0]:g} to {bounds[-1][1]:g} s')]
    levels, rows = averaging.levels(rate)
    n = int(averaging.frame_length)
    shape = np.asarray(window_shape(averaging.window, n, averaging.window_parameter),
                       dtype=float)
    at = np.linspace(0, n - 1, min(WINDOW_POINTS, n)).astype(int)
    peak = float(shape.max())
    # as the shading weighs the record: a flat top's small negative lobes draw as none
    weights = [round(float(w), 4)
               for w in np.clip(shape[at] / peak if peak > 0 else shape[at], 0.0, 1.0)]
    return [v.region(a, b, 'frame', f'Frame {k + 1}, {a:g} to {b:g} s',
                     window=weights, level=levels[k], rows=rows)
            for k, (a, b) in enumerate(bounds)]


def averaging_regions(name: str, history: Any, averaging: Any = None) -> list[dict]:
    """What draws an averaging on a history's plot: its frames, then the
    span that drags them. `averaging` is the history's own when None."""
    averaging = history.averaging if averaging is None else averaging
    rate = _rate(history)
    origin = float(history.abscissa[0])
    return frame_regions(averaging, rate, origin) + [
        span_region(name, averaging, rate, origin, len(history.abscissa))]


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
    record = _shown(history, name, params)
    history.averaging = averaging
    project.record_setting(name, 'averaging', averaging)
    return [_trace(history, name, record, averaging_regions(name, history)),
            _averaging_reading(name, history)]


def spanned(averaging: Any, rate: float, origin: float, samples: int,
            start: float, stop: float, was_stop: float | None = None) -> Any:
    """`averaging` moved to the span `start` to `stop`, seconds on the record's
    clock, with as many whole frames as the span holds, at least one. It starts
    where the span does, unless the span still ends at `was_stop`, where it
    ended before the drag: then its left edge was the one dragged, so the end
    stays and whole frames are added or removed at the start. A span slid past
    the end of the record is slid back until its frames fit, and one dragged
    longer than the record keeps the frames that fit."""
    from dataclasses import replace

    n, hop = int(averaging.frame_length), int(averaging.hop)
    first = max(round((float(start) - origin) * rate), 0)
    last = round((float(stop) - origin) * rate)
    length = last - first
    # a slid span comes back a sample short or long of its frames; that is not a frame lost
    frames = 1 + max(length - n + min(2, hop - 1), 0) // hop
    if was_stop is not None and abs(float(stop) - float(was_stop)) * rate < 1.5:
        # the left edge dragged: the frames end where they did
        frames = max(min(frames, 1 + max(last - n, 0) // hop), 1)
        first = max(last - n - (frames - 1) * hop, 0)
    moved = replace(averaging, start=first / rate, frames=frames, start_set=True)
    if not moved.fits(samples, rate):
        first = min(first, max(samples - moved.span, 0))
        moved = replace(moved, start=first / rate)
    if not moved.fits(samples, rate):
        moved = replace(moved, frames=max(moved.most_frames(samples, rate), 1))
    return moved


def drag_averaging(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.drag_averaging`: a history's frames after their span was
    dragged on its plot to `start` and `stop`, seconds on the record's clock
    (`spanned`), with the frame length, overlap, window and detrend the plot drew.
    A span that still ends at `was_stop` had its left edge dragged."""
    from hertzforge.core.averaging import Averaging

    name = params['source']
    history = project[name]
    try:
        drawn = Averaging(params['frame_length'], params['overlap'], params['window'], 1,
                          0.0, params['detrend'], start_set=True)
    except ValueError as error:
        raise refused(str(error)) from None
    rate = _rate(history)
    moved = spanned(drawn, rate, float(history.abscissa[0]),
                    len(history.abscissa), params['start'], params['stop'],
                    params['was_stop'] if params['was_stop'] >= 0 else None)
    if not moved.fits(len(history.abscissa), rate):
        raise refused(f'{name} is shorter than one frame of {moved.frame_length} samples')
    history.averaging = moved
    project.record_setting(name, 'averaging', moved)
    return [_averaging_reading(name, history)]


def detect_averaging(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.detect_averaging`: the steady stretch of a history found
    and as many frames as fit in it (`TimeHistory.suggest_averaging`),
    keeping the frame, window and overlap it already has."""
    name = params['source']
    history = project[name]
    record = _shown(history, name, params)
    try:
        history.averaging = history.suggest_averaging()
    except ValueError as error:
        raise refused(str(error)) from None
    project.record_setting(name, 'averaging', history.averaging)
    return [_trace(history, name, record, averaging_regions(name, history)),
            _averaging_reading(name, history)]


def set_filter(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_filter`: a time history's filter, set and applied in
    one place (Brandon, 2026-11-17: Set Filter and Apply Filter were two
    actions for one thing). A low-pass takes the high edge, a high-pass the
    low edge and a band-pass both. The settings are stored on the history
    either way. Without `keep` the picked record is drawn as recorded and
    as the filter would leave it; with it the filtered history is added
    (`Project.filter_data`), every channel at zero phase."""
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
    record = _shown(history, name, params)
    try:
        filtering = Filtering(order=params['order'], **edges)
    except ValueError as error:
        raise refused(str(error)) from None
    history.filtering = filtering
    project.record_setting(name, 'filtering', filtering)
    if params.get('keep'):
        from .ops_project import _derived

        return _derived(project, lambda: project.filter_data(name))
    # the preview is what Apply will make of this record: the same design, zero phase
    from scipy.signal import sosfiltfilt

    from hertzforge.core.filters import design

    preview = sosfiltfilt(design(filtering, _rate(history)),
                          np.real(np.asarray(history.ordinate[record])))
    return [_trace(history, name, record, [], filtered=preview),
            v.reading(f'Filter of {name}', [
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
    record = _shown(history, name, params)
    try:
        history.truncation = Truncation(start, stop)
    except ValueError as error:
        raise refused(str(error)) from None
    project.record_setting(name, 'truncation', history.truncation)
    cut = [v.region(a, b, 'cut', label) for a, b, label in (
        (first, start, f'Cut, {first:g} to {start:g} s'),
        (stop, last, f'Cut, {stop:g} to {last:g} s')) if b > a]
    # the kept span, its edges or the whole dragged
    cut.append(v.region(start, stop, 'span', 'Drag an edge, or the middle to slide it',
                        v.dragging('project.set_truncation',
                                   {'source': name, 'record': record + 1}, 'start', 'stop')))
    return [_trace(history, name, record, cut),
            v.reading(f'Truncation of {name}', [
        v.row('From', start, 's', 'info'), v.row('To', stop, 's', 'info'),
        v.row('Kept', stop - start, 's', 'info')])]


OPERATIONS = (
    Operation(
        id='project.set_averaging', title='Averaging', workspace='signals', everywhere=True,
        summary='The frames a time history\'s spectra are averaged from: their '
                'length, overlap, window, number and start.',
        params=(HISTORY, FRAME, OVERLAP, WINDOW, FRAMES, START, DETREND, SHOWN),
        outputs=('spectrum', 'reading'), run=set_averaging, edits=True),
    Operation(
        id='project.drag_averaging', title='Drag Averaging', workspace='signals', everywhere=True,
        summary='A time history\'s frames after their span was dragged on its plot: '
                'slid along the record, or made more or fewer.',
        params=(HISTORY, FRAME, OVERLAP, WINDOW, DETREND, SPAN_START, SPAN_STOP, SPAN_WAS),
        outputs=('reading',), run=drag_averaging,
        edits=True),
    Operation(
        id='project.detect_averaging', title='Detect Averaging', workspace='signals', everywhere=True,
        summary='The steady stretch of a time history found, and as many frames '
                'as fit in it, keeping its frame length, window and overlap.',
        params=(HISTORY, SHOWN), outputs=('spectrum', 'reading'), run=detect_averaging, edits=True),
    Operation(
        id='project.set_filter', title='Filter', workspace='signals', everywhere=True,
        summary='A time history filtered to a pass band at zero phase: Preview draws a '
                'record as the filter would leave it, and Apply adds the filtered history.',
        params=(HISTORY, PASS, LOW, HIGH, ORDER, SHOWN, FILTER_KEEP),
        outputs=('spectrum', 'reading', 'waterfall'), run=set_filter, edits=True,
        commits='keep'),
    Operation(
        id='project.set_truncation', title='Truncation', workspace='signals', everywhere=True,
        summary='The span of a time history Apply Truncation keeps.',
        params=(HISTORY, CUT_START, CUT_STOP, SHOWN), outputs=('spectrum', 'reading'),
        run=set_truncation, edits=True),
)
