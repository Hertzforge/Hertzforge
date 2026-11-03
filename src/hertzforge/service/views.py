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


def shaped(name: str, role: Role, data: Any, x: Any, y: Any) -> dict[str, Any]:
    """A curve drawn the way Visual Dynamics draws `data`.

    The shape is VD's own (`plot.drawing_shape`): a density per bin is
    stepped across its bin, with VD's `plot.bin_edges` (the object's own
    band edges when it has them); a breakpoint law is resampled along its
    power law with VD's `plot.as_power_law`; anything else is a line.
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
             regions: Sequence[dict] = ()) -> dict[str, Any]:
    """A `spectrum` view of `lines` on the axes `x` and `y`. Panes with
    the same `link` share a cursor position on x; `regions` are shaded
    behind the curves."""
    out = {'view': 'spectrum', 'title': title, 'x': x, 'y': y,
           'series': list(lines)}
    if link is not None:
        out['link'] = link
    if regions:
        out['regions'] = list(regions)
    return out


def region(x0: float, x1: float, kind: Literal['frame', 'cut'],
           label: str | None = None) -> dict[str, Any]:
    """A span of x shaded behind a spectrum's curves: a `frame` averaged
    from, or a `cut` left out."""
    out: dict[str, Any] = {'x0': float(x0), 'x1': float(x1), 'kind': kind}
    if label:
        out['label'] = label
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


def reading(title: str, rows: Sequence[dict]) -> dict[str, Any]:
    """A `reading` view: a titled table of `row`s."""
    return {'view': 'reading', 'title': title, 'rows': list(rows)}


def shape3d(title: str, unit: str, nodes: np.ndarray,
            modes: Sequence[dict], triangles: np.ndarray | None = None,
            lines: np.ndarray | None = None,
            **extra: Any) -> dict[str, Any]:
    """A mesh and its modes. Nodes and shapes are f32 [N, 3]; triangles and
    lines u32 [T, 3] and [L, 2]. `extra` fields ride along (the schema
    allows them), such as how the shapes are normalized."""
    out: dict[str, Any] = {'view': 'shape3d', 'title': title, 'unit': unit,
                           'nodes': np.ascontiguousarray(nodes, np.float32)}
    if lines is not None and len(lines):
        out['lines'] = np.ascontiguousarray(lines, np.uint32).reshape(-1, 2)
    if triangles is not None and len(triangles):
        out['triangles'] = np.ascontiguousarray(triangles,
                                                np.uint32).reshape(-1, 3)
    out['modes'] = list(modes)
    out.update(extra)
    return out


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


def table(title: str, columns: Sequence[str], rows: Sequence[Sequence[str]]) -> dict[str, Any]:
    """A `table` view: rows of text under column headings, one cell per
    column."""
    columns = [str(c) for c in columns]
    rows = [[str(cell) for cell in row] for row in rows]
    for row in rows:
        if len(row) != len(columns):
            raise ValueError(f'table {title!r}: a row of {len(row)} cells under '
                             f'{len(columns)} columns')
    return {'view': 'table', 'title': title, 'columns': columns, 'rows': rows}
