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
from .waterfalls import unit_text

SPECTRUM = Param('spectrum', 'Spectrum', 'enum', options_from='objects:psd')
HISTORY = Param('history', 'Time history', 'enum', options_from='objects:time_history')
RECORD = Param('record', 'Record', 'integer', default=1, min=1, step=1)


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
)
