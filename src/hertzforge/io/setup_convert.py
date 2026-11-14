"""Conversions between a control test's tables and the analysis objects
they describe, written once, both ways.

The control service is configured in its own forms: a Random profile of
breakpoints in g²/Hz, a Sine profile of rows in g, in/s or inches peak
to peak, an SRS target of points in g, a classical pulse of a shape, a
peak in g and a duration in milliseconds, a TWR reference of samples in
the control channel's unit. The analysis side holds the same
requirements as objects, in SI: a `Specification`, a
`SineSweepSpecification`, a `ShockSpecification`, a `ClassicalPulse`, a
`TransientSpecification`. Each function here goes one way between them;
the run converter (`io.run_convert`) uses the same functions to turn a
run's setup into objects, and the seeds (`io.seeds`) use their inverses
to fill a test's setup from an object.

Units are never implied. Every scale comes from the package's unit code
(`units.si_transform`): `G` is one g in m/s², `IPS` one inch a second
in m/s, `INCH` one inch in metres; a profile's value in inches peak to
peak is twice the peak displacement. A band in decibels is a ratio of
amplitudes (20 log10) on a Sine profile and an SRS target, and of
densities (10 log10) on a Random profile, as the control service reads
each.

What a test cannot control to is refused with the reason
(`ConversionRefused`), never adapted: a Random profile is one
autospectrum, so a specification with cross terms or with channels at
different levels is refused; a Sine test plays one tone at one rate on
one law; the SRS module holds one band over its target, at maximax.

Tolerance: a conversion there and back gives the same numbers within
`TOLERANCE` (relative) and its bands within `DB_TOLERANCE`; a recognized
classical pulse is within `PULSE_TOLERANCE` of the waveform it was read
from, as a share of its peak.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any

import numpy as np

from ..units import UNKNOWN, dimension_of, si_transform

#: one g, in m/s²
G = si_transform('g')[0]
#: one inch a second, in m/s
IPS = si_transform('inch/s')[0]
#: one inch, in metres
INCH = si_transform('inch')[0]

#: a round trip agrees to this, relative
TOLERANCE = 1e-6
#: bands agree, and a band is read as one band, to this many decibels
DB_TOLERANCE = 1e-4
#: a waveform is a classical pulse when it is within this share of its
#: peak of the pulse read from it, at every sample
PULSE_TOLERANCE = 0.02

#: a reference longer than this many samples is imported as a file
MOST_REFERENCE_SAMPLES = 1 << 20

#: the density dimension a Random profile is of
_ACCEL_DENSITY = 'acceleration**2/frequency'

#: a Sine profile row's quantity, by the dimension of the analysis
#: object, and the factor from the row's value to SI peak
SINE_QUANTITIES = {'acceleration': ('accel_g', G),
                   'velocity': ('velocity_ips', IPS),
                   # a row's displacement is peak to peak; the analysis holds the peak
                   'displacement': ('disp_pp_in', INCH / 2.0)}

#: a classical pulse's shape in the analysis, by its name on the wire
PULSE_SHAPES = {'half_sine': 'half-sine', 'terminal_peak_sawtooth': 'sawtooth',
                'trapezoid': 'trapezoid'}


class ConversionRefused(ValueError):
    """An object a test cannot control to, or a table no analysis object
    holds, with the reason in words."""


def _close(a: Any, b: Any, rtol: float = TOLERANCE) -> bool:
    return bool(np.allclose(np.asarray(a, dtype=float), np.asarray(b, dtype=float),
                            rtol=rtol, atol=0.0))


def _band(values: Sequence[float] | None, default: Sequence[float]) -> tuple[float, float]:
    """A wire band `[below, above]`, positive decibels."""
    below, above = values if values is not None else default
    return float(below), float(above)


def _db_pairs(target: np.ndarray, lower: np.ndarray, upper: np.ndarray,
              per: float) -> np.ndarray:
    """`[below, above]` in decibels at each point: `per` 20 for
    amplitudes, 10 for densities."""
    with np.errstate(divide='ignore', invalid='ignore'):
        below = per * np.log10(target / lower)
        above = per * np.log10(upper / target)
    return np.stack([below, above], axis=-1)


def _uniform(pairs: np.ndarray) -> bool:
    return bool(len(pairs) and np.all(np.abs(pairs - pairs[0]) <= DB_TOLERANCE))


def _wire_db(pair: Sequence[float]) -> list[float]:
    """A band as the wire carries it, to the microdecibel."""
    return [round(float(pair[0]), 6), round(float(pair[1]), 6)]


# ---- Random: a profile and a Specification ---------------------------------------------------

def specification_from_random_profile(profile: Mapping[str, Any], channels: Sequence[str],
                                      comment: str = '') -> Any:
    """A Random profile (its breakpoints in g²/Hz, a level at every row)
    as the analysis holds a target: a `Specification` asked of each of
    `channels`, in SI, its alarm band as the warning limits and its abort
    band as the abort limits. A row's bands hold from that row to the
    next, the profile's own where a row has none, as the control service
    reads them.

    Raises
    ------
    ConversionRefused
        When a row gives no level (a slope row: resolve the profile
        first) or there are no channels.
    """
    from ..core.author import SpecificationDraft

    points = [p for p in profile.get('breakpoints') or [] if isinstance(p, Mapping)]
    if len(points) < 2:
        raise ConversionRefused('the profile has fewer than two breakpoints')
    try:
        frequencies = [float(p['freq_hz']) for p in points]
        levels = [float(p['psd_g2_hz']) for p in points]
    except (KeyError, TypeError, ValueError) as error:
        raise ConversionRefused('the profile does not give every breakpoint as freq_hz and '
                                'psd_g2_hz') from error
    if not channels:
        raise ConversionRefused('no channel holds the profile')
    scale = G ** 2

    def band(name: str, default: Sequence[float]) -> list[tuple[float, float]]:
        # the wire's [below, above] in positive dB, as the draft holds a
        # band: signed, below the target negative
        own = profile.get(name, default)
        return [(-float(below), float(above))
                for below, above in (p.get(name) or own for p in points[:-1])]

    warning, abort = band('alarm_db', (3.0, 3.0)), band('abort_db', (6.0, 6.0))
    draft = SpecificationDraft(
        channels=list(channels), dims=['acceleration'] * len(channels),
        frequencies=frequencies, levels=[[v * scale for v in levels] for _ in channels],
        bands=[{'warning': warning, 'abort': abort,
                'symmetric': all(lo == -hi for lo, hi in warning + abort),
                'uniform': len(set(warning)) == 1 and len(set(abort)) == 1}
               for _ in channels])
    spec = draft.make(comment or 'a Random profile')
    spec.ordinate_unit = ['g'] * spec.num_records
    return spec


def _has_limit(spec: Any, kind: str, record: int) -> bool:
    """Whether `spec` gives both sides of its `kind` ('warning' or
    'abort') limit at `record`, somewhere in its band."""
    sides = [spec.limits.get(f'{kind}_{side}') for side in ('lower', 'upper')]
    return all(s is not None and np.isfinite(np.real(s[record])).any() for s in sides)


def random_profile(spec: Any, channel: str | None = None, *,
                   floor: bool = False) -> dict[str, Any]:
    """The Random profile a `Specification` asks for: its breakpoints in
    g²/Hz and its bands as decibels, the inverse of
    `specification_from_random_profile`.

    Parameters
    ----------
    spec : Specification
        Autospectra of acceleration, with no cross term.
    channel : str, optional
        The channel whose autospectrum is the profile. Needed when the
        channels' autospectra differ.

    Returns
    -------
    dict
        `breakpoints` (rows of `freq_hz` and `psd_g2_hz`, with a row's
        own `alarm_db` or `abort_db` where the bands vary), `alarm_db`
        and `abort_db` (one band for the profile, or None where the bands
        vary or the specification has no such limit), `channel` (the
        channel read), `limits` (which kinds the specification gave) and
        `notes` (what the reading did, in words). With `floor` the
        specification is a Mixed Mode floor, and its narrowbands are
        left to `mixed_parts`.

    Raises
    ------
    ConversionRefused
        For an object that is not a random specification, has cross
        terms or (unless `floor`) narrowbands, is not of acceleration, or whose channels
        differ when no channel is named.
    """
    from ..core.author import SpecificationDraft
    from ..core.data import Specification, autospectrum

    if not isinstance(spec, Specification):
        raise ConversionRefused(f'a {type(spec).__name__} is not a random specification')
    if getattr(spec, 'narrowbands', None) and not floor:
        raise ConversionRefused('it holds narrowbands, which a Random test does not play; it '
                                'seeds Mixed Mode')
    autos = [i for i in range(spec.num_records) if autospectrum(spec, i)]
    if len(autos) != spec.num_records:
        raise ConversionRefused('it holds cross terms between channels, which a Random test '
                                'does not control; it seeds MIMO Random')
    for i in autos:
        dim = spec.ordinate_dim[i]
        if dim == UNKNOWN:
            raise ConversionRefused(f'{spec.response_dof[i]} has no declared units; declare '
                                    'them first')
        if dim != _ACCEL_DENSITY:
            raise ConversionRefused(f'{spec.response_dof[i]} is a density of {dim}, and a '
                                    'Random profile is of acceleration (g²/Hz)')
    dofs = [spec.response_dof[i] for i in autos]
    if channel is None:
        first = autos[0]
        same = all(_close(np.real(spec.ordinate[i]), np.real(spec.ordinate[first]))
                   for i in autos)
        if not same:
            raise ConversionRefused(
                f'its {len(autos)} channels ({", ".join(dofs[:4])}) ask for different levels, '
                'and a Random test controls every control channel to one profile; choose one '
                'channel')
        channel = dofs[0]
    elif channel not in dofs:
        raise ConversionRefused(f'it has no autospectrum at {channel}')
    record = autos[dofs.index(channel)]
    try:
        draft = SpecificationDraft.from_specification(spec, channels=[channel])
    except ValueError as error:
        raise ConversionRefused(str(error)) from error
    notes = list(draft.notes)
    if draft.form == 'lines':
        draft = draft.to_breakpoints()
        notes = list(draft.notes)
    levels = np.asarray(draft.levels[0], dtype=float) / G ** 2
    rows: list[dict[str, Any]] = [{'freq_hz': float(f), 'psd_g2_hz': float(v)}
                                  for f, v in zip(draft.frequencies, levels, strict=True)]
    out: dict[str, Any] = {'breakpoints': rows, 'alarm_db': None, 'abort_db': None,
                           'channel': channel, 'limits': [], 'notes': notes}
    for kind, wire in (('warning', 'alarm_db'), ('abort', 'abort_db')):
        if not _has_limit(spec, kind, record):
            continue
        out['limits'].append(kind)
        pairs = np.array([(-lo, hi) for lo, hi in draft.bands[0][kind]], dtype=float)
        if np.any(pairs < 0):
            raise ConversionRefused(f'its {kind} limit crosses the target, so it is no band '
                                    'around it')
        if _uniform(pairs):
            out[wire] = _wire_db(pairs[0])
        else:
            for row, pair in zip(rows, pairs, strict=False):
                row[wire] = _wire_db(pair)
    return out


# ---- Sine: a profile and a SineSweepSpecification --------------------------------------------

def sine_tone(test: Mapping[str, Any], frequencies: Any, target: Any,
              limits: Mapping[str, Any], columns: int, name: str = 'Tone 1') -> Any:
    """The tone a Sine test plays, as the analysis holds one: a sweep is
    one pass across its band in the direction it starts, at its rate on
    its law; a dwell holds its frequency for its time. `frequencies` (Hz)
    and `target` (SI peak) are the profile at its points, `limits` the
    curves {'warning_lower': values, ...} at the same points, each asked
    of `columns` control channels alike.

    Raises
    ------
    ConversionRefused
        For a test that is neither a sweep nor a dwell.
    """
    from ..core.sine import LINEAR, LOG, SineTone

    kind = test.get('kind')
    frequencies = np.asarray(frequencies, dtype=np.float64)
    target = np.asarray(target, dtype=np.float64)

    def held(values: Any) -> np.ndarray:
        return np.tile(np.asarray(values, dtype=np.float64)[:, np.newaxis], (1, columns))

    if kind == 'dwell':
        dwell = test.get('dwell') or test.get('end') or {}
        frequency = float(test['freq_hz'])
        seconds = (float(dwell['seconds']) if 'seconds' in dwell
                   else float(dwell.get('cycles', 0)) / frequency)
        at = np.interp(frequency, frequencies, target)
        return SineTone.dwell(name, frequency, np.full(columns, at), seconds,
                              **{k: np.full(columns, np.interp(frequency, frequencies, v))
                                 for k, v in limits.items()})
    if kind != 'sweep':
        raise ConversionRefused(f'a {kind or "Sine"} test plays no one tone the analysis holds')
    rate = test.get('rate') or {}
    law = LOG if rate.get('law') == 'log' else LINEAR
    speed = float(rate['oct_per_min'] if law == LOG else rate['hz_per_s'])
    order = np.argsort(frequencies)
    if test.get('direction') == 'down':
        order, speed = order[::-1], -speed
    return SineTone(name, 0.0, frequencies[order], held(target[order]),
                    [law] * (len(order) - 1), [speed] * (len(order) - 1),
                    **{k: held(np.asarray(v, dtype=np.float64)[order])
                       for k, v in limits.items()})


def sine_specification_from_profile(profile: Mapping[str, Any], test: Mapping[str, Any],
                                    channels: Sequence[str], comment: str = '') -> Any:
    """A Sine profile and its test as a `SineSweepSpecification`: the
    rows' values in SI, the bands (a row's own, else the profile's; a
    segment takes its left row's, as the control service reads it) as
    the tone's warning and abort curves at the rows.

    Raises
    ------
    ConversionRefused
        For rows of more than one quantity, which no one tone holds.
    """
    from ..core.sine import SineSweepSpecification

    rows = [r for r in profile.get('rows') or [] if isinstance(r, Mapping)]
    quantities = {r.get('quantity') for r in rows}
    if len(rows) < 2 or len(quantities) != 1:
        raise ConversionRefused('a tone holds rows of one quantity, at least two of them')
    quantity = quantities.pop()
    found = [(dim, scale) for dim, (q, scale) in SINE_QUANTITIES.items() if q == quantity]
    if not found:
        raise ConversionRefused(f'{quantity!r} is no Sine quantity')
    dim, scale = found[0]
    frequencies = np.array([float(r['freq_hz']) for r in rows])
    target = np.array([float(r['value']) for r in rows]) * scale
    limits = {}
    for kind, wire, default in (('warning', 'alarm_db', (1.5, 1.5)),
                                ('abort', 'abort_db', (3.0, 3.0))):
        own = _band(profile.get(wire), default)
        bands = [_band(r.get(wire), own) for r in rows[:-1]]
        bands.append(bands[-1])
        below = np.array([b for b, _a in bands])
        above = np.array([a for _b, a in bands])
        limits[f'{kind}_lower'] = target * 10.0 ** (-below / 20.0)
        limits[f'{kind}_upper'] = target * 10.0 ** (above / 20.0)
    tone = sine_tone(test, frequencies, target, limits, len(channels))
    unit = {'acceleration': 'm/s**2', 'velocity': 'm/s', 'displacement': 'm'}[dim]
    return SineSweepSpecification([tone], list(channels), ordinate_dim=dim, ordinate_unit=unit,
                                  comment=comment or 'a Sine profile')


def sine_profile(spec: Any) -> dict[str, Any]:
    """The Sine profile and test a `SineSweepSpecification` asks for, the
    inverse of `sine_specification_from_profile`.

    Returns
    -------
    dict
        `rows` (`freq_hz`, `quantity`, `value`, with a row's own bands
        where they vary), `alarm_db` and `abort_db` (one band, or None),
        `test` (a sweep of one pass at its rate on its law, or a dwell),
        `limits` (which kinds the tone gave) and `notes`.

    Raises
    ------
    ConversionRefused
        For several tones at once, channels asked for different levels,
        a sweep that changes law or rate or turns back, a dwell inside a
        sweep, or a quantity a Sine profile does not hold.
    """
    from ..core.sine import LOG, SineSweepSpecification

    if not isinstance(spec, SineSweepSpecification):
        raise ConversionRefused(f'a {type(spec).__name__} is not a sine specification')
    if len(spec.tones) != 1:
        raise ConversionRefused(f'it plays {len(spec.tones)} tones at once, and a Sine test '
                                'plays one; Mixed Mode plays several')
    if spec.ordinate_dim not in SINE_QUANTITIES:
        raise ConversionRefused(f'it is in {spec.ordinate_dim}, which a Sine profile does not '
                                'hold (acceleration, velocity or displacement)')
    quantity, scale = SINE_QUANTITIES[spec.ordinate_dim]
    tone = spec.tones[0]
    amplitude = tone.amplitude
    if not all(_close(amplitude[:, c], amplitude[:, 0]) for c in range(amplitude.shape[1])):
        raise ConversionRefused('its control channels are asked for different levels, and a '
                                'Sine test controls them all to one profile')
    notes = []
    if tone.start_time:
        notes.append(f'the tone starts {tone.start_time:g} s into its test; the Sine test '
                     'starts it at once')
    n = len(tone.frequency)
    if n == 2 and tone.is_dwell(0):
        order = np.array([0, 1])
        test: dict[str, Any] = {'kind': 'dwell', 'freq_hz': float(tone.frequency[0]),
                                'end': {'seconds': float(f'{tone.duration():.9g}')}}
    else:
        if any(tone.is_dwell(s) for s in range(n - 1)):
            raise ConversionRefused('its tone holds a dwell inside a sweep, which a Sine test '
                                    'does not play')
        laws = {int(t) for t in tone.segment_type}
        rates = np.abs(tone.segment_rate)
        steps = np.sign(np.diff(tone.frequency))
        if len(laws) != 1 or not _close(rates, rates[0]):
            raise ConversionRefused('its tone changes its sweep law or rate, and a Sine test '
                                    'sweeps at one rate on one law')
        if len(set(steps)) != 1:
            raise ConversionRefused('its tone turns back, and a Sine test sweeps one way at a '
                                    'time')
        law = laws.pop()
        down = steps[0] < 0
        order = np.argsort(tone.frequency)
        rate = ({'law': 'log', 'oct_per_min': float(rates[0])} if law == LOG
                else {'law': 'linear', 'hz_per_s': float(rates[0])})
        test = {'kind': 'sweep',
                'band_hz': [float(tone.frequency.min()), float(tone.frequency.max())],
                'rate': rate, 'start_hz': float(tone.frequency[0]),
                'direction': 'down' if down else 'up', 'end': {'sweeps': 1}}
    frequency = tone.frequency[order]
    target = amplitude[order, 0]
    rows: list[dict[str, Any]] = [{'freq_hz': float(f), 'quantity': quantity,
                                   'value': float(v / scale)}
                                  for f, v in zip(frequency, target, strict=True)]
    out: dict[str, Any] = {'rows': rows, 'alarm_db': None, 'abort_db': None, 'test': test,
                           'limits': [], 'notes': notes}
    for kind, wire in (('warning', 'alarm_db'), ('abort', 'abort_db')):
        lower, upper = tone.limits.get(f'{kind}_lower'), tone.limits.get(f'{kind}_upper')
        if lower is None or upper is None:
            continue
        out['limits'].append(kind)
        pairs = _db_pairs(target, lower[order, 0], upper[order, 0], 20.0)
        if not np.all(np.isfinite(pairs)) or np.any(pairs < 0):
            raise ConversionRefused(f'its {kind} limit is no band around the tone')
        segments = pairs[:-1] if len(pairs) > 1 else pairs
        if _uniform(pairs):
            out[wire] = _wire_db(pairs[0])
        else:
            notes.append(f'its {kind} band varies; each row holds its own, from that row to '
                         'the next')
            for row, pair in zip(rows, segments, strict=False):
                row[wire] = _wire_db(pair)
    return out


# ---- SRS: a target and a ShockSpecification --------------------------------------------------

def shock_specification_from_target(target: Mapping[str, Any], bands: Mapping[str, Any],
                                    envelope: Mapping[str, Any] | None, channel: str,
                                    comment: str = '') -> Any:
    """An SRS target (points in g, at the bands' Q) as a
    `ShockSpecification` at `channel`, in SI, maximax, its alarm band as
    the warning limits and its abort band as the abort limits (amplitude
    decibels; the module's defaults where the envelope gives none).

    Raises
    ------
    ConversionRefused
        For a flat target, which has no points, or per-band bands, which
        are on the module's analysis bands and not on the target.
    """
    from ..core.data import ShockSpecification

    if target.get('kind') != 'breakpoints':
        raise ConversionRefused('a flat target has no points of its own')
    points = np.asarray(target.get('points') or [], dtype=float)
    if points.ndim != 2 or points.shape[0] < 2 or points.shape[1] != 2:
        raise ConversionRefused('the target has fewer than two points')
    envelope = envelope or {}
    if envelope.get('per_band'):
        raise ConversionRefused('the bands are per analysis band, not on the target')
    level = points[:, 1] * G
    limits = {}
    for kind, wire, default in (('warning', 'alarm_db', (3.0, 3.0)),
                                ('abort', 'abort_db', (9.0, 6.0))):
        below, above = _band(envelope.get(wire), default)
        limits[f'{kind}_lower'] = (level * 10.0 ** (-below / 20.0))[np.newaxis]
        limits[f'{kind}_upper'] = (level * 10.0 ** (above / 20.0))[np.newaxis]
    return ShockSpecification(
        abscissa=points[:, 0], ordinate=level[np.newaxis], response_dof=[channel],
        ordinate_dim=['acceleration'], ordinate_unit=['g'],
        comment=comment or 'an SRS target', q=bands.get('q', 10.0), kind='maximax', **limits)


def srs_target(spec: Any, channel: str | None = None) -> dict[str, Any]:
    """The SRS target a `ShockSpecification` asks for: its points in g,
    the band they span and their Q, and its bands in decibels, the
    inverse of `shock_specification_from_target`.

    Returns
    -------
    dict
        `target`, `bands` (`f_lo_hz`, `f_hi_hz`, and `q` when the
        specification states one), `alarm_db` and `abort_db` (or None
        where it has no such limit), `channel`, `limits` and `notes`.

    Raises
    ------
    ConversionRefused
        For an object that is not a shock specification, not maximax,
        not of acceleration, whose channels differ when none is named, or
        whose bands vary from point to point.
    """
    from ..core.data import ShockSpecification

    if not isinstance(spec, ShockSpecification):
        raise ConversionRefused(f'a {type(spec).__name__} is not a shock specification')
    kind = str(getattr(spec, 'kind', 'maximax') or 'maximax')
    if kind != 'maximax':
        raise ConversionRefused(f'it is a {kind} SRS, and the SRS module controls to maximax')
    dofs = list(spec.response_dof)
    for i, dim in enumerate(spec.ordinate_dim):
        if dim == UNKNOWN:
            raise ConversionRefused(f'{dofs[i]} has no declared units; declare them first')
        if dim != 'acceleration':
            raise ConversionRefused(f'{dofs[i]} is of {dim}, and an SRS target is of '
                                    'acceleration (g)')
    if channel is None:
        if not all(_close(spec.ordinate[i], spec.ordinate[0]) for i in range(len(dofs))):
            raise ConversionRefused(f'its {len(dofs)} channels ask for different spectra; '
                                    'choose one channel')
        channel = dofs[0]
    elif channel not in dofs:
        raise ConversionRefused(f'it has no record at {channel}')
    record = dofs.index(channel)
    frequency = np.asarray(spec.abscissa, dtype=float)
    level = np.real(np.asarray(spec.ordinate[record], dtype=float))
    if np.any(frequency <= 0) or np.any(np.diff(frequency) <= 0) or np.any(level <= 0):
        raise ConversionRefused('its points are not positive levels at ascending positive '
                                'frequencies')
    bands: dict[str, Any] = {'f_lo_hz': float(frequency[0]), 'f_hi_hz': float(frequency[-1])}
    notes = []
    if spec.q is not None:
        bands['q'] = float(spec.q)
    else:
        notes.append('it states no Q; the setup keeps its own')
    out: dict[str, Any] = {
        'target': {'kind': 'breakpoints',
                   'points': [[float(f), float(v / G)] for f, v in zip(frequency, level,
                                                                       strict=True)]},
        'bands': bands, 'alarm_db': None, 'abort_db': None, 'channel': channel,
        'limits': [], 'notes': notes}
    for kind, wire in (('warning', 'alarm_db'), ('abort', 'abort_db')):
        if not _has_limit(spec, kind, record):
            continue
        out['limits'].append(kind)
        pairs = _db_pairs(level, np.real(spec.limits[f'{kind}_lower'][record]),
                          np.real(spec.limits[f'{kind}_upper'][record]), 20.0)
        if not np.all(np.isfinite(pairs)) or np.any(pairs < 0):
            raise ConversionRefused(f'its {kind} limit is no band around the target')
        if not _uniform(pairs):
            raise ConversionRefused(f'its {kind} band varies from point to point, and the SRS '
                                    'module holds one band over the target')
        out[wire] = _wire_db(pairs[0])
    return out


# ---- Classical Shock: a pulse ----------------------------------------------------------------

def classical_pulse(pulse: Mapping[str, Any]) -> Any:
    """A Classical Shock pulse on the wire as a `ClassicalPulse`: the
    peak signed in SI, the duration in seconds, a trapezoid's edge from
    its fractions.

    Raises
    ------
    ConversionRefused
        For a shape the analysis has no pulse of (haversine, initial-peak
        sawtooth, rectangle) or a trapezoid whose rise and fall differ.
    """
    from ..core.classical_pulse import ClassicalPulse

    shape = PULSE_SHAPES.get(str(pulse.get('shape')))
    if shape is None:
        raise ConversionRefused(f'a {pulse.get("shape")} pulse has no analysis shape')
    sign = -1.0 if pulse.get('polarity') == 'negative' else 1.0
    duration = float(pulse['duration_ms']) * 1e-3
    edge = None
    if shape == 'trapezoid':
        rise, fall = float(pulse.get('rise_frac') or 0.0), float(pulse.get('fall_frac') or 0.0)
        if not np.isclose(rise, fall, rtol=TOLERANCE, atol=0.0):
            raise ConversionRefused('the trapezoid rises and falls at different rates, and the '
                                    'analysis pulse has one edge')
        edge = rise * duration
    elif shape == 'sawtooth':
        edge = 0.0
    return ClassicalPulse(shape, sign * float(pulse['amplitude_g']) * G, duration, edge)


def shock_pulse(pulse: Any) -> dict[str, Any]:
    """A `ClassicalPulse` as the Classical Shock pulse on the wire, the
    inverse of `classical_pulse`: `shape`, `amplitude_g`, `duration_ms`,
    `polarity`, and a trapezoid's `rise_frac` and `fall_frac`.

    Raises
    ------
    ConversionRefused
        For a sawtooth whose fall takes time: the module's terminal-peak
        sawtooth falls at once.
    """
    wire = {v: k for k, v in PULSE_SHAPES.items()}[pulse.shape]
    out: dict[str, Any] = {'shape': wire, 'amplitude_g': abs(pulse.peak) / G,
                           'duration_ms': pulse.duration * 1e3,
                           'polarity': 'negative' if pulse.peak < 0 else 'positive',
                           'rise_frac': None, 'fall_frac': None}
    if pulse.shape == 'trapezoid':
        share = float(pulse.edge) / pulse.duration
        out['rise_frac'] = out['fall_frac'] = share
    elif pulse.shape == 'sawtooth' and pulse.edge:
        raise ConversionRefused(f'its sawtooth falls over {pulse.edge * 1e3:g} ms, and the '
                                'module\'s terminal-peak sawtooth falls at once')
    return out


def pulse_waveform(pulse: Any, rate_hz: float, before: float = 0.5) -> tuple[np.ndarray,
                                                                            np.ndarray]:
    """(time, acceleration in SI) of `pulse` sampled at `rate_hz`, with
    `before` durations of quiet before it and after it."""
    quiet = before * pulse.duration
    count = round((pulse.extent + 2 * quiet) * rate_hz) + 1
    time = np.arange(count) / rate_hz - quiet
    return time, pulse.at(time)


def pulse_of_waveform(time: Any, values: Any) -> tuple[Any, float]:
    """The classical pulse a waveform is, and how far the waveform is
    from it as a share of its peak; the shape that fits best.

    Raises
    ------
    ConversionRefused
        When the waveform is not within `PULSE_TOLERANCE` of any of the
        three shapes.
    """
    from ..core.classical_pulse import SHAPES, measure_pulse, suggest_pulse

    time = np.asarray(time, dtype=float)
    values = np.real(np.asarray(values))
    best, best_off = None, np.inf
    for shape in SHAPES:
        try:
            pulse = suggest_pulse(time, values, shape)
            found = measure_pulse(time, values, shape, 1.0 if pulse.peak > 0 else -1.0)
        except ValueError:
            continue
        off = float(np.max(np.abs(values - pulse.at(time - found.start))) / abs(pulse.peak))
        if off < best_off:
            best, best_off = pulse, off
    if best is None or best_off > PULSE_TOLERANCE:
        raise ConversionRefused(
            'its waveform is no classical pulse: ' + (
                'none of the shapes reads from it' if best is None else
                f'the nearest, a {best.shape}, is {100 * best_off:.3g}% of its peak away '
                f'(within {100 * PULSE_TOLERANCE:g}% is a pulse)'))
    return best, best_off


# ---- TWR: a reference and a time history -----------------------------------------------------

def twr_reference(history: Any, record: int = 0, units: str = 'g') -> dict[str, Any]:
    """A record of a `TimeHistory` (or `TransientSpecification`) as a TWR
    reference the control service imports as pasted text: one sample a
    line, in `units` (the control channel's), at `fs_hz`.

    Returns
    -------
    dict
        `text`, `fs_hz`, `units`, `samples`, `channel`.

    Raises
    ------
    ConversionRefused
        For units not declared, of another quantity than `units`, an
        abscissa that is not evenly spaced, or a record too long to send
        as text.
    """
    from ..core.data import TimeHistory

    if not isinstance(history, TimeHistory):
        raise ConversionRefused(f'a {type(history).__name__} is not a time history')
    if not 0 <= record < history.num_records:
        raise ConversionRefused(f'it has no record {record}')
    dim = history.ordinate_dim[record]
    want = dimension_of(units)
    if dim == UNKNOWN:
        raise ConversionRefused(f'{history.response_dof[record]} has no declared units; '
                                'declare them first')
    if want is None or dim != want:
        raise ConversionRefused(f'{history.response_dof[record]} is of {dim}, and the control '
                                f'channel is in {units} ({want or "no quantity"})')
    time = np.asarray(history.abscissa, dtype=float)
    steps = np.diff(time)
    if len(time) < 2 or not _close(steps, steps[0], rtol=1e-6) or steps[0] <= 0:
        raise ConversionRefused('its samples are not evenly spaced in time')
    if len(time) > MOST_REFERENCE_SAMPLES:
        raise ConversionRefused(f'it holds {len(time)} samples; import a reference that long '
                                'from a file')
    values = np.real(np.asarray(history.ordinate[record])) / si_transform(units)[0]
    fs = 1.0 / float(np.mean(steps))
    return {'text': '\n'.join(f'{v:.12g}' for v in values) + '\n', 'fs_hz': fs,
            'units': units, 'samples': len(values),
            'channel': history.response_dof[record]}


def transient_from_reference(values: Any, fs_hz: float, units: str, channel: str,
                             comment: str = '') -> Any:
    """A TWR reference (samples in `units` at `fs_hz`) as a
    `TransientSpecification` at `channel`, in SI, time from its first
    sample: the inverse of `twr_reference`."""
    from ..core.data import TransientSpecification

    scale = si_transform(units)[0]
    values = np.asarray(values, dtype=float)
    return TransientSpecification(
        np.arange(len(values)) / float(fs_hz), values[np.newaxis] * scale,
        response_dof=[channel], ordinate_dim=[dimension_of(units) or UNKNOWN],
        ordinate_unit=[units], comment=comment or 'a TWR reference')


# ---- Mixed Mode: the parts and a mixed specification -----------------------------------------

def _sweep(start: float, end: float, rate: float, mode: int) -> dict[str, Any]:
    """A sweep schedule from `start` towards `end`: the inverse of the
    run converter's reading of one (`run_convert._mixed_span`)."""
    from ..core.sine import LOG

    return {'kind': 'sweep', 'band_hz': [min(start, end), max(start, end)],
            'rate': ({'law': 'log', 'oct_per_min': float(rate)} if mode == LOG
                     else {'law': 'linear', 'hz_per_s': float(rate)}),
            'start_hz': float(start), 'direction': 'up' if end > start else 'down'}


def mixed_parts(floor: Any, tones: Any = None, channel: str | None = None) -> dict[str, Any]:
    """The Mixed Mode parts a mixed specification asks for: the floor
    (`random_profile` of the floor), a narrowband part for each of the
    floor's narrowbands, and a tone part for each tone of `tones` (a
    `SineSweepSpecification`, or None), with the width left out around
    each tone. The run converter reads a run's parts into the same
    objects (`run_convert._mixed_parts`); this is that reading the other
    way.

    Returns
    -------
    dict
        `parts`, `exclusion_hz` (or None), `floor` (the floor's
        `random_profile`), `tone_limits` ({part id: {'alarm_db', 'abort_db'}}
        for the tones whose specification gives them), `notes`.

    Raises
    ------
    ConversionRefused
        For a floor `random_profile` refuses, more parts than the module
        plays, a tone whose amplitude changes along it, or one of
        several segments.
    """
    from ..core.mixed_mode import Tone
    from ..core.sine import SineSweepSpecification

    profile = random_profile(floor, channel, floor=True)
    notes = list(profile['notes'])
    part: dict[str, Any] = {'kind': 'floor', 'id': 'floor', 'name': 'Floor',
                            'breakpoints': profile['breakpoints']}
    for wire in ('alarm_db', 'abort_db'):
        if profile[wire] is not None:
            part[wire] = profile[wire]
    parts = [part]
    bands = list(getattr(floor, 'narrowbands', ()) or ())
    if len(bands) > 12:
        raise ConversionRefused(f'it has {len(bands)} narrowbands, and Mixed Mode plays 12')
    for k, band in enumerate(bands, 1):
        schedule = ({'kind': 'fixed', 'freq_hz': float(band.center)} if band.sweep_to is None
                    else _sweep(band.center, band.sweep_to, band.rate, band.mode))
        out: dict[str, Any] = {'kind': 'narrowband', 'id': f'band{k}',
                               'name': band.name or f'Narrowband {k}', 'schedule': schedule,
                               'level_g2_hz': float(band.level) / G ** 2}
        if band.width is not None:
            out['width_hz'] = float(band.width)
        else:
            out['width_share'] = float(band.share)
        if band.tolerance_db is not None:
            out['alarm_db'] = [float(band.tolerance_db), float(band.tolerance_db)]
        parts.append(out)
    tone_limits: dict[str, dict[str, Any]] = {}
    if tones is not None:
        if not isinstance(tones, SineSweepSpecification):
            raise ConversionRefused(f'a {type(tones).__name__} is not a sine specification')
        if tones.ordinate_dim != 'acceleration':
            raise ConversionRefused(f'its tones are in {tones.ordinate_dim}, and a Mixed Mode '
                                    'tone is of acceleration (g)')
        if len(tones.tones) > 12:
            raise ConversionRefused(f'it has {len(tones.tones)} tones, and Mixed Mode plays 12')
        for k, tone in enumerate(tones.tones, 1):
            amplitude = tone.amplitude[:, 0]
            if not _close(amplitude, amplitude[0]):
                raise ConversionRefused(f'{tone.name}\'s amplitude changes along it, and a '
                                        'Mixed Mode tone holds one amplitude')
            row = Tone.of(tone)
            if not row.fixed and row.rate is None:
                raise ConversionRefused(f'{tone.name} sweeps in several segments, and a Mixed '
                                        'Mode tone sweeps in one')
            ident = f'tone{k}'
            schedule = ({'kind': 'fixed', 'freq_hz': float(row.frequency),
                         'end': {'seconds': float(f'{tone.duration():.9g}')}} if row.fixed
                        else {**_sweep(row.frequency, row.sweep_to, row.rate, row.mode),
                              'end': {'sweeps': 1}})
            parts.append({'kind': 'tone', 'id': ident, 'name': tone.name or f'Tone {k}',
                          'schedule': schedule, 'amplitude_g': float(row.amplitude) / G})
            found: dict[str, Any] = {}
            for kind, wire in (('warning', 'alarm_db'), ('abort', 'abort_db')):
                lower, upper = tone.limits.get(f'{kind}_lower'), tone.limits.get(f'{kind}_upper')
                if lower is None or upper is None:
                    continue
                pairs = _db_pairs(amplitude, lower[:, 0], upper[:, 0], 20.0)
                if not _uniform(pairs) or np.any(pairs < 0):
                    raise ConversionRefused(f'{tone.name}\'s {kind} band varies, and a Mixed '
                                            'Mode tone holds one band')
                found[wire] = _wire_db(pairs[0])
            if found:
                tone_limits[ident] = found
    exclusion = getattr(floor, 'tone_exclusion', None)
    return {'parts': parts, 'exclusion_hz': None if exclusion is None else float(exclusion),
            'floor': profile, 'tone_limits': tone_limits, 'notes': notes}


# ---- limits and a ceiling --------------------------------------------------------------------

def limit_profile(spec: Any, channel: int, record: int = 0) -> dict[str, Any]:
    """A Random limit channel's ceiling (`limits[]` on the wire, a
    response density in g²/Hz at each row) from a `Specification` record
    whose target is that ceiling. A ceiling is a protective number: a
    seed never fills it without the operator's typed word.

    Raises
    ------
    ConversionRefused
        For a record that is not an acceleration autospectrum.
    """
    from ..core.data import Specification, autospectrum

    if not isinstance(spec, Specification) or not 0 <= record < spec.num_records:
        raise ConversionRefused('a ceiling is read from a record of a random specification')
    if not autospectrum(spec, record) or spec.ordinate_dim[record] != _ACCEL_DENSITY:
        raise ConversionRefused(f'{spec.response_dof[record]} is not an acceleration '
                                'autospectrum, which a Random ceiling is')
    ceiling = np.real(np.asarray(spec.ordinate[record])) / G ** 2
    return {'channel': int(channel), 'kind': 'response',
            'rows': [{'freq_hz': float(f), 'ceiling': float(c)}
                     for f, c in zip(np.asarray(spec.abscissa, dtype=float), ceiling,
                                     strict=True)]}


def specification_from_limit(limit: Mapping[str, Any], dof: str) -> Any:
    """A Random limit channel's ceiling as a `Specification` at `dof`
    whose target is the ceiling, in SI: the inverse of `limit_profile`."""
    from ..core.data import Specification

    if limit.get('kind', 'response') != 'response':
        raise ConversionRefused('a force limit is no acceleration ceiling')
    rows = list(limit.get('rows') or [])
    frequency = np.array([float(r['freq_hz']) for r in rows])
    ceiling = np.array([float(r['ceiling']) for r in rows]) * G ** 2
    spec = Specification(abscissa=frequency, ordinate=ceiling[np.newaxis], response_dof=[dof],
                         ordinate_dim=[_ACCEL_DENSITY], ordinate_unit=['g**2/Hz'],
                         comment='a Random limit channel\'s ceiling')
    spec.interpolation = Specification.reading_of(frequency)
    return spec
