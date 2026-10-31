"""Operations on a random vibration run: a channel's PSD against its
specification, and its error in dB.

The judgment is `visualdynamics.core.compliance`'s, called as the
application calls it: `compare` for the channel's numbers, `judge` for its
cells, `readouts` and `verdict` for the states. Nothing here recomputes a
level; the per-cell dB is the ratio of the two powers `judge` returns for
each cell, and a limit's dB is its power over the target's on that cell.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .datasets import Channel, RandomRun, quantity_of
from .descriptors import Operation, Param
from .protocol import refused

#: limit curve -> the line role the viz kit styles it with
ROLES = {'warning_lower': 'tolerance', 'warning_upper': 'tolerance',
         'abort_lower': 'abort', 'abort_upper': 'abort'}
NAMES = {'warning_lower': 'Warning, lower', 'warning_upper': 'Warning, upper',
         'abort_lower': 'Abort, lower', 'abort_upper': 'Abort, upper'}

CHANNEL = Param('channel', 'Channel', 'enum', options_from='channels')
TOLERANCE = Param('tolerance_db', 'Tolerance (if the specification has no '
                  'limits)', 'number', default=3.0, unit='dB', min=0.0,
                  max=12.0, step=0.5)

FREQUENCY = v.axis('Frequency', 'Hz', 'log')


def _sig(value: float, digits: int = 4) -> float:
    """A reading rounded for display to `digits` significant figures."""
    value = float(value)
    if value == 0.0 or not np.isfinite(value):
        return value
    return float(f'{value:.{digits}g}')


def _unit_text(dimension: str) -> str | None:
    """The SI unit of a dimension as the application writes it ('m/s²');
    None when the units are undefined."""
    import visualdynamics as vd

    try:
        return vd.SI.label_text(dimension) or None
    except Exception:
        return None


# ---- the pair judged -----------------------------------------------------------

def _judged(run: RandomRun, channel: Channel,
            tolerance_db: float) -> tuple[Any, str, bool]:
    """(specification, how its limits read, whether the tolerance param
    supplied them). The run's own limits are used when it carries any;
    otherwise a copy of the specification gets abort limits at
    +/- `tolerance_db`, so the share outside is read by the same `compare`.
    """
    spec = run.specification
    if spec is None:
        raise refused(f'{run.title} carries no specification, so there is '
                      'nothing to judge the channel against.')
    if channel.spec_record is None:
        controls = ', '.join(c.label for c in run.channels
                             if c.spec_record is not None)
        raise refused(
            f'{channel.label} is not a control channel: the specification '
            f'has no record for it. It bounds {controls or "no channel"}; '
            'choose one of those.')
    if spec.has_limits:
        r = channel.spec_record
        written = spec.written(r)
        parts = []
        for pair in ('warning', 'abort'):
            edges = []
            for edge in ('lower', 'upper'):
                curve = spec.limits.get(f'{pair}_{edge}')
                if curve is None:
                    continue
                with np.errstate(divide='ignore', invalid='ignore'):
                    db = 10.0 * np.log10(curve[r][written]
                                         / spec.ordinate[r][written])
                db = db[np.isfinite(db)]
                if db.size:
                    edges.append(f'{float(np.median(db)):+.1f}')
            if edges:
                parts.append(f'{pair} {"/".join(edges)} dB')
        return spec, 'specification: ' + ', '.join(parts), False
    copy = spec.as_specification()
    factor = 10.0 ** (tolerance_db / 10.0)
    copy.limits = {'abort_lower': spec.ordinate / factor,
                   'abort_upper': spec.ordinate * factor}
    return copy, f'tolerance param: +/-{tolerance_db:g} dB', True


def _compare(run: RandomRun, spec: Any, channel: Channel) -> dict[str, Any]:
    from visualdynamics.core import compliance

    result = compliance.compare(spec, run.psds, channel.spec_record,
                                channel.record, scale_db=run.scale_db)
    if result.get('refused'):
        raise refused(f'{channel.label} cannot be judged: {result["refused"]}.')
    if not result.get('lines'):
        raise refused(f'{channel.label}: nothing to compare. The '
                      'specification and the measurement share no written '
                      'line, so no cell was judged.')
    return result


def _cells(run: RandomRun, spec: Any, channel: Channel) -> dict[str, Any]:
    """Per cell of the comparison: its midpoint, the error in dB, each
    limit in dB about the target, and whether it is out of each pair."""
    from visualdynamics.core import compliance

    r, m, scale = channel.spec_record, channel.record, run.scale_db
    target = compliance.judge(spec, run.psds, r, m, None, True, scale)
    found = target['cells']
    centers = np.array([(c['low'] + c['high']) / 2.0 for c in found])
    with np.errstate(divide='ignore', invalid='ignore'):
        error = np.where(target['judged'],
                         10.0 * np.log10(target['held'] / target['asked']),
                         np.nan)
    limits, out = {}, {}
    for name in spec.LIMITS:
        if spec.limits.get(name) is None:
            continue
        over = name.endswith('upper')
        judged = compliance.judge(spec, run.psds, r, m, name, over, scale)
        with np.errstate(divide='ignore', invalid='ignore'):
            limits[name] = np.where(target['judged'], 10.0 * np.log10(
                judged['asked'] / target['asked']), np.nan)
        pair = name.split('_')[0]
        out[pair] = out.get(pair, np.zeros(len(found), bool)) | judged['out']
    return {'centers': centers, 'error_db': error, 'limits_db': limits,
            'out': out, 'judged': target['judged']}


def _positive(y: np.ndarray) -> np.ndarray:
    """Non-positive values as NaN, a gap on a log axis."""
    y = np.array(y, dtype=float)
    y[~(y > 0.0)] = np.nan
    return y


def _spec_line(spec: Any, record: int, values: np.ndarray
               ) -> tuple[np.ndarray, np.ndarray]:
    """One curve of a specification over the lines it is written on:
    unwritten lines inside the span are NaN gaps (a controller's zero
    lines ask for nothing), and the ends are trimmed."""
    x = np.asarray(spec.abscissa, dtype=float)
    written = np.asarray(spec.written(record)) & (x > 0.0)
    if not written.any():
        return x[:0], x[:0]
    first, last = np.flatnonzero(written)[[0, -1]]
    y = np.where(written, values, np.nan)[first:last + 1]
    return x[first:last + 1], _positive(y)


# ---- the operations -----------------------------------------------------------

def psd_vs_spec(run: RandomRun, params: dict[str, Any]) -> list[dict]:
    from visualdynamics.core import compliance

    channel = run.channel(params['channel'])
    spec, limits_text, from_param = _judged(run, channel,
                                            params['tolerance_db'])
    result = _compare(run, spec, channel)
    cells = _cells(run, spec, channel)
    psds, r, m = run.psds, channel.spec_record, channel.record

    x = np.asarray(psds.abscissa, dtype=float)
    keep = x > 0.0
    measured = psds.ordinate[m] * 10.0 ** (run.scale_db / 10.0)
    # drawn as VD draws them: the PSD stepped across its bins, the
    # specification by its own interpolation (`views.shaped`)
    lines = [v.shaped(channel.label, 'measured', psds, x[keep],
                      _positive(measured[keep]))]
    lines.append(v.shaped('Specification', 'reference', spec,
                          *_spec_line(spec, r, spec.ordinate[r])))
    for name in spec.LIMITS:
        curve = spec.limits.get(name)
        if curve is None:
            continue
        label = (f'Tolerance {"+" if name.endswith("upper") else "-"}'
                 f'{params["tolerance_db"]:g} dB') if from_param else NAMES[name]
        role = 'tolerance' if from_param else ROLES[name]
        lines.append(v.shaped(label, role, spec,
                              *_spec_line(spec, r, curve[r])))
    dimension = str(psds.ordinate_dim[m])
    plot = v.spectrum(f'{channel.label}: PSD Against Specification',
                      FREQUENCY, v.axis('PSD', _unit_text(dimension), 'log'),
                      lines, link='frequency')

    rms_unit = _unit_text(quantity_of(dimension))
    db = float(result['difference_db'])
    rows = [
        v.row('Measured rms', _sig(result['measured_rms']), rms_unit, 'info'),
        v.row('Specification rms', _sig(result['specification_rms']),
              rms_unit, 'info'),
    ]
    if np.isfinite(db):
        state = compliance.rms_state(db)
        rows.append(v.row(f'RMS error (limit +/-{compliance.ERROR_DB:g} dB)',
                          round(db, 2), 'dB',
                          'pass' if state == 'in' else 'fail'))
    else:
        rows.append(v.row('RMS error', 'not read: an rms is zero', None,
                          'warn'))
    error = cells['error_db']
    if np.isfinite(error).any():
        worst = int(np.nanargmax(np.abs(error)))

        def outside(pair: str) -> bool:
            marks = cells['out'].get(pair)
            return marks is not None and bool(marks[worst])

        status = ('fail' if outside('abort') else 'warn' if outside('warning')
                  else 'pass')
        rows += [v.row('Worst line deviation', round(float(error[worst]), 2),
                       'dB', status),
                 v.row('Worst line at', _sig(cells['centers'][worst], 5),
                       'Hz', 'info')]
    for pair, word in (('warning', 'warning'), ('abort', 'abort')):
        percent = result.get(f'{pair}_percent')
        if percent is None:
            continue
        inside = round(100.0 - float(percent), 1)
        if pair == 'abort':
            label = ('Lines within tolerance' if from_param
                     else 'Lines within abort')
            status = ('pass' if compliance.lines_state(percent) == 'in'
                      else 'fail')
            label += f' (at least {100 - compliance.LINES_PERCENT:g} %)'
        else:
            label, status = f'Lines within {word}', 'info'
        rows.append(v.row(label, inside, '%', status))
    rows.append(v.row('Lines judged', int(result['lines']), None, 'info'))
    rows.append(v.row('Limits from', limits_text, None, 'info'))
    if result.get('scale_db'):
        rows.append(v.row('Comparison scale', float(result['scale_db']), 'dB',
                          'info'))
    if result.get('note'):
        rows.append(v.row('Note', str(result['note']), None, 'warn'))
    states = [item['state'] for item in compliance.readouts(result)]
    channel_ok = bool(states) and all(state == 'in' for state in states)
    rows.append(v.row('Channel', 'pass' if channel_ok else 'fail', None,
                      'pass' if channel_ok else 'fail'))
    verdict = (compliance.verdict(compliance.compare_all(
        spec, psds, scale_db=run.scale_db)) if from_param else run.verdict)
    passed = verdict.get('passed')
    rows.append(v.row(
        f'Run ({verdict.get("channels", 0)} control channels)',
        'not judged' if passed is None else 'pass' if passed else 'fail',
        None, 'warn' if passed is None else 'pass' if passed else 'fail'))
    return [plot, v.reading(f'{channel.label}: Overall', rows)]


def error_db(run: RandomRun, params: dict[str, Any]) -> list[dict]:
    channel = run.channel(params['channel'])
    spec, _limits_text, from_param = _judged(run, channel,
                                             params['tolerance_db'])
    _compare(run, spec, channel)          # refuses what cannot be judged
    cells = _cells(run, spec, channel)
    from visualdynamics.plot import bin_edges

    # one number per judged line, so every curve here is stepped across
    # the line's bin, as VD draws a per-line error
    x = cells['centers']
    edges = bin_edges(x)
    lines = [v.series(channel.label, 'measured', x, cells['error_db'],
                      'steps', edges),
             v.series('Specification', 'reference', x,
                      np.where(cells['judged'], 0.0, np.nan), 'steps', edges)]
    for name in spec.LIMITS:
        if name not in cells['limits_db']:
            continue
        label = (f'Tolerance {"+" if name.endswith("upper") else "-"}'
                 f'{params["tolerance_db"]:g} dB') if from_param else NAMES[name]
        lines.append(v.series(label, 'tolerance' if from_param else ROLES[name],
                              x, cells['limits_db'][name], 'steps', edges))
    return [v.spectrum(f'{channel.label}: Error Against Specification',
                       FREQUENCY, v.axis('Error', 'dB', 'linear'), lines,
                       link='frequency')]


OPERATIONS = (
    Operation(
        id='random.psd_vs_spec', title='PSD Against Specification',
        workspace='analyze', accepts='random_run',
        summary='A channel\'s PSD against its specification and limits, with '
                'its rms, its rms error and the share of lines in tolerance.',
        params=(CHANNEL, TOLERANCE), outputs=('spectrum', 'reading'),
        run=psd_vs_spec),
    Operation(
        id='random.error_db', title='Error Against Specification',
        workspace='analyze', accepts='random_run',
        summary='A channel\'s departure from its specification in dB, line '
                'by line, with its limits about zero.',
        params=(CHANNEL, TOLERANCE), outputs=('spectrum',), run=error_db),
)
