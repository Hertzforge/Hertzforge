"""Drawing decisions from Visual Dynamics' `plot.signature`: what a figure needs
to know, with none of the code that draws it."""

from __future__ import annotations

from typing import Any

import numpy as np


def _shown(values: Any, dimension: str, unit_system: Any) -> np.ndarray:
    """Magnitudes in display units; a record whose units are undeclared
    is drawn as it is."""
    from ...units import UNKNOWN

    values = np.abs(np.asarray(values)).astype(np.float64)
    if dimension == UNKNOWN:
        return values
    return np.asarray(unit_system.from_si(values, dimension), dtype=float)


def display_scale(dimension: str, unit_system: Any = None) -> float:
    """The factor from SI to the units a record of `dimension` is drawn
    in, so a height read in SI lands on the curve as drawn; 1 for a
    record whose units are undeclared."""
    from ...units import DEFAULT_SYSTEM

    us = DEFAULT_SYSTEM if unit_system is None else unit_system
    return float(_shown(np.array([1.0]), dimension, us)[0])


def display_unit(dimension: str, unit_system: Any = None) -> str:
    """The unit a record of `dimension` is drawn in, as plain text ('g/g',
    'g/lbf'); empty for a record whose units are undeclared."""
    from ...units import DEFAULT_SYSTEM, UNKNOWN

    if dimension == UNKNOWN:
        return ''
    us = DEFAULT_SYSTEM if unit_system is None else unit_system
    return us.label_text(dimension)


def percent(value: float) -> str:
    """A shift as the table writes it: '−3.3 %'."""
    return f'{value:+.1f} %'.replace('-', '−')
