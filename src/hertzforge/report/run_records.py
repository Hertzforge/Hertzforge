"""What a report reads from a run record.

A run the control service recorded arrives in a project with its
`RunRecord` (`core.run_record`), linked to the run's objects through one
object group (`io.run_convert`). The record holds what a report would
otherwise have to infer from file names and signal levels: who ran the
test, on which article, to which document and clause; every level
change, hold, alarm, trip and operator action the controller logged,
with its time; how the run ended and in which words; and the counts the
test keeps (pulses, plays, control loops, recordings).

`facts` reads those out of one record as `RunFacts`, in words a report
can print. Nothing is invented: a fact the record does not hold is said
to be not in the run record, never filled in from elsewhere. A
simulated run is labeled SIM. `record_of` finds the record a time
history belongs to, and `field_value` answers the live references a
report's text makes to a record (`{{@basis:RunRecord.operator}}`), so a
template's words fill themselves in from whichever run the project
holds.

The module-specific readings (`condition`, `drive_prediction`, the
Mixed Mode parts' levels, the Explorer survey, the Recorder's
recordings) read the record's own arrays and descriptions, each with
the unit the folder wrote, and say plainly when the record has none.
"""

from __future__ import annotations

import datetime as _dt
import math
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any

import numpy as np

#: what a fact the record does not hold reads as
NOT_RECORDED = 'not in the run record'

#: each test by the name a report gives it
TEST_NAMES = {'random': 'Random', 'mimo_random': 'MIMO Random', 'sine': 'Sine',
              'mixed': 'Mixed Mode', 'classical_shock': 'Classical Shock',
              'srs': 'SRS Shock', 'twr': 'Time Waveform Replication',
              'mimo_shock': 'MIMO Shock', 'explorer': 'Explorer',
              'recorder': 'Recorder', 'playback': 'Playback',
              'modal_test': 'Modal Test'}

#: how a run ended, in words; an outcome not here is printed as written
OUTCOMES = {'completed': 'completed', 'aborted': 'aborted',
            'operator_stop': 'stopped by the operator',
            'law_fault': 'stopped: the control law faulted',
            'held': 'held', 'stopped': 'stopped'}

#: the end's summary counts a report prints, in the order it prints them
SUMMARY_COUNTS = (('loops', 'control loop', 'control loops'),
                  ('spectra_kept', 'spectrum kept', 'spectra kept'),
                  ('law_calls', 'law call', 'law calls'),
                  ('pulses_fired', 'pulse fired', 'pulses fired'),
                  ('pulses_passed', 'pulse passed', 'pulses passed'),
                  ('plays', 'play', 'plays'),
                  ('playback_plays', 'playback play', 'playback plays'))

#: log kinds counted from the log, by the words a count takes
LOG_COUNTS = (('pulse', 'pulse logged', 'pulses logged'),
              ('play', 'play logged', 'plays logged'),
              ('intent', 'steering action', 'steering actions'),
              ('recording', 'recording entry', 'recording entries'))

#: the states a run is held in, as the services name them
HOLD_STATES = frozenset({'hold', 'held', 'paused', 'review', 'awaiting_review'})

#: log kinds that are a hold or a resume of their own
HOLD_KINDS = frozenset({'hold', 'held'})
RESUME_KINDS = frozenset({'resume', 'resumed'})


@dataclass(frozen=True)
class Event:
    """One entry of the run's log as a report reads it: when (seconds
    from the run's start, None when the record has no time for it), its
    kind and its words."""

    at_s: float | None
    kind: str
    text: str
    data: Mapping[str, Any] = field(default_factory=dict)

    def when(self) -> str:
        """'at 15.0 s', or that the record holds no time for the event."""
        return 'at a time not recorded' if self.at_s is None else f'at {self.at_s:.1f} s'


@dataclass(frozen=True)
class RunFacts:
    """What one run record says of its run, in a report's words.

    `levels` are the level changes as (seconds from the start, level in
    dB), `level_db` the level the run ended at (the end's own, else the
    last level change), None when the record holds none. `holds`,
    `resumes`, `alarms`, `trips`, `overloads` and `operator` are the
    log's entries of each sort. `counts` are (count, words) in the order
    printed. `outcome` is the end's code ('' for a run that did not end
    cleanly) and `outcome_text` it in words with the end's message.
    """

    test: str
    run_id: str
    title: str
    source: str
    operator: str
    article: str
    specification: str
    notes: str
    start: str
    complete: bool
    outcome: str
    outcome_text: str
    level_db: float | None
    levels: tuple[tuple[float | None, float], ...] = ()
    at_full_level_s: float | None = None
    holds: tuple[Event, ...] = ()
    resumes: tuple[Event, ...] = ()
    alarms: tuple[Event, ...] = ()
    trips: tuple[Event, ...] = ()
    overloads: tuple[Event, ...] = ()
    operator_actions: tuple[Event, ...] = ()
    counts: tuple[tuple[int, str], ...] = ()
    events: tuple[Event, ...] = ()
    missing: tuple[str, ...] = ()

    @property
    def test_name(self) -> str:
        """The test in words, or the record's own word for one with no
        name here."""
        return TEST_NAMES.get(self.test, self.test or NOT_RECORDED)

    def levels_text(self) -> str:
        """'-3 dB at 15.0 s, 0 dB at 17.0 s', or that the record holds
        no level change."""
        if not self.levels:
            return 'no level change in the run record'
        return ', '.join(f'{_db(level)} ' + ('at a time not recorded' if at is None
                                              else f'at {at:.1f} s')
                         for at, level in self.levels)

    def interruptions_text(self) -> str:
        """The holds, resumes and trips, in order, with their times."""
        found = sorted((*self.holds, *self.resumes, *self.trips),
                       key=lambda e: (e.at_s is None, e.at_s or 0.0))
        if not found:
            return 'none in the run record'
        return '; '.join(f'{_sort(e)} {e.when()}: {e.text}' for e in found)

    def counts_text(self) -> str:
        """'3 pulses, 2 levels', or that the record holds no counts."""
        if not self.counts:
            return 'no counts in the run record'
        return ', '.join(f'{n} {words}' for n, words in self.counts)

    def summary_line(self, name: str, together: Sequence[str] = ()) -> str:
        """One line under the chronology for the run of record `name`:
        its test and source, who, what and to which document, its levels,
        alarms and counts."""
        with_ = f' (with {", ".join(together)})' if together else ''
        alarms = ('; '.join(f'{e.when()}: {e.text}' for e in self.alarms)
                  if self.alarms else 'none')
        overloads = ('; '.join(f'{e.when()}: {e.text}' for e in self.overloads)
                     if self.overloads else 'none')
        return (f'{name}{with_}, from its run record: {self.test_name} run '
                f'{self.run_id or NOT_RECORDED}, {self.source}; operator '
                f'{self.operator}; article {self.article}; specification '
                f'{self.specification}; levels {self.levels_text()}; alarms '
                f'{alarms}; overloads {overloads}; {self.counts_text()}; ended '
                f'{self.outcome_text.rstrip(".")}.')


def _db(level: float) -> str:
    level = float(level) + 0.0
    return f'{level:+g} dB' if level else '0 dB'


def _sort(event: Event) -> str:
    if event.kind in RESUME_KINDS or event.kind == 'resume':
        return 'resumed'
    if event.kind == 'trip':
        return 'trip'
    return 'hold'


def _number(value: Any) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    value = float(value)
    return value if math.isfinite(value) else None


def _text(value: Any) -> str:
    text = str(value or '').strip()
    return text or NOT_RECORDED


def source_words(source: str | None) -> str:
    """'SIM (simulator)', 'HARDWARE', or that the record does not say:
    a label is never guessed."""
    if source == 'SIM':
        return 'SIM (simulator)'
    if source == 'HARDWARE':
        return 'HARDWARE'
    return 'source not stated in the run record'


def start_text(manifest: Mapping[str, Any]) -> str:
    """The run's start with its UTC offset, ISO 8601, or ''."""
    start = manifest.get('start') or {}
    unix = _number(start.get('unix')) if isinstance(start, Mapping) else None
    if unix is None:
        return ''
    offset = _number(start.get('utc_offset_s')) or 0.0
    zone = _dt.timezone(_dt.timedelta(seconds=offset))
    return _dt.datetime.fromtimestamp(unix, zone).isoformat()


def _specification(manifest: Mapping[str, Any]) -> str:
    spec = manifest.get('specification')
    if not isinstance(spec, Mapping):
        return NOT_RECORDED
    document = str(spec.get('document') or '').strip()
    clause = str(spec.get('clause') or '').strip()
    if not document and not clause:
        return NOT_RECORDED
    return ', '.join(part for part in (document or 'document not recorded',
                                       f'clause {clause}' if clause else '') if part)


def _end(record: Any) -> dict[str, Any]:
    end = (record.end or {}).get('end') if record.end else None
    return dict(end) if isinstance(end, Mapping) else {}


def _plural(n: int, one: str, many: str) -> tuple[int, str]:
    return n, one if n == 1 else many


def facts(record: Any) -> RunFacts:
    """What `record` (a `RunRecord`) says of its run.

    Parameters
    ----------
    record : RunRecord
        The run's record.

    Returns
    -------
    RunFacts
    """
    manifest = record.manifest
    start = manifest.get('start') if isinstance(manifest.get('start'), Mapping) else {}
    origin = _number(start.get('unix'))
    events: list[Event] = []
    for entry in record.log:
        at = _number(entry.get('at_unix'))
        data = entry.get('data') if isinstance(entry.get('data'), Mapping) else {}
        events.append(Event(None if at is None or origin is None else at - origin,
                            str(entry.get('kind', '') or ''),
                            str(entry.get('text', '') or ''), dict(data)))
    levels: list[tuple[float | None, float]] = []
    holds: list[Event] = []
    resumes: list[Event] = []
    full_from: float | None = None
    full_s: float | None = None
    in_hold = False
    for event in events:
        if event.kind == 'level':
            level = _number(event.data.get('level_db'))
            if level is None:
                level = _number(event.data.get('to_db'))
            if level is not None:
                levels.append((event.at_s, level))
        elif event.kind in HOLD_KINDS:
            holds.append(event)
        elif event.kind in RESUME_KINDS:
            resumes.append(event)
        elif event.kind == 'state':
            to = str(event.data.get('to', '') or '')
            was = str(event.data.get('from', '') or '')
            if to in HOLD_STATES and not in_hold:
                holds.append(event)
                in_hold = True
            elif was in HOLD_STATES and in_hold:
                resumes.append(Event(event.at_s, 'resume', event.text, event.data))
                in_hold = False
            if to == 'full_level' and full_from is None:
                full_from = event.at_s
            elif was == 'full_level' and full_from is not None and full_s is None \
                    and event.at_s is not None:
                full_s = event.at_s - full_from
    end = _end(record)
    trips = [e for e in events if e.kind == 'trip']
    trip = end.get('trip')
    if isinstance(trip, Mapping) and not trips:
        words = str(trip.get('message') or trip.get('kind') or 'a trip')
        trips.append(Event(_number(end.get('at_s')), 'trip', words, dict(trip)))
    level_db = _number(end.get('level_db'))
    if level_db is None and levels:
        level_db = levels[-1][1]
    outcome = record.outcome
    if outcome:
        words = OUTCOMES.get(outcome, outcome.replace('_', ' '))
        message = str(end.get('message') or '').strip()
        outcome_text = f'{words}: {message}' if message else words
    else:
        outcome_text = 'did not end cleanly: the run record holds no end'
    counts: list[tuple[int, str]] = []
    summary = (record.end or {}).get('summary') if record.end else None
    if isinstance(summary, Mapping):
        for key, one, many in SUMMARY_COUNTS:
            value = summary.get(key)
            if isinstance(value, int) and not isinstance(value, bool):
                counts.append(_plural(value, one, many))
    recordings = [name for name, stream in record.streams.items()
                  if stream.get('kind') == 'recording']
    if recordings or record.test == 'recorder':
        counts.append(_plural(len(recordings), 'recording', 'recordings'))
    passes = _sweep_passes(record)
    if passes is not None:
        counts.append(_plural(passes, 'sweep pass', 'sweep passes'))
    for kind, one, many in LOG_COUNTS:
        n = sum(1 for e in events if e.kind == kind)
        if n:
            counts.append(_plural(n, one, many))
    return RunFacts(
        test=record.test, run_id=record.run_id, title=record.title,
        source=source_words(record.source),
        operator=_text(manifest.get('operator')),
        article=_text(manifest.get('article')),
        specification=_specification(manifest),
        notes=_text(manifest.get('notes')), start=start_text(manifest),
        complete=record.complete, outcome=outcome, outcome_text=outcome_text,
        level_db=level_db, levels=tuple(levels), at_full_level_s=full_s,
        holds=tuple(holds), resumes=tuple(resumes),
        alarms=tuple(e for e in events if e.kind == 'alarm'), trips=tuple(trips),
        overloads=tuple(e for e in events if e.kind == 'overload'),
        operator_actions=tuple(e for e in events if e.kind == 'operator'
                               or 'by' in e.data and e.data.get('by')),
        counts=tuple(counts), events=tuple(events), missing=tuple(record.missing))


def _sweep_passes(record: Any) -> int | None:
    """The one-way sweeps a Sine run's levels hold (the distinct `pass`
    values its records filled), None when the record has no levels."""
    held = record.arrays.get('channel_levels', {})
    passes = held.get('pass')
    if passes is None:
        return None
    values = np.asarray(passes, dtype=float).ravel()
    values = values[np.isfinite(values)]
    return len(np.unique(values))


# ---- finding a history's record ------------------------------------------

def records(objects: Mapping[str, Any]) -> list[str]:
    """The run records in the project, by name, in its order."""
    from ..core.run_record import RunRecord

    return [name for name, obj in objects.items() if isinstance(obj, RunRecord)]


def record_of(objects: Mapping[str, Any], name: str,
              object_groups: Sequence[Mapping[str, Any]] | None = None) -> Any:
    """The run record the object `name` belongs to: the one run record in
    an object group that holds `name`. None when no group links it to
    exactly one, so data with no run record is read as before."""
    from ..core.run_record import RunRecord

    if object_groups is None:
        object_groups = getattr(objects, 'object_groups', None) or []
    for group in object_groups:
        members = list(group.get('members', ()))
        if name not in members:
            continue
        found = [objects[m] for m in members
                 if m in objects and isinstance(objects[m], RunRecord)]
        if len(found) == 1:
            return found[0]
    return None


# ---- the live references a report's text makes to a record ---------------

def _array(record: Any, result: str, name: str) -> np.ndarray | None:
    held = record.arrays.get(result, {})
    return None if name not in held else np.asarray(held[name])


def _unit(record: Any, result: str, name: str) -> str:
    for array in record.results.get(result, {}).get('arrays', ()) or ():
        if array.get('name') == name:
            return str(array.get('unit', '') or '')
    return ''


def _lines(record: Any, result: str, count: int) -> np.ndarray | None:
    """The frequencies of a result's lines from its abscissa's start and
    step, None when its description gives neither."""
    abscissa = record.results.get(result, {}).get('abscissa') or {}
    start, step = _number(abscissa.get('start')), _number(abscissa.get('step'))
    if start is None or step is None:
        return None
    return start + step * np.arange(count)


def _with_unit(value: float, unit: str) -> str:
    return f'{value:.4g} {unit}'.rstrip() if unit else f'{value:.4g}'


def condition_text(record: Any) -> str:
    """The identification's condition number (dimensionless): its
    largest over the band and the line it falls on, its smallest, and
    the gate's limit and verdict as the end's report gives them."""
    condition = _array(record, 'identification', 'condition')
    if condition is None or condition.size == 0:
        return f'the condition number is {NOT_RECORDED}'
    values = condition.astype(float).ravel()
    finite = np.isfinite(values)
    if not finite.any():
        return 'the condition number recorded holds no finite value'
    worst = int(np.nanargmax(np.where(finite, values, -np.inf)))
    lines = _lines(record, 'identification', values.size)
    at = '' if lines is None else f' at {lines[worst]:g} Hz'
    text = (f'the condition number (dimensionless) of the identified transfer '
            f'function over {values.size} line{"s" * (values.size != 1)} runs from '
            f'{np.nanmin(values[finite]):.4g} to {values[worst]:.4g}{at}')
    gates = ((record.end or {}).get('report') or {}).get('gates') if record.end else None
    if isinstance(gates, Mapping) and _number(gates.get('condition_limit')) is not None:
        verdict = gates.get('passed')
        text += (f'; the gate allowed at most {gates["condition_limit"]:g}, and '
                 + ('passed' if verdict is True else 'failed' if verdict is False
                    else 'its verdict is not recorded'))
    return text


def _drive_labels(record: Any, count: int) -> list[str]:
    drives = (record.resolved.get('channels') or {}).get('drives') or []
    labels = [str(d.get('label') or f'drive {d.get("channel")}') for d in drives
              if isinstance(d, Mapping)]
    return labels if len(labels) == count else [f'drive {k + 1}' for k in range(count)]


def drive_prediction_text(record: Any) -> str:
    """The drives the control law's seed asked for before the loop
    engaged, the drive prediction: each drive's rms from its spectral
    density on the band's lines (V rms from V^2/Hz), against its ceiling
    where the setup gives one."""
    drive = _array(record, 'law_calls', 'drive')
    calls = record.results.get('law_calls', {}).get('records') or []
    seeds = [k for k, line in enumerate(calls)
             if isinstance(line, Mapping) and line.get('call') == 'seed']
    abscissa = record.results.get('law_calls', {}).get('abscissa') or {}
    step = _number(abscissa.get('step'))
    if drive is None or not seeds or drive.ndim != 3 or step is None:
        return f'the drive prediction is {NOT_RECORDED}'
    unit = _unit(record, 'law_calls', 'drive')
    if unit != 'V^2/Hz':
        return (f'the drive prediction is recorded in {unit or "no unit"}, not V^2/Hz, so '
                'its rms is not read')
    # each line's matrix is packed by its upper triangle, row by row,
    # down the first axis; the lines run along the second
    seed = drive[seeds[0]]
    size = seed.shape[0]
    count = round((math.sqrt(8 * size + 1) - 1) / 2)
    if count * (count + 1) // 2 != size:
        return ('the drive prediction is packed in a form this report does not read, '
                'so its rms is not given')
    rows, cols = np.triu_indices(count)
    diagonal = np.real(seed[rows == cols])
    rms = np.sqrt(np.sum(diagonal, axis=1) * step)
    drives = (record.resolved.get('channels') or {}).get('drives') or []
    ceilings = [_number(d.get('ceiling_vrms')) for d in drives if isinstance(d, Mapping)]
    labels = _drive_labels(record, count)
    parts = []
    for k, value in enumerate(rms):
        part = f'{labels[k]} {value:.4g} V rms'
        if len(ceilings) == count and ceilings[k] is not None:
            part += f' (ceiling {ceilings[k]:g} V rms)'
        parts.append(part)
    return ('the control law\'s seed, before the loop engaged, asked for '
            + ', '.join(parts))


def drive_rms_text(record: Any) -> str:
    """Each drive's rms over the run as the end's report gives it, and
    the smallest cap the guards set on it."""
    report = (record.end or {}).get('report') if record.end else None
    values = report.get('drive_rms_v') if isinstance(report, Mapping) else None
    if not isinstance(values, list) or not values:
        return f'the drives\' rms is {NOT_RECORDED}'
    labels = _drive_labels(record, len(values))
    caps = report.get('min_cap') if isinstance(report.get('min_cap'), list) else []
    parts = []
    for k, value in enumerate(values):
        if _number(value) is None:
            parts.append(f'{labels[k]} not recorded')
            continue
        part = f'{labels[k]} {value:.4g} V rms'
        if len(caps) == len(values) and _number(caps[k]) is not None:
            part += f' (smallest cap {caps[k]:g})'
        parts.append(part)
    return 'over the run the drives played ' + ', '.join(parts)


def control_error_text(record: Any) -> str:
    """Each controlled quantity's rms against its reference and the
    error in dB, as the end's report gives them."""
    report = (record.end or {}).get('report') if record.end else None
    if not isinstance(report, Mapping):
        return f'the control error is {NOT_RECORDED}'
    measured, wanted = report.get('control_rms'), report.get('reference_rms')
    errors = report.get('control_error_db')
    if not all(isinstance(v, list) for v in (measured, wanted, errors)) \
            or not len(measured) == len(wanted) == len(errors):
        return f'the control error is {NOT_RECORDED}'
    unit = ''
    controls = (record.resolved.get('channels') or {}).get('controls') or []
    units = {str(c.get('unit') or '') for c in controls if isinstance(c, Mapping)}
    if len(units) == 1:
        unit = f' {units.pop()} rms'
    labels = [str(c.get('label') or f'control {k + 1}') for k, c in enumerate(controls)
              if isinstance(c, Mapping)]
    if len(labels) != len(errors):
        labels = [f'control {k + 1}' for k in range(len(errors))]
    return 'at the end ' + ', '.join(
        f'{labels[k]} {measured[k]:.4g}{unit} against its reference '
        f'{wanted[k]:.4g}{unit}, {errors[k]:+.2f} dB'
        for k in range(len(errors)))


def _last_at_level(level: np.ndarray | None, time: np.ndarray | None,
                   *arrays: np.ndarray | None) -> tuple[float, float, tuple[float, ...]] | None:
    """The last point at the highest level the series reached where every
    array holds a finite value: (time, level, values). None when there
    is no such point or an array is missing."""
    if level is None or time is None or any(a is None for a in arrays):
        return None
    flats = [np.asarray(a, dtype=float).ravel() for a in (level, time, *arrays)]
    if len({f.size for f in flats}) != 1:
        return None
    held = np.logical_and.reduce([np.isfinite(f) for f in flats])
    if not held.any():
        return None
    top = np.max(flats[0][held])
    at = np.flatnonzero(held & (flats[0] == top))
    k = int(at[-1])
    return float(flats[1][k]), float(flats[0][k]), tuple(float(f[k]) for f in flats[2:])


def mixed_parts_text(record: Any) -> str:
    """Each part of a Mixed Mode run at its last recorded point at the
    highest level the run reached: each tone's and narrowband's
    frequency, measured level and reference, and the controller's error,
    and the floor's, with their units and the time of the point."""
    parts = []
    for result, what in (('tone_levels', 'tone'), ('band_levels', 'narrowband'),
                         ('floor_levels', 'floor')):
        level = _array(record, result, 'level_db')
        time = _array(record, result, 'time')
        target = _array(record, result, 'target')
        if level is None or target is None:
            continue
        unit = _unit(record, result, 'target')
        names = ('measured', 'target', 'error_db')
        if target.ndim == 3:
            series = [(f'{what} {k + 1}',
                       [_array(record, result, n)[:, k] for n in (*names, 'frequency')])
                      for k in range(target.shape[1])]
        else:
            series = [(f'the {what}', [_array(record, result, n) for n in names])]
        for label, arrays in series:
            point = _last_at_level(level, time, *arrays)
            if point is None:
                parts.append(f'{label}: no point recorded')
                continue
            at_s, at_db, values = point
            m, t, e = values[:3]
            where = f' at {values[3]:g} Hz' if len(values) > 3 else ''
            rms = ' rms' if what == 'floor' else ''
            parts.append(f'{label}{where}: {_with_unit(m, unit)}{rms} measured, '
                         f'{_with_unit(t, unit)}{rms} its reference, error {e:+.2f} dB '
                         f'(at {at_s:g} s from engage, {_db(at_db)})')
    if not parts:
        return f'the parts\' levels are {NOT_RECORDED}'
    return ('at the last point recorded at the highest level reached, as the controller '
            'measured it: ' + '; '.join(parts))


def survey_text(record: Any) -> str:
    """An Explorer run's survey: each frequency the tone visited with
    the control row's amplitude there, peak, and the highest point."""
    frequency = _array(record, 'survey', 'frequency')
    level = _array(record, 'survey', 'level')
    if frequency is None or level is None or frequency.size == 0:
        return f'the survey is {NOT_RECORDED}'
    unit = _unit(record, 'survey', 'level')
    points = '; '.join(f'{f:g} Hz {_with_unit(v, unit)} peak'
                       for f, v in zip(frequency.ravel(), level.ravel(), strict=True))
    worst = int(np.nanargmax(level.ravel()))
    return (f'{frequency.size} point{"s" * (frequency.size != 1)} ({points}); the highest '
            f'{_with_unit(float(level.ravel()[worst]), unit)} peak at '
            f'{float(frequency.ravel()[worst]):g} Hz')


def measured_text(record: Any) -> str:
    """What an Explorer run measured at its end, as its end gives it:
    each acquired channel's rms, the measured frequency, the control
    row's motion and the survey's peak, with their units."""
    measured = (record.end or {}).get('measured') if record.end else None
    if not isinstance(measured, Mapping) or not measured:
        return f'what the run measured at its end is {NOT_RECORDED}'
    parts = []
    for channel in measured.get('channels') or ():
        if isinstance(channel, Mapping) and _number(channel.get('rms')) is not None:
            parts.append(f'channel {channel.get("index")} {channel["rms"]:.4g} '
                         f'{channel.get("unit", "")} rms'.rstrip())
    hz = _number(measured.get('measured_hz'))
    parts.append(f'measured frequency {hz:g} Hz' if hz is not None
                 else 'measured frequency not recorded')
    control = measured.get('control')
    if isinstance(control, Mapping):
        for key, words, unit in (('velocity_ips', 'velocity', 'in/s'),
                                 ('displacement_mil', 'displacement', 'mil')):
            value = _number(control.get(key))
            if value is not None:
                parts.append(f'control row {words} {value:.4g} {unit}')
    peak = measured.get('peak')
    if isinstance(peak, Mapping) and _number(peak.get('hz')) is not None:
        level = _number(peak.get('level'))
        parts.append(f'survey peak at {peak["hz"]:g} Hz'
                     + (f', {level:.4g}' if level is not None else ''))
    return 'at its end, as the run wrote it: ' + '; '.join(parts)


def recordings_text(record: Any) -> str:
    """A Recorder run's recordings: each one's name, trigger, length and
    pre-window, gaps and fault."""
    found = []
    for name, stream in record.streams.items():
        if stream.get('kind') != 'recording':
            continue
        rate = _number(stream.get('fs_hz'))
        frames = stream.get('frames')
        length = (f'{frames} frames' + (f', {frames / rate:g} s' if rate else '')
                  if isinstance(frames, int) else 'length not recorded')
        gaps = stream.get('gaps') or []
        fault = stream.get('fault')
        found.append(f'{name}: trigger {stream.get("trigger") or NOT_RECORDED}, {length}, '
                     f'{stream.get("pre_frames", 0)} frames before its trigger, '
                     f'{len(gaps)} gap{"s" * (len(gaps) != 1)}'
                     + (f', fault: {fault}' if fault else ''))
    if not found:
        return 'the run made no recording'
    return '; '.join(found)


def row_statistics_text(record: Any) -> str:
    """Each recorded row's statistics over a Recorder run, as its end
    gives them, in the row's unit."""
    rows = (record.end or {}).get('rows') if record.end else None
    if not isinstance(rows, list) or not rows:
        return f'the rows\' statistics are {NOT_RECORDED}'
    units = {int(r['index']): str(r.get('units', '') or '')
             for r in record.table.get('rows', ()) if isinstance(r, Mapping)
             and isinstance(r.get('index'), int)}
    parts = []
    for row in rows:
        if not isinstance(row, Mapping):
            continue
        unit = units.get(row.get('index'), '')
        parts.append(f'row {row.get("index")}: mean {row.get("mean", 0):g} {unit}, rms '
                     f'{row.get("rms", 0):g} {unit}, peak {row.get("peak", 0):g} {unit}, '
                     f'crest {row.get("crest", 0):g}, kurtosis {row.get("kurtosis", 0):g}'
                     .replace('  ', ' '))
    return '; '.join(parts)


def events_text(found: Sequence[Event]) -> str:
    """The events in order, each with its time, or 'none'."""
    if not found:
        return 'none'
    return '; '.join(f'{e.when()}: {e.text}' for e in found)


#: every live reference a report may make to a run record, by field
FIELDS = {
    'test': lambda r, f: f.test_name,
    'run_id': lambda r, f: f.run_id or NOT_RECORDED,
    'title': lambda r, f: f.title or NOT_RECORDED,
    'source': lambda r, f: f.source,
    'operator': lambda r, f: f.operator,
    'article': lambda r, f: f.article,
    'specification': lambda r, f: f.specification,
    'notes': lambda r, f: f.notes,
    'start': lambda r, f: f.start.replace('T', ' ') or NOT_RECORDED,
    'outcome': lambda r, f: f.outcome_text.rstrip('.'),
    'level': lambda r, f: NOT_RECORDED if f.level_db is None else _db(f.level_db),
    'levels': lambda r, f: f.levels_text(),
    'interruptions': lambda r, f: f.interruptions_text(),
    'alarms': lambda r, f: events_text(f.alarms),
    'trips': lambda r, f: events_text(f.trips),
    'overloads': lambda r, f: events_text(f.overloads),
    'operator_actions': lambda r, f: events_text(f.operator_actions),
    'counts': lambda r, f: f.counts_text(),
    'missing': lambda r, f: '; '.join(f.missing) or 'nothing',
    'condition': lambda r, f: condition_text(r),
    'drive_prediction': lambda r, f: drive_prediction_text(r),
    'drive_rms': lambda r, f: drive_rms_text(r),
    'control_error': lambda r, f: control_error_text(r),
    'parts': lambda r, f: mixed_parts_text(r),
    'survey': lambda r, f: survey_text(r),
    'measured': lambda r, f: measured_text(r),
    'steering': lambda r, f: events_text([e for e in f.events if e.kind == 'intent']),
    'recordings': lambda r, f: recordings_text(r),
    'row_statistics': lambda r, f: row_statistics_text(r),
    'pulses': lambda r, f: events_text([e for e in f.events if e.kind == 'pulse']),
    'plays': lambda r, f: events_text([e for e in f.events if e.kind == 'play']),
}


def field_value(record: Any, name: str) -> str | None:
    """The live reference `name` of a run record, in words; None for a
    field a run record does not answer, which the page leaves as
    written."""
    reader = FIELDS.get(name)
    return None if reader is None else reader(record, facts(record))
