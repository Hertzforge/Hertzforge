"""Builders for the viz kit's views (`schema/PROTOCOL.md`, "Views").

Arrays are placed in the view as numpy arrays; the codec (`frames.pack`)
turns each into an {"$array": id} reference on the way out.
"""

from __future__ import annotations

from collections.abc import Sequence
from typing import Any, Literal

import numpy as np

Scale = Literal['log', 'linear']
Role = Literal['measured', 'reference', 'tolerance', 'abort', 'drive', 'other']
Status = Literal['pass', 'warn', 'fail', 'info']
Shape = Literal['line', 'steps', 'law']


def axis(label: str, unit: str | None, scale: Scale) -> dict[str, Any]:
    return {'label': label, 'unit': unit, 'scale': scale}


def series(name: str, role: Role, x: Any, y: Any, shape: Shape = 'line',
           edges: Any = None) -> dict[str, Any]:
    """One curve. f64 is kept for spectra, which are short.

    `shape` is how the points mean to be drawn (schema/PROTOCOL.md, Views);
    'steps' needs `edges`, one more than there are values.
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
    return out


def shaped(name: str, role: Role, data: Any, x: Any, y: Any) -> dict[str, Any]:
    """A curve drawn the way Visual Dynamics draws `data`.

    The shape is VD's own (`plot.drawing_shape`): a density per bin is
    stepped across its bin, with VD's `plot.bin_edges` (the object's own
    band edges when it has them); a breakpoint law is resampled along its
    power law with VD's `plot.as_power_law`; anything else is a line.
    `x` and `y` are the lines to draw, already cut to what is shown.
    """
    from visualdynamics.plot import as_power_law, bin_edges, drawing_shape

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
             link: str | None = None) -> dict[str, Any]:
    out = {'view': 'spectrum', 'title': title, 'x': x, 'y': y,
           'series': list(lines)}
    if link is not None:
        out['link'] = link
    return out


def row(label: str, value: float | str, unit: str | None,
        status: Status) -> dict[str, Any]:
    if isinstance(value, float) and not np.isfinite(value):
        raise ValueError(f'reading {label!r} is {value}; a value that was not '
                         'read is said in words, not sent as a number')
    return {'label': label, 'value': value, 'unit': unit, 'status': status}


def reading(title: str, rows: Sequence[dict]) -> dict[str, Any]:
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


def mode(index: int, frequency_hz: float, damping: float | None,
         shape: np.ndarray) -> dict[str, Any]:
    return {'index': int(index), 'frequency_hz': float(frequency_hz),
            'damping': None if damping is None else float(damping),
            'shape': np.ascontiguousarray(shape, np.float32)}
