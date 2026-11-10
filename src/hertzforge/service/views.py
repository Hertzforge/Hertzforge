"""Builders for the viz kit's views (`schema/PROTOCOL.md`, "Views").

Arrays are placed in the view as numpy arrays; the codec (`frames.pack`)
turns each into an {"$array": id} reference on the way out.
"""

from __future__ import annotations

from collections.abc import Sequence
from typing import Any, Literal

import numpy as np

Scale = Literal['log', 'linear']
Role = Literal['measured', 'reference', 'tolerance', 'abort', 'limit', 'drive', 'other']
Status = Literal['pass', 'warn', 'fail', 'info']
Shape = Literal['line', 'steps', 'law']


def axis(label: str, unit: str | None, scale: Scale) -> dict[str, Any]:
    """A view's axis: its label, unit (None when undefined) and scale."""
    return {'label': label, 'unit': unit, 'scale': scale}


def series(name: str, role: Role, x: Any, y: Any, shape: Shape = 'line',
           edges: Any = None, follows: int | None = None) -> dict[str, Any]:
    """One curve. f64 is kept for spectra, which are short.

    `shape` is how the points mean to be drawn (schema/PROTOCOL.md, Views);
    'steps' needs `edges`, one more than there are values. `follows` makes
    the curve a follower of the view's series at that index: a model's
    prediction of that measurement, drawn dashed in its colour.
    """
    x = np.ascontiguousarray(x, dtype=np.float64)
    y = np.ascontiguousarray(y, dtype=np.float64)
    if x.shape != y.shape or x.ndim != 1:
        raise ValueError(f'series {name!r}: x {x.shape} and y {y.shape} must '
                         'be one-dimensional and the same length')
    out = {'name': name, 'role': role, 'x': x, 'y': y}
    if shape == 'steps':
        edges = np.ascontiguousarray(edges, dtype=np.float64)
        if edges.shape != (y.size + 1,):
            raise ValueError(f'series {name!r}: {y.size} binned values need '
                             f'{y.size + 1} edges, not {edges.shape}')
        out.update(shape='steps', edges=edges)
    elif shape != 'line':
        out['shape'] = shape
    if follows is not None:
        out['follows'] = follows
    return out


def drawable(name: str, role: Any, x: Any, y: Any, positive: bool = False,
             x_above_zero: bool = False) -> dict[str, Any]:
    """A `series` with the points that cannot be drawn left out: not a
    number anywhere, and at or under zero on a log axis, `positive` for
    the values and `x_above_zero` for the abscissa."""
    x, y = np.asarray(x, dtype=float), np.asarray(y, dtype=float)
    keep = np.isfinite(x) & np.isfinite(y)
    if positive:
        keep &= y > 0.0
    if x_above_zero:
        keep &= x > 0.0
    return series(name, role, x[keep], y[keep])


def shaped(name: str, role: Role, data: Any, x: Any, y: Any) -> dict[str, Any]:
    """A curve drawn as `data` calls for.

    The shape is `plot.drawing_shape`'s: a density per bin is
    stepped across its bin, with `plot.bin_edges` (the object's own
    band edges when it has them); a breakpoint law is resampled along its
    power law with `plot.as_power_law`; anything else is a line.
    `x` and `y` are the lines to draw, already cut to what is shown.
    """
    from hertzforge.drawing.plot import as_power_law, bin_edges, drawing_shape

    shape = drawing_shape(data)
    x = np.asarray(x, dtype=float)
    y = np.asarray(y, dtype=float)
    if shape == 'steps' and x.size > 1:
        widths = getattr(data, 'bandwidth', None)
        if widths is not None and np.ndim(widths) == 0:
            widths = np.full(x.size, float(widths))
        elif widths is not None:
            widths = np.asarray(widths, dtype=float)
            full = np.asarray(data.abscissa, dtype=float)
            widths = widths[np.searchsorted(full, x)]
        return series(name, role, x, y, 'steps', bin_edges(x, widths))
    if shape == 'law':
        finite = np.isfinite(y)
        if finite.sum() > 1:
            x, y = as_power_law(x[finite], y[finite])
        return series(name, role, x, y, 'law')
    return series(name, role, x, y)


def spectrum(title: str, x: dict, y: dict, lines: Sequence[dict],
             link: str | None = None,
             regions: Sequence[dict] = (), handles: Sequence[dict] = ()) -> dict[str, Any]:
    """A `spectrum` view of `lines` on the axes `x` and `y`. Panes with
    the same `link` share a cursor position on x; `regions` are shaded
    behind the curves."""
    out = {'view': 'spectrum', 'title': title, 'x': x, 'y': y,
           'series': list(lines)}
    if link is not None:
        out['link'] = link
    if regions:
        out['regions'] = list(regions)
    if handles:
        out['handles'] = list(handles)
    return out


def region(x0: float, x1: float, kind: Literal['frame', 'cut', 'span'],
           label: str | None = None, drag: dict[str, Any] | None = None) -> dict[str, Any]:
    """A span of x behind a spectrum's curves: a `frame` averaged from, a
    `cut` left out, or a `span` (its edges drawn, nothing shaded). With
    `drag` (`dragging`) it is moved on the plot: its middle slides it and
    an edge moves that edge."""
    out: dict[str, Any] = {'x0': float(x0), 'x1': float(x1), 'kind': kind}
    if label:
        out['label'] = label
    if drag is not None:
        out['drag'] = drag
    return out


def dragging(op: str, params: dict[str, Any], low: str, high: str | None = None
             ) -> dict[str, Any]:
    """What makes a region draggable: the operation that sets the span,
    run with `params` and the new x0 as `low` and, when `high` is given,
    the new x1 as `high`. Without `high` only the whole span slides."""
    out: dict[str, Any] = {'op': op, 'params': dict(params), 'low': low}
    if high is not None:
        out['high'] = high
    return out


def picking(op: str, params: dict[str, Any], target: Literal['point', 'nodes', 'faces'],
            hint: str, **fields: Any) -> dict[str, Any]:
    """What a click in a `shape3d` view does: run `op` with `params` and
    what was clicked. A 'point' click sends the place it lands as `x`, `y`
    and `z`, on the model's surface or plane. `ids` is the node id of each
    of the first rows, the model's own nodes. A 'nodes' click picks the
    node under it of those, and `count` of them (0: as many as wanted,
    then Enter) go as `param`, a list such as '4, 7, 9'. A 'faces' click
    toggles the surface its
    triangle belongs to (`faces`, one owner a triangle, -1 for none): a
    surface in `kept` is named in the `out` param, any other in `back`.
    `hint` says what a click does."""
    out: dict[str, Any] = {'op': op, 'params': dict(params), 'target': target, 'hint': hint}
    for key, value in fields.items():
        out[key] = np.asarray(value, np.int32) if key == 'faces' else value
    return out


def row(label: str, value: float | str, unit: str | None,
        status: Status) -> dict[str, Any]:
    """One row of a `reading` view. A non-finite float is a ValueError:
    a value that could not be read is sent as words, never as NaN or
    inf."""
    if isinstance(value, float) and not np.isfinite(value):
        raise ValueError(f'reading {label!r} is {value}; a value that was not '
                         'read is said in words, not sent as a number')
    return {'label': label, 'value': value, 'unit': unit, 'status': status}


def notes(said: Any, status: Status = 'info') -> list[dict]:
    """A `row` per note, each as a sentence; an empty note is no row."""
    return [row('Note', note[0].upper() + note[1:], None, status) for note in said if note]


def reading(title: str, rows: Sequence[dict]) -> dict[str, Any]:
    """A `reading` view: a titled table of `row`s."""
    return {'view': 'reading', 'title': title, 'rows': list(rows)}


def gizmo(origin: Sequence[float], axes: Sequence[Sequence[float]], slide: dict[str, Any],
          turn: dict[str, Any]) -> dict[str, Any]:
    """Arrows and rings at `origin`, along the three `axes`, to drag in a `shape3d` view:
    letting go runs `slide` or `turn`, each {'op', 'params', 'values', 'adds'}: `values` names
    the param a drag along an axis goes in, in meters or degrees, and `adds` (optional) the
    other params that axis adds."""
    return {'origin': [float(v) for v in origin], 'axes': [[float(v) for v in row] for row in axes],
            'slide': slide, 'turn': turn}


def shape3d(title: str, unit: str, nodes: np.ndarray,
            modes: Sequence[dict], triangles: np.ndarray | None = None,
            lines: np.ndarray | None = None,
            **extra: Any) -> dict[str, Any]:
    """A mesh and its modes. Nodes and shapes are f32 [N, 3]; triangles and
    lines u32 [T, 3] and [L, 2]. `extra` fields ride along (the schema
    allows them), such as how the shapes are normalized, or `node_ids`,
    the drawn nodes' ids (u32 [N]) for the view's hover captions."""
    out: dict[str, Any] = {'view': 'shape3d', 'title': title, 'unit': unit,
                           'nodes': np.ascontiguousarray(nodes, np.float32)}
    if extra.get('node_ids') is not None:
        extra['node_ids'] = np.ascontiguousarray(extra['node_ids'], np.uint32)
    if lines is not None and len(lines):
        out['lines'] = np.ascontiguousarray(lines, np.uint32).reshape(-1, 2)
    if triangles is not None and len(triangles):
        out['triangles'] = np.ascontiguousarray(triangles,
                                                np.uint32).reshape(-1, 3)
    out['modes'] = list(modes)
    out.update(extra)
    return out


def image(title: str, fmt: str, data: bytes) -> dict[str, Any]:
    """An `image` view: a photo as its encoded bytes (png or jpeg), never
    re-encoded, titled by its name. The shell draws it at its own aspect."""
    if fmt not in ('png', 'jpeg'):
        raise ValueError(f'an image view is png or jpeg, not {fmt!r}')
    return {'view': 'image', 'title': title, 'format': fmt,
            'bytes': np.frombuffer(bytes(data), dtype=np.uint8)}


def waterfall(title: str, x: dict, z: dict, records: Sequence[str],
              curves: Sequence[tuple[Any, Any]], x_range: tuple[float, float],
              z_range: tuple[float, float] | None = None,
              page: dict[str, int] | None = None) -> dict[str, Any]:
    """A `waterfall` view: each record a polyline at its own depth.

    `curves` is one (x, z) pair per record, front to back, in display
    units and not logged (the shell takes the log when an axis reads
    log); a NaN is a gap. They go out as two flat f32 arrays and the u32
    offsets where each record starts, which is what the shell uploads
    to the GPU as one geometry. `x_range` is the span where something
    is drawn, `z_range` a level range the object pins (a coherence's 0
    to 1.05), and `page` which page of how many this is, of how many
    records."""
    counts = [len(cx) for cx, _cz in curves]
    offsets = np.concatenate([[0], np.cumsum(counts)]).astype(np.uint32)
    xs = [np.asarray(cx, dtype=np.float32) for cx, _cz in curves]
    zs = [np.asarray(cz, dtype=np.float32) for _cx, cz in curves]
    out: dict[str, Any] = {
        'view': 'waterfall', 'title': title, 'x': x, 'z': z,
        'records': [str(label) for label in records],
        'offsets': offsets,
        'x_values': np.concatenate(xs) if xs else np.empty(0, np.float32),
        'z_values': np.concatenate(zs) if zs else np.empty(0, np.float32),
        'x_range': [float(x_range[0]), float(x_range[1])],
    }
    if z_range is not None:
        out['z_range'] = [float(z_range[0]), float(z_range[1])]
    if page is not None:
        out['page'] = dict(page)
    return out


def staged(view: dict[str, Any], stations: Sequence[str], depths: Sequence[int],
           roles: Sequence[str], zones: Sequence[dict] = (), exceed: Sequence[dict] = (),
           walls: Sequence[Any] = (), surface: bool = False) -> dict[str, Any]:
    """A `waterfall` view turned into one of the stages: its
    records stand at named `stations` (`depths`, one station index a
    record, so two records can share one) and are drawn by `roles` (a
    'measured' record colored by level, any other in its role's color).

    `zones` are a station's limit bands, each {station, kind 'warning' or
    'abort', side 'upper' or 'lower', x, inner, outer}, with no `outer`
    when the band runs to the stage's top or floor. `exceed` are the lines
    a measurement went past an abort limit, each {station, side, x0, x1,
    z}: one rectangle a line from its limit to the top or the floor.
    `walls` are curves of one abscissa a station, drawn as translucent
    walls from floor to top (a scalogram's cone of influence). `surface`
    draws the records as one surface, every record on the same abscissa."""
    view['stations'] = [str(s) for s in stations]
    view['depths'] = [int(d) for d in depths]
    view['roles'] = list(roles)
    if zones:
        view['zones'] = [{**z, **{k: np.asarray(z[k], np.float32)
                                  for k in ('x', 'inner', 'outer') if k in z}} for z in zones]
    if exceed:
        view['exceed'] = [{**e, **{k: np.asarray(e[k], np.float32) for k in ('x0', 'x1', 'z')}}
                          for e in exceed]
    if len(walls):
        view['walls'] = [np.asarray(w, np.float32) for w in walls]
    if surface:
        view['surface'] = True
    return view


def pathed(view: dict[str, Any], y: dict, y_values: Any, y_range: tuple[float, float],
           roles: Sequence[str], zones: Sequence[dict] = (),
           exceed: Sequence[dict] = ()) -> dict[str, Any]:
    """A `waterfall` view whose records are paths: each point at a depth of
    its own on the axis `y` (`y_values`, one a point, `y_range` the span),
    rather than at its record's station, as the sine stage places a tone
    through time. `zones` carry `y` beside `x` and `exceed` carries `y0`
    and `y1`, so the bands and the rectangles follow the path."""
    view['y'] = dict(y)
    view['y_values'] = np.ascontiguousarray(y_values, dtype=np.float32)
    if view['y_values'].shape != view['x_values'].shape:
        raise ValueError('a path needs one depth a point')
    view['y_range'] = [float(y_range[0]), float(y_range[1])]
    view['roles'] = list(roles)
    if zones:
        view['zones'] = [{**z, **{k: np.asarray(z[k], np.float32)
                                  for k in ('x', 'y', 'inner', 'outer') if k in z}} for z in zones]
    if exceed:
        view['exceed'] = [{**e, **{k: np.asarray(e[k], np.float32)
                                   for k in ('x0', 'x1', 'y0', 'y1', 'z')}} for e in exceed]
    return view


def mode(index: int, frequency_hz: float, damping: float | None,
         shape: np.ndarray, label: str | None = None) -> dict[str, Any]:
    """One mode of a `shape3d` view: its 1-based index, frequency,
    damping (None when not stated) and its [N, 3] shape. A complex shape
    travels as its real part in `shape` and its imaginary part in
    `shape_imag`, both f32; a real one, or a complex one whose imaginary
    part is all zero, carries `shape` alone. `label` names a shape that is
    not a mode, shown in place of "Mode <index>"."""
    out = {'index': int(index), 'frequency_hz': float(frequency_hz),
           'damping': None if damping is None else float(damping),
           'shape': np.ascontiguousarray(np.real(shape), np.float32)}
    if np.iscomplexobj(shape) and np.any(np.imag(shape)):
        out['shape_imag'] = np.ascontiguousarray(np.imag(shape), np.float32)
    if label is not None:
        out['label'] = label
    return out


def matrix(title: str, rows: dict[str, Any], columns: dict[str, Any],
           values: np.ndarray, value_label: str,
           value_range: tuple[float, float]) -> dict[str, Any]:
    """A `matrix` view: one value per (row, column) cell, f32 [R, C], drawn
    as a colored grid. `rows` and `columns` are each `{"label", "ticks"}`,
    one tick per row or column. `value_range` pins the color scale, so two
    matrices of the same quantity read alike."""
    values = np.ascontiguousarray(values, np.float32)
    return {'view': 'matrix', 'title': title,
            'rows': {'label': rows['label'], 'ticks': list(rows['ticks'])},
            'columns': {'label': columns['label'],
                        'ticks': list(columns['ticks'])},
            'values': values.reshape(len(rows['ticks']), len(columns['ticks'])),
            'value_label': value_label,
            'value_range': [float(value_range[0]), float(value_range[1])]}


def table(title: str, columns: Sequence[str], rows: Sequence[Sequence[str]],
          edit: dict[str, Any] | None = None) -> dict[str, Any]:
    """A `table` view: rows of text under column headings, one cell per
    column. With `edit` ({'op', 'params', 'columns'}, see `editing`) its
    cells are typed into in place."""
    columns = [str(c) for c in columns]
    rows = [[str(cell) for cell in row] for row in rows]
    for row in rows:
        if len(row) != len(columns):
            raise ValueError(f'table {title!r}: a row of {len(row)} cells under '
                             f'{len(columns)} columns')
    out = {'view': 'table', 'title': title, 'columns': columns, 'rows': rows}
    if edit is not None:
        if len(edit['columns']) != len(columns):
            raise ValueError(f'table {title!r}: {len(edit["columns"])} edit keys for '
                             f'{len(columns)} columns')
        out['edit'] = edit
    return out


def editing(op: str, params: dict[str, Any], columns: Sequence[str | None],
            choices: dict[str, list[str]] | None = None,
            kinds: dict[str, str] | None = None) -> dict[str, Any]:
    """What makes a `table` view's cells editable: the operation that sets
    one (taking `row` from 1, `column` and `value` beside `params`), each
    column's key, or None for a column that is read only, what a column
    offers by key, a shortlist the cell can still be typed past, and how a
    column is edited (`kinds`, by key: 'choice' from its list, 'flag' by a
    check box, 'date' in a date field, 'unit' from a shortlist, 'number'
    or 'text' typed)."""
    out = {'op': op, 'params': dict(params), 'columns': list(columns)}
    if choices:
        out['choices'] = {key: list(values) for key, values in choices.items()}
    if kinds:
        out['kinds'] = dict(kinds)
    return out
