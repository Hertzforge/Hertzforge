"""Readings of a time history that Visual Dynamics shows beside its trace:
the kurtosis of each channel, the amplitude spectrum, and what taking the
record to a lower rate does. Each reads the record and adds nothing to the
project.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .descriptors import Operation, Option, Param
from .ops_sine_shock import _record
from .protocol import refused
from .waterfalls import unit_text

HISTORY = Param('history', 'Time history', 'enum', options_from='objects:time_history')
RECORD = Param('record', 'Record', 'integer', default=1, min=1, step=1)
LOW = Param('low', 'Band, low', 'number', default=2.0, min=0.0, step=0.1)
HIGH = Param('high', 'Band, high', 'number', default=4.0, min=0.0, step=0.1)
WINDOW = Param('window', 'Window', 'enum', default='hann',
               options=tuple(Option(name, label) for name, label in (
                   ('hann', 'Hann'), ('flattop', 'Flat top'), ('boxcar', 'Rectangular'),
                   ('hamming', 'Hamming'), ('blackman', 'Blackman'),
                   ('blackmanharris', 'Blackman-Harris'))))
FACTOR = Param('factor', 'Keep every', 'integer', default=2, min=2, step=1, unit='samples')
FOLD = Param('frequencies', 'Frequencies to fold', 'string', default='', unit='Hz')


def kurtosis(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.kurtosis`: each channel's kurtosis over the span its
    spectra are read from (`core.kurtosis.channel_kurtosis`), judged
    against a band, 2 to 4 by default: Visual Dynamics' kurtosis bars.
    Every channel shares the reading whatever it measures, since kurtosis
    is dimensionless."""
    from hertzforge.core.kurtosis import analyzed_span, channel_kurtosis

    name = params['history']
    history = project[name]
    low, high = params['low'], params['high']
    if not low < high:
        raise refused(f'the band runs from its low edge up; {low:g} to {high:g} does not')
    _spans, phrase = analyzed_span(history)
    rows = []
    outside = 0
    for label, value in channel_kurtosis(history):
        if not np.isfinite(value):
            rows.append(v.row(label, 'dead: no spread to read', None, 'warn'))
            outside += 1
            continue
        inside = low <= value <= high
        outside += not inside
        rows.append(v.row(label, float(value), '', 'pass' if inside else 'warn'))
    rows.append(v.row('Outside the band', f'{outside} of {len(rows)} channels, '
                      f'band {low:g} to {high:g}', None, 'warn' if outside else 'pass'))
    return [v.reading(f'Kurtosis of {name}, {phrase}', rows)]


def amplitude_spectrum(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.amplitude_spectrum`: every channel's amplitude spectrum,
    each line scaled so a sine reads its amplitude
    (`TimeHistory.amplitude_spectrum`), on the history's own frames with
    this window, and the tallest line of one channel read off it, as
    Visual Dynamics' Amplitude reading shows them."""
    name = params['history']
    history = project[name]
    try:
        spectrum = history.amplitude_spectrum(window=params['window'])
    except ValueError as error:
        raise refused(str(error)) from None
    count = len(spectrum.response_dof)
    if params['record'] > count:
        raise refused(f'{name} has {count} channels; there is no channel {params["record"]}')
    record = params['record'] - 1
    f = np.asarray(spectrum.frequencies, dtype=float)
    lines = []
    for k in range(len(spectrum.response_dof)):
        y = np.asarray(spectrum.amplitude[k], dtype=float)
        keep = np.isfinite(y) & (y > 0) & (f > 0)
        lines.append(v.series(spectrum.label(k), 'measured', f[keep], y[keep]))
    unit = unit_text(spectrum.ordinate_dim[record])
    rows = [v.row('Channel', spectrum.label(record), None, 'info')]
    try:
        at, amplitude = spectrum.tallest(record)
        rows += [v.row('Tallest line', amplitude, unit, 'info'),
                 v.row('At', at, 'Hz', 'info')]
        note = spectrum.mirror_note(at)
        if note:
            rows.append(v.row('Note', note, None, 'warn'))
    except ValueError as error:
        rows.append(v.row('Tallest line', str(error), None, 'warn'))
    rows += [v.row('Window', spectrum.correction_text(), None, 'info'),
             v.row('Line spacing', spectrum.spacing, 'Hz', 'info'),
             v.row('Noise bandwidth', float(spectrum.noise_bandwidth), 'lines', 'info'),
             v.row('Scalloping loss', spectrum.scalloping_db, 'dB', 'info'),
             v.row('Frames averaged', float(spectrum.frames[record]), '', 'info')]
    y_unit = unit if len({spectrum.ordinate_dim[k] for k in range(len(lines))}) == 1 else None
    view = v.spectrum(f'Amplitude spectrum of {name}', v.axis('Frequency', 'Hz', 'linear'),
                      v.axis('Amplitude, peak', y_unit, 'log'), lines, link='frequency')
    return [view, v.reading(f'Amplitude of {name}, {spectrum.label(record)}', rows)]


def resample_reading(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.resample_reading`: what taking one channel to a lower
    rate does (`core.resample.read_resample`): its autospectrum as
    recorded, decimated bare, low-passed after and filtered first, the
    lines above the alias-free span grayed, and where each frequency
    above the new Nyquist frequency folds to, as Visual Dynamics'
    Resample reading shows it."""
    from hertzforge.core.resample import read_resample

    name = params['history']
    history = project[name]
    record = _record(history, params['record'], name)
    rate = float(history.sample_rate) / params['factor']
    text = params['frequencies'].replace(',', ' ').split()
    try:
        frequencies = [float(t) for t in text] or None
    except ValueError:
        raise refused(f'{params["frequencies"]!r} is not a list of frequencies in Hz') from None
    try:
        reading = read_resample(history, rate, record, frequencies=frequencies)
    except ValueError as error:
        raise refused(str(error)) from None
    looks = (('source', 'As recorded', 'reference'), ('bare', 'Decimated bare', 'abort'),
             ('after', 'Low-passed after', 'tolerance'), ('filtered', 'Filtered first',
                                                          'measured'))
    lines = []
    for key, label, role in looks:
        if key not in reading.spectra:
            continue
        f, g = (np.asarray(a, dtype=float) for a in reading.spectra[key])
        keep = np.isfinite(g) & (g > 0) & (f > 0)
        lines.append(v.series(label, role, f[keep], g[keep]))
    top = max(float(np.max(line['x'])) for line in lines) if lines else reading.nyquist
    dropped = [v.region(reading.span, max(top, reading.nyquist), 'cut',
                        f'Dropped lines, above the {reading.span:g} Hz span')]
    rows = [v.row('Rate', f'{reading.source_rate:g} Hz to {reading.rate:g} Hz', None, 'info'),
            v.row('Nyquist frequency', reading.nyquist, 'Hz', 'info'),
            v.row('Alias-free span', reading.span, 'Hz', 'info'),
            v.row('Lines kept', f'{reading.kept} of {reading.lines}', None, 'info'),
            v.row('Anti-alias corner', reading.corner, 'Hz', 'info'),
            v.row('Gain at the span', reading.gain_at_span, 'dB', 'info'),
            v.row('Gain at the lowest fold', reading.gain_at_fold, 'dB', 'info')]
    rows += [v.row('Fold', fold.describe(), None, 'warn' if fold.in_span else 'info')
             for fold in reading.folds]
    rows += [v.row('Note', note, None, 'info') for note in reading.notes]
    unit = unit_text(reading.density_dim)
    view = v.spectrum(f'{name}, {reading.label}: ' + reading.describe(),
                      v.axis('Frequency', 'Hz', 'linear'), v.axis('PSD', unit, 'log'),
                      lines, regions=dropped)
    return [view, v.reading(f'Resampling {name}, {reading.label}', rows)]


OPERATIONS = (
    Operation(
        id='project.kurtosis', title='Kurtosis', workspace='signals',
        summary='Each channel\'s kurtosis over the span its spectra are read from, judged '
                'against a band: a Gaussian record reads 3.',
        params=(HISTORY, LOW, HIGH), outputs=('reading',), run=kurtosis),
    Operation(
        id='project.amplitude_spectrum', title='Amplitude Spectrum', workspace='signals',
        summary='Every channel\'s spectrum scaled so a sine reads its peak amplitude, on '
                'the history\'s frames with a window of its own, and the tallest line of '
                'one channel.',
        params=(HISTORY, WINDOW, RECORD), outputs=('spectrum', 'reading'),
        run=amplitude_spectrum),
    Operation(
        id='project.resample_reading', title='Resample Reading', workspace='signals',
        summary='What taking one channel to a lower rate does: its spectrum as recorded, '
                'decimated bare, low-passed after and filtered first, and where content '
                'above the new Nyquist frequency folds to.',
        params=(HISTORY, FACTOR, RECORD, FOLD), outputs=('spectrum', 'reading'),
        run=resample_reading),
)
