"""Visual Dynamics' file-level tools: saving one object in a foreign
format, importing part of a long Rattlesnake run on a preview of it,
cutting a run and writing it back, and reading delimited text with the
layout its reader found corrected where it is wrong.

A file reaches the service by its path on the service's machine, as
`project.import` and `project.save` take it. Each tool calls the core's
own reader or writer (`Project.export`, `io.save`,
`io.rattlesnake.stream_summary`, `stream_preview`, `save_window`,
`io.delimited.text_layout`, `read_delimited`, `Project.import_file`);
the previews add nothing to the project, and the imports add what the
same import in Visual Dynamics adds.
"""

from __future__ import annotations

import os
import warnings
from collections.abc import Callable
from typing import Any

import numpy as np

from . import views as v
from .descriptors import Operation, Option, Param
from .projects import kind_of
from .protocol import not_found, refused
from .waterfalls import unit_text

GB = 2 ** 30

#: how many data rows a text preview reads from the head of the file:
#: Visual Dynamics' dialog's own `PREVIEW_ROWS`
TEXT_PREVIEW_ROWS = 20000
#: how many of them the preview lists as rows of text
LISTED_ROWS = 10

#: every kind a project holds; each is written as a `.hforge` object, and
#: the foreign formats each take some of them (`io.exporters(obj)`)
SAVED_KINDS = ('geometry', 'time_history', 'spectrum', 'frf', 'psd', 'coherence',
               'multiple_coherence', 'specification', 'transient_specification', 'srs',
               'shock_specification', 'sine_target', 'sine_level', 'shape_set',
               'channel_table', 'report', 'photos', 'matched_modes',
               'sine_sweep_specification', 'sine_level_set', 'predictions',
               'run_log_extract')

NATIVE = Option('hforge', 'Hertzforge object (.hforge)')


def _formats() -> tuple[Option, ...]:
    from hertzforge.io import exporters

    return (NATIVE, *(Option(e.name, e.description) for e in exporters()))


OBJECT = Param('object', 'Object', 'enum', options_from='objects:' + '|'.join(SAVED_KINDS))
FORMAT = Param('format', 'Format', 'enum', default='hforge', options=_formats())
PATH = Param('path', 'File', 'string', default='')
CREATOR = Param('creator', 'Creator, for ESCDF (blank: the login name)', 'string',
                default='')

RUN = Param('path', 'Rattlesnake run (.nc4)', 'string', default='')
START = Param('start', 'Start (0: the run\'s start)', 'number', default=0.0, min=0.0,
              step=0.1, unit='s')
STOP = Param('stop', 'Stop (0: the run\'s end)', 'number', default=0.0, min=0.0, step=0.1,
             unit='s')
LAST = Param('last', 'Last of the run (0: not used)', 'number', default=0.0, min=0.0,
             step=1.0, unit='s')
CHANNELS = Param('channels', 'Channels (coordinates or rows from 1; blank: all)', 'string',
                 default='')
PREVIEWED = Param('record', 'Preview channel', 'integer', default=1, min=1, step=1)
STREAM = Param('stream', 'Stream (0: the largest)', 'integer', default=0, min=0, step=1)
TARGET = Param('target', 'Copy (blank: named for the window, beside the run)', 'string',
               default='')

TEXT = Param('path', 'Text file', 'string', default='')
DELIMITER = Param('delimiter', 'Delimiter', 'enum', default='found', options=(
    Option('found', 'As found'), Option('tab', 'Tab'), Option('semicolon', 'Semicolon'),
    Option('comma', 'Comma'), Option('pipe', 'Pipe'), Option('spaces', 'Spaces')))
DECIMAL = Param('decimal', 'Decimal point', 'enum', default='found', options=(
    Option('found', 'As found'), Option('.', 'Point (0.25)'), Option(',', 'Comma (0,25)')))
TIME = Param('time_column', 'Time column (blank: as found; none: rows evenly sampled)',
             'string', default='')
RATE = Param('sample_rate', 'Sample rate (0: as found)', 'number', default=0.0, min=0.0,
             step=1.0, unit='Hz')
COLUMNS = Param('columns', 'Columns to read (names or numbers from 1; blank: all)', 'string',
                default='')
NAMES = Param('names', 'Names, one per column read (blank: the header\'s)', 'string',
              default='')
UNITS = Param('units', 'Units: one for all, one per column, or column=unit (blank: the '
              'header\'s)', 'string', default='')


def _file(given: str, what: str) -> str:
    """`given` as an absolute path to a file that is there."""
    if not given.strip():
        raise refused(f'{what} needs a file\'s path on the analysis service\'s machine')
    path = os.path.abspath(os.path.expanduser(given.strip()))
    if not os.path.exists(path):
        raise not_found(f'no file at {path}')
    return path


def _noted(call: Callable[[], Any]) -> tuple[Any, list[str]]:
    """What `call` returns and the notes its reader gave on the way
    (`io.ImportNote`), which Visual Dynamics lists after an import."""
    from hertzforge.io import ImportNote

    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter('always')
        try:
            out = call()
        except (ValueError, OSError) as error:
            raise refused(str(error)) from None
    return out, [str(w.message) for w in caught if issubclass(w.category, ImportNote)]


def _size(path: str) -> float:
    if os.path.isdir(path):
        return float(sum(os.path.getsize(os.path.join(folder, name))
                         for folder, _dirs, names in os.walk(path) for name in names))
    return float(os.path.getsize(path))


def _added(project: Any, added: list[str], notes: list[str]) -> list[dict]:
    rows = []
    for name in added:
        obj = project[name]
        records = getattr(obj, 'num_records', None)
        said = kind_of(obj).replace('_', ' ')
        rows.append(v.row(name, f'{said}, {records} records' if records is not None
                          else said, None, 'pass'))
    rows += [v.row('Note', note, None, 'warn') for note in notes]
    return rows


# ---- Save As -------------------------------------------------------------------

def _exodus_geometry(project: Any, name: str, obj: Any) -> tuple[str, Any]:
    """The geometry shapes or data ride on in an Exodus file, as Visual
    Dynamics picks it: the one in the object's group, else the active
    one, else the only one; refused when there is none, or when none of
    the object's DOFs are on it."""
    from hertzforge.core.geometry import Geometry
    from hertzforge.core.shapes import ShapeSet

    what = 'mode shapes' if isinstance(obj, ShapeSet) else 'data'
    found = project.geometry_for(name)
    if found is None:
        geometries = [(key, value) for key, value in project.items()
                      if isinstance(value, Geometry)]
        found = geometries[0] if len(geometries) == 1 else None
    if found is None:
        raise refused(f'Writing {what} to Exodus needs a geometry in the project: the file '
                      'is a mesh with results')
    geometry_name, geometry = found
    dofs = list(obj.coordinate if isinstance(obj, ShapeSet) else obj.response_dof)
    # a record with no DOF is a global variable and needs no node
    if all(d.strip() for d in dofs) and len(geometry.missing_dofs(dofs)) >= len(dofs):
        raise refused(f'{geometry_name} has none of the {what} DOFs; make the right '
                      'geometry active and save again')
    return geometry_name, geometry


def save_as(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.save_as`: one object written to a file in the format
    chosen, as Visual Dynamics' Save As writes it: a `.hforge` object
    (`io.save`), or a foreign format by `Project.export`, in SI (the
    service has no display units yet). The format's suffix is added when
    the path lacks it. A format that cannot write the object is refused,
    naming those that can. Shapes and data written to Exodus ride on a
    geometry, and an ESCDF file records `creator` as its creator."""
    import hertzforge as hz
    from hertzforge.io import exporters
    from hertzforge.io import save as save_object
    from hertzforge.io.escdf_objects import SUFFIXES
    from hertzforge.io.exporters import units_written
    from hertzforge.io.native import hforge_path

    name, chosen = params['object'], params['format']
    obj = project[name]
    if not params['path'].strip():
        raise refused('Save As needs a path to write to, on the analysis service\'s machine')
    path = os.path.abspath(os.path.expanduser(params['path'].strip()))
    available = exporters(obj)
    labels = {option.value: option.label for option in FORMAT.options}
    rows = [v.row('Object', f'{name}, a {kind_of(obj).replace("_", " ")}', None, 'info'),
            v.row('Format', labels[chosen], None, 'info')]
    if chosen == NATIVE.value:
        try:
            save_object(obj, path)
        except (ValueError, TypeError, OSError) as error:
            raise refused(str(error)) from None
        written = hforge_path(path)
        units = 'SI, as stored'
    else:
        exporter = next((e for e in available if e.name == chosen), None)
        if exporter is None:
            can = ', '.join([NATIVE.label, *(e.description for e in available)])
            raise refused(f'{name} cannot be written as {labels[chosen]}; it can be '
                          f'written as {can}')
        if exporter.suffix and not path.endswith(exporter.suffix) and not (
                chosen == 'escdf' and path.endswith(SUFFIXES)):
            path += exporter.suffix
        extras: dict[str, Any] = {}
        if chosen == 'exodus' and kind_of(obj) != 'geometry':
            geometry_name, extras['geometry'] = _exodus_geometry(project, name, obj)
            rows.append(v.row('On geometry', geometry_name, None, 'info'))
        if chosen == 'escdf':
            extras['created_by'] = params['creator'].strip() or None
            rows.append(v.row('Creator', params['creator'].strip() or 'the login name',
                              None, 'info'))
        try:
            project.export(name, path, unit_system=hz.SI, format=chosen, **extras)
        except (ValueError, OSError) as error:
            raise refused(str(error)) from None
        written = path
        units = units_written(chosen, hz.SI)
    declared = getattr(obj, 'units_defined', True)
    rows += [v.row('Written', written, None, 'pass'),
             v.row('Size', _size(written), 'bytes', 'info'),
             v.row('Units', units if declared else 'undefined, written as they stand',
                   None, 'info' if declared else 'warn')]
    return [v.reading(f'Saved {os.path.basename(written)}', rows)]


# ---- Rattlesnake runs ------------------------------------------------------------

def _run(given: str) -> tuple[str, dict[str, Any]]:
    """A Rattlesnake run's path and its `stream_summary`, refused when it
    is not a run or holds no streamed time data."""
    from hertzforge.io import rattlesnake

    path = _file(given, 'A Rattlesnake run')
    base = os.path.basename(path)
    if not rattlesnake.sniff(path):
        raise refused(f'{base} is not a Rattlesnake run')
    summary = rattlesnake.stream_summary(path)
    if not summary['streams']:
        raise refused(f'{base} holds no streamed time data')
    return path, summary


def _largest(summary: dict[str, Any]) -> dict[str, Any]:
    return max(summary['streams'], key=lambda s: s['bytes'])


def _window(params: dict[str, Any], seconds: float) -> tuple[float | None, float | None]:
    """(start, stop) in seconds from the params, None for an open end: the
    last so many seconds of the run (`rattlesnake.last_window`) when
    `last` is set, else `start` and `stop`, 0 standing for the run's own
    ends."""
    from hertzforge.io import rattlesnake

    if params['last'] > 0:
        if params['start'] > 0 or params['stop'] > 0:
            raise refused('give the last so many seconds of the run, or a start and a stop, '
                          'not both')
        return rattlesnake.last_window(seconds, params['last']), None
    return params['start'] or None, params['stop'] or None


def _samples(stream: dict[str, Any], rate: float, start: float | None,
             stop: float | None) -> tuple[int, int]:
    """The samples [a, b) the window holds of a stream, by the import's own
    rule (`rattlesnake._sample_window`); (0, 0) when it misses it."""
    from hertzforge.io import rattlesnake

    try:
        found = rattlesnake._sample_window(start, stop, int(stream['samples']), rate)
    except ValueError as error:
        raise refused(str(error)) from None
    return found if found is not None else (0, 0)


def _channels(text: str, dofs: list[str]) -> list[int | str] | None:
    """The channels a `channels` param names: coordinates as the table
    spells them, or rows counted from 1 (the core's rows count from 0).
    None for blank, every channel."""
    picked: list[int | str] = []
    for token in text.replace(',', ' ').split():
        if token.isdigit():
            row = int(token)
            if not 1 <= row <= len(dofs):
                raise refused(f'there is no channel {row}; the run has {len(dofs)}')
            picked.append(row - 1)
        else:
            picked.append(token)
    return picked or None


def rattlesnake_preview(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.rattlesnake_preview`: a long run as Visual Dynamics'
    import window shows it before anything is read whole: the size of
    each stream (`io.rattlesnake.stream_summary`), one channel's envelope
    over a stream (`stream_preview`, in SI), the window chosen shaded off
    the rest, and what importing that window and those channels would
    cost against the machine's memory. Adds nothing to the project."""
    from hertzforge.io import rattlesnake
    from hertzforge.units import si_factor

    path, summary = _run(params['path'])
    base = os.path.basename(path)
    streams, rate, dofs = summary['streams'], float(summary['sample_rate']), summary['channels']
    if params['stream'] > len(streams):
        raise refused(f'{base} has {len(streams)} streams; there is no stream {params["stream"]}')
    stream = streams[params['stream'] - 1] if params['stream'] else _largest(summary)
    if params['record'] > len(dofs):
        raise refused(f'{base} has {len(dofs)} channels; there is no channel {params["record"]}')
    seconds = float(stream['seconds'])
    start, stop = _window(params, seconds)
    first, last = _samples(stream, rate, start, stop)
    chosen = _channels(params['channels'], dofs)
    try:
        kept = len(dofs) if chosen is None else len(rattlesnake._channel_rows(chosen, dofs))
        preview = rattlesnake.stream_preview(path, params['record'] - 1,
                                             stream=stream['variable'])
    except ValueError as error:
        raise refused(str(error)) from None

    scale = si_factor(preview['unit']) if preview['unit'] else 1.0
    times = np.repeat(preview['times'], 2)
    values = np.stack([preview['low'], preview['high']], axis=1).ravel() * scale
    keep = np.isfinite(values)
    unit = unit_text(preview['dim']) if preview['unit'] else None
    begin, end = (0.0 if start is None else float(start)), (seconds if stop is None
                                                            else min(float(stop), seconds))
    regions = []
    if begin > 0:
        regions.append(v.region(0.0, begin, 'cut', 'Left out, before the window'))
    if end < seconds:
        regions.append(v.region(end, seconds, 'cut', 'Left out, after the window'))
    view = v.spectrum(f'{preview["dof"]} over {stream["key"]} of {base}',
                      v.axis('Time', 's', 'linear'),
                      v.axis(preview['dof'], unit, 'linear'),
                      [v.series(preview['dof'], 'measured', times[keep], values[keep])],
                      regions=regions)

    memory = int(summary.get('memory') or 0)
    total = sum(s['bytes'] for s in streams)
    samples = last - first
    size = samples * kept * 8
    rows = [v.row('Run', f'{len(dofs)} channels for {seconds:.4g} s at {rate:g} Hz', None,
                  'info'),
            v.row('Time data', total / GB, 'GB', 'info')]
    if memory:
        rows.append(v.row('Machine memory', memory / GB, 'GB', 'info'))
    rows += [v.row('Stream', f'{s["key"]}: {s["samples"]:,} samples, {s["seconds"]:.4g} s',
                   None, 'info') for s in streams]
    rows += [v.row('Window', f'{begin:.4g} to {end:.4g} s', None, 'info'),
             v.row('Samples of each channel', float(samples), '',
                   'pass' if samples >= 2 else 'warn'),
             v.row('Channels kept', f'{kept} of {len(dofs)}', None, 'info')]
    large = memory and size >= rattlesnake.LARGE_STREAM_SHARE * memory
    rows.append(v.row('Import size', size / GB, 'GB', 'warn' if large else 'pass'))
    if samples < 2:
        rows.append(v.row('Note', 'the span holds fewer than two samples', None, 'warn'))
    elif large:
        rows.append(v.row('Note', f'the import would take {size / memory:.0%} of the '
                          'machine\'s memory', None, 'warn'))
    channels = v.table(f'Channels of {base}', ['Row', 'Channel', 'Unit'],
                       [[str(k + 1), dof, unit or 'undefined']
                        for k, (dof, unit) in enumerate(zip(dofs, summary['units'],
                                                            strict=True))])
    return [view, v.reading(f'Importing part of {base}', rows), channels]


def import_rattlesnake_part(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.import_rattlesnake_part`: a window of a run and a pick of
    its channels imported, with the `Project.import_file(path, start=...,
    stop=..., channels=...)` call Visual Dynamics' import window makes,
    holding only what differs from importing the whole run. Adds what
    that import adds: the windowed histories, the channel table and the
    environments' results, linked as one group."""
    from hertzforge.io import rattlesnake

    path, summary = _run(params['path'])
    dofs = summary['channels']
    start, stop = _window(params, float(_largest(summary)['seconds']))
    options: dict[str, Any] = {}
    if start is not None:
        options['start'] = float(start)
    if stop is not None:
        options['stop'] = float(stop)
    chosen = _channels(params['channels'], dofs)
    if chosen is not None:
        options['channels'] = chosen
    added, notes = _noted(lambda: project.import_file(path, **options))
    kept = len(dofs) if chosen is None else len(rattlesnake._channel_rows(chosen, dofs))
    begin = 0.0 if start is None else float(start)
    end = 'the end' if stop is None else f'{float(stop):.4g} s'
    rows = [v.row('File', path, None, 'info'),
            v.row('Window', f'{begin:.4g} s to {end}', None, 'info'),
            v.row('Channels', f'{kept} of {len(dofs)}', None, 'info')]
    return [v.reading(f'Imported part of {os.path.basename(path)}',
                      rows + _added(project, added, notes))]


def truncate_rattlesnake_file(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.truncate_rattlesnake_file`: a run cut to a window and
    written as a `.nc4` of its own (`io.rattlesnake.save_window`), every
    channel and every setting as the file has them and its clock
    starting at zero, as Visual Dynamics' Truncate Rattlesnake File
    writes it. Named for the window beside the run
    (`rattlesnake.window_name`) unless a copy is named. A window that is
    the whole run is refused, as Visual Dynamics offers no copy of it.
    Adds nothing to the project; the project's journal records the
    call."""
    from hertzforge.io import rattlesnake

    path, summary = _run(params['path'])
    rate = float(summary['sample_rate'])
    stream = _largest(summary)
    start, stop = _window(params, float(stream['seconds']))
    first, last = _samples(stream, rate, start, stop)
    if (first, last) == (0, int(stream['samples'])):
        raise refused('the window is the whole run; a copy of it would be the run itself')
    target = (os.path.abspath(os.path.expanduser(params['target'].strip()))
              if params['target'].strip() else rattlesnake.window_name(path, start, stop))
    try:
        kept = rattlesnake.save_window(path, target, start, stop)
    except (ValueError, OSError) as error:
        raise refused(str(error)) from None
    project.journal.append(f'hertzforge.io.rattlesnake.save_window({path!r}, {target!r}, '
                           f'{start!r}, {stop!r})')
    begin = 0.0 if start is None else float(start)
    end = (last - 1) / rate
    rows = [v.row('Written', target, None, 'pass'),
            v.row('Size', _size(target), 'bytes', 'info'),
            v.row('Span', f'{begin:.4g} to {end:.4g} s of {os.path.basename(path)}, its '
                  'clock starting at zero', None, 'info')]
    rows += [v.row(f'Samples kept, {variable}', float(count), '', 'info')
             for variable, count in kept.items()]
    return [v.reading(f'Saved {os.path.basename(target)}', rows)]


# ---- delimited text ----------------------------------------------------------------

def _text(given: str) -> str:
    from hertzforge.io import delimited

    path = _file(given, 'A text import')
    if not delimited.sniff(path):
        raise refused(f'{os.path.basename(path)} does not read as delimited text')
    return path


def _key(token: str) -> int | str:
    """A column by name, or by number counted from 1 (the core counts
    from 0)."""
    if token.isdigit():
        if int(token) < 1:
            raise refused('columns are counted from 1')
        return int(token) - 1
    return token


def _list(text: str) -> list[str]:
    return [token.strip() for token in text.split(',')] if text.strip() else []


def _text_options(path: str, params: dict[str, Any]) -> tuple[Any, dict[str, Any]]:
    """(layout, options): the file's layout as read with the delimiter and
    decimal point given, and `read_delimited`'s keywords for the
    corrections, holding only what differs from what the reader found,
    as Visual Dynamics' text import dialog answers."""
    from hertzforge.io import delimited

    names = {name: mark for mark, name in delimited.DELIMITER_NAMES.items()}
    try:
        found = delimited.text_layout(path)
        delimiter = (found.delimiter if params['delimiter'] == 'found'
                     else names[params['delimiter']])
        decimal = found.decimal if params['decimal'] == 'found' else params['decimal']
        layout = delimited.text_layout(path, delimiter=delimiter, decimal=decimal)
    except ValueError as error:
        raise refused(str(error)) from None
    out: dict[str, Any] = {}
    if layout.delimiter != found.delimiter:
        out['delimiter'] = layout.delimiter
    if layout.decimal != found.decimal:
        out['decimal'] = layout.decimal
    time = params['time_column'].strip()
    if time:
        out['time_column'] = False if time.lower() == 'none' else _key(time)
    if params['sample_rate'] > 0 and params['sample_rate'] != (layout.sample_rate or 0.0):
        out['sample_rate'] = float(params['sample_rate'])
    if params['columns'].strip():
        out['columns'] = [_key(token) for token in _list(params['columns'])]
    if params['names'].strip():
        out['names'] = _list(params['names'])
    units = params['units'].strip()
    if units:
        tokens = _list(units)
        if '=' in units:
            pairs = [token.split('=', 1) for token in tokens]
            out['units'] = {_key(key.strip()): unit.strip() or None for key, unit in pairs}
        elif len(tokens) == 1:
            out['units'] = tokens[0]
        else:
            out['units'] = [token or None for token in tokens]
    return layout, out


def _line(label: str, number: int) -> dict:
    """A row giving a line of the file by its number, as an editor counts
    it, or saying the file has none."""
    return v.row(label, float(number), '', 'info') if number else v.row(label, 'none', None,
                                                                        'info')


def text_preview(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.text_preview`: a delimited text file as Visual Dynamics'
    text import dialog shows it: the layout `io.delimited.text_layout`
    found (delimiter, decimal point, header lines, time column, a stated
    sample rate), its columns, and the head of the file read by the same
    `read_delimited` call the import makes with the corrections given,
    as rows and as traces in SI, with the notes the read gives. A
    refusal is the one the import would give. Adds nothing to the
    project."""
    from hertzforge.io import delimited

    path = _text(params['path'])
    base = os.path.basename(path)
    layout, options = _text_options(path, params)
    history, notes = _noted(lambda: delimited.read_delimited(
        path, max_rows=TEXT_PREVIEW_ROWS, **options))
    time = layout.time_column
    rows = [v.row('Found', layout.describe(), None, 'info'),
            v.row('Delimiter', delimited.DELIMITER_NAMES[layout.delimiter], None, 'info'),
            v.row('Decimal point', 'comma' if layout.decimal == ',' else 'point', None,
                  'info'),
            _line('Names line', layout.names_line),
            _line('Units line', layout.units_line),
            _line('Data from line', layout.data_line),
            v.row('Time column found', 'none' if time is None else layout.columns[time],
                  None, 'info')]
    if layout.sample_rate is not None:
        rows.append(v.row('Sample rate stated', layout.sample_rate, '1/s', 'info'))
    count = history.num_records
    rows.append(v.row('Read', f'{count} channel{"s" * (count != 1)} at '
                      f'{history.sample_rate:.6g} samples per second', None, 'pass'))
    rows += [v.row('Note', note, None, 'warn') for note in notes]
    if options:
        rows.append(v.row('Corrections', ', '.join(f'{key}={value!r}'
                                                   for key, value in options.items()),
                          None, 'info'))

    columns = v.table(f'Columns of {base}', ['Column', 'Name', 'Unit in the file'],
                      [[str(k + 1), name, unit or '']
                       for k, (name, unit) in enumerate(zip(layout.columns, layout.units,
                                                            strict=True))])
    t = np.asarray(history.abscissa, dtype=float)
    values = np.asarray(history.ordinate, dtype=float)
    labels = [history.record_label(k) for k in range(count)]
    units = [unit_text(history.ordinate_dim[k]) for k in range(count)]
    heads = ['Time [s]', *(f'{label} [{unit}]' if unit else f'{label} [undefined]'
                           for label, unit in zip(labels, units, strict=True))]

    def cell(x: float, digits: int = 6) -> str:
        return f'{x:.{digits}g}' if np.isfinite(x) else 'missing'

    first = v.table(f'First rows of {base}, in SI', heads,
                    [[cell(t[i], 10), *(cell(values[k, i]) for k in range(count))]
                     for i in range(min(LISTED_ROWS, t.size))])
    lines = []
    for k in range(count):
        keep = np.isfinite(values[k])
        lines.append(v.series(labels[k], 'measured', t[keep], values[k][keep]))
    shared = units[0] if len(set(units)) == 1 else None
    traces = v.spectrum(f'The head of {base}, up to {TEXT_PREVIEW_ROWS:,} rows',
                        v.axis('Time', 's', 'linear'), v.axis('Value', shared, 'linear'),
                        lines)
    return [v.reading(f'Layout of {base}', rows), columns, first, traces]


def import_text(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.import_text`: a delimited text file imported as one time
    history with the layout's corrections, by the
    `Project.import_file(path, **options)` call Visual Dynamics' text
    import dialog makes, holding only what differs from what the reader
    found; the notes the read gives are listed."""
    path = _text(params['path'])
    _layout, options = _text_options(path, params)
    added, notes = _noted(lambda: project.import_file(path, **options))
    rows = [v.row('File', path, None, 'info')]
    if options:
        rows.append(v.row('Corrections', ', '.join(f'{key}={value!r}'
                                                   for key, value in options.items()),
                          None, 'info'))
    return [v.reading(f'Imported {os.path.basename(path)}',
                      rows + _added(project, added, notes))]


OPERATIONS = (
    Operation(
        id='project.save_as', title='Save As', workspace='analyze', accepts='project',
        summary='Write one object to a file on the service\'s machine, as a Hertzforge object '
                'or in a format another tool reads; a format that cannot hold the object is '
                'refused, naming those that can.',
        params=(OBJECT, FORMAT, PATH, CREATOR), outputs=('reading',), run=save_as),
    Operation(
        id='project.rattlesnake_preview', title='Preview a Rattlesnake Run',
        workspace='analyze', accepts='project',
        summary='One channel of a long run over its whole length, the window to import shaded '
                'off the rest, and what importing that window and those channels would cost.',
        params=(RUN, PREVIEWED, STREAM, START, STOP, LAST, CHANNELS),
        outputs=('spectrum', 'reading', 'table'), run=rattlesnake_preview),
    Operation(
        id='project.import_rattlesnake_part', title='Import Part of a Rattlesnake Run',
        workspace='analyze', accepts='project',
        summary='Import a window of a run and a pick of its channels, reading only that part '
                'of the file.',
        params=(RUN, START, STOP, LAST, CHANNELS), outputs=('reading',),
        run=import_rattlesnake_part),
    Operation(
        id='project.truncate_rattlesnake_file', title='Truncate Rattlesnake File',
        workspace='analyze', accepts='project',
        summary='Write a run cut to a window as a .nc4 of its own, with every channel and '
                'setting as the file has them and its clock starting at zero.',
        params=(RUN, START, STOP, LAST, TARGET), outputs=('reading',),
        run=truncate_rattlesnake_file),
    Operation(
        id='project.text_preview', title='Preview Delimited Text', workspace='analyze',
        accepts='project',
        summary='The layout the reader finds in a text file, and the head of the file read '
                'with the corrections given, as the import would read it.',
        params=(TEXT, DELIMITER, DECIMAL, TIME, RATE, COLUMNS, NAMES, UNITS),
        outputs=('reading', 'table', 'table', 'spectrum'), run=text_preview),
    Operation(
        id='project.import_text', title='Import Delimited Text', workspace='analyze',
        accepts='project',
        summary='Import a text file as a time history, with the layout the reader found '
                'corrected where it is wrong.',
        params=(TEXT, DELIMITER, DECIMAL, TIME, RATE, COLUMNS, NAMES, UNITS),
        outputs=('reading',), run=import_text),
)
