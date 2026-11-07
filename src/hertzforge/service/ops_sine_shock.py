"""Sine and shock readings of a time history, as Visual Dynamics shows
them beside the record: the sine extraction's settings and the levels
against their specification, the distortion, dwell, frequency and
mixed-mode readouts under them, and the classical pulse, stroke,
replication fit and scalogram readings of a shock record. Each reads the
record and adds nothing to the project; the sine extraction's settings
and the shock windows are stored on the record, as Visual Dynamics'
views store them.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .descriptors import Operation, Option, Param
from .protocol import refused
from .waterfalls import unit_text

HISTORY = Param('history', 'Time history', 'enum', options_from='objects:time_history')
SWEEP = Param('specification', 'Sine specification', 'enum',
              options_from='objects:sine_sweep_specification')
LEVELS = Param('levels', 'Sine levels', 'enum', options_from='objects:sine_level_set')
FLOOR = Param('floor', 'Random specification', 'enum', options_from='objects:specification')
TONE = Param('tone', 'Tone', 'integer', default=1, min=1, step=1)
RECORD = Param('record', 'Record', 'integer', default=1, min=1, step=1)
CONTROL = Param('record', 'Control channel', 'integer', default=1, min=1, step=1)
#: a record counted from 1, or 0 for the one the reading picks itself
CHOSEN = Param('record', 'Record, 0 for the reading\'s own', 'integer', default=0, min=0,
               step=1)

#: the most points a trace is drawn with: peak-held, so no extreme is lost
TRACE_POINTS = 4000
#: the scalogram's time columns: a peak hold to about a picture's width
SCALOGRAM_COLUMNS = 400


def _record(data: Any, number: int, name: str) -> int:
    if number > data.num_records:
        raise refused(f'{name} has {data.num_records} records; there is no record {number}')
    return number - 1


def _chosen(data: Any, number: int, name: str) -> int | None:
    """A record counted from 1 as an index, or None for 0."""
    return None if number == 0 else _record(data, number, name)


def _nth(names: list[str], number: int, what: str, owner: str) -> str:
    if number > len(names):
        raise refused(f'{owner} holds {len(names)} {what}{"s" * (len(names) != 1)}; '
                      f'there is no {what} {number}')
    return names[number - 1]


def _trace(x: Any, y: Any) -> tuple[np.ndarray, np.ndarray]:
    from hertzforge.decimate import peak_decimate

    return peak_decimate(np.asarray(x, dtype=float), np.asarray(y, dtype=float),
                         TRACE_POINTS)


def _flat(name: str, role: Any, x0: float, x1: float, y: float) -> dict:
    return v.series(name, role, [x0, x1], [y, y])


def _positive(name: str, role: Any, x: Any, y: Any) -> dict:
    x = np.asarray(x, dtype=float)
    y = np.asarray(y, dtype=float)
    keep = np.isfinite(y) & (y > 0) & (x > 0)
    return v.series(name, role, x[keep], y[keep])


def _verdict(passed: bool | None) -> str:
    return {True: 'pass', False: 'fail', None: 'info'}[passed]


# ---- the sine extraction and the levels -------------------------------------------------------

SMOOTHING = Param('smoothing', 'Smoothing', 'enum', default='automatic', options=(
    Option('automatic', 'Automatic'), Option('set', 'Set')))
CYCLES = Param('cycles', 'Cycles', 'number', default=10.0, min=10.0, max=640.0, step=10.0)
TARGET = Param('target', 'Scatter under', 'number', default=1.0, min=0.1, max=6.0, step=0.5,
               unit='dB')
REFINE = Param('refine', 'Follow the clock', 'boolean', default=True)


def sine_extraction(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.sine_extraction`: the settings a sweep's levels are read
    out of a record with, stored on the record as Visual Dynamics' sine
    view stores them, and what follows from them: the cycles the
    automatic smoothing chose (`core.sine.suggest_cycles`), the
    smoothing window at the first tone's start and end, the median
    scatter a reading is predicted to have and the share of samples under
    the noise floor, read off the levels sampled at that smoothing
    (`core.sine.sample_levels`), which are drawn against frequency.
    Extract Sine then reads the levels with these settings."""
    from dataclasses import replace

    from hertzforge.core.sine import SineExtraction, sample_levels, suggest_cycles

    name, spec_name = params['history'], params['specification']
    history, spec = project[name], project[spec_name]
    automatic = params['smoothing'] == 'automatic'
    try:
        rate = float(history.sample_rate)
        setting = SineExtraction(cycles=None if automatic else params['cycles'],
                                 target_db=params['target'], refine=params['refine'])
        cycles = (suggest_cycles(history, spec, setting.target_db) if automatic
                  else float(params['cycles']))
        sampled = sample_levels(history, spec, cycles)
    except ValueError as error:
        raise refused(str(error)) from None
    history.sine_extraction = replace(setting, chosen=cycles if automatic else None)
    project.record_setting(name, 'sine_extraction', history.sine_extraction)
    rows = [v.row('Smoothing', history.sine_extraction.describe(), None, 'info')]
    if automatic:
        rows.append(v.row('Cycles used', cycles, 'cycles', 'info'))
    _t, f = spec.tones[0].trajectory(1.0 / rate)
    first, last = cycles / max(float(f[0]), 1e-9), cycles / max(float(f[-1]), 1e-9)
    rows.append(v.row('Window, start to end', f'{first:.2f} s to {last:.2f} s', None, 'info'))
    scatter = np.concatenate([np.ravel(r['scatter_db']) for r in sampled])
    finite = scatter[np.isfinite(scatter)]
    rows.append(v.row('Predicted scatter', float(np.median(finite)), 'dB', 'info')
                if finite.size else v.row('Predicted scatter', 'under the floor', None, 'warn'))
    under = np.concatenate([np.ravel(r['amplitude'] <= r['floor']) for r in sampled])
    rows.append(v.row('Under the floor', f'{100.0 * under.mean():.0f} % of samples'
                      if under.size else 'no samples', None, 'info'))
    lines = []
    for reading in sampled:
        frequency = np.asarray(reading['frequency'], dtype=float)
        amplitude = np.asarray(reading['amplitude'], dtype=float)
        floor = np.asarray(reading['floor'], dtype=float)
        for k, dof in enumerate(spec.response_dof[:amplitude.shape[0]]):
            hidden = amplitude[k] <= floor[k]
            lines.append(_positive(f'{reading["tone"]} {dof}', 'measured',
                                   frequency[~hidden], amplitude[k][~hidden]))
            if hidden.any():
                lines.append(_positive(f'{reading["tone"]} {dof}, floor where under', 'other',
                                       frequency[hidden], floor[k][hidden]))
    view = v.spectrum(f'Sine levels of {name}, sampled at {cycles:g} cycles',
                      v.axis('Frequency', 'Hz', 'log'),
                      v.axis('Amplitude', unit_text(spec.ordinate_dim)
                             if spec.ordinate_unit is not None else None, 'log'),
                      lines, link='frequency')
    return [view, v.reading(f'Sine Levels of {name} against {spec_name}', rows)]


THRESHOLD = Param('threshold', 'Threshold', 'number', default=3.0, min=0.1, step=0.5, unit='dB')


def sine_dofs(grouped: Any) -> list[str]:
    """The control DOFs a sine specification or a level set covers, in
    order: the specification's own, or the union of its levels', since a
    tone the instructions windowed may have been read at fewer DOFs (VD's
    `record_grid.sine_dofs`)."""
    if hasattr(grouped, 'tones'):
        return [str(dof) for dof in grouped.response_dof]
    out: list[str] = []
    for level in grouped.levels:
        for dof in level.response_dof:
            if str(dof) not in out:
                out.append(str(dof))
    return out


def sine_lines(spec: Any, tone: str, levels: list[Any], dofs: list[str] | None = None
               ) -> tuple[list[dict], str | None]:
    """The flat reading of one tone: each level of it at each control
    channel (measured), over the tone's requirement and its warning and
    abort limits from `spec` when there is one; (lines, amplitude unit).
    `dofs` restricts the channels drawn, as picks in the tree do."""
    lines: list[dict] = []
    unit: str | None = None
    wanted = None if dofs is None else set(dofs)
    for level in levels:
        if level.tone != tone:
            continue
        for i, dof in enumerate(level.response_dof):
            dof = str(dof)
            if wanted is not None and dof not in wanted:
                continue
            if spec is not None and dof not in spec.response_dof:
                continue
            lines.append(_positive(f'{tone} level, {dof}', 'measured', level.abscissa,
                                   np.abs(level.ordinate[i])))
            if unit is None and level.ordinate_unit is not None and level.ordinate_unit[i]:
                unit = unit_text(level.ordinate_dim[i])
    if spec is not None:
        unit = unit_text(spec.ordinate_dim) if spec.ordinate_unit is not None else None
        target = spec.tone_curve(tone)
        looks = (('ordinate', 'requirement', 'reference'),
                 ('warning_lower', 'warning', 'tolerance'),
                 ('warning_upper', 'warning', 'tolerance'),
                 ('abort_lower', 'abort', 'abort'), ('abort_upper', 'abort', 'abort'))
        for k, dof in enumerate(spec.response_dof):
            if wanted is not None and str(dof) not in wanted:
                continue
            for attribute, words, role in looks:
                values = getattr(target, attribute, None)
                if values is not None and np.isfinite(np.asarray(values)[k]).any():
                    lines.append(_positive(f'{tone} {words}, {dof}', role, target.abscissa,
                                           np.abs(np.asarray(values)[k])))
    return lines, unit


def sine_error_rows(spec: Any, levels: list[Any], threshold: float,
                    dofs: list[str] | None = None) -> list[dict]:
    """The Level Error reading's rows: each channel's signed RMS deviation
    in dB across the sweep, per tone (`core.compliance.sine_errors`),
    judged against `threshold`; a pair that cannot be scored keeps its row
    with the reason. `dofs` restricts the channels, as picks do."""
    from hertzforge.core.compliance import comparison_note, sine_errors

    tones = [tone.name for tone in spec.tones]
    errors = sine_errors(spec, levels)
    if dofs is not None:
        errors = [error for error in errors if error.dof in dofs]
    rows = []
    for error in errors:
        label = f'{error.dof} {error.tone}' if len(tones) > 1 else error.dof
        if error.refused:
            rows.append(v.row(label, f'not scored: {error.refused}', None, 'warn'))
        elif np.isfinite(error.db):
            rows.append(v.row(label, float(error.db), 'dB',
                              'pass' if abs(error.db) <= threshold else 'fail'))
    if not rows:
        raise refused('nothing here can be scored')
    rows.append(v.row('Threshold', f'±{threshold:g} dB, RMS across the sweep', None, 'info'))
    note = comparison_note(errors)
    if note:
        rows.append(v.row('Note', note, None, 'warn'))
    return rows


def level_error(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.level_error`: extracted sine levels against the
    specification they were controlled to, Visual Dynamics' Sine view.
    The Levels reading draws one tone's levels at each control channel
    over its requirement (`SineSweepSpecification.tone_curve`); the Level
    Error reading is each channel's signed RMS deviation in dB across the
    sweep, per tone (`core.compliance.sine_errors`), judged against the
    threshold either way."""
    spec_name, levels_name = params['specification'], params['levels']
    spec, levels = project[spec_name], project[levels_name]
    tones = [tone.name for tone in spec.tones]
    matched = [level for level in levels.levels if level.tone in tones]
    if not matched:
        raise refused(f'{levels_name} holds no tone of {spec_name} '
                      f'({", ".join(tones)})')
    tone = _nth(tones, params['tone'], 'tone', spec_name)
    lines, unit = sine_lines(spec, tone, matched)
    levels_view = v.spectrum(f'{tone}: the extracted levels against the requirement of '
                             f'{spec_name}', v.axis('Frequency', 'Hz', 'log'),
                             v.axis('Amplitude', unit, 'log'), lines, link='frequency')
    rows = sine_error_rows(spec, matched, params['threshold'])
    return [levels_view, v.reading(f'Level Error of {levels_name} against {spec_name}', rows)]


# ---- the readouts under the levels ------------------------------------------------------------

def _extracted(project: Any, history: str, specification: str) -> tuple[str, Any]:
    """The sine levels most recently extracted from `history` against
    `specification`, by the project's record of what each object was made
    from, as Visual Dynamics' readouts find them; refused when there are
    none, or when they are stale."""
    for name in reversed(list(project.names)):
        made = project.provenance.get(name, {})
        if (made.get('verb') == 'extract_sine' and made.get('source') == history
                and (made.get('params') or {}).get('specification') == specification):
            stale = project.stale().get(name)
            if stale:
                raise refused(f'These levels are stale ({stale}); extract them again to '
                              'check the record against them.')
            return name, project[name]
    raise refused(f'no sine levels have been extracted from {history} against '
                  f'{specification}; Extract Sine reads them')


def _readout_rows(check: Any, dof: str, label: str, over_words: str, inside_words: str,
                  specified: np.ndarray) -> list[dict]:
    """The rows the distortion and frequency readouts share after the
    worst line: the lines over, under `label`, and the verdict, as Visual
    Dynamics words them."""
    row = check.row(dof)
    over = int(check.over[row].sum())
    judged = int(check.judged[row].sum())
    if check.worst(dof) is None:
        return [v.row(label, 'no line judged', None, 'info'),
                v.row('Verdict', 'not judged', None, 'info')]
    if over:
        span = specified[check.over[row]]
        return [v.row(label, f'{over} of {judged} lines, {span.min():.4g} to '
                      f'{span.max():.4g} Hz', None, 'fail'),
                v.row('Verdict', over_words, None, 'fail')]
    return [v.row(label, f'none of {judged} lines', None, 'pass'),
            v.row('Verdict', inside_words, None, 'pass')]


EXCESS = Param('excess', 'Unfiltered over tone', 'number', default=5.0, min=0.1, max=50.0,
               step=1.0, unit='%')


def distortion(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.sine_distortion`: what a sine record carries besides its
    tones (`core.sine_distortion.check_distortion`), read from the levels
    last extracted from it: the RMS of the record less the tracked tones
    against the RMS of the tones at every line of the sweep, as a
    distortion in percent, against the limit a tolerance on their ratio
    sets, on one control channel."""
    from hertzforge.core.sine_distortion import check_distortion, decibels

    name, spec_name = params['history'], params['specification']
    _levels_name, levels = _extracted(project, name, spec_name)
    tone = _nth(list(levels.tone_names), params['tone'], 'tone', 'the levels')
    dof = _nth([str(d) for d in levels.response_dof], params['record'], 'control channel',
               'the levels')
    try:
        check = check_distortion(project[name], project[spec_name], levels, tone=tone,
                                 excess=params['excess'] / 100.0)
        row = check.row(dof)
    except (ValueError, KeyError) as error:
        raise refused(str(error).strip('\'"')) from None
    limit = check.limit
    rows = [v.row('Distortion limit', f'{100.0 * limit:.1f} % '
                  f'({float(decibels(limit)):.1f} dB)', None, 'info')]
    worst = check.worst(dof)
    if worst is not None:
        at, value = worst
        rows.append(v.row('Worst', f'{100.0 * value:.1f} % ({float(decibels(value)):.1f} dB) '
                          f'at {at:.4g} Hz', None, 'fail' if value > limit else 'pass'))
    rows += _readout_rows(check, dof, 'Over the limit', 'over the limit', 'within the limit',
                          check.frequency)
    rows += [v.row('Note', note, None, 'warn') for note in check.notes]
    judged = check.judged[row] & np.isfinite(check.distortion[row])
    f = check.frequency
    lines = [v.series(dof, 'measured', f[judged], 100.0 * check.distortion[row][judged]),
             _flat(f'limit {100.0 * limit:.1f} %', 'tolerance', float(f.min()),
                   float(f.max()), 100.0 * limit)]
    view = v.spectrum(f'Distortion of {tone} on {dof}', v.axis('Frequency', 'Hz', 'log'),
                      v.axis('Distortion', '%', 'linear'), lines, link='frequency')
    return [view, v.reading(f'Distortion of {name}, {tone}, {dof}', rows)]


TOLERANCE = Param('tolerance', 'Band', 'number', default=2.0, min=0.01, max=50.0, step=0.25,
                  unit='%')
AT_LEAST = Param('floor', 'At least', 'number', default=0.0, min=0.0, max=100.0, step=0.5,
                 unit='Hz')


def frequency_check(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.sine_frequency`: whether a sine record's tone was at the
    frequency its profile asks for (`core.sine_frequency.check_frequency`),
    read from the levels last extracted from it: the tone's own frequency
    against the specification's at every line of the sweep, as an error
    in percent, against a band set as a share of the frequency with a
    floor in hertz, on one control channel."""
    from hertzforge.core.sine_frequency import check_frequency

    name, spec_name = params['history'], params['specification']
    _levels_name, levels = _extracted(project, name, spec_name)
    tone = _nth(list(levels.tone_names), params['tone'], 'tone', 'the levels')
    dof = _nth([str(d) for d in levels.response_dof], params['record'], 'control channel',
               'the levels')
    try:
        check = check_frequency(project[name], project[spec_name], levels, tone=tone,
                                tolerance=params['tolerance'] / 100.0, floor=params['floor'])
        row = check.row(dof)
    except (ValueError, KeyError) as error:
        raise refused(str(error).strip('\'"')) from None
    rows = [v.row('Band', check.band_words(), None, 'info')]
    worst = check.worst(dof)
    if worst is not None:
        at, error, relative = worst
        inside = abs(error) <= float(np.interp(at, check.specified, check.allowed))
        rows.append(v.row('Worst', (f'{100.0 * relative:+.3g} % ({error:+.3g} Hz) at '
                                    f'{at:.4g} Hz').replace('-', '−'), None,
                          'pass' if inside else 'fail'))
    rows += _readout_rows(check, dof, 'Outside the band', 'outside the band',
                          'within the band', check.specified)
    rows += [v.row('Note', note, None, 'warn') for note in check.notes]
    relative = 100.0 * check.relative[row]
    judged = check.judged[row] & np.isfinite(relative)
    f = check.specified
    band = 100.0 * check.allowed / f
    lines = [v.series(dof, 'measured', f[judged], relative[judged]),
             v.series(f'band {check.band_words()}', 'tolerance', f, band),
             v.series('band, below', 'tolerance', f, -band)]
    view = v.spectrum(f'Frequency error of {tone} on {dof}', v.axis('Frequency', 'Hz', 'log'),
                      v.axis('Frequency error', '%', 'linear'), lines, link='frequency')
    return [view, v.reading(f'Frequency of {name}, {tone}, {dof}', rows)]


SHARE = Param('share', 'Share', 'number', default=95.0, min=50.0, max=99.9, step=1.0, unit='%')


def dwell(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.dwell`: a held tone's build-up and ring-down
    (`core.dwell.read_dwell`): on one channel (a response by default),
    the peak over each cycle against time, the steady level the dwell
    reached, the cycles it took to reach the share set here and the
    cycles after the stop for the level to fall to the rest of it."""
    from hertzforge.core.dwell import read_dwell

    name, spec_name = params['history'], params['specification']
    history, spec = project[name], project[spec_name]
    if not any(tone.dwell_segments() for tone in spec.tones):
        raise refused(f'{spec_name} holds no dwell: each of its tones sweeps')
    row = _chosen(history, params['record'], name)
    share = round(params['share'] / 100.0, 6)
    try:
        reading = read_dwell(history, spec, share=share,
                             channel=None if row is None else str(history.response_dof[row]))
    except ValueError as error:
        raise refused(str(error)) from None
    unit = unit_text(reading.dimension)
    percent, rest = f'{100.0 * share:g} %', f'{100.0 * (1.0 - share):g} %'

    def count(cycles: float | None, seconds: float | None) -> str:
        return ('not read' if cycles is None
                else f'{cycles:.1f} cycles, {seconds:.3g} s')

    rows = [v.row('Channel', reading.dof, None, 'info'),
            v.row('Frequency', reading.frequency, 'Hz', 'info'),
            v.row('Steady', reading.steady, unit, 'info'),
            v.row(f'To {percent}', count(reading.built, reading.built_seconds), None, 'info'),
            v.row(f'Down to {rest}', count(reading.rung, reading.rung_seconds), None, 'info'),
            v.row('Settled', 'yes' if reading.settled else 'no', None,
                  'pass' if reading.settled else 'warn')]
    rows += [v.row('Note', note, None, 'warn') for note in reading.notes]
    x = np.asarray(reading.stands, dtype=float)
    t0, t1 = float(x.min()), float(x.max())
    steady = reading.steady
    lines = [v.series(reading.dof, 'measured', x, reading.level),
             _flat(f'steady {steady:.4g} {unit or ""}'.strip(), 'reference', t0, t1, steady),
             _flat(f'{percent} of steady', 'tolerance', t0, t1, share * steady),
             _flat(f'{rest} of steady', 'tolerance', t0, t1, (1.0 - share) * steady)]
    view = v.spectrum(f'Dwell of {reading.tone} on {reading.dof}', v.axis('Time', 's', 'linear'),
                      v.axis('Level', unit, 'linear'), lines,
                      regions=[v.region(reading.start, reading.stop, 'frame', 'the dwell')])
    return [view, v.reading(f'Dwell of {name}, {reading.tone}', rows)]


EXCLUSION = Param('exclusion', 'Left out', 'number', default=2.0, min=0.0, max=100.0, step=0.5,
                  unit='Hz')
TRACKING = Param('band', 'Tracking band', 'number', default=0.5, min=0.01, max=100.0, step=0.1,
                 unit='Hz')
TONE_TOLERANCE = Param('tone_tolerance', 'Tone tolerance', 'number', default=10.0, min=0.1,
                       max=100.0, step=1.0, unit='%')


def _signed(value: float, unit: str, places: int = 1) -> str:
    if not np.isfinite(value):
        return 'not read'
    value = 0.0 if round(value, places) == 0.0 else value
    return f'{value:+.{places}f} {unit}'.replace('-', '−')


def _level(value: float, unit: str) -> str:
    return f'{value:.3g} {unit}'.strip() if np.isfinite(value) else 'not read'


def mixed_mode(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.mixed_mode`: a sine-on-random record's data sheet
    (`core.mixed_mode.judge_mixed`): the tones tracked and taken out, the
    floor judged on what is left with the lines about each tone left out,
    each tone in amplitude, each narrowband at its own degrees of freedom
    and the total unjudged, with the residual drawn over the reference."""
    from hertzforge.core.matrix_checks import runs
    from hertzforge.core.mixed_mode import MixedSpecification, judge_mixed
    from hertzforge.core.sine_tracking import SineTracking

    name, floor_name, spec_name = params['history'], params['floor'], params['specification']
    history, floor = project[name], project[floor_name]
    held = [str(dof) for dof in history.response_dof]
    channels = [str(dof) for dof in floor.response_dof if str(dof) in held]
    if not channels:
        raise refused(f'{floor_name} is written at no channel {name} holds')
    channel = _nth(channels, params['record'], 'control channel', f'{floor_name} at {name}')
    try:
        spec = MixedSpecification(floor, project[spec_name], exclusion=params['exclusion'])
        reading = judge_mixed(history, spec, channel=channel,
                              band=SineTracking(fixed=params['band']),
                              tone_tolerance=params['tone_tolerance'])
    except ValueError as error:
        raise refused(str(error)) from None
    unit = unit_text(reading.dimension) or ''
    rms, peak = f'{unit} rms'.strip(), f'{unit} peak'.strip()
    sheet = reading.floor
    if sheet.assessed:
        floor_words = (f'written {_level(sheet.reference, rms)}, measured '
                       f'{_level(sheet.measured, rms)}, error {_signed(sheet.error, "%")}; '
                       f'±{sheet.rms_percent:g} %, lines ±{sheet.db:g} dB ({sheet.outside} of '
                       f'{sheet.judged} outside, {sheet.expected:.2g} by chance, '
                       f'{sheet.allowed} allowed)')
    else:
        floor_words = (f'±{sheet.rms_percent:g} %, lines ±{sheet.db:g} dB; no line left to '
                       'judge')
    rows = [v.row('Floor, tone and band lines left out', floor_words, None,
                  _verdict(sheet.passed))]
    if reading.tones:
        error = (100.0 * (reading.tones_measured / reading.tones_reference - 1.0)
                 if reading.tones_reference else float('nan'))
        rows.append(v.row(f'Tones, {len(reading.tones)} together',
                          f'written {_level(reading.tones_reference, rms)}, measured '
                          f'{_level(reading.tones_measured, rms)}, error {_signed(error, "%")}; '
                          'none: each tone is judged', None, 'info'))
    for tone in reading.tones:
        where = f'{tone.low:.4g} Hz' if tone.fixed else f'{tone.low:.4g} to {tone.high:.4g} Hz'
        rows.append(v.row(f'Tone {tone.name}',
                          f'{where}: written {_level(tone.reference, peak)}, measured '
                          f'{_level(tone.measured, peak)}, error {_signed(tone.error, "%")}; '
                          f'±{tone.tolerance:g} %' + ('' if tone.readings
                                                      else '; no reading left'),
                          None, _verdict(tone.passed)))
    for band in reading.narrowbands:
        where = (f'{band.low:.4g} to {band.high:.4g} Hz' if not band.swept
                 else f'{band.span[0]:.4g} to {band.span[1]:.4g} Hz, swept')
        tolerance = f'{_signed(band.low_db, "dB")} to {_signed(band.high_db, "dB")}'
        tolerance += (f' at {band.dof:.0f} dof' if band.slices else '; not read, see the notes')
        if band.swept and band.slices:
            tolerance += (f'; {band.slices} slices of {band.slice_dof:.0f} dof, '
                          f'{100 * band.inside:.0f} % inside')
        rows.append(v.row(f'Band {band.name}',
                          f'{where}: written {_level(band.reference, rms)}, measured '
                          f'{_level(band.measured, rms)}, error '
                          f'{_signed(band.error_db, "dB")}; {tolerance}', None,
                          _verdict(band.passed)))
    total = reading.total
    error = (100.0 * (total.measured / total.reference - 1.0) if total.reference
             else float('nan'))
    rows.append(v.row('Total, the whole drive',
                      f'written {_level(total.reference, rms)}, measured '
                      f'{_level(total.measured, rms)}, error {_signed(error, "%")}; none: the '
                      'machine\'s figure', None, 'info'))
    tones = (f'tones read through {reading.tones[0].band}, from {reading.start:g} s'
             if reading.tones else f'no tones; read from {reading.start:g} s')
    rows += [v.row('Lines', f'{sheet.spacing:.3g} Hz, {sheet.dof:.0f} degrees of freedom a line',
                   None, 'info'),
             v.row('Left out', f'±{sheet.exclusion:g} Hz about each tone; tone and band lines '
                   f'{100 * sheet.left_out:.1f} % of the floor\'s', None, 'info'),
             v.row('Tracking', tones, None, 'info'),
             v.row('Residual', sheet.residual, rms, 'info')]
    rows += [v.row('Note', note, None, 'warn') for note in reading.notes]
    rows.append(v.row('Reading', reading.describe(unit), None, _verdict(reading.passed)))
    f = np.asarray(reading.frequencies, dtype=float)
    keep = f > 0.0
    f = f[keep]
    reference = np.asarray(reading.reference, dtype=float)[keep]
    factor = 10.0 ** (sheet.db / 10.0)
    lines = [_positive('reference: floor and fixed bands', 'reference', f, reference),
             _positive(f'±{sheet.db:g} dB on the floor', 'tolerance', f, reference * factor),
             _positive(f'−{sheet.db:g} dB on the floor', 'tolerance', f, reference / factor)]
    if reading.tones:
        lines.append(_positive('record, tones in', 'other', f,
                               np.asarray(reading.composite, dtype=float)[keep]))
    lines.append(_positive('residual, tones taken out', 'measured', f,
                           np.asarray(reading.residual, dtype=float)[keep]))
    spacing = float(sheet.spacing)
    left = [v.region(max(f[a] - 0.5 * spacing, f[0]), f[b] + 0.5 * spacing, 'cut', 'left out')
            for a, b in runs(np.asarray(reading.left_out, dtype=bool)[keep])]
    view = v.spectrum(f'{channel}: the residual over the reference',
                      v.axis('Frequency', 'Hz', 'log'),
                      v.axis('PSD', unit_text(f'{reading.dimension}**2/frequency'), 'log'),
                      lines, link='frequency', regions=left)
    return [view, v.reading(f'Data sheet of {name}, {channel}', rows)]


# ---- a shock record ---------------------------------------------------------------------------

SHAPE = Param('shape', 'Shape', 'enum', default='half-sine', options=(
    Option('half-sine', 'Half-sine'), Option('sawtooth', 'Sawtooth'),
    Option('trapezoid', 'Trapezoid')))
PEAK = Param('peak', 'Peak, 0 for the record\'s own', 'number', default=0.0, unit='m/s²',
             step=1.0)
DURATION = Param('duration', 'Duration, 0 for the record\'s own', 'number', default=0.0,
                 min=0.0, unit='s', step=0.0005)
EDGE = Param('edge', 'Edge, 0 for a twentieth of the duration', 'number', default=0.0, min=0.0,
             unit='s', step=0.0001)
BAND = Param('band', 'Band', 'number', default=20.0, min=0.1, max=99.0, step=1.0, unit='%')
DURATION_TOLERANCE = Param('duration_tolerance', 'Duration tolerance', 'number', default=10.0,
                           min=0.1, max=99.0, step=1.0, unit='%')
EDGE_LIMIT = Param('edge_limit', 'Edge limit', 'number', default=10.0, min=0.0, max=49.0,
                   step=1.0, unit='%')
VELOCITY_TOLERANCE = Param('velocity_tolerance', 'Velocity tolerance', 'number', default=20.0,
                           min=0.1, max=99.0, step=1.0, unit='%')
QUIET = Param('quiet', 'Quiet', 'number', default=2.5, min=0.0, max=20.0, step=0.5,
              unit='durations')

#: the pulse the check opens on when the record holds nothing that reads as
#: one: Visual Dynamics' 50 g, 11 ms half-sine
OPENING_PULSE = (50.0 * 9.80665, 0.011)
#: the points the envelope is drawn with across the judged span
ENVELOPE_POINTS = 2000


def pulse_check(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.pulse_check`: one acceleration channel judged against a
    classical pulse's tolerance envelope and its velocity change
    (`core.classical_pulse.check_pulse`). A peak or duration left at zero
    is the half-sine the record most resembles, rounded to three figures
    (`core.classical_pulse.suggest_pulse`), as Visual Dynamics' Pulse
    Check opens on a record."""
    import hertzforge as hz
    from hertzforge.core import classical_pulse as pulses
    from hertzforge.drawing.plot.classical_pulse import readout

    name = params['history']
    history = project[name]
    row = _chosen(history, params['record'], name)
    try:
        channel = pulses.pulse_channel(history, row)
    except ValueError as error:
        raise refused(str(error)) from None
    dof = str(history.response_dof[channel])
    peak, duration = params['peak'], params['duration']
    if peak == 0.0 or duration == 0.0:
        try:
            near = pulses.suggest_pulse(np.asarray(history.abscissa, dtype=float),
                                        np.asarray(history.ordinate)[channel])
            suggested = (float(f'{near.peak:.3g}'), float(f'{near.duration:.3g}'))
        except ValueError:
            suggested = OPENING_PULSE
        peak = peak or suggested[0]
        duration = duration or suggested[1]
    shape = params['shape']
    edge = None if shape == 'half-sine' else (params['edge'] or 0.05 * duration)
    try:
        pulse = pulses.ClassicalPulse(shape, peak, duration, edge)
        check = pulses.check_pulse(
            history, pulse, channel, band=params['band'] / 100.0,
            duration_tolerance=params['duration_tolerance'] / 100.0,
            edge_limit=params['edge_limit'] / 100.0,
            velocity_tolerance=params['velocity_tolerance'] / 100.0, quiet=params['quiet'])
    except ValueError as error:
        raise refused(str(error)) from None
    words = readout(check, hz.SI)
    names = {'peak': 'Peak', 'duration': 'Duration', 'rise': 'Rise', 'fall': 'Fall',
             'velocity': 'Velocity change', 'outside': 'Outside'}
    rows = [v.row('Pulse', pulse.describe('m/s²', 1.0), None, 'info')]
    for key, text in words.items():
        status = ('pass' if check.envelope_ok else 'fail') if key == 'outside' else (
            'fail' if text.endswith(', out') else 'pass')
        rows.append(v.row(names[key], text, None, status))
    rows.append(v.row('Verdict', check.describe(), None, 'pass' if check.passed else 'fail'))
    unit = unit_text('acceleration')
    lines = []
    if check.time.size:
        fine = np.union1d(np.linspace(check.time[0], check.time[-1], ENVELOPE_POINTS),
                          check.time)
        lower, upper = pulses.pulse_envelope(
            pulse, fine - check.measured.start, band=check.band,
            duration_tolerance=check.duration_tolerance, edge_limit=check.edge_limit)
        lines += [v.series(f'envelope: ±{100 * check.band:g} % of the peak', 'tolerance',
                           fine, upper),
                  v.series('envelope, lower edge', 'tolerance', fine, lower),
                  v.series(f'nominal {pulse.shape}', 'reference', fine,
                           pulse.at(fine - check.measured.start))]
    lines.append(v.series(dof, 'measured', *_trace(check.time, check.acceleration)))
    out = np.flatnonzero(check.outside)
    if out.size:
        lines.append(v.series(f'outside the envelope: {check.outside_count} of {check.points} '
                              'points', 'abort', check.time[out], check.acceleration[out]))
    record_view = v.spectrum(f'{dof}: {pulse.describe(unit, 1.0)}; {check.describe()}',
                             v.axis('Time', 's', 'linear'), v.axis('Acceleration', unit, 'linear'),
                             lines, link='time',
                             regions=[v.region(*check.window, 'frame', 'velocity window')])
    nominal = check.nominal_velocity_change
    reach = check.velocity_tolerance * abs(nominal)
    t = np.asarray(check.velocity_time, dtype=float)
    velocity_view = v.spectrum(
        f'{dof}: the velocity across the window', v.axis('Time', 's', 'linear'),
        v.axis('Velocity', unit_text('velocity'), 'linear'),
        [v.series(dof, 'measured', *_trace(t, check.velocity)),
         _flat('nominal velocity change', 'reference', float(t[0]), float(t[-1]), nominal),
         _flat(f'±{100 * check.velocity_tolerance:g} %', 'tolerance', float(t[0]),
               float(t[-1]), nominal + reach),
         _flat(f'−{100 * check.velocity_tolerance:g} %', 'tolerance', float(t[0]),
               float(t[-1]), nominal - reach)], link='time')
    return [record_view, velocity_view, v.reading(f'Pulse Check of {name}, {dof}', rows)]


def _rating(name: str, label: str, unit: str) -> Param:
    return Param(name, f'{label}, 0 for none', 'number', default=0.0, min=0.0, unit=unit)


STROKE_RATINGS = (_rating('stroke', 'Stroke', 'm'), _rating('velocity', 'Velocity', 'm/s'),
                  _rating('force', 'Force', 'N'), _rating('moving_mass', 'Moving mass', 'kg'),
                  _rating('body_mass', 'Body mass', 'kg'))


def stroke(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.stroke`: whether a shock pulse fits on the shaker
    (`core.stroke.read_stroke`): one acceleration channel integrated
    raw to velocity and displacement, and its peak velocity, its travel
    each way and the force it asks for, each against the shaker's shock
    rating where one is given."""
    import hertzforge as hz
    from hertzforge.core.stroke import amount, read_stroke

    name = params['history']
    history = project[name]
    row = _chosen(history, params['record'], name)
    ratings = {param.name: params[param.name] or None for param in STROKE_RATINGS}
    try:
        reading = read_stroke(history, row, **ratings)
    except ValueError as error:
        raise refused(str(error)) from None
    us = hz.SI
    rows = [v.row('Peak', reading.peak_acceleration, unit_text('acceleration'), 'info'),
            v.row('Peak velocity', reading.peak_velocity, unit_text('velocity'), 'info'),
            v.row('Peak to peak', reading.peak_to_peak, 'm', 'info'),
            v.row('Half-sine rule', reading.rule, 'm', 'info'),
            v.row('Travel p-p', reading.travel, 'm', 'info'),
            v.row('Down / up', f'{amount(reading.travel_down, "length", us)} / '
                  f'{amount(reading.travel_up, "length", us)}', None, 'info'),
            v.row('Force', 'no moving mass given' if reading.force is None else reading.force,
                  None if reading.force is None else 'N', 'info')]
    judged = [line for line in reading.describe(us) if ' against ' in line]
    for line in judged:
        rows.append(v.row('Against the rating', line, None,
                          'fail' if 'OVER' in line else 'pass' if 'within' in line else 'info'))
    if not judged:
        rows.append(v.row('Against the rating', 'No ratings given: the reading stands on its '
                          'own.', None, 'info'))
    rows.append(v.row('Verdict', {True: 'fits', False: 'does not fit',
                                  None: 'no ratings given'}[reading.fits], None,
                      _verdict(reading.fits)))
    t = reading.time
    t0, t1 = float(t[0]), float(t[-1])
    rating = reading.rating
    dof = reading.dof
    acceleration = v.spectrum(
        f'{dof}: peak {amount(reading.peak_acceleration, "acceleration", us)}'
        + ('' if reading.force is None else f', force {amount(reading.force, "force", us)}'),
        v.axis('Time', 's', 'linear'), v.axis('Acceleration', unit_text('acceleration'),
                                              'linear'),
        [v.series(dof, 'measured', *_trace(t, reading.acceleration))], link='time')
    speed = [v.series('table', 'measured', *_trace(t, reading.velocity))]
    travel = [v.series('table', 'measured', *_trace(t, reading.displacement))]
    if reading.counts_body:
        speed.append(v.series('against the body', 'other',
                              *_trace(t, rating.relative(reading.velocity))))
        travel.append(v.series('against the body', 'other',
                               *_trace(t, rating.relative(reading.displacement))))
    if rating.velocity is not None:
        speed += [_flat('velocity rating', 'abort', t0, t1, rating.velocity),
                  _flat('velocity rating, down', 'abort', t0, t1, -rating.velocity)]
    if rating.room is not None:
        travel += [_flat('stop', 'abort', t0, t1, rating.room),
                   _flat('stop, below', 'abort', t0, t1, -rating.room)]
    velocity = v.spectrum(
        f'peak velocity{" with the body" * reading.counts_body} '
        f'{amount(reading.relative_velocity, "velocity", us)}', v.axis('Time', 's', 'linear'),
        v.axis('Velocity', unit_text('velocity'), 'linear'), speed, link='time')
    displacement = v.spectrum(
        reading.to_the_stop(us) or f'travel {amount(reading.travel, "length", us)}',
        v.axis('Time', 's', 'linear'), v.axis('Displacement', 'm', 'linear'), travel,
        link='time')
    return [acceleration, velocity, displacement,
            v.reading(f'Stroke of {name}, {dof}', rows)]


#: Visual Dynamics' starting shaker: 4000 lbf, 2 in of stroke and 70 in/s
#: carrying 150 lb, in SI; a starting point to be overwritten
FIT_RATINGS = (
    Param('force', 'Force', 'number', default=17792.89, min=0.0, unit='N'),
    Param('stroke', 'Stroke p-p', 'number', default=0.0508, min=0.0, unit='m', step=0.001),
    Param('velocity', 'Velocity', 'number', default=1.778, min=0.0, unit='m/s', step=0.01),
    Param('moving_mass', 'Moving mass', 'number', default=68.04, min=0.0, unit='kg'),
    Param('body_mass', 'Body mass, 0 not counted', 'number', default=0.0, min=0.0, unit='kg'))
CUTOFFS = Param('cutoffs', 'Cutoffs', 'string', default='2, 3, 5, 8', unit='Hz')
DRAWN = Param('cutoff', 'Drawn at, 0 for the lowest that fits', 'number', default=0.0, min=0.0,
              unit='Hz')
LOGGED = Param('logged', 'Draw the record as logged', 'boolean', default=False)


def _cutoffs(text: str) -> tuple[float, ...]:
    try:
        values = sorted({float(part) for part in text.replace(',', ' ').split()})
    except ValueError:
        values = []
    if not values or values[0] <= 0.0:
        raise refused(f'{text!r} is not a list of cutoffs above zero, in Hz')
    return tuple(values)


def replication_fit(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.replication_fit`: whether a shaker can play a reference
    record (`core.replication_fit.replication_fit`): the force at its
    largest peak, and its travel, velocity and the price of each
    candidate high-pass cutoff, each case judged against the ratings; the
    travel at one cutoff is drawn inside the stroke, over the sweep of
    every candidate. The cutoff drawn when none is asked for is the
    lowest that fits, else the lowest candidate, Visual Dynamics'
    choice."""
    from hertzforge.core.replication_fit import replication_fit as fit_record
    from hertzforge.core.shaker import ShakerRating

    name = params['history']
    history = project[name]
    row = _chosen(history, params['record'], name)
    cutoffs = _cutoffs(params['cutoffs'])
    try:
        from hertzforge.core.shaker import acceleration_channel

        channel = acceleration_channel(history, row)
        rating = ShakerRating(**{param.name: params[param.name] or (
            None if param.name == 'body_mass' else 0.0) for param in FIT_RATINGS})
        fit = fit_record(history, rating, cutoffs=cutoffs, channel=channel)
        if params['logged']:
            case = fit.as_logged
        elif params['cutoff']:
            case = fit.case(params['cutoff'])
        else:
            case = fit.lowest_fitting or fit.cases[0]
    except (ValueError, KeyError) as error:
        raise refused(str(error).strip('\'"')) from None
    rows = [v.row('Largest peak', f'{fit.peak_acceleration:.4g} m/s² at {fit.peak_time:.4g} s',
                  None, 'info'),
            v.row('Force needed', f'{fit.force:.4g} N, {100.0 * fit.force_share:.0f} %', None,
                  'pass' if fit.fits_force else 'fail'),
            v.row('Crest factor', float(fit.crest_factor), '', 'info')]
    for each in (fit.as_logged, *fit.cases):
        rows.append(v.row('Logged' if each.cutoff is None else f'{each.cutoff:g} Hz',
                          f'down {each.travel_down:.3g} m, up {each.travel_up:.3g} m, velocity '
                          f'{each.relative_velocity:.3g} m/s, mean square out '
                          f'{100.0 * each.mean_square_removed:.2f} %, peak change '
                          f'{100.0 * each.peak_change:+.2f} %'.replace('-', '−'), None,
                          'pass' if each.fits else 'fail'))
    words = fit.describe()
    if all(params[param.name] == param.default for param in FIT_RATINGS):
        words += ' The ratings are a starting point: set them to the shaker\'s own.'
    rows.append(v.row('Reading', words, None, 'info'))
    title = case.describe()
    if case.cutoff is not None:
        past = fit.as_logged.first_past_stroke()
        title += (f' As logged the travel spans {fit.as_logged.peak_to_peak:.4g} m'
                  + (f' and leaves the room {past:.3g} s in' if past is not None
                     else ', inside the room')
                  + f'; the record\'s mean of {fit.offset:.3g} m/s² alone integrates to '
                  f'{fit.offset_displacement:.4g} m by its end.')
    t = case.time
    room = rating.room
    counted = 'travel, body counted' if rating.body_share > 0.0 else 'travel'
    travel = v.spectrum(title, v.axis('Time', 's', 'linear'), v.axis(counted.capitalize(), 'm',
                                                                     'linear'),
                        [v.series(counted, 'measured', *_trace(t, case.travel)),
                         _flat('stop', 'abort', float(t[0]), float(t[-1]), room),
                         _flat('stop, below', 'abort', float(t[0]), float(t[-1]), -room)])
    x = np.array([c.cutoff for c in fit.cases])
    cut = v.axis('Cutoff', 'Hz', 'linear')
    sweep = [
        v.spectrum('Travel at each cutoff', cut, v.axis('Travel', 'm', 'linear'), [
            v.series('up', 'measured', x, [c.travel_up for c in fit.cases]),
            v.series('down', 'other', x, [c.travel_down for c in fit.cases]),
            _flat('room to the stop', 'abort', float(x[0]), float(x[-1]), room)],
            link='cutoff'),
        v.spectrum('Peak velocity' + ', body counted' * (rating.body_share > 0.0)
                   + ' at each cutoff', cut, v.axis('Velocity', 'm/s', 'linear'), [
            v.series('velocity', 'measured', x, [c.relative_velocity for c in fit.cases]),
            _flat('velocity rating', 'abort', float(x[0]), float(x[-1]), rating.velocity)],
            link='cutoff'),
        v.spectrum('The price of each cutoff', cut, v.axis('Price', '%', 'linear'), [
            v.series('mean square removed', 'measured', x,
                     [100.0 * c.mean_square_removed for c in fit.cases]),
            v.series('largest peak lost', 'other', x,
                     [-100.0 * c.peak_change for c in fit.cases])], link='cutoff')]
    return [travel, *sweep, v.reading(f'Replication Fit of {name}, {fit.dof}', rows)]


LOW_HZ = Param('low', 'From, 0 for the record\'s own', 'number', default=0.0, min=0.0,
               unit='Hz')
HIGH_HZ = Param('high', 'To, 0 for the record\'s own', 'number', default=0.0, min=0.0,
                unit='Hz')
PER_OCTAVE = Param('per_octave', 'Per octave', 'integer', default=12, min=1, max=96, step=1)
OMEGA0 = Param('omega0', 'Cycles', 'number', default=6.0, min=3.0, max=30.0, step=1.0)


def wavelet(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.wavelet`: where one channel's frequencies are, moment by
    moment: its Morlet scalogram over a log frequency axis, computed band
    by band and peak-held to a picture's width of time columns
    (`core.wavelet.scalogram_peaks`, as `core.wavelet.reading` does), the
    ridge (`core.wavelet.ridge`) and the largest magnitude, with what of
    the range is cone of influence. The range is clamped to what the
    record carries, as Visual Dynamics' panel clamps it."""
    from hertzforge.core import wavelet as cwt

    name = params['history']
    history = project[name]
    row = _record(history, params['record'], name)
    try:
        rate = float(history.sample_rate)
    except ValueError as error:
        raise refused(str(error)) from None
    duration = len(history.abscissa) / rate
    low, high = cwt.default_range(rate, duration)
    low, high = params['low'] or low, params['high'] or high
    nyquist = rate / 2.0
    high = min(max(high, nyquist / 1000.0), nyquist * 0.98)
    low = min(max(low, nyquist / 10000.0), high / 2.0)
    omega0, per_octave = params['omega0'], params['per_octave']
    frequencies = cwt.log_frequencies(low, high, per_octave)
    frequencies = frequencies[frequencies < nyquist]
    if frequencies.size < 2:
        raise refused(f'no frequencies this record can carry in that range; it reaches '
                      f'{nyquist:g} Hz')
    values = np.real(np.asarray(history.ordinate)[row])
    clock, magnitude = cwt.scalogram_peaks(values, rate, frequencies, omega0,
                                           columns=SCALOGRAM_COLUMNS)
    clock = clock + float(history.abscissa[0])
    unit = unit_text(history.ordinate_dim[row])
    label = history.record_label(row)
    top = float(np.max(magnitude)) if magnitude.size else 0.0
    at = np.unravel_index(int(np.argmax(magnitude)), magnitude.shape)
    cone = float(cwt.cone_of_influence([low], rate, omega0)[0])
    rows = [v.row('From', low, 'Hz', 'info'), v.row('To', high, 'Hz', 'info'),
            v.row('Per octave', float(per_octave), 'lines', 'info'),
            v.row('Cycles', omega0, '', 'info'),
            v.row('Cone at bottom', cone, 's', 'info'),
            v.row('Resolves', low / omega0 * 2, 'Hz', 'info'),
            v.row('Lines', float(frequencies.size), '', 'info'),
            v.row('Largest', top, unit, 'info'),
            v.row('At', f'{frequencies[at[0]]:.4g} Hz, {clock[at[1]]:.4g} s', None, 'info')]
    if 2 * cone >= duration:
        rows.append(v.row('Note', f'The cone at {low:.3g} Hz covers the whole record: nothing '
                          'at the bottom of this range is measurement. Raise the bottom, or '
                          'read a longer stretch.', None, 'warn'))
    elif 2 * cone >= duration / 3.0:
        rows.append(v.row('Note', f'The bottom {2 * cone / duration:.0%} of this record is '
                          'inside the cone at the lowest frequency.', None, 'warn'))
    picture = v.matrix(f'Scalogram of {name}, {label}',
                       {'label': 'Frequency', 'ticks': [f'{f:.4g} Hz'
                                                        for f in frequencies[::-1]]},
                       {'label': 'Time', 'ticks': [f'{t:.4g} s' for t in clock]},
                       magnitude[::-1], f'{history.ordinate_dim[row]}'
                       + (f' [{unit}]' if unit else ''), (0.0, top if top > 0.0 else 1.0))
    surface = v.waterfall(
        f'Scalogram of {name}, {label}', v.axis('Time', 's', 'linear'),
        v.axis(str(history.ordinate_dim[row]).capitalize(), unit, 'linear'),
        [f'{f:.4g} Hz' for f in frequencies], [(clock, m) for m in magnitude],
        (float(clock[0]), float(clock[-1])))
    start, end = float(clock[0]), float(clock[-1])
    cones = cwt.cone_of_influence(frequencies, rate, omega0)
    walls = [np.clip(start + cones, start, end), np.clip(end - cones, start, end)]
    v.staged(surface, [f'{f:.4g} Hz' for f in frequencies], range(frequencies.size),
             ['measured'] * frequencies.size, walls=walls, surface=True)
    ridge = v.spectrum(f'Ridge of {name}, {label}: the frequency of the largest magnitude',
                       v.axis('Time', 's', 'linear'), v.axis('Frequency', 'Hz', 'log'),
                       [v.series(label, 'measured', clock, cwt.ridge(magnitude, frequencies))],
                       link='time')
    return [surface, picture, ridge, v.reading(f'Wavelet of {name}, {label}', rows)]


# ---- the shock windows ------------------------------------------------------------------------

SHOCK = Param('shock', 'Shock, one past the last adds one', 'integer', default=1, min=1, step=1)
SHOWN = Param('record', 'Record shown', 'integer', default=1, min=1, step=1)
START = Param('start', 'Start', 'number', default=0.0, unit='s', step=0.001)
STOP = Param('stop', 'Stop', 'number', default=0.1, unit='s', step=0.001)


def _shocks_reading(name: str, history: Any) -> dict:
    origin = float(history.abscissa[0])
    shocks = tuple(history.shocks or ())
    rows = [v.row(f'Shock {k + 1}', f'{origin + shock.start:.4f} s to '
                  f'{origin + shock.stop:.4f} s, {shock.duration:.4f} s long', None, 'info')
            for k, shock in enumerate(shocks)]
    covered = sum(shock.duration for shock in shocks)
    rows.append(v.row('Shocks', f'{len(shocks)} shock{"s" * (len(shocks) != 1)}, '
                      f'{covered:.3f} s of record analyzed. One spectrum per channel per '
                      'shock.' if shocks else 'No shocks marked.', None, 'info'))
    return v.reading(f'Shocks on {name}', rows)


def _shock_views(name: str, history: Any, params: dict[str, Any]) -> list[dict]:
    """The shock windows where they can be dragged, as Visual Dynamics shows
    them on the record and on its 3-D stage: one record with each window a
    span (`ops_settings._trace`), the whole history on the stage with the
    same spans (`waterfalls.waterfall_view`), and the windows read. Dragging
    a window's middle slides it and an edge moves that edge, through Shock
    Window; a record a file cut into captures has its events settled, and
    its windows are shown but not dragged."""
    from .ops_settings import _trace
    from .waterfalls import waterfall_view

    record = _record(history, params.get('record', 1), name)
    origin = float(history.abscissa[0])
    settled = history.split_into_frames
    regions = [v.region(origin + shock.start, origin + shock.stop, 'span', f'Shock {k + 1}',
                        None if settled else v.dragging(
                            'project.set_shock', {'history': name, 'shock': k + 1,
                                                  'record': record + 1}, 'start', 'stop'))
               for k, shock in enumerate(history.shocks or ())]
    stage = waterfall_view(name, history)
    stage.pop('quantities', None)
    stage.pop('quantity', None)
    if regions:
        stage['regions'] = regions
    return [_trace(history, name, record, regions), stage, _shocks_reading(name, history)]


def shocks(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.shocks`: a time history's shock windows on one record and on
    its stage, each dragged to move it (`_shock_views`)."""
    name = params['history']
    return _shock_views(name, project[name], params)


def _store_shocks(history: Any, shocks: tuple, rate: float) -> None:
    """The windows on the history, each inside the record and off its
    neighbors, as Visual Dynamics stores every edit
    (`core.shocks.held_apart`)."""
    from hertzforge.core.shocks import held_apart

    samples = int(np.asarray(history.ordinate).shape[-1])
    history.shocks = held_apart(tuple(
        s if s.fits(samples, rate) else s.clipped(samples, rate) for s in shocks))


def _editable(history: Any) -> tuple[tuple, float]:
    """The record's shock windows and its sample rate; refused for a
    record a file cut into captures, or one not evenly sampled."""
    if history.split_into_frames:
        raise refused('The file already cut this record into captures, so the events are '
                      'settled.')
    try:
        rate = float(history.sample_rate)
    except ValueError as error:
        raise refused(str(error)) from None
    return tuple(history.shocks or ()), rate


def set_shock(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_shock`: one shock window typed by its start and stop
    on the record's clock, as Visual Dynamics' shock table edits one; a
    number one past the last adds a window. The window is held inside
    the gap its neighbors leave (`core.shocks.held_apart`)."""
    from hertzforge.core.shocks import Shock, held_apart

    name = params['history']
    history = project[name]
    shocks, rate = _editable(history)
    number = params['shock']
    if number > len(shocks) + 1:
        raise refused(f'{name} has {len(shocks)} shock{"s" * (len(shocks) != 1)}; the next '
                      f'is shock {len(shocks) + 1}')
    origin = float(history.abscissa[0])
    try:
        moved = Shock(params['start'] - origin, params['stop'] - params['start'])
    except ValueError as error:
        raise refused(str(error)) from None
    if number <= len(shocks):
        edited = tuple(moved if k == number - 1 else shock for k, shock in enumerate(shocks))
        index = number - 1
    else:
        edited = tuple(sorted((*shocks, moved), key=lambda shock: shock.start))
        index = edited.index(moved)
    _store_shocks(history, held_apart(edited, moved=index), rate)
    project.record_setting(name, 'shocks', history.shocks)
    return _shock_views(name, history, params)


REMOVED = Param('shock', 'Shock', 'integer', default=1, min=1, step=1)


def remove_shock(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.remove_shock`: one shock window dropped, as Visual
    Dynamics' Remove drops an event that was never a shock."""
    name = params['history']
    history = project[name]
    shocks, _rate = _editable(history)
    number = params['shock']
    if number > len(shocks):
        raise refused(f'{name} has {len(shocks)} shock{"s" * (len(shocks) != 1)}; there is '
                      f'no shock {number}')
    history.shocks = tuple(shock for k, shock in enumerate(shocks) if k != number - 1)
    project.record_setting(name, 'shocks', history.shocks)
    return _shock_views(name, history, params)


OPERATIONS = (
    Operation(
        id='project.sine_extraction', title='Sine Levels', workspace='sine',
        summary='How a sweep\'s levels are read out of a time history, stored on it for '
                'Extract Sine: the smoothing, chosen from the data or set in cycles, and '
                'the readings it gives sampled along each tone.',
        params=(HISTORY, SWEEP, SMOOTHING, CYCLES, TARGET, REFINE),
        outputs=('spectrum', 'reading'), run=sine_extraction, edits=True),
    Operation(
        id='project.level_error', title='Level Error', workspace='sine',
        summary='Extracted sine levels against the specification they were controlled to: '
                'one tone\'s levels over its requirement, and each channel\'s RMS deviation '
                'in dB across the sweep.',
        params=(SWEEP, LEVELS, TONE, THRESHOLD), outputs=('spectrum', 'reading'),
        run=level_error),
    Operation(
        id='project.sine_distortion', title='Distortion', workspace='sine',
        summary='What a sine record carries besides its tones, from the levels last '
                'extracted from it: the record less the tones against the tones, at every '
                'line of the sweep.',
        params=(HISTORY, SWEEP, TONE, CONTROL, EXCESS), outputs=('spectrum', 'reading'),
        run=distortion),
    Operation(
        id='project.sine_frequency', title='Frequency', workspace='sine',
        summary='Whether a sine record\'s tone was at the frequency its profile asks for, '
                'from the levels last extracted from it, at every line of the sweep.',
        params=(HISTORY, SWEEP, TONE, CONTROL, TOLERANCE, AT_LEAST),
        outputs=('spectrum', 'reading'), run=frequency_check),
    Operation(
        id='project.dwell', title='Dwell', workspace='sine',
        summary='A held tone read against time: the steady level it reached, the cycles '
                'it took to build to a share of it, and the cycles it rang down after the '
                'stop.',
        params=(HISTORY, SWEEP, CHOSEN, SHARE), outputs=('spectrum', 'reading'), run=dwell),
    Operation(
        id='project.mixed_mode', title='Mixed-Mode Data Sheet', workspace='sine',
        summary='A sine-on-random record as one test: the tones tracked and taken out, the '
                'floor, each tone, each narrowband and the total, each by its own rule.',
        params=(HISTORY, FLOOR, SWEEP, CONTROL, EXCLUSION, TRACKING, TONE_TOLERANCE),
        outputs=('spectrum', 'reading'), run=mixed_mode),
    Operation(
        id='project.pulse_check', title='Pulse Check', workspace='shock',
        summary='One acceleration channel against a classical pulse\'s tolerance envelope '
                'and its velocity change; a peak or duration of zero takes the half-sine the '
                'record most resembles.',
        params=(HISTORY, CHOSEN, SHAPE, PEAK, DURATION, EDGE, BAND, DURATION_TOLERANCE,
                EDGE_LIMIT, VELOCITY_TOLERANCE, QUIET),
        outputs=('spectrum', 'reading'), run=pulse_check),
    Operation(
        id='project.stroke', title='Stroke', workspace='shock',
        summary='Whether a shock pulse fits on the shaker: its acceleration integrated raw '
                'to velocity and travel, each against the shaker\'s shock rating where one '
                'is given.',
        params=(HISTORY, CHOSEN, *STROKE_RATINGS),
        outputs=('spectrum', 'reading'), run=stroke),
    Operation(
        id='project.replication_fit', title='Replication Fit', workspace='transient',
        summary='Whether a shaker can play a reference record: its travel, velocity and '
                'force at each candidate high-pass cutoff against the ratings, and what each '
                'cutoff costs.',
        params=(HISTORY, CHOSEN, *FIT_RATINGS, CUTOFFS, DRAWN, LOGGED),
        outputs=('spectrum', 'reading'),
        run=replication_fit),
    Operation(
        id='project.wavelet', title='Wavelet', workspace='shock',
        summary='Where one channel\'s frequencies are, moment by moment: its scalogram as a '
                'surface and flat, the ridge it follows and how much of the range is cone of '
                'influence.',
        params=(HISTORY, RECORD, LOW_HZ, HIGH_HZ, PER_OCTAVE, OMEGA0),
        outputs=('waterfall', 'matrix', 'spectrum', 'reading'), run=wavelet),
    Operation(
        id='project.set_shock', title='Shock Window', workspace='shock',
        summary='One shock window typed by its start and stop on the record\'s clock, held '
                'off its neighbors; one past the last adds a window.',
        params=(HISTORY, SHOCK, START, STOP, SHOWN),
        outputs=('spectrum', 'waterfall', 'reading'), run=set_shock, edits=True),
    Operation(
        id='project.remove_shock', title='Remove Shock', workspace='shock',
        summary='One shock window dropped from a time history.',
        params=(HISTORY, REMOVED, SHOWN), outputs=('spectrum', 'waterfall', 'reading'),
        run=remove_shock, edits=True),
    Operation(
        id='project.shocks', title='Shocks', workspace='shock',
        summary='A time history\'s shock windows on one record and on its 3-D stage; drag a '
                'window to move it, or an edge to change it.',
        params=(HISTORY, SHOWN), outputs=('spectrum', 'waterfall', 'reading'), run=shocks),
)
