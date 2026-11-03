"""Readings on the connection's project: numbers read off an object and
judged, as Visual Dynamics' reading panes show them, without adding
anything to the project.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .descriptors import Operation, Param
from .protocol import refused
from .waterfalls import axes, unit_text

SPECTRUM = Param('spectrum', 'Spectrum', 'enum', options_from='objects:psd')
HISTORY = Param('history', 'Time history', 'enum', options_from='objects:time_history')
RECORD = Param('record', 'Record', 'integer', default=1, min=1, step=1)
RANGED = Param('history', 'Time history', 'enum', options_from='verb:overload_check')
STEP = Param('step_db', 'Next level', 'number', default=6.0, unit='dB', min=0.0, max=40.0,
             step=1.0)
MARGIN = Param('margin', 'Margin', 'number', default=0.1, unit='%', min=0.0, max=10.0,
               step=0.05)
SHOWN = Param('record', 'Channel', 'integer', default=0, min=0, step=1)


def _record(data: Any, number: int, name: str) -> int:
    if number > data.num_records:
        raise refused(f'{name} has {data.num_records} records; there is no record {number}')
    return number - 1


def _band(value: float, band: tuple[float, float]) -> str:
    return 'pass' if band[0] <= value <= band[1] else 'warn'


def rms_check(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.rms_check`: one record of an autospectrum against the time
    history it came from (`Project.rms_check`): the RMS from the area under
    the spectrum beside the record's own, and the gap in dB judged against
    the core's threshold."""
    name = params['spectrum']
    record = _record(project[name], params['record'], name)
    try:
        check = project.rms_check(name, record)
    except ValueError as error:
        raise refused(str(error)) from None
    unit = unit_text(project[name].ordinate_dim[record].partition('**')[0])
    gap = check.gap_db
    rows = [v.row('Spectrum RMS', float(check.spectrum_rms), unit, 'info'),
            v.row('Record RMS', float(check.time_rms), unit, 'info'),
            v.row('Gap', float(gap) if np.isfinite(gap) else 'not read', 'dB',
                  'pass' if np.isfinite(gap) and abs(gap) <= check.threshold_db else 'fail')]
    if check.framed_gap_db is not None:
        rows.append(v.row('Gap to the frames averaged', float(check.framed_gap_db), 'dB', 'info'))
    rows.append(v.row('Lines summed', float(check.lines), '', 'info'))
    rows += [v.row('Note', note, None, 'info') for note in check.notes]
    if check.coverage:
        rows.append(v.row('Lines cover', check.coverage, None, 'warn'))
    return [v.reading(f'RMS check of {name}, {check.dof}', rows)]


def time_statistics(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.time_statistics`: one channel's statistics in the time
    domain, each judged against its band (`TimeHistory.time_statistics`):
    the moments, the crest factor, the tails and the kurtosis frame by
    frame."""
    name = params['history']
    data = project[name]
    record = _record(data, params['record'], name)
    try:
        stats = data.time_statistics(record)
    except ValueError as error:
        raise refused(str(error)) from None
    unit = unit_text(data.ordinate_dim[record])
    rows = [
        v.row('RMS', float(stats.rms), unit, 'info'),
        v.row('Mean', float(stats.mean), unit,
              'pass' if abs(stats.mean) <= stats.offset_share * stats.rms else 'warn'),
        v.row('Peak', float(stats.peak), unit, 'info'),
        v.row('Crest factor', float(stats.crest), '', _band(stats.crest, stats.crest_band)),
        v.row('Skewness', float(stats.skewness), '', _band(stats.skewness, stats.skewness_band)),
        v.row('Kurtosis', float(stats.kurtosis), '', _band(stats.kurtosis, stats.kurtosis_band)),
        v.row('Frames with kurtosis outside its band', float(stats.frames_outside), '',
              'pass' if not stats.frames_outside else 'warn'),
        v.row('Expected largest peak, in sigmas', float(stats.expected_peak), '', 'info'),
        v.row('Reading', stats.describe(), None, 'warn' if stats.flagged else 'pass')]
    return [v.reading(f'Time statistics of {name}, {data.record_label(record)}', rows)]


def _overload_trace(history: Any, channel: Any, title: str) -> dict:
    """One channel's trace over the span read, between its range either
    way and, where the range has held, the largest peak scaled to the next
    level: what Visual Dynamics draws above its headroom bars."""
    import hertzforge as hz
    from hertzforge.drawing.viz.waterfall import waterfall_arrays

    arrays = waterfall_arrays(history, [channel.record], unit_system=hz.SI,
                              exponents=False, component='real')
    x, y = arrays['curves'][0]
    kept = np.isfinite(x) & np.isfinite(y)
    x, y = x[kept], y[kept]
    lines = [v.series(channel.label, 'measured', x, y)]
    ends = np.array([x[0], x[-1]]) if x.size else np.zeros(2)
    if channel.has_range:
        reach = channel.full_scale
        lines += [v.series('Range', 'abort', ends, np.full(2, reach)),
                  v.series('Range, below zero', 'abort', ends, np.full(2, -reach))]
        if not channel.statistics.rail and not channel.beyond:
            # a clipped record's peak is the range, and scaled up it
            # predicts nothing
            ahead = channel.next_peak
            lines += [v.series(f'Largest peak at +{channel.step_db:g} dB', 'tolerance',
                               ends, np.full(2, ahead)),
                      v.series(f'Largest peak at +{channel.step_db:g} dB, below zero',
                               'tolerance', ends, np.full(2, -ahead))]
    return v.spectrum(title, *axes(history, channel.record, 'real', arrays), lines)


def _channel_rows(channel: Any, unit: str | None) -> list[dict]:
    """The numbers for the channel the trace draws, as Visual Dynamics'
    overload panel lists them."""
    rows = [v.row('Channel', channel.label, None, 'info')]
    if channel.statistics is None:
        return rows + [v.row('Not read', channel.reason, None, 'warn')]
    stats = channel.statistics
    rows += [v.row('Largest peak', channel.peak, unit, 'info'),
             v.row('RMS', float(channel.rms), unit, 'info'),
             v.row('Crest factor', float(channel.crest), '', 'info')]
    if not channel.has_range:
        return rows + [v.row('No range', channel.reason, None, 'warn')]
    flags = channel.flags()
    rows += [
        v.row('Range', channel.full_scale, unit, 'info'),
        v.row('Headroom', channel.headroom_db if np.isfinite(channel.headroom_db)
              else 'never leaves zero', 'dB', 'fail' if flags['rail'] else 'pass'),
        v.row('On the rail', float(stats.rail), '', 'fail' if flags['rail'] else 'pass'),
        v.row('Rail per second', float(channel.rail_rate), '', 'info'),
        v.row('Peak at next level', channel.next_peak, unit,
              'warn' if flags['next'] else 'pass'),
        v.row('Headroom there', channel.next_headroom_db
              if np.isfinite(channel.next_headroom_db) else 'never leaves zero', 'dB',
              'warn' if flags['next'] else 'pass'),
        v.row('Kurtosis', float(stats.kurtosis), '', 'info')]
    if channel.beyond:
        rows.append(v.row('Beyond the range', float(channel.beyond), '', 'fail'))
    return rows + [v.row('Note', caution, None, 'warn') for caution in channel.cautions()]


def overload(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.overload`: every channel of a time history read against
    the range its linked channel table gives it (`TimeHistory.overload_check`),
    as Visual Dynamics' Overload reading shows it: one channel's trace
    over its range, the headroom of every channel, that channel's numbers
    and the judgment in words. Channel 0 draws the worst channel."""
    name = params['history']
    history = project[name]
    found = project.channel_table_for(name)
    if found is None:
        raise refused(f'no linked channel table describes {name}: link one, and its '
                      'DAQ Range, Sensor Range and Sensitivity columns give each '
                      'channel its range')
    table_name, table = found
    try:
        check = history.overload_check(table, step_db=params['step_db'],
                                       margin=params['margin'] / 100.0)
    except ValueError as error:
        raise refused(str(error)) from None
    channels = list(check.channels)
    if params['record']:
        number = _record(history, params['record'], name)
        shown = next(c for c in channels if c.record == number)
    else:
        shown = check.worst or channels[0]
    unit = unit_text(history.ordinate_dim[shown.record])
    headroom = []
    for channel in channels:
        if channel.has_range:
            value: float | str = (channel.headroom_db if np.isfinite(channel.headroom_db)
                                  else 'never leaves zero')
            status = ('fail' if {'rail', 'beyond'} & set(channel.flagged)
                      else 'warn' if channel.flagged else 'pass')
            headroom.append(v.row(channel.label, value, 'dB', status))
        else:
            headroom.append(v.row(channel.label, 'not read' if channel.statistics is None
                                  else 'no range', None, 'info'))
    # each reason once, rather than on every channel it applies to
    reasons = [v.row('Not read' if c.statistics is None else 'No range', c.reason, None,
                     'info') for c in channels if not c.has_range]
    reasons = list({(r['label'], r['value']): r for r in reasons}.values())
    verdict = ('info' if not check.checked
               else 'fail' if any({'rail', 'beyond'} & set(c.flagged) for c in channels)
               else 'warn' if check.flagged else 'pass')
    return [_overload_trace(history, shown, f'{name}, {shown.describe(unit or "")}'),
            v.reading(f'Headroom of {name}, ranges from {table_name}',
                      headroom + reasons
                      + [v.row('Judgment', check.describe(), None, verdict)]),
            v.reading(f'{shown.label} against its range', _channel_rows(shown, unit))]


OPERATIONS = (
    Operation(
        id='project.rms_check', title='RMS Check', workspace='analyze', accepts='project',
        summary='One record of an autospectrum against the time history it came '
                'from: the RMS from the area under the spectrum beside the record\'s '
                'own, and the gap in dB.',
        params=(SPECTRUM, RECORD), outputs=('reading',), run=rms_check),
    Operation(
        id='project.time_statistics', title='Time Statistics', workspace='analyze',
        accepts='project',
        summary='One channel\'s statistics in the time domain, each judged against '
                'its band: the moments, the crest factor and the kurtosis frame by '
                'frame.',
        params=(HISTORY, RECORD), outputs=('reading',), run=time_statistics),
    Operation(
        id='project.overload', title='Overload', workspace='analyze', accepts='project',
        summary='Each channel of a time history against the range its linked channel '
                'table gives it: the headroom over its largest peak, its samples on '
                'the rail, and that peak at the next level of the test. Channel 0 '
                'draws the worst.',
        params=(RANGED, STEP, MARGIN, SHOWN), outputs=('spectrum', 'reading'),
        run=overload),
)
