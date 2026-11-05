"""How a specification and the PSDs judged against it are drawn: the
limit curves' roles and names, the log frequency axis, a curve over the
lines a specification is written on, and a reading's rounding."""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v

#: limit curve -> the line role the viz kit styles it with
ROLES = {'warning_lower': 'tolerance', 'warning_upper': 'tolerance',
         'abort_lower': 'abort', 'abort_upper': 'abort'}
NAMES = {'warning_lower': 'Warning, lower', 'warning_upper': 'Warning, upper',
         'abort_lower': 'Abort, lower', 'abort_upper': 'Abort, upper'}

FREQUENCY = v.axis('Frequency', 'Hz', 'log')


def sig(value: float, digits: int = 4) -> float:
    """A reading rounded for display to `digits` significant figures."""
    value = float(value)
    if value == 0.0 or not np.isfinite(value):
        return value
    return float(f'{value:.{digits}g}')


def quantity_of(dimension: str) -> str:
    """'acceleration**2/frequency' -> 'acceleration': what a PSD record
    measures, for its label."""
    head = dimension.split('/')[0]
    return head.split('**')[0].split('*')[0] or dimension


def positive(y: np.ndarray) -> np.ndarray:
    """Non-positive values as NaN, a gap on a log axis."""
    y = np.array(y, dtype=float)
    y[~(y > 0.0)] = np.nan
    return y


def spec_line(spec: Any, record: int, values: np.ndarray
              ) -> tuple[np.ndarray, np.ndarray]:
    """One curve of a specification over the lines it is written on:
    unwritten lines inside the span are NaN gaps (a controller's zero
    lines ask for nothing), and the ends are trimmed."""
    x = np.asarray(spec.abscissa, dtype=float)
    written = np.asarray(spec.written(record)) & (x > 0.0)
    if not written.any():
        return x[:0], x[:0]
    first, last = np.flatnonzero(written)[[0, -1]]
    y = np.where(written, values, np.nan)[first:last + 1]
    return x[first:last + 1], positive(y)
