"""Points measured with a coordinate measuring arm, read as a `Scan`.

Two files are read. A "Feature Measured Points Table" is what an arm's
software exports for probed features: a header that gives the length
unit, then one block per feature with its name and its points. A point
file is bare `x, y, z` rows, as a laser scan is exported, with nothing
said about units or what any point is.
"""

from __future__ import annotations

import csv
import os
import warnings

import numpy as np

from ..core.scan import Scan
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


def _note(text: str) -> None:
    warnings.warn(text, ImportNote, stacklevel=3)


# ---- the feature table ---------------------------------------------------------

def sniff_table(path: str | os.PathLike) -> bool:
    """Whether a file is a feature measured points table: its first cell
    says so."""
    head = text_head(path, 256)
    return head is not None and head.lstrip('﻿ \t').startswith(TABLE_TITLE)


def load_table(path: str | os.PathLike, length_unit: str | None = None) -> Scan:
    """Read a feature measured points table as a scan with features.

    Parameters
    ----------
    path : str or path-like
        The exported table (``.csv``).
    length_unit : str, optional
        The unit of the coordinates, in place of the one the file's
        ``Length Units`` row names.

    Returns
    -------
    Scan
        Every point, feature by feature in the file's order and, inside
        a feature, in the order the points were measured (the file lists
        them by deviation). `deviation` is the file's ``Dev`` column and
        `properties` its other header rows.

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
    if unit is None:
        _note(f'{base}: the length unit {named!r} is not one Hertzforge knows, so the '
              'coordinates are left as they are until a unit is declared')
    return Scan(table[:, 3:6], length_unit=unit, features=names, feature_of=feature_of,
                deviation=table[:, 2], properties=header)


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


def load_points(path: str | os.PathLike, length_unit: str | None = None) -> Scan:
    """Read a file of bare ``x, y, z`` rows as a scan with no features.

    Parameters
    ----------
    path : str or path-like
        The point file, its columns parted by commas, semicolons, tabs
        or spaces.
    length_unit : str, optional
        The unit of the coordinates, which the file cannot state.
        Without it the scan imports unit-less and the unit can be
        defined later.

    Returns
    -------
    Scan
        The points in file order.

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
    return Scan(points, length_unit=length_unit)
