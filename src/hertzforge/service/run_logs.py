"""The Run Log window's service side: Visual Dynamics' run-log dock
(`gui/run_log_dock.py`, `gui/run_log_forms.py`) as `runlog.*` requests.

The log keeper writes the run log while a test runs: the level rows of the
step-up, the abort and what the first minute owes, the small acts (a note, a
clock check, a person in or out, a run started or stopped, a cause found), the
test opened, rolled, closed and released with its header, the readings each
kind of test takes, and a correction of any entry. Each connection holds one
log open (`Keeper`), its lock released when the log is closed or the
connection ends.

The shell shows what `snapshot` returns and draws each form from `form_spec`;
it sends what was typed in each box as text, and the box kinds read it here
(`Reader`), so a box is read one way whichever front end typed it. What a form
computes from what is typed (the projection to full level, a sweep's current
against its rating, a shot's checks and pulses, a notch's place in the plan, a
deviation's hold) comes from `preview`, through the same functions the log
judges by. Nothing here drives the controller: every check is a reminder.

Beside what is read, the window shows the predictions drawn for the project
open on the connection, as Visual Dynamics' dock shows the ones its window drew:
the notch Notch Prediction cut (`core.limit_prediction`) beside a notch entry,
and the drives Drive Prediction asked for (`core.drive_prediction`) beside the
level row's drive. Each tool hands its prediction over as it draws it (`offer`);
with none drawn, which is always so for a log kept with no project, the place
it would be shown says so.
"""

from __future__ import annotations

import contextlib
import os
import weakref
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, replace
from datetime import datetime, time
from typing import Any

from hertzforge.core import run_log_model as model_module
from hertzforge.core.run_log import (
    ANSWERS,
    DIRECTIONS,
    FIELD_TYPES,
    HEADER_FIELDS,
    KINDS,
    SETTING_CLASSES,
    STOPS,
    Entry,
    FileRef,
    RunLog,
    RunLogError,
    band_hz,
    check_fields,
    file_ref,
)
from hertzforge.core.run_log_export import _correction_text, _fields_text, _where
from hertzforge.core.run_log_model import (
    LEVEL_MATCH_DB,
    RUN_SHAPES,
    SHOT_CHECKS,
    TEST_KINDS,
    RunLogModel,
    judge_pulses,
    notch_in_plan,
    project,
)

from .protocol import bad_request, refused

RUN_LOG_METHODS = ('runlog.new', 'runlog.open', 'runlog.close', 'runlog.reload',
                   'runlog.state', 'runlog.form', 'runlog.preview', 'runlog.write',
                   'runlog.export')

#: what a prediction field reads when none is drawn
NO_PREDICTION = 'no prediction in a project'

#: the header's fields in the groups the header form shows them in
HEADER_GROUPS: tuple[tuple[str, tuple[tuple[str, str, str], ...]], ...] = (
    ('Identity', (
        ('test_kind', 'Kind', 'choice'), ('title', 'Title', 'text'),
        ('test_class', 'Class', 'text'), ('purpose', 'Purpose', 'text'),
        ('article', 'Article', 'text'), ('serial', 'Serial', 'text'),
        ('configuration', 'Configuration', 'text'))),
    ('Governing documents', (
        ('document', 'Governing document', 'text'),
        ('document_revision', 'Its revision', 'text'),
        ('plan_revision', 'Plan revision', 'text'), ('procedure', 'Procedure', 'text'),
        ('procedure_revision', 'Procedure revision', 'text'),
        ('profile_source', 'Profile source', 'text'))),
    ('Step schedule and hold points', (
        ('steps_db', 'Step schedule (dB)', 'numbers'),
        ('hold_points_db', 'Hold points (dB)', 'numbers'))),
    ('Planned notches', (
        ('notches_planned', 'Planned notches (axis: band, depth)', 'notches'),)),
    ('Limits and tolerances', (
        ('limits', 'Limits', 'text'), ('tolerance', 'Tolerance', 'text'),
        ('abort_bands', 'Abort bands', 'text'))),
    ('Duration', (
        ('duration_s', 'Duration (s)', 'number'),
        ('duration_minus_pct', 'Tolerance under (%)', 'number'),
        ('duration_plus_pct', 'Tolerance over (%)', 'number'))),
    ('Amplifier', (
        ('sigma', 'Sigma', 'number'),
        ('rated_current_peak', 'Peak current rating (A)', 'number'),
        ('rated_current_rms', 'Continuous current rating (A)', 'number'),
        ('rail_voltage', 'Rail (V)', 'number'),
        ('drive_window', 'Drive window at full level (low, high)', 'numbers'))),
    ('Projected headroom', (
        ('projected_current', 'Projected peak current (A)', 'number'),
        ('projected_voltage', 'Projected peak voltage (V)', 'number'))),
    ('Shock', (('pulse_budget', 'Pulse budget', 'whole'),)),
)

#: the readings forms by name, with the kind of entry each writes
READINGS = {
    'sweep': 'sweep', 'shot': 'shot', 'result': 'run_stop', 'notch': 'notch',
    'deviation': 'deviation', 'anomaly': 'anomaly_opened',
    'disposition': 'anomaly_dispositioned', 'setting change': 'setting_change',
    'opinion': 'opinion_given',
}

#: the readings forms that belong to one shape of run (`RUN_SHAPES`), offered
#: only for a test whose runs hold it
FOR_SHAPES = {
    'sweep': ('sweeps', 'levels and sweeps', 'any'),
    'shot': ('shots', 'any'),
    'result': ('result', 'any'),
}

#: the small acts and the kind of entry each writes
SMALL = {'note': 'note', 'clock check': 'clock_check', 'person in': 'person_in',
         'person out': 'person_out', 'run start': 'run_start', 'run stop': 'run_stop',
         'cause found': 'cause_found'}

#: every form the window opens, in the order its bars offer them
FORMS = ('level row', 'abort', *SMALL, 'correct', 'open test', 'roll', 'close test',
         'release', 'header', *READINGS, 'new log')

#: what a box is typed as
BOX_KINDS = ('text', 'number', 'whole', 'numbers', 'texts', 'flag', 'choice', 'path',
             'entry', 'notches', 'files', 'answers', 'heading')


@dataclass(frozen=True)
class Predictions:
    """The predictions drawn for a project: the notch prediction
    (`core.limit_prediction.LimitPrediction`) and the drive prediction
    (`core.drive_prediction.DrivePrediction`), each None until one is drawn,
    with what each was made from in words."""

    limit: Any = None
    limit_from: str = ''
    drive: Any = None
    drive_from: str = ''


#: the predictions each open project has drawn, by its id; dropped with it
_DRAWN: dict[int, Predictions] = {}


def offer(project: Any, kind: str, prediction: Any, source: str) -> None:
    """Keep `prediction` for the run log of `project`'s connection: `kind`
    'limit' (a notch prediction) or 'drive', `source` naming the objects it
    was made from. The next of a kind replaces the last."""
    key = id(project)
    if key not in _DRAWN:
        weakref.finalize(project, _DRAWN.pop, key, None)
    held = _DRAWN.get(key, Predictions())
    if kind == 'limit':
        _DRAWN[key] = replace(held, limit=prediction, limit_from=source)
    elif kind == 'drive':
        _DRAWN[key] = replace(held, drive=prediction, drive_from=source)
    else:
        raise ValueError(f'{kind!r} is not a kind of prediction; it is limit or drive')


def drawn(project: Any) -> Predictions:
    """The predictions drawn for `project`, none for no project."""
    return Predictions() if project is None else _DRAWN.get(id(project), Predictions())


def _now() -> datetime:
    return datetime.now().astimezone()


def _clock(when: datetime) -> str:
    return when.strftime('%H:%M:%S')


def _iso(when: datetime | None) -> str | None:
    return None if when is None else when.isoformat()


# ---- the window's view of a log ------------------------------------------------


def entry_line(entry: Entry) -> str:
    """An entry on one line, as the day list shows it: its time, kind,
    place, fields as the plain text export writes them, free text and
    author."""
    parts = [_clock(entry.event_time), entry.label, _where(entry)]
    fields = _fields_text(entry, entry.fields)
    if fields:
        parts.append(fields)
    if entry.text:
        parts.append(f'"{entry.text}"')
    parts.append(entry.author)
    return '   '.join(parts)


def next_level(steps: Any, written: Sequence[float]) -> float | None:
    """The level a level row offers: the first step of the header's step
    schedule above the last level written in the run, the first step when
    none is, the last written when the schedule is used up or absent, and
    None with neither."""
    schedule = sorted(float(step) for step in steps or ()
                      if isinstance(step, (int, float)) and not isinstance(step, bool))
    last = written[-1] if written else None
    if not schedule:
        return last
    if last is None:
        return schedule[0]
    higher = [step for step in schedule if step > last + LEVEL_MATCH_DB]
    return higher[0] if higher else last


def snapshot(log: RunLog, read_only: str = '') -> dict[str, Any]:
    """The window's view of `log` in plain values: its model read once
    (`RunLogModel`) and everything the window shows taken from it.

    `rows` is the corrected view, each entry as it was written when a
    correction changed it (struck through, with what changed beneath), a
    late entry marked with when it was entered. `status` is what the strip
    reads, `holds` and `owed` the whole log's, `header` the current test's
    as it stands, `run` the run a level row goes into (the latest, while it
    runs, otherwise the next) and `level` the level it offers. The rest
    feed the forms: what is carried, the sweeps in the run, the pulse
    count, the entries a deviation answers, the open anomalies and aborts,
    and the tests closed to runs and not released."""
    model = RunLogModel(log)
    rows = []
    for entry in model.entries:
        stored = log.entry(entry.id)
        corrections = [f'{_correction_text(log, correction)} (#{correction.id}, '
                       f'{correction.author}, {_clock(correction.event_time)})'
                       for correction in log.corrections_of(entry.id)]
        rows.append({'id': entry.id, 'kind': entry.kind, 'test': entry.test,
                     'date': entry.event_time.date().isoformat(),
                     'line': entry_line(stored if corrections else entry),
                     'corrections': corrections,
                     'late': _clock(entry.entered_at) if entry.is_late() else None})
    problems = [str(problem) for problem in log.problems]
    for test in model.tests():
        problems.extend(test.problems)
    current = model.current
    status: dict[str, Any] = {'test': None}
    header: dict[str, Any] = {}
    run_number = None
    levels: list[float] = []
    sweeps: list[dict[str, Any]] = []
    if current is not None:
        header = dict(current.header.values)
        latest = max(current.runs, key=lambda run: run.number, default=None)
        status = {'test': current.number, 'title': current.title, 'kind': current.kind,
                  'axis': current.axes[-1] if current.axes else None, 'run': None,
                  'level_db': None, 'outcome': '', 'started': None, 'stopped': None}
        run_number = model.next_run(current.number)
        if latest is not None:
            level = None
            for entry in latest.entries:
                if isinstance(entry.fields.get('level_db'), (int, float)):
                    level = float(entry.fields['level_db'])
            running = latest.outcome == 'running'
            stop = latest.stop
            status.update(axis=latest.axis, run=latest.number, level_db=level,
                          outcome=latest.outcome,
                          started=_iso(latest.entries[0].event_time),
                          stopped=None if running or stop is None else _iso(stop.event_time))
            if running:
                run_number = latest.number
                levels = [float(entry.fields['level_db']) for entry in latest.levels
                          if 'level_db' in entry.fields]
                sweeps = [dict(entry.fields) for entry in latest.sweeps]
    holds = list(model.holds())
    shape = RUN_SHAPES.get(status.get('kind')) if current is not None else None
    pulses = model.pulses(current.number) if current is not None else None
    return {
        'path': log.path, 'title': log.header.title, 'writable': log.writable,
        'read_only': read_only, 'mirror': log.mirror, 'mirror_problem': log.mirror_problem,
        'problems': problems, 'rows': rows,
        'tests': [{'number': test.number, 'title': test.title, 'kind': test.kind,
                   'state': test.state} for test in model.tests()],
        'status': status, 'header': _plain(header), 'holds': [str(hold) for hold in holds],
        'owed': [{'title': owed.title, 'test': owed.test, 'lines': len(owed.lines),
                  'owed': [line.label for line in owed.owed]} for owed in model.owed()],
        'run': run_number, 'level': next_level(header.get('steps_db'), levels),
        'readings': [name for name in READINGS
                     if name not in FOR_SHAPES or shape in FOR_SHAPES[name]],
        # what the forms read, not shown as such
        '_levels': levels, '_sweeps': sweeps, '_pulses': pulses,
        '_carried': list(current.header.carried) if current is not None else [],
        '_holds': holds,
        '_deviation_wanted': [hold.entry.id for hold in holds
                              if hold.rule in ('unplanned-notch', 'plan-change')],
        '_open_aborts': [hold.entry.id for hold in holds if hold.rule == 'abort-cause'],
        '_open_anomalies': [found.number for found in model.open_anomalies()],
        '_unreleased': [test.number for test in model.tests()
                        if test.state == 'closed to runs'],
    }


def _plain(value: Any) -> Any:
    """A header's values as JSON holds them: tuples as lists."""
    if isinstance(value, Mapping):
        return {str(key): _plain(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_plain(item) for item in value]
    return value


def shown(seen: dict[str, Any]) -> dict[str, Any]:
    """A snapshot as the shell is sent it: without what only the forms read."""
    return {key: value for key, value in seen.items() if not key.startswith('_')}


# ---- reading what is typed -----------------------------------------------------


def _number(text: str, what: str) -> float | None:
    text = text.strip()
    if not text:
        return None
    try:
        return float(text)
    except ValueError:
        raise RunLogError(f'{what}: {text!r} is not a number') from None


def _whole(text: str, what: str) -> int | None:
    text = text.strip().lstrip('#')
    if not text:
        return None
    try:
        return int(text)
    except ValueError:
        raise RunLogError(f'{what}: {text!r} is not a whole number') from None


def _numbers(text: str, what: str) -> list[float] | None:
    items = [item for item in text.replace(';', ',').split(',') if item.strip()]
    return [_number(item, what) for item in items] or None  # type: ignore[misc]


def _texts(text: str) -> list[str] | None:
    items = [item.strip() for item in text.replace(';', ',').split(',')]
    return [item for item in items if item] or None


def channel_answers(text: str) -> dict[str, str] | None:
    """Answers by channel as typed in a level row: 'channel: answer' items
    separated by commas or semicolons, such as '12: not explained, 4: as
    predicted'. None for nothing typed."""
    answers: dict[str, str] = {}
    for item in text.replace(';', ',').split(','):
        if not item.strip():
            continue
        name, colon, answer = item.partition(':')
        answer = answer.strip().lower()
        if not colon or not name.strip():
            raise RunLogError(f'channel answers: {item.strip()!r} is not "channel: answer"')
        if answer not in ANSWERS:
            raise RunLogError(f'channel {name.strip()}: {answer!r} is not an answer; it is '
                              'one of: ' + ', '.join(ANSWERS))
        answers[name.strip()] = answer
    return answers or None


def planned_notches(text: str) -> list[dict[str, Any]] | None:
    """A header's planned notches as typed: items separated by semicolons,
    each its band with the axis before a colon and the depth after a comma
    where the plan gives them, such as 'Z: 180-220 Hz, 6 dB; 400-450 Hz'."""
    notches: list[dict[str, Any]] = []
    for item in text.split(';'):
        if not item.strip():
            continue
        axis, colon, rest = item.partition(':')
        if not colon:
            axis, rest = '', item
        band, comma, depth = rest.partition(',')
        notch: dict[str, Any] = {}
        if axis.strip():
            notch['axis'] = axis.strip()
        if not band.strip():
            raise RunLogError(f'planned notches: {item.strip()!r} gives no band')
        band_hz(band.strip(), f'planned notches: {item.strip()!r}: its band')
        notch['band'] = band.strip()
        if comma:
            number = depth.strip()
            if number.lower().endswith('db'):
                number = number[:-2].strip()
            notch['depth_db'] = _number(number,
                                        f'planned notches: {item.strip()!r}: its depth')
            if notch['depth_db'] is None:
                raise RunLogError(f'planned notches: {item.strip()!r}: the depth after the '
                                  'comma is blank')
        notches.append(notch)
    return notches or None


def _number_text(value: Any) -> str:
    # as short as reads back to the same number, so a box left as it was
    # offered reads as unchanged
    if isinstance(value, float) and float(f'{value:g}') == value:
        return f'{value:g}'
    return str(value)


def notches_text(notches: Any) -> str:
    """Planned notches as `planned_notches` reads them back."""
    items = []
    for notch in notches or ():
        band = notch.get('band')
        if not isinstance(band, str):
            band = f'{_number_text(band[0])}-{_number_text(band[1])} Hz'
        text = f'{notch["axis"]}: {band}' if notch.get('axis') else band
        if notch.get('depth_db') is not None:
            text += f', {_number_text(notch["depth_db"])} dB'
        items.append(text)
    return '; '.join(items)


def _paths(text: str) -> list[str] | None:
    paths = [item.strip() for item in text.split(';')]
    return [path for path in paths if path] or None


def hashed(paths: Sequence[str]) -> list[FileRef]:
    """Each file at `paths` as a `FileRef`, read and hashed now."""
    refs = []
    for path in paths:
        try:
            refs.append(file_ref(path))
        except OSError as error:
            raise RunLogError(f'{path} could not be read to attach it: '
                              f'{error.strerror or error}') from None
    return refs


def event_time(text: str, taken: datetime) -> datetime:
    """The event time a form gives: `taken` (when the form was opened)
    while its box still reads that time, otherwise the time typed (HH:MM or
    HH:MM:SS) on `taken`'s date and zone, so an event entered late is
    written when it happened."""
    text = text.strip()
    if not text or text == _clock(taken):
        return taken
    for pattern in ('%H:%M:%S', '%H:%M'):
        try:
            typed = datetime.strptime(text, pattern).time()
            break
        except ValueError:
            continue
    else:
        raise RunLogError(f'event time: {text!r} is not a time of day (HH:MM or HH:MM:SS)')
    return datetime.combine(taken.date(), time(typed.hour, typed.minute, typed.second),
                            tzinfo=taken.tzinfo)


def _box_text(kind: str, value: Any) -> str:
    """A default as its box shows it."""
    if value is None:
        return ''
    if kind == 'flag':
        return 'yes' if value else 'no'
    if kind == 'notches':
        return notches_text(value)
    if kind in ('path', 'files') and isinstance(value, (list, tuple)):
        return '; '.join(str(item) for item in value)
    if isinstance(value, (list, tuple)):
        return ', '.join(_number_text(item) for item in value)
    return _number_text(value)


class Reader:
    """What was typed in a form's boxes (`texts`, by field), read as each
    field holds it by its box kind; None for a blank box. `RunLogError`
    naming the box for one that cannot be read."""

    def __init__(self, spec: Mapping[str, Any], texts: Mapping[str, Any]) -> None:
        self.boxes: dict[str, Mapping[str, Any]] = {
            f['field']: f for f in spec['fields'] if f['box'] != 'heading'}
        self.required: tuple[str, ...] = tuple(spec['required'])
        self.texts: dict[str, str] = {field: str(texts.get(field, '') or '')
                                      for field in self.boxes}

    def value(self, field: str) -> Any:
        """Box `field` read by its kind, None when blank."""
        box = self.boxes[field]
        kind, text, label = box['box'], self.texts[field], box['label']
        if kind in ('text', 'path', 'choice'):
            return text.strip() or None
        if kind == 'number':
            return _number(text, label)
        if kind in ('whole', 'entry'):
            return _whole(text, label)
        if kind == 'numbers':
            return _numbers(text, label)
        if kind == 'texts':
            return _texts(text)
        if kind == 'notches':
            return planned_notches(text)
        if kind == 'files':
            return _paths(text)
        if kind == 'answers':
            return channel_answers(text)
        return {'yes': True, 'no': False}.get(text.strip().lower())

    def values(self) -> dict[str, Any]:
        """Every box read, blank ones left out; a required box left blank
        is refused by its label."""
        values: dict[str, Any] = {}
        for field, box in self.boxes.items():
            value = self.value(field)
            if value is None and field in self.required:
                raise RunLogError(f'{box["label"]} is needed')
            if value is not None:
                values[field] = value
        return values

    def typed(self, *names: str) -> dict[str, Any]:
        """The boxes `names` that hold something, read."""
        return {name: value for name in names
                if (value := self.value(name)) is not None}


# ---- the forms -----------------------------------------------------------------


def _form(name: str, title: str, fields: Sequence[tuple[str, str, str, Any]], *,
          required: Sequence[str] = (), text: str = 'Note', note: str = '',
          defaults: Mapping[str, Any] | None = None, with_time: bool = True,
          live: bool = False) -> dict[str, Any]:
    defaults = dict(defaults or {})
    boxes = []
    for field, label, box, choices in fields:
        if box not in BOX_KINDS:
            raise ValueError(f'{box!r} is not a kind of box')
        entry: dict[str, Any] = {'field': field, 'label': label, 'box': box}
        if box == 'choice':
            entry['choices'] = [str(choice) for choice in choices]
        if box != 'heading':
            entry['value'] = _box_text(box, defaults.get(field))
        boxes.append(entry)
    return {'name': name, 'title': title, 'fields': boxes, 'required': list(required),
            'text': text, 'note': note, 'with_time': with_time, 'live': live}


def _header_form(name: str, seen: Mapping[str, Any] | None) -> dict[str, Any]:
    header = dict(seen['header']) if seen is not None and name != 'open test' else {}
    carried = set(seen['_carried']) if seen is not None and name == 'header' else set()
    fields: list[tuple[str, str, str, Any]] = []
    for group, rows in HEADER_GROUPS:
        kept = [row for row in rows if not (name == 'header' and row[0] == 'test_kind')]
        if not kept:
            continue
        fields.append(('', group, 'heading', None))
        for field, label, kind in kept:
            choices = None
            if field == 'test_kind':
                choices = ('', *TEST_KINDS) if name == 'roll' else TEST_KINDS
            if (name == 'roll' and field in header) or field in carried:
                label += ' (carried)'
            fields.append((field, label, kind, choices))
    if name == 'header':
        fields.append(('', 'Carried fields', 'heading', None))
        fields.append(('confirm', 'Confirm those left as they are', 'flag', None))
    else:
        fields.append(('', 'Attached', 'heading', None))
        fields.append(('files', 'Files (the plan, the procedure, the profile)', 'files', None))
    if name == 'roll':
        fields.append(('reason', 'Why this test closes', 'text', None))
    if name == 'open test':
        title, note = 'Open the next test', ''
    elif name == 'roll':
        title = 'Roll to the next test'
        note = ('Every field left as it is offered is carried to the next test, and marked '
                'carried there until it is confirmed or changed. A box emptied is carried as '
                'it was.')
    else:
        status = seen['status'] if seen is not None else {'test': None}
        title = (f'Header of test {status["test"]}: {status["title"]} ({status["kind"]})'
                 if status['test'] is not None else 'Header')
        note = ('Each field changed is written as a header change. Once the test is running, '
                'a change of a limit, tolerance, abort band, planned notch or hold point raises '
                'a hold until a deviation names it.')
        if carried:
            note += (' Carried from the last test and not yet confirmed: '
                     + ', '.join(field for field in HEADER_FIELDS if field in carried) + '.')
    return _form(name, title, fields, required=('test_kind',) if name == 'open test' else (),
                 note=note, defaults=header, text='Why' if name == 'header' else 'Note')


def _correct_form(log: RunLog, target: Any) -> dict[str, Any]:
    if target is None:
        raise RunLogError('Pick the entry to correct in the day list first.')
    try:
        entry = log.current(int(target))
    except (KeyError, ValueError, TypeError):
        raise RunLogError(f'the log has no entry #{target}') from None
    if entry.kind == 'correction':
        raise RunLogError('A correction is not corrected; correct the original.')
    fields = []
    for name in KINDS[entry.kind].fields:
        held = FIELD_TYPES.get(name, 'text')
        box = {'number': 'number', 'whole': 'whole', 'entry': 'entry', 'flag': 'flag',
               'numbers': 'numbers', 'texts': 'texts', 'notches': 'notches'}.get(held, 'text')
        if held in ('answers', 'scalar') and name in entry.fields \
                and not isinstance(entry.fields[name], str):
            continue          # not typed in a box; corrected by script
        choices = None
        if name == 'stop':
            box, choices = 'choice', ('', *STOPS)
        fields.append((name, name.replace('_', ' ').capitalize(), box, choices))
    spec = _form('correct', f'Correct #{entry.id} ({entry.label})', fields, text='Why',
                 defaults=dict(entry.fields), with_time=False)
    spec['target'] = entry.id
    return spec


def form_spec(name: str, seen: Mapping[str, Any] | None, log: RunLog | None = None,
              axis: str | None = None, target: Any = None) -> dict[str, Any]:
    """The form `name` (one of `FORMS`) as the shell draws it: its title,
    its boxes (field, label, box kind, choices, and the text it opens
    with), the required fields, the label of its line of free text (none
    when ''), a note, whether it takes an event time, and whether `preview`
    reads it as it is typed. A correction names the entry it corrects
    (`target`)."""
    if name == 'level row':
        return _form('level row', 'Level row', [
            ('level_db', 'Level (dB)', 'number', None),
            ('voltage', 'Voltage (V)', 'number', None),
            ('current', 'Current (A)', 'number', None),
            ('drive_fraction', 'Drive fraction', 'number', None),
            ('answer', 'Answer', 'choice', ('', *ANSWERS)),
            ('channel_answers', 'By channel', 'answers', None),
            ('go', 'Go', 'flag', None),
            ('agreed_by', 'Agreed by', 'text', None)], text='',
            defaults={'level_db': seen['level'] if seen is not None else None}, live=True)
    if name == 'abort':
        fields = [('stop', 'Kind of stop', 'choice', ('', *STOPS))]
        fields += [(field, field.replace('_', ' ').capitalize(),
                    'number' if FIELD_TYPES[field] == 'number' else 'text', None)
                   for field in KINDS['abort'].optional]
        return _form('abort', 'ABORT', fields, required=('stop',), text='What was seen',
                     note='Only the kind of stop is needed now; what is left blank is listed '
                          'as owed.')
    if name == 'header' and (seen is None or seen['status']['test'] is None):
        raise RunLogError('No test is open: open one first, and its header is here.')
    if name in ('open test', 'roll', 'header'):
        return _header_form(name, seen)
    if name == 'correct':
        if log is None:
            raise RunLogError('No log is open.')
        return _correct_form(log, target)
    specs: dict[str, Callable[[], dict[str, Any]]] = {
        'note': lambda: _form('note', 'Note', [], text='Note'),
        'clock check': lambda: _form('clock check', 'Clock check', [
            ('offset_s', 'Recorders less this computer (s)', 'number', None),
            ('recorder', 'Recorder', 'text', None), ('method', 'Method', 'text', None)],
            required=('offset_s',)),
        'person in': lambda: _form('person in', 'Person in', [
            ('person', 'Person', 'text', None), ('role', 'Role', 'text', None)],
            required=('person',)),
        'person out': lambda: _form('person out', 'Person out', [
            ('person', 'Person', 'text', None), ('role', 'Role', 'text', None)],
            required=('person',)),
        'run start': lambda: _form('run start', 'Run start', [
            ('level_db', 'Level (dB)', 'number', None), ('counter', 'Counter', 'text', None)]),
        'run stop': lambda: _form('run stop', 'Run stop (not an abort)', [
            ('counter', 'Counter', 'text', None), ('scheduled', 'Scheduled stop', 'flag', None),
            ('result', 'Result', 'text', None), ('by', 'Run by', 'text', None),
            ('reason', 'Reason', 'text', None)]),
        'cause found': lambda: _form('cause found', 'Cause found', [
            ('cause', 'Cause', 'text', None), ('refers_to', 'Of entry #', 'entry', None)],
            required=('cause',),
            defaults={'refers_to': seen['_open_aborts'][-1]
                      if seen is not None and seen['_open_aborts'] else None}),
        'close test': lambda: _form('close test', 'Close the test to runs', [
            ('reason', 'Reason', 'text', None)], text=''),
        'release': lambda: _form('release', 'Release a test', [
            ('test', 'Test', 'whole', None)], required=('test',), text='Released by',
            defaults={'test': seen['_unreleased'][-1]
                      if seen is not None and seen['_unreleased'] else None}),
        'new log': lambda: _form('new log', 'New run log', [
            ('path', 'File', 'path', None), ('title', 'Article or program', 'text', None),
            ('mirror', 'Second copy (optional)', 'path', None)], required=('path',), text='',
            with_time=False),
        'sweep': lambda: _sweep_form(seen),
        'shot': lambda: _form('shot', 'Shot', [
            ('polarity', 'Polarity', 'choice', ('', 'positive', 'negative')),
            ('n', 'Shot n', 'whole', None), ('of', 'of N', 'whole', None),
            ('level_db', 'Level (dB)', 'number', None),
            ('pulses', 'Pulses (1 when blank)', 'whole', None),
            ('', 'The four checks', 'heading', None),
            ('in_tolerance', 'Pulse or spectrum in tolerance, both polarities', 'flag', None),
            ('within_stroke', 'Displacement and velocity within the stroke', 'flag', None),
            ('peak_current', 'Amplifier current peak (A)', 'number', None),
            ('peak_voltage', 'Amplifier voltage peak (V)', 'number', None),
            ('prediction', 'Controller\'s prediction for the next level', 'text', None)],
            live=True),
        'result': lambda: _form('result', 'Survey or functional result', [
            ('result', 'Result', 'text', None), ('by', 'Run by', 'text', None),
            ('counter', 'Counter', 'text', None)], required=('result',)),
        'notch': lambda: _form('notch', 'Notch', [
            ('axis', 'Axis', 'text', None), ('channel', 'Channel', 'text', None),
            ('band', 'Band (Hz)', 'text', None), ('depth_db', 'Depth (dB)', 'number', None)],
            required=('band',), defaults={'axis': axis}, live=True),
        'deviation': lambda: _form('deviation', 'Deviation', [
            ('fact', 'Fact at the console', 'text', None),
            ('cause', 'Cause (when known)', 'text', None),
            ('effect', 'Effect (when known)', 'text', None),
            ('approval', 'Approval (when known)', 'text', None),
            ('agreed_by', 'Agreed at the console by', 'text', None),
            ('refers_to', 'Names entry #', 'entry', None)],
            defaults={'refers_to': seen['_deviation_wanted'][-1]
                      if seen is not None and seen['_deviation_wanted'] else None},
            note='The names typed are recorded as agreed at the console; the log does not '
                 'track signatures or approvals.', live=True),
        'anomaly': lambda: _form('anomaly', 'Anomaly opened', [
            ('fact', 'What was seen', 'text', None), ('owner', 'Owner', 'text', None),
            ('refers_to', 'Names entry #', 'entry', None)]),
        'disposition': lambda: _disposition_form(seen),
        'setting change': lambda: _form('setting change', 'Setting changed', [
            ('setting', 'Setting', 'text', None), ('value', 'Now', 'text', None),
            ('was', 'Was', 'text', None),
            ('governs', 'Governs', 'choice', ('', *SETTING_CLASSES))], required=('setting',),
            note='A limit, tolerance or abort band changed holds the test until a deviation '
                 'names the change.'),
        'opinion': lambda: _form('opinion', 'Opinion given', [
            ('to', 'Given to', 'text', None), ('opinion', 'Opinion', 'text', None)]),
    }
    builder = specs.get(name)
    if builder is None:
        raise bad_request(f'{name!r} is not a form of the run log; it is one of '
                          + ', '.join(FORMS))
    return builder()


def _sweep_form(seen: Mapping[str, Any] | None) -> dict[str, Any]:
    sweeps = seen['_sweeps'] if seen is not None else []
    last = dict(sweeps[-1]) if sweeps else {}
    defaults = {'n': len(sweeps) + 1,
                **{name: last[name] for name in ('of', 'rate', 'span', 'level_db')
                   if name in last}}
    return _form('sweep', 'Sweep', [
        ('direction', 'Direction', 'choice', ('', *DIRECTIONS)),
        ('n', 'Sweep n', 'whole', None), ('of', 'of N', 'whole', None),
        ('rate', 'Rate (oct/min or Hz/s)', 'text', None), ('span', 'Span (Hz)', 'text', None),
        ('level_db', 'Level (dB)', 'number', None),
        ('current', 'Amplifier current, rms (A)', 'number', None)],
        defaults=defaults, live=True)


def _disposition_form(seen: Mapping[str, Any] | None) -> dict[str, Any]:
    open_ones = seen['_open_anomalies'] if seen is not None else []
    return _form('disposition', 'Anomaly dispositioned', [
        ('number', 'Anomaly number', 'text', None),
        ('disposition', 'Disposition', 'text', None), ('by', 'By', 'text', None)],
        required=('number',), defaults={'number': open_ones[-1] if open_ones else None},
        note='Open: ' + ', '.join(open_ones) if open_ones else 'No anomaly is open.')


# ---- what a form computes as it is typed ---------------------------------------


def projection_text(header: Mapping[str, Any], level: Any, *, voltage: Any = None,
                    current: Any = None, drive_fraction: Any = None) -> str:
    """The projection of a level row's readings to full level, in words,
    against the test's header (`run_log_model.project`)."""
    if level is None:
        return 'not judged: no level is given'
    try:
        projection = project(level, voltage=voltage, current=current,
                             drive_fraction=drive_fraction, sigma=header.get('sigma'),
                             rated_current_peak=header.get('rated_current_peak'),
                             rated_current_rms=header.get('rated_current_rms'),
                             rail_voltage=header.get('rail_voltage'),
                             drive_window=header.get('drive_window'))
    except RunLogError as error:
        return f'not judged: {error}'
    return str(projection)


def _level_text(header: Mapping[str, Any], form: Reader, predictions: Predictions,
                system: Any) -> str:
    fields = form.typed('level_db', 'voltage', 'current', 'drive_fraction')
    projection = projection_text(header, fields.get('level_db'),
                                 voltage=fields.get('voltage'), current=fields.get('current'),
                                 drive_fraction=fields.get('drive_fraction'))
    return f'{projection}\n{drive_text(predictions, header, fields, system)}'


def drive_text(predictions: Predictions, header: Mapping[str, Any],
               fields: Mapping[str, Any], system: Any = None) -> str:
    """The drive predicted for the test's setup (each drive demand in the
    prediction's own words, `Demand.describe`) beside the drive the level row
    reads, its drive fraction projected to full level
    (`run_log_model.project`)."""
    prediction = predictions.drive
    if prediction is None:
        return f'Predicted drive: {NO_PREDICTION}'
    drives = [demand.describe(system) for demand in prediction.demands
              if demand.kind == 'drive']
    said = (f'Predicted drive (from {predictions.drive_from}): '
            + ('; '.join(drives) if drives else 'the prediction judges no drive'))
    level = fields.get('level_db')
    fraction = fields.get('drive_fraction')
    if fraction is None:
        return f'{said}\nRead drive: no drive fraction read'
    if level is None:
        return f'{said}\nRead drive: {fraction:g} of the range, at no level'
    try:
        check = project(level, drive_fraction=fraction,
                        drive_window=header.get('drive_window')).check('drive fraction')
    except RunLogError as error:
        return f'{said}\nRead drive: not projected: {error}'
    return f'{said}\nRead drive: {fraction:g} of the range at {level:+g} dB; {check.message}'


def sweep_text(header: Mapping[str, Any], form: Reader) -> str:
    """A sweep's rms current against the header's continuous rating."""
    typed = form.typed('n', 'of', 'level_db', 'current')
    count = f'Sweep {typed["n"]} of {typed["of"]}. ' if 'n' in typed and 'of' in typed else ''
    rating = header.get('rated_current_rms')
    if rating is None:
        return count + ('Continuous rating: not judged: the header gives no continuous '
                        'current rating')
    if 'current' not in typed:
        return count + 'Continuous rating: not judged: no current was read'
    if 'level_db' not in typed:
        return count + 'Continuous rating: not judged: no level is given'
    projection = project(typed['level_db'], current=typed['current'], rated_current_rms=rating)
    return count + 'Continuous rating: ' + projection.check('rms current').message


def shot_text(seen: Mapping[str, Any] | None, axis: str | None, form: Reader) -> str:
    """The four checks answered and owed, and the pulse count so far and
    with the shot being typed, against the header's budget."""
    owed = [label for _key, names, label in SHOT_CHECKS
            if any(form.value(name) is None for name in names)]
    checks = f'Checks answered: {len(SHOT_CHECKS) - len(owed)} of {len(SHOT_CHECKS)}'
    if owed:
        checks += '; owed: ' + '; '.join(label.lower() for label in owed)
    counted = seen['_pulses'] if seen is not None else None
    if counted is None:
        return checks + '\nPulses: no test is open'
    pulses = form.value('pulses') or 1
    by_axis = dict(counted.by_axis)
    by_axis[axis] = by_axis.get(axis, 0) + pulses
    after = judge_pulses(counted.test, by_axis, counted.budget)
    return f'{checks}\nPulses so far: {counted.message}\nWith this shot: {after.message}'


def notch_text(header: Mapping[str, Any], form: Reader, predictions: Predictions,
               system: Any = None) -> str:
    """A notch's place in the plan, then the prediction beside it."""
    typed = form.typed('band', 'depth_db')
    planned, why = notch_in_plan(header.get('notches_planned'), form.value('axis'), typed)
    plan = (f'Planned: {why}' if planned else
            f'Not planned: {why} Written, it holds the test until a deviation names it.')
    return plan + '\n' + predicted_notch_text(predictions, typed.get('band'),
                                              typed.get('depth_db'), system)


def predicted_notch_text(predictions: Predictions, band: Any, depth_db: float | None,
                         system: Any = None) -> str:
    """The notch the limit prediction cuts at `band` beside the notch
    observed there, each predicted notch in the prediction's own words
    (`Notch.describe`); with no band typed, the prediction's verdict."""
    prediction = predictions.limit
    if prediction is None:
        return f'Predicted: {NO_PREDICTION}'
    said = f'Predicted (from {predictions.limit_from})'
    if band is None:
        return f'{said}: {prediction.verdict()}'
    try:
        low, high = band_hz(band, 'the band')
    except RunLogError as error:
        return f'{said}: not compared: {error}'
    observed = (f'Observed: {low:g} to {high:g} Hz, '
                + (f'{abs(depth_db):g} dB deep' if depth_db is not None else 'no depth given'))
    near = [notch for notch in prediction.notches if notch.low <= high and notch.high >= low]
    if not near:
        return (f'{said}: no notch from {low:g} to {high:g} Hz; {prediction.verdict()}\n'
                f'{observed}')
    return (f'{said}: ' + '; '.join(notch.describe(prediction.dimension, prediction.tolerance,
                                                    system) for notch in near)
            + f'\n{observed}')


def deviation_text(seen: Mapping[str, Any] | None, form: Reader) -> str:
    """Which hold a deviation answers: the one the entry it names raised."""
    named = form.value('refers_to')
    if named is None:
        return ('Names no entry: a deviation answers a hold only when it names the entry '
                'that raised it')
    holds = seen['_holds'] if seen is not None else []
    hold = next((found for found in holds if found.entry.id == named
                 and found.rule in ('unplanned-notch', 'plan-change')), None)
    if hold is None:
        return f'#{named} raises no hold that a deviation answers'
    return f'Answers the hold at #{named}: {hold.message}'


def preview(name: str, spec: Mapping[str, Any], texts: Mapping[str, Any],
            seen: Mapping[str, Any] | None, axis: str | None,
            predictions: Predictions | None = None, system: Any = None) -> str:
    """What form `name` computes from what is typed, in words, the
    predictions drawn shown beside what is read; '' for a form that computes
    nothing. A box that cannot be read says so."""
    if not spec.get('live'):
        return ''
    predictions = predictions or Predictions()
    form = Reader(spec, texts)
    header = dict(seen['header']) if seen is not None else {}
    try:
        if name == 'level row':
            return _level_text(header, form, predictions, system)
        if name == 'sweep':
            return sweep_text(header, form)
        if name == 'shot':
            return shot_text(seen, axis, form)
        if name == 'notch':
            return notch_text(header, form, predictions, system)
        if name == 'deviation':
            return deviation_text(seen, form)
    except RunLogError as error:
        return f'not read: {error}'
    return ''


# ---- writing -------------------------------------------------------------------


def _place(seen: Mapping[str, Any], axis: str | None) -> tuple[int | None, str | None,
                                                                int | None]:
    status = seen['status']
    if status['test'] is None:
        return None, axis, None
    return status['test'], axis, seen['run']


def write(log: RunLog, seen: Mapping[str, Any], name: str, spec: Mapping[str, Any],
          texts: Mapping[str, Any], text: str, when_text: str, taken: datetime,
          author: str, axis: str | None) -> Any:
    """Write form `name` as typed into `log`, as Visual Dynamics' dock writes
    it: the event time read from its box against `taken`, every entry
    checked before any is written. `RunLogError` saying why for anything
    the log refuses."""
    form = Reader(spec, texts)
    values = form.values()
    when = event_time(when_text, taken) if spec['with_time'] else taken
    test, axis, run = _place(seen, axis)
    if name == 'level row':
        if not values:
            raise RunLogError('the row is empty')
        return model_module.append(log, 'level_row', author=author, test=test, axis=axis,
                                   run=run, fields=values, event_time=when)
    if name == 'abort':
        stop_fields = {'reason': f'abort: {values.get("stop", "")}'}
        if 'counter' in values:
            stop_fields['counter'] = values['counter']
        # both checked before either is written, so a refusal never leaves the
        # stop written without its abort
        check_fields('run_stop', stop_fields)
        check_fields('abort', values)
        model_module.admit(RunLogModel(log), 'abort', test)
        log.append('run_stop', author=author, test=test, axis=axis, run=run,
                   fields=stop_fields, event_time=when)
        return log.append('abort', author=author, test=test, axis=axis, run=run,
                          fields=values, text=text, event_time=when)
    kind = SMALL.get(name)
    if kind is not None:
        in_run = kind in model_module.RUN_KINDS
        return model_module.append(log, kind, author=author, test=test,
                                   axis=axis if in_run else None,
                                   run=run if in_run else None, fields=values, text=text,
                                   event_time=when)
    if name == 'correct':
        target = log.current(int(spec['target']))
        changed = {field: value for field, value in values.items()
                   if target.fields.get(field) != value}
        for field in form.boxes:
            # a box emptied strikes an optional field it held
            if field not in values and field in target.fields \
                    and field in KINDS[target.kind].optional:
                changed[field] = None
        if not changed:
            raise RunLogError('Nothing was changed.')
        return log.correct(target.id, author=author, fields=changed, text=text)
    if name in ('open test', 'roll', 'header'):
        return _write_header(log, seen, name, form, values, text, when, author, test)
    if name in READINGS:
        return _write_reading(log, name, values, text, when, author, test, axis, run)
    if name == 'close test':
        if test is None:
            raise RunLogError('No test is open to runs.')
        return model_module.close_test(log, test, author=author,
                                       reason=values.get('reason', ''), event_time=when)
    if name == 'release':
        return model_module.release(log, values['test'], author=author, text=text,
                                    event_time=when)
    raise bad_request(f'{name!r} is not a form that writes an entry')


def header_changes(values: Mapping[str, Any], header: Mapping[str, Any]) -> dict[str, Any]:
    """The header fields among `values` that differ from `header`."""
    return {field: value for field, value in values.items()
            if field in HEADER_FIELDS and not _same(field, value, header.get(field))}


def _same(field: str, value: Any, held: Any) -> bool:
    if field == 'notches_planned':
        return held is not None and notches_text(value) == notches_text(held)
    if isinstance(held, (list, tuple)) and isinstance(value, (list, tuple)):
        return list(value) == list(held)
    return value == held


def _write_header(log: RunLog, seen: Mapping[str, Any], name: str, form: Reader,
                  values: dict[str, Any], text: str, when: datetime, author: str,
                  test: int | None) -> Any:
    values = dict(values)
    paths = values.pop('files', None) or []
    reason = values.pop('reason', '')
    confirming = values.pop('confirm', False)
    held = dict(seen['header'])
    if name == 'open test':
        test_kind = values.pop('test_kind')
        return model_module.open_test(log, author=author, test_kind=test_kind, header=values,
                                      text=text, event_time=when, files=hashed(paths))
    if name == 'roll':
        given = header_changes(values, held)
        kind = given.pop('test_kind', None)
        return model_module.roll(log, author=author, test_kind=kind, header=given,
                                 reason=reason, text=text, event_time=when, files=hashed(paths))
    if test is None:
        raise RunLogError('No test is open.')
    emptied = [form.boxes[field]['label'] for field in held
               if field in form.boxes and field not in values]
    if emptied:
        raise RunLogError(', '.join(emptied) + ': a header field is changed to its new value, '
                          'not emptied; put the value back or type the new one.')
    changed = header_changes(values, held)
    carried = [field for field in seen['_carried'] if field not in changed] if confirming else []
    if not changed and not carried:
        raise RunLogError('Nothing was changed.' + (
            ' Tick Confirm to confirm the carried fields as they are.'
            if seen['_carried'] else ''))
    # every change checked before any is written, so a refusal never leaves
    # half the form in the log
    for field, value in changed.items():
        check_fields('header_change', {'field': field, 'value': value})
    written = [model_module.change_header(log, test, field, value, author=author, text=text,
                                          event_time=when) for field, value in changed.items()]
    if carried:
        written += model_module.confirm(log, test, carried, author=author, event_time=when)
    return written


def _write_reading(log: RunLog, name: str, values: dict[str, Any], text: str,
                   when: datetime, author: str, test: int | None, axis: str | None,
                   run: int | None) -> Entry:
    """A readings form written as its kind of entry: a run's entries to the
    current run on the axis, a finding naming an entry to that entry's
    test, a disposition to its anomaly's; a notch marked planned or not as
    the header's list has it."""
    kind = READINGS[name]
    fields = dict(values)
    if name == 'notch':
        axis = fields.pop('axis', None)
    in_run = kind in model_module.RUN_KINDS
    model = RunLogModel(log)
    owner = test
    named = fields.get('refers_to')
    if named is not None:
        # an entry the log does not hold is refused by the log, with its
        # number, when the entry is written
        with contextlib.suppress(KeyError):
            owner = log.current(named).test
    if kind == 'anomaly_dispositioned':
        owner = next((entry.test for entry in model.entries
                      if entry.kind == 'anomaly_opened'
                      and entry.fields.get('number') == fields.get('number')), test)
    model_module.admit(model, kind, owner)
    if kind == 'notch' and owner is not None:
        fields['planned'] = notch_in_plan(model.test(owner).header.get('notches_planned'),
                                          axis, fields)[0]
    return log.append(kind, author=author, test=owner, axis=axis if in_run else None,
                      run=run if in_run else None, fields=fields, text=text, event_time=when)


def said_after(name: str, written: Any, seen: Mapping[str, Any]) -> str:
    """What the window says once form `name` is written: an abort's owed
    first minute, an unplanned notch's hold."""
    if name == 'abort' and isinstance(written, Entry):
        owed = next((found for found in seen['owed'] if found['title'].endswith(
            f'#{written.id}')), None)
        if owed is not None and owed['owed']:
            return f'Abort #{written.id} written. Owed: ' + ', '.join(owed['owed'])
    if name == 'notch' and isinstance(written, Entry) \
            and written.id in seen['_deviation_wanted']:
        return (f'Notch #{written.id} is not planned: the test holds until a deviation names '
                'it.')
    return ''


# ---- one connection's log ------------------------------------------------------


class Keeper:
    """The run log a connection has open: written to here, or read-only
    (`read_only` saying why) when another writer holds it."""

    def __init__(self) -> None:
        self.log: RunLog | None = None
        self.read_only: str = ''
        #: the predictions drawn for the connection's project, and the unit
        #: system its words are in, as the request that reads them sets them
        self.predictions: Predictions = Predictions()
        self.system: Any = None

    def close(self) -> None:
        """Close the log, releasing its lock. Safe to call again."""
        log, self.log, self.read_only = self.log, None, ''
        if log is not None:
            log.close()

    def _seen(self) -> dict[str, Any]:
        if self.log is None:
            raise refused('No log is open: start one with New Log, or open one.')
        return snapshot(self.log, self.read_only)

    def state(self) -> dict[str, Any]:
        """`{'log': snapshot}`, or `{'log': None}` with none open."""
        return {'log': None if self.log is None else shown(self._seen())}

    def new(self, path: str, title: str = '', mirror: str | None = None) -> dict[str, Any]:
        """Start a log at `path` and hold it open for writing."""
        self.close()
        try:
            self.log = RunLog.create(path, title=title, mirror=mirror or None)
        except (RunLogError, OSError) as error:
            raise refused(str(error)) from None
        return self.state()

    def open(self, path: str, mirror: str | None = None) -> dict[str, Any]:
        """Open the log at `path` for writing; one that cannot be written
        here (another writer holds it, or it is of an older format) opens
        read-only, saying why."""
        self.close()
        try:
            try:
                self.log = RunLog.open(path, mirror=mirror or None)
            except RunLogError as why:
                self.log, self.read_only = RunLog.read(path), str(why)
        except (RunLogError, OSError) as error:
            raise refused(str(error)) from None
        return self.state()

    def reload(self) -> dict[str, Any]:
        """Read the log again: one read-only shows what its writer has
        added since."""
        if self.log is not None and not self.log.writable:
            try:
                self.log = RunLog.read(self.log.path)
            except (RunLogError, OSError) as error:
                raise refused(str(error)) from None
        return self.state()

    def export(self, path: str, view: str, test: Any) -> dict[str, Any]:
        """One view of the log written to `path` (`core.run_log_export`):
        'text' (the log as it reads), 'csv' (every entry as stored,
        corrections included) or 'machine' (the trips and machine events for
        the lab's own log), the whole log ('all') or one test by number, as
        VD's run-log dock exports; `{'path'}` written."""
        from hertzforge.core.run_log import RunLogError
        from hertzforge.core.run_log_export import VIEWS, export

        if self.log is None:
            raise refused('no run log is open to export')
        if view not in VIEWS:
            raise bad_request('runlog.export "view" is one of ' + ', '.join(VIEWS))
        if not path.strip():
            raise bad_request('runlog.export needs "path", where to write')
        if test is None or test == 'all':
            which: Any = 'all'
        elif isinstance(test, int) and not isinstance(test, bool) and test > 0:
            which = test
        else:
            raise bad_request('runlog.export "test" is a test number, or "all" for the whole log')
        try:
            written = export(self.log, os.path.expanduser(path.strip()), view, test=which)
        except (RunLogError, OSError) as error:
            raise refused(str(error)) from None
        return {'path': written}

    def form(self, name: str, axis: str | None, target: Any) -> dict[str, Any]:
        """Form `name` as it opens now: `form_spec`, when it was taken, and
        what it computes from what it opens with."""
        seen = snapshot(self.log, self.read_only) if self.log is not None else None
        if name != 'new log' and seen is None:
            raise refused('No log is open: start one with New Log, or open one.')
        try:
            spec = form_spec(name, seen, self.log, axis, target)
        except RunLogError as error:
            raise refused(str(error)) from None
        now = _now()
        return {**spec, 'taken': now.isoformat(), 'time': _clock(now),
                'preview': preview(name, spec, {f['field']: f.get('value', '')
                                                for f in spec['fields']}, seen, axis,
                                   self.predictions, self.system)}

    def preview(self, name: str, texts: Mapping[str, Any], axis: str | None,
                target: Any) -> dict[str, Any]:
        """What form `name` computes from `texts` as typed."""
        seen = self._seen()
        try:
            spec = form_spec(name, seen, self.log, axis, target)
        except RunLogError as error:
            raise refused(str(error)) from None
        return {'text': preview(name, spec, texts, seen, axis, self.predictions, self.system)}

    def write(self, name: str, texts: Mapping[str, Any], text: str, when: str, taken: str,
              author: str, axis: str | None, target: Any) -> dict[str, Any]:
        """Write form `name` as typed by `author`, and the log as it then
        stands with what the window says after (`said_after`)."""
        if name == 'new log':
            spec = form_spec('new log', None)
            try:
                values = Reader(spec, texts).values()
            except RunLogError as error:
                raise refused(str(error)) from None
            return self.new(values['path'], values.get('title', ''), values.get('mirror'))
        seen = self._seen()
        log = self.log
        assert log is not None
        if not log.writable:
            raise refused('This log is open read-only. ' + self.read_only)
        if not author.strip():
            raise refused('Type your initials first: every entry records its author.')
        try:
            # in this computer's zone, as the log's own times are, whatever
            # zone the front end's clock wrote it in
            taken_at = (datetime.fromisoformat(taken) if taken else _now()).astimezone()
        except ValueError:
            raise bad_request(f'runlog.write "taken" is not a time: {taken!r}') from None
        try:
            spec = form_spec(name, seen, log, axis, target)
            written = write(log, seen, name, spec, texts, text, when, taken_at,
                            author.strip(), axis)
        except RunLogError as error:
            raise refused(str(error)) from None
        except OSError as error:
            raise refused(f'the log could not be written: {error}') from None
        after = snapshot(log, self.read_only)
        return {'log': shown(after), 'said': said_after(name, written, after)}


def _text(params: Mapping[str, Any], key: str, method: str, default: str = '') -> str:
    value = params.get(key, default)
    if value is None:
        return default
    if not isinstance(value, str):
        raise bad_request(f'{method} "{key}" is text')
    return value


def request(keeper: Keeper, method: str, params: Any, project: Any = None,
            system: Any = None) -> dict[str, Any]:
    """Answer one `runlog.*` request on `keeper`, run on a worker thread, the
    predictions drawn for `project` (the connection's) shown in `system`'s
    units."""
    keeper.predictions, keeper.system = drawn(project), system
    if not isinstance(params, Mapping):
        raise bad_request(f'{method} "params" must be an object')
    axis = _text(params, 'axis', method).strip() or None
    target = params.get('target')
    if method == 'runlog.state':
        return keeper.state()
    if method == 'runlog.new':
        return keeper.new(_text(params, 'path', method), _text(params, 'title', method),
                          _text(params, 'mirror', method) or None)
    if method == 'runlog.open':
        return keeper.open(_text(params, 'path', method), _text(params, 'mirror', method) or None)
    if method == 'runlog.close':
        keeper.close()
        return keeper.state()
    if method == 'runlog.reload':
        return keeper.reload()
    if method == 'runlog.export':
        return keeper.export(_text(params, 'path', method), _text(params, 'view', method),
                             params.get('test', 'all'))
    name = _text(params, 'form', method)
    if name not in FORMS:
        raise bad_request(f'{method} "form" is one of ' + ', '.join(FORMS))
    if method == 'runlog.form':
        return keeper.form(name, axis, target)
    texts = params.get('values', {})
    if not isinstance(texts, Mapping):
        raise bad_request(f'{method} "values" is an object of the boxes\' text, by field')
    if method == 'runlog.preview':
        return keeper.preview(name, texts, axis, target)
    return keeper.write(name, texts, _text(params, 'text', method), _text(params, 'time', method),
                        _text(params, 'taken', method), _text(params, 'author', method), axis,
                        target)
