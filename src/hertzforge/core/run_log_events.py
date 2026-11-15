"""A recorded run's events, offered to the run log.

The control service logs every event of a run it records: its states,
levels, alarms, trips, pulses and plays, the operator's actions, how it
ended (`log.jsonl` in the run folder, kept in the project's
`RunRecord`). The run log (`core.run_log`) is the log keeper's own
record of the floor, in its own format (`.hflog`). Every one of the
service's events can be an entry of that log, of any kind the service
has, but none is written there by itself: an entry in the run log is
the log keeper's, so each event is **offered** (`offer`) as a proposed
entry, and the keeper accepts it (`accept`), which writes it at the
time the controller logged it, or leaves it, which leaves no trace.

A log already in the run log's own format beside the run (a `.hflog`
in the run folder) is not offered again: it is the log, and is read
directly (`read_beside`).

Each proposal says which event it came from: the run's identity and the
event's place in the service's log, written into the entry's text, so a
second acceptance of the same event is refused and a reader of the log
can trace the entry back to the run.
"""

from __future__ import annotations

import datetime as _dt
import os
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any

from .run_log import SUFFIX, Entry, RunLog, RunLogError

#: the run log's entry kind each service event kind is offered as; a
#: kind not here (the list is open: the service may add kinds) is
#: offered as a note
KIND_OF = {'level': 'level_row', 'pulse': 'shot', 'trip': 'abort'}


class RunEventsError(ValueError):
    """An event that cannot be offered or accepted; the message names
    it."""


@dataclass(frozen=True)
class Proposal:
    """A run's event proposed as a run log entry.

    `kind` and `fields` are the entry's, `event_time` when the
    controller logged it, with its zone (None when the record has no
    time for it; the keeper then gives one), `text` the entry's words,
    ending with `marker`, which names the run and the event. `source`
    is the service's own entry as recorded.
    """

    kind: str
    fields: Mapping[str, Any]
    text: str
    event_time: _dt.datetime | None
    marker: str
    source: Mapping[str, Any] = field(default_factory=dict)


def _zone(manifest: Mapping[str, Any]) -> _dt.timezone:
    start = manifest.get('start') if isinstance(manifest.get('start'), Mapping) else {}
    offset = start.get('utc_offset_s')
    seconds = float(offset) if isinstance(offset, (int, float)) else 0.0
    return _dt.timezone(_dt.timedelta(seconds=seconds))


def _number(value: Any) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    return float(value)


def _fields(kind: str, data: Mapping[str, Any]) -> dict[str, Any]:
    """The entry's fields from the event's facts, only those the event
    states: nothing is filled in."""
    out: dict[str, Any] = {}
    if kind == 'level_row':
        level = _number(data.get('level_db'))
        if level is None:
            level = _number(data.get('to_db'))
        if level is not None:
            out['level_db'] = level
    elif kind == 'shot':
        level = _number(data.get('level_db'))
        if level is not None:
            out['level_db'] = level
        pulse = data.get('pulse')
        if isinstance(pulse, int) and not isinstance(pulse, bool) and pulse >= 0:
            out['n'] = pulse + 1
        if isinstance(data.get('passed'), bool):
            out['in_tolerance'] = data['passed']
    elif kind == 'abort':
        # the controller stopped the run itself: its own controlled stop,
        # which is what this kind of stop records
        out['stop'] = 'controller ramp'
        level = _number(data.get('level_db'))
        if level is not None:
            out['level_db'] = level
    return out


def offer(record: Any) -> list[Proposal]:
    """Every event of a recorded run as a proposed run log entry, in the
    order the service logged them, and how the run ended as a proposed
    'run stopped' entry. Nothing is written.

    Parameters
    ----------
    record : RunRecord
        The run's record.

    Returns
    -------
    list of Proposal
    """
    manifest = record.manifest
    zone = _zone(manifest)
    run = record.run_uid or record.run_id or 'unnamed run'
    test = record.test or 'unknown'
    source = record.source or 'source not stated'
    proposals = []
    for entry in record.log:
        service_kind = str(entry.get('kind', '') or '')
        kind = KIND_OF.get(service_kind, 'note')
        data = entry.get('data') if isinstance(entry.get('data'), Mapping) else {}
        at = _number(entry.get('at_unix'))
        when = None if at is None else _dt.datetime.fromtimestamp(at, zone)
        seq = entry.get('seq')
        marker = f'[{test} run {run}, {source}, controller log entry {seq}, {service_kind}]'
        words = str(entry.get('text', '') or '').strip()
        proposals.append(Proposal(kind, _fields(kind, data), f'{words} {marker}'.strip(),
                                  when, marker, dict(entry)))
    if record.end is not None:
        end = record.end.get('end') if isinstance(record.end.get('end'), Mapping) else {}
        at = _number(record.end.get('at_unix'))
        when = None if at is None else _dt.datetime.fromtimestamp(at, zone)
        outcome = str(end.get('outcome', '') or '')
        fields = {'result': outcome} if outcome else {}
        if outcome:
            fields['scheduled'] = outcome == 'completed'
        marker = f'[{test} run {run}, {source}, end]'
        message = str(end.get('message', '') or '').strip()
        proposals.append(Proposal('run_stop', fields, f'{message} {marker}'.strip(),
                                  when, marker, dict(record.end)))
    return proposals


def accept(log: RunLog, proposal: Proposal, *, author: str,
           test: int | None = None, axis: str | None = None, run: int | None = None,
           event_time: _dt.datetime | None = None) -> Entry:
    """Write a proposed event into the log, at the time the controller
    logged it, as the keeper confirms it.

    Parameters
    ----------
    log : RunLog
        The log, open for writing.
    proposal : Proposal
        The event.
    author : str
        The log keeper's initials.
    test, run : int, optional
        The test and run it belongs to, by number.
    axis : str, optional
        The axis it belongs to.
    event_time : datetime, optional
        When it happened, for an event the record has no time for, or
        to correct it; the controller's time otherwise.

    Returns
    -------
    Entry
        The entry written.

    Raises
    ------
    RunEventsError
        When the log already holds this event (its marker), or the event
        has no time and none is given.
    RunLogError
        For anything the log refuses (`RunLog.append`).
    """
    for entry in log.entries:
        if proposal.marker in entry.text:
            raise RunEventsError(f'the log holds this event already, entry #{entry.id}')
    when = event_time or proposal.event_time
    if when is None:
        raise RunEventsError('the run record has no time for this event; give the time '
                             'it happened')
    return log.append(proposal.kind, author=author, fields=dict(proposal.fields),
                      text=proposal.text, event_time=when, test=test, axis=axis, run=run)


def read_beside(folder: str | os.PathLike) -> list[RunLog]:
    """The run logs already in the run log's own format in a run folder
    (`.hflog` files), read as they are; empty when there is none, and the
    run's events are then offered (`offer`)."""
    found = []
    for name in sorted(os.listdir(folder)):
        if name.endswith(SUFFIX):
            found.append(RunLog.read(os.path.join(folder, name)))
    return found


def offered_or_read(record: Any, folder: str | os.PathLike | None = None
                    ) -> tuple[str, Sequence[Any]]:
    """How a run's events reach the run log: ('read', logs) when its
    folder holds a log in the run log's own format, else ('offered',
    proposals) for the keeper to confirm. Never written silently."""
    if folder is not None and os.path.isdir(folder):
        logs = read_beside(folder)
        if logs:
            return 'read', logs
    return 'offered', offer(record)


__all__ = ['KIND_OF', 'Proposal', 'RunEventsError', 'RunLogError', 'accept', 'offer',
           'offered_or_read', 'read_beside']
