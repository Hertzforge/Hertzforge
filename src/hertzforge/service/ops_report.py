"""Visual Dynamics' report features, and the data a report rests on, as
operations on a report the project holds (made by `report.generate`).

Which optional features a report carries (the Report Features panel,
`Project.report_features`); the panes those features open in the report
editor: the test requirement, the findings register, check blocks, the
test dates and calibration status, responses against prediction, the runs
and their chronology, and the flash report's typed parts; the project's
data index (`Project.index_file`, `unindex_file`, `verify_data_index`);
report templates (`io.report_template`); Reports from Runs
(`run_report`); and run logs brought into the project as extracts
(`Project.attach_run_log` and its kin). Each calls the project method
Visual Dynamics' editor or script calls, and shows what VD's pane shows
afterwards; the report itself is drawn again by the Report workspace.
"""

from __future__ import annotations

import os
from collections.abc import Callable
from typing import Any

from . import views as v
from .descriptors import Operation, Option, Param
from .protocol import refused

REPORT = Param('report', 'Report', 'enum', options_from='objects:report')
#: named `source` so the feature switches open on the report's own state
SOURCE = Param('source', 'Report', 'enum', options_from='objects:report')
RUN_LOG = Param('run_log', 'Run log extract', 'enum', options_from='objects:run_log_extract')
SPECIFICATION = Param('specification', 'Specification', 'enum',
                      options_from='objects:specification')
MEASURED_PSD = Param('measured', 'Measured', 'enum', options_from='objects:psd')
NUMBER = Param('number', 'Finding', 'string', default='')
SEARCH = Param('search', 'Also look in', 'string', default='')


def _core(call: Callable[..., Any], *args: Any, **kwargs: Any) -> Any:
    """`call` with a refusal of the core's said in its own words."""
    try:
        return call(*args, **kwargs)
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    except OSError as error:
        where = f': {error.filename}' if error.filename else ''
        raise refused(f'{error.strerror or error}{where}') from None


def _words(text: str) -> str:
    return text.strip()


def _list(text: str) -> list[str]:
    """Items typed one per line or separated by semicolons."""
    return [item.strip() for item in text.replace(';', '\n').splitlines() if item.strip()]


def _markdown(title: str, text: str) -> list[dict]:
    """Markdown the core offers (a sentence, a pipe table, the lines
    under it) as views: each pipe table a `table`, the rest a `reading`
    of its lines."""
    tables: list[tuple[list[str], list[list[str]]]] = []
    lines: list[str] = []
    rows: list[list[str]] | None = None
    for raw in text.splitlines():
        line = raw.strip()
        if line.startswith('|') and line.endswith('|'):
            cells = [cell.strip() for cell in line[1:-1].split('|')]
            if rows is None:
                rows = []
                tables.append((cells, rows))
            elif not all(set(cell) <= set('-: ') for cell in cells):
                rows.append(cells)
            continue
        rows = None
        if line:
            lines.append(line.removeprefix('- '))
    out = [v.table(title, headers, body) for headers, body in tables]
    if lines:
        out.append(v.reading(title, [v.row('', line, None, 'info') for line in lines]))
    return out


# ---- report features ---------------------------------------------------------------

def _feature_params() -> tuple[Param, ...]:
    from hertzforge.report.features import FEATURES

    return tuple(Param(key, feature.title, 'boolean', default=False,
                       setting=f'features.{key}') for key, feature in FEATURES.items())


def _preset_param() -> Param:
    from hertzforge.report.features import PRESETS

    return Param('preset', 'Preset', 'enum', default='custom', options=(
        Option('custom', 'As ticked'),
        *(Option(preset.key, preset.title) for preset in PRESETS.values())))


def _features_reading(name: str, on: tuple[str, ...], notes: tuple[str, ...] = ()) -> dict:
    from hertzforge.report.features import FEATURES, GROUPS, PRESETS, preset_of, reason

    preset = preset_of(on)
    rows = [v.row('Preset', PRESETS[preset].title if preset else 'Custom', None, 'info')]
    for group in GROUPS:
        for key, feature in FEATURES.items():
            if feature.group != group:
                continue
            why = reason(key, on)
            value = 'on' if key in on else 'off' + (f'; {why}' if why else '')
            rows.append(v.row(f'{group}: {feature.title}', value, None,
                              'pass' if key in on else 'info'))
    rows += [v.row('Note', note, None, 'warn') for note in notes]
    return v.reading(f'Report Features of {name}', rows)


def report_features(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.report_features`: turn a report's optional features on or
    off (`Project.report_features`, the Report Features panel), by a
    preset or by a switch per feature, and show each feature on, off, or
    off with what it waits on. A feature switched on whose requirement
    is not met is refused, and nothing changes."""
    from hertzforge.report.features import FEATURES, enabled

    name = params['source']
    if params['preset'] != 'custom':
        chosen = _core(project.report_features, name, preset=params['preset'])
    else:
        wanted = {key for key in FEATURES if params[key]}
        held = set(enabled(project[name]))
        chosen = _core(project.report_features, name,
                       enable=[key for key in FEATURES if key in wanted - held],
                       disable=[key for key in FEATURES if key in held - wanted])
    return [_features_reading(name, chosen.enabled, chosen.notes)]


# ---- the requirement ---------------------------------------------------------------

STATES = {'met': 'pass', 'not met': 'fail', 'not judged': 'info'}


def _requirement_reading(project: Any, report: str, index: int) -> dict:
    """The requirement block's pane as VD shows it: what it is bound to,
    the requirement in words and each criterion's result
    (`report.requirement.read`)."""
    from hertzforge.report.requirement import read, requirement_lines, roles_on

    held = project[report]
    block = held.blocks[index]
    rows = [v.row('Specification', str(block.get('source', '')), None, 'info'),
            v.row('Run', str(block.get('measured', '')), None, 'info')]
    rows += [v.row('Requirement', line, None, 'info') for line in requirement_lines(block)]
    try:
        found = read(block, project, project.object_groups, roles=roles_on(held))
    except (TypeError, ValueError) as error:
        rows.append(v.row('Not read', str(error), None, 'warn'))
        found = None
    if found is not None:
        rows += [v.row(f'{row.name}, {row.required}', f'{row.state} ({row.measured})', None,
                       STATES.get(row.state, 'info')) for row in found.rows]
        rows.append(v.row('Result', found.state, None, STATES.get(found.state, 'info')))
    return v.reading(f'Test requirement in {report}', rows)


def report_requirement(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.report_requirement`: bind a test requirement to a run in a
    report (`Project.report_requirement`), one control band about the
    specification, and read each criterion against the run as the
    report's result table does. An edge left at 0 Hz is the
    specification's own end; a tolerance or a duration left at 0 is not
    part of the requirement."""
    low, high = params['low'] or None, params['high'] or None
    channels = params['dofs'].replace(',', ' ').split()
    index = _core(
        project.report_requirement, params['report'], params['specification'],
        params['measured'], params['document'],
        [(low, high, -params['below_db'], params['above_db'])],
        revision=params['revision'], clause=params['clause'], channels=channels,
        grms_percent=params['grms_percent'] or None, rule=params['rule'],
        combine=params['combine'], duration_run=params['duration_run'] or None)
    return [_requirement_reading(project, params['report'], index)]


# ---- the findings register ---------------------------------------------------------

FINDING_COLUMNS = ('Number', 'Kind', 'Run', 'Channel', 'Band', 'Status', 'Fact', 'Cause',
                   'Effect', 'Approval or disposition', 'Owes', 'Draft')


def _register(project: Any, report: str) -> list[dict]:
    """The findings register as VD's pane lists it, a row per finding with
    the parts it owes and its draft, and the findings a run log offers
    beside it (`report.findings.register`, `log_drafts`)."""
    from hertzforge.core import findings as core
    from hertzforge.report import findings

    held = project[report]
    rows = []
    for entry in findings.register(held):
        band = entry['band']
        owed = [core.FOURTH[entry['kind']].lower() if part == 'approval' else part
                for part in core.owed(entry)]
        draft = entry['draft']
        rows.append([entry['number'], entry['kind'], entry['run'], entry['channel'],
                     f'{band[0]:g} to {band[1]:g} Hz' if band else '', entry['status'],
                     entry['fact'], entry['cause'], entry['effect'], entry['approval'],
                     ', '.join(owed), f'{draft["text"]} ({draft["source"]})' if draft else ''])
    out = [v.table(f'Findings register of {report}', FINDING_COLUMNS, rows)]
    offered = findings.log_drafts(held)
    if offered:
        out.append(v.table(f'Offered from the run log, not yet accepted ({report})',
                           ('Number', 'Kind', 'Run', 'Fact', 'Source'),
                           [[draft['number'], draft['kind'], str(draft.get('run', '')),
                             draft['fact'], draft['source']] for draft in offered]))
    return out


FINDING_STATUS = Param('status', 'Status', 'enum', default='unchanged', options=(
    Option('unchanged', 'Unchanged'), Option('open', 'Open'), Option('closed', 'Closed'),
    Option('not stated', 'Not stated')))


def report_finding(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.report_finding`: enter a finding in a report's register,
    change one, or take it out (`Project.report_finding`). With no
    number a finding of the kind is entered under the next number of
    its kind; a field left empty is left as it is. The register follows,
    each finding with the parts it still owes."""
    name = params['report']
    fields: dict[str, Any] = {key: _words(params[key]) for key in
                              ('run', 'fact', 'cause', 'effect', 'approval')
                              if _words(params[key])}
    if _words(params['dof']):
        fields['channel'] = _words(params['dof'])
    if params['low'] or params['high']:
        fields['band'] = (params['low'], params['high'])
    if params['status'] != 'unchanged':
        fields['status'] = '' if params['status'] == 'not stated' else params['status']
    number = _words(params['number']) or None
    if params['remove']:
        if number is None:
            raise refused('name the finding to remove, such as D-1')
        _core(project.report_finding, name, number, remove=True)
    else:
        _core(project.report_finding, name, number,
              kind=None if number else params['kind'], **fields)
    return _register(project, name)


def draft_finding_fact(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.draft_finding_fact`: offer a finding a draft of its fact,
    read off the comparison of a measured PSD with a specification on
    the same cells as the comparison figure
    (`Project.draft_finding_fact`). The draft fills nothing until it is
    accepted."""
    name = params['report']
    number = _words(params['number'])
    if not number:
        raise refused('name the finding the draft is for, such as D-1')
    text = _core(project.draft_finding_fact, name, number, params['measured'],
                 params['specification'], _words(params['dof']) or None,
                 limits=params['limits'])
    return [v.reading(f'Draft of {number}', [v.row('Draft', text, None, 'info')]),
            *_register(project, name)]


def draft_finding_from_requirement(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.draft_finding_from_requirement`: offer a finding the fact
    a row of a requirement's result table reads, a notch the plan does
    not name or a cross-axis channel over its limit
    (`Project.draft_finding_from_requirement`). The row is its number in
    the table, from 1, or its name."""
    name = params['report']
    number = _words(params['number'])
    if not number:
        raise refused('name the finding the draft is for, such as D-1')
    row = _words(params['criterion'])
    criterion: int | str = int(row) if row.isdigit() else row
    text = _core(project.draft_finding_from_requirement, name, number, params['measured'],
                 params['specification'], criterion)
    return [v.reading(f'Draft of {number}', [v.row('Draft', text, None, 'info')]),
            *_register(project, name)]


def draft_findings_from_run_log(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.draft_findings_from_run_log`: offer the report the
    deviations and anomalies a run log extract holds, as drafts beside
    the register (`Project.draft_findings_from_run_log`); nothing enters
    the register until a draft is accepted."""
    name = params['report']
    lines = _core(project.draft_findings_from_run_log, name, params['run_log'])
    return [v.reading(f'From {params["run_log"]}',
                      [v.row('', line, None, 'info') for line in lines]),
            *_register(project, name)]


def accept_finding(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.accept_finding`: accept or discard a draft: the fact
    drafted on a finding (`Project.report_finding` with accept or
    discard) or a finding a run log offered
    (`Project.accept_logged_finding`), which accepting enters in the
    register. A fact typed here is the draft as edited."""
    name = params['report']
    number = _words(params['number'])
    if not number:
        raise refused('name the finding whose draft to accept or discard, such as D-1')
    fact = _words(params['fact']) or None
    discard = params['decision'] == 'discard'
    if discard and fact is not None:
        raise refused(f'{number}: a discarded draft takes no fact')
    if params['offered'] == 'run_log':
        _core(project.accept_logged_finding, name, number, fact=fact, discard=discard)
    elif discard:
        _core(project.report_finding, name, number, discard=True)
    else:
        _core(project.report_finding, name, number, fact=fact, accept=True)
    return _register(project, name)


# ---- check blocks and the report check ---------------------------------------------

def _settings(text: str) -> dict[str, Any]:
    """'peak=490; duration=0.011; modes=[[120, 25]]' as keyword
    arguments: each value read as JSON where it reads as JSON, else as
    text."""
    import json

    out: dict[str, Any] = {}
    for item in _list(text):
        key, said, value = item.partition('=')
        if not said or not key.strip():
            raise refused(f'{item!r} is not a setting; write each as name=value')
        try:
            out[key.strip()] = json.loads(value)
        except ValueError:
            out[key.strip()] = value.strip()
    return out


def _check_reading(project: Any, report: str, index: int) -> dict:
    """A check block's pane as VD shows it: the check, what it reads and
    its settings, as the block holds them."""
    from hertzforge.report.check_blocks import CHECKS

    block = project[report].blocks[index]
    found = CHECKS.get(str(block.get('check')))
    title = found.title if found is not None else str(block.get('check'))
    rows = [v.row('Check', title, None, 'info')]
    if found is not None:
        rows.append(v.row('Reads', found.description, None, 'info'))
    rows += [v.row(field, str(block[field]), None, 'info')
             for field in ('source', 'measured', 'specification', 'shapes') if block.get(field)]
    rows += [v.row(key, repr(value), None, 'info')
             for key, value in (block.get('settings') or {}).items()]
    return v.reading(f'{title} check in {report}, block {index + 1}', rows)


def _check_options() -> tuple[Option, ...]:
    from hertzforge.report.check_blocks import CHECKS

    return tuple(Option(key, check.title) for key, check in CHECKS.items())


def report_check_block(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.report_check_block`: give a report a check of its own,
    with what it reads and its settings (`Project.report_check_block`,
    report feature Check blocks). A field left empty reads its default
    binding; the settings are the check's reading's keyword arguments,
    typed name=value."""
    bindings = {key: _words(params[key]) for key in
                ('source', 'measured', 'specification', 'shapes') if _words(params[key])}
    index = _core(project.report_check_block, params['report'], params['check'],
                  **bindings, **_settings(params['settings']))
    return [_check_reading(project, params['report'], index)]


def report_check(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.report_check`: what a report still owes before it is
    issued, a line per section (`Project.report_check`, what Export
    lists); the flash report's owed lines with that feature on."""
    name = params['report']
    lines = _core(project.report_check, name)
    rows = ([v.row(*line.split(': ', 1), None, 'warn') if ': ' in line
             else v.row('', line, None, 'warn') for line in lines]
            or [v.row('Owed', 'nothing: every section says something', None, 'pass')])
    return [v.reading(f'What {name} still owes', rows)]


# ---- test dates and calibration ----------------------------------------------------

def _calibration(project: Any, report: str) -> list[dict]:
    """Each sensor of the report's channel table against the test dates,
    as the Calibration column writes it, and the check in words
    (`Project.report_calibration`)."""
    from hertzforge.core.calibration import EXPIRED, IN_DATE, LAPSES

    found = _core(project.report_calibration, report)
    status = {IN_DATE: 'pass', EXPIRED: 'fail', LAPSES: 'fail'}
    rows = [v.row(f'Channel {each.channel}, {each.key[0]}', each.words, None,
                  status.get(each.status, 'info')) for each in found.channels]
    rows += [v.row('', line, None, 'warn' if found.lapsed else 'info')
             for line in found.summary()]
    return [v.reading(f'Calibration of {report}, {found.span}', rows)]


def report_test_date(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.report_test_date`: the dates of the test a report covers
    (`Project.report_test_date`), YYYY-MM-DD, the last day left empty
    for a one-day test, and each sensor's calibration judged against
    them. An empty first day clears the dates."""
    name = params['report']
    start = _words(params['start'])
    _core(project.report_test_date, name, start or None, _words(params['end']) or None)
    if not start:
        return [v.reading(f'Calibration of {name}',
                          [v.row('Test dates', 'cleared: nothing to judge against', None,
                                 'info')])]
    return _calibration(project, name)


def report_calibration(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.report_calibration`: each sensor's calibration status on
    the test dates (`Project.report_calibration`), and the equipment list
    the Setup and Method section is offered
    (`Project.report_calibration_text`), put at the end of that section
    when asked."""
    from hertzforge.report.skeleton import words

    name = params['report']
    views = _calibration(project, name)
    text = _core(project.report_calibration_text, name)
    if params['insert']:
        held = words(project[name], 'setup')
        _core(project.report_section, name, 'setup', f'{held}\n\n{text}' if held else text)
    return views + _markdown(f'Equipment list offered to Setup and Method of {name}', text)


# ---- prediction --------------------------------------------------------------------

def report_prediction(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.report_prediction`: add a block reading a response against
    predictions to the end of the report (`Project.report_prediction`),
    and show its table: each predicted peak against the measured one,
    in SI (`report.prediction.build`). The level is the one the record
    was run at, dB relative to full level; a transmissibility takes
    none, and a PSD that holds its scaling uses its own."""
    import hertzforge as hz
    from hertzforge.core.data import Frf
    from hertzforge.core.prediction import level_run
    from hertzforge.report.prediction import build

    name, measured = params['report'], params['measured']
    held = project[measured]
    level: float | None = params['level']
    if isinstance(held, Frf) or _core(level_run, held, None)[0] is not None:
        level = None
    index = _core(project.report_prediction, name, measured, params['predictions'],
                  level=level, caption=params['caption'])
    out = []
    for block in build(project[name].blocks[index], project, hz.SI, project.object_groups) or ():
        if block['kind'] == 'table':
            out.append(v.table(block['caption'], block['headers'], block['rows']))
        elif block['kind'] == 'text':
            out += _markdown(f'Responses against prediction in {name}', block['text'])
    return out


# ---- runs and chronology -----------------------------------------------------------

RUN_COLUMNS = (('Record', ''), ('Axis', 'axis'), ('Run', 'run'), ('Level (dB)', 'level_db'),
               ('Start', 'start'), ('Comment', 'comment'))


def _require_runs(project: Any, name: str) -> None:
    from hertzforge.report.features import enabled
    from hertzforge.report.runs import FEATURE

    report = project[name]
    if FEATURE not in enabled(report):
        raise refused(f'report {report.title!r} does not have Runs and chronology enabled')


def _runs_table(project: Any, report: str) -> dict:
    """The runs, a row each, as VD's Runs panel shows them
    (`report.runs.run_order`): a value the record could not give one way
    says why."""
    from hertzforge.core.runs import level_text
    from hertzforge.report.runs import histories, run_order

    names, found = run_order(project, histories(project), project.object_groups)
    rows = []
    for name in names:
        identity = found[name]
        row = [name]
        for _title, field in RUN_COLUMNS[1:]:
            value, why = identity.value(field), identity.reason(field)
            row.append(f'not read: {why}' if why else '' if value in ('', None)
                       else level_text(value).removesuffix(' dB') if field == 'level_db'
                       else str(value))
        rows.append(row)
    return v.table(f'Runs of {report}', [title for title, _field in RUN_COLUMNS], rows)


def report_run_identity(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.report_run_identity`: the runs a report draws on, a row
    each with its axis, run, level, start and comment, and one field of
    one run entered (`Project.set_run_identity`), as a cell of VD's Runs
    panel is typed in. An empty value goes back to what the record
    shows."""
    name = params['report']
    _require_runs(project, name)
    field = params['field']
    if field != 'none':
        value: Any = _words(params['value'])
        if field == 'level_db' and value:
            try:
                value = float(value.removesuffix('dB').strip())
            except ValueError:
                pass
        _core(project.set_run_identity, params['history'], **{field: value})
    return [_runs_table(project, name)]


def report_runs(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.report_runs`: repeat the report's Results for every run it
    does not draw on yet, each figure bound to that run's own objects
    (`Project.report_runs`, Results for Every Run)."""
    name = params['report']
    lines = _core(project.report_runs, name)
    return [v.reading(f'Results for every run of {name}',
                      [v.row('', line, None, 'info') for line in lines]),
            _runs_table(project, name)]


def report_chronology(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.report_chronology`: draft the chronology of the runs and
    offer it to the report's Chronology section
    (`Project.report_chronology`, Draft from the Runs): each run with
    its time at level and the interruptions found on the running RMS.
    The section stays owed until the draft is accepted."""
    name = params['report']
    text = _core(project.report_chronology, name, window_s=params['window'],
                 drop_db=params['drop'], min_s=params['shortest'])
    return _markdown(f'Chronology drafted for {name}, not yet accepted', text)


def accept_chronology(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.accept_chronology`: make the drafted chronology the
    report's Chronology section, as drafted or as edited here
    (`Project.accept_chronology`, Accept Draft)."""
    from hertzforge.report.skeleton import words

    name = params['report']
    _core(project.accept_chronology, name, _words(params['text']) or None)
    return _markdown(f'Chronology of {name}', words(project[name], 'chronology'))


# ---- the flash report --------------------------------------------------------------

OPINIONS_NONE = {'unchanged': None, 'none': True, 'take_back': False}


def report_flash(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.report_flash`: type the parts of a flash report only the
    author can write (`Project.report_flash`): the condition on release,
    what happens next, the report date and the opinions, each 'who:
    what was said'. A part left empty is left as it is. What the page
    still owes follows (`Project.report_check`)."""
    from hertzforge.report.flash import LABELS, held

    name = params['report']
    opinions = None
    if _words(params['opinions']):
        opinions = []
        for item in _list(params['opinions']):
            by, said, text = item.partition(':')
            if not said:
                raise refused(f'{item!r}: write an opinion as who: what was said')
            opinions.append((by.strip(), text.strip()))
    _core(project.report_flash, name,
          **{key: _words(params[key]) for key in LABELS if _words(params[key])},
          opinions=opinions, opinions_none=OPINIONS_NONE[params['opinions_none']])
    page = held(project[name])
    rows = [v.row(label, page[key], None, 'info') for key, label in LABELS.items() if key in page]
    rows += [v.row(f'Opinion, {opinion["by"]}', opinion['text'], None, 'info')
             for opinion in page.get('opinions', [])]
    if page.get('opinions_none'):
        rows.append(v.row('Opinions', 'none given', None, 'info'))
    rows += [v.row(*line.split(': ', 1), None, 'warn') if ': ' in line
             else v.row('', line, None, 'warn') for line in _core(project.report_check, name)]
    return [v.reading(f'Flash report {name}', rows)]


# ---- the data index ----------------------------------------------------------------

def _index(project: Any) -> list[dict]:
    """The data index as the report prints it (`report.data_index.table`)."""
    from hertzforge.report.data_index import EMPTY, table

    index = project.data_index
    if not len(index):
        return [v.reading('Data index', [v.row('', EMPTY, None, 'info')])]
    built = table(index)
    return [v.table('Data index', built['headers'], built['rows'])]


def index_file(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.index_file`: add a file on the service's machine to the
    project's data index, hashed now (`Project.index_file`, Insert,
    Attached File): a document attached to the report, or a file it
    names and the archive keeps."""
    path = _words(params['path'])
    if not path:
        raise refused('name the file to index by its path')
    _core(project.index_file, path, _words(params['holds']) or None, role=params['role'])
    return _index(project)


def unindex_file(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.unindex_file`: take a file out of the data index by the
    name the index prints, or its path when two share a name
    (`Project.unindex_file`, Insert, Remove from Index)."""
    name = _words(params['file'])
    if not name:
        raise refused('name the file to take out of the index')
    _core(project.unindex_file, name)
    return _index(project)


def verify_data_index(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.verify_data_index`: hash every indexed file again and
    record whether it matches, has changed or is not found
    (`Project.verify_data_index`, what Export does), looking in the
    folders given for a file not where it was indexed."""
    from hertzforge.core.data_index import CHANGED, MATCHES, NOT_FOUND
    from hertzforge.report.data_index import checked

    lines = _core(project.verify_data_index, _list(params['search']))
    status = {MATCHES: 'pass', CHANGED: 'fail', NOT_FOUND: 'fail'}
    rows = [v.row(entry.name, entry.status, None, status.get(entry.status, 'info'))
            for entry in project.data_index]
    if lines:
        rows.append(v.row('', checked(project.data_index), None,
                          'pass' if all(e.status == MATCHES for e in project.data_index)
                          else 'warn'))
    return [v.reading('Data index checked', rows), *_index(project)]


# ---- report templates --------------------------------------------------------------

def export_report_template(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.export_report_template`: save a report on its own as a
    template another project can load (`io.report_template`, a
    `.vdreport`), in the templates folder Generate Report looks in when
    no path is given."""
    from pathlib import Path

    from hertzforge.io.report_template import SUFFIX, templates_folder

    name = params['report']
    folder = templates_folder()
    path = os.path.expanduser(_words(params['path'])) or str(folder / f'{name}{SUFFIX}')
    if not path.endswith(SUFFIX):
        path += SUFFIX
    if Path(path).parent == folder:
        folder.mkdir(parents=True, exist_ok=True)
    _core(project.export, name, path)
    rows = [v.row('Saved', path, None, 'pass'),
            v.row('Generate Report', 'offers it as ' + Path(path).name[:-len(SUFFIX)]
                  if Path(path).parent == folder
                  else 'opens it by its path; it is not in the templates folder', None, 'info')]
    return [v.reading(f'Template of {name}', rows)]


def generate_from_template(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.generate_from_template`: a report built from a saved
    template, by the name it was saved under or a `.vdreport` path,
    bound by role to this project's objects (`Project.generate_report`)."""
    from hertzforge.io.report_template import saved_templates

    template = os.path.expanduser(_words(params['template']))
    if not template:
        saved = ', '.join(name for name, _path in saved_templates()) or 'none saved'
        raise refused(f'name a saved template ({saved}) or a .vdreport file')
    made = _core(project.generate_report, template, name=_words(params['name']) or 'Report')
    report = project[made]
    rows = [v.row('Report', made, None, 'pass'),
            v.row('Blocks', float(report.num_blocks), None, 'info'),
            v.row('Unbound blocks', float(len(report.unbound(project))), None,
                  'warn' if report.unbound(project) else 'info')]
    return [v.reading(f'Generated from {params["template"]}', rows)]


# ---- reports from runs -------------------------------------------------------------

RUN_KINDS = (Option('auto', 'As the file records'), Option('random', 'Random Vibration'),
             Option('mixed', 'Random and Sine'), Option('sine', 'Sine Sweep'),
             Option('sysid', 'System ID'))


def reports_from_runs(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.reports_from_runs`: the one-call report each controller
    run's type calls for, written beside it or into a folder
    (`run_report`, File, Reports from Runs). The open project is not
    changed: each run is worked up in a project of its own. A random
    and sine run gets two reports."""
    from hertzforge.project import report_kind, run_report

    runs = [os.path.expanduser(run) for run in _list(params['runs'])]
    if not runs:
        raise refused('name the runs, the controller files, one per line')
    folder = os.path.expanduser(_words(params['folder']))
    path = (folder if folder.endswith(('/', os.sep)) else folder + os.sep) if folder else None
    kinds = None if params['kind'] == 'auto' else dict.fromkeys(runs, params['kind'])
    chosen = [kinds[run] if kinds else _core(report_kind, run) for run in runs]
    written = _core(run_report, runs, path, kinds=kinds, last=params['last'] or None,
                    geometry=os.path.expanduser(_words(params['geometry'])) or None,
                    marking=_words(params['marking']) or None)
    written = written if isinstance(written, list) else [written]
    names = {option.value: option.label for option in RUN_KINDS}
    rows, at = [], 0
    for run, kind in zip(runs, chosen, strict=True):
        count = 2 if kind == 'mixed' else 1
        for report in written[at:at + count]:
            rows.append([os.path.basename(run), names[kind], report])
        at += count
    return [v.table(f'{len(written)} report{"s" * (len(written) != 1)} written',
                    ('Run', 'Report', 'Written to'), rows)]



# ---- run logs ----------------------------------------------------------------------

def _extract(project: Any, name: str) -> list[dict]:
    """A run log extract read as the report reads it: the log, the test,
    the entry it was taken to, whether the log has moved on, and what it
    holds."""
    extract = project[name]
    status = extract.status()
    rows = [v.row('Run log', extract.log_title or os.path.basename(extract.path), None, 'info'),
            v.row('Path', extract.path, None, 'info'),
            v.row('Test', f'{extract.test}: {extract.title} ({extract.kind})', None, 'info'),
            v.row('Taken to entry', float(extract.upto), None, 'info'),
            v.row('Test head hash', extract.head, None, 'info'),
            v.row('Log now', status.message, None, 'info' if status.state == 'current' else 'warn'),
            v.row('Runs', float(len(extract.runs())), None, 'info'),
            v.row('Interruptions', float(len(extract.interruptions())), None, 'info'),
            v.row('Findings', float(len(extract.findings())), None, 'info'),
            v.row('Files named', float(len(extract.files())), None, 'info')]
    rows += [v.row('Credited', line.removeprefix('- '), None, 'info')
             for line in extract.credit_lines()]
    return [v.reading(name, rows)]


def attach_run_log(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.attach_run_log`: bring one test of a run log into the
    project, frozen at an entry, as a run log extract
    (`Project.attach_run_log`). A test left at 0 is the log's one test;
    an entry left at 0 the log's last."""
    path = os.path.expanduser(_words(params['path']))
    if not path:
        raise refused('name the run log by its path')
    name = _core(project.attach_run_log, path, params['test'] or None, params['upto'] or None,
                 name=_words(params['name']) or None)
    return _extract(project, name)


def refresh_run_log(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.refresh_run_log`: take an extract again from its log, to
    the log's newest entry or to the entry given
    (`Project.refresh_run_log`); refused when the log is another log or
    has changed under the extract."""
    name = params['run_log']
    _core(project.refresh_run_log, name, params['upto'] or None)
    return _extract(project, name)


def detach_run_log(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.detach_run_log`: take a run log extract out of the project
    (`Project.detach_run_log`); attaching the same log, test and entry
    brings back the same entries."""
    name = params['run_log']
    extract = _core(project.detach_run_log, name)
    return [v.reading(f'{name} taken out', [
        v.row('Run log', extract.path, None, 'info'),
        v.row('Test', float(extract.test), None, 'info'),
        v.row('Taken to entry', float(extract.upto), None, 'info')])]


def index_run_log_files(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.index_run_log_files`: add the files a run log extract
    names to the data index, each checked against the SHA-256 the log
    records (`Project.index_run_log_files`), looking in the folders
    given, then beside the log and the project file."""
    lines = _core(project.index_run_log_files, params['run_log'], search=_list(params['search']))
    rows = [v.row(*line.split(': ', 1), None,
                  'pass' if line.endswith(': indexed.') else 'warn') if ': ' in line
            else v.row('', line, None, 'info') for line in lines]
    return [v.reading(f'Files named in {params["run_log"]}', rows), *_index(project)]


def _identity_params() -> tuple[Param, ...]:
    from hertzforge.report.identity import FIELDS

    return tuple(Param(field.key, field.label + ('' if field.required else ' (optional)'),
                       'string', default='', setting=f'identity.{field.key}')
                 for field in FIELDS)


def report_identity(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.report_identity`: the report's identification fields as
    entered (`Project.report_identity`), which the page carries in its
    margins and its identification block while the report feature
    `report_identity` is on; then what is still missing before it can be
    issued. People authorized are comma-separated."""
    from hertzforge.report.identity import FIELDS, missing, value

    name = params['source']
    report = project[name]
    fields = {}
    for field in FIELDS:
        given = params[field.key].strip()
        if field.key == 'authorized':
            fields[field.key] = [person.strip() for person in given.split(',') if person.strip()]
        else:
            fields[field.key] = given
    try:
        project.report_identity(name, **fields)
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    rows = [v.row(field.label, ', '.join(value(report, field.key))
                  if field.key == 'authorized' else value(report, field.key) or 'not entered',
                  None, 'info' if value(report, field.key) or not field.required else 'warn')
            for field in FIELDS]
    owed = missing(report)
    rows.append(v.row('Status', value(report, 'status'), None, 'info'))
    rows.append(v.row('Before issue', 'every required field entered' if not owed
                      else 'still to enter: ' + ', '.join(owed), None,
                      'pass' if not owed else 'warn'))
    return [v.reading(f'Identity of {name}', rows)]


def amend_report(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.amend_report`: an issued report amended
    (`Project.amend_report`): the change, where it is, what it said, what
    it says now and why, under a new revision (and a number of its own
    when given). The amendment is then issued like any report."""
    from hertzforge.report.identity import record, value

    name = params['source']
    change = {key: params[key].strip() for key in ('where', 'was', 'now', 'reason')
              if params[key].strip()}
    try:
        project.amend_report(name, [change], revision=params['revision'].strip(),
                             number=params['number'].strip() or None)
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    report = project[name]
    rows = [v.row('Status', value(report, 'status'), None, 'info'),
            v.row('Revision', value(report, 'revision'), None, 'info'),
            v.row('Issues recorded', float(len(record(report))), '', 'info')]
    rows += [v.row(f'Change, {part}', text, None, 'info') for part, text in change.items()]
    return [v.reading(f'Amendment of {name}', rows)]


OPERATIONS = (
    Operation(
        id='project.report_features', title='Report Features', workspace='report',
        accepts='project',
        summary='Which optional features a report carries, from a preset or switched one by '
                'one; a feature that cannot be switched on says what it waits on.',
        params=(SOURCE, _preset_param(), *_feature_params()), outputs=('reading',),
        run=report_features, edits=True),
    Operation(
        id='project.report_requirement', title='Test Requirement', workspace='report',
        accepts='project',
        summary='Bind a test requirement to a run, one control band about the specification, '
                'and read each criterion met, not met or not judged. An edge, tolerance or '
                'duration left at 0 is the specification\'s own or not part of it.',
        params=(REPORT, SPECIFICATION, MEASURED_PSD,
                Param('document', 'Document', 'string', default=''),
                Param('revision', 'Revision', 'string', default=''),
                Param('clause', 'Tolerance clause', 'string', default=''),
                Param('dofs', 'Control channels', 'string', default=''),
                Param('low', 'Band, low', 'number', default=0.0, unit='Hz', min=0.0, step=10.0),
                Param('high', 'Band, high', 'number', default=0.0, unit='Hz', min=0.0,
                      step=10.0),
                Param('below_db', 'Tolerance below', 'number', default=3.0, unit='dB',
                      min=0.0, step=0.5),
                Param('above_db', 'Tolerance above', 'number', default=3.0, unit='dB',
                      min=0.0, step=0.5),
                Param('grms_percent', 'Overall rms tolerance', 'number', default=0.0,
                      unit='%', min=0.0, step=1.0),
                Param('rule', 'Decision rule', 'enum', default='any_line', options=(
                    Option('any_line', 'Any line'), Option('combined', 'Combined lines'))),
                Param('combine', 'Lines combined', 'integer', default=1, min=1, step=1),
                Param('duration_run', 'Time at level', 'number', default=0.0, unit='s',
                      min=0.0, step=10.0)),
        outputs=('reading',), run=report_requirement, edits=True),
    Operation(
        id='project.report_finding', title='Finding', workspace='report', accepts='project',
        summary='Enter a deviation, anomaly or nonconformance in the findings register, change '
                'one by its number, or take it out. A field left empty is left as it is.',
        params=(REPORT, NUMBER,
                Param('kind', 'Kind, for a new finding', 'enum', default='deviation', options=(
                    Option('deviation', 'Deviation'), Option('anomaly', 'Anomaly'),
                    Option('nonconformance', 'Nonconformance'))),
                Param('run', 'Run', 'string', default=''),
                Param('dof', 'Channel', 'string', default=''),
                Param('low', 'Band, low', 'number', default=0.0, unit='Hz', min=0.0),
                Param('high', 'Band, high', 'number', default=0.0, unit='Hz', min=0.0),
                FINDING_STATUS,
                Param('fact', 'Fact', 'string', default=''),
                Param('cause', 'Cause', 'string', default=''),
                Param('effect', 'Effect', 'string', default=''),
                Param('approval', 'Approval or disposition', 'string', default=''),
                Param('remove', 'Remove it', 'boolean', default=False)),
        outputs=('table',), run=report_finding, edits=True),
    Operation(
        id='project.draft_finding_fact', title='Draft Fact from a Measurement',
        workspace='report', accepts='project',
        summary='Offer a finding a draft of its fact read off a measured PSD against a '
                'specification\'s limits; the channel left empty is the finding\'s own.',
        params=(REPORT, NUMBER, MEASURED_PSD, SPECIFICATION,
                Param('dof', 'Channel', 'string', default=''),
                Param('limits', 'Limits', 'enum', default='abort', options=(
                    Option('abort', 'Abort limits'), Option('warning', 'Warning limits')))),
        outputs=('reading', 'table'), run=draft_finding_fact, edits=True),
    Operation(
        id='project.draft_finding_from_requirement', title='Draft Fact from the Requirement',
        workspace='report', accepts='project',
        summary='Offer a finding the fact a row of the requirement\'s result table reads: a '
                'notch the plan does not name, or a cross-axis channel over its limit.',
        params=(REPORT, NUMBER, MEASURED_PSD, SPECIFICATION,
                Param('criterion', 'Row', 'string', default='')),
        outputs=('reading', 'table'), run=draft_finding_from_requirement, edits=True),
    Operation(
        id='project.draft_findings_from_run_log', title='Draft Findings from the Run Log',
        workspace='report', accepts='project',
        summary='Offer the report the deviations and anomalies a run log extract holds, as '
                'drafts beside the register, entered only when accepted.',
        params=(REPORT, RUN_LOG), outputs=('reading', 'table'),
        run=draft_findings_from_run_log, edits=True),
    Operation(
        id='project.accept_finding', title='Accept or Discard a Draft', workspace='report',
        accepts='project',
        summary='Accept a drafted fact, as offered or as edited here, or discard it: a fact '
                'drafted on a finding, or a finding the run log offered.',
        params=(REPORT, NUMBER,
                Param('offered', 'Drafted', 'enum', default='fact', options=(
                    Option('fact', 'On the finding'), Option('run_log', 'From the run log'))),
                Param('decision', 'Decision', 'enum', default='accept', options=(
                    Option('accept', 'Accept'), Option('discard', 'Discard'))),
                Param('fact', 'Fact as edited', 'string', default='')),
        outputs=('table',), run=accept_finding, edits=True),
    Operation(
        id='project.report_check_block', title='Check Block', workspace='report',
        accepts='project',
        summary='Give the report a check of its own: what it reads (empty for its default '
                'binding) and its settings, typed name=value.',
        params=(REPORT,
                Param('check', 'Check', 'enum', default='pulse', options=_check_options()),
                Param('source', 'Source', 'string', default=''),
                Param('measured', 'Measured', 'string', default=''),
                Param('specification', 'Specification', 'string', default=''),
                Param('shapes', 'Shapes', 'string', default=''),
                Param('settings', 'Settings', 'string', default='')),
        outputs=('reading',), run=report_check_block, edits=True),
    Operation(
        id='project.report_check', title='Report Check', workspace='report', accepts='project',
        summary='What the report still owes before it is issued, a line per section.',
        params=(REPORT,), outputs=('reading',), run=report_check),
    Operation(
        id='project.report_test_date', title='Test Dates', workspace='report', accepts='project',
        summary='The first and last day of the test, YYYY-MM-DD, and each sensor\'s '
                'calibration judged against them. An empty first day clears them.',
        params=(REPORT, Param('start', 'Test date', 'string', default=''),
                Param('end', 'Last test day', 'string', default='')),
        outputs=('reading',), run=report_test_date, edits=True),
    Operation(
        id='project.report_calibration', title='Calibration Status', workspace='report',
        accepts='project',
        summary='Each sensor\'s calibration on the test dates, and the equipment list offered '
                'to the Setup and Method section, put at its end when asked.',
        params=(REPORT, Param('insert', 'Insert into Setup and Method', 'boolean',
                              default=False)),
        outputs=('reading', 'table'), run=report_calibration, edits=True),
    Operation(
        id='project.report_prediction', title='Responses Against Prediction',
        workspace='report', accepts='project',
        summary='Add a block reading a response against predictions, and show each predicted '
                'peak against the measured one. The level is dB relative to full level.',
        params=(REPORT,
                Param('measured', 'Measured', 'enum',
                      options_from='objects:psd|spectrum|sine_level|frf'),
                Param('predictions', 'Predictions', 'enum', options_from='objects:predictions'),
                Param('level', 'Level run', 'number', default=0.0, unit='dB', step=1.0),
                Param('caption', 'Caption', 'string', default='')),
        outputs=('table', 'reading'), run=report_prediction, edits=True),
    Operation(
        id='project.report_run_identity', title='Runs', workspace='report', accepts='project',
        summary='The runs the report draws on, a row each, and one field of one run entered; '
                'an empty value goes back to what the record shows.',
        params=(REPORT, Param('history', 'Run', 'enum', options_from='objects:time_history'),
                Param('field', 'Field', 'enum', default='none', options=(
                    Option('none', 'None, show the runs'), Option('axis', 'Axis'),
                    Option('run', 'Run'), Option('level_db', 'Level (dB)'),
                    Option('start', 'Start'), Option('comment', 'Comment'))),
                Param('value', 'Value', 'string', default='')),
        outputs=('table',), run=report_run_identity, edits=True),
    Operation(
        id='project.report_runs', title='Results for Every Run', workspace='report',
        accepts='project',
        summary='Repeat the report\'s Results for every run it does not draw on yet, bound to '
                'that run\'s own objects.',
        params=(REPORT,), outputs=('reading', 'table'), run=report_runs, edits=True),
    Operation(
        id='project.report_chronology', title='Draft Chronology from the Runs',
        workspace='report', accepts='project',
        summary='Draft the chronology of the runs, each with its time at level and the '
                'interruptions on its running RMS, and offer it to the Chronology section.',
        params=(REPORT,
                Param('window', 'RMS window', 'number', default=0.5, unit='s', min=0.01,
                      step=0.1),
                Param('drop', 'Interruption drop', 'number', default=6.0, unit='dB', min=0.1,
                      step=1.0),
                Param('shortest', 'Shortest interruption', 'number', default=1.0, unit='s',
                      min=0.0, step=0.5)),
        outputs=('table', 'reading'), run=report_chronology, edits=True),
    Operation(
        id='project.accept_chronology', title='Accept Chronology', workspace='report',
        accepts='project',
        summary='Make the drafted chronology the Chronology section, as drafted or as edited '
                'here.',
        params=(REPORT, Param('text', 'As edited', 'string', default='')),
        outputs=('table', 'reading'), run=accept_chronology, edits=True),
    Operation(
        id='project.report_flash', title='Flash Report', workspace='report', accepts='project',
        summary='The parts of the flash report only the author can write, and what its page '
                'still owes. A part left empty is left as it is.',
        params=(REPORT, Param('condition', 'Condition on release', 'string', default=''),
                Param('next_steps', 'What happens next', 'string', default=''),
                Param('report_date', 'Report date', 'string', default=''),
                Param('opinions', 'Opinions (who: what was said)', 'string', default=''),
                Param('opinions_none', 'No opinion given', 'enum', default='unchanged',
                      options=(Option('unchanged', 'Unchanged'), Option('none', 'Say none'),
                               Option('take_back', 'Take back')))),
        outputs=('reading',), run=report_flash, edits=True),
    Operation(
        id='project.index_file', title='Attach File', workspace='report', accepts='project',
        summary='Add a file on the service\'s machine to the data index, hashed now: a '
                'document sent with the report, or a file it names.',
        params=(Param('path', 'File', 'string', default=''),
                Param('holds', 'What it holds', 'string', default=''),
                Param('role', 'Role', 'enum', default='attached', options=(
                    Option('attached', 'Attached to the report'),
                    Option('referenced', 'Referenced, kept in the archive')))),
        outputs=('table',), run=index_file, edits=True),
    Operation(
        id='project.unindex_file', title='Remove from Index', workspace='report',
        accepts='project',
        summary='Take a file out of the data index, by the name the index prints.',
        params=(Param('file', 'File', 'string', default=''),),
        outputs=('table',), run=unindex_file, edits=True),
    Operation(
        id='project.verify_data_index', title='Verify Data Index', workspace='report',
        accepts='project',
        summary='Hash every indexed file again: it matches, has changed, or is not found. '
                'Folders to look in, one per line, for a file that has moved.',
        params=(SEARCH,), outputs=('reading', 'table'), run=verify_data_index, edits=True),
    Operation(
        id='project.export_report_template', title='Save Report Template', workspace='report',
        accepts='project',
        summary='Save a report on its own as a template another project can generate from, '
                'in the templates folder when no path is given.',
        params=(REPORT, Param('path', 'Path', 'string', default='')),
        outputs=('reading',), run=export_report_template),
    Operation(
        id='project.generate_from_template', title='Generate from Saved Template',
        workspace='report', accepts='project',
        summary='A report built from a saved template, by its name or a .vdreport path, bound '
                'by role to this project\'s objects.',
        params=(Param('template', 'Template', 'string', default=''),
                Param('name', 'Name', 'string', default='Report')),
        outputs=('reading',), run=generate_from_template),
    Operation(
        id='project.reports_from_runs', title='Reports from Runs', workspace='report',
        accepts='project',
        summary='Write the report each controller run\'s type calls for, beside the run or '
                'into a folder; the open project is not changed. Last left at 0 reads the '
                'whole run.',
        params=(Param('runs', 'Runs', 'string', default=''),
                Param('folder', 'Folder', 'string', default=''),
                Param('kind', 'Report', 'enum', default='auto', options=RUN_KINDS),
                Param('last', 'Last', 'number', default=0.0, unit='s', min=0.0, step=10.0),
                Param('geometry', 'Geometry file', 'string', default=''),
                Param('marking', 'Marking', 'string', default='')),
        outputs=('table',), run=reports_from_runs),
    Operation(
        id='project.attach_run_log', title='Attach Run Log', workspace='report',
        accepts='project',
        summary='Bring one test of a run log into the project, frozen at an entry. A test or '
                'entry left at 0 is the log\'s one test or its last entry.',
        params=(Param('path', 'Run log', 'string', default=''),
                Param('test', 'Test', 'integer', default=0, min=0, step=1),
                Param('upto', 'To entry', 'integer', default=0, min=0, step=1),
                Param('name', 'Name', 'string', default='')),
        outputs=('reading',), run=attach_run_log),
    Operation(
        id='project.refresh_run_log', title='Refresh Run Log', workspace='report',
        accepts='project',
        summary='Take an extract again from its log, to the newest entry or to the entry '
                'given.',
        params=(RUN_LOG, Param('upto', 'To entry', 'integer', default=0, min=0, step=1)),
        outputs=('reading',), run=refresh_run_log, edits=True),
    Operation(
        id='project.detach_run_log', title='Detach Run Log', workspace='report',
        accepts='project',
        summary='Take a run log extract out of the project.',
        params=(RUN_LOG,), outputs=('reading',), run=detach_run_log),
    Operation(
        id='project.index_run_log_files', title='Index Run Log Files', workspace='report',
        accepts='project',
        summary='Add the files a run log names to the data index, each checked against the '
                'checksum the log records.',
        params=(RUN_LOG, SEARCH), outputs=('reading', 'table'), run=index_run_log_files,
        edits=True),
    Operation(
        id='project.report_identity', title='Report Identity', workspace='report',
        accepts='project',
        summary='The report\'s number, revision, laboratory, customer, dates and who '
                'authorized it, carried in its margins while Report Identity is on; what '
                'is still to enter before it is issued.',
        params=(SOURCE, *_identity_params()), outputs=('reading',), run=report_identity,
        edits=True),
    Operation(
        id='project.amend_report', title='Amend Report', workspace='report',
        accepts='project',
        summary='An issued report amended: where the change is, what it said, what it says '
                'now and why, under a new revision.',
        params=(SOURCE, Param('where', 'Where', 'string', default=''),
                Param('was', 'Was', 'string', default=''),
                Param('now', 'Now', 'string', default=''),
                Param('reason', 'Reason', 'string', default=''),
                Param('revision', 'New revision', 'string', default=''),
                Param('number', 'Number of its own (optional)', 'string', default='')),
        outputs=('reading',), run=amend_report, edits=True),
)
