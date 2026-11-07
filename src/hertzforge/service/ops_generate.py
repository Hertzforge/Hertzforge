"""Visual Dynamics' tools that generate, convert and predict: a signal made
from a spectrum, a specification or a channel table; a shock synthesized to
a shock spectrum; how each mode of a shape set answers a record's forces;
a sine sweep's rate against the modes it passes; the extreme response and
fatigue damage spectra of a record; the drives a specification asks for
through an FRF; the notch a set of limit channels would cut; and the
conditioning and realizability of a matrix.

Generate Signal, SRS Synthesis and Compute Modal Responses add an object,
as Visual Dynamics does, and draw what they added; the others are readings
of objects that exist and add nothing. Every number is the core's:
`core.signals`, `core.shock_synthesis`, `core.modal_response`,
`core.sweep_rate`, `core.fatigue`, `core.drive_prediction`,
`core.limit_prediction` and `core.matrix_checks`.
"""

from __future__ import annotations

import re
from typing import Any

import numpy as np

from . import run_logs
from . import views as v
from .descriptors import Operation, Option, Param
from .ops_sine_shock import (
    EDGE_WORDS,
    STROKE_RATINGS,
    _flat,
    _positive,
    _record,
    _trace,
    pulse_edge,
)
from .protocol import refused
from .waterfalls import unit_text, waterfall_view

FREQUENCY = v.axis('Frequency', 'Hz', 'log')
TIME = v.axis('Time', 's', 'linear')


def _finite(name: str, role: Any, x: Any, y: Any) -> dict:
    """A curve on a linear y axis: the finite points, x above zero."""
    x, y = np.asarray(x, dtype=float), np.asarray(y, dtype=float)
    keep = np.isfinite(x) & np.isfinite(y) & (x > 0.0)
    return v.series(name, role, x[keep], y[keep])


def _sentence(text: str) -> str:
    return text[0].upper() + text[1:] if text else text


def _notes(notes: Any, status: str = 'info') -> list[dict]:
    return [v.row('Note', _sentence(note), None, status) for note in notes if note]


def _core(call: Any) -> Any:
    """Run a core call, refusing in its own words."""
    try:
        return call()
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None


def _picked(text: str, count: int, what: str) -> list[int] | None:
    """The 0-based items '1-3, 7' names, numbered from 1; None when blank,
    which is all of them."""
    if not text.strip():
        return None
    out: list[int] = []
    for part in text.replace(' ', '').split(','):
        first, dash, last = part.partition('-')
        try:
            low, high = int(first), int(last) if dash else int(first)
        except ValueError:
            raise refused(f'{part!r} is not a {what} number or a range such as 3-6') from None
        if not 1 <= low <= high <= count:
            raise refused(f'{what}s run from 1 to {count}; {part} is outside them')
        out.extend(range(low - 1, high))
    return sorted(set(out))


def _dofs(text: str) -> tuple[str, ...] | None:
    return tuple(text.replace(',', ' ').split()) or None


# ---- Generate Signal ----------------------------------------------------------------

SIGNAL_SOURCE = Param('source', 'Source', 'enum', options_from='verb:generate_signal')
KIND = Param('kind', 'Kind', 'enum', default='random', options=tuple(
    Option(key, label) for key, label in (
        ('random', 'Random'), ('burst', 'Burst random'), ('periodic', 'Periodic random'),
        ('impulse', 'Impulse'), ('pulse', 'Classical pulse'), ('sine', 'Sine'))))
SAMPLE_RATE = Param('sample_rate', 'Sample rate, 0 for the one suggested', 'number',
                    default=0.0, unit='Hz', min=0.0, step=1024.0)
DURATION = Param('duration', 'Duration', 'number', default=10.0, unit='s', min=0.001, step=1.0)
FRAME = Param('frame_length', 'Frame, 0 for automatic', 'integer', default=0, min=0,
              step=1024, unit='samples')
SEED = Param('seed', 'Seed, 0 to draw one', 'integer', default=0, min=0, step=1)
CROSS_TERMS = Param('correlated', 'Cross terms', 'boolean', default=True)
BURSTS = Param('bursts', 'Bursts', 'integer', default=10, min=1, step=1)
BURST = Param('burst', 'On', 'number', default=50.0, unit='%', min=1.0, max=100.0, step=5.0)
TAPER = Param('taper', 'Taper', 'number', default=5.0, unit='%', min=0.0, max=49.0, step=1.0)
PERIODS = Param('periods', 'Periods', 'integer', default=10, min=1, step=1)
REPEATS = Param('repeats', 'Periods per realization, 0 for all', 'integer', default=0, min=0,
                step=1)
BAND_LOW = Param('low', 'From', 'number', default=20.0, unit='Hz', min=0.001, step=10.0)
BAND_HIGH = Param('high', 'To', 'number', default=2000.0, unit='Hz', min=0.001, step=10.0)
LEVEL = Param('level', "Level, in the channels' unit squared per Hz", 'number', default=1.0,
              min=0.0, step=0.1)
STARTS = Param('at', 'Starts', 'number', default=0.0, unit='s', min=0.0, step=0.01)
PEAK = Param('peak', "Peak, in the channels' unit", 'number', default=1.0, step=0.1)
WIDTH = Param('width', 'Impulse width', 'number', default=0.001, unit='s', min=1e-6,
              step=0.0005)
SHAPE = Param('shape', 'Pulse shape', 'enum', default='half-sine', options=(
    Option('half-sine', 'Half-sine'), Option('sawtooth', 'Sawtooth'),
    Option('trapezoid', 'Trapezoid')))
PULSE = Param('pulse_duration', 'Pulse duration', 'number', default=0.011, unit='s',
              min=1e-6, step=0.001)
EDGE = Param('edge', EDGE_WORDS, 'number', default=0.0, unit='s', min=0.0, step=0.001)
TONE = Param('tone', 'Tone', 'integer', default=1, min=1, step=1)
DOFS = Param('dofs', 'DOFs, blank for all', 'string', default='')


def _settings(source: Any, params: dict[str, Any]) -> dict[str, Any]:
    """The `SignalSettings` fields the params give, in SI; the sample
    rate the source suggests (`core.signals.suggested`) when none is
    given, as Visual Dynamics' pane opens on it."""
    from hertzforge.core.signals import suggested
    from hertzforge.core.sine import SineSweepSpecification

    tone = None
    if isinstance(source, SineSweepSpecification):
        tones = source.tones
        if params['tone'] > len(tones):
            raise refused(f'the specification has {len(tones)} tones; there is no tone '
                          f'{params["tone"]}')
        tone = tones[params['tone'] - 1].name
    return {'duration': params['duration'],
            'sample_rate': params['sample_rate'] or suggested(source).sample_rate,
            'frame_length': params['frame_length'] or None,
            'seed': params['seed'] or None,
            'correlated': params['correlated'], 'dofs': _dofs(params['dofs']),
            'bursts': params['bursts'], 'burst': params['burst'] / 100.0,
            'taper': params['taper'] / 100.0, 'periods': params['periods'],
            'repeats': params['repeats'] or None, 'at': params['at'],
            'width': params['width'], 'shape': params['shape'], 'peak': params['peak'],
            'pulse_duration': params['pulse_duration'],
            'edge': pulse_edge(params['shape'], params['edge'], params['pulse_duration']),
            'band': (params['low'], params['high']), 'level': params['level'],
            'tone': tone}


def _traces(name: str, history: Any) -> dict:
    """Every record of a time history on one time axis, its extremes kept."""
    t = np.asarray(history.abscissa, dtype=float)
    quantities = {str(d) for d in history.ordinate_dim}
    unit = unit_text(quantities.pop()) if len(quantities) == 1 else None
    lines = [v.series(history.record_label(k), 'measured', *_trace(t, history.ordinate[k]))
             for k in range(history.num_records)]
    return v.spectrum(name, TIME, v.axis('Signal', unit, 'linear'), lines, link='time')


def generate_signal(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.generate_signal`: a reference signal made from a spectrum
    (random, burst random, periodic random at its DOFs), a sine
    specification (its tone) or a channel table (the random kinds at its
    reference channels to a typed flat band and level, an impulse or a
    classical pulse), added beside its source (`Project.generate_signal`,
    `core.signals.generate`). Drawn as Visual Dynamics previews it: the
    random kinds' PSD over the target they were drawn to
    (`signals.target_at`), an impulse's trace and its spectrum
    (`signals.impulse_spectrum`), the others' trace; with the samples, the
    line spacing, the rms and the seed kept with it."""
    from hertzforge.core.signals import (
        KINDS,
        RANDOM_KINDS,
        SignalSettings,
        frame_used,
        impulse_rolloff,
        impulse_spectrum,
        target_at,
        target_of,
    )

    name, kind = params['source'], params['kind']
    source = project[name]
    settings = _settings(source, params)
    added = _core(lambda: project.generate_signal(name, kind, **settings))
    made = project[added]
    chosen = SignalSettings.from_dict(project.provenance[added]['params'])
    what = KINDS[kind].lower()
    views = []
    if kind in RANDOM_KINDS:
        drawn = _core(made.compute_psds)
        target = target_at(target_of(source, chosen), drawn.abscissa)
        f = np.asarray(drawn.abscissa, dtype=float)
        lines = []
        for k in range(drawn.num_records):
            dof = str(drawn.response_dof[k])
            lines.append(_positive(f'{dof} generated', 'measured', f, drawn.ordinate[k]))
            at = [j for j in range(target.num_records) if str(target.response_dof[j]) == dof]
            if at:
                lines.append(_positive(f'{dof} target', 'reference', f,
                                       target.ordinate[at[0]]))
        unit = unit_text(str(drawn.ordinate_dim[0]))
        views.append(v.spectrum(f'{added}: {what} over its target', FREQUENCY,
                                v.axis('PSD', unit, 'log'), lines, link='frequency'))
    else:
        views.append(_traces(f'{added}: {chosen.describe()}', made))
        if kind == 'impulse':
            spectrum = impulse_spectrum(made)
            views.append(v.spectrum(
                f'{added}: what the impulse excites', FREQUENCY,
                v.axis('Amplitude', unit_text(str(spectrum.ordinate_dim[0])), 'log'),
                [_positive('Impulse spectrum', 'measured', spectrum.abscissa,
                           np.abs(spectrum.ordinate[0]))], link='frequency'))
    rate = float(made.sample_rate)
    first = np.asarray(made.ordinate[0], dtype=float)
    rows = [v.row('Added', added, None, 'info'),
            v.row('Made', chosen.describe(), None, 'info'),
            v.row('DOFs', ', '.join(str(d) for d in made.response_dof), None, 'info'),
            v.row('Samples', float(first.size), '', 'info')]
    frame = frame_used(source, chosen)
    if frame:
        rows.append(v.row('Line spacing', rate / frame, 'Hz', 'info'))
    rows.append(v.row(f'RMS, {made.record_label(0)}', float(np.sqrt(np.mean(first ** 2))),
                      unit_text(str(made.ordinate_dim[0])), 'info'))
    if kind == 'impulse':
        nyquist = rate / 2.0
        for label, at in zip(('−3 dB at', '−10 dB at', 'First zero'),
                             impulse_rolloff(chosen.width), strict=True):
            rows.append(v.row(label, at, 'Hz', 'info') if at <= nyquist
                        else v.row(label, 'above the Nyquist frequency', None, 'info'))
    if chosen.seed is not None and kind in RANDOM_KINDS:
        rows.append(v.row('Seed', float(chosen.seed), '', 'info'))
    return [*views, v.reading(f'Generate Signal from {name}', rows)]


# ---- SRS Synthesis ------------------------------------------------------------------

REQUIREMENT = Param('requirement', 'Shock specification', 'enum',
                    options_from='objects:shock_specification')
SYNTHESIS_RATE = Param('sample_rate', 'Sample rate, 0 for the one suggested', 'number',
                       default=0.0, unit='Hz', min=0.0, step=1024.0)
HALF_CYCLES = Param('half_cycles', 'Half cycles, at least', 'integer', default=5, min=3,
                    max=15, step=2)
DRAWN = Param('record', 'Channel drawn', 'integer', default=1, min=1, step=1)


def _synthesis_spectrum(made: Any, k: int) -> dict:
    """One channel's spectrum against the requirement, with the band every
    point was put inside, as `plot.srs_tolerance` draws it under a
    synthesis."""
    from hertzforge.core.compliance import srs_band_text

    from .ops_compare import _edge

    spec, chosen = made.specification, made.channels[k]
    dof = chosen.dof
    judged = next(result for each, _event, result in made.tolerance() if each == dof)
    x = np.asarray(chosen.points, dtype=float)
    target = np.asarray(chosen.target, dtype=float)
    lines = [_positive(f'{dof} synthesized', 'measured', x, chosen.spectrum),
             _positive('Required', 'reference', x, target)]
    for band, role, which in ((judged['inner'], 'tolerance', 'Inner band'),
                              (judged['outer'], 'abort', 'Outer band')):
        if band is None:
            continue
        for edge, db in zip(('lower', 'upper'), band, strict=True):
            if np.ndim(db) or np.isfinite(db):
                first = int(np.searchsorted(np.asarray(spec.abscissa, dtype=float), x[0]))
                lines.append(_edge(f'{which}, {edge}', role, x, target,
                                   np.asarray(db)[first:first + x.size] if np.ndim(db)
                                   else db))
    return v.spectrum(f'{dof}: {judged["inside_count"]} of {judged["points"]} points inside, '
                      f'{srs_band_text(judged["inner"], judged["outer"])}', FREQUENCY,
                      v.axis('Peak response', unit_text('acceleration'), 'log'), lines,
                      link='frequency')


def srs_synthesis(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.srs_synthesis`: a shock a shaker can play, synthesized to a
    shock specification as a sum of wavelets and read against the shaker's
    shock ratings given (`core.shock_synthesis.synthesize`), added as a
    time history (`Project.generate_signal` with the kind 'srs'). Drawn as
    Visual Dynamics draws a synthesis (`plot.shock_synthesis`): the
    channel's stroke reading, then its spectrum against the requirement's
    band, with the wavelets, passes, event length and points inside."""
    import hertzforge as hz
    from hertzforge.core.shock_synthesis import suggested_rate, synthesize
    from hertzforge.core.signals import SignalSettings

    from .ops_sine_shock import stroke

    name = params['requirement']
    spec = project[name]
    rate = params['sample_rate'] or suggested_rate(spec)
    ratings = {param.name: params[param.name] or None for param in STROKE_RATINGS}
    settings = {'sample_rate': rate, 'half_cycles': params['half_cycles'],
                'dofs': _dofs(params['dofs']), **ratings}
    made = _core(lambda: synthesize(spec, sample_rate=rate,
                                    rating=SignalSettings(**settings).rating(),
                                    half_cycles=params['half_cycles'],
                                    dofs=settings['dofs'], unit_system=hz.SI))
    if params['record'] > len(made.channels):
        raise refused(f'the synthesis has {len(made.channels)} channels; there is no channel '
                      f'{params["record"]}')
    k = params['record'] - 1
    added = _core(lambda: project.generate_signal(name, 'srs', **settings))
    chosen = made.channels[k]
    judged = next(result for dof, _event, result in made.tolerance() if dof == chosen.dof)
    *readings, stroke_rows = stroke(project, {'history': added, 'record': k + 1,
                                              **{p.name: params[p.name]
                                                 for p in STROKE_RATINGS}})
    event = float(chosen.samples.max()) / float(made.history.sample_rate)
    rows = [v.row('Added', added, None, 'info'),
            v.row('Sample rate', rate, 'Hz', 'info'),
            v.row('Samples', float(made.history.ordinate.shape[1]), '', 'info'),
            v.row('Wavelets', float(chosen.points.size), '', 'info'),
            v.row('Half cycles kept', float(chosen.half_cycles), '', 'info'),
            v.row('Passes', float(chosen.passes), '', 'info'),
            v.row('Event', event, 's', 'info'),
            v.row('Inside the band', f'{judged["inside_count"]} of {judged["points"]} points',
                  None, 'pass' if judged['inside_count'] == judged['points'] else 'warn'),
            v.row('Within', chosen.worst_db, 'dB', 'info')]
    for half, limit, share in chosen.tried:
        rows.append(v.row(f'{half} half cycles', f'ran out of {limit} at {100 * share:.0f} %',
                          None, 'info'))
    rows += [row for row in stroke_rows['rows'] if row['label'] in ('Against the rating',
                                                                    'Verdict')]
    lines = made.describe(hz.SI)
    rows += [v.row('Channel', _sentence(line), None, 'info') for line in lines
             if line.startswith(tuple(f'{c.dof}:' for c in made.channels))
             and not line.startswith(f'{chosen.dof}:')]
    return [*readings, _synthesis_spectrum(made, k),
            v.reading(f'SRS synthesis of {name}, {chosen.dof}', rows)]


# ---- Compute Modal Responses --------------------------------------------------------

FORCES = Param('source', 'Forces', 'enum', options_from='objects:time_history|psd')
SHAPES = Param('shapes', 'Shape set', 'enum', options_from='objects:shape_set')
QUANTITY = Param('response', 'Response', 'enum', default='acceleration', options=(
    Option('displacement', 'Displacement'), Option('velocity', 'Velocity'),
    Option('acceleration', 'Acceleration')))
MODES = Param('modes', 'Modes, blank for all', 'string', default='')
FORCE_RECORDS = Param('records', 'Force records, blank for all', 'string', default='')


def compute_modal_responses(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.compute_modal_responses`: how each mode of a shape set
    answers the forces in a time history, or a force CPSD, added as an
    object of its own beside the forces (`Project.compute_modal_responses`,
    `core.modal_response`). Drawn as a waterfall, with the counts Visual
    Dynamics' pane gives: the modes, the force channels, the samples or
    lines and the highest mode, and a mode past the record's reach."""
    from hertzforge.core.data import Psd, channel_quantities

    name, set_name = params['source'], params['shapes']
    forces, shapes = project[name], project[set_name]
    modes = _picked(params['modes'], shapes.num_shapes, 'mode')
    records = _picked(params['records'], forces.num_records, 'record')
    extra: dict[str, Any] = {'quantity': params['response']}
    if modes is not None:
        extra['modes'] = modes
    if records is not None:
        extra['records'] = records
    added = _core(lambda: project.compute_modal_responses(name, set_name, **extra))
    view = waterfall_view(added, project[added])
    view.pop('quantities', None)
    view.pop('quantity', None)
    used = list(range(shapes.num_shapes)) if modes is None else modes
    frequency = np.asarray(shapes.frequency, dtype=float)[used]
    picked = range(forces.num_records) if records is None else records
    density = isinstance(forces, Psd)
    if density:
        count = len({str(forces.response_dof[i]) for i in picked
                     if channel_quantities(forces.known_dim(i))[0] in ('force', 'modal_force')})
        last = float(forces.abscissa[-1])
        above = int(np.sum(frequency > last))
        reach = f'above the highest line, {last:g} Hz'
    else:
        count = sum(str(forces.ordinate_dim[i]) in ('force', 'modal_force') for i in picked)
        nyquist = float(forces.sample_rate) / 2.0
        above = int(np.sum(frequency >= nyquist))
        reach = f'at or above the Nyquist frequency, {nyquist:g} Hz'
    rows = [v.row('Added', added, None, 'info'),
            v.row('Modes', f'{len(used)} of {shapes.num_shapes}', None, 'info'),
            v.row('Force channels', float(count), '', 'info'),
            v.row('Lines' if density else 'Samples', float(len(forces.abscissa)), '', 'info'),
            v.row('Highest mode', float(frequency.max()), 'Hz', 'info')]
    if above:
        rows.append(v.row('Note', f'{above} mode{"s" * (above != 1)} {reach}', None, 'warn'))
    return [view, v.reading(f'{name} through {set_name}: modal {params["response"]}', rows)]


# ---- Sweep Rate ---------------------------------------------------------------------

SWEEP = Param('specification', 'Sine specification', 'enum',
              options_from='objects:sine_sweep_specification')
SWEEP_TONE = Param('tone', 'Tone', 'integer', default=1, min=1, step=1)
SWEEP_MODES = Param('modes', 'Modes, a frequency and a Q each', 'string', default='',
                    unit='Hz Q')
MARGIN = Param('margin', 'Flag above', 'number', default=1.0, unit='× rule', min=0.05,
               max=100.0, step=0.25)
CROSSING_POINTS = 60


def parse_modes(text: str) -> list[tuple[float, float]]:
    """'22 40, 180 20' as [(22.0, 40.0), (180.0, 20.0)], as Visual
    Dynamics' Sweep Rate pane reads its field: pairs of natural frequency
    in Hz and Q, the pairs separated by commas or semicolons and the two
    numbers of a pair by spaces or a slash."""
    modes = []
    for piece in re.split(r'[;,]', text):
        piece = piece.strip()
        if not piece:
            continue
        numbers = [part for part in re.split(r'[\s/]+', piece) if part]
        try:
            frequency, q = (float(number) for number in numbers)
        except ValueError:
            raise refused(f'{piece!r} is not a frequency and a Q, such as 22 40') from None
        if not frequency > 0.0:
            raise refused(f'{piece!r}: the frequency must be positive')
        if not q > 0.5:
            raise refused(f'{piece!r}: a Q of 0.5 or less is critically damped and has no '
                          'band to sweep through')
        modes.append((frequency, q))
    return modes


def _cell(value: Any, places: int | None = None) -> str:
    if value is None or isinstance(value, float) and not np.isfinite(value):
        return ''
    if isinstance(value, float):
        return f'{value:.{places}f}' if places is not None else f'{value:.4g}'
    return str(value)


SWEPT_LEVELS = Param('levels', 'Q read off', 'enum', default='',
                     options_from='objects:sine_level_set|sine_level')


def _swept_q(project: Any, params: dict[str, Any], spec: Any, modes: list) -> list[dict]:
    """With a tracked sine level named, the Q read off it at each mode on
    each of its channels, judged against the rate it was swept at
    (`core.sweep_rate.read_swept_q`), as Visual Dynamics lists them under
    the passes when levels of the tone are selected."""
    from hertzforge.core.sweep_rate import read_swept_q

    name = params.get('levels') or ''
    if not name:
        return []
    level = project[name]
    tone = spec.tones[params['tone'] - 1]
    if hasattr(level, 'tone'):
        # a set of an extraction's levels: this tone's
        if tone.name not in level.tone_names:
            raise refused(f'{name} holds the tones {", ".join(level.tone_names)}; not '
                          f'{tone.name}')
        level = level.tone(tone.name)
    rows = []
    for record in range(level.num_records):
        dof = str(level.response_dof[record])
        for frequency, _q in modes:
            reading = _core(lambda f=frequency, r=record: read_swept_q(
                level, spec, f, record=r, margin=params['margin']))
            if reading is None:
                rows.append(v.row(f'{dof}, {frequency:g} Hz', 'no peak to read there', None,
                                  'info'))
            else:
                rows.append(v.row(f'{dof}, {frequency:g} Hz', _sentence(reading.describe()),
                                  None, 'warn' if reading.flagged else 'pass'))
    return rows


def sweep_rate(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.sweep_rate`: one tone of a sine specification checked
    against typed modes (`core.sweep_rate.check_sweep_rate`): its path as
    frequency against time with each mode's half-power band across it,
    each pass through a band marked, too fast where its rate is over the
    margin times the rule's, and every pass in Visual Dynamics' sweep-rate
    table. With a tracked level named, the Q read off it at each mode is
    judged as well (`_swept_q`)."""
    from hertzforge.core.sweep_rate import check_sweep_rate, half_power_band

    name = params['specification']
    spec = project[name]
    if params['tone'] > len(spec.tones):
        raise refused(f'{name} has {len(spec.tones)} tones; there is no tone {params["tone"]}')
    tone = spec.tones[params['tone'] - 1]
    modes = parse_modes(params['modes'])
    checks = _core(lambda: check_sweep_rate(tone, modes, margin=params['margin']))
    duration = tone.duration()
    t, f = tone.trajectory(duration / 2000.0)
    lines = [v.series(f'{tone.name} sweep', 'measured', t, f)]
    t0, t1 = float(t[0]), float(t[-1])
    rushed = {(c.frequency, c.q) for c in checks if c.too_fast}
    banded = set()
    for check in checks:
        key = (check.frequency, check.q)
        if key in banded:
            continue
        banded.add(key)
        role = 'abort' if key in rushed else 'tolerance'
        for side, edge in zip(('lower', 'upper'), half_power_band(*key), strict=True):
            lines.append(_flat(f'{check.frequency:g} Hz, Q {check.q:g}: half-power band, {side}',
                               role, t0, t1, edge))
    for check in checks:
        inside = np.linspace(check.start, check.start + check.seconds, CROSSING_POINTS)
        lines.append(v.series(
            f'{check.frequency:.4g} Hz {check.direction}: {check.ratio:.2g} × rule, '
            f'{check.verdict}', 'abort' if check.too_fast else 'drive', inside,
            tone.frequency_at(inside)))
    fast = sum(c.too_fast for c in checks)
    plot = v.spectrum(f'{tone.name}: {len(checks)} pass{"es" * (len(checks) != 1)} through '
                      f'{len(modes)} mode{"s" * (len(modes) != 1)}, {fast} too fast', TIME,
                      v.axis('Frequency', 'Hz', 'log'), lines)
    if not modes:
        return [plot, v.reading(f'Sweep rate of {name}, {tone.name}', [
            v.row('Modes', 'none given: type each mode as a frequency in Hz and a Q, such as '
                  '22 40, 180 20', None, 'warn')])]
    rows = [v.row('Passes', float(len(checks)), '', 'info'),
            v.row('Too fast', float(fast), '', 'fail' if fast else 'pass'),
            v.row('Flagged above', params['margin'], '× rule', 'info')]
    if not checks:
        rows.append(v.row('Note', 'No mode lies inside this tone\'s sweep', None, 'info'))
    rows += [v.row(f'{c.frequency:.4g} Hz, {c.direction}', _sentence(c.describe()), None,
                   'fail' if c.too_fast else 'pass') for c in checks]
    rows += _swept_q(project, params, spec, modes)
    table = v.table(
        f'Sweep rate of {name}, {tone.name}',
        ('Tone', 'Mode [Hz]', 'Q', 'Pass', 'Rate', "Rule's rate", '× Rule', 'In band [s]',
         'Cycles', 'Peak read [%]', 'Q read', 'Verdict'),
        [(tone.name, _cell(c.frequency), _cell(c.q),
          c.direction + ('' if c.crossed else ', partial'),
          f'{abs(c.rate):.3g} {c.unit}', f'{c.limit:.3g} {c.unit}', _cell(c.ratio, 2),
          _cell(c.seconds), _cell(c.cycles, 1), _cell(100.0 * c.peak_share, 1),
          _cell(c.q_read, 1), c.verdict) for c in checks])
    return [plot, table, v.reading(f'Sweep rate of {name}, {tone.name}', rows)]


# ---- Fatigue ------------------------------------------------------------------------

ACCELERATION = 'objects:psd|specification|time_history'
FIELD = Param('source', 'Record', 'enum', options_from=ACCELERATION)
FIELD_RECORD = Param('record', 'Record number', 'integer', default=1, min=1, step=1)
EXPONENT = Param('b', 'Exponent b', 'number', default=7.5, min=2.0, max=25.0, step=0.5)
FATIGUE_Q = Param('q', 'Q', 'number', default=10.0, min=1.0, max=500.0, step=1.0)
METHOD = Param('method', 'Reading of a spectrum', 'enum', default='narrowband', options=(
    Option('narrowband', 'Narrowband'), Option('rice', 'Rice'), Option('dirlik', 'Dirlik')))
FIELD_DURATION = Param('duration', 'Duration, 0 for none', 'number', default=0.0, unit='s',
                       min=0.0, step=60.0)
TEST = Param('test', 'Against', 'enum', options_from=ACCELERATION)
TEST_RECORD = Param('test_record', 'Against record, 0 for no test', 'integer', default=0,
                    min=0, step=1)
TEST_DURATION = Param('test_duration', 'Test duration, 0 for none', 'number', default=0.0,
                      unit='s', min=0.0, step=60.0)
#: the exaggeration factors the rule's line is drawn over, on the ASD
FACTORS = (0.5, 8.0)


def _seconds_words(value: float) -> str:
    from hertzforge.core.fatigue import _seconds

    return _seconds(value)


def fatigue(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.fatigue`: the extreme response and fatigue damage spectra
    of one acceleration record, a record of a spectrum or a channel of a
    time history, through a resonator at each natural frequency
    (`core.fatigue.fatigue_spectra`), and with a candidate test, the two
    set against each other on the same natural frequencies
    (`core.fatigue.compared`), as Visual Dynamics' Fatigue reading draws
    them (`plot.fatigue`): the ERS, the FDS and the time-compression rule
    with the cap and the exaggeration the test carries."""
    from hertzforge.core.data import TimeHistory
    from hertzforge.core.fatigue import CAP, compared, compression_ratio, fatigue_spectra

    name = params['source']
    data = project[name]
    record = _record(data, params['record'], name)
    given: dict[str, Any] = {'b': params['b'], 'q': params['q'], 'method': params['method']}
    pair = None
    if params['test_record']:
        test_name = params['test']
        test_data = project[test_name]
        test_record = _record(test_data, params['test_record'], test_name)
        if (test_name, test_record) == (name, record):
            raise refused('the record is set against another record, not itself')
        pair = _core(lambda: compared(data, test_data, record, test_record,
                                      field_duration=params['duration'] or None,
                                      test_duration=params['test_duration'] or None, **given))
        field = pair.field
    else:
        if isinstance(data, TimeHistory):
            given.pop('method')
        field = _core(lambda: fatigue_spectra(data, record, duration=params['duration'] or None,
                                              **given))
    unit = unit_text(field.dimension)
    sides = [(field, name, 'measured')]
    if pair is not None:
        sides.append((pair.test, params['test'], 'reference'))
    if field.source == 'time history':
        how = f"the largest in the record's {_seconds_words(field.ers_duration)}"
    else:
        how = f'expected over {_seconds_words(field.ers_duration)}'
    reading = ('rainflow' if field.source == 'time history' else f'{field.method} reading')
    natural = v.axis('Natural frequency', 'Hz', 'log')
    ers = v.spectrum(f'Extreme response: Q = {field.q:g}, {how}', natural,
                     v.axis('Extreme response', unit, 'log'),
                     [_positive(f'{label}: {spectra.source}', role, spectra.frequency,
                                spectra.ers) for spectra, label, role in sides],
                     link='frequency')
    fds = v.spectrum(f'Fatigue damage: b = {field.b:g}, {reading}, over '
                     f'{_seconds_words(field.duration)}, C = 1', natural,
                     v.axis('Damage', f'({unit})^{field.b:g}' if unit else None, 'log'),
                     [_positive(f'{label}: {spectra.source}', role, spectra.frequency,
                                spectra.fds) for spectra, label, role in sides],
                     link='frequency')
    cap = pair.cap if pair is not None else CAP['asd']
    factors = np.geomspace(*FACTORS, 64)
    allowed = compression_ratio(cap, field.b)
    ratios = np.array([compression_ratio(x, field.b) for x in factors])
    rule_lines = [v.series(f'Equal damage at b = {field.b:g}', 'measured', factors, ratios),
                  v.series(f'Cap {cap:g} on the ASD: {allowed:.4g} to 1', 'abort', [cap, cap],
                           [ratios[0], ratios[-1]])]
    title = f'Time compression at b = {field.b:g}'
    if pair is not None:
        title = _sentence(pair.describe())
        exaggeration = np.asarray(pair.exaggeration, dtype=float)
        judged = np.isfinite(exaggeration) & (exaggeration > 0.0)
        if judged.any():
            low = int(np.argmin(np.where(judged, exaggeration, np.inf)))
            high = int(np.argmax(np.where(judged, exaggeration, -np.inf)))
            rule_lines.append(v.series(
                f'{params["test"]}: {exaggeration[low]:.3g} at {pair.frequency[low]:.4g} Hz '
                f'to {exaggeration[high]:.3g} at {pair.frequency[high]:.4g} Hz', 'reference',
                [exaggeration[low], exaggeration[high]], [pair.time_ratio] * 2))
    rule = v.spectrum(title, v.axis('Exaggeration factor on the ASD', None, 'log'),
                      v.axis('Service time over test time', None, 'log'), rule_lines)
    f = field.frequency
    rows = [v.row('Field', f'{name}, {field.label}, {_seconds_words(field.duration)}', None,
                  'info'),
            v.row('Natural frequencies', f'{f.size}, {f[0]:.4g} to {f[-1]:.4g} Hz'
                  if f.size > 1 else f'{f[0]:.4g} Hz', None, 'info')]
    if pair is not None:
        test = pair.test
        rows.append(v.row('Test', f'{params["test"]}, {test.label}, '
                          f'{_seconds_words(test.duration)}', None, 'info'))
        least = 10.0 * np.log10(pair.least_ratio)
        rows.append(v.row('Least damage margin', f'{least:+.2f} dB at {pair.governs:.4g} Hz'
                          if np.isfinite(least) else 'not read', None,
                          'pass' if pair.covers else 'fail'))
        rows.append(v.row('Factor that covers', f'{pair.factor_needed:.4g} on the ASD', None,
                          'info'))
        rows.append(v.row('Duration that covers', _seconds_words(pair.duration_needed), None,
                          'info'))
        exaggeration = np.asarray(pair.exaggeration, dtype=float)
        exaggeration = exaggeration[np.isfinite(exaggeration)]
        if exaggeration.size:
            rows.append(v.row('Exaggeration', f'{exaggeration.min():.3g} to '
                              f'{exaggeration.max():.3g}, cap {pair.cap:g}', None,
                              'warn' if pair.over_cap.any() else 'info'))
        ers_db = np.asarray(pair.ers_db, dtype=float)
        ers_db = ers_db[np.isfinite(ers_db)]
        if ers_db.size:
            rows.append(v.row('Least ERS margin', float(ers_db.min()), 'dB',
                              'pass' if pair.ers_covers else 'warn'))
    rows.append(v.row('Rests on', _sentence((pair or field).rests_on), None, 'info'))
    rows.append(v.row('Verdict', _sentence((pair or field).describe()), None,
                      'info' if pair is None else 'pass' if pair.covers else 'fail'))
    notes = list(field.notes) + ([] if pair is None else
                                 [f'test: {note}' for note in pair.test.notes])
    rows += _notes(notes)
    return [ers, fds, rule, v.reading(f'Fatigue of {name}, {field.label}', rows)]


# ---- Drive Prediction ---------------------------------------------------------------

PLANT = Param('frf', 'FRF', 'enum', options_from='objects:frf')
TARGET = Param('specification', 'Specification', 'enum', options_from='objects:specification')
INVERSE = Param('method', 'Inverse', 'enum', default='truncated', options=(
    Option('truncated', 'Truncated SVD'), Option('tikhonov', 'Tikhonov')))
TOLERANCE = Param('tolerance', 'Tolerance', 'number', default=0.01, min=0.0001, max=0.5,
                  step=0.005)
WEIGHT = Param('weight', 'Tikhonov weight', 'number', default=0.001, min=1e-6, max=1.0,
               step=0.001)
RUN = Param('duration', 'Duration', 'number', default=60.0, unit='s', min=1.0, step=10.0)
DRIVE_LIMIT = Param('drive_limit', 'Drive limit, 0 for none', 'number', default=0.0, min=0.0,
                    step=1.0)
RESPONSE = Param('response', 'Response drawn, 0 for the first control channel', 'integer',
                 default=0, min=0, step=1)


def drive_prediction(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.drive_prediction`: the drives a specification asks for at
    its control channels through an FRF's regularized inverse, the
    responses they make and what they ask of the amplifiers and shakers
    (`core.drive_prediction.predict_drives`), as Visual Dynamics' Drive
    Prediction draws it (`plot.drive_prediction`): each drive's
    autospectrum, one channel's predicted response against its target
    with the lines missed shaded, and every quantity against its
    rating."""
    from hertzforge.core.drive_prediction import UNMET_DB, predict_drives
    from hertzforge.core.matrix_checks import runs
    from hertzforge.core.shaker import ShakerRating

    frf_name, spec_name = params['frf'], params['specification']
    ratings = {p.name: params[p.name] for p in STROKE_RATINGS if params[p.name]}
    shakers = _core(lambda: ShakerRating(**ratings)) if ratings else None
    prediction = _core(lambda: predict_drives(
        project[frf_name], project[spec_name], method=params['method'],
        tolerance=params['tolerance'], weight=params['weight'],
        duration=params['duration'], drive_limit=params['drive_limit'] or None,
        shakers=shakers))
    run_logs.offer(project, 'drive', prediction, f'{spec_name} through {frf_name}')
    f = np.asarray(prediction.frequency, dtype=float)
    if params['response'] > len(prediction.rows):
        raise refused(f'the FRF has {len(prediction.rows)} response channels; there is no '
                      f'channel {params["response"]}')
    row = (params['response'] - 1 if params['response']
           else int(prediction.controls[0]))
    drive_unit = unit_text(f'{prediction.drive_quantity}**2/frequency')
    drives = [d for d in prediction.demands if d.kind == 'drive']
    rated = [d for d in drives if d.share is not None]
    lead = (max(rated, key=lambda d: d.share) if rated
            else max(drives, key=lambda d: d.peak) if drives else None)
    top = v.spectrum(_sentence(lead.describe(_si())) if lead else 'Drives', FREQUENCY,
                     v.axis('Drive PSD', drive_unit, 'log'),
                     [_positive(f'drive at {label}', 'drive', f, prediction.drive_asd(j))
                      for j, label in enumerate(prediction.drives)], link='frequency')
    label = prediction.rows[row]
    quantity = prediction.row_quantities[row]
    lines = [_positive(f'{label} predicted', 'measured', f, prediction.asd[:, row])]
    regions = []
    words = f'{label}: predicted response'
    if row in prediction.controls:
        k = prediction.controls.index(row)
        lines.append(_positive('target', 'reference', f,
                               np.real(prediction.target[:, k, k])))
        off = prediction.missed(k)
        half = np.gradient(f) / 2.0 if f.size > 1 else np.zeros_like(f)
        for first, last in runs(off):
            regions.append(v.region(max(f[first] - half[first], f[first] * 0.999),
                                    f[last] + half[last], 'cut',
                                    f'Missed by more than {UNMET_DB:g} dB'))
        words = (f'{label}: the predicted response meets the target within {UNMET_DB:g} dB'
                 if not off.any() else
                 f'{label}: the predicted response misses the target by more than '
                 f'{UNMET_DB:g} dB at {int(off.sum())} line{"s" * (int(off.sum()) != 1)}, '
                 'shaded')
    else:
        limited = [d for d in prediction.demands
                   if d.kind == 'response' and d.channel == label]
        if limited:
            words = limited[0].describe(_si())
    middle = v.spectrum(_sentence(words), FREQUENCY,
                        v.axis('PSD', unit_text(f'{quantity}**2/frequency'), 'log'), lines,
                        link='frequency', regions=regions)
    lines_said = prediction.describe(_si())
    rows = [v.row('Verdict', _sentence(lines_said[0]), None,
                  {True: 'pass', False: 'fail', None: 'info'}[prediction.fits])]
    for demand in prediction.demands:
        status = 'info' if demand.share is None else 'fail' if demand.exceeds else 'pass'
        words = demand.describe(_si()).removeprefix(f'{demand.name}: ')
        rows.append(v.row(_sentence(demand.name), words, None, status))
    rows += _notes(lines_said[1 + len(prediction.demands):])
    return [top, middle, v.reading(f'Drive prediction: {spec_name} through {frf_name}', rows)]


def _si() -> Any:
    import hertzforge as hz

    return hz.SI


# ---- Notch Prediction ---------------------------------------------------------------

TRANSMISSIBILITY = Param('frf', 'Transmissibility', 'enum', options_from='objects:frf')
PROFILES = Param('profiles', 'Limit profiles, the specification for its own', 'enum',
                 options_from='objects:specification|psd')
PAIR = Param('tolerance', 'Tolerance', 'enum', default='abort', options=(
    Option('abort', 'Abort'), Option('warning', 'Warning')))


def notch_prediction(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.notch_prediction`: where each limit channel would exceed
    its limit profile at full level and the notch the controller would cut
    in the control spectrum to hold it there, predicted from a
    transmissibility from the control channel
    (`core.limit_prediction.predict_limits`), as Visual Dynamics' Notch
    Prediction draws it (`plot.limit_prediction`): the control against
    the reference with each notch shaded, each limited channel against its
    profile, and a row a notch."""
    from hertzforge.core.limit_prediction import predict_limits

    frf_name, spec_name = params['frf'], params['specification']
    profiles_name = params['profiles']
    profiles = None if profiles_name == spec_name else project[profiles_name]
    prediction = _core(lambda: predict_limits(project[frf_name], project[spec_name],
                                              profiles, tolerance=params['tolerance']))
    run_logs.offer(project, 'limit', prediction, f'{spec_name} through {frf_name}')
    f = np.asarray(prediction.frequency, dtype=float)
    half = np.gradient(f) / 2.0 if f.size > 1 else np.zeros_like(f)
    unit = unit_text(prediction.dimension)
    lines = [_positive('reference', 'reference', f, prediction.reference),
             _positive(f'{prediction.control_label} predicted control', 'measured', f,
                       prediction.control)]
    if prediction.lower is not None:
        lines.append(_positive(f'{prediction.tolerance} lower edge', 'abort'
                               if prediction.tolerance == 'abort' else 'tolerance', f,
                               prediction.lower))
    regions = [v.region(max(n.low - half[int(np.searchsorted(f, n.low))], n.low * 0.999),
                        n.high + half[min(int(np.searchsorted(f, n.high)), f.size - 1)], 'cut',
                        f'Notch {n.depth_db:.1f} dB deep at {n.frequency:.4g} Hz')
               for n in prediction.notches]
    views = [v.spectrum(_sentence(prediction.verdict()), FREQUENCY,
                        v.axis('PSD', unit, 'log'), lines, link='frequency', regions=regions)]
    for channel in prediction.limited:
        channel_unit = unit_text(channel.dimension)
        curves = [_positive(f'{channel.label} at full level', 'measured', f, channel.response)]
        if channel.profile is not None:
            curves.append(_positive(f'{channel.label} limit profile', 'limit', f,
                                    channel.profile))
        views.append(v.spectrum(_sentence(channel.describe(f, _si())), FREQUENCY,
                                v.axis('PSD', channel_unit, 'log'), curves, link='frequency'))
    table = prediction.table(_si())
    if table:
        views.append(v.table(f'Notches in {prediction.control_label}',
                             ('Band [Hz]', 'Depth [dB]', 'Control left', 'Channels',
                              'Tolerance'),
                             [(r['band'], r['depth'], r['level'], r['channels'],
                               r['tolerance']) for r in table]))
    said = prediction.describe(_si())
    verdict = prediction.within_tolerance
    rows = [v.row('Verdict', _sentence(said[0]), None,
                  {True: 'pass', False: 'fail', None: 'info'}[verdict])]
    for notch in prediction.notches:
        rows.append(v.row(f'Notch at {notch.frequency:.4g} Hz', _sentence(notch.describe(
            prediction.dimension, prediction.tolerance, _si())), None,
            'fail' if notch.outside else 'warn' if notch.outside is None else 'pass'))
    rows += [v.row(c.label, _sentence(c.describe(f, _si())), None, 'info')
             for c in prediction.channels]
    rows += _notes(said[1 + len(prediction.notches) + len(prediction.channels):])
    return [*views, v.reading(f'Notch prediction: {spec_name} through {frf_name}', rows)]


# ---- Conditioning and Realizability ---------------------------------------------------

MATRIX = Param('data', 'FRF or CPSD', 'enum', options_from='objects:frf|psd|specification')
CPSD = Param('data', 'CPSD', 'enum', options_from='objects:psd|specification')
AT = Param('frequency', 'At, 0 for the first line flagged', 'number', default=0.0, unit='Hz',
           min=0.0, step=1.0)


def _cursor(frequency: np.ndarray, flagged: np.ndarray, given: float) -> float:
    """Where Visual Dynamics puts its cursor: the frequency given, else
    the first flagged line off DC, else the middle of the span."""
    positive = frequency[frequency > 0.0]
    if given and positive.size and positive[0] <= given <= frequency[-1]:
        return given
    lit = np.flatnonzero(flagged & (frequency > 0.0))
    if lit.size:
        return float(frequency[lit[0]])
    return float(positive[positive.size // 2]) if positive.size else 1.0


def _span_regions(frequency: np.ndarray, flags: np.ndarray, label: str) -> list[dict]:
    from hertzforge.core.matrix_checks import runs

    half = np.gradient(frequency) / 2.0 if frequency.size > 1 else np.zeros_like(frequency)
    return [v.region(max(frequency[a] - half[a], frequency[a] * 0.999),
                     frequency[b] + half[b], 'cut', label)
            for a, b in runs(flags & (frequency > 0.0))]


def conditioning(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.conditioning`: the singular values and condition number of
    an FRF from several references, or of a CPSD, at every line
    (`Frf.conditioning`, `Psd.conditioning`: `core.matrix_checks`), as
    Visual Dynamics' Conditioning reading draws them
    (`plot.matrix_checks`), and the reading at one line as its pane gives
    it at the cursor."""
    from hertzforge.units import UNKNOWN

    name = params['data']
    reading = _core(project[name].conditioning)
    f = np.asarray(reading.frequency, dtype=float)
    judged = reading.judged & (f > 0.0)
    unit = None if reading.dimension == UNKNOWN else unit_text(reading.dimension)
    lines = [_positive(f'σ{k + 1}', 'measured' if k == 0 else 'reference', f[judged],
                       reading.singular[k][judged]) for k in range(reading.size)]
    lines.append(_positive(f'σ1 / {reading.limit:g}, the rank threshold', 'tolerance',
                           f[judged], reading.singular[0][judged] / reading.limit))
    top = v.spectrum(_sentence(reading.describe()), FREQUENCY,
                     v.axis('Singular value', unit, 'log'), lines, link='frequency')
    condition = reading.condition
    bottom = v.spectrum(f'Condition number (limit {reading.limit:g})', FREQUENCY,
                        v.axis('Condition number', None, 'log'),
                        [_positive('Condition number', 'measured', f[judged],
                                   condition[judged]),
                         _positive('Limit', 'abort', f[judged],
                                   np.full(int(judged.sum()), reading.limit))],
                        link='frequency',
                        regions=_span_regions(f, reading.ill, 'Over the limit'))
    point = reading.at(_cursor(f, reading.ill, params['frequency']))
    shown = not np.isnan(point.condition)
    rows = [v.row('Matrix', reading.title, None, 'info'),
            v.row('At', point.frequency, 'Hz', 'info')]
    if shown:
        rows += [v.row('Condition number', f'{point.condition:.4g} '
                       f'({"over" if point.ill else "within"} {point.limit:g})', None,
                       'warn' if point.ill else 'pass'),
                 v.row('Effective rank', f'{point.rank} of {point.size}', None, 'info'),
                 v.row('Singular values', ', '.join(f'{s:.3g}' for s in point.singular)
                       + (f' {unit}' if unit else ''), None, 'info')]
    else:
        rows.append(v.row('Condition number', 'not judged', None, 'info'))
    rows.append(v.row('Verdict', _sentence(reading.describe()), None,
                      'warn' if reading.ill.any() else 'pass'))
    notes = list(reading.notes)
    if shown and point.ill:
        notes.insert(0, 'at this line an inverse of the matrix amplifies an error in it by up '
                        'to the condition number: the references reach the rows in nearly the '
                        'same proportions')
    rows += _notes(notes)
    return [top, bottom, v.reading(f'Conditioning of {name}', rows)]


def realizability(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.realizability`: whether some set of signals has a CPSD,
    line by line, from the eigenvalues of its coherence matrix
    (`Psd.realizability`: `core.matrix_checks`), as Visual Dynamics'
    Realizability reading draws it (`plot.matrix_checks`): the smallest
    and largest eigenvalue with the failing lines shaded, and for the pair
    the repair moves most, its coherence and phase before and after; and
    the reading at one line as its pane gives it at the cursor. Make
    Realizable adds the repair."""
    name = params['data']
    reading = _core(project[name].realizability)
    f = np.asarray(reading.frequency, dtype=float)
    judged = reading.judged & (f > 0.0)
    n = len(reading.channels)
    lines = [_finite('smallest eigenvalue', 'measured', f[judged], reading.smallest[judged]),
             _finite('largest eigenvalue', 'reference', f[judged], reading.largest[judged])]
    lines += [_finite(f'eigenvalue {k + 1}', 'other', f[judged], reading.eigenvalues[k][judged])
              for k in range(1, n - 1)]
    lines.append(v.series('zero', 'tolerance', [f[judged][0], f[judged][-1]], [0.0, 0.0])
                 if judged.any() else v.series('zero', 'tolerance', [], []))
    views = [v.spectrum(_sentence(reading.describe()), FREQUENCY,
                        v.axis('Eigenvalue of the coherence matrix', None, 'linear'), lines,
                        link='frequency',
                        regions=_span_regions(f, reading.failing, 'Not realizable'))]
    worst = reading.worst()
    if worst is not None:
        i, j, _line = worst
        before, after, phase_before, phase_after = reading.pair(i, j)
        pair = f'{reading.channels[i]} and {reading.channels[j]}'
        views.append(v.spectrum(f'{pair}: coherence', FREQUENCY,
                                v.axis('Coherence', None, 'linear'),
                                [_finite(f'{pair} as written', 'measured', f[judged],
                                         before[judged]),
                                 _finite(f'{pair} repaired', 'reference', f[judged],
                                         after[judged])], link='frequency'))
        views.append(v.spectrum(f'{pair}: phase', FREQUENCY, v.axis('Phase', '°', 'linear'),
                                [_finite(f'{pair} as written', 'measured', f[judged],
                                         phase_before[judged]),
                                 _finite(f'{pair} repaired', 'reference', f[judged],
                                         phase_after[judged])], link='frequency'))
    point = reading.at(_cursor(f, reading.failing, params['frequency']))
    shown = bool(point.eigenvalues) and not np.isnan(point.eigenvalues[0])
    rows = [v.row('Channels', float(n), '', 'info'),
            v.row('At', point.frequency, 'Hz', 'info')]
    if shown:
        rows += [v.row('This line', 'realizable' if point.realizable else 'not realizable',
                       None, 'pass' if point.realizable else 'fail'),
                 v.row('Smallest eigenvalue', point.eigenvalues[0], '', 'info'),
                 v.row('Largest eigenvalue', point.eigenvalues[-1], '', 'info'),
                 v.row('Rank', f'{point.rank} of {len(point.eigenvalues)}', None, 'info'),
                 v.row('Off Hermitian', point.asymmetry, '', 'info')]
    else:
        rows.append(v.row('This line', 'not judged', None, 'info'))
    coherence, phase = reading.changes()
    rows.append(v.row('Repair moves', f'coherence {coherence:.3g}, phase {phase:.3g}°'
                      if not reading.realizable and reading.judged.any() else 'nothing',
                      None, 'info'))
    rows.append(v.row('Verdict', _sentence(reading.describe()), None,
                      'pass' if reading.realizable else 'fail'))
    notes = list(reading.notes)
    if shown and not point.realizable:
        notes.insert(0, 'at this line no set of signals has this matrix: a combination of the '
                        'channels is asked for negative power')
    rows += _notes(notes)
    return [*views, v.reading(f'Realizability of {name}', rows)]


OPERATIONS = (
    Operation(
        id='project.generate_signal', title='Generate Signal', workspace='signals',
        summary='A signal made from a spectrum, a sine specification or a channel table\'s '
                'reference channels, added beside its source: random, burst random, '
                'periodic random, an impulse, a classical pulse or a sine. Only the settings '
                'its kind reads are used.',
        params=(SIGNAL_SOURCE, KIND, SAMPLE_RATE, DURATION, FRAME, SEED, CROSS_TERMS, BURSTS,
                BURST, TAPER, PERIODS, REPEATS, BAND_LOW, BAND_HIGH, LEVEL, STARTS, PEAK,
                WIDTH, SHAPE, PULSE, EDGE, TONE, DOFS),
        outputs=('spectrum', 'reading'), run=generate_signal),
    Operation(
        id='project.srs_synthesis', title='SRS Synthesis', workspace='shock',
        summary='A shock a shaker can play, synthesized as a sum of wavelets to meet a shock '
                'specification and read against the shaker\'s ratings, added as a time '
                'history.',
        params=(REQUIREMENT, SYNTHESIS_RATE, HALF_CYCLES, *STROKE_RATINGS, DOFS, DRAWN),
        outputs=('spectrum', 'reading'), run=srs_synthesis),
    Operation(
        id='project.compute_modal_responses', title='Compute Modal Responses',
        workspace='modal',
        summary='How each mode of a shape set answers the forces in a time history or a '
                'force CPSD, added as an object of its own, one record per mode.',
        params=(FORCES, SHAPES, QUANTITY, MODES, FORCE_RECORDS),
        outputs=('waterfall', 'reading'), run=compute_modal_responses),
    Operation(
        id='project.sweep_rate', title='Sweep Rate', workspace='sine',
        summary='One tone of a sine specification against typed modes, each a frequency and '
                'a Q: every pass through a mode\'s half-power band, and whether it is swept '
                'too fast for the mode to build.',
        params=(SWEEP, SWEEP_TONE, SWEEP_MODES, MARGIN, SWEPT_LEVELS),
        outputs=('spectrum', 'table', 'reading'), run=sweep_rate),
    Operation(
        id='project.fatigue', title='Fatigue', workspace='specify',
        summary='The extreme response and fatigue damage spectra of one acceleration record, '
                'and with a candidate test, the margin, factor and duration that cover the '
                'record at every natural frequency.',
        params=(FIELD, FIELD_RECORD, EXPONENT, FATIGUE_Q, METHOD, FIELD_DURATION, TEST,
                TEST_RECORD, TEST_DURATION),
        outputs=('spectrum', 'reading'), run=fatigue),
    Operation(
        id='project.drive_prediction', title='Drive Prediction', workspace='specify',
        summary='The drives a specification asks for through an FRF\'s regularized inverse, '
                'the response they make, and what they ask of the amplifiers and shakers.',
        params=(PLANT, TARGET, INVERSE, TOLERANCE, WEIGHT, RUN, DRIVE_LIMIT,
                *STROKE_RATINGS, RESPONSE),
        outputs=('spectrum', 'reading'), run=drive_prediction),
    Operation(
        id='project.notch_prediction', title='Notch Prediction', workspace='specify',
        summary='Where each limit channel would exceed its profile at full level, from a '
                'transmissibility from the control channel, and the notch the controller '
                'would cut in the control to hold it there.',
        params=(TRANSMISSIBILITY, TARGET, PROFILES, PAIR),
        outputs=('spectrum', 'table', 'reading'), run=notch_prediction),
    Operation(
        id='project.conditioning', title='Conditioning', workspace='modal',
        summary='The singular values and condition number of an FRF from several references '
                'or of a CPSD at every line, read at one frequency.',
        params=(MATRIX, AT), outputs=('spectrum', 'reading'), run=conditioning),
    Operation(
        id='project.realizability', title='Realizability', workspace='random',
        summary='Whether any set of signals has a CPSD, line by line, from the eigenvalues of '
                'its coherence matrix, read at one frequency.',
        params=(CPSD, AT), outputs=('spectrum', 'reading'), run=realizability),
)
