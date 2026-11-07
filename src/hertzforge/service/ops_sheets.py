"""Sheets an author fills in by hand: Visual Dynamics' channel table
(New Channel Table, Add Channel and its typed cells, with the sensor
catalog filling a model's maker and a triax's other legs, and the
calibration dates read against the test's), its specification sheet
(Create Specification and Edit Specification: breakpoints, cross terms
and bands, at a table's control or picked channels or on a
specification), and the analyst's predicted responses a report reads a
measurement against.

Every channel table these show is a `table` view in `core.tables.table_of`
order, edited in place through `project.set_channel_cell`; a breakpoint
table is edited through `project.set_breakpoint`. The cells are written
by the core's own setters (`ChannelTable.set_cell`,
`SpecificationDraft.with_level` and `with_slope`), and a specification
is written by `Project.author_specification`, as the app writes it.
"""

from __future__ import annotations

import re
from typing import Any

import numpy as np

from hertzforge.core.channel_table import ChannelTable, title_of
from hertzforge.core.sensor_catalog import CATALOG

from . import views as v
from .descriptors import Operation, Option, Param
from .ops_project import chosen_records
from .protocol import refused
from .spec_lines import FREQUENCY, NAMES, ROLES, sig, spec_line
from .waterfalls import unit_text

# ---- channel tables ------------------------------------------------------------------

TABLE = Param('table', 'Channel table', 'enum', options_from='objects:channel_table')
ROW = Param('row', 'Row', 'integer', default=1, min=1, step=1)
COLUMN = Param('column', 'Column', 'enum', default='node', options=tuple(
    Option(key, title_of(key)) for key in ChannelTable.SCHEMA))
VALUE = Param('value', 'Value', 'string', default='')
TABLE_NAME = Param('name', 'Name', 'string', default='Channel Table')
COUNT = Param('count', 'Channels to add', 'integer', default=1, min=1, step=1)
MODEL = Param('model', 'Sensor model', 'enum', default=CATALOG[0].model, options=tuple(
    Option(entry.model, f'{entry.model}, {entry.make} {entry.description}')
    for entry in CATALOG))
START = Param('start', 'First test day (YYYY-MM-DD)', 'string', default='')
END = Param('end', 'Last test day (blank: the first)', 'string', default='')


#: how each column's cells are edited, from the schema's own kinds
_CELL_KINDS = {'int': 'number', 'index': 'number', 'float': 'number'}


def cell_widgets() -> tuple[dict[str, str], dict[str, list[str]]]:
    """(kinds, choices) of a channel table's columns for the table view's
    `edit`: each column's kind from `ChannelTable.COLUMNS`, a choice
    column's list, the unit shortlist, and the sensor catalog's models."""
    from hertzforge.core.unit_choices import units_for

    kinds: dict[str, str] = {}
    choices: dict[str, list[str]] = {}
    for key, spec in ChannelTable.COLUMNS.items():
        kinds[key] = _CELL_KINDS.get(spec.kind, spec.kind)
        if spec.kind == 'choice':
            choices[key] = list(spec.choices)
        elif spec.kind == 'unit':
            choices[key] = list(units_for(None))
    choices['model'] = [entry.model for entry in CATALOG]
    return kinds, choices


def channel_table(project: Any, name: str, title: str | None = None,
                  environment: str | None = None) -> dict:
    """A channel table as Visual Dynamics shows it (`core.tables.table_of`):
    its columns, then the direction columns derived from the geometry
    linked with it, each of its own columns edited through
    `project.set_channel_cell`, with VD's widgets (`cell_widgets`). The
    derived columns are read only, and so is Control on a table whose
    control channels are per environment; `environment` names the loop
    whose control channels that column shows."""
    from hertzforge.core.tables import table_of

    table = project[name]
    home = project.geometry_for(name)
    headers, rows = table_of(table, geometry=None if home is None else home[1],
                             environment=environment)
    keys: list[str | None] = [None if key == 'control' and table.environments else key
                              for key in table.column_names]
    keys += [None] * (len(headers) - len(keys))
    kinds, choices = cell_widgets()
    return v.table(title or name, headers, rows,
                   edit=v.editing('project.set_channel_cell', {'table': name}, keys,
                                  choices=choices, kinds=kinds))


def _cell(table: Any, row: int, key: str) -> str:
    return str(table.frame.loc[row, key]).strip()


def _changes(before: Any, table: Any, skip: tuple[int, str] | None) -> list[dict]:
    """A reading row for each cell the edit changed besides the one typed."""
    out = []
    for row in range(min(len(before), table.num_channels)):
        for key in table.column_names:
            if (row, key) == skip:
                continue
            old, new = str(before.loc[row, key]).strip(), _cell(table, row, key)
            if old != new:
                out.append(v.row(f'Channel {table.frame.loc[row, "channel"]}, '
                                 f'{title_of(key)}', f'{new or "blank"} (was {old or "blank"})',
                                 None, 'info'))
    return out


def _catalog_row(model: str) -> dict:
    from hertzforge.core.sensor_catalog import lookup

    entry = lookup(model)
    if entry is None:
        return v.row('Sensor catalog', f'{model} is not in the catalog; typed as it is',
                     None, 'info')
    return v.row('Sensor catalog', f'{entry.model}: {entry.make} {entry.description}, '
                 f'{entry.mass:g} g', None, 'info')


def _rows_of(table: Any) -> dict:
    return v.row('Channels', f'{table.num_channels}', None, 'info')


def new_channel_table(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.new_channel_table`: an empty channel table added to the
    project to fill in by hand (`Project.new_channel_table`), as Visual
    Dynamics' New Channel Table makes one; Add Channel adds its rows."""
    name = project.new_channel_table(params['name'].strip() or 'Channel Table')
    return [channel_table(project, name),
            v.reading(f'{name}: an empty channel table',
                      [v.row('Added', name, None, 'pass'),
                       v.row('Next', 'Add Channel adds a row to type into', None, 'info')])]


def add_channels(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.add_channels`: blank channels added at the end of a channel
    table, numbered on from its highest (`Project.add_channels`), as Visual
    Dynamics' Add Channel adds one; their cells are typed in after."""
    name = params['table']
    try:
        numbers = project.add_channels(name, params['count'])
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    table = project[name]
    said = (f'channel {numbers[0]}' if len(numbers) == 1
            else f'channels {numbers[0]} to {numbers[-1]}')
    return [channel_table(project, name),
            v.reading(f'{name}: added {said}',
                      [v.row('Added', said, None, 'pass'), _rows_of(table)])]


def _set_cell(project: Any, name: str, number: int, key: str, text: str) -> list[dict]:
    table = project[name]
    if not 1 <= number <= table.num_channels:
        raise refused(f'{name} has {table.num_channels} channels; there is no row {number}')
    row = number - 1
    before = table.frame.copy()
    noted = []
    if key == 'role' and project._role_partners(name):
        # linked with the time data it describes, the Role column is that
        # data's roles, and the edit is the data's (Visual Dynamics'
        # `_apply_channel_role`)
        dof, quantity = table.dof_strings()[row], table.types()[row]
        role = text.strip().lower()
        try:
            changed = project.set_channel_role(name, dof, quantity, role)
        except (ValueError, TypeError) as error:
            raise refused(f'{name}: {error}') from None
        noted.append(v.row('Role', f'{dof} ({quantity}) is a {role} on '
                           f'{", ".join(changed)}', None, 'info'))
    else:
        try:
            table.set_cell(key, row, text)
        except (ValueError, KeyError) as error:
            raise refused(str(error).strip('"')) from None
    channel = table.frame.loc[row, 'channel']
    stored = _cell(table, row, key)
    rows = [v.row('Set', f'{title_of(key)} of channel {channel}: {stored or "blank"}', None,
                  'pass')]
    if stored != text.strip() and key != 'role':
        rows.append(v.row('Read as', f'{text.strip()!r} is stored as {stored or "blank"}',
                          None, 'info'))
    if key == 'model' and stored:
        rows.append(_catalog_row(stored))
    rows += noted + _changes(before, table, (row, key))
    return [channel_table(project, name),
            v.reading(f'{name}, channel {channel}', rows)]


def set_channel_cell(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_channel_cell`: one cell of a channel table typed over,
    by row (from 1) and column, through the column's own rule
    (`ChannelTable.set_cell`): a value the column cannot hold is refused in
    the core's words and the cell is unchanged. A model the sensor catalog
    knows fills a blank Make, and a triax's other legs at its node; a
    type clears a unit of another kind; on a table linked with the time
    data it describes, Role is set on both (`Project.set_channel_role`).
    What else the edit changed is read out beside the table."""
    return _set_cell(project, params['table'], params['row'], params['column'],
                     params['value'])


def set_sensor_model(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_sensor_model`: a channel's model picked from the sensor
    catalog (`core.sensor_catalog`), the drop-down of Visual Dynamics'
    Model cell, written as the cell is (`ChannelTable.set_cell`): its
    maker fills a blank Make, and a triax fills its other legs at the
    node."""
    return _set_cell(project, params['table'], params['row'], 'model', params['model'])


def calibration_check(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.calibration_check`: each sensor's calibration expiration
    against the test's first and last day (`core.calibration.span_of` and
    `check`): in date, expired before the test, lapsing during it, or no
    date given, which is never read as in date. The table beside it is the
    channel table, its Expiration cells typed in place."""
    from hertzforge.core.calibration import EXPIRED, IN_DATE, LAPSES, check, span_of

    name = params['table']
    try:
        span = span_of(params['start'].strip() or None, params['end'].strip() or None)
        if span is None:
            raise ValueError('enter the first test day, YYYY-MM-DD')
        result = check(project[name], span)
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    status = {IN_DATE: 'pass', EXPIRED: 'fail', LAPSES: 'warn'}
    head = result.summary()[0]
    rows = [v.row('Test dates', str(span), None, 'info'),
            v.row('Calibration', head, None,
                  'fail' if result.lapsed else 'pass' if result.judged else 'warn')]
    rows += [v.row(found.name[:1].upper() + found.name[1:], found.words, None,
                   status.get(found.status, 'info')) for found in result.channels]
    return [channel_table(project, name), v.reading(f'Calibration of {name}', rows)]


# ---- the specification sheet ----------------------------------------------------------

SOURCE_TABLE = Param('table', 'Channel table', 'enum', options_from='objects:channel_table')
PICKED = Param('rows', 'Rows (blank: the control channels)', 'string', default='')
SPEC_NAME = Param('name', 'Name (blank: the table\'s name and Specification)', 'string',
                  default='')
SPECIFICATION = Param('specification', 'Specification', 'enum',
                      options_from='objects:specification')
CHANNELS = Param('channels', 'Channels (blank: all)', 'string', default='')
BREAKPOINTS = Param('breakpoints', 'Breakpoints: "Hz, level" per line, one level or one per '
                    'channel, SI density (blank: keep)', 'string', default='')
ENTRY = Param('entry', 'Enter', 'enum', default='levels', options=(
    Option('levels', 'Levels: a level at every breakpoint'),
    Option('slopes', 'Slopes: a level at the first, then dB/octave into each')))
PAIRS = Param('pairs', 'Cross terms: "channel, channel, coherence, phase" per line', 'string',
              default='')
ALL_PAIRS = Param('all_pairs', 'Every pair: "coherence, phase" (blank: keep)', 'string',
                  default='')
WARNING = Param('warning', 'Warning band: "below, above" (blank: keep)', 'string',
                default='', unit='dB')
ABORT = Param('abort', 'Abort band: "below, above" (blank: keep)', 'string', default='',
              unit='dB')
_SWITCH = (Option('keep', 'Keep'), Option('on', 'On'), Option('off', 'Off'))
SYMMETRIC = Param('symmetric', 'Bands symmetric', 'enum', default='keep', options=_SWITCH)
UNIFORM = Param('uniform', 'Bands uniform', 'enum', default='keep', options=_SWITCH)
FORM = Param('form', 'Form', 'enum', default='keep', options=(
    Option('keep', 'Keep'), Option('breakpoints', 'Breakpoints'),
    Option('lines', 'Interpolated onto lines')))
SPACING = Param('spacing', 'Line spacing (0: the specification\'s own, or a time history\'s)',
                'number', default=0.0, min=0.0, unit='Hz')
DRAWN = Param('drawn', 'Bands drawn for channel', 'integer', default=1, min=1, step=1)
BREAKPOINT_AT = Param('frequency', 'Frequency', 'number', default=100.0, unit='Hz', min=0.0)
SCALE_DB = Param('decibels', 'Change', 'number', default=0.0, unit='dB', step=1.0)
SHEET = (BREAKPOINTS, ENTRY, PAIRS, ALL_PAIRS, WARNING, ABORT, SYMMETRIC, UNIFORM, FORM,
         SPACING, DRAWN)

#: why the first breakpoint's slope cell takes nothing (Visual Dynamics'
#: `author_panel.NO_SLOPE_IN`)
NO_SLOPE_IN = ('the first breakpoint has no slope into it; type its level, and a slope into '
               'each breakpoint after it')
SLOPE = ' dB/oct'


def _numbers(text: str, what: str) -> list[float]:
    try:
        return [float(t) for t in text.replace(',', ' ').split()]
    except ValueError:
        raise refused(f'{text.strip()!r} is not a list of numbers ({what})') from None


def _lines(text: str) -> list[str]:
    return [line.strip() for line in re.split(r'[\n;]+', text) if line.strip()]


def _two(text: str, what: str, second: float | None = None) -> tuple[float, float]:
    values = _numbers(text, what)
    if len(values) == 1 and second is not None:
        values.append(second)
    if len(values) != 2:
        raise refused(f'{what} is two numbers, such as -3, 3; not {text.strip()!r}')
    return values[0], values[1]


def _typed_breakpoints(draft: Any, text: str, entry: str) -> Any:
    """The draft with its breakpoints typed anew, a line each: a
    frequency then one level for every channel or one per channel; by
    slope, every line after the first a slope in dB per octave into it.
    Each segment keeps the band of the old segment its start lies in."""
    from hertzforge.core.breakpoints import levels_from_slopes

    n = draft.num_channels
    rows = []
    for line in _lines(text):
        values = _numbers(line, 'a breakpoint')
        if len(values) not in (2, n + 1):
            raise refused(f'a breakpoint line is a frequency and one level, or a level for '
                          f'each of the {n} channels; {line!r} is not')
        rows.append(values)
    if entry == 'levels':
        # typed out of order, the sheet re-sorts (Visual Dynamics' `Sheet.sort`)
        rows.sort(key=lambda values: values[0])
    frequencies = [values[0] for values in rows]
    columns = [[values[1] if len(values) == 2 else values[1 + k] for values in rows]
               for k in range(n)]
    if entry == 'slopes':
        columns = [[float(level) for level in levels_from_slopes(frequencies, column[0],
                                                                  column[1:])]
                   for column in columns]
    return draft._copy(frequencies=frequencies, levels=columns, form='breakpoints',
                       bands=draft._bands_on(frequencies))


def _channel_at(draft: Any, text: str) -> int:
    text = text.strip()
    if text in draft.channels:
        return draft.channels.index(text)
    if text.isdigit() and 1 <= int(text) <= draft.num_channels:
        return int(text) - 1
    raise refused(f'the sheet has no channel {text!r}; its channels are '
                  f'{", ".join(draft.channels)}')


def _typed_pairs(draft: Any, text: str) -> Any:
    """The draft with the cross terms typed, a line each: two channels
    (by name, or number from 1), a coherence and a phase in degrees of
    the later channel against the earlier; the two channels alone leave
    the pair unstated again."""
    pairs = dict(draft.pairs)
    for line in _lines(text):
        parts = [p for p in re.split(r'[,\s]+', line) if p]
        if len(parts) not in (2, 3, 4):
            raise refused(f'a cross term is two channels, a coherence and a phase; not '
                          f'{line!r}')
        i, j = _channel_at(draft, parts[0]), _channel_at(draft, parts[1])
        if i == j:
            raise refused(f'{draft.channels[i]} with itself is its autospectrum, not a '
                          'cross term')
        if len(parts) == 2:
            pairs[(min(i, j), max(i, j))] = None
            continue
        values = _numbers(' '.join(parts[2:]), 'a cross term')
        coherence, phase = values[0], values[1] if len(values) == 2 else 0.0
        pairs[(min(i, j), max(i, j))] = (coherence, phase if i < j else -phase)
    return draft._copy(pairs=pairs)


def _spacing_hint(project: Any, name: str) -> float | None:
    """A line spacing when the sheet has none: a time history's averaging,
    one linked with the object first (Visual Dynamics' `_spacing_hint`)."""
    from hertzforge.core.data import TimeHistory

    linked = [n for n in (project.object_group_of(name) or []) if n in project]
    for other in linked + [n for n in project if n not in linked]:
        obj = project[other]
        averaging = getattr(obj, 'averaging', None)
        if isinstance(obj, TimeHistory) and averaging is not None:
            return float(obj.sample_rate) / int(averaging.frame_length)
    return None


def _sheet(project: Any, name: str, draft: Any, params: dict[str, Any]) -> Any:
    """The draft with the sheet's fields applied in the order the panel
    holds them: the band constraints, the breakpoints, the bands, the
    cross terms (every pair, then each one typed), then the form."""
    try:
        constraints = {key: params[key] == 'on' for key in ('symmetric', 'uniform')
                       if params[key] != 'keep'}
        if constraints:
            draft = draft.constrained(**constraints)
        draft = draft.with_entry(params['entry'])
        if params['breakpoints'].strip():
            draft = _typed_breakpoints(draft, params['breakpoints'], params['entry'])
        for kind in ('warning', 'abort'):
            if params[kind].strip():
                draft = draft.with_band(kind, *_two(params[kind], f'the {kind} band'))
        if params['all_pairs'].strip():
            draft = draft.with_all_pairs(*_two(params['all_pairs'], 'every pair', 0.0))
        if params['pairs'].strip():
            draft = _typed_pairs(draft, params['pairs'])
        if params['form'] == 'breakpoints' and draft.form == 'lines':
            draft = draft.to_breakpoints()
        elif params['form'] == 'lines':
            draft = draft.to_lines(params['spacing'] or draft.spacing
                                   or _spacing_hint(project, name))
    except ValueError as error:
        raise refused(str(error)) from None
    return draft


def _autos(spec: Any) -> dict[str, int]:
    from hertzforge.core.data import autospectrum

    return {str(spec.response_dof[k]): k for k in range(spec.num_records)
            if autospectrum(spec, k)}


def _density_unit(dim: str) -> str | None:
    return unit_text(f'{dim}**2/frequency')


def breakpoint_table(name: str, draft: Any, channels: str = '') -> dict:
    """The sheet's breakpoints as Visual Dynamics' grid shows them: a row
    per breakpoint, its frequency and a level per channel in the channel's
    SI density, and by slope the slope into it beside each level; each
    cell edited through `project.set_breakpoint`."""
    by_slope = draft.entry == 'slopes' and draft.form == 'breakpoints'
    headers, keys = ['Hz'], ['frequency']
    for channel, dim in zip(draft.channels, draft.dims, strict=True):
        unit = _density_unit(dim)
        headers.append(f'{channel} ({unit})' if unit else channel)
        keys.append(channel)
        if by_slope:
            headers.append(f'{channel}{SLOPE}')
            keys.append(f'{channel}{SLOPE}')
    slopes = draft.slopes_db() if by_slope else None
    rows = []
    for r, frequency in enumerate(draft.frequencies):
        row = [f'{frequency:g}']
        for k in range(draft.num_channels):
            row.append(f'{draft.levels[k][r]:.6g}')
            if slopes is not None:
                row.append('' if r == 0 else f'{slopes[k][r - 1]:.4g}')
        rows.append(row)
    fixed = {'specification': name, 'entry': draft.entry}
    if channels.strip():
        fixed['channels'] = channels
    title = f'{name}, {"breakpoints" if draft.form == "breakpoints" else "lines"}'
    return v.table(title, headers, rows, edit=v.editing('project.set_breakpoint', fixed, keys))


def _band_handles(name: str, draft: Any, drawn: int, key: str, series: int,
                  channels: str) -> list[dict]:
    """The handles on one band edge, one per run of segments as VD's flat plot has them:
    each at its run's middle on the edge's `series`, and what letting go runs. A sheet at
    lines has no segments to move."""
    if draft.form != 'breakpoints':
        return []
    kind, side = key.split('_')
    edge = draft.band_db(kind, drawn - 1)[0 if side == 'lower' else 1]
    out = []
    for run in draft.band_runs(drawn - 1, kind):
        low, high = float(draft.frequencies[run[0]]), float(draft.frequencies[run[-1] + 1])
        out.append({'series': series, 'x': float(np.sqrt(low * high)) if low > 0 else high / 2.0,
                    'decibels': float(edge[run[0]]), 'label': f'{kind} band {side}',
                    'op': 'project.set_band_edge',
                    'params': {'specification': name, 'channels': channels, 'kind': kind,
                               'side': side, 'segments': ', '.join(map(str, run)),
                               'drawn': drawn}})
    return out


def _spec_views(project: Any, name: str, draft: Any, drawn: int, said: str,
                channels: str = '') -> list[dict]:
    """The specification drawn (each channel's target, and the drawn
    channel's warning and abort limits, as Visual Dynamics draws the
    sheet's preview), its breakpoint table, and a reading of the sheet."""
    from hertzforge.core.author import band_notes

    spec = project[name]
    autos = _autos(spec)
    if drawn > draft.num_channels:
        raise refused(f'the sheet has {draft.num_channels} channels; there is no channel '
                      f'{drawn}')
    banded = draft.channels[drawn - 1]
    lines = []
    handles: list[dict] = []
    for channel in draft.channels:
        r = autos.get(channel)
        if r is None:
            continue
        lines.append(v.shaped(channel, 'reference', spec,
                              *spec_line(spec, r, np.real(spec.ordinate[r]))))
        if channel == banded:
            for key in spec.LIMITS:
                curve = spec.limits.get(key)
                if curve is not None:
                    lines.append(v.shaped(f'{channel}: {NAMES[key]}', ROLES[key], spec,
                                          *spec_line(spec, r, np.real(curve[r]))))
                    handles += _band_handles(name, draft, drawn, key, len(lines) - 1, channels)
    units = {_density_unit(dim) for dim in draft.dims}
    plot = v.spectrum(f'{name}: {draft.num_channels} channel'
                      f'{"s" * (draft.num_channels != 1)}, bands of {banded}', FREQUENCY,
                      v.axis('PSD', units.pop() if len(units) == 1 else None, 'log'), lines,
                      link='frequency', handles=handles)
    rows = [v.row('Specification', said, None, 'pass'),
            v.row('Channels', ', '.join(draft.channels), None, 'info'),
            v.row('Form', f'{len(draft.frequencies)} '
                  + ('breakpoints' if draft.form == 'breakpoints'
                     else f'lines, {draft.spacing:g} Hz apart' if draft.spacing
                     else 'lines'), None, 'info')]
    if draft.form == 'breakpoints':
        for k, (channel, dim) in enumerate(zip(draft.channels, draft.dims, strict=True)):
            rows.append(v.row(f'{channel} rms', sig(draft.reading(k).rms), unit_text(dim),
                              'info'))
    unset = draft.unset_pairs()
    stated = len(draft.pairable()) - len(unset)
    if draft.pairable():
        rows.append(v.row('Cross terms', f'{stated} of {len(draft.pairable())} stated'
                          + ('; an unstated pair is absent from the specification'
                             if unset else ''), None, 'warn' if unset else 'pass'))
    entry = draft.bands[drawn - 1]
    for kind in ('warning', 'abort'):
        low, high = draft.band_db(kind, drawn - 1)
        if len(low):
            rows.append(v.row(f'{kind.title()} band, {banded}',
                              f'{float(np.min(low)):+g} to {float(np.max(high)):+g} dB', None,
                              'info'))
    rows.append(v.row('Bands', ', '.join(word for word, on in (
        ('symmetric', entry['symmetric']), ('uniform', entry['uniform'])) if on) or 'free',
        None, 'info'))
    rows += [v.row('Note', note, None, 'info') for note in [*draft.notes, *band_notes(spec)]]
    return [plot, breakpoint_table(name, draft, channels),
            v.reading(f'{name}, specification sheet', rows)]


def create_specification(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.create_specification`: a specification sheet opened at a
    channel table's control channels (`SpecificationDraft
    .at_control_channels`) or at picked rows whatever their role
    (`at_rows`), filled in, and written beside the table
    (`Project.author_specification`), as Visual Dynamics' Create
    Specification makes it. Left blank, the sheet is the starter: a flat
    line from 20 to 2000 Hz at one SI unit of density, ±3 dB warning and
    ±6 dB abort, no cross term stated."""
    from hertzforge.core.author import SpecificationDraft

    name = params['table']
    table = project[name]
    try:
        if params['rows'].strip():
            draft = SpecificationDraft.at_rows(
                table, chosen_records(params['rows'], table.num_channels))
        else:
            draft = SpecificationDraft.at_control_channels(table)
    except ValueError as error:
        raise refused(f'{name}: {error}') from None
    draft = _sheet(project, name, draft, params)
    try:
        added = project.author_specification(name, draft, name=params['name'].strip() or None)
    except ValueError as error:
        raise refused(str(error)) from None
    where = (f'the picked rows {params["rows"].strip()}' if params['rows'].strip()
             else 'the control channels')
    return _spec_views(project, added, draft, params['drawn'],
                       f'{added}, made at {where} of {name}')


def _opened(project: Any, name: str, channels: str) -> Any:
    from hertzforge.core.author import SpecificationDraft

    picked = [c.strip() for c in channels.replace(';', ',').split(',') if c.strip()] or None
    try:
        return SpecificationDraft.from_specification(project[name], picked)
    except ValueError as error:
        raise refused(f'{name}: {error}') from None


def _write(project: Any, name: str, draft: Any) -> None:
    try:
        project.author_specification(name, draft, replace=True)
    except ValueError as error:
        raise refused(f'{name}: {error}') from None


def _areas_table(name: str, draft: Any, drawn: int) -> dict:
    """VD's Areas and running rms: each segment of the drawn channel's breakpoints, its
    slope, its area, its share of the whole and the rms accumulated to its end."""
    reading = draft.reading(drawn - 1)
    whole = reading.total_area or 1.0
    rows = [[f'{reading.frequencies[k]:g} to {reading.frequencies[k + 1]:g}',
             f'{reading.slopes[k]:.3g}', f'{reading.areas[k]:.4g}',
             f'{100.0 * reading.areas[k] / whole:.1f}', f'{reading.running[k]:.4g}']
            for k in range(len(reading.areas))]
    return v.table(f'{name}, {draft.channels[drawn - 1]}: areas and running rms',
                   ['Hz', 'dB/octave', 'Area', 'Share (%)', 'Running rms'], rows)


def specification_areas(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.specification_areas`: the Areas and running rms of a channel's breakpoints,
    written nowhere (`SpecificationDraft.reading`)."""
    name = params['specification']
    draft = _opened(project, name, params['channels'])
    if not 1 <= params['drawn'] <= draft.num_channels:
        raise refused(f'{name} has {draft.num_channels} channels; not {params["drawn"]}')
    if draft.form != 'breakpoints':
        raise refused(f'{name} is written at lines; areas are read from breakpoints')
    return [_areas_table(name, draft, params['drawn'])]


def add_breakpoint(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.add_breakpoint`: a breakpoint at a frequency, its levels read off the power
    law between its neighbors and the segment's band kept on both halves
    (`SpecificationDraft.with_breakpoint`)."""
    name = params['specification']
    draft = _opened(project, name, params['channels'])
    frequency = float(params['frequency'])
    if frequency in [float(f) for f in draft.frequencies]:
        raise refused(f'{name} already has a breakpoint at {frequency:g} Hz')
    try:
        draft = draft.with_breakpoint(frequency)
    except ValueError as error:
        raise refused(f'{name}: {error}') from None
    _write(project, name, draft)
    return _spec_views(project, name, draft, params['drawn'], f'{name}, a breakpoint added',
                       params['channels'])


def remove_breakpoint(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.remove_breakpoint`: a breakpoint gone, its neighbors joined
    (`SpecificationDraft.without_breakpoint`)."""
    name = params['specification']
    draft = _opened(project, name, params['channels'])
    frequency = float(params['frequency'])
    if frequency not in [float(f) for f in draft.frequencies]:
        raise refused(f'{name} has no breakpoint at {frequency:g} Hz')
    try:
        draft = draft.without_breakpoint(frequency)
    except ValueError as error:
        raise refused(f'{name}: {error}') from None
    _write(project, name, draft)
    return _spec_views(project, name, draft, params['drawn'], f'{name}, a breakpoint removed',
                       params['channels'])


def scale_specification(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.scale_specification`: a specification's levels raised or lowered by decibels,
    on every channel or the ones named (`SpecificationDraft.scaled_db`)."""
    name = params['specification']
    draft = _opened(project, name, params['channels'])
    try:
        draft = draft.scaled_db(params['decibels'])
    except ValueError as error:
        raise refused(f'{name}: {error}') from None
    _write(project, name, draft)
    return _spec_views(project, name, draft, params['drawn'],
                       f'{name}, scaled {params["decibels"]:+g} dB', params['channels'])


def set_band_edge(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_band_edge`: one edge of a band, in the segments named, set to a level in
    decibels from the target (`SpecificationDraft.with_band_edge`), as a drag on VD's flat
    plot lands: it snaps to the grid, and a symmetric or uniform channel follows."""
    name = params['specification']
    draft = _opened(project, name, params['channels'])
    segments = [int(part) for part in params['segments'].replace(' ', '').split(',') if part]
    try:
        draft = draft.with_band_edge(params['kind'], params['side'], segments,
                                     params['decibels'])
    except ValueError as error:
        raise refused(f'{name}: {error}') from None
    _write(project, name, draft)
    return _spec_views(project, name, draft, params['drawn'],
                       f'{name}, {params["kind"]} band {params["side"]} edge set',
                       params['channels'])


def edit_specification(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.edit_specification`: a specification opened as a sheet
    (`SpecificationDraft.from_specification`), on every channel or the
    ones named, the sheet's fields applied, and written back into it
    under its own name (`Project.author_specification` with replace), as
    each edit on Visual Dynamics' Edit Specification sheet lands. Named
    channels are merged back; the others are unchanged."""
    name = params['specification']
    draft = _sheet(project, name, _opened(project, name, params['channels']), params)
    typed = (any(params[key].strip() for key in ('breakpoints', 'pairs', 'all_pairs',
                                                  'warning', 'abort'))
             or any(params[key] != 'keep' for key in ('symmetric', 'uniform', 'form')))
    if not typed:
        # opening the sheet writes nothing; the entry is the sheet's alone
        return _spec_views(project, name, draft, params['drawn'], f'{name}, opened as a sheet',
                           params['channels'])
    _write(project, name, draft)
    return _spec_views(project, name, draft, params['drawn'], f'{name}, written as a sheet',
                       params['channels'])


def set_breakpoint(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_breakpoint`: one cell of a specification's breakpoint
    table typed over, by row (from 1) and column ('frequency', a
    channel's level, or by slope a channel's slope in dB per octave), and
    written into the specification (`Project.author_specification` with
    replace). A level is `SpecificationDraft.with_level` (by slope, every
    slope after it held), a slope `with_slope`. A frequency moves the
    breakpoint and the rows re-sort, as Visual Dynamics' grid does it
    (`author_panel._set_frequency`, computed here: the core has no call
    for it)."""
    from hertzforge.core.breakpoints import levels_from_slopes, segment_slopes

    name = params['specification']
    draft = _opened(project, name, params['channels'])
    try:
        draft = draft.with_entry(params['entry'])
    except ValueError as error:
        raise refused(str(error)) from None
    by_slope = draft.entry == 'slopes' and draft.form == 'breakpoints'
    count = len(draft.frequencies)
    if not 1 <= params['row'] <= count:
        raise refused(f'{name} has {count} breakpoints; there is no row {params["row"]}')
    row, key, text = params['row'] - 1, params['column'].strip(), params['value'].strip()
    try:
        value = float(text)
    except ValueError:
        raise refused(f'{text!r} is not a number') from None
    channel = key.removesuffix(SLOPE)
    try:
        if key == 'frequency':
            if not value > 0:
                raise ValueError('a breakpoint frequency is positive')
            if value in [f for i, f in enumerate(draft.frequencies) if i != row]:
                raise ValueError(f'there is already a breakpoint at {value:g} Hz')
            held = ([segment_slopes(draft.frequencies, levels) for levels in draft.levels]
                    if by_slope else None)
            frequencies = list(draft.frequencies)
            frequencies[row] = value
            order = np.argsort(frequencies, kind='stable')
            moved = list(order) != list(range(count))
            levels = [[levels[k] for k in order] for levels in draft.levels]
            frequencies = [frequencies[k] for k in order]
            if held is not None and not moved:
                levels = [[float(x) for x in levels_from_slopes(frequencies, levels[0], slopes)]
                          for levels, slopes in zip(levels, held, strict=True)]
            draft = draft._copy(frequencies=frequencies, levels=levels)
        elif key in draft.channels:
            if not value > 0:
                raise ValueError(f'{key} needs a positive level at '
                                 f'{draft.frequencies[row]:g} Hz')
            draft = draft.with_level(row, value, [key])
        elif by_slope and key.endswith(SLOPE) and channel in draft.channels:
            if row == 0:
                raise ValueError(NO_SLOPE_IN)
            draft = draft.with_slope(row - 1, value, [channel])
        else:
            columns = ['frequency', *draft.channels]
            if by_slope:
                columns += [f'{c}{SLOPE}' for c in draft.channels]
            raise ValueError(f'the sheet has no column {key!r}; its columns are '
                             f'{", ".join(columns)}')
    except ValueError as error:
        raise refused(str(error)) from None
    _write(project, name, draft)
    return _spec_views(project, name, draft, 1, f'{name}, row {params["row"]} {key} set to '
                       f'{value:g}', params['channels'])


# ---- predictions --------------------------------------------------------------------

PREDICTIONS = Param('name', 'Predictions object', 'string', default='Predictions')
DOF = Param('dof', 'Response DOF', 'string', default='')
QUANTITY = Param('predicted', 'Quantity', 'enum', default='asd', options=(
    Option('asd', 'ASD'), Option('amplitude', 'Amplitude'),
    Option('transmissibility', 'Transmissibility')))
PEAKS = Param('peaks', 'Peaks: "Hz, level, Q, band low, band high" per line (Q and band '
              'optional)', 'string', default='')
UNIT = Param('unit', 'Unit of the quantity (g for g²/Hz; blank for a transmissibility)',
             'string', default='')
LEVEL = Param('level_db', 'Input level predicted at (not for a transmissibility)', 'number',
              default=0.0, unit='dB', step=1.0)
REFERENCE = Param('reference', 'Input DOF (a transmissibility)', 'string', default='')
FROM = Param('source', 'Source: the analysis, document and revision', 'string', default='')
CUSTOMER = Param('customer', 'Customer-supplied', 'boolean', default=False)
FREQUENCY_UNCERTAINTY = Param('frequency_uncertainty', 'Frequency uncertainty (0: unstated)',
                              'number', default=0.0, min=0.0, unit='%', step=1.0)
LEVEL_UNCERTAINTY = Param('level_uncertainty', 'Level uncertainty (0: unstated)', 'number',
                          default=0.0, min=0.0, unit='dB', step=0.5)


def _peaks(text: str) -> list[tuple]:
    out = []
    for line in _lines(text):
        parts = [p.strip() for p in line.split(',')]
        if not 1 <= len(parts) <= 5 or len(parts) == 4:
            raise refused(f'a peak is a frequency, a level, a Q and a band low and high; '
                          f'not {line!r}')
        try:
            numbers = [None if p in ('', '-') else float(p) for p in parts]
        except ValueError:
            raise refused(f'{line!r} is not a list of numbers (a peak)') from None
        frequency, level, q = (numbers + [None, None])[:3]
        band = (numbers[3], numbers[4]) if len(numbers) == 5 else None
        out.append((frequency, level, q, band))
    return out


def prediction_table(name: str, predictions: Any) -> dict:
    """A predictions object as a table: a row per channel predicted."""
    rows = []
    for held in predictions:
        peaks = '; '.join(f'{peak.frequency:g} Hz'
                          + (f' {peak.level:.3g}' if peak.level is not None else '')
                          + (f' Q {peak.q:.3g}' if peak.q is not None else '')
                          for peak in held.peaks)
        bands = '; '.join(f'{low:.4g}-{high:.4g}' for low, high in held.bands)
        uncertainty = ', '.join(part for part in (
            f'±{held.frequency_uncertainty:g}%' if held.frequency_uncertainty else '',
            f'±{held.level_uncertainty:g} dB' if held.level_uncertainty else '') if part)
        rows.append([held.channel, held.quantity, held.unit_label,
                     '' if held.level_db is None else f'{held.level_db:+g}',
                     held.reference or '', peaks, bands, uncertainty,
                     held.source + (' (customer-supplied)' if held.customer else '')])
    return v.table(name, ['Channel', 'Quantity', 'Unit', 'Input level (dB)', 'Reference',
                          'Peaks', 'Bands (Hz)', 'Uncertainty', 'Source'], rows)


def predict(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.predict`: one response channel's predicted response, the
    analyst's peaks, entered into a predictions object, made when the
    project has none of that name (`Project.predict`); a prediction held
    for the channel and quantity is replaced. A report reads a measurement
    against these (Responses Against Prediction)."""
    transmissibility = params['predicted'] == 'transmissibility'
    name = params['name'].strip() or 'Predictions'
    before = list(project.get(name) or [])
    try:
        name = project.predict(
            name, params['dof'], params['predicted'], _peaks(params['peaks']),
            unit=params['unit'].strip() or None,
            level_db=None if transmissibility else params['level_db'],
            reference=params['reference'].strip() or None, source=params['source'].strip(),
            customer=params['customer'],
            frequency_uncertainty=params['frequency_uncertainty'] or None,
            level_uncertainty=params['level_uncertainty'] or None)
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    held = project[name]
    entered = next(p for p in held if all(p is not q for q in before))
    rows = [v.row('Entered', entered.describe(), None, 'pass'),
            v.row('Channels predicted', f'{len(held)}', None, 'info')]
    return [prediction_table(name, held), v.reading(f'Prediction in {name}', rows)]


OPERATIONS = (
    Operation(
        id='project.new_channel_table', title='New Channel Table', workspace='geometry',
        summary='An empty channel table, to fill in by hand: Add Channel adds its rows.',
        params=(TABLE_NAME,), outputs=('table', 'reading'), run=new_channel_table),
    Operation(
        id='project.add_channels', title='Add Channel', workspace='geometry',
        summary='Blank channels added at the end of a channel table, numbered on from its '
                'highest, to type into.',
        params=(TABLE, COUNT), outputs=('table', 'reading'), run=add_channels, edits=True),
    Operation(
        id='project.set_channel_cell', title='Set Channel Cell', workspace='geometry', everywhere=True,
        summary='One cell of a channel table typed over, by row and column; a value the '
                'column cannot hold is refused and the cell is unchanged. The tables edit '
                'through it.',
        params=(TABLE, ROW, COLUMN, VALUE), outputs=('table', 'reading'),
        run=set_channel_cell, edits=True),
    Operation(
        id='project.set_sensor_model', title='Sensor Model', workspace='geometry',
        summary='A channel\'s model picked from the sensor catalog: its maker fills a blank '
                'Make, and a triax fills its other legs at the node.',
        params=(TABLE, ROW, MODEL), outputs=('table', 'reading'), run=set_sensor_model,
        edits=True),
    Operation(
        id='project.calibration_check', title='Calibration Dates', workspace='geometry',
        summary='Each sensor\'s calibration expiration against the test\'s first and last '
                'day: in date, expired, lapsing during the test, or no date given.',
        params=(TABLE, START, END), outputs=('table', 'reading'), run=calibration_check),
    Operation(
        id='project.create_specification', title='Create Specification', workspace='specify',
        summary='A specification sheet at a channel table\'s control channels or picked rows, '
                'written beside the table: breakpoints, cross terms and bands. Left blank, a '
                'flat starter from 20 to 2000 Hz with ±3 and ±6 dB bands.',
        params=(SOURCE_TABLE, PICKED, SPEC_NAME, *SHEET),
        outputs=('spectrum', 'table', 'reading'), run=create_specification),
    Operation(
        id='project.edit_specification', title='Edit Specification', workspace='specify',
        summary='A specification opened as a sheet, on every channel or the ones named, its '
                'breakpoints, cross terms, bands or form changed, and written back in place.',
        params=(SPECIFICATION, CHANNELS, *SHEET), outputs=('spectrum', 'table', 'reading'),
        run=edit_specification, edits=True),
    Operation(
        id='project.set_breakpoint', title='Set Breakpoint', workspace='specify',
        summary='One cell of a specification\'s breakpoint table typed over, by row and '
                'column, and written into it. The breakpoint tables edit through it.',
        params=(SPECIFICATION, ROW, Param('column', 'Column', 'string', default='frequency'),
                VALUE, ENTRY, CHANNELS),
        outputs=('spectrum', 'table', 'reading'), run=set_breakpoint, edits=True),
    Operation(
        id='project.specification_areas', title='Areas and Running RMS', workspace='specify',
        summary='Each segment of a channel\'s breakpoints: its slope, its area, its share of '
                'the whole and the rms accumulated to its end.',
        params=(SPECIFICATION, CHANNELS, DRAWN), outputs=('table',), run=specification_areas),
    Operation(
        id='project.add_breakpoint', title='Add Breakpoint', workspace='specify',
        summary='A breakpoint at a frequency, its levels read off the power law between its '
                'neighbors.',
        params=(SPECIFICATION, BREAKPOINT_AT, CHANNELS, DRAWN),
        outputs=('spectrum', 'table', 'reading'), run=add_breakpoint, edits=True),
    Operation(
        id='project.remove_breakpoint', title='Remove Breakpoint', workspace='specify',
        summary='A breakpoint gone, its neighbors joined.',
        params=(SPECIFICATION, BREAKPOINT_AT, CHANNELS, DRAWN),
        outputs=('spectrum', 'table', 'reading'), run=remove_breakpoint, edits=True),
    Operation(
        id='project.scale_specification', title='Scale Specification', workspace='specify',
        summary='The levels raised or lowered by decibels, on every channel or the ones named.',
        params=(SPECIFICATION, SCALE_DB, CHANNELS, DRAWN),
        outputs=('spectrum', 'table', 'reading'), run=scale_specification, edits=True),
    Operation(
        id='project.set_band_edge', title='Set Band Edge', workspace='specify',
        summary='One edge of a warning or abort band set to a level in decibels from the '
                'target. Dragging the band on the plot runs it.',
        params=(SPECIFICATION, Param('kind', 'Band', 'enum', default='abort', options=(
            Option('warning', 'Warning'), Option('abort', 'Abort'))),
            Param('side', 'Edge', 'enum', default='upper', options=(
                Option('lower', 'Lower'), Option('upper', 'Upper'))),
            Param('segments', 'Segments, as "0, 1"', 'string', default='0'),
            Param('decibels', 'Level from the target', 'number', default=6.0, unit='dB'),
            CHANNELS, DRAWN),
        outputs=('spectrum', 'table', 'reading'), run=set_band_edge, edits=True),
    Operation(
        id='project.predict', title='Enter Prediction', workspace='report',
        summary='One response channel\'s predicted peaks entered into a predictions object, '
                'which a report reads a measurement against.',
        params=(PREDICTIONS, DOF, QUANTITY, PEAKS, UNIT, LEVEL, REFERENCE, FROM, CUSTOMER,
                FREQUENCY_UNCERTAINTY, LEVEL_UNCERTAINTY),
        outputs=('table', 'reading'), run=predict, edits=True),
)
