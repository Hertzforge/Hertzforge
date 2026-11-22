"""Points measured with a coordinate measuring arm, read as a geometry.

Two files are read. A "Feature Measured Points Table" is what an arm's
software exports for probed features: a header that gives the length
unit, then one block per feature with its name and its points. A point
file is bare `x, y, z` rows, as a laser scan is exported, with nothing
said about units or what any point is.

Either way each point is a node. The points of a feature are one
element group of measured-point elements, named as the file names the
feature, so the features show under the geometry and are renamed or
deleted there, and the geometry's coordinate system moves and turns
the whole scan.
"""

from __future__ import annotations

import csv
import os
import warnings
from collections.abc import Sequence

import numpy as np

from ..core.geometry import MEASURED_POINT, Geometry
from .notes import ImportNote
from .sniffing import text_head

#: the first cell of a feature table
TABLE_TITLE = 'Feature Measured Points Table'

#: the length units a feature table names, as Hertzforge spells them
LENGTH_UNITS = {'millimeters': 'mm', 'millimetres': 'mm', 'mm': 'mm',
                'centimeters': 'cm', 'centimetres': 'cm', 'cm': 'cm',
                'meters': 'm', 'metres': 'm', 'm': 'm',
                'inches': 'in', 'in': 'in', 'feet': 'ft', 'ft': 'ft'}

#: the names a bare point file goes by
POINT_SUFFIXES = ('.txt', '.csv', '.xyz', '.asc', '.pts')

#: how many rows of a point file a sniff reads
SNIFF_ROWS = 200


#: the color indices features take in turn (`drawing.viz.geometry.PALETTE`)
FEATURE_COLORS = np.array([4, 2, 3, 6, 8, 9, 5, 7])


def _note(text: str) -> None:
    warnings.warn(text, ImportNote, stacklevel=3)


def points_geometry(points: np.ndarray, length_unit: str | None = None,
                    features: Sequence[str] | None = None,
                    feature_of: np.ndarray | None = None) -> Geometry:
    """A geometry whose nodes are measured points.

    Parameters
    ----------
    points : array_like, [N, 3]
        The points, which become nodes 1 to N in this order.
    length_unit : str, optional
        The unit the points are in; the geometry is unit-less without it.
    features : sequence of str, optional
        The name of each feature the points were measured as.
    feature_of : array_like of int, [N], optional
        The row in `features` each point belongs to, or -1 for none.

    Returns
    -------
    Geometry
        The nodes, and for each feature one element group of its name
        holding a measured-point element at each of its points. The
        groups take colors in turn so neighboring features differ.
    """
    points = np.asarray(points, dtype=np.float64).reshape(-1, 3)
    names = [str(name) for name in (features or [])]
    owner = (np.full(len(points), -1, dtype=np.int64) if feature_of is None
             else np.asarray(feature_of, dtype=np.int64).reshape(-1))
    held = np.flatnonzero(owner >= 0)
    geometry = Geometry(
        node_id=np.arange(1, len(points) + 1), node_xyz=points,
        elem_id=np.arange(1, len(held) + 1),
        elem_type=np.full(len(held), MEASURED_POINT),
        elem_color=FEATURE_COLORS[owner[held] % len(FEATURE_COLORS)],
        elem_conn=[np.array([row + 1]) for row in held],
        elem_group=owner[held] + 1,
        group_id=np.arange(1, len(names) + 1), group_name=names)
    # the constructor takes coordinates already in meters; these are the
    # file's own numbers, converted by declaring their unit
    return geometry if length_unit is None else geometry.define_units(length_unit)


# ---- the feature table ---------------------------------------------------------

def sniff_table(path: str | os.PathLike) -> bool:
    """Whether a file is a feature measured points table: its first cell
    says so."""
    head = text_head(path, 256)
    return head is not None and head.lstrip('﻿ \t').startswith(TABLE_TITLE)


def load_table(path: str | os.PathLike, length_unit: str | None = None) -> Geometry:
    """Read a feature measured points table as a geometry of points in
    named element groups.

    Parameters
    ----------
    path : str or path-like
        The exported table (``.csv``).
    length_unit : str, optional
        The unit of the coordinates, in place of the one the file's
        ``Length Units`` row names.

    Returns
    -------
    Geometry
        Every point as a node, numbered from 1, feature by feature in
        the file's order and, inside a feature, in the order the points
        were measured (the file lists them by deviation). Each feature
        is an element group of its name. The file's deviations and its
        other header rows are not kept.

    Raises
    ------
    ValueError
        When the file holds no points.
    """
    header: dict[str, str] = {}
    names: list[str] = []
    counts: list[int | None] = []
    rows: list[tuple[int, int, float, float, float, float]] = []
    with open(path, newline='', encoding='utf-8-sig', errors='replace') as handle:
        for cells in csv.reader(handle):
            cells = [cell.strip() for cell in cells]
            if not any(cells):
                continue
            key, value = cells[0], cells[1] if len(cells) > 1 else ''
            if key == 'Name':
                names.append(value)
                counts.append(None)
            elif key == 'Feature' and names:
                try:
                    index, dev, x, y, z = int(cells[1]), *(float(c) for c in cells[2:6])
                except (ValueError, IndexError):
                    _note(f'{os.path.basename(str(path))}: a row of {names[-1]} is not '
                          'an index, a deviation and three coordinates, and was left out')
                    continue
                rows.append((len(names) - 1, index, dev, x, y, z))
            elif key == 'No. Pts' and names:
                counts[-1] = int(value) if value.isdigit() else None
            elif key and not names and key != TABLE_TITLE:
                header[key] = value
    if not rows:
        raise ValueError(f'{path} holds no measured points')
    # in the order measured: by feature, then by the point's index
    rows.sort(key=lambda row: (row[0], row[1]))
    table = np.array(rows, dtype=np.float64)
    feature_of = table[:, 0].astype(np.int32)
    base = os.path.basename(str(path))
    for row, (name, stated) in enumerate(zip(names, counts, strict=True)):
        found = int((feature_of == row).sum())
        if stated is not None and stated != found:
            _note(f'{base}: {name} says {stated} points and holds {found}')
    named = header.pop('Length Units', '')
    unit = length_unit or LENGTH_UNITS.get(named.lower())
    if unit is None and not named:
        _note(f'{base}: the file names no length unit, so the coordinates are left as they '
              'are until a unit is declared')
    elif unit is None:
        _note(f'{base}: the length unit {named!r} is not one Hertzforge knows, so the '
              'coordinates are left as they are until a unit is declared')
    return points_geometry(table[:, 3:6], length_unit=unit, features=names,
                           feature_of=feature_of)


# ---- bare points -----------------------------------------------------------------

def _fields(line: str) -> list[str]:
    for mark in (',', ';', '\t'):
        if mark in line:
            return [cell.strip() for cell in line.split(mark)]
    return line.split()


def _delimiter(line: str) -> str | None:
    return next((mark for mark in (',', ';', '\t') if mark in line), None)


def sniff_points(path: str | os.PathLike) -> bool:
    """Whether a file is bare points: a `POINT_SUFFIXES` name whose rows
    are three numbers each, with no header, and whose first column does
    not rise from row to row. A first column that rises is a time or a
    frequency, and that file is delimited data."""
    if not str(path).lower().endswith(POINT_SUFFIXES):
        return False
    head = text_head(path, 16384)
    if head is None:
        return False
    lines = [line for line in head.splitlines()[:-1] if line.strip()][:SNIFF_ROWS]
    if len(lines) < 2:
        return False
    try:
        rows = [[float(cell) for cell in _fields(line)] for line in lines]
    except ValueError:
        return False
    if any(len(row) != 3 for row in rows):
        return False
    first = np.array([row[0] for row in rows])
    return bool(np.isfinite(rows).all()) and not bool((np.diff(first) > 0).all())


def load_points(path: str | os.PathLike, length_unit: str | None = None) -> Geometry:
    """Read a file of bare ``x, y, z`` rows as a geometry of nodes.

    Parameters
    ----------
    path : str or path-like
        The point file, its columns parted by commas, semicolons, tabs
        or spaces.
    length_unit : str, optional
        The unit of the coordinates, which the file cannot state.
        Without it the geometry imports unit-less and the unit can be
        defined later.

    Returns
    -------
    Geometry
        The points as nodes, numbered from 1 in file order, with no
        elements.

    Raises
    ------
    ValueError
        When the rows are not three numbers each.
    """
    with open(path, errors='replace') as handle:
        first = next((line for line in handle if line.strip()), '')
    try:
        points = np.loadtxt(path, delimiter=_delimiter(first), dtype=np.float64, ndmin=2)
    except ValueError as error:
        raise ValueError(f'{path} is not rows of three numbers: {error}') from None
    if points.shape[1] != 3:
        raise ValueError(f'{path} has {points.shape[1]} columns; a point file has three')
    return points_geometry(points, length_unit=length_unit)
