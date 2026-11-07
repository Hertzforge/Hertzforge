"""Comparisons of a pair or group of the project's objects, as Visual
Dynamics reads them when two objects are selected together: a measured PSD
against its specification (the spectra, the RMS error and lines out of every
channel, the lines outside beside what chance puts there, the composite of the
control channels), a specification's own levels, two densities as signal over
noise, two signatures mode by mode, a cross-spectral matrix as a grid, shock
spectra against their requirement, and a transient run against its reference.

Every number is the core's: `core.compliance`, `core.chance`,
`core.multi_point`, `core.snr`, `core.signature`, `core.matrix_comparison`
and `core.replication`. Each reading adds nothing to the project; Comparison
Scaling holds a scale on the measurement, as Visual Dynamics' scaling field
does.
"""

from __future__ import annotations

import re
from typing import Any

import numpy as np

from . import display_units
from . import views as v
from .descriptors import Operation, Option, Param
from .parsing import parse_numbers
from .protocol import refused
from .spec_lines import FREQUENCY, NAMES, ROLES, positive, quantity_of, sig, spec_line
from .waterfalls import unit_text

SPECTRUM = Param('spectrum', 'Measured PSD', 'enum', options_from='objects:psd')
SPECIFICATION = Param('specification', 'Specification', 'enum',
                      options_from='objects:specification')
RECORD = Param('record', 'Specification record', 'integer', default=1, min=1, step=1)
FIRST = Param('first', 'First PSD', 'enum', options_from='objects:psd')
SECOND = Param('second', 'Second PSD', 'enum', options_from='objects:psd')
SCALE = Param('scale', 'Scaling (blank detects it)', 'string', default='', unit='dB')

TIME = v.axis('Time', 's', 'linear')
#: the points a time trace is drawn with, every extreme kept
TRACE_POINTS = 8000


def _refusing(call: Any, *args: Any, **kwargs: Any) -> Any:
    """`call(*args, **kwargs)`, a ValueError from the core refused in its
    own words."""
    try:
        return call(*args, **kwargs)
    except ValueError as error:
        raise refused(str(error)) from None


def _two(params: dict[str, Any], first: str, second: str) -> None:
    if params[first] == params[second]:
        raise refused(f'{params[first]} against itself is no comparison; choose two objects')


def _db(value: float) -> float:
    """A reading to two places, never a negative zero."""
    return round(float(value), 2) + 0.0


def _status(state: str) -> str:
    return 'pass' if state == 'in' else 'fail'


def _verdict(passed: bool | None) -> tuple[str, str]:
    return {True: ('passes', 'pass'), False: ('fails', 'fail'),
            None: ('not judged', 'warn')}[passed]


# ---- a specification against a measured PSD -----------------------------------

def _scale(project: Any, spec_name: str, name: str) -> tuple[int, str]:
    """The comparison's scale (`Project.comparison_scale`) and where it
    came from, in words."""
    scale = project.comparison_scale(spec_name, name)
    held = any(project[n].scale_db is not None for n in project.scale_family(name))
    return scale, 'held on the measurement' if held else 'detected from the data'


def _scale_row(scale: int, source: str) -> dict:
    return v.row(f'Comparison scale ({source})', float(scale), 'dB', 'info')


def _matched(spec: Any, psd: Any, record: int, spec_name: str,
             name: str) -> tuple[str, int, int]:
    """(label, spec record, measured record) for the specification's
    record `record`, counted from 1 (`compliance.matched_records`)."""
    from hertzforge.core.compliance import matched_records

    if record > spec.num_records:
        raise refused(f'{spec_name} has {spec.num_records} records; there is no record {record}')
    pairs = matched_records(spec, psd)
    for label, j, i in pairs:
        if j == record - 1:
            return label, j, i
    shared = ', '.join(f'{j + 1} ({label})' for label, j, _i in pairs) or 'none'
    raise refused(f'{name} has no channel at {spec_name} record {record} '
                  f'({spec.record_label(record - 1)}); the records it shares are {shared}')


def _compared(project: Any, params: dict[str, Any]) -> tuple[Any, Any, list, int, str]:
    """(specification, measured, `compliance.compare_all` rows, scale,
    where the scale came from), refused when the two share no channel."""
    from hertzforge.core.compliance import compare_all

    spec_name, name = params['specification'], params['spectrum']
    spec, psd = project[spec_name], project[name]
    scale, source = _scale(project, spec_name, name)
    rows = _refusing(compare_all, spec, psd, scale_db=scale)
    if not rows:
        raise refused(f'{name} and {spec_name} have no channel in common to compare')
    return spec, psd, rows, scale, source


def _count_row(values: list[float], beyond: Any, where: str, notes: int,
               note: str | None, counted: str = 'channels') -> dict:
    """The bar chart's count as Visual Dynamics writes it: how many
    channels (or whatever is `counted`) are out, of how many scored, and
    the share. With none scored there is no verdict to give."""
    scored = [value for value in values if np.isfinite(value)]
    out = sum(1 for value in scored if beyond(value))
    if scored:
        text = f'{out} of {len(values) - notes} {counted} {where} — '
        text += f'{100.0 * out / len(scored):.0f}%'
    else:
        text = f'no {counted} scored'
    if notes:
        text += f'; {notes} not scored'
    if note:
        text += f'; {note}'
    status = 'warn' if not scored else 'fail' if out else 'pass'
    return v.row(f'{counted.capitalize()} out', text, None, status)


def spec_spectra(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.spec_spectra`: one channel of a measured PSD over its
    specification and the specification's warning and abort limits, the
    PSD scaled by the comparison's scale, with the channel's numbers
    (`compliance.compare`) and the run's verdict over every channel
    (`compliance.verdict`), as Visual Dynamics' Spectra reading draws a
    comparison one channel at a time."""
    from hertzforge.core import compliance

    spec, psd, rows, scale, source = _compared(project, params)
    label, r, m = _matched(spec, psd, params['record'], params['specification'],
                           params['spectrum'])
    result = compliance.compare(spec, psd, r, m, scale_db=scale)
    if result.get('refused'):
        raise refused(f'{label} cannot be judged: {result["refused"]}')
    x = np.asarray(psd.abscissa, dtype=float)
    keep = x > 0.0
    measured = np.real(np.asarray(psd.ordinate[m])) * 10.0 ** (scale / 10.0)
    name = params['spectrum'] if not scale else f'{params["spectrum"]} ({scale:+d} dB)'
    lines = [v.shaped(f'{name}: {label}', 'measured', psd, x[keep], positive(measured[keep])),
             v.shaped('Specification', 'reference', spec,
                      *spec_line(spec, r, np.real(spec.ordinate[r])))]
    for key in spec.LIMITS:
        curve = spec.limits.get(key)
        if curve is not None:
            lines.append(v.shaped(NAMES[key], ROLES[key], spec,
                                  *spec_line(spec, r, np.real(curve[r]))))
    dimension = str(psd.ordinate_dim[m])
    plot = v.spectrum(f'{label}: {params["spectrum"]} against {params["specification"]}',
                      FREQUENCY, v.axis('PSD', unit_text(dimension), 'log'), lines,
                      link='frequency')
    found = []
    if result.get('lines'):
        unit = unit_text(quantity_of(dimension))
        found += [v.row('Measured RMS', sig(result['measured_rms']), unit, 'info'),
                  v.row('Specification RMS', sig(result['specification_rms']), unit, 'info')]
        db = float(result['difference_db'])
        found.append(v.row(f'RMS error (±{compliance.ERROR_DB:g} dB)', _db(db), 'dB',
                           _status(compliance.rms_state(db))) if np.isfinite(db)
                     else v.row('RMS error', 'not read: an RMS is zero', None, 'warn'))
        for pair in ('abort', 'warning'):
            percent = result.get(f'{pair}_percent')
            if percent is None:
                continue
            status = (_status(compliance.lines_state(percent)) if pair == 'abort' else 'info')
            label_text = (f'Lines outside abort (at most {compliance.LINES_PERCENT:g} %)'
                          if pair == 'abort' else 'Lines outside warning')
            found.append(v.row(label_text, _db(float(percent)), '%', status))
        found.append(v.row('Lines judged', float(result['lines']), '', 'info'))
    else:
        found.append(v.row('Lines judged', 'none: the two share no written line', None, 'warn'))
    found.append(_scale_row(scale, source))
    if result.get('note'):
        found.append(v.row('Note', str(result['note']), None, 'warn'))
    verdict = compliance.verdict(rows)
    words, status = _verdict(verdict['passed'])
    found.append(v.row(f'Run ({verdict["channels"]} channels)', words, None, status))
    return [plot, v.reading(f'{label}: {params["spectrum"]} against {params["specification"]}',
                            found)]


ERROR_LOW = Param('low', 'Lowest RMS error', 'number', default=-3.0, unit='dB', max=0.0, step=0.5)
ERROR_HIGH = Param('high', 'Highest RMS error', 'number', default=3.0, unit='dB', min=0.0,
                   step=0.5)
LINES_CEILING = Param('ceiling', 'Lines outside abort, ceiling', 'number', default=10.0,
                      unit='%', min=0.0, max=100.0, step=1.0)


def spec_rms_error(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.spec_rms_error`: every channel's RMS against its
    specification's in dB (`compliance.channel_errors`), each judged
    against a floor and a ceiling, -3 and +3 dB by default: Visual
    Dynamics' RMS Error bars as a table."""
    from hertzforge.core import compliance

    low, high = params['low'], params['high']
    if not low < high:
        raise refused(f'the band runs from its low edge up; {low:g} to {high:g} dB does not')
    _spec, _psd, rows, scale, source = _compared(project, params)
    errors = compliance.channel_errors(rows)
    if not errors:
        raise refused('nothing in common to compare')
    notes = compliance.channel_refusals(rows)
    found = []
    for k, (label, db, _percent) in enumerate(errors):
        if k in notes:
            found.append(v.row(label, notes[k], None, 'warn'))
        elif not np.isfinite(db):
            found.append(v.row(label, 'not read: an RMS is zero', None, 'warn'))
        else:
            found.append(v.row(label, _db(db), 'dB', 'pass' if low <= db <= high else 'fail'))
    found.append(_count_row([db for _l, db, _p in errors], lambda db: db > high or db < low,
                            f'outside {low:g} to {high:g} dB', len(notes),
                            compliance.comparison_note(rows)))
    found.append(_scale_row(scale, source))
    return [v.reading(f'RMS error of {params["spectrum"]} against {params["specification"]}',
                      found)]


def spec_lines_out(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.spec_lines_out`: the share of every channel's lines
    outside its abort limits (`compliance.channel_errors`), each judged
    against a ceiling, 10 % by default: Visual Dynamics' Lines Out bars
    as a table."""
    from hertzforge.core import compliance

    ceiling = params['ceiling']
    spec, _psd, rows, scale, source = _compared(project, params)
    if not any(key.startswith('abort') for key in spec.limits):
        raise refused(f'{params["specification"]} carries no abort limits, so no line is '
                      'outside them')
    errors = compliance.channel_errors(rows)
    if not errors:
        raise refused('nothing in common to compare')
    notes = compliance.channel_refusals(rows)
    found = []
    for k, (label, _error, percent) in enumerate(errors):
        if k in notes:
            found.append(v.row(label, notes[k], None, 'warn'))
        else:
            found.append(v.row(label, _db(percent), '%',
                               _status(compliance.lines_state(percent, ceiling))))
    found.append(_count_row([p for _l, _e, p in errors],
                            lambda p: compliance.lines_state(p, ceiling) == 'over',
                            f'over {ceiling:g}%', len(notes), compliance.comparison_note(rows)))
    found.append(_scale_row(scale, source))
    return [v.reading(f'Lines out of {params["spectrum"]} against {params["specification"]}',
                      found)]


COMBINE = Param('combine', 'Combine lines', 'integer', default=1, min=1, max=64, step=1)
DOF = Param('dof', 'DOF per line (0 reads the PSD\'s own)', 'number', default=0.0, min=0.0,
            step=10.0)


def _steps(low: np.ndarray, high: np.ndarray) -> np.ndarray:
    """The edges of cells that run from `low` to `high`: their own where
    they tile, the midpoints between their centers where they do not."""
    from hertzforge.drawing.plot import bin_edges

    if low.size and np.allclose(high[:-1], low[1:]):
        return np.concatenate([low, high[-1:]])
    return bin_edges(np.sqrt(low * high))


def spec_chance(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.spec_chance`: one channel's miss from its target in dB
    against each pair of limits, and the lines outside each beside how
    many chance alone puts there at the comparison's degrees of freedom
    (`chance.compare_chance`), as Visual Dynamics' Chance reading shows
    them."""
    from hertzforge.core.chance import PAIRS, compare_chance, describe_chance

    spec_name, name = params['specification'], params['spectrum']
    spec, psd = project[spec_name], project[name]
    label, r, m = _matched(spec, psd, params['record'], spec_name, name)
    scale, source = _scale(project, spec_name, name)
    dof = params['dof'] or None
    result = _refusing(compare_chance, spec, psd, r, m, dof=dof, combine=params['combine'],
                       scale_db=scale)
    if 'refused' in result:
        raise refused(result['refused'])
    if not result['lines']:
        raise refused(f'nothing of {label} to compare')
    groups = result['groups']
    low, high = np.asarray(groups['low'], float), np.asarray(groups['high'], float)
    edges = _steps(low, high)
    x = np.sqrt(low * high)
    combined = result['combine'] > 1
    lines = [v.series('Target', 'reference', x, np.zeros_like(x), 'steps', edges)]
    if combined:
        single = compare_chance(spec, psd, r, m, dof=result['dof'], combine=1, scale_db=scale)
        each = single['groups']
        lo, hi = np.asarray(each['low'], float), np.asarray(each['high'], float)
        lines.append(v.series('Each line', 'other', np.sqrt(lo * hi), each['miss_db'], 'steps',
                              _steps(lo, hi)))
    lines.append(v.series(f'{result["combine"]} lines combined' if combined else 'Each line',
                          'measured', x, groups['miss_db'], 'steps', edges))
    roles = {'warning': 'tolerance', 'abort': 'abort'}
    for pair in PAIRS:
        if f'{pair}_lines' not in result:
            continue
        for side in ('upper', 'lower'):
            edge = np.asarray(groups[f'{pair}_{side}_db'], float)
            if np.isfinite(edge).any():
                lines.append(v.series(f'{pair.capitalize()} band, {side}', roles[pair], x, edge,
                                      'steps', edges))
    unit = 'lines' if not combined else 'groups'
    head = (f'{label} at {result["dof"]:.4g} DOF' if not combined else
            f'{label}, {result["combine"]} lines combined at {result["judged_dof"]:.4g} DOF')
    plot = v.spectrum(head, FREQUENCY, v.axis('Measured over target', 'dB', 'linear'), lines,
                      link='frequency')
    rows = [v.row('DOF per line', sig(result['dof']), '', 'info'),
            v.row('DOF judged', sig(result['judged_dof']), '', 'info'),
            v.row(f'{unit.capitalize()} judged',
                  float(result['lines']) if not combined
                  else f'{result["lines"]} groups of {result["combine"]}', None, 'info')]
    for pair in PAIRS:
        if f'{pair}_lines' not in result:
            continue
        rows.append(v.row(f'{pair.capitalize()} band', describe_chance(result, pair), None,
                          'info'))
    if not any(f'{pair}_lines' in result for pair in PAIRS):
        rows.append(v.row('Bands', 'the specification carries no bands', None, 'warn'))
    rows.append(_scale_row(scale, source))
    if result.get('note'):
        rows.append(v.row('Note', str(result['note']), None, 'warn'))
    return [plot, v.reading(f'Lines by chance: {head}', rows)]


STRATEGY = Param('strategy', 'Strategy', 'enum', default='average',
                 options=tuple(Option(key, label) for key, label in (
                     ('average', 'Average'), ('weighted', 'Weighted'),
                     ('maximum', 'Maximum'), ('minimum', 'Minimum'))))
WEIGHTS = Param('weights', 'Weights (weighted only)', 'string', default='')
BANDS = tuple(Option(key, label) for key, label in (
    ('warning', 'Spec warning limits'), ('abort', 'Spec abort limits'), ('db', 'dB band')))
COMPOSITE_BAND = Param('composite_band', 'Composite band', 'enum', default='warning',
                       options=BANDS)
COMPOSITE_LOW = Param('composite_low', 'Composite band, below', 'number', default=-3.0,
                      unit='dB', min=-40.0, max=0.0, step=0.5)
COMPOSITE_HIGH = Param('composite_high', 'Composite band, above', 'number', default=3.0,
                       unit='dB', min=0.0, max=40.0, step=0.5)
INDIVIDUAL_BAND = Param('individual_band', 'Channel band', 'enum', default='db', options=BANDS)
INDIVIDUAL_LOW = Param('individual_low', 'Channel band, below', 'number', default=-6.0,
                       unit='dB', min=-40.0, max=0.0, step=0.5)
INDIVIDUAL_HIGH = Param('individual_high', 'Channel band, above', 'number', default=6.0,
                        unit='dB', min=0.0, max=40.0, step=0.5)


def _numbers(text: str, what: str) -> tuple[float, ...] | None:
    """Numbers typed, separated by commas or spaces; None for none."""
    return tuple(parse_numbers(text, what)) or None


def _band(params: dict[str, Any], key: str) -> str | tuple[float, float]:
    chosen = params[f'{key}_band']
    return (params[f'{key}_low'], params[f'{key}_high']) if chosen == 'db' else chosen


def spec_composite(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.spec_composite`: the control channels combined as the
    controller held them, the composite against its band and each channel
    against its own (`multi_point.multi_point`, the channels by
    `multi_point.control_channels`), as Visual Dynamics' Composite reading
    draws them."""
    from hertzforge.core.multi_point import control_channels, multi_point

    spec_name, name = params['specification'], params['spectrum']
    spec, psd = project[spec_name], project[name]
    found = project.channel_table_for(name)
    channels = _refusing(control_channels, spec, psd, None,
                         found[1] if found is not None else None)
    weights = (_numbers(params['weights'], 'a list of weights, one per channel')
               if params['strategy'] == 'weighted' else None)
    scale, source = _scale(project, spec_name, name)
    reading = _refusing(multi_point, spec, psd, channels, strategy=params['strategy'],
                        weights=weights, composite_band=_band(params, 'composite'),
                        individual_band=_band(params, 'individual'), scale_db=scale)
    factor = 10.0 ** (reading.scale_db / 10.0)
    fx = np.asarray(psd.abscissa, dtype=float)
    keep = fx > 0.0
    band, outer = reading.composite_band, reading.individual_band
    lines = [v.shaped('Reference', 'reference', band,
                      *spec_line(band, 0, np.real(band.ordinate[0])))]
    for which, data, role in (('Composite band', band, 'tolerance'),
                              ('Channel band', outer, 'abort')):
        for edge in ('lower', 'upper'):
            values = data.limits.get(f'abort_{edge}')
            if values is not None:
                lines.append(v.shaped(f'{which}, {edge}', role, data,
                                      *spec_line(data, 0, np.real(values[0]))))
    for channel in reading.channels:
        y = np.real(np.asarray(psd.ordinate[channel.record])) * factor
        lines.append(v.shaped(channel.label, 'measured', psd, fx[keep], positive(y[keep])))
    y = np.real(np.asarray(reading.composite.ordinate[0])) * factor
    lines.append(v.shaped(f'Composite, {reading.strategy} of {len(reading.channels)}',
                          'measured', psd, fx[keep], positive(y[keep])))
    dimension = str(spec.ordinate_dim[reading.spec_record])
    plot = v.spectrum(f'Composite of {name} against {spec_name}', FREQUENCY,
                      v.axis('PSD', unit_text(dimension), 'log'), lines, link='frequency')
    rows = [_composite_row(reading)]
    for channel in reading.channels:
        rows.append(_channel_row(channel, reading))
    rows.append(v.row('Lines judged', float(reading.result.get('lines', 0)), '', 'info'))
    rows.append(_scale_row(scale, source))
    rows += [v.row('Note', note, None, 'warn') for note in reading.notes]
    if reading.hiding and reading.chance_share is not None and reading.chance_share >= 0.001:
        rows.append(v.row('By chance', f'about {100.0 * reading.chance_share:.2g} % of a working '
                          'channel\'s lines are outside its band by chance', None, 'info'))
    return [plot, v.reading(f'Composite of {name} against {spec_name}', rows)]


def _composite_row(reading: Any) -> dict:
    """The composite's line, as `plot.multi_point.describe_composite`
    words it."""
    head = f'{reading.strategy} of {len(reading.channels)}'
    if not reading.judged_lines:
        return v.row('Composite', f'{head}: not judged, since it was formed at no line', None,
                     'warn')
    count = reading.outside_lines
    return v.row('Composite', f'{head}: {count} line{"s" * (count != 1)} outside '
                 f'{reading.composite_text} ({reading.result.get("outside_percent", 0.0):.1f} %)',
                 None, 'warn' if count else 'pass')


def _channel_row(channel: Any, reading: Any) -> dict:
    """One channel's line, as `plot.multi_point.describe_channel` words
    it."""
    if not channel.judged_lines:
        return v.row(channel.label, 'not judged, since it has no finite line', None, 'warn')
    count = channel.outside_lines
    text = f'{count} line{"s" * (count != 1)} outside {reading.individual_text}'
    if count:
        text += f', {channel.hidden_lines} with the composite inside'
    return v.row(channel.label, text, None, 'warn' if channel.hidden_lines else 'info')


def comparison_scale(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.comparison_scale`: hold the decibels a measured PSD is
    scaled by when it is compared with a specification, on it and its
    whole family (`Project.set_comparison_scale`), or with the field
    blank go back to detecting it (`Project.comparison_scale`), as Visual
    Dynamics' scaling field does. The data is never changed."""
    spec_name, name = params['specification'], params['spectrum']
    raw = params['scale'].strip().lower().removesuffix('db').strip()
    try:
        value = int(raw) if raw else None
    except ValueError:
        raise refused('Scaling is whole decibels — "+6", "-3", 0, or blank to detect it '
                      'from the data') from None
    family = _refusing(project.set_comparison_scale, name, value)
    scale = project.comparison_scale(spec_name, name)
    rows = [v.row('Comparison scale', float(scale), 'dB', 'info'),
            v.row('Scaled', 'detected from the data' if value is None
                  else 'held: the data itself is unchanged', None, 'info'),
            v.row('Held on' if value is not None else 'Detected for', ', '.join(family), None,
                  'info')]
    return [v.reading(f'{name} against {spec_name}', rows)]


def spec_rms(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.spec_rms`: the RMS level each channel of a specification
    asks for (`compliance.specification_rms`), one row per autospectrum,
    as Visual Dynamics' RMS bars of a specification on its own show
    them, with no limit to be out of."""
    from hertzforge.core.compliance import specification_rms
    from hertzforge.core.data import autospectrum
    from hertzforge.drawing.plot import pair_label

    name = params['specification']
    spec = project[name]
    references = spec.reference_dof
    wanted = [i for i in range(spec.num_records) if autospectrum(spec, i)]
    if not wanted:
        raise refused(f'{name} holds no autospectrum, so no channel asks for a level')
    rows, top = [], None
    for i in wanted:
        level = float(specification_rms(spec, i))
        quantity = quantity_of(str(spec.ordinate_dim[i]))
        unit = unit_text(quantity)
        label = pair_label((str(spec.response_dof[i]),
                            str(spec.response_dof[i] if references is None else references[i])))
        if np.isfinite(level):
            rows.append(v.row(label, sig(level), unit, 'info'))
            if top is None or level > top[0]:
                top = (level, label, quantity if unit else None)
        else:
            rows.append(v.row(label, 'not read', None, 'warn'))
    count = f'{len(wanted)} channel{"s" * (len(wanted) != 1)}'
    if top is not None:
        said, scale = display_units.words_in(top[2]) if top[2] else ('', 1.0)
        count += f' — {top[1]} highest at {top[0] * scale:.4g} {said}'.rstrip()
    rows.append(v.row('Channels', count, None, 'info'))
    return [v.reading(f'RMS levels of {name}', rows)]


# ---- two densities -------------------------------------------------------------

def _loud_quiet(project: Any, params: dict[str, Any]) -> tuple[tuple[str, Any], tuple[str, Any]]:
    """The two densities, louder first: the signal and the noise, by the
    rule Visual Dynamics reads the pair by (the higher mean density)."""
    _two(params, 'first', 'second')
    a, b = ((params[key], project[params[key]]) for key in ('first', 'second'))
    level = [float(np.nanmean(np.real(np.asarray(data.ordinate)))) for _n, data in (a, b)]
    return (a, b) if level[0] >= level[1] else (b, a)


def psd_overlay(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.psd_overlay`: two densities drawn over each other, channel
    by channel where they share one (`data.paired_channels`), the louder
    solid and the quieter dashed: Visual Dynamics' Overlaid reading."""
    from hertzforge.core.data import paired_channels

    (loud_name, loud), (quiet_name, quiet) = _loud_quiet(project, params)
    pairs = _refusing(paired_channels, loud, quiet)
    if not pairs:
        raise refused(f'{loud_name} and {quiet_name} share no channel')
    x = np.asarray(loud.abscissa, dtype=float)
    keep = x > 0.0
    lines = []
    for name, data, role, index in ((loud_name, loud, 'measured', 0),
                                    (quiet_name, quiet, 'reference', 1)):
        for pair in pairs:
            y = np.real(np.asarray(data.ordinate[pair[index]]))
            lines.append(v.shaped(f'{name}: {pair[2]}', role, data, x[keep],
                                  positive(y[keep])))
    dims = {str(loud.ordinate_dim[i]) for i, _j, _d, _q in pairs}
    unit = unit_text(dims.pop()) if len(dims) == 1 else None
    return [v.spectrum(f'{loud_name} over {quiet_name}', FREQUENCY, v.axis('PSD', unit, 'log'),
                       lines, link='frequency')]


def signal_to_noise(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.signal_to_noise`: the louder density's signal over the
    quieter's noise, line by line and channel by channel, in dB
    (`snr.signal_to_noise`: the louder less the quieter, over the
    quieter), with the median as Visual Dynamics' Signal to noise reading
    gives it."""
    from hertzforge.core.snr import signal_to_noise as ratio

    (loud_name, loud), (quiet_name, quiet) = _loud_quiet(project, params)
    x, rows, dofs, _dims = _refusing(ratio, loud, quiet)
    if not dofs:
        raise refused(f'{loud_name} and {quiet_name} share no channel')
    with np.errstate(divide='ignore', invalid='ignore'):
        decibels = 10.0 * np.log10(np.real(rows))
    x = np.asarray(x, dtype=float)
    lines = []
    for row, dof in zip(decibels, dofs, strict=True):
        finite = np.isfinite(row) & (x > 0.0)
        if finite.any():
            lines.append(v.series(str(dof), 'measured', x[finite], row[finite]))
    lines.append(v.series('Signal equal to noise', 'reference', x[[0, -1]], np.zeros(2)))
    finite = np.real(rows)[np.isfinite(np.real(rows))]
    median = (v.row('Median', _db(10.0 * np.log10(np.median(finite))), 'dB', 'info')
              if finite.size and np.median(finite) > 0
              else v.row('Median', 'no line above the floor', None, 'warn'))
    reading = v.reading(f'{loud_name} over {quiet_name}', [
        v.row('Signal', loud_name, None, 'info'), v.row('Noise', quiet_name, None, 'info'),
        v.row('Channels', float(len(dofs)), '', 'info'), median])
    return [v.spectrum(f'{loud_name} over {quiet_name}: signal to noise',
                       v.axis('Frequency', 'Hz', 'linear'),
                       v.axis('Signal to noise', 'dB', 'linear'), lines, link='frequency'),
            reading]


FLOOR = Param('floor', 'Signal to noise, floor', 'number', default=10.0, unit='dB', step=1.0)


def rms_signal_to_noise(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.rms_signal_to_noise`: each channel's signal over its
    noise across the whole band, from the area under the two densities
    (`snr.rms_signal_to_noise`), judged against a floor, 10 dB by
    default: Visual Dynamics' RMS signal to noise bars as a table."""
    from hertzforge.core.snr import AT_FLOOR
    from hertzforge.core.snr import rms_signal_to_noise as rms

    (loud_name, loud), (quiet_name, quiet) = _loud_quiet(project, params)
    found, silent = _refusing(rms, loud, quiet)
    if not found and not silent:
        raise refused(f'{loud_name} and {quiet_name} share no channel')
    low = params['floor']
    rows = []
    for label, value in found:
        if np.isfinite(value):
            rows.append(v.row(label, _db(float(value)), 'dB',
                              'fail' if value < low else 'pass'))
        else:
            rows.append(v.row(label, AT_FLOOR, None, 'fail'))
    out = sum(1 for _l, value in found if not np.isfinite(value) or value < low)
    share = 100.0 * out / len(found) if found else 0.0
    rows.append(v.row('Channels under the floor',
                      f'{out} of {len(found)} channels under {low:g}dB — {share:.0f}%', None,
                      'fail' if out else 'pass'))
    if silent:
        rows.append(v.row('Not read', f'{len(silent)} channel{"s" * (len(silent) != 1)} '
                          f'recorded no ambient noise: {", ".join(silent)}', None, 'warn'))
    return [v.reading(f'RMS signal to noise, {loud_name} over {quiet_name}', rows)]


# ---- two signatures ------------------------------------------------------------

BEFORE = Param('before', 'Before', 'enum', options_from='objects:frf|spectrum')
AFTER = Param('after', 'After', 'enum', options_from='objects:frf|spectrum')
SIGNATURE_RECORD = Param('record', 'Record (of the one before)', 'integer', default=1, min=1,
                         step=1)
MODES = Param('modes', 'Modes (blank finds them)', 'string', default='', unit='Hz')
FIND = Param('find', 'Peaks found', 'integer', default=2, min=1, max=20, step=1)
FREQUENCY_LIMIT = Param('frequency_limit', 'Frequency, either way', 'number', default=5.0,
                        unit='%', min=0.0, max=100.0, step=1.0)
AMPLITUDE_LIMIT = Param('amplitude_limit', 'Amplitude, either way', 'number', default=20.0,
                        unit='%', min=0.0, max=100.0, step=1.0)


def _modes(text: str) -> list[tuple[float, bool]] | None:
    """The modes typed: frequencies in Hz, one that does not count in
    parentheses ('84, (412)'); None for none."""
    out = []
    for token in text.replace(',', ' ').split():
        counts = not (token.startswith('(') and token.endswith(')'))
        try:
            frequency = float(token.strip('()'))
        except ValueError:
            raise refused(f'{token!r} is not a frequency') from None
        if not frequency > 0.0:
            raise refused(f'{token!r} is not a positive frequency')
        out.append((frequency, counts))
    return out or None


def signature(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.signature`: one record of a signature before a test and
    the same record after it, compared mode by mode against frequency and
    amplitude limits (`signature.compare_signatures`), the modes named or
    the most prominent peaks before (`signature.find_modes`): Visual
    Dynamics' Signature reading, each mode's window drawn as its two
    height limits across its frequency limits."""
    from hertzforge.core.data import Frf
    from hertzforge.core.signature import compare_signatures, find_modes, matching_record

    _two(params, 'before', 'after')
    before_name, after_name = params['before'], params['after']
    before, after = project[before_name], project[after_name]
    if isinstance(before, Frf) != isinstance(after, Frf):
        raise refused(f'{before_name} and {after_name} are not two signatures of one kind: '
                      'two FRFs, or two spectra')
    if params['record'] > before.num_records:
        raise refused(f'{before_name} has {before.num_records} records; there is no record '
                      f'{params["record"]}')
    record = params['record'] - 1
    modes = _modes(params['modes'])
    found = modes is None
    if found:
        modes = find_modes(before, record, params['find'])
        if not modes:
            raise refused(f'{before_name} has no peak to compare')
    comparison = _refusing(compare_signatures, before, after, modes, record=record,
                           frequency_limit=params['frequency_limit'],
                           amplitude_limit=params['amplitude_limit'])
    other = matching_record(before, record, after)
    dimension = str(before.ordinate_dim[record])
    unit = unit_text(dimension)
    lines = []
    for name, data, i in ((before_name, before, record), (after_name, after, other)):
        x = np.asarray(data.abscissa, dtype=float)
        y = np.abs(np.atleast_2d(data.ordinate)[i]).astype(float)
        keep = np.isfinite(y) & (y > 0.0)
        lines.append(v.series(name, 'measured', x[keep], y[keep]))
    rows = []
    for shift in comparison.shifts:
        f_low, f_high, h_low, h_high = shift.box()
        role = 'tolerance' if shift.mode.counts else 'other'
        for edge, height in (('upper', h_high), ('lower', h_low)):
            lines.append(v.series(f'{shift.mode.label()} window, {edge}', role,
                                  [f_low, f_high], [height, height]))
        status = {True: 'pass', False: 'fail', None: 'info'}[shift.passed]
        rows.append(v.row(shift.mode.label(), shift.describe(), None, status))
        rows.append(v.row(f'{shift.mode.label()}: frequency shift', _db(shift.frequency_shift),
                          '%', 'info'))
        rows.append(v.row(f'{shift.mode.label()}: amplitude shift', _db(shift.amplitude_shift),
                          '%', 'info'))
        for which, reading in (('before', shift.before), ('after', shift.after)):
            q = reading.q
            rows.append(v.row(f'{shift.mode.label()}: Q {which}',
                              'not read' if q is None else round(float(q), 3), None, 'info'))
        rows += [v.row(f'{shift.mode.label()}: note', note, None, 'warn') for note in shift.notes]
    status = _verdict(comparison.passed)[1]
    rows.insert(0, v.row('Verdict', comparison.describe(), None, status))
    rows.insert(1, v.row('Modes', 'found: the most prominent peaks before' if found
                         else 'named', None, 'info'))
    where = comparison.dof or before.record_label(record)
    title = f'{where}: {before_name} before, {after_name} after'
    plot = v.spectrum(title, v.axis('Frequency', 'Hz', 'linear'),
                      v.axis('Magnitude', unit, 'log'), lines, link='frequency')
    return [plot, v.reading(title, rows)]


# ---- a cross-spectral matrix against its specification ------------------------

CPSD = Param('cpsd', 'Cross-spectral matrix', 'enum', options_from='objects:psd')
COHERENCE_TOLERANCE = Param('coherence_tolerance', 'Coherence, either way', 'number',
                            default=0.1, min=0.01, max=1.0, step=0.01)
PHASE_TOLERANCE = Param('phase_tolerance', 'Phase, either way', 'number', default=10.0,
                        unit='deg', min=0.5, max=180.0, step=1.0)
COHERENCE_FLOOR = Param('coherence_floor', 'Coherence a phase is judged from', 'number',
                        default=0.5, min=0.0, max=0.99, step=0.05)

#: how a cell's state reads as a status
STATES = {'in': 'pass', 'out': 'fail'}


def matrix_comparison(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.matrix_comparison`: a measured cross-spectral matrix
    judged whole against its specification
    (`matrix_comparison.compare_matrix`): each level on the diagonal, each
    cross term's coherence above it and phase below it, as the share of
    their lines out, and every cell's verdict, as Visual Dynamics' Matrix
    reading gives them."""
    from hertzforge.core.matrix_comparison import compare_matrix

    spec_name, name = params['specification'], params['cpsd']
    spec, measured = project[spec_name], project[name]
    scale, source = _scale(project, spec_name, name)
    comparison = _refusing(compare_matrix, spec, measured,
                           coherence_tolerance=params['coherence_tolerance'],
                           phase_tolerance=params['phase_tolerance'],
                           coherence_floor=params['coherence_floor'], scale_db=scale)
    size = comparison.size
    values = np.full((size, size), np.nan)
    for k, level in enumerate(comparison.levels):
        if level.state in ('in', 'out'):
            values[k, k] = float(level.result.get('abort_percent', np.nan))
    for pair in comparison.pairs:
        for (r, c), reading in (((pair.row, pair.column), 'coherence'),
                                ((pair.column, pair.row), 'phase')):
            if getattr(pair, f'{reading}_state') in ('in', 'out'):
                values[r, c] = float(getattr(pair, f'{reading}_percent'))
    ticks = list(comparison.channels)
    grid = v.matrix(f'{name} against {spec_name}: lines out, levels on the diagonal, coherence '
                    'above, phase below', {'label': 'Channel', 'ticks': ticks},
                    {'label': 'Channel', 'ticks': ticks}, values, '% of lines out', (0.0, 20.0))
    rows = [v.row('Verdict', comparison.describe(), None, _verdict(comparison.passed)[1])]
    for entry in comparison.table():
        rows.append(v.row(f'{entry["label"]} {entry["reading"]}', entry['words'], None,
                          STATES.get(entry['state'], 'warn' if entry['state'] != 'not specified'
                                     else 'info')))
    rows.append(_scale_row(scale, source))
    rows += [v.row('Note', note, None, 'warn') for note in comparison.notes]
    return [grid, v.reading(f'{name} against {spec_name}', rows)]


# ---- shock spectra against their requirement -----------------------------------

SRS = Param('srs', 'Measured SRS', 'enum', options_from='objects:srs')
REQUIREMENT = Param('requirement', 'Required SRS', 'enum',
                    options_from='objects:shock_specification')
REQUIREMENT_RECORD = Param('record', 'Requirement record', 'integer', default=1, min=1, step=1)
SRS_LOW = Param('low', 'Lowest SRS error', 'number', default=-3.0, unit='dB', max=0.0, step=0.5)
SRS_HIGH = Param('high', 'Highest SRS error', 'number', default=3.0, unit='dB', min=0.0,
                 step=0.5)


def _srs_rows(requirement: Any, measured: Any) -> list[Any]:
    from hertzforge.core.compliance import srs_errors

    return srs_errors(requirement, measured)


def srs_spectra(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.srs_spectra`: every event's shock spectrum at one channel
    against the one it had to meet and its limits, with each event's
    signed RMS deviation in dB (`compliance.srs_errors`), as Visual
    Dynamics' Spectra reading of a shock comparison shows one channel at
    a time."""
    spec_name, name = params['requirement'], params['srs']
    spec, measured = project[spec_name], project[name]
    if params['record'] > spec.num_records:
        raise refused(f'{spec_name} has {spec.num_records} records; there is no record '
                      f'{params["record"]}')
    j = params['record'] - 1
    dof = str(spec.response_dof[j])
    keep = [i for i in range(measured.num_records) if str(measured.response_dof[i]) == dof]
    if not keep:
        raise refused(f'{name} has no spectrum at {dof}')
    lines = []
    for i in keep:
        x = np.asarray(measured.abscissa, dtype=float)
        y = np.real(np.asarray(measured.ordinate[i]))
        good = (x > 0.0) & np.isfinite(y) & (y > 0.0)
        event = '' if measured.block is None else f', {measured.block[i]}'
        lines.append(v.shaped(f'{dof}{event}', 'measured', measured, x[good], y[good]))
    x = np.asarray(spec.abscissa, dtype=float)
    lines.append(v.shaped('Required', 'reference', spec, x, positive(np.real(spec.ordinate[j]))))
    for key in getattr(spec, 'LIMITS', ()):
        curve = (getattr(spec, 'limits', None) or {}).get(key)
        if curve is not None:
            lines.append(v.shaped(NAMES[key], ROLES[key], spec, x, positive(np.real(curve[j]))))
    plot = v.spectrum(f'{dof}: {len(keep)} event{"s" * (len(keep) != 1)} against the one '
                      'required', FREQUENCY,
                      v.axis('Peak response', unit_text(str(spec.ordinate_dim[j])), 'log'),
                      lines, link='frequency')
    rows = []
    for row in _srs_rows(spec, measured):
        if row.dof != dof:
            continue
        label = row.dof if row.event is None else f'{row.dof}, {row.event}'
        rows.append(v.row(label, f'not scored: {row.refused}', None, 'warn') if row.refused
                    else v.row(label, _db(row.db), 'dB', 'info'))
    unread = next((row.unread for row in _srs_rows(spec, measured)
                   if row.dof == dof and row.unread), None)
    if unread:
        rows.append(v.row('Not read', unread, None, 'warn'))
    return [plot, v.reading(f'{dof}: {name} against {spec_name}', rows)]


def srs_error(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.srs_error`: every channel's shock spectrum against the
    one it had to meet, one signed RMS deviation in dB across the band
    per channel and event (`compliance.srs_errors`), judged against a
    floor and a ceiling, -3 and +3 dB by default: Visual Dynamics' SRS
    Error bars as a table."""
    from hertzforge.core.compliance import comparison_note

    low, high = params['low'], params['high']
    if not low < high:
        raise refused(f'the band runs from its low edge up; {low:g} to {high:g} dB does not')
    spec_name, name = params['requirement'], params['srs']
    found = _srs_rows(project[spec_name], project[name])
    if not found:
        raise refused(f'{name} and {spec_name} have no channel in common to compare')
    events = sorted({row.event for row in found if row.event}, key=lambda b: (len(b), b))
    rows, values, notes = [], [], 0
    for row in found:
        label = f'{row.dof} e{events.index(row.event) + 1}' if len(events) > 1 else row.dof
        if row.refused:
            rows.append(v.row(label, f'not scored: {row.refused}', None, 'warn'))
            notes += 1
            values.append(float('nan'))
            continue
        values.append(row.db)
        inside = 'warn' if row.unread else 'pass'
        rows.append(v.row(label, _db(row.db), 'dB',
                          inside if low <= row.db <= high else 'fail'))
    count = _count_row(values, lambda db: db > high or db < low,
                       f'outside {low:g} to {high:g} dB', notes, comparison_note(found),
                       'readings' if len(events) > 1 else 'channels')
    unread = next((row.unread for row in found if row.unread), None)
    if unread and count['status'] == 'pass':
        count['status'] = 'warn'
    rows.append(count)
    if unread:
        rows.append(v.row('Not read', unread, None, 'warn'))
    return [v.reading(f'SRS error of {name} against {spec_name}', rows)]


SRS_POINT = Param('record', 'Measured record', 'integer', default=1, min=1, step=1)
RULE = Param('rule', 'Rule', 'enum', default='share',
             options=(Option('share', 'Share inside'), Option('above_lower', 'All above lower')))
PERCENT = Param('percent', 'Share inside', 'number', default=90.0, unit='%', min=0.0, max=100.0,
                step=5.0)
LIMITS = Param('limits', 'Judge against the requirement\'s limits', 'boolean', default=True)
INNER_LOW = Param('inner_low', 'Inner band, below', 'number', default=-1.5, unit='dB',
                  min=-40.0, max=0.0, step=0.5)
INNER_HIGH = Param('inner_high', 'Inner band, above', 'number', default=3.0, unit='dB', min=0.0,
                   max=40.0, step=0.5)
OUTER = Param('outer', 'Outer band', 'boolean', default=False)
OUTER_LOW = Param('outer_low', 'Outer band, below', 'number', default=-3.0, unit='dB',
                  min=-40.0, max=0.0, step=0.5)
OUTER_HIGH = Param('outer_high', 'Outer band, above', 'number', default=6.0, unit='dB', min=0.0,
                   max=40.0, step=0.5)


def _edge(name: str, role: str, x: np.ndarray, target: np.ndarray, db: Any) -> dict:
    with np.errstate(invalid='ignore'):
        y = target * 10.0 ** (np.asarray(db, dtype=float) / 20.0)
    return v.series(name, role, x, positive(np.broadcast_to(y, x.shape)))


def srs_points(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.srs_points`: one measured shock spectrum judged point by
    point against its requirement's tolerance band under the rule the
    test plan names (`compliance.srs_tolerance`), the bands its own
    warning and abort limits unless typed: Visual Dynamics' Points Rule
    reading."""
    from hertzforge.core.compliance import (
        log_interpolate,
        srs_band_text,
        srs_limit_bands,
        srs_tolerance,
    )

    spec_name, name = params['requirement'], params['srs']
    spec, measured = project[spec_name], project[name]
    if params['record'] > measured.num_records:
        raise refused(f'{name} has {measured.num_records} records; there is no record '
                      f'{params["record"]}')
    i = params['record'] - 1
    dof = str(measured.response_dof[i])
    j = next((k for k in range(spec.num_records) if str(spec.response_dof[k]) == dof), None)
    if j is None:
        raise refused(f'{spec_name} requires nothing at {dof}')
    inner_limits, outer_limits = (band is not None for band in srs_limit_bands(spec, j))
    use = params['limits']
    from_inner, from_outer = use and inner_limits, use and outer_limits
    result = _refusing(
        srs_tolerance, spec, measured, j, i,
        inner=None if from_inner else (params['inner_low'], params['inner_high']),
        outer=((params['outer_low'], params['outer_high'])
               if params['outer'] and not from_outer else None),
        rule=params['rule'], percent=params['percent'], limits=from_inner or from_outer)
    if 'refused' in result:
        raise refused(result['refused'])
    x = np.asarray(measured.abscissa, dtype=float)
    got = np.real(np.atleast_2d(measured.ordinate)[i]).astype(float)
    target = log_interpolate(x, spec.abscissa, np.real(np.atleast_2d(spec.ordinate)[j]))
    keep = x > 0.0
    lines = [v.series(f'{dof}' + ('' if result['event'] is None else f', {result["event"]}'),
                      'measured', x[keep], positive(got[keep])),
             v.series('Required', 'reference', x[keep], positive(target[keep]))]
    for band, role, which in ((result['inner'], 'tolerance', 'Inner band'),
                              (result['outer'], 'abort', 'Outer band')):
        if band is None:
            continue
        for edge, db in zip(('lower', 'upper'), band, strict=True):
            if np.ndim(db) or np.isfinite(db):
                lines.append(_edge(f'{which}, {edge}', role, x[keep], target[keep],
                                   db[keep] if np.ndim(db) else db))
    where = dof if result['event'] is None else f'{dof}, {result["event"]}'
    band = srs_band_text(result['inner'], result['outer'])
    plot = v.spectrum(f'{where}: {band}', FREQUENCY,
                      v.axis('Peak response', unit_text(str(spec.ordinate_dim[j])), 'log'),
                      lines, link='frequency')
    words, status = _verdict(result['passed'])
    kind = result.get('kind') or 'measured'
    rows = [v.row('Verdict', words, None, status),
            v.row('Band', band + (' (the requirement\'s limits)' if result['limits'] else ''),
                  None, 'info'),
            v.row('Rule', 'share inside' if result['rule'] == 'share' else 'all above lower',
                  None, 'info'),
            v.row('Points judged', float(result['points']), '', 'info')]
    if result['points']:
        rows += [v.row(f'Points inside ({kind})', f'{result["inside_count"]} of '
                       f'{result["points"]}', None, status),
                 v.row('Share inside', _db(result['inside_percent']), '%', status),
                 v.row('Over the band', float(result['over_count']), '', 'info'),
                 v.row('Under the band', float(result['under_count']), '', 'info')]
        if result['outer'] is not None:
            rows.append(v.row('Outside the outer band', float(result['outer_count']), '',
                              'fail' if result['outer_count'] else 'pass'))
        rows += [v.row('Worst point', _db(result['worst_db']), 'dB', 'info'),
                 v.row('Worst at', sig(result['worst_frequency']), 'Hz', 'info')]
    if result.get('note'):
        rows.append(v.row('Note', str(result['note']), None, 'warn'))
    if result.get('unread'):
        rows.append(v.row('Not read', result['unread'], None, 'warn'))
    return [plot, v.reading(f'Points rule: {where}', rows)]


# ---- a transient run against its reference ------------------------------------

HISTORY = Param('history', 'Run', 'enum', options_from='objects:time_history')
REFERENCE = Param('reference', 'Reference', 'enum',
                  options_from='objects:transient_specification')
REFERENCE_RECORD = Param('record', 'Reference record', 'integer', default=1, min=1, step=1)
PLAYING = Param('playing', 'Playing', 'integer', default=1, min=1, step=1)
WAVEFORM_CEILING = Param('ceiling', 'Waveform error, ceiling', 'number', default=20.0, unit='%',
                         min=0.0, step=1.0)


def _reference_dof(project: Any, params: dict[str, Any]) -> str:
    reference = project[params['reference']]
    if params['record'] > reference.num_records:
        raise refused(f'{params["reference"]} has {reference.num_records} records; there is no '
                      f'record {params["record"]}')
    return str(reference.response_dof[params['record'] - 1])


def _trace(name: str, role: str, x: np.ndarray, y: np.ndarray) -> dict:
    from hertzforge.decimate import peak_decimate

    return v.series(name, role, *peak_decimate(x, y, TRACE_POINTS))


def replication_waveforms(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.replication_waveforms`: one channel of a transient run
    over one playing of the reference it was controlled to, aligned by
    the run's one shift (`replication_segments.aligned_traces`), with its
    waveform error at that playing (`replication.compare`): Visual
    Dynamics' Waveforms reading."""
    from hertzforge.core import replication as rep
    from hertzforge.core.replication_segments import aligned_traces

    run_name, ref_name = params['history'], params['reference']
    run, reference = project[run_name], project[ref_name]
    dof = _reference_dof(project, params)
    playing = params['playing'] - 1
    time, target, control, dof, lag = _refusing(aligned_traces, run, reference, dof,
                                                playing=playing)
    found = rep.playings(run, reference)
    unit = unit_text(str(reference.ordinate_dim[params['record'] - 1]))
    plot = v.spectrum(f'{dof}: event {playing + 1} of {len(found)} against {ref_name}', TIME,
                      v.axis('Amplitude', unit, 'linear'),
                      [_trace(f'{run_name}, event {playing + 1}', 'measured', time, control),
                       _trace(ref_name, 'reference', time, target)], link='time')
    row = next((r for r in rep.compare(run, reference, lag=lag, frame=playing,
                                       metrics=('waveform',)) if r['label'] == dof), None)
    rows = [v.row('Channel', dof, None, 'info'),
            v.row('Playing', f'{playing + 1} of {len(found)}', None, 'info'),
            v.row('Aligned', float(lag), 'samples', 'info')]
    rows.append(_waveform_row('Waveform error', row, rep.WAVEFORM_PERCENT))
    spare = rep.leftover(run, reference, lag=lag)
    if spare is not None:
        rows.append(v.row('Further event', f'{spare[0] / run.sample_rate:.2f} s ({spare[1]:.0%}) '
                          'was recorded and is too short to analyze', None, 'info'))
    return [plot, v.reading(f'{dof}: {run_name} against {ref_name}', rows)]


def _waveform_row(label: str, row: dict | None, ceiling: float) -> dict:
    if row is None:
        return v.row(label, 'not scored: no record at this channel', None, 'warn')
    if row.get('refused'):
        return v.row(label, f'not scored: {row["refused"]}', None, 'warn')
    if row.get('zero_target'):
        return v.row(label, 'not scored: the reference asks for nothing here', None, 'warn')
    value = float(row['waveform'])
    if not np.isfinite(value):
        return v.row(label, 'not read', None, 'warn')
    return v.row(label, _db(value), '%', 'fail' if value >= ceiling else 'pass')


def waveform_error(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.waveform_error`: every control channel's waveform error at
    every playing of the reference (`replication.compare`), judged against
    a ceiling, 20 % by default: Visual Dynamics' Waveform Error bars as a
    table."""
    from hertzforge.core import replication as rep
    from hertzforge.core.compliance import comparison_note

    run_name, ref_name = params['history'], params['reference']
    run, reference = project[run_name], project[ref_name]
    if rep.averaging_for(run, reference) is None:
        raise refused('the record is shorter than one playing of the specification')
    found = rep.compare(run, reference, metrics=('waveform',))
    if not found:
        raise refused(f'{run_name} has no record at a channel {ref_name} controls')
    ceiling = params['ceiling']
    many = len({row['frame'] for row in found}) > 1
    rows, values, notes = [], [], 0
    for row in found:
        label = f'{row["label"]} e{row["frame"] + 1}' if many else row['label']
        rows.append(_waveform_row(label, row, ceiling))
        value = float(row['waveform'])
        values.append(value)
        notes += bool(row.get('refused') or row.get('zero_target'))
    rows.append(_count_row(values, lambda value: value >= ceiling, f'over {ceiling:g}%', notes,
                           comparison_note(found), 'readings' if many else 'channels'))
    lag = rep.lag_of(run, reference)
    rows.append(v.row('Aligned', float(lag), 'samples', 'info'))
    return [v.reading(f'Waveform error of {run_name} against {ref_name}', rows)]


SEGMENTS = Param('segments', 'Segments (start end kind [name]; ...)', 'string', default='',
                 unit='s')
TAPER = Param('taper_percent', 'Taper', 'number', default=10.0, unit='%', min=0.0, max=100.0,
              step=5.0)
BAND_LOW = Param('band_low', 'Test band, low (0 judges every line)', 'number', default=0.0,
                 unit='Hz', min=0.0, step=1.0)
BAND_HIGH = Param('band_high', 'Test band, high', 'number', default=0.0, unit='Hz', min=0.0,
                  step=10.0)
ASD = Param('asd_db', 'ASD, either way', 'number', default=3.0, unit='dB', min=0.1, max=40.0,
            step=0.5)
VARIANCE = Param('variance_percent', 'Mean square, over', 'number', default=10.0, unit='%',
                 min=0.0, max=1000.0, step=1.0)
ENVELOPE = Param('envelope_db', 'Envelope, either way', 'number', default=1.0, unit='dB',
                 min=0.1, max=40.0, step=0.5)
WINDOW = Param('window', 'Envelope window (0 is one percent of the record)', 'number',
               default=0.0, unit='s', min=0.0, step=0.1)
SRS_BELOW = Param('srs_low', 'Shock SRS, below', 'number', default=-6.0, unit='dB', min=-40.0,
                  max=0.0, step=0.5)
SRS_ABOVE = Param('srs_high', 'Shock SRS, above', 'number', default=3.0, unit='dB', min=0.1,
                  max=40.0, step=0.5)
PEAKS = Param('peak_percent', 'Major peaks', 'number', default=20.0, unit='%', min=0.1,
              max=100.0, step=5.0)


def _marks(text: str) -> list[tuple[Any, ...]]:
    """The segments typed, one per ';' or line: start and end in seconds,
    the kind, then any name."""
    out = []
    for part in re.split(r'[;\n]+', text):
        words = part.replace(',', ' ').split()
        if not words:
            continue
        if len(words) < 3:
            raise refused(f'{part.strip()!r} is not a segment: start, end and kind, as '
                          '"6 30 stationary rough road"')
        try:
            start, end = float(words[0]), float(words[1])
        except ValueError:
            raise refused(f'{part.strip()!r} does not start with two times in seconds') from None
        out.append((start, end, words[2], ' '.join(words[3:])))
    return out


def replication_segments(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.replication_segments`: one channel of a transient run over
    one playing judged segment by segment, each marked segment by the
    rule for its kind (`replication_segments.replication_segments`, which
    calls `judge_segments` on the aligned traces), beside the
    whole-record waveform error: Visual Dynamics' Segments reading. The
    marks are typed, or the ones the reference carries."""
    from hertzforge.core.replication_segments import aligned_traces, as_segments
    from hertzforge.core.replication_segments import replication_segments as judge

    run_name, ref_name = params['history'], params['reference']
    run, reference = project[run_name], project[ref_name]
    dof = _reference_dof(project, params)
    marks = (_marks(params['segments']) if params['segments'].strip()
             else list(getattr(reference, 'segments', None) or ()))
    if not marks:
        raise refused(f'no segment is marked on {ref_name}; type them, as '
                      '"6 30 stationary; 31.9 32.7 shock pothole"')
    marks = _refusing(as_segments, marks)
    low, high = params['band_low'], params['band_high']
    if high and not low < high:
        raise refused(f'the test band runs from its low edge up; {low:g} to {high:g} Hz does not')
    settings = {'taper_percent': params['taper_percent'],
                'band': (low, high) if high else None,
                'asd_db': (-params['asd_db'], params['asd_db']),
                'variance_percent': (-np.inf, params['variance_percent']),
                'envelope_db': (-params['envelope_db'], params['envelope_db']),
                'window': params['window'] or None,
                'srs_db': (params['srs_low'], params['srs_high']),
                'peak_percent': params['peak_percent']}
    playing = params['playing'] - 1
    result = _refusing(judge, run, reference, marks, channel=dof, playing=playing, **settings)
    time, target, control, _dof, _lag = aligned_traces(run, reference, dof, result.lag, playing)
    unit = unit_text(str(reference.ordinate_dim[params['record'] - 1]))
    plot = v.spectrum(f'{dof}: {result.describe()}', TIME, v.axis('Amplitude', unit, 'linear'),
                      [_trace(f'{run_name}, event {playing + 1}', 'measured', time, control),
                       _trace(ref_name, 'reference', time, target)], link='time')
    words, status = _verdict(result.passed)
    rows = [v.row('Verdict', f'{words}: {result.describe()}', None, status),
            v.row('Whole-record RMS error', _db(result.rms_error), '%', 'info')]
    for segment in result.segments:
        verdict = {True: 'pass', False: 'fail', None: 'warn'}[segment.passed]
        rows.append(v.row(segment.label, segment.describe(), None, verdict))
        rows.append(v.row(f'{segment.label}: share of the record', _db(segment.share), '%',
                          'info'))
        rows.append(v.row(f'{segment.label}: RMS error', _db(segment.rms_error), '%',
                          'info'))
        for check in segment.checks:
            rows.append(v.row(f'{segment.label}: {check.reading}', check.describe(), None,
                              {True: 'pass', False: 'fail', None: 'warn'}[check.passed]))
    return [plot, v.reading(f'Segments of {dof}, {run_name} against {ref_name}', rows)]


# ---- the operations ------------------------------------------------------------

PAIR = (SPECTRUM, SPECIFICATION)

OPERATIONS = (
    Operation(
        id='project.spec_spectra', title='Spectra', workspace='random',
        summary='One channel of a measured PSD over its specification and limits, scaled as '
                'the comparison scales it, with its RMS error and lines out.',
        params=(*PAIR, RECORD), outputs=('spectrum', 'reading'), run=spec_spectra),
    Operation(
        id='project.spec_rms_error', title='RMS Error', workspace='random',
        summary='How far each channel\'s RMS sits from the level its specification asks for, '
                'in dB, against a floor and a ceiling.',
        params=(*PAIR, ERROR_LOW, ERROR_HIGH), outputs=('reading',), run=spec_rms_error),
    Operation(
        id='project.spec_lines_out', title='Lines Out', workspace='random',
        summary='How much of each channel\'s band fell outside its abort limits, against a '
                'ceiling.',
        params=(*PAIR, LINES_CEILING), outputs=('reading',), run=spec_lines_out),
    Operation(
        id='project.spec_chance', title='Chance', workspace='random',
        summary='One channel\'s miss from its target in dB, and the lines outside each band '
                'beside how many chance alone puts there at its degrees of freedom.',
        params=(*PAIR, RECORD, COMBINE, DOF), outputs=('spectrum', 'reading'),
        run=spec_chance),
    Operation(
        id='project.spec_composite', title='Composite', workspace='random',
        summary='The control channels combined as the controller held them: the composite '
                'against its band and each channel against its own.',
        params=(*PAIR, STRATEGY, WEIGHTS, COMPOSITE_BAND, COMPOSITE_LOW, COMPOSITE_HIGH,
                INDIVIDUAL_BAND, INDIVIDUAL_LOW, INDIVIDUAL_HIGH),
        outputs=('spectrum', 'reading'), run=spec_composite),
    Operation(
        id='project.comparison_scale', title='Comparison Scaling', workspace='random',
        summary='Hold the decibels a measured PSD is scaled by when it is compared with a '
                'specification, or leave it blank to detect them; the data is not changed.',
        params=(*PAIR, SCALE), outputs=('reading',), run=comparison_scale, edits=True),
    Operation(
        id='project.spec_rms', title='Specification RMS', workspace='random',
        summary='The RMS level each channel of a specification asks for.',
        params=(SPECIFICATION,), outputs=('reading',), run=spec_rms),
    Operation(
        id='project.psd_overlay', title='Overlaid', workspace='random',
        summary='Two densities drawn over each other, channel by channel, the louder solid '
                'and the quieter dashed.',
        params=(FIRST, SECOND), outputs=('spectrum',), run=psd_overlay),
    Operation(
        id='project.signal_to_noise', title='Signal to Noise (dB)', workspace='random',
        summary='The louder density over the quieter, line by line in dB: the louder less '
                'the quieter, over the quieter.',
        params=(FIRST, SECOND), outputs=('spectrum', 'reading'), run=signal_to_noise),
    Operation(
        id='project.rms_signal_to_noise', title='RMS Signal to Noise', workspace='random',
        summary='Each channel\'s signal over its noise across the whole band, from the RMS '
                'of the two densities, against a floor.',
        params=(FIRST, SECOND, FLOOR), outputs=('reading',), run=rms_signal_to_noise),
    Operation(
        id='project.signature', title='Signature', workspace='modal',
        summary='Two signatures of one article, before and after a test, compared mode by '
                'mode: each mode\'s frequency and height shift against its limits.',
        params=(BEFORE, AFTER, SIGNATURE_RECORD, MODES, FIND, FREQUENCY_LIMIT, AMPLITUDE_LIMIT),
        outputs=('spectrum', 'reading'), run=signature),
    Operation(
        id='project.matrix_comparison', title='Matrix', workspace='random',
        summary='A cross-spectral matrix judged whole against its specification: the levels '
                'on the diagonal, each cross term\'s coherence above it and phase below it.',
        params=(SPECIFICATION, CPSD, COHERENCE_TOLERANCE, PHASE_TOLERANCE, COHERENCE_FLOOR),
        outputs=('matrix', 'reading'), run=matrix_comparison),
    Operation(
        id='project.srs_spectra', title='Shock Spectra', workspace='shock',
        summary='Every event\'s shock spectrum at one channel against the one it had to meet '
                'and its limits, with each event\'s RMS deviation in dB.',
        params=(SRS, REQUIREMENT, REQUIREMENT_RECORD), outputs=('spectrum', 'reading'),
        run=srs_spectra),
    Operation(
        id='project.srs_error', title='SRS Error', workspace='shock',
        summary='How far each channel\'s shock spectrum sits from the one it had to meet, '
                'RMS across the band in dB, against a floor and a ceiling.',
        params=(SRS, REQUIREMENT, SRS_LOW, SRS_HIGH), outputs=('reading',), run=srs_error),
    Operation(
        id='project.srs_points', title='Points Rule', workspace='shock',
        summary='One shock spectrum against its requirement\'s tolerance band point by point: '
                'how many points are inside, under the rule the test plan names.',
        params=(SRS, REQUIREMENT, SRS_POINT, RULE, PERCENT, LIMITS, INNER_LOW, INNER_HIGH,
                OUTER, OUTER_LOW, OUTER_HIGH),
        outputs=('spectrum', 'reading'), run=srs_points),
    Operation(
        id='project.replication_waveforms', title='Waveforms', workspace='transient',
        summary='One channel of a transient run over one playing of the waveform it was '
                'controlled to, aligned, with its waveform error.',
        params=(HISTORY, REFERENCE, REFERENCE_RECORD, PLAYING), outputs=('spectrum', 'reading'),
        run=replication_waveforms),
    Operation(
        id='project.waveform_error', title='Waveform Error', workspace='transient',
        summary='Every control channel\'s waveform error at every playing of the reference, '
                'against a ceiling.',
        params=(HISTORY, REFERENCE, WAVEFORM_CEILING), outputs=('reading',),
        run=waveform_error),
    Operation(
        id='project.replication_segments', title='Segments', workspace='transient',
        summary='One channel of a transient run judged segment by segment, each marked '
                'segment by the rule for its kind, beside the whole-record error.',
        params=(HISTORY, REFERENCE, REFERENCE_RECORD, PLAYING, SEGMENTS, TAPER, BAND_LOW,
                BAND_HIGH, ASD, VARIANCE, ENVELOPE, WINDOW, SRS_BELOW, SRS_ABOVE, PEAKS),
        outputs=('spectrum', 'reading'), run=replication_segments),
)
