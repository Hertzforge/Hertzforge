"""Channel tables and breakpoint tables as files, in four forms.

Two kinds of table are kept here. A `channels` table is the channel table
(`core.channel_table.ChannelTable`): one table that holds both what the
analysis reads (the DOFs, the sensors, their calibration) and the settings a
controller runs each channel at, so a test's channel table is the same object
its analysis is done with. A `breakpoints` table is one of the breakpoint
tables behind a test's profile, target or limits (its `table_kind`, such as
`random_profile`), with the units of its x and y columns. Both are read and
written in four forms, chosen by the path's suffix:

- `.hforge`: a single-object HDF5 file. A channel table is the channel
  table's own `channel_table/` group (`io.native`); a breakpoint table is a
  `breakpoint_table/` group laid out the same way, a `column_order`
  attribute and one dataset per column;
- `.csv`: comma-separated text;
- `.tsv` and `.txt`: tab-delimited text;
- `.xlsx`: an Excel workbook, the table on its first sheet. A channel table
  is the workbook `io.excel` writes and reads.

A channel table's rows are read as every other source of one is: a heading
is matched to a column by `canonical_name`, with the channel table's own
aliases, a column the table does not have is left out, and a value that
cannot be read takes the column's default. A table read here is for a test,
so one without the controller's settings is converted to carry them
(`ChannelTable.add_controller_settings`). What was left out, could not be
read or was added is reported in `warnings`, so a person sees it. A channel
table written here is written as it is given: with the controller's columns
when they are among its columns, and as the measurement's table alone when
they are not.

A breakpoint table is read leniently too: a column is found by its name or a
common alias, in any case and spacing, with any unit given in the heading
(`Frequency (Hz)`); comment lines (`#` or `//`) and blank rows are skipped,
and a column that is not one of the table's is reported and left out. A
cell that cannot be read is refused with its row and column.

Writing is strict for both: the columns are the kind's own names, every
value of the column's type, and a table that is not well formed is refused
before anything is written. The text and workbook forms of a breakpoint
table start with comment lines that say which table the file holds
(`# Hertzforge table: breakpoint_table` and its `table_kind`), so a file of
another kind is refused when it is opened.
"""

from __future__ import annotations

import contextlib
import csv
import io
import math
import os
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any

#: the two kinds of table, by the name a request gives
KINDS = ('channels', 'breakpoints')

#: the root group, and the comment line's word, for each kind
OBJECTS = {'channels': 'channel_table', 'breakpoints': 'breakpoint_table'}

#: what each kind is called in a message
KIND_WORDS = {'channels': 'a channel table', 'breakpoints': 'a breakpoint table'}

#: the form each suffix is read and written in
FORMS = {'.hforge': 'hforge', '.csv': 'csv', '.tsv': 'tsv', '.txt': 'tsv', '.xlsx': 'xlsx'}

#: the suffix a path with none is given
DEFAULT_SUFFIX = '.hforge'


class TableFileError(ValueError):
    """A table file that cannot be read or written, said in words."""


class TableContentError(ValueError):
    """A table given to be written that is not well formed, said in words."""


@dataclass(frozen=True)
class Column:
    """One column of a kind of breakpoint table.

    `type` is `number`, `choice` or `text` (a Mixed Mode part's id, name,
    the part it follows, or its amplitude profile's rows). `choices` maps each word a file may
    hold, in normalized form, to the value it stands for; `values` are the
    values themselves, which a write must use.
    """

    name: str
    label: str
    type: str
    default: Any = None
    nullable: bool = False
    aliases: tuple[str, ...] = ()
    values: tuple[str, ...] = ()
    choices: Mapping[str, str] = field(default_factory=dict)


@dataclass(frozen=True)
class BreakpointKind:
    """A kind of breakpoint table: its columns, which is the x column, the
    units it is in unless a file says otherwise, the columns of which at
    least one must be present, and the order a file with no heading row is
    read in."""

    label: str
    columns: tuple[Column, ...]
    x: str
    x_unit: str
    y_unit: str
    needs: tuple[str, ...]
    positional: tuple[str, ...] = ()


@dataclass
class Table:
    """A table as read or to be written: its kind, the columns by name, the
    rows (one value per column, None where a cell holds nothing), and for a
    breakpoint table its `table_kind` and units. `warnings` are what a read
    left out or assumed."""

    kind: str
    columns: list[str]
    rows: list[list[Any]]
    table_kind: str | None = None
    x_unit: str | None = None
    y_unit: str | None = None
    warnings: list[str] = field(default_factory=list)


def normalize(text: str) -> str:
    """A heading or a word as a breakpoint table matches it: lower case,
    with any unit in parentheses or brackets dropped, and spaces,
    underscores, hyphens and the characters `#./^,:²` taken out."""
    base = text.split('(')[0].split('[')[0]
    return ''.join(c for c in base.lower() if c not in ' \t_-#./^,:²')


def _words(*words: str) -> dict[str, str]:
    """{normalized word: value}: each argument is 'value: word word ...'."""
    out: dict[str, str] = {}
    for spec in words:
        value, _, said = spec.partition(':')
        for word in (value, *said.split()):
            out[normalize(word)] = value
    return out


#: what a nullable number's cell may hold to say it has none
NONE_WORDS = {'', 'none', 'null', 'nan', 'off', 'na', 'n/a', '-'}

FREQUENCY = Column('freq_hz', 'Frequency', 'number', aliases=(
    'freq', 'frequency', 'hz', 'f', 'freqhz'))

#: a Sine table's row quantity: what its value is, in that quantity's own unit
QUANTITY = Column('quantity', 'Quantity', 'choice', 'accel_g', aliases=(
    'quantity', 'kind', 'type', 'measure'),
    values=('accel_g', 'velocity_ips', 'disp_pp_in'),
    choices=_words('accel_g: accel acceleration g a gpk gpeak',
                   'velocity_ips: velocity vel v ips in/s',
                   'disp_pp_in: displacement disp d inpp pp disppp'))


def _bands() -> tuple[Column, ...]:
    """The four band columns a profile's rows may carry, dB below and above
    the reference."""
    return tuple(
        Column(f'{band}_{side}_db', f'{band.title()} {side}', 'number', None, nullable=True,
               aliases=tuple(f'{band}{word}' for word in words))
        for band in ('alarm', 'abort')
        for side, words in (('below', ('below', 'minus', 'lo', 'low', 'belowdb')),
                            ('above', ('above', 'plus', 'hi', 'high', 'abovedb'))))


BREAKPOINT_KINDS: dict[str, BreakpointKind] = {
    'random_profile': BreakpointKind(
        'Random profile',
        (FREQUENCY,
         Column('psd_g2_hz', 'PSD', 'number', None, nullable=True, aliases=(
             'psd', 'level', 'g2', 'g2hz', 'asd', 'accel', 'acceleration', 'ghz', 'psdg2hz')),
         Column('slope_db_oct', 'Slope', 'number', None, nullable=True, aliases=(
             'slope', 'dboct', 'db', 'oct', 'dbperoct', 'slopedboct')),
         *_bands()),
        x='freq_hz', x_unit='Hz', y_unit='g²/Hz', needs=('psd_g2_hz', 'slope_db_oct'),
        positional=('freq_hz', 'psd_g2_hz', 'slope_db_oct')),
    'sine_profile': BreakpointKind(
        'Sine profile',
        (FREQUENCY, QUANTITY,
         Column('value', 'Value', 'number', aliases=('value', 'amplitude', 'level', 'amp')),
         *_bands()),
        x='freq_hz', x_unit='Hz', y_unit='', needs=('value',),
        positional=('freq_hz', 'value')),
    'sine_limit': BreakpointKind(
        'Sine limit',
        (FREQUENCY, QUANTITY,
         Column('bound', 'Bound', 'number', aliases=(
             'bound', 'value', 'limit', 'ceiling', 'level', 'amplitude', 'amp'))),
        x='freq_hz', x_unit='Hz', y_unit='', needs=('bound',),
        positional=('freq_hz', 'bound')),
    'srs_target': BreakpointKind(
        'SRS target',
        (FREQUENCY,
         Column('accel_g', 'Acceleration', 'number', aliases=(
             'accel', 'acceleration', 'accelg', 'g', 'srs', 'level', 'peak', 'amplitude')),),
        x='freq_hz', x_unit='Hz', y_unit='g', needs=('accel_g',),
        positional=('freq_hz', 'accel_g')),
    'limit': BreakpointKind(
        'Limit',
        (FREQUENCY,
         Column('ceiling', 'Ceiling', 'number', aliases=(
             'ceiling', 'limit', 'level', 'max', 'psd', 'value')),),
        x='freq_hz', x_unit='Hz', y_unit='', needs=('ceiling',),
        positional=('freq_hz', 'ceiling')),
    'drive_gain': BreakpointKind(
        'Drive gain',
        (FREQUENCY,
         Column('gain', 'Gain', 'number', aliases=(
             'gain', 'mag', 'magnitude', 'frf', 'h', 'gv', 'gpv')),),
        x='freq_hz', x_unit='Hz', y_unit='g/V', needs=('gain',),
        positional=('freq_hz', 'gain')),
}


def _num(name: str, label: str, *aliases: str) -> Column:
    return Column(name, label, 'number', None, nullable=True, aliases=aliases)


def _choice(name: str, label: str, default: str | None, *words: str,
            aliases: tuple[str, ...] = ()) -> Column:
    values = tuple(w.partition(':')[0] for w in words)
    return Column(name, label, 'choice', default, nullable=default is None, aliases=aliases,
                  values=values, choices=_words(*words))


def _txt(name: str, label: str, *aliases: str) -> Column:
    return Column(name, label, 'text', None, nullable=True, aliases=aliases)


#: the columns a Mixed Mode tone and a narrowband share: who the part is, when
#: it plays, how it moves (each kind of schedule's fields, empty where the
#: part's kind does not use them) and its bands
_PART_COLUMNS = (
    _txt('id', 'Id', 'partid', 'part'),
    _txt('name', 'Name', 'label'),
    _choice('enabled', 'On', 'on', 'on: yes true 1 enabled', 'off: no false 0 disabled',
            aliases=('enabled', 'starts on')),
    _choice('schedule', 'Moves', None, 'fixed: standsstill dwell dwells', 'sweep: sweeps swept',
            'follow: follows follower', aliases=('schedule', 'kind', 'motion')),
    _num('freq_hz', 'Frequency', 'freq', 'frequency', 'hz'),
    _num('dwell_s', 'Stands for', 'dwells', 'dwell seconds'),
    _num('dwell_cycles', 'Stands for cycles', 'cycles'),
    _num('lo_hz', 'From', 'low', 'lo', 'from hz'),
    _num('hi_hz', 'To', 'high', 'hi', 'to hz'),
    _choice('rate_law', 'Sweep law', None, 'log: logarithmic octaves', 'linear: lin hz',
            aliases=('law', 'sweep law')),
    _num('rate', 'Rate', 'sweep rate'),
    _num('start_hz', 'Starts at', 'start'),
    _choice('direction', 'First goes', None, 'up: rising', 'down: falling',
            aliases=('direction', 'first leg')),
    _num('end_sweeps', 'Sweeps', 'sweeps', 'count'),
    _num('end_s', 'Sweeps for', 'duration', 'duration s'),
    _txt('follows', 'Follows', 'of', 'leader'),
    _num('ratio', 'Ratio', 'times'),
    *_bands(),
    _num('on_s', 'On at', 'window on', 'on s'),
    _num('off_s', 'Off at', 'window off', 'off s'),
)

BREAKPOINT_KINDS['mixed_tones'] = BreakpointKind(
    'Tones table',
    (*_PART_COLUMNS,
     _num('amplitude_g', 'Amplitude', 'amplitude', 'amp', 'g', 'gpk'),
     _txt('profile', 'Amplitude profile', 'amplitude profile', 'profile rows')),
    x='id', x_unit='', y_unit='', needs=('schedule',))

BREAKPOINT_KINDS['mixed_bands'] = BreakpointKind(
    'Narrowbands table',
    (*_PART_COLUMNS,
     _num('level_g2_hz', 'Level', 'level', 'psd', 'g2hz'),
     _num('width_hz', 'Width', 'width'),
     _num('width_share', 'Width share', 'share', 'width share of center')),
    x='id', x_unit='', y_unit='', needs=('schedule',))


def _breakpoint_kind(table_kind: str | None) -> BreakpointKind:
    if table_kind not in BREAKPOINT_KINDS:
        raise TableContentError(
            f'{table_kind!r} is not a kind of breakpoint table; the kinds are '
            f'{", ".join(BREAKPOINT_KINDS)}')
    return BREAKPOINT_KINDS[table_kind]


def _kind_word(kind: str, table_kind: str | None) -> str:
    if kind == 'breakpoints' and table_kind in BREAKPOINT_KINDS:
        label = BREAKPOINT_KINDS[table_kind].label
        # an initialism keeps its capitals and takes 'an' (an SRS target)
        if label.split()[0].isupper():
            return f'an {label}'
        return f'a {label[0].lower()}{label[1:]}'
    if kind == 'breakpoints' and table_kind:
        return f'a {table_kind.replace("_", " ")} table'
    return KIND_WORDS[kind]


def form_of(path: str) -> str:
    """The form a path is read or written in, by its suffix."""
    suffix = os.path.splitext(path)[1].lower()
    if suffix not in FORMS:
        names = list(FORMS)
        raise TableFileError(f'{os.path.basename(path)}: a table file is '
                             f'{", ".join(names[:-1])} or {names[-1]}')
    return FORMS[suffix]


def table_kinds() -> dict[str, str]:
    """{table_kind: label}: the kinds of breakpoint table there are."""
    return {key: kind.label for key, kind in BREAKPOINT_KINDS.items()}


# ---- writing ------------------------------------------------------------------------------


def write(path: str | os.PathLike, table: Table) -> str:
    """Write `table` to `path` in the form its suffix names, after checking
    it is well formed; a path with no suffix is given `.hforge`.

    Parameters
    ----------
    path : str or os.PathLike
        Where to write; the file is replaced in one step once written.
    table : Table
        The table: its kind, its columns by name, and its rows.

    Returns
    -------
    str
        The path written, absolute.
    """
    path = os.path.abspath(os.path.expanduser(str(path)))
    if not os.path.splitext(path)[1]:
        path += DEFAULT_SUFFIX
    form = form_of(path)
    if table.kind not in KINDS:
        raise TableContentError(f'the kind of table is one of {", ".join(KINDS)}, '
                                f'not {table.kind!r}')
    try:
        if table.kind == 'channels':
            _write_channels(path, form, channel_table(table))
            return path
        checked = _checked(table)
        writer = {'hforge': _write_hforge, 'csv': _write_delimited, 'tsv': _write_delimited,
                  'xlsx': _write_xlsx}[form]
        with _replaced(path) as partial:
            writer(partial, checked, form)
    except OSError as error:
        raise TableFileError(f'{path} could not be written: '
                             f'{error.strerror or error}') from None
    return path


@contextlib.contextmanager
def _replaced(path: str) -> Iterator[str]:
    """A path beside `path` to write to; on success it replaces `path` in
    one rename, and on failure it is removed and `path` is left as it was."""
    folder, name = os.path.split(path)
    partial = os.path.join(folder, f'.{name}.{os.getpid()}.partial')
    try:
        yield partial
        os.replace(partial, path)
    finally:
        if os.path.exists(partial):
            os.remove(partial)


def _text(value: Any) -> str:
    """A value as a text cell: '' for none, true or false, and a number in
    the fewest digits that read back as the same number."""
    if value is None:
        return ''
    if isinstance(value, bool):
        return 'true' if value else 'false'
    if isinstance(value, float):
        return repr(value).removesuffix('.0')
    return str(value)


# ---- channel tables -----------------------------------------------------------------------


def channel_table(table: Table) -> Any:
    """A `ChannelTable` of the rows given, each value checked as the column
    holds it.

    The columns are the channel table's own names (`ChannelTable.SCHEMA`).
    `channel` is required; a column not given takes its default in every
    row. A value is JSON's: a whole number for `channel`, a whole number or
    None for a blank-able number (`node`, `control_group`), a number or
    None for a measurement (`sensitivity`, the ranges, the abort limits),
    true or false for a flag, and text, '' for blank, for the rest.

    Raises TableContentError, naming the row and column, for anything the
    column cannot hold.
    """
    from ..core.channel_table import ChannelTable, title_of

    if table.table_kind or table.x_unit is not None or table.y_unit is not None:
        raise TableContentError('a channel table has no table_kind or units')
    names = list(table.columns)
    for name in names:
        if name not in ChannelTable.COLUMNS:
            raise TableContentError(
                f'{name!r} is not a column of a channel table; its columns are '
                f'{", ".join(ChannelTable.SCHEMA)}')
    if len(set(names)) != len(names):
        raise TableContentError('a column is named twice')
    if 'channel' not in names:
        raise TableContentError("a channel table has a Channel column ('channel')")
    columns: dict[str, list[Any]] = {name: [] for name in names}
    for at, row in enumerate(table.rows, start=1):
        if not isinstance(row, Sequence) or isinstance(row, str) or len(row) != len(names):
            raise TableContentError(f'row {at} has {_count(row)}, not one for each of the '
                                    f'{len(names)} columns')
        for name, value in zip(names, row):
            where = f'row {at}, {title_of(name)}'
            text = _channel_text(name, value, where)
            try:
                ChannelTable._coerce(name, text)
            except (ValueError, TypeError) as error:
                raise TableContentError(f'{where}: {error}') from None
            columns[name].append(int(text) if name == 'channel' else text)
    seen: set[int] = set()
    for number in columns['channel']:
        if number in seen:
            raise TableContentError(f'channel {number} is in the table twice; a channel '
                                    'number is the join key and is used once')
        seen.add(number)
    for core in ChannelTable.CORE:
        columns.setdefault(core, [''] * len(table.rows))
    return ChannelTable(columns)


def _channel_text(name: str, value: Any, where: str) -> str:
    """A JSON value as the channel table's text for column `name`."""
    from ..core.channel_table import ChannelTable

    kind = ChannelTable.COLUMNS[name].kind
    if value is None and kind != 'int':
        return ''
    if kind == 'flag':
        if isinstance(value, bool):
            return str(value)
        raise TableContentError(f'{where}: {value!r} is not true or false')
    if kind in ('int', 'index', 'float') and (
            isinstance(value, bool) or not isinstance(value, (int, float))):
        what = 'a number' if kind == 'float' else 'a whole number'
        raise TableContentError(f'{where}: {value!r} is not {what}')
    if kind in ('int', 'index'):
        if isinstance(value, float) and not value.is_integer():
            raise TableContentError(f'{where}: {value!r} is not a whole number')
        return str(int(value))
    if kind == 'float':
        if not math.isfinite(value):
            raise TableContentError(f'{where}: {value!r} is not a finite number')
        return _text(float(value))
    if not isinstance(value, str):
        raise TableContentError(f'{where}: {value!r} is not text')
    return value


def channel_rows(table: Any) -> tuple[list[str], list[list[Any]]]:
    """A `ChannelTable`'s columns and its rows as JSON values: each column
    as `channel_table` takes it."""
    from ..core.channel_table import ChannelTable

    names = table.column_names
    kinds = [ChannelTable.COLUMNS[name].kind for name in names]
    cells = [table[name] for name in names]
    rows = []
    for r in range(table.num_channels):
        row: list[Any] = []
        for kind, column in zip(kinds, cells):
            text = str(column[r])
            if kind == 'int':
                row.append(int(column[r]))
            elif kind == 'index':
                row.append(int(text) if text else None)
            elif kind == 'float':
                row.append(float(text) if text else None)
            elif kind == 'flag':
                row.append(text == 'True')
            else:
                row.append(text)
        rows.append(row)
    return list(names), rows


def _write_channels(path: str, form: str, table: Any) -> None:
    from . import excel, native

    if form == 'hforge':
        native.save(table, path)
        return
    with _replaced(path) as partial:
        if form == 'xlsx':
            excel.save(table, partial)
            return
        from ..core.channel_table import title_of

        names = table.column_names
        with open(partial, 'w', encoding='utf-8', newline='') as out:
            out.write(f'# Hertzforge table: {OBJECTS["channels"]}\r\n')
            writer = csv.writer(out, delimiter=',' if form == 'csv' else '\t',
                                lineterminator='\r\n')
            writer.writerow([title_of(name) for name in names])
            for r in range(table.num_channels):
                writer.writerow([_text(excel._cell(table, name, r)) for name in names])


def _read_channels(path: str, form: str, name: str) -> Table:
    from ..core.channel_table import ChannelTable
    from . import excel, native

    if form == 'hforge':
        order = _hforge_holding(path, name, 'channels')
        return _channel_result(native.load(path), [_left_out(key) for key in order
                                                   if key not in ChannelTable.COLUMNS], name)
    if form == 'xlsx':
        try:
            columns = excel.read_columns(path)
        except ValueError as error:
            if _xlsx_said(path).get('hertzforgetable') == OBJECTS['breakpoints']:
                raise TableFileError(f'{name} holds a breakpoint table, not a channel '
                                     'table') from None
            raise TableFileError(str(error)) from None
        except Exception as error:      # noqa: BLE001 - openpyxl's refusals vary by fault
            raise TableFileError(f'{name} is not a readable Excel workbook '
                                 f'({type(error).__name__})') from None
    else:
        columns = _delimited_channel_columns(path, form, name)
    warnings = _channel_warnings(columns, name)
    return _channel_result(ChannelTable(columns), warnings, name)


def _channel_result(table: Any, warnings: list[str], name: str) -> Table:
    """The table read, converted to carry the controller's settings when it
    does not (`ChannelTable.add_controller_settings`): its columns and rows
    as JSON values, with what was left out or added named."""
    if not table.has_controller_settings:
        table.add_controller_settings()
        added = (f'{name} had no controller settings, so each channel was given them at '
                 'their defaults: on, unbound and without abort limits.')
        warnings = [*warnings, added]
    names, rows = channel_rows(table)
    if not rows:
        warnings = [*warnings, f'{name} has no rows.']
    return Table('channels', names, rows, warnings=warnings)


def _left_out(key: str) -> str:
    return f'Left out the column {key!r}: it is not a column of a channel table.'


def _channel_warnings(columns: Mapping[str, list[str]], name: str) -> list[str]:
    """What reading `columns` into a channel table leaves out: the columns
    it does not have, and each cell it cannot read, which takes its
    column's default. A channel number that cannot be read is refused."""
    from ..core.channel_table import ChannelTable, title_of

    warnings = [_left_out(key) for key in columns if key and key not in ChannelTable.COLUMNS]
    numbers = list(columns.get('channel', []))
    for raw in numbers:
        try:
            ChannelTable._coerce('channel', raw)
        except (ValueError, TypeError):
            raise TableFileError(f'{name}: {raw!r} in the Channel column is not a channel '
                                 'number') from None
    for key, values in columns.items():
        if key not in ChannelTable.COLUMNS or key == 'channel':
            continue
        for number, raw in zip(numbers, values):
            text = '' if raw is None else str(raw).strip()
            if not text:
                continue
            try:
                ChannelTable._coerce(key, text)
            except (ValueError, TypeError):
                default = ChannelTable.COLUMNS[key].default
                warnings.append(f'Channel {number}, {title_of(key)}: {text!r} could not be '
                                f'read; it was left {default or "blank"}.')
    return warnings


def _delimited_channel_columns(path: str, form: str, name: str) -> dict[str, list[str]]:
    """A text file's channel table as `io.excel.read_columns` reads a
    workbook's: the header is the first of the top rows naming a channel
    column, each heading read by `canonical_name`, a column named twice
    keeping the first, and every value text."""
    from . import excel

    grid = _delimited_rows(path, form)
    if grid.said.get('hertzforgetable') == OBJECTS['breakpoints']:
        raise TableFileError(f'{name} holds a breakpoint table, not a channel table')
    header = None
    for at, (_line, cells) in enumerate(grid.rows[:excel.HEADER_SEARCH_ROWS]):
        keys = [excel._key(cell) for cell in cells]
        if any(key in excel.HEADER_MARKERS for key in keys):
            header = at, keys
            break
    if header is None:
        raise TableFileError(f'{name} has no channel table in it: no row in the first '
                             f'{excel.HEADER_SEARCH_ROWS} names a channel column')
    at, keys = header
    seen: set[str] = set()
    wanted = []
    for i, key in enumerate(keys):
        if key and key not in seen:
            seen.add(key)
            wanted.append((i, key))
    columns: dict[str, list[str]] = {key: [] for _i, key in wanted}
    for _line, cells in grid.rows[at + 1:]:
        values = [cells[i] if i < len(cells) else '' for i, _k in wanted]
        if all(str(v).strip() == '' for v in values):
            continue
        for (_i, key), value in zip(wanted, values):
            columns[key].append(str(value).strip())
    for core in ('channel', 'node', 'direction', 'unit'):
        columns.setdefault(core, [''] * len(columns.get('channel', [])))
    return columns


def _count(row: Any) -> str:
    if isinstance(row, Sequence) and not isinstance(row, str):
        return f'{len(row)} value{"" if len(row) == 1 else "s"}'
    return 'no list of values'


# ---- breakpoint tables --------------------------------------------------------------------


def _checked(table: Table) -> Table:
    """A breakpoint table with its values checked against its columns'
    types, and its units filled from its kind where it gives none."""
    kind = _breakpoint_kind(table.table_kind)
    columns = {c.name: c for c in kind.columns}
    names = list(table.columns)
    for name in names:
        if name not in columns:
            raise TableContentError(
                f'{name!r} is not a column of {_kind_word(table.kind, table.table_kind)}; '
                f'its columns are {", ".join(columns)}')
    if len(set(names)) != len(names):
        raise TableContentError('a column is named twice')
    if kind.x not in names:
        raise TableContentError(
            f'{_kind_word(table.kind, table.table_kind)} has a {columns[kind.x].label} '
            f'column ({kind.x!r})')
    rows = []
    for at, row in enumerate(table.rows, start=1):
        if not isinstance(row, Sequence) or isinstance(row, str) or len(row) != len(names):
            raise TableContentError(f'row {at} has {_count(row)}, not one for each of the '
                                    f'{len(names)} columns')
        rows.append([_strict(columns[name], value, at) for name, value in zip(names, row)])
    x_unit = kind.x_unit if table.x_unit is None else table.x_unit
    y_unit = kind.y_unit if table.y_unit is None else table.y_unit
    for unit in (x_unit, y_unit):
        if not isinstance(unit, str):
            raise TableContentError('a unit is text')
    return Table(table.kind, names, rows, table.table_kind, x_unit, y_unit)


def _strict(column: Column, value: Any, at: int) -> Any:
    """A value as the column holds it, or a refusal naming row and column."""
    where = f'row {at}, {column.label}'
    if value is None:
        if column.nullable:
            return None
        raise TableContentError(f'{where}: a value is needed')
    if (column.type == 'number' and isinstance(value, (int, float))
            and not isinstance(value, bool) and math.isfinite(value)):
        return float(value)
    if column.type == 'choice' and value in column.values:
        return value
    if column.type == 'text' and isinstance(value, str):
        return value
    wanted = ('a finite number' if column.type == 'number'
              else 'text' if column.type == 'text'
              else 'one of ' + ', '.join(column.values))
    raise TableContentError(f'{where}: {value!r} is not {wanted}')


def _comments(table: Table) -> list[str]:
    """The comment lines that say which table a text or workbook file holds."""
    return [f'# Hertzforge table: {OBJECTS[table.kind]}', f'# table_kind: {table.table_kind}',
            f'# x_unit: {table.x_unit}', f'# y_unit: {table.y_unit}']


def _write_delimited(path: str, table: Table, form: str) -> None:
    delimiter = ',' if form == 'csv' else '\t'
    with open(path, 'w', encoding='utf-8', newline='') as out:
        out.writelines(line + '\r\n' for line in _comments(table))
        writer = csv.writer(out, delimiter=delimiter, lineterminator='\r\n')
        writer.writerow(table.columns)
        for row in table.rows:
            writer.writerow([_text(value) for value in row])


def _write_xlsx(path: str, table: Table, form: str) -> None:
    from openpyxl import Workbook
    from openpyxl.styles import Font

    book = Workbook()
    sheet = book.active
    sheet.title = BREAKPOINT_KINDS[table.table_kind].label.title()
    for line in _comments(table):
        sheet.append([line])
    sheet.append(list(table.columns))
    header = sheet.max_row
    for cell in sheet[header]:
        cell.font = Font(bold=True)
    for row in table.rows:
        sheet.append(list(row))
    sheet.freeze_panes = sheet.cell(row=header + 1, column=1)
    book.save(path)


def _write_hforge(path: str, table: Table, form: str) -> None:
    import h5py
    import numpy as np

    from .native import BASE_SCHEMA, STAMP

    columns = {c.name: c for c in _breakpoint_kind(table.table_kind).columns}
    with h5py.File(path, 'w') as f:
        f.attrs[STAMP] = BASE_SCHEMA
        group = f.create_group(OBJECTS[table.kind])
        group.attrs['column_order'] = list(table.columns)
        group.attrs['table_kind'] = table.table_kind
        group.attrs['x_unit'] = table.x_unit
        group.attrs['y_unit'] = table.y_unit
        for k, name in enumerate(table.columns):
            values = [row[k] for row in table.rows]
            if columns[name].type == 'number':
                group.create_dataset(name, data=np.asarray(
                    [math.nan if v is None else v for v in values], dtype=np.float64))
            else:
                group.create_dataset(name, data=['' if v is None else v for v in values],
                                     dtype=h5py.string_dtype('utf-8'))


# ---- reading ------------------------------------------------------------------------------


def read(path: str | os.PathLike, kind: str, table_kind: str | None = None,
         x_unit: str | None = None, y_unit: str | None = None) -> Table:
    """Read the table a file holds, in the form its suffix names.

    Parameters
    ----------
    path : str or os.PathLike
        The file.
    kind : str
        `channels` or `breakpoints`: the kind of table wanted. A file that
        holds another is refused.
    table_kind : str, optional
        For a breakpoint table, which one is wanted (`random_profile` and so
        on); a file that says it holds another is refused. When not given,
        the file's own is taken.
    x_unit, y_unit : str, optional
        For a breakpoint table, the units it is taken to be in where the
        file does not say (the units of the table it is opened into); the
        kind's own when not given.

    Returns
    -------
    Table
        The columns found, by name, and the rows, with what was left out or
        assumed in `warnings`. A channel table comes back with every column
        of the channel table, as it holds them.
    """
    if kind not in KINDS:
        raise TableContentError(f'the kind of table is one of {", ".join(KINDS)}, '
                                f'not {kind!r}')
    if kind == 'channels' and table_kind:
        raise TableContentError('a channel table has no table_kind')
    if kind == 'channels' and (x_unit is not None or y_unit is not None):
        raise TableContentError('a channel table has no x_unit or y_unit')
    for unit in (x_unit, y_unit):
        if unit is not None and not isinstance(unit, str):
            raise TableContentError('a unit is text')
    if table_kind is not None:
        _breakpoint_kind(table_kind)
    path = os.path.abspath(os.path.expanduser(str(path)))
    form = form_of(path)
    if not os.path.isfile(path):
        raise FileNotFoundError(f'no file at {path}')
    name = os.path.basename(path)
    if kind == 'channels':
        return _read_channels(path, form, name)
    units = (x_unit, y_unit)
    if form == 'hforge':
        return _read_hforge(path, name, table_kind, units)
    grid = _xlsx_rows(path, name) if form == 'xlsx' else _delimited_rows(path, form)
    return _from_grid(grid, name, table_kind, units)


@dataclass
class _Grid:
    """A text or workbook file as rows of cells: the comment lines' fields,
    and each other row with its row number in the file."""

    said: dict[str, str]
    rows: list[tuple[int, list[Any]]]


def _comment(cell: Any) -> str | None:
    """The text of a comment cell (`#` or `//`), or None for any other."""
    if not isinstance(cell, str):
        return None
    text = cell.lstrip()
    for mark in ('#', '//'):
        if text.startswith(mark):
            return text[len(mark):].strip()
    return None


def _said(said: dict[str, str], text: str) -> None:
    """Keep a comment line's `key: value`, the key normalized."""
    key, colon, value = text.partition(':')
    if colon:
        said.setdefault(normalize(key), value.strip())


def _decoded(data: bytes) -> str:
    """A text file's characters: UTF-8 (with or without its byte order
    mark), else the Windows code page a spreadsheet saves in."""
    for encoding in ('utf-8-sig', 'cp1252'):
        try:
            return data.decode(encoding)
        except UnicodeDecodeError:
            continue
    return data.decode('latin-1')


def _delimited_rows(path: str, form: str) -> _Grid:
    with open(path, 'rb') as stream:
        text = _decoded(stream.read())
    said: dict[str, str] = {}
    kept: list[tuple[int, str]] = []
    for number, line in enumerate(text.splitlines(), start=1):
        comment = _comment(line)
        if comment is not None:
            _said(said, comment)
        elif line.strip():
            kept.append((number, line))
    if not kept:
        return _Grid(said, [])
    first = kept[0][1]
    delimiter = ',' if form == 'csv' else '\t'
    if delimiter not in first:
        delimiter = next((d for d in ('\t', ',', ';') if d in first), delimiter)
    rows = []
    reader = csv.reader(io.StringIO('\n'.join(line for _, line in kept), newline=''),
                        delimiter=delimiter, strict=True)
    read_to = 0
    try:
        for cells in reader:
            # a quoted cell may run over several lines; its row is the line it began on
            rows.append((kept[read_to][0], cells))
            read_to = reader.line_num
    except csv.Error as error:
        raise TableFileError(f'{os.path.basename(path)} is not readable as '
                             f'{"comma" if delimiter == "," else "tab"}-delimited text: '
                             f'{error}') from None
    return _Grid(said, rows)


def _xlsx_rows(path: str, name: str) -> _Grid:
    from openpyxl import load_workbook

    try:
        book = load_workbook(path, read_only=True, data_only=True)
    except Exception as error:          # noqa: BLE001 - openpyxl's refusals vary by fault
        raise TableFileError(f'{name} is not a readable Excel workbook '
                             f'({type(error).__name__})') from None
    said: dict[str, str] = {}
    rows = []
    try:
        for number, cells in enumerate(book.active.iter_rows(values_only=True), start=1):
            cells = list(cells)
            while cells and cells[-1] in (None, ''):
                cells.pop()
            if not cells:
                continue
            comment = _comment(cells[0])
            if comment is not None:
                _said(said, comment)
                continue
            rows.append((number, cells))
    finally:
        book.close()
    return _Grid(said, rows)


def _xlsx_said(path: str) -> dict[str, str]:
    """A workbook's comment lines, or none when it cannot be read."""
    try:
        return _xlsx_rows(path, '').said
    except TableFileError:
        return {}


def _stated(said: dict[str, str], name: str, table_kind: str | None) -> str:
    """The breakpoint table kind a file is read as: the one asked for,
    checked against what the file says it holds."""
    held = said.get('hertzforgetable')
    if held is not None:
        held_kind = next((k for k, o in OBJECTS.items() if normalize(o) == normalize(held)), None)
        if held_kind is None:
            raise TableFileError(f'{name} says it holds a {held}, which is not a table '
                                 'Hertzforge reads')
        if held_kind != 'breakpoints':
            raise TableFileError(f'{name} holds {KIND_WORDS[held_kind]}, not '
                                 f'{KIND_WORDS["breakpoints"]}')
    file_kind = said.get('tablekind') or None
    if file_kind is not None and table_kind is not None and file_kind != table_kind:
        raise TableFileError(f'{name} holds {_kind_word("breakpoints", file_kind)}, not '
                             f'{_kind_word("breakpoints", table_kind)}')
    chosen = table_kind or file_kind
    if chosen is None:
        raise TableFileError(f'{name} does not say which breakpoint table it holds; open it '
                             'from the table it belongs to')
    _breakpoint_kind(chosen)
    return chosen


def _unit_in(heading: Any) -> str | None:
    """The unit a heading gives in parentheses or brackets, or None."""
    if not isinstance(heading, str):
        return None
    for start, end in (('(', ')'), ('[', ']')):
        if start in heading and end in heading.split(start, 1)[1]:
            unit = heading.split(start, 1)[1].split(end, 1)[0].strip()
            return unit or None
    return None


class _Matcher:
    """Headings to the columns of one kind of breakpoint table."""

    def __init__(self, columns: Sequence[Column]) -> None:
        self.columns: dict[str, Column] = {c.name: c for c in columns}
        self.words: dict[str, str] = {}
        for c in columns:
            for word in (c.name, c.label, *c.aliases):
                self.words.setdefault(normalize(word), c.name)

    def match(self, heading: Any) -> list[str]:
        """The columns a heading names: none, one, or both sides of a band
        (`Alarm` alone stands for the same dB below and above)."""
        if heading is None:
            return []
        raw = str(heading).strip()
        base = raw.split('(')[0].split('[')[0].strip()
        if base.endswith(('-', '+')) and base[:-1].strip():
            word = normalize(base[:-1]) + ('below' if base.endswith('-') else 'above')
            if word in self.words:
                return [self.words[word]]
        key = normalize(raw)
        if key in self.words:
            return [self.words[key]]
        for band in ('alarm', 'abort'):
            if key in (band, f'{band}db') and f'{band}_below_db' in self.columns:
                return [f'{band}_below_db', f'{band}_above_db']
        return []


def _names_a_channel(headings: Sequence[Any]) -> bool:
    """Whether a heading row is a channel table's: a cell names its channel
    column."""
    from . import excel

    return any(excel._key(h) in excel.HEADER_MARKERS for h in headings if h is not None)


def _from_grid(grid: _Grid, name: str, table_kind: str | None,
               assumed: tuple[str | None, str | None] = (None, None)) -> Table:
    """The breakpoint table a file's rows hold. Its units are the ones the
    file says, in a comment line or a heading, else `assumed`, else the
    kind's own."""
    if grid.said.get('hertzforgetable') is None and grid.rows and _names_a_channel(
            grid.rows[0][1]):
        raise TableFileError(f'{name} holds a channel table, not '
                             f'{_kind_word("breakpoints", table_kind)}')
    table_kind = _stated(grid.said, name, table_kind)
    bk = BREAKPOINT_KINDS[table_kind]
    matcher = _Matcher(bk.columns)
    if not grid.rows:
        raise TableFileError(f'{name} has no heading row and no rows')
    warnings: list[str] = []
    _first, headings = grid.rows[0]
    matched = [matcher.match(h) for h in headings]
    data = grid.rows[1:]
    if not any(matched):
        if bk.positional and _number(headings[0]) is not None:
            matched = [[c] for c in bk.positional[:len(headings)]]
            data = grid.rows
            warnings.append(f'{name} has no heading row; its columns were read as '
                            + ', '.join(matcher.columns[c].label
                                        for c in bk.positional[:len(headings)]))
        else:
            raise TableFileError(f'{name} has none of the columns of '
                                 f'{_kind_word("breakpoints", table_kind)} in its heading row')
    taken: dict[str, int] = {}
    units: dict[str, str] = {}
    for at, (heading, names) in enumerate(zip(headings, matched)):
        if not names:
            if heading not in (None, ''):
                warnings.append(f'Left out the column {str(heading).strip()!r}: it is not a '
                                f'column of {_kind_word("breakpoints", table_kind)}.')
            continue
        for column in names:
            if column in taken:
                warnings.append(f'Left out the column {str(heading).strip()!r}: '
                                f'{matcher.columns[column].label} was read from an earlier '
                                'column.')
                continue
            taken[column] = at
            unit = _unit_in(heading)
            if unit:
                units[column] = unit
    out_columns = [c.name for c in bk.columns if c.name in taken]
    rows = []
    wide = 0
    for line, cells in data:
        if all(c in (None, '') for c in cells):
            continue
        if any(c not in (None, '') for c in cells[len(headings):]):
            wide += 1
        rows.append([_lenient(matcher.columns[c],
                              cells[taken[c]] if taken[c] < len(cells) else None, line)
                     for c in out_columns])
    if wide:
        warnings.append(f'Left out cells beyond the last heading in {wide} '
                        f'row{"" if wide == 1 else "s"}.')
    if bk.x not in out_columns:
        raise TableFileError(f'{name} has no {bk.columns[0].label} column')
    if not any(c in out_columns for c in bk.needs):
        needs = ' or '.join(c.label for c in bk.columns if c.name in bk.needs)
        raise TableFileError(f'{name} has no {needs} column, which '
                             f'{_kind_word("breakpoints", table_kind)} needs')
    y = next(c for c in bk.needs if c in out_columns)
    x_assumed, y_assumed = assumed
    x_unit = grid.said.get('xunit') or units.get(bk.x) or (
        bk.x_unit if x_assumed is None else x_assumed)
    y_unit = grid.said.get('yunit') or units.get(y) or (
        bk.y_unit if y_assumed is None else y_assumed)
    if not rows:
        warnings.append(f'{name} has no rows.')
    return Table('breakpoints', out_columns, rows, table_kind, x_unit, y_unit, warnings)


def _number(cell: Any) -> float | None:
    """A cell's number, or None when it holds none. Text is read up to its
    first space, so `0.04 g2/Hz` reads as 0.04."""
    if isinstance(cell, bool):
        return None
    if isinstance(cell, (int, float)):
        return float(cell)
    if not isinstance(cell, str):
        return None
    token = cell.strip().split(' ')[0].lstrip('+') if cell.strip() else ''
    try:
        return float(token)
    except ValueError:
        return None


def _lenient(column: Column, cell: Any, line: int) -> Any:
    """A cell as the column holds it: its default when empty, else read by
    the column's type, or a refusal naming the row and column."""
    where = f'Row {line}, {column.label}'
    if isinstance(cell, str):
        cell = cell.strip()
    empty = cell is None or cell == ''
    if column.type == 'number' and column.nullable and (
            empty or (isinstance(cell, str) and cell.lower() in NONE_WORDS)
            or (isinstance(cell, float) and math.isnan(cell))):
        return None
    if empty:
        return column.default
    if column.type == 'text':
        return _text(cell) if not isinstance(cell, str) else cell
    if column.type == 'number':
        value = _number(cell)
        if value is None or not math.isfinite(value):
            raise TableFileError(f'{where}: {cell!r} is not a number')
        return value
    text = _text(cell) if not isinstance(cell, str) else cell
    if text in column.values:
        return text
    word = normalize(text)
    if word in column.choices:
        return column.choices[word]
    raise TableFileError(f'{where}: {cell!r} is not one of {", ".join(column.values)}')


def _hforge_holding(path: str, name: str, kind: str) -> list[str]:
    """Check a `.hforge` file holds the kind of table wanted, refusing any
    other in words; the column order the table's group lists."""
    import h5py

    from .native import SCHEMA_VERSION, STAMP, VDYN_STAMP

    try:
        f = h5py.File(path, 'r')
    except OSError:
        raise TableFileError(f'{name} is not a readable .hforge file') from None
    with f:
        if STAMP not in f.attrs:
            what = ('a .vdyn file' if VDYN_STAMP in f.attrs
                    else 'not a Hertzforge file (it has no schema stamp)')
            raise TableFileError(f'{name} is {what}, not a table file')
        if int(f.attrs[STAMP]) > SCHEMA_VERSION:
            raise TableFileError(f'{name} was written by a newer Hertzforge (schema '
                                 f'{int(f.attrs[STAMP])}); update Hertzforge to open it')
        held = next((k for k, o in OBJECTS.items() if o in f), None)
        if held is None:
            what = ('a Hertzforge project' if 'objects' in f
                    else f'a {next(iter(f)).replace("_", " ")} object' if len(f)
                    else 'nothing')
            raise TableFileError(f'{name} holds {what}, not {KIND_WORDS[kind]}')
        if held != kind:
            raise TableFileError(f'{name} holds {KIND_WORDS[held]}, not {KIND_WORDS[kind]}')
        return [_attr_text(v) for v in f[OBJECTS[kind]].attrs.get('column_order', [])]


def _read_hforge(path: str, name: str, table_kind: str | None,
                 assumed: tuple[str | None, str | None]) -> Table:
    import h5py

    order = _hforge_holding(path, name, 'breakpoints')
    with h5py.File(path, 'r') as f:
        group = f[OBJECTS['breakpoints']]
        said = {normalize(k): _attr_text(v) for k, v in group.attrs.items()
                if k in ('table_kind', 'x_unit', 'y_unit')}
        table_kind = _stated({'tablekind': said.get('tablekind', '')}, name, table_kind)
        stored: dict[str, list[Any]] = {}
        for column in order:
            if column not in group:
                raise TableFileError(f'{name} lists the column {column!r} but holds no '
                                     'values for it')
            stored[column] = _dataset_values(group[column])
    if not order:
        raise TableFileError(f'{name} holds a table with no columns')
    counts = {len(v) for v in stored.values()}
    if len(counts) > 1:
        raise TableFileError(f'{name} has columns of different lengths')
    rows = [[stored[c][k] for c in order] for k in range(counts.pop())]
    grid = _Grid({'hertzforgetable': OBJECTS['breakpoints'], 'xunit': said.get('xunit', ''),
                  'yunit': said.get('yunit', '')},
                 [(0, list(order)), *((k, row) for k, row in enumerate(rows, start=1))])
    return _from_grid(grid, name, table_kind, assumed)


def _attr_text(value: Any) -> str:
    return value.decode('utf-8') if isinstance(value, bytes) else str(value)


def _dataset_values(dataset: Any) -> list[Any]:
    """A dataset's values as plain Python: text, or numbers with None for
    NaN."""
    values = dataset[()]
    kind = values.dtype.kind
    if kind in 'SO':
        return [v.decode('utf-8') if isinstance(v, bytes) else str(v) for v in values]
    if kind in 'iuf':
        return [None if math.isnan(v) else float(v) for v in values.astype(float)]
    raise TableFileError(f'the column {dataset.name.rsplit("/", 1)[-1]!r} holds values of a '
                         'type a table does not have')
