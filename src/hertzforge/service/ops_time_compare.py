"""Two sets of time responses compared station by station: TRAC / Overlay.

The test's records and the model's are paired by DOF and quantity and put
on one clock, and every pair is scored over the scoring window
(`core.time_compare`). The reading is the score table, one row a station
with its TRAC, sTRAC, correlation and RMS ratio, the picked station's two
traces drawn over each other with the window marked, and a summary. A
row picked in the table draws that station.

Save adds the comparison to the project (`Project.compare_time_histories`).
A saved comparison picked in the tree draws the same reading, the stations
picked under it overlaid, and Scoring Window keeps a new window on it,
also by dragging the window's edges on the plot.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .descriptors import Operation, Param
from .ops_project import picked_records, picks_param
from .protocol import refused
from .waterfalls import unit_text

TEST = Param('first', 'Test', 'enum', options_from='objects:time_history')
MODEL = Param('second', 'Model', 'enum', options_from='objects:time_history')
COMPARISON = Param('comparison', 'Comparison', 'enum', options_from='objects:time_comparison')
#: the comparison Scoring Window keeps a window on: `source`, so its fields open on the window
#: it holds (`Param.setting`)
WINDOWED = Param('source', 'Comparison', 'enum', options_from='objects:time_comparison')
WHOLE = Param('whole', 'Score the whole shared clock', 'boolean', default=True)
START = Param('start', 'TRAC start', 'number', default=0.0, unit='s', step=0.1,
              shown_with=('whole', False))
STOP = Param('stop', 'TRAC end', 'number', default=0.0, unit='s', step=0.1,
             shown_with=('whole', False))
WINDOW_START = Param('start', 'TRAC start', 'number', default=0.0, unit='s', step=0.1,
                     setting='time_comparison.start')
WINDOW_STOP = Param('stop', 'TRAC end', 'number', default=0.0, unit='s', step=0.1,
                    setting='time_comparison.stop')
#: the station drawn, from 1: set by picking its row in the score table
STATION = Param('station', 'Station', 'integer', default=1, min=1, step=1, in_view=True)
KEEP = Param('keep', 'Save the comparison (else the preview)', 'boolean', default=False)
STATIONS = Param('records', 'Records (blank: all)', 'string', default='')

#: the stations a saved comparison overlays at once when several are picked
DRAWN = 6
#: points a trace is drawn with, its peaks kept
TRACE_POINTS = 4000

#: what the scores leave alone, said where they are read
NOTHING_FITTED = ('Nothing is fitted: no lag is removed, no gain or sign is fitted and no '
                  'filter is applied. A source sampled faster than the shared clock is '
                  'interpolated onto it without an anti-alias filter.')


def _number(value: float | None) -> str:
    return '' if value is None else f'{value:.3f}'


def _trace(name: str, x: np.ndarray, y: np.ndarray, follows: int | None = None) -> dict:
    from hertzforge.decimate import peak_decimate

    return v.series(name, 'measured', *peak_decimate(x, y, TRACE_POINTS), follows=follows)


def comparison_views(title: str, data: Any, stations: list[int], *,
                     select: dict[str, Any] | None = None,
                     drag: dict[str, Any] | None = None) -> list[dict]:
    """The reading of a comparison, saved or not: the picked stations'
    traces overlaid with the scoring window marked, the score table, and
    the summary. `select` is what picking a table row runs, `drag` what
    dragging the window's edges runs."""
    from hertzforge.core.time_compare import scores

    rows = scores(data)
    count = len(rows)
    stations = [k for k in stations if 0 <= k < count] or [0]
    first, second = data.sources[0], data.sources[-1]
    lines: list[dict] = []
    for k in stations[:DRAWN]:
        a, _i, b, _j = data.pairs[k]
        lines.append(_trace(f'{data.response_dof[2 * k]} {a}', data.abscissa,
                            data.ordinate[2 * k]))
        lines.append(_trace(f'{data.response_dof[2 * k + 1]} {b}', data.abscissa,
                            data.ordinate[2 * k + 1], follows=len(lines) - 1))
    unit = unit_text(str(data.ordinate_dim[2 * stations[0]]))
    if len(stations) == 1:
        row = rows[stations[0]]
        heading = (f'{row["dof"]}: TRAC {row["trac"]:.3f}, sTRAC {row["strac"]:.3f}'
                   if row['trac'] is not None else f'{row["dof"]}: not scored, {row["reason"]}')
    else:
        heading = f'{min(len(stations), DRAWN)} of {len(stations)} picked stations'
    window = v.region(data.start, data.stop, 'span', 'Scoring window', drag)
    plot = v.spectrum(f'{heading}, {first} solid and {second} dashed',
                      v.axis('Time', 's', 'linear'), v.axis('Amplitude', unit, 'linear'),
                      lines, link='time', regions=[window])
    cells = []
    for k, row in enumerate(rows):
        cells.append([str(k + 1), row['dof'], row['quantity'].replace('_', ' '),
                      _number(row['trac']), _number(row['strac']),
                      _number(row['correlation']), _number(row['rms_ratio']),
                      row['reason']])
    table = v.table(f'{title}: scores, {data.start:g} s to {data.stop:g} s',
                    ['Station', 'DOF', 'Quantity', 'TRAC', 'sTRAC', 'Correlation', 'RMS ratio',
                     'Note'], cells)
    if select is not None:
        table['select'] = {**select, 'param': 'station', 'column': 0}
    scored = [row for row in rows if row['trac'] is not None]
    summary = [v.row('Window', f'{data.start:g} s to {data.stop:g} s', None, 'info'),
               v.row('Samples in the window', float(rows[0]['samples']), '', 'info'),
               v.row('Stations scored', f'{len(scored)} of {count}', None,
                     'info' if len(scored) == count else 'warn')]
    if scored:
        traces = np.array([row['trac'] for row in scored])
        lowest = scored[int(np.argmin(traces))]
        summary += [v.row('Mean TRAC', float(traces.mean()), '', 'info'),
                    v.row('Lowest TRAC', f'{lowest["trac"]:.3f} at {lowest["dof"]}', None,
                          'info')]
    else:
        summary.append(v.row('Mean TRAC', 'not read: no station could be scored', None, 'warn'))
    summary.append(v.row('Note', NOTHING_FITTED, None, 'info'))
    return [plot, table, v.reading(f'{title}: {first} against {second}', summary)]


def _sets(project: Any, params: dict[str, Any]) -> tuple[dict[str, Any], dict[str, list[int]]]:
    """The two time histories by name, and the records picked under each."""
    first, second = params['first'], params['second']
    if first == second:
        raise refused('pick two time histories, the test and the model')
    records = {}
    for key in ('first', 'second'):
        name = params[key]
        picked = picked_records(project, name, params.get(f'{key}_records', ''))
        if picked is not None:
            records[name] = picked
    return {first: project[first], second: project[second]}, records


def compare_time_histories(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.compare_time_histories`: the test's and the model's
    records paired and scored (`core.time_compare.compare`), previewed;
    with `keep`, the comparison added (`Project.compare_time_histories`)
    and its reading drawn."""
    from hertzforge.core.time_compare import compare

    histories, records = _sets(project, params)
    window = {} if params['whole'] else {'start': params['start'], 'stop': params['stop']}
    if params['keep']:
        try:
            name = project.compare_time_histories(*histories, records=records or None,
                                                  **window)
        except (ValueError, TypeError) as error:
            raise refused(str(error)) from None
        return time_comparison(project, {'comparison': name, 'station': params['station'],
                                         'records': ''})
    try:
        made = compare(histories, records=records or None, **window)
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    select = {'op': 'project.compare_time_histories',
              'params': {key: value for key, value in params.items() if key != 'station'}}
    return comparison_views('Preview', made, [params['station'] - 1], select=select)


def time_comparison(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.time_comparison`: a saved comparison's reading, the
    station picked in its table or the stations picked under it in the
    tree drawn (each station is two records, the test's and the
    model's)."""
    from hertzforge.core.data import TimeComparison

    name = params['comparison']
    data = project[name]
    if not isinstance(data, TimeComparison):
        raise refused(f'{name} is not a time comparison')
    picked = picked_records(project, name, params['records'])
    stations = (sorted({i // 2 for i in picked}) if picked
                else [params['station'] - 1])
    try:
        return comparison_views(
            name, data, stations,
            select={'op': 'project.time_comparison', 'params': {'comparison': name}},
            drag=v.dragging('project.set_time_comparison_window',
                            {'source': name, 'station': stations[0] + 1}, 'start', 'stop'))
    except ValueError as error:
        raise refused(str(error)) from None


def set_time_comparison_window(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_time_comparison_window`: a new scoring window kept on a
    saved comparison (`Project.set_time_comparison_window`), and its
    reading over it."""
    name = params['source']
    try:
        project.set_time_comparison_window(name, params['start'], params['stop'])
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    return time_comparison(project, {'comparison': name, 'station': params['station'],
                                     'records': ''})


OPERATIONS = (
    Operation(
        id='project.compare_time_histories', title='TRAC / Overlay', workspace='transient',
        summary='A test\'s time responses against a model\'s, station by station: each pair '
                'on one clock, drawn over each other and scored by TRAC, sTRAC, correlation '
                'and RMS ratio over a window. Save adds the comparison.',
        params=(TEST, MODEL, picks_param('first'), picks_param('second'), WHOLE, START, STOP,
                STATION, KEEP),
        outputs=('spectrum', 'table', 'reading'), run=compare_time_histories, commits='keep',
        opens_previewed=True),
    Operation(
        id='project.time_comparison', title='Time Comparison', workspace='transient',
        summary='A saved time comparison: its scores, and the stations picked drawn over '
                'each other.',
        params=(COMPARISON, STATION, STATIONS), outputs=('spectrum', 'table', 'reading'),
        run=time_comparison),
    Operation(
        id='project.set_time_comparison_window', title='Scoring Window', workspace='transient',
        summary='The window a saved time comparison is scored over, in seconds inside its '
                'shared clock, typed or dragged on the plot.',
        params=(WINDOWED, WINDOW_START, WINDOW_STOP, STATION),
        outputs=('spectrum', 'table', 'reading'), run=set_time_comparison_window, edits=True),
)
