"""Two sets of time responses compared station by station: TRAC / Overlay.

The test's records and the model's are paired by DOF and quantity and put
on one clock, and every pair is scored over the scoring window
(`core.time_compare`). The reading is the score table, one row a station
with its TRAC, sTRAC, correlation and RMS ratio, the picked station's two
traces drawn over each other with the window marked, and a summary. A
row picked in the table draws that station.

The two sets are two time histories, or every history of two object groups
picked together (`Project.time_history_sets`), as when each channel of a
file is a history of its own and each file's histories are a group. Swap
makes the second set the test.

Save adds the comparison to the project (`Project.compare_time_histories`).
A saved comparison picked in the tree draws the same reading, the stations
picked under it overlaid, and Scoring Window keeps a new window on it,
also by dragging the window's edges on the plot.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .descriptors import Operation, Option, Param
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
#: the rest of each set when the histories of two groups are compared, one name a line: filled
#: from the tree (`Project.time_history_sets`) and left off the form
FIRST_MORE = Param('first_more', 'More test histories', 'string', default='')
SECOND_MORE = Param('second_more', 'More model histories', 'string', default='')
SWAP = Param('swap', 'Swap test and model', 'boolean', default=False)
PAIRING = Param('pairing', 'Pair records', 'enum', default='dof',
                options=(Option('dof', 'By DOF and quantity'),
                         Option('order', 'In record order')))
ROW = Param('row', 'Pair', 'integer', default=1, min=1, step=1)
COLUMN = Param('column', 'Column', 'string', default='second')
VALUE = Param('value', 'Value', 'string', default='')
IDS = Param('ids', 'Pairs, as 1-3, 7', 'string', default='')
STATIONS = Param('records', 'Records (blank: all)', 'string', default='')
#: the bar's 2D: the picked stations overlaid; otherwise every station on the 3-D stage
FLAT = Param('flat', 'Overlay (else the stage)', 'boolean', default=True, in_view=True)

#: the stations a saved comparison overlays at once when several are picked
DRAWN = 6
#: points a trace is drawn with, its peaks kept
TRACE_POINTS = 4000
#: points the stage draws in all, its traces sharing them
STAGE_POINTS = 240_000

#: what the scores leave alone, said where they are read
NOTHING_FITTED = ('Nothing is fitted: no lag is removed, no gain or sign is fitted and no '
                  'filter is applied. A source sampled faster than the shared clock is '
                  'interpolated onto it without an anti-alias filter.')


def _number(value: float | None) -> str:
    return '' if value is None else f'{value:.3f}'


def _trace(name: str, x: np.ndarray, y: np.ndarray, follows: int | None = None) -> dict:
    from hertzforge.decimate import peak_decimate

    return v.series(name, 'measured', *peak_decimate(x, y, TRACE_POINTS), follows=follows)


def side_label(project: Any, names: list[str]) -> str:
    """What one side of a comparison is called: its one history's name,
    or the name of the group its histories share (an imported file's), or
    how many histories it is."""
    if len(names) == 1:
        return names[0]
    groups = [next((g for g in project.object_groups if name in g['members']), None)
              for name in names]
    if groups[0] is not None and all(g is groups[0] for g in groups):
        return str(groups[0].get('name') or groups[0].get('role') or f'{len(names)} histories')
    return f'{len(names)} histories'


def labels_of(project: Any, data: Any) -> tuple[str, str]:
    """The two sides of a comparison as `side_label` names them, from its pairs."""
    firsts = list(dict.fromkeys(a for a, _i, _b, _j in data.pairs))
    seconds = list(dict.fromkeys(b for _a, _i, b, _j in data.pairs))
    return side_label(project, firsts), side_label(project, seconds)


def comparison_stage(data: Any, rows: list[dict], labels: tuple[str, str],
                     stations: list[int]) -> dict:
    """Every station of a comparison at once on the 3-D stage: time across,
    stations receding, amplitude up, the test's trace colored by level and
    the model's in the reference gray at the same depth, each station named
    with its TRAC. The stations picked are the ones drawn when more than one
    is picked; a quantity other than the first station's is left out, since
    the stage has one amplitude axis."""
    from hertzforge.decimate import peak_decimate

    picked = stations if len(stations) > 1 else list(range(len(rows)))
    dimension = str(data.ordinate_dim[2 * picked[0]])
    drawn = [k for k in picked if str(data.ordinate_dim[2 * k]) == dimension]
    budget = max(64, min(TRACE_POINTS, STAGE_POINTS // max(1, 2 * len(drawn))))
    names, curves, records, depths, roles = [], [], [], [], []
    for depth, k in enumerate(drawn):
        row = rows[k]
        score = f'TRAC {row["trac"]:.3f}' if row['trac'] is not None else 'not scored'
        names.append(f'{row["dof"]}  {score}')
        for offset, side, role in ((1, labels[1], 'reference'), (0, labels[0], 'measured')):
            curves.append(peak_decimate(data.abscissa, data.ordinate[2 * k + offset],
                                        budget))
            records.append(f'{side}: {data.response_dof[2 * k + offset]}')
            depths.append(depth)
            roles.append(role)
    view = v.waterfall(f'{labels[0]} and {labels[1]}, every station',
                       v.axis('Time', 's', 'linear'),
                       v.axis('Amplitude', unit_text(dimension), 'linear'), records, curves,
                       (float(data.abscissa[0]), float(data.abscissa[-1])))
    left = len(picked) - len(drawn)
    if left:
        view['note'] = (f'{left} station{"s" * (left != 1)} of another quantity left out; '
                        f'pick them to draw them.')
    return v.staged(view, names, depths, roles)


def comparison_views(title: str, data: Any, stations: list[int], labels: tuple[str, str], *,
                     select: dict[str, Any] | None = None,
                     drag: dict[str, Any] | None = None, flat: bool = True) -> list[dict]:
    """The reading of a comparison, saved or not: the picked stations'
    traces overlaid with the scoring window marked, the score table, and
    the summary. `labels` name the test's side and the model's,
    `select` is what picking a table row runs, `drag` what dragging the
    window's edges runs. Without `flat` the stage of every station
    (`comparison_stage`) takes the overlay's place."""
    from hertzforge.core.time_compare import scores

    rows = scores(data)
    count = len(rows)
    stations = [k for k in stations if 0 <= k < count] or [0]
    first, second = labels
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
    shown = plot if flat else comparison_stage(data, rows, labels, stations)
    return [shown, table, v.reading(f'{title}: {first} against {second}', summary)]


def _names(text: str) -> list[str]:
    return [line.strip() for line in text.splitlines() if line.strip()]


def _sets(project: Any, params: dict[str, Any]
          ) -> tuple[list[str], list[str], dict[str, list[int]], tuple[str, str]]:
    """The test's histories and the model's by name, the records picked
    under each, and the two sides' labels. Two histories, or the histories
    of two groups (`first_more` and `second_more`), are exchanged when
    `swap` says so."""
    tests = [params['first'], *_names(params.get('first_more', ''))]
    models = [params['second'], *_names(params.get('second_more', ''))]
    if set(tests) & set(models):
        raise refused('pick two time histories, the test and the model')
    missing = [name for name in tests + models if name not in project]
    if missing:
        raise refused(f'the project has no object named {missing[0]!r}')
    records = {}
    for key, names in (('first', tests), ('second', models)):
        picked = picked_records(project, names[0], params.get(f'{key}_records', ''))
        if picked is not None:
            records[names[0]] = picked
    if params.get('swap'):
        tests, models = models, tests
    return tests, models, records, (side_label(project, tests), side_label(project, models))


def compare_time_histories(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.compare_time_histories`: the test's and the model's
    records paired and scored (`core.time_compare.compare`), previewed;
    with `keep`, the comparison added (`Project.compare_time_histories`)
    and its reading drawn."""
    from hertzforge.core.time_compare import compare, pairs_in_order

    tests, models, records, labels = _sets(project, params)
    histories = {name: project[name] for name in tests + models}
    window = {} if params['whole'] else {'start': params['start'], 'stop': params['stop']}
    pairs = None
    if params['pairing'] == 'order':
        try:
            pairs = pairs_in_order({n: project[n] for n in tests},
                                   {n: project[n] for n in models}, records or None)
        except (ValueError, TypeError) as error:
            raise refused(str(error)) from None
    if params['keep']:
        try:
            name = project.compare_time_histories(*histories, records=records or None,
                                                  pairs=pairs, **window)
        except (ValueError, TypeError) as error:
            raise refused(str(error)) from None
        return time_comparison(project, {'comparison': name, 'station': params['station'],
                                         'records': '', 'flat': True})
    try:
        made = compare(histories, records=records or None, pairs=pairs, **window)
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    select = {'op': 'project.compare_time_histories',
              'params': {key: value for key, value in params.items() if key != 'station'}}
    return comparison_views('Preview', made, [params['station'] - 1], labels, select=select)


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
            name, data, stations, labels_of(project, data), flat=params['flat'],
            select={'op': 'project.time_comparison',
                    'params': {'comparison': name, 'flat': params['flat']}},
            drag=v.dragging('project.set_time_comparison_window',
                            {'source': name, 'station': stations[0] + 1}, 'start', 'stop'))
    except ValueError as error:
        raise refused(str(error)) from None


def _record_labels(project: Any, names: list[str]) -> dict[str, tuple[str, int]]:
    """Each response record of `names` by the text a pair's cell shows: its
    DOF and its history, and its record number where one history holds two
    at a DOF."""
    from hertzforge.core.time_compare import response_records

    out: dict[str, tuple[str, int]] = {}
    for name in names:
        data = project[name]
        held = response_records(data)
        dofs = [data.response_dof[i] for i in held]
        for i in held:
            text = f'{data.response_dof[i]} · {name}'
            if dofs.count(data.response_dof[i]) > 1:
                text += f', record {i + 1}'
            out[text] = (name, i)
    return out


def pairs_table(project: Any, name: str) -> dict:
    """A comparison's pairs as a table to edit (Edit Pairs): one row a
    station, its first record and its second, each chosen from the
    response records of its side's histories; rows are removed with
    Delete."""
    data = project[name]
    firsts = list(dict.fromkeys(a for a, _i, _b, _j in data.pairs))
    seconds = list(dict.fromkeys(b for _a, _i, b, _j in data.pairs))
    first_labels, second_labels = _record_labels(project, firsts), _record_labels(project, seconds)
    shown = {value: text for text, value in {**first_labels, **second_labels}.items()}
    rows = [[str(k + 1), shown.get((a, i), f'{a}, record {i + 1}'),
             shown.get((b, j), f'{b}, record {j + 1}')]
            for k, (a, i, b, j) in enumerate(data.pairs)]
    labels = labels_of(project, data)
    edit = v.editing('project.set_time_comparison_pair', {'source': name},
                     [None, 'first', 'second'],
                     choices={'first': list(first_labels), 'second': list(second_labels)},
                     kinds={'first': 'choice', 'second': 'choice'})
    edit['remove'] = {'op': 'project.remove_time_comparison_pairs', 'params': {'source': name},
                      'column': 0}
    return v.table(f'{name}: pairs', ['Station', f'First ({labels[0]})',
                                      f'Second ({labels[1]})'], rows, edit)


def edit_time_comparison_pairs(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.edit_time_comparison_pairs`: a saved comparison's pairs, a
    table whose cells pick another record and whose rows are deleted."""
    return [pairs_table(project, params['source'])]


def set_time_comparison_pair(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_time_comparison_pair`: one record of one pair chosen
    anew (`Project.set_time_comparison_pairs`); a record another pair
    holds changes places with this pair's."""
    name = params['source']
    data = project[name]
    k = params['row'] - 1
    if not 0 <= k < len(data.pairs):
        raise refused(f'{name} has no pair {params["row"]}')
    if params['column'] not in ('first', 'second'):
        raise refused(f'a pair has a first and a second record, not {params["column"]!r}')
    side = [a if params['column'] == 'first' else b for a, _i, b, _j in data.pairs]
    labels = _record_labels(project, list(dict.fromkeys(side)))
    chosen = labels.get(params['value'].strip())
    if chosen is None:
        raise refused(f'{params["value"]!r} is not a response record of that side; pick one '
                      f'from the list')
    pairs = [list(pair) for pair in data.pairs]
    at = slice(0, 2) if params['column'] == 'first' else slice(2, 4)
    # a record another pair holds changes places with this pair's, so a pairing
    # is put right one cell at a time
    other = next((m for m, pair in enumerate(pairs) if m != k and tuple(pair[at]) == chosen),
                 None)
    if other is not None:
        pairs[other][at] = pairs[k][at]
    pairs[k][at] = list(chosen)
    try:
        project.set_time_comparison_pairs(name, pairs)
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    return [pairs_table(project, name)]


def remove_time_comparison_pairs(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.remove_time_comparison_pairs`: pairs taken out of a saved
    comparison, by station number; one pair at least stays."""
    from .parsing import parse_picks

    name = params['source']
    data = project[name]
    try:
        gone = set(parse_picks(params['ids'], len(data.pairs), 'pair'))
    except ValueError as error:
        raise refused(str(error)) from None
    kept = [pair for k, pair in enumerate(data.pairs) if k not in gone]
    if not kept:
        raise refused('a comparison keeps one pair at least; delete the comparison instead')
    try:
        project.set_time_comparison_pairs(name, kept)
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    return [pairs_table(project, name)]


TEST_GEOMETRY = Param('geometry', 'Test geometry', 'enum', options_from='objects:geometry')
SCORE = Param('score', 'Score', 'enum', default='trac',
              options=(Option('trac', 'TRAC'), Option('strac', 'sTRAC')))
DRAW = Param('draw', 'Draw', 'enum', default='points',
             options=(Option('points', 'Points at the stations'),
                      Option('nearest', 'Contour on the FE model, nearest station'),
                      Option('smooth', 'Contour on the FE model, smoothed')))
FEM = Param('fem', 'FE model', 'enum', options_from='objects:geometry', default='')
#: the color scale of each score: TRAC is 0 to 1; sTRAC's ideal is 1, the middle of 0 to 2
SCALES = {'trac': (0.0, 1.0), 'strac': (0.0, 2.0)}
#: a station drawn without a score, and the test geometry's lines over a contour
UNSCORED = (140, 140, 140)
#: how see-through a contour's model is, so the stations over it show
CONTOUR_OPACITY = 0.6


def _viridis(values: np.ndarray, low: float, high: float) -> np.ndarray:
    """`values` on the app's one color scale (`drawing.theme.VIRIDIS`), u8 RGB,
    clipped to `low` to `high`."""
    from hertzforge.drawing.theme import VIRIDIS

    stops = np.asarray(VIRIDIS, dtype=float)
    where = np.clip((np.asarray(values, dtype=float) - low) / (high - low), 0.0, 1.0)
    at = where * (len(stops) - 1)
    lower = np.minimum(at.astype(int), len(stops) - 2)
    frac = (at - lower)[:, None]
    return np.round(stops[lower] * (1 - frac) + stops[lower + 1] * frac).astype(np.uint8)


def time_comparison_map(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.time_comparison_map`: a saved comparison's scores on the
    test's geometry, each station a point at its node colored by its TRAC
    or sTRAC, or carried onto a finite element model as a see-through
    contour under the stations (`core.time_compare.spread_scores`, the
    nearest station's score or an inverse-distance-squared mean), with the
    mean over the stations drawn in the note."""
    from hertzforge.core.data import TimeComparison
    from hertzforge.core.time_compare import scores, spread_scores, station_positions

    from .mesh import drawn_mesh
    from .ops_geometry import _drawn
    from .show import _home

    name, geometry_name = params['comparison'], params['geometry']
    data = project[name]
    if not isinstance(data, TimeComparison):
        raise refused(f'{name} is not a time comparison')
    if params['draw'] == 'points' and params['fem']:
        raise refused('points are drawn on the test geometry alone; leave FE model blank, or '
                      'draw a contour')
    if params['draw'] != 'points' and not params['fem']:
        raise refused('pick the FE model the contour is drawn on')
    rows = scores(data)
    key, label = params['score'], 'TRAC' if params['score'] == 'trac' else 'sTRAC'
    where, found = station_positions(data, project[geometry_name])
    if not found:
        raise refused(f'none of the stations\' nodes is in {geometry_name}: link the '
                      f'comparison with the test\'s geometry')
    values = np.array([np.nan if rows[k][key] is None else rows[k][key] for k in found])
    scored = np.isfinite(values)
    low, high = SCALES[key]
    colors = np.tile(np.asarray(UNSCORED, np.uint8), (len(found), 1))
    colors[scored] = _viridis(values[scored], low, high)
    stations = {'xyz': np.ascontiguousarray(where, np.float32), 'colors': colors, 'size': 14}
    if params['draw'] == 'points':
        view = _drawn(f'{label} of {name} on {geometry_name}', project[geometry_name])
        view.pop('element_colors', None)
    else:
        fem = project[params['fem']]
        view = _drawn(f'{label} of {name} on {params["fem"]}', fem)
        if not scored.any():
            raise refused('no station has a score to draw a contour from')
        held = spread_scores(where[scored], values[scored],
                             np.asarray(fem.node_xyz, dtype=float).reshape(-1, 3),
                             smooth=params['draw'] == 'smooth')
        triangles = np.asarray(view.get('triangles', np.zeros((0, 3))), dtype=np.int64)
        triangles = triangles[(triangles < len(held)).all(axis=1)]
        if not len(triangles):
            raise refused(f'{params["fem"]} has no faces to draw a contour on')
        view['element_colors'] = {
            'triangles': triangles.astype(np.uint32),
            'triangle_colors': _viridis(held[triangles].mean(axis=1), low, high),
            'lines': np.zeros((0, 2), np.uint32), 'line_colors': np.zeros((0, 3), np.uint8)}
        view['opacity'] = CONTOUR_OPACITY
        # the test geometry's lines over the contour, so the stations read on their structure
        test = project[geometry_name]
        used, _triangles, lines = drawn_mesh(test)
        xyz = np.asarray(test.node_xyz, dtype=float).reshape(-1, 3)
        if len(lines):
            ends = xyz[used[lines].reshape(-1)]
            view['segments'] = [{'xyz': np.ascontiguousarray(ends, np.float32),
                                 'color': list(UNSCORED)}]
    view['points'] = [*view.get('points', []), stations]
    said = (f'Mean {label} {float(values[scored].mean()):.3f} over {int(scored.sum())} '
            f'station{"s" * (int(scored.sum()) != 1)}' if scored.any() else
            f'No station has a {label}')
    said += f'; colored from {low:g} (purple) to {high:g} (yellow), unscored gray'
    missing = len(rows) - len(found)
    if missing:
        said += f'; {missing} station{"s" * (missing != 1)} not on {geometry_name}'
    view['note'] = said + '.'
    _home(project, [view], [geometry_name])
    return [view]


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
                                     'records': '', 'flat': True})


OPERATIONS = (
    Operation(
        id='project.compare_time_histories', title='TRAC / Overlay', workspace='transient',
        summary='A test\'s time responses against a model\'s, station by station: each pair '
                'on one clock, drawn over each other and scored by TRAC, sTRAC, correlation '
                'and RMS ratio over a window. Two histories, or every history of two groups '
                '(an imported file\'s each). Save adds the comparison.',
        params=(TEST, MODEL, picks_param('first'), picks_param('second'), FIRST_MORE,
                SECOND_MORE, SWAP, PAIRING, WHOLE, START, STOP, STATION, KEEP),
        outputs=('spectrum', 'table', 'reading'), run=compare_time_histories, commits='keep',
        opens_previewed=True),
    Operation(
        id='project.time_comparison', title='Time Comparison', workspace='transient',
        summary='A saved time comparison: its scores, and the stations picked drawn over '
                'each other.',
        params=(COMPARISON, STATION, STATIONS, FLAT),
        outputs=('spectrum', 'waterfall', 'table', 'reading'),
        run=time_comparison),
    Operation(
        id='project.set_time_comparison_window', title='Scoring Window', workspace='transient',
        summary='The window a saved time comparison is scored over, in seconds inside its '
                'shared clock, typed or dragged on the plot.',
        params=(WINDOWED, WINDOW_START, WINDOW_STOP, STATION),
        outputs=('spectrum', 'table', 'reading'), run=set_time_comparison_window, edits=True),
    Operation(
        id='project.edit_time_comparison_pairs', title='Edit Pairs', workspace='transient',
        summary='A saved time comparison\'s pairs as a table: each cell picks another record '
                'of its side, and Delete takes pairs out; the comparison is worked out again.',
        params=(WINDOWED,), outputs=('table',), run=edit_time_comparison_pairs),
    Operation(
        id='project.set_time_comparison_pair', title='Set Comparison Pair',
        workspace='transient',
        summary='One record of one pair of a saved time comparison chosen anew.',
        params=(WINDOWED, ROW, COLUMN, VALUE), outputs=('table',),
        run=set_time_comparison_pair, edits=True),
    Operation(
        id='project.remove_time_comparison_pairs', title='Remove Comparison Pairs',
        workspace='transient',
        summary='Pairs taken out of a saved time comparison, by station number.',
        params=(WINDOWED, IDS), outputs=('table',), run=remove_time_comparison_pairs,
        edits=True),
    Operation(
        id='project.time_comparison_map', title='Score Map', workspace='transient',
        summary='A saved time comparison\'s TRAC or sTRAC drawn on the test\'s geometry, a '
                'colored point at each station, or as a see-through contour on a finite '
                'element model under them, with the mean over the stations.',
        params=(COMPARISON, TEST_GEOMETRY, SCORE, DRAW, FEM), outputs=('shape3d',),
        run=time_comparison_map),
)
