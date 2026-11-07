"""Readings of spectra, FRFs and cross-spectral densities that Visual
Dynamics shows beside the object they read: a tone at a frequency, the
transmissibility behind an FRF, the confidence band of an averaged PSD,
the partial coherence behind a multiple coherence, what a rigid-body
transformation leaves, the field limit over a set, the octave bands a
PSD would make, a sine tone read as a profile, and an FRF set's CMIF and
drive points. Each reads what exists and adds nothing to the project.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .descriptors import Operation, Option, Param
from .ops_project import COMPONENT
from .ops_specify import (
    BREAKPOINTS,
    CONFIDENCE,
    COVERAGE,
    MARGIN,
    OVERSHOOT,
    RMS,
    SIGMA,
    STATISTIC,
    K,
    limit_settings,
)
from .protocol import refused
from .waterfalls import unit_text

PSD = Param('spectrum', 'Spectrum', 'enum', options_from='objects:psd')
DENSITY = Param('spectrum', 'Spectrum', 'enum', options_from='objects:psd|specification')
FRF = Param('frf', 'FRF', 'enum', options_from='objects:frf')
COHERENCE = Param('coherence', 'Multiple coherence', 'enum',
                  options_from='objects:multiple_coherence')
TRANSFORMED = Param('density', 'Transformed density', 'enum', options_from='objects:psd')
MEASURED = Param('source', 'Measured', 'enum', options_from='objects:psd|srs')
SINE = Param('specification', 'Sine specification', 'enum',
             options_from='objects:sine_sweep_specification')
DIAGONAL = Param('data', 'FRF or CPSD', 'enum', options_from='objects:frf|psd')
RECORD = Param('record', 'Record', 'integer', default=1, min=1, step=1)
RECORDS = Param('records', 'Records', 'string', default='')
AT = Param('frequency', 'At', 'number', default=None, unit='Hz', min=0.0, step=1.0)
TONE_WINDOW = Param('window', 'Window', 'enum', default='averaged', options=(
    Option('averaged', 'As averaged'), Option('hann', 'Hann'), Option('flattop', 'Flat top'),
    Option('boxcar', 'Rectangular'), Option('hamming', 'Hamming'),
    Option('blackman', 'Blackman'), Option('blackmanharris', 'Blackman-Harris'),
    Option('tukey', 'Tukey'), Option('kaiser', 'Kaiser')))
WINDOW_NUMBER = Param('parameter', 'Alpha or beta', 'number', default=None, min=0.0,
                      max=40.0, step=0.05)
LEVEL = Param('level', 'Confidence', 'enum', default=0.9,
              options=(Option(0.9, '90 %'), Option(0.95, '95 %')))
DOF = Param('dof', 'Equivalent DOF', 'number', default=0.0, min=0.0, max=1.0e6, step=1.0)
BANDS = Param('per_octave', 'Spacing', 'enum', default=6, options=tuple(
    Option(n, 'full octave' if n == 1 else f'1/{n} octave') for n in (1, 2, 3, 6, 12, 24)))
TONE = Param('tone', 'Tone', 'string', default='')
CONTROL = Param('control', 'Control channel', 'string', default='')
SLOPE = Param('slope_tolerance', 'Slope within', 'number', default=0.3, unit='dB/oct',
              min=0.0, max=6.0, step=0.1)
OWN_LEVEL = Param('interpolated', "Tone's own level", 'boolean', default=True)

FREQUENCY = v.axis('Frequency', 'Hz', 'linear')
LOG_FREQUENCY = v.axis('Frequency', 'Hz', 'log')


def _record(data: Any, number: int, name: str) -> int:
    if number > data.num_records:
        raise refused(f'{name} has {data.num_records} records; there is no record {number}')
    return number - 1


def _finite(name: str, role: Any, x: Any, y: Any, positive: bool = False) -> dict:
    """A curve with the points that cannot be drawn left out: not a
    number anywhere, and nothing at or under zero on a log axis."""
    x, y = np.asarray(x, dtype=float), np.asarray(y, dtype=float)
    keep = np.isfinite(x) & np.isfinite(y)
    if positive:
        keep &= y > 0.0
    return v.series(name, role, x[keep], y[keep])


def _shaped(name: str, role: Any, data: Any, record: int) -> dict:
    """One record of a density drawn as Visual Dynamics draws it, lines
    off the log axis left out."""
    f = np.asarray(data.abscissa, dtype=float)
    y = np.abs(np.asarray(data.ordinate[record]))
    keep = np.isfinite(y) & (y > 0.0) & (f > 0.0)
    return v.shaped(name, role, data, f[keep], y[keep])


def _notes(notes: Any, status: str = 'info') -> list[dict]:
    return [v.row('Note', note[0].upper() + note[1:], None, status) for note in notes if note]


def _amplitude_unit(dimension: str) -> str | None:
    from hertzforge.core.data import channel_quantities
    from hertzforge.units import UNKNOWN

    if dimension == UNKNOWN:
        return None
    return unit_text(channel_quantities(dimension)[0])


def tone_readout(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.tone_readout`: the tone nearest a frequency read off one
    record of an autospectrum (`Psd.read_tone`), as Visual Dynamics' Tone
    Readout shows it: the amplitude and rms, the spike beside the height
    it would have on a line, the floor, the line spacing and the window's
    noise bandwidth. The window is the one the spectrum was averaged with
    (`Project.averaging_of`) unless one is chosen, and Hann, said to be
    assumed, when nothing records one. With no frequency given the
    reading starts where Visual Dynamics puts its cursor, on the tallest
    line off DC."""
    name = params['spectrum']
    psd = project[name]
    record = _record(psd, params['record'], name)
    if not psd.can_read_tone(record):
        raise refused(psd.tone_refusal(record))
    averaging = project.averaging_of(name)
    chosen = params['window']
    if chosen != 'averaged':
        window, parameter, source = chosen, params['parameter'] or None, 'chosen'
    elif averaging is not None:
        window, parameter, source = averaging, None, 'the averaging'
    else:
        window, parameter, source = 'hann', None, 'assumed'
    f = np.asarray(psd.abscissa, dtype=float)
    # blank, or 0 as the shell sends a number left empty: where VD's cursor starts
    frequency = params['frequency']
    if not frequency or not f[0] <= frequency <= f[-1]:
        values = np.abs(np.asarray(psd.ordinate[record]))[1:]
        finite = np.isfinite(values)
        frequency = (float(f[1 + int(np.argmax(np.where(finite, values, -np.inf)))])
                     if finite.any() else float(f[len(f) // 2]))
    try:
        reading = psd.read_tone(frequency, record, window=window, parameter=parameter,
                                warn=False)
    except ValueError as error:
        raise refused(str(error)) from None
    channel = str(psd.response_dof[record])
    unit = _amplitude_unit(str(psd.ordinate_dim[record]))
    density = unit_text(str(psd.ordinate_dim[record]))
    window_name = reading.window if chosen == 'averaged' else chosen
    rows = [v.row('Window from', f'{source} ({window_name})', None, 'info'),
            v.row('Channel', channel, None, 'info'),
            v.row('Tone at', reading.frequency, 'Hz', 'info'),
            v.row('Tone', float(reading.amplitude) if reading.amplitude else 'none',
                  unit if reading.amplitude else None, 'info'),
            v.row('Tone rms', reading.rms, unit, 'info'),
            v.row('Spike', float(reading.spike), density, 'info'),
            v.row('On a line', float(reading.on_line + reading.floor), density, 'info'),
            v.row('Floor', float(reading.floor), density, 'info'),
            v.row('Line spacing', float(reading.spacing), 'Hz', 'info'),
            v.row('Bandwidth', float(reading.noise_bandwidth), 'lines', 'info'),
            v.row('Off its line', abs(float(reading.offset)), 'of a line',
                  'warn' if reading.between_lines else 'info')]
    rows += _notes(reading.notes_in(unit or '', 1.0), 'warn')
    lines = [_shaped(psd.record_label(record), 'measured', psd, record)]
    floor = max(float(reading.floor), 0.0)
    top = floor + float(reading.height)
    if floor > 0.0:
        low, high = float(f[reading.lines[0]]), float(f[reading.lines[1]])
        lines += [v.series('Tone, spike above the floor', 'other',
                           [reading.frequency] * 2, [floor, top]),
                  v.series('Floor taken away', 'reference', [low, high], [floor, floor])]
    title = f'Tone readout of {name}, {channel}: ' + reading.describe(unit or '')
    view = v.spectrum(title, FREQUENCY, v.axis('PSD', density, 'log'), lines,
                      link='frequency')
    return [view, v.reading(f'Tone Readout, {channel}', rows)]


def transmissibility(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.transmissibility`: the ASD-ratio transmissibility behind
    one record of an FRF computed in the project, beside H1 and the band
    H1's random error puts about it (`Project.read_transmissibility`),
    read at the line nearest a frequency, as Visual Dynamics'
    Transmissibility reading shows it. With no frequency given the
    reading starts where Visual Dynamics puts its cursor: the tallest H1
    off DC that the input accounts for at least half of."""
    name = params['frf']
    frf = project[name]
    record = _record(frf, params['record'], name)
    try:
        reading = project.read_transmissibility(name, record)
    except ValueError as error:
        raise refused(str(error)) from None
    estimator = str(project.provenance.get(name, {}).get('params', {}).get('method', ''))
    f = np.asarray(reading.frequency, dtype=float)
    # blank, or 0 as the shell sends a number left empty: where VD's cursor starts
    frequency = params['frequency']
    if not frequency or not f[0] <= frequency <= f[-1]:
        values = reading.magnitude[1:]
        finite = np.isfinite(values) & (reading.coherence[1:] >= 0.5)
        frequency = (float(f[1 + int(np.argmax(np.where(finite, values, -np.inf)))])
                     if finite.any() else float(f[len(f) // 2]))
    point = reading.at(frequency)
    unit = None if reading.is_transmissibility else unit_text(reading.dimension)
    # two numbers and not a sentence: only a number is taken into the display units
    band = [v.row('Band, low', float(point.lower), unit, 'info')] if point.lower is not None \
        and np.isfinite(point.upper) else []
    if np.isfinite(point.upper):
        band.append(v.row('Band, high', float(point.upper), unit, 'info'))
    else:
        band = [v.row('Band', 'none', None, 'info')]
    rows = [v.row('Channel', f'{reading.response} over {reading.reference}', None, 'info'),
            v.row('FRF drawn', estimator or 'not recorded', None, 'info'),
            v.row('At', point.frequency, 'Hz', 'info'),
            v.row('H1', point.magnitude, unit, 'info'),
            v.row('H1 phase', point.phase, 'deg', 'info'),
            v.row('ASD ratio', point.asd_ratio, unit, 'info')
            if np.isfinite(point.asd_ratio) else v.row('ASD ratio', 'not read', None, 'warn'),
            v.row('Ratio to H1', point.ratio_to_h1_db, 'dB', 'info')
            if np.isfinite(point.ratio_to_h1_db)
            else v.row('Ratio to H1', 'not read', None, 'warn'),
            v.row('Coherence', point.coherence, '', 'info')
            if np.isfinite(point.coherence) else v.row('Coherence', 'not read', None, 'warn'),
            v.row('Random error', 100.0 * point.error, '%', 'info' if point.small else 'warn')
            if np.isfinite(point.error) else v.row('Random error', 'unbounded', None, 'warn'),
            *band,
            v.row('Averages', f'{reading.frames} frames, {reading.averages:.4g} independent',
                  None, 'info')]
    notes = list(reading.notes)
    if not point.small:
        notes.insert(0, 'at this line the random error is past what its expression can '
                        'size: the estimate is poor, by more than the band says')
    if estimator and estimator != 'H1':
        notes.insert(0, f'the FRF drawn is {estimator}; the band and the ratio are against '
                        'H1 from the same frames, drawn with them')
    rows += _notes(notes)
    lower_edge, upper_edge = reading.band()
    lines = [_finite(frf.record_label(record), 'measured', frf.abscissa,
                     np.abs(np.asarray(frf.ordinate[record])), positive=True)]
    if estimator != 'H1':
        lines.append(_finite('H1 = Gxy/Gxx', 'other', f, reading.magnitude, positive=True))
    lines += [_finite('ASD ratio √(Gyy/Gxx), magnitude only', 'reference', f,
                      reading.asd_ratio, positive=True),
              _finite(f'H1 - 1 random error, {reading.averages:.4g} averages', 'tolerance', f,
                      lower_edge, positive=True),
              _finite(f'H1 + 1 random error, {reading.averages:.4g} averages', 'tolerance', f,
                      upper_edge, positive=True)]
    title = (f'{reading.title}, {reading.response} over {reading.reference}: '
             + point.describe(unit or ''))
    view = v.spectrum(title, FREQUENCY, v.axis('FRF magnitude', unit, 'log'), lines,
                      link='frequency')
    return [view, v.reading(f'{reading.title} of {name}', rows)]


def confidence_band(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.confidence_band`: the chi-square band the true spectrum
    lies in about one record of an averaged autospectrum, at 90 or 95 %
    (`Psd.confidence_band`, drawn about the PSD), and what the band is
    (`Psd.confidence_reading`): the degrees of freedom and where they
    came from, and the edges in decibels and as factors. The count is the
    spectrum's own (`Psd.statistical_dof`) unless one is typed; 0 takes
    the spectrum's own."""
    from hertzforge.core.averaging import db_text, dof_text

    name = params['spectrum']
    psd = project[name]
    record = _record(psd, params['record'], name)
    level = float(params['level'])
    typed = params['dof'] or None
    refusal = psd.confidence_refusal()
    if refusal is not None:
        raise refused(refusal)
    if typed is None and psd.statistical_dof is None:
        raise refused(f'{name} does not say what it was averaged over; give the degrees '
                      'of freedom it was averaged to, from the system that computed it')
    try:
        lower, upper = psd.confidence_band(level, typed)
        band = psd.confidence_reading(level, typed)[record]
    except ValueError as error:
        raise refused(str(error)) from None
    values = np.real(np.asarray(psd.ordinate[record]))
    if not np.any(np.isfinite(values) & (values > 0.0)):
        raise refused(f'no line of {psd.record_label(record)} is finite and above zero, so '
                      'there is no band to draw or read')
    share = round(level * 100)
    channel = str(psd.response_dof[record])
    rows = [v.row('DOF from', 'typed' if typed is not None else 'the averaging', None, 'info'),
            v.row('Channel', channel, None, 'info'),
            v.row('Equivalent DOF', dof_text(band.dof), None, 'info'),
            v.row('Below', db_text(band.lower_db), None, 'info'),
            v.row('Above', db_text(band.upper_db), None, 'info'),
            v.row('Factors', f'×{band.lower_factor:.3g} to ×{band.upper_factor:.3g}', None,
                  'info'),
            v.row('Note', f'At {share} % confidence the true spectrum lies inside the band at '
                          f'{share} lines in 100; a line outside it is not by itself a fault.',
                  None, 'info')]
    density = unit_text(str(psd.ordinate_dim[record]))
    lines = [_shaped(psd.record_label(record), 'measured', psd, record),
             _shaped(f'{share} % lower bound', 'tolerance', lower, record),
             _shaped(f'{share} % upper bound', 'tolerance', upper, record)]
    view = v.spectrum(f'{name}, {channel}: ' + band.describe(), FREQUENCY,
                      v.axis('PSD', density, 'log'), lines, link='frequency')
    return [view, v.reading(f'Confidence of {name}, {channel}', rows)]


def _coherence_channel(project: Any, name: str, record: int, reading: Any) -> int:
    """Where one record of a multiple coherence is among the responses of
    the reading made from its history. The record names only a DOF, which
    can hold two quantities: its quantity is that of the history's response
    channel in the same place, as `compute_multiple_coherence` wrote them."""
    from hertzforge.core.matrix_checks import channel_labels

    history = project[project.provenance[name]['source']]
    channels = history.response_channels()
    dofs = [str(dof) for dof in project[name].response_dof]
    if [dof for dof, _quantity in channels] != dofs:
        raise refused(f'{name} reads {dofs[record]}, and the responses of {project.provenance[name]['source']} '
                      'are not the ones it was computed from; compute it again')
    labels = channel_labels([*history.reference_channels(), *channels])
    return reading.responses.index(labels[len(labels) - len(channels) + record])


def partial_coherence(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.partial_coherence`: which reference explains one response
    once the others are removed, and whether the references were
    independent (`Project.read_partial_coherence`), behind a multiple
    coherence computed in the project. Drawn as Visual Dynamics draws it,
    three plots on one frequency axis: the multiple coherence over each
    reference's partial coherence, the coherence between the references,
    and the principal references' powers as shares of the first."""
    from hertzforge.core.partial_coherence import COUPLED

    name = params['coherence']
    coherence = project[name]
    record = _record(coherence, params['record'], name)
    try:
        reading = project.read_partial_coherence(name)
    except ValueError as error:
        raise refused(str(error)) from None
    k = _coherence_channel(project, name, record, reading)
    x = np.asarray(reading.frequency, dtype=float)
    references = reading.references

    def median(values: np.ndarray) -> float:
        finite = values[np.isfinite(values)]
        return float(np.median(finite)) if finite.size else float('nan')

    medians = [median(row) for row in reading.partial[k]]
    multiple = median(reading.multiple[k])
    rows = [v.row('Response', reading.responses[k], None, 'info'),
            v.row('References', ', '.join(references), None, 'info'),
            v.row('Averages', f'{reading.averages} frames', None, 'info'),
            v.row('Multiple, median', multiple if np.isfinite(multiple) else 'undefined',
                  '' if np.isfinite(multiple) else None, 'info')]
    if np.isfinite(medians).any():
        rows.append(v.row('Explains most', references[int(np.nanargmax(medians))], None,
                          'info'))
    rows += [v.row(f'Partial with {ref}, median', value if np.isfinite(value) else 'undefined',
                   '' if np.isfinite(value) else None, 'info')
             for ref, value in zip(references, medians, strict=True)]
    pairs = [(references[i], references[j], median(reading.reference[i, j]))
             for i in range(len(references)) for j in range(i + 1, len(references))]
    pairs = [pair for pair in pairs if np.isfinite(pair[2])]
    if pairs:
        one, other, value = max(pairs, key=lambda pair: pair[2])
        rows.append(v.row('Most alike', f'{one}, {other}: {value:.2f}', None,
                          'warn' if value > COUPLED else 'info'))
    live = reading.effective_references()
    live = live[live > 0]
    if live.size:
        rows.append(v.row('Drives in effect', f'{int(np.median(live))} of {len(references)}',
                          None, 'info'))
    notes = list(reading.notes)
    coupled = reading.coupled()
    if coupled:
        one, other, share, _top = coupled[0]
        notes.append(f'{one} and {other} are coherent over {COUPLED:g} on {share:.0%} of '
                     'the lines, where the identification cannot tell them apart')
    rows += _notes(notes)

    unit_axis = v.axis('Coherence', None, 'linear')
    top = v.spectrum(f'{reading.responses[k]} coherence: ' + reading.describe(k), FREQUENCY,
                     unit_axis,
                     [_finite('multiple, every reference', 'measured', x, reading.multiple[k])]
                     + [_finite(f'partial with {ref}, the others removed', 'other', x,
                                reading.partial[k, i]) for i, ref in enumerate(references)],
                     link='frequency')
    between = [_finite(f'{references[i]} with {references[j]}', 'other', x,
                       reading.reference[i, j])
               for i in range(len(references)) for j in range(i + 1, len(references))]
    between.append(v.series(f'coupled, {COUPLED:g}', 'tolerance', x[[0, -1]],
                            [COUPLED, COUPLED]))
    middle = v.spectrum('Between references', FREQUENCY, unit_axis, between, link='frequency')
    first = reading.principal[0]
    with np.errstate(divide='ignore', invalid='ignore'):
        shares = np.where(first > 0.0, reading.principal / first, np.nan)
        decibels = np.maximum(10.0 * np.log10(shares), -60.0)
    principal = []
    for i in range(len(references)):
        virtual = reading.virtual[k, i]
        defined = virtual[np.isfinite(virtual)]
        said = (f', virtual coherence median {np.median(defined):.2f}'
                if defined.size else '')
        principal.append(_finite(f'principal {i + 1}{said}', 'measured' if i == 0 else 'other',
                                 x, decibels[i]))
    bottom = v.spectrum('Principal references', FREQUENCY,
                        v.axis('Share of principal 1', 'dB', 'linear'), principal,
                        link='frequency')
    return [top, middle, bottom,
            v.reading(f'Partial Coherence of {name}, {reading.responses[k]}', rows)]


def rigid_body_residual(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.rigid_body_residual`: what a rigid-body transformation
    leaves unexplained of the density it was applied to
    (`Project.read_transformation`), as Visual Dynamics' Rigid-Body
    Residual shows it: the measured power at the channels the transform
    used, the part the six motions explain and the residual, the
    residual's share against frequency, and the transformation's
    conditioning and the frequency the rigid body holds to."""
    from hertzforge.core.transformation import MAX_CONDITION

    name = params['density']
    try:
        reading = project.read_transformation(name)
    except ValueError as error:
        raise refused(str(error)) from None
    t = reading.transformation
    condition = float(t.condition)
    singular = t.singular_values
    rows = [v.row('Channels', f'{len(t.inputs)} to {len(t.outputs)} motions', None, 'info'),
            v.row('Conditioning', condition if np.isfinite(condition) else 'infinite',
                  '' if np.isfinite(condition) else None,
                  'info' if condition <= MAX_CONDITION else 'warn'),
            v.row('Singular values', f'{singular[0]:.3g} to {singular[-1]:.3g}', None, 'info'),
            v.row('Length scale', float(t.length_scale), 'm', 'info')]
    if np.isfinite(reading.overall):
        holds = reading.holds_to
        rows += [v.row('Residual, band', 100.0 * float(reading.overall), '%',
                       'pass' if reading.overall <= reading.threshold else 'warn'),
                 v.row('Holds to', 'every line' if reading.holds_throughout
                       else float(holds) if holds is not None else 'not at the first line',
                       'Hz' if holds is not None and not reading.holds_throughout else None,
                       'pass' if reading.holds_throughout else 'info')]
    rows.append(v.row('Threshold', f'{reading.threshold:.0%} of the power', None, 'info'))
    worst = reading.worst_channel
    if worst is not None:
        share = reading.channel_fractions[reading.channels.index(worst)]
        rows.append(v.row('Worst channel', f'{worst}, {share:.1%}', None, 'info'))
    notes = []
    if not condition <= MAX_CONDITION:
        notes.append(f'the conditioning is past {MAX_CONDITION:g}: an error at the channels '
                     'reaches the motions that many times larger, and building the '
                     'transformation is refused there unless the limit is raised')
    rows += _notes(notes + list(reading.notes))
    x = np.asarray(reading.frequency, dtype=float)
    unit = unit_text(reading.dimension)
    spectra = v.spectrum(
        f'{t.describe()}: {reading.describe()}', LOG_FREQUENCY, v.axis('PSD', unit, 'log'),
        [_finite('measured, summed over the inputs', 'measured', x, reading.total, True),
         _finite('explained by the outputs', 'tolerance', x,
                 np.sum(reading.explained, axis=0), True),
         _finite('residual, what the outputs leave', 'reference', x,
                 np.sum(reading.residual, axis=0), True)], link='frequency')
    keep = x > 0.0
    share_lines = [_finite('residual share of the measured power', 'measured', x[keep],
                           reading.fraction[keep]),
                   v.series(f'threshold, {reading.threshold:.0%}', 'tolerance',
                            x[keep][[0, -1]], [reading.threshold] * 2)]
    holds = reading.holds_to
    if holds is not None and holds > 0.0 and not reading.holds_throughout:
        share_lines.append(v.series(f'holds to {holds:.4g} Hz', 'reference', [holds, holds],
                                    [0.0, 1.0]))
    share = v.spectrum('Residual share', LOG_FREQUENCY, v.axis('Residual share', None,
                                                               'linear'),
                       share_lines, link='frequency')
    return [spectra, share, v.reading(f'Rigid-Body Residual of {name}', rows)]


def field_limit(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.field_limit`: the statistical limit over a set of
    measured PSDs or SRSs and the straight-line envelope over it
    (`Project.field_limit`), drawn with the set, the mean in decibels and
    the envelope raised by the margin, and read out as Visual Dynamics'
    Field Limit panel reads it. Nothing is added: Derive Specification
    writes the specification shown. The records are one quantity: blank,
    the largest group of the source's records."""
    from hertzforge.drawing.plot import as_power_law
    from hertzforge.drawing.viz.waterfall import waterfall_group

    from .ops_project import chosen_records

    name = params['source']
    data = project[name]
    named = (chosen_records(params['records'], data.num_records)
             if params['records'].strip() else None)
    records, _left = waterfall_group(data, named)
    if not records:
        raise refused(f'{name} has no records to take a limit over')
    try:
        fit = project.field_limit(name, records, **limit_settings(params))
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    limit = fit.limit
    fieldset = limit.fieldset
    margin = params['margin_db']
    unit = unit_text(fieldset.dim)
    rows = [v.row('Limit', limit.describe(), None, 'info'),
            v.row('Records', float(limit.count), '', 'info')]
    if limit.factor is not None:
        rows.append(v.row('Factor k', float(limit.factor), '', 'info'))
    if limit.sigma_db is not None:
        rows.append(v.row('Scatter (median)', f'{limit.sigma_db:g} dB stated', None, 'info'))
    elif limit.scatter_db is not None:
        rows.append(v.row('Scatter (median)', float(np.median(limit.scatter_db)), 'dB',
                          'info'))
    rows += [v.row('Breakpoints', f'{fit.breakpoints}' + ('' if fit.met else ', the most'),
                   None, 'info' if fit.met else 'warn'),
             v.row('Most over limit', float(fit.overshoot_db), 'dB', 'info')]
    if limit.kind == 'psd':
        rms_unit = unit_text(fieldset.dim.partition('**')[0])
        rows += [v.row('Limit rms', float(fit.limit_rms), rms_unit, 'info'),
                 v.row('Envelope rms', float(fit.envelope_rms), rms_unit, 'info'),
                 v.row('Area added', f'{fit.added_db:+.2f} dB' if fit.rule == 'exceed'
                       else f'kept; {fit.below} lines under', None, 'info')]
    if margin:
        rows.append(v.row('Margin', float(margin), 'dB', 'info'))
    rows.append(v.row('Rests on', limit.rests_on[0].upper() + limit.rests_on[1:], None,
                      'info'))
    rows += _notes(fit.notes)
    f = np.asarray(fieldset.frequencies, dtype=float)
    lines = [_finite(label, 'drive', f, row, True)
             for label, row in zip(fieldset.labels, fieldset.levels, strict=True)]
    lines += [_finite('mean in dB', 'tolerance', f,
                      10.0 ** (limit.mean_db / fieldset.db_scale), True),
              _finite(limit.describe(), 'reference', f, limit.levels, True)]

    def envelope(label: str, role: Any, levels: Any) -> dict:
        x, y = as_power_law(np.asarray(fit.frequencies, dtype=float),
                            np.asarray(levels, dtype=float))
        return v.series(label, role, x, y, 'law')

    lines.append(envelope(f'envelope, {fit.breakpoints} breakpoints'
                          + ('' if fit.rule == 'exceed' else ', rms kept'), 'measured',
                          fit.levels))
    if margin:
        lines.append(envelope(f'with {margin:+g} dB margin', 'other', fit.margined(margin)))
    quantity = 'PSD' if limit.kind == 'psd' else 'SRS'
    view = v.spectrum(f'{limit.describe()}; {fit.describe()}', LOG_FREQUENCY,
                      v.axis(quantity, unit, 'log'), lines, link='frequency')
    return [view, v.reading(f'Field Limit over {limit.count} records of {name}', rows)]


def octave_bands(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.octave_bands`: the banded spectrum Compute Octave Bands
    would make (`Psd.to_octave`), previewed as steps over the narrowband
    record it integrates, with how many bands the spacing gives, as
    Visual Dynamics' Octave Bands panel previews it before Apply."""
    name = params['spectrum']
    psd = project[name]
    record = _record(psd, params['record'], name)
    per_octave = int(params['per_octave'])
    try:
        banded = psd.to_octave(per_octave)
    except ValueError as error:
        raise refused(str(error)) from None
    values = np.abs(np.asarray(banded.ordinate[record], dtype=complex)).real
    spacing = 'full octave' if per_octave == 1 else f'1/{per_octave} octave'
    rows = [v.row('Spacing', spacing, None, 'info'),
            v.row('Bands', float(len(banded.abscissa)), '', 'info'),
            v.row('Grid', 'ANSI S1.11, base 10', None, 'info'),
            v.row('From', float(banded.bin_edges()[0]), 'Hz', 'info'),
            v.row('To', float(banded.bin_edges()[-1]), 'Hz', 'info')]
    density = unit_text(str(psd.ordinate_dim[record]))
    lines = [_shaped(psd.record_label(record), 'measured', psd, record),
             v.series(f'{spacing} bands', 'reference', banded.abscissa,
                      np.where(np.isfinite(values), values, 0.0), 'steps',
                      banded.bin_edges())]
    view = v.spectrum(f'{name}, {psd.record_label(record)}, in {spacing} bands',
                      LOG_FREQUENCY, v.axis('PSD', density, 'log'), lines, link='frequency')
    return [view, v.reading(f'Octave Bands of {name}', rows)]


def sine_profile(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.sine_profile`: one tone of a sine specification read back
    as segments of constant displacement, velocity and acceleration
    (`core.sine_profile.read_profile`), drawn three ways against
    frequency with the tone's own level as it interpolates its
    breakpoints (`core.sine_profile.interpolated`), and every segment,
    crossover and maximum read out, as Visual Dynamics' Sine Profile
    shows it."""
    from hertzforge.core.sine_profile import convert, interpolated, read_profile

    name = params['specification']
    spec = project[name]
    tone = params['tone'].strip() or None
    control = params['control'].strip() or None
    try:
        profile = read_profile(spec, tone, control,
                               slope_tolerance=params['slope_tolerance'])
        level = interpolated(spec, tone, control) if params['interpolated'] else None
    except ValueError as error:
        raise refused(str(error)) from None
    tone_name = tone or spec.tones[0].name
    low, high = profile.band
    f = np.unique(np.concatenate([np.geomspace(low, high, 400), profile.bounds]))
    crossings = profile.crossings
    bad = sum(not crossing.checks for crossing in crossings)
    maxima = profile.maxima()
    rows = [v.row('Segments', float(len(profile.segments)), '', 'info'),
            v.row('Crossovers', 'none, one segment' if not crossings
                  else f'{bad} of {len(crossings)} do not check' if bad
                  else f'{len(crossings)}, every one checks', None,
                  'warn' if bad else 'pass')]
    units = {'displacement': ('m', ' peak to peak'), 'velocity': ('m/s', ''),
             'acceleration': ('m/s²', '')}
    for quantity, (unit, convention) in units.items():
        at, value = maxima[quantity]
        rows.append(v.row(f'Most {quantity}{convention}', float(value), unit, 'info'))
        rows.append(v.row(f'Most {quantity}, at', float(at), 'Hz', 'info'))
    for line in profile.describe()[:-1]:
        label = 'Crossover' if line.startswith(('crossover', 'joined', 'step')) else 'Segment'
        status = 'warn' if 'does not check' in line or line.startswith('step') else 'info'
        rows.append(v.row(label, line, None, status))
    views = []
    for quantity, (unit, convention) in units.items():
        dimension = 'peak to peak' if quantity == 'displacement' else 'peak'
        lines = [_finite(f'{tone_name}, read as a profile', 'measured', f,
                         profile.level(f, quantity, dimension), True)]
        if level is not None:
            lf, la = (np.asarray(a, dtype=float) for a in level)
            shown = convert(la, lf, 'acceleration', quantity)
            if quantity == 'displacement':
                shown = 2.0 * shown
            lines.append(_finite('as the tone interpolates its breakpoints', 'reference', lf,
                                 shown, True))
        at, value = maxima[quantity]
        views.append(v.spectrum(
            f'{quantity}: most {value:.4g} {unit}{convention} at {at:.4g} Hz', LOG_FREQUENCY,
            v.axis(quantity.capitalize() + convention, unit, 'log'), lines,
            link='frequency'))
    return [*views, v.reading(f'Sine Profile of {name}, {tone_name}', rows)]


def cmif(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.cmif`: the Complex Mode Indicator Function of an FRF set
    outside a fit: the singular values of the response-by-reference
    matrix at every line, a missing pair taken as zero
    (`drawing.plot.cmif_curves`, the matrix of
    `core.matrix_checks.frf_matrix`), one curve per singular value, as
    Visual Dynamics' CMIF toggle draws it. Values numerically zero are
    left out. Records "1-8"; blank for every record."""
    import hertzforge as hz
    from hertzforge.drawing.plot import cmif_curves

    from .ops_project import chosen_records

    name = params['frf']
    frf = project[name]
    records = (chosen_records(params['records'], frf.num_records)
               if params['records'].strip() else None)
    try:
        singular, x = cmif_curves(frf, records, hz.SI)
    except ValueError as error:
        raise refused(str(error)) from None
    if not np.isfinite(singular).any():
        raise refused(f'no line of {name} is a finite matrix to decompose')
    floor = np.nanmax(singular) * 1e-10
    lines = [_finite(f'CMIF {k + 1}', 'measured', x, singular[k], True)
             for k in range(singular.shape[0]) if np.nanmax(singular[k]) > floor]
    unit = unit_text(str(frf.ordinate_dim[0]))
    return [v.spectrum(f'CMIF of {name}: {len(lines)} singular value '
                       f'curve{"s" * (len(lines) != 1)}', FREQUENCY,
                       v.axis('CMIF', unit, 'log'), lines, link='frequency')]


def drive_points(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.drive_points`: only the records of an FRF set whose
    response and reference are one DOF, its drive points, or the
    autospectra of a cross-spectral matrix, drawn as Visual Dynamics'
    Drive points (Autospectra) toggle selects them."""
    import hertzforge as hz
    from hertzforge.core.data import Psd, autospectrum
    from hertzforge.drawing.viz.waterfall import waterfall_arrays

    from .waterfalls import axes

    name = params['data']
    data = project[name]
    what = 'autospectra' if isinstance(data, Psd) else 'drive points'
    if data.reference_dof is None:
        raise refused(f'{name} has no references: every record is an autospectrum')
    # a drive point of an FRF is a ratio of two quantities at one DOF; in a
    # density, the same two are a cross term
    records = [i for i in range(data.num_records)
               if (autospectrum(data, i) if isinstance(data, Psd)
                   else data.response_dof[i] == data.reference_dof[i])]
    if not records:
        raise refused(f'{name} has no {what}: no record has its response and reference '
                      'at one DOF')
    arrays = waterfall_arrays(data, records, unit_system=hz.SI, exponents=False,
                              component=params['component'])
    lines = []
    for record, (x, y) in zip(arrays['drawn'], arrays['curves'], strict=True):
        kept = np.isfinite(x) & np.isfinite(y)
        lines.append(v.series(data.record_label(record), 'measured', x[kept], y[kept]))
    left = len(records) - len(arrays['drawn'])
    title = f'{what.capitalize()} of {name}: {len(records)} of {data.num_records} records'
    if left:
        title += f' ({left} of another quantity left out)'
    return [v.spectrum(title, *axes(data, arrays['drawn'][0], params['component'], arrays),
                       lines, link='frequency')]


OPERATIONS = (
    Operation(
        id='project.tone_readout', title='Tone Readout', workspace='sine',
        summary='The tone nearest a frequency read off one record of an autospectrum: its '
                'amplitude beside the spike it stands at, the floor and the window. '
                'Blank starts at the tallest line.',
        params=(PSD, RECORD, AT, TONE_WINDOW, WINDOW_NUMBER),
        outputs=('spectrum', 'reading'), run=tone_readout),
    Operation(
        id='project.transmissibility', title='Transmissibility', workspace='random',
        summary='The ratio of the two autospectra behind one record of an FRF computed '
                'here, beside H1 and the band its random error puts about it, read at a '
                'line. Blank starts at the resonance.',
        params=(FRF, RECORD, AT), outputs=('spectrum', 'reading'), run=transmissibility),
    Operation(
        id='project.confidence_band', title='Confidence', workspace='random',
        summary='The chi-square band the true spectrum lies in about one record of an '
                'averaged PSD, at 90 or 95 %. A count of 0 takes the spectrum\'s own.',
        params=(PSD, RECORD, LEVEL, DOF), outputs=('spectrum', 'reading'),
        run=confidence_band),
    Operation(
        id='project.partial_coherence', title='Partial Coherence', workspace='random',
        summary='Which reference explains a response once the others are removed, how '
                'alike the references are, and how many drives the test had in effect.',
        params=(COHERENCE, RECORD), outputs=('spectrum', 'reading'), run=partial_coherence),
    Operation(
        id='project.rigid_body_residual', title='Rigid-Body Residual', workspace='modal',
        summary='What the six rigid-body motions leave unexplained of a density '
                'transformed through rigid shapes, and the frequency they hold to.',
        params=(TRANSFORMED,), outputs=('spectrum', 'reading'), run=rigid_body_residual),
    Operation(
        id='project.field_limit', title='Field Limit', workspace='specify',
        summary='The statistical limit over a set of measured PSDs or SRSs and the '
                'straight-line envelope over it, before a specification is derived. '
                'A stated deviation of 0 is estimated from the records.',
        params=(MEASURED, RECORDS, STATISTIC, COVERAGE, CONFIDENCE, K, SIGMA, OVERSHOOT,
                BREAKPOINTS, RMS, MARGIN),
        outputs=('spectrum', 'reading'), run=field_limit),
    Operation(
        id='project.octave_bands', title='Octave Bands', workspace='random',
        summary='The octave bands a spectrum would be integrated onto, previewed as steps '
                'over the narrowband record, before Compute Octave Bands makes them.',
        params=(DENSITY, RECORD, BANDS), outputs=('spectrum', 'reading'), run=octave_bands),
    Operation(
        id='project.sine_profile', title='Sine Profile', workspace='sine',
        summary='A sine tone read back as segments of constant displacement, velocity and '
                'acceleration, every crossover checked. Blank tone and channel take the '
                'first.',
        params=(SINE, TONE, CONTROL, SLOPE, OWN_LEVEL), outputs=('spectrum', 'reading'),
        run=sine_profile),
    Operation(
        id='project.cmif', title='CMIF', workspace='modal',
        summary='The singular values of an FRF matrix at every line; peaks mark the '
                'modes. Records "1-8"; blank for every record.',
        params=(FRF, RECORDS), outputs=('spectrum',), run=cmif),
    Operation(
        id='project.drive_points', title='Drive Points', workspace='modal',
        summary='Only the FRFs whose response and reference are one DOF, or the '
                'autospectra of a cross-spectral matrix.',
        params=(DIAGONAL, COMPONENT), outputs=('spectrum',), run=drive_points),
)
