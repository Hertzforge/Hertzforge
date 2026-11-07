"""The display unit system: the views the service sends, drawn in SI by
every operation, put into the units the connection has chosen, as Visual
Dynamics' unit selector puts every plot and reading into its system.

A view says its units as text (`waterfalls.unit_text`, SI's label for a
dimension), so the dimension is found again from that text (`dimension_of`)
over the dimensions the core names, their squares and densities per hertz,
and the ratios and products of two (an FRF, a CPSD). A value is then taken
out of SI by the system's own transform (`UnitSystem.transform`: SI =
value * scale + offset) and labeled by it. Text the map does not hold, and
anything without a unit, is left as it is.

A sentence holds its numbers where nothing can find them again, so the
operation writes it in the system to begin with: the server runs it under
`running_in`, and it asks `words_system` or `words_in`.

Converted: a `spectrum`'s axes, its series (and a stepped series' edges)
and regions; a `reading`'s rows; a `waterfall`'s axes, values and stage marks; a
`table`'s columns headed "<name> [<SI unit>]", each number in them, except
a column typed into (`edit`), which stays in SI so a value typed means what
the operation reads. A `shape3d`'s coordinates are put in the system's
length unit, as Visual Dynamics draws a geometry (`display_points`), and a
measured motion's values in their own; its shapes are a picture and stay as
they are, and a click that places a point (`pick`) says how to take its
coordinates back to SI. A geometry whose units are undefined is drawn in
its file's own numbers. A `matrix` carries its own words.
"""

from __future__ import annotations

import re
from collections.abc import Callable
from contextvars import ContextVar
from functools import cache
from typing import Any

import numpy as np

#: the system every view is computed in: nothing to convert
DEFAULT = 'm-kg-N-s'

#: the system the operation running now writes its sentences in
_WORDS: ContextVar[str] = ContextVar('display_units_words', default=DEFAULT)


def _start() -> str:
    from hertzforge.units import DEFAULT_SYSTEM

    return DEFAULT_SYSTEM.name


#: the system a connection starts in: Visual Dynamics' default
START = _start()


def systems() -> list[str]:
    """The unit systems a connection may choose, the core's, by name."""
    from hertzforge.units import SYSTEMS

    return list(SYSTEMS)


@cache
def _dimensions() -> dict[str, str]:
    """{SI label: dimension} over every dimension a view's unit can be:
    the core's simple dimensions first, so a label two share keeps the
    simple one, then their squares and densities, then the ratios and
    products of two."""
    import hertzforge as hz
    from hertzforge.units import _DIMENSIONALITY

    simple = [d for d in _DIMENSIONALITY if d != 'dimensionless']
    candidates = list(simple)
    candidates += [f'{d}**2' for d in simple] + [f'{d}**2/frequency' for d in simple]
    candidates += ['length**3']
    candidates += [f'{d}/frequency' for d in simple]
    candidates += [f'{a}/{b}' for a in simple for b in simple if a != b]
    candidates += [f'{a}*{b}/frequency' for a in simple for b in simple if a != b]
    out: dict[str, str] = {}
    for dimension in candidates:
        label = _label(hz.SI, dimension)
        if label and label not in out:
            out[label] = dimension
    return out


def _label(system: Any, dimension: str) -> str | None:
    """`system`'s label for `dimension`, or None for a combination the core
    does not label."""
    try:
        return system.label_text(dimension)
    except Exception:  # noqa: BLE001 - any refusal means the same: no label
        return None


def dimension_of(unit: str | None) -> str | None:
    """The dimension whose SI label is `unit`, or None."""
    return None if not unit else _dimensions().get(unit)


def _system(name: str) -> Any:
    from hertzforge.units import SYSTEMS

    return SYSTEMS[name]


def running_in(name: str, call: Callable[..., Any], *args: Any) -> Any:
    """`call(*args)` with its sentences in the system `name`. Called on the
    thread `call` runs on, so nothing depends on a context being carried
    across to a worker."""
    token = _WORDS.set(name)
    try:
        return call(*args)
    finally:
        _WORDS.reset(token)


def words_system() -> Any:
    """The `UnitSystem` an operation's sentences are written in; SI, as its
    numbers are, outside `running_in`."""
    return _system(_WORDS.get())


def words_in(dimension: str) -> tuple[str, float]:
    """(label, scale) for writing a value in SI of `dimension` in a
    sentence: the system's label, and what takes the value into it."""
    system = words_system()
    return _label(system, dimension) or '', 1.0 / float(system.transform(dimension)[0])


def _change(unit: str | None, system: Any) -> tuple[float, float, str] | None:
    """(scale, offset, new label) taking a value in SI `unit` into
    `system`, or None when the unit is not one the map knows or does not
    change."""
    dimension = dimension_of(unit)
    if dimension is None:
        return None
    scale, offset = system.transform(dimension)
    label = system.label_text(dimension)
    if scale == 1.0 and offset == 0.0 and label == unit:
        return None
    return float(scale), float(offset), label


def _values(values: Any, change: tuple[float, float, str]) -> Any:
    scale, offset, _label = change
    array = np.asarray(values)
    return ((array - offset) / scale).astype(array.dtype, copy=False)


def _axis(axis: dict, system: Any) -> tuple[float, float, str] | None:
    change = _change(axis.get('unit'), system)
    if change is not None:
        axis['unit'] = change[2]
    return change


#: a table column's heading that states its unit: "X [m]"
HEADING = re.compile(r'^(.*) \[(.+)\]$')


def _cell(text: str, change: tuple[float, float, str]) -> str:
    try:
        number = float(text)
    except ValueError:
        return text
    return f'{float(_values(number, change)):.6g}'


def _table(view: dict, system: Any) -> None:
    edited = (view.get('edit') or {}).get('columns') or []
    for k, heading in enumerate(view['columns']):
        found = HEADING.match(heading)
        if not found or (k < len(edited) and edited[k]):
            continue
        change = _change(found.group(2), system)
        if change is None:
            continue
        view['columns'][k] = f'{found.group(1)} [{change[2]}]'
        for row in view['rows']:
            row[k] = _cell(row[k], change)


def length_of(name: str) -> dict[str, Any]:
    """What the shell labels and converts a typed length by in the system `name`."""
    system = _system(name)
    return {'unit': system.label_text('length'), 'to_si': float(system.transform('length')[0])}


#: a number in a text of lengths; a piece's id before its colon ('3: 0.01') is not one
_TYPED = re.compile(r'(\d+\s*:)|([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)')


def typed_lengths(params: Any, values: dict[str, Any], name: str, geometry: Any) -> dict[str, Any]:
    """`values` with each `length` param taken from the system `name` to meters. An
    undefined geometry's numbers are its own, and pass as they are."""
    if not geometry.units_defined or name == DEFAULT:
        return values
    scale = float(_system(name).transform('length')[0])

    def scaled(found: re.Match) -> str:
        return found.group(1) or repr(float(found.group(2)) * scale)

    out = dict(values)
    for param in params:
        if param.length:
            held = out[param.name]
            out[param.name] = _TYPED.sub(scaled, held) if isinstance(held, str) else held * scale
    return out


#: a length said in words: '0.2 m', not '0.2 m²' or '1e+06 N/m'
_SAID = re.compile(r'(?<![\w.])(-?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?) m\b')


def lengths_in_words(text: str, name: str) -> str:
    """`text` with each length said in meters said in the system `name`."""
    if name == DEFAULT:
        return text
    length = length_of(name)
    return _SAID.sub(lambda found: f'{float(found.group(1)) / length["to_si"]:.4g} {length["unit"]}',
                     text)


def say_lengths(views: list[dict], name: str) -> None:
    """The lengths in a reading's words and in a solids table's cells, in the system `name`."""
    for view in views:
        if view.get('view') == 'reading':
            for row in view['rows']:
                if isinstance(row['value'], str):
                    row['value'] = lengths_in_words(row['value'], name)
        elif view.get('view') == 'table':
            spoken = [k for k, heading in enumerate(view['columns'])
                      if heading in ('Dimensions', 'Center')]
            for row in view['rows']:
                for k in spoken:
                    row[k] = lengths_in_words(row[k], name)


def _shape(view: dict, system: Any) -> None:
    change = _change(view.get('unit'), system)
    if change is not None:
        view['nodes'] = _values(view['nodes'], change)
        view['unit'] = change[2]
        marks = view.get('marks')
        if marks is not None:
            # the arrows and sensors over the model, in its coordinates
            marks['nodes'] = np.asarray(_values(marks['nodes'], change), dtype=np.float32)
        gizmo = view.get('gizmo')
        if gizmo is not None:
            gizmo['origin'] = [float((v - change[1]) / change[0]) for v in gizmo['origin']]
            if not gizmo['slide'].get('in_display'):
                gizmo['to_si'] = change[0]
        pick = view.get('pick')
        if pick is not None and pick.get('target') == 'point':
            pick['to_si'] = [change[0], change[1]]
    motion = view.get('motion')
    if motion is not None:
        values = _change(motion.get('value_unit'), system)
        if values is not None:
            motion['values'] = _values(motion['values'], values)
            motion['peak'] = abs(float(_values(motion['peak'], values)))
            motion['value_unit'] = values[2]


def convert(views: list[dict], name: str) -> list[dict]:
    """`views` put into the system `name`, in place; returned for chaining.
    SI leaves them untouched."""
    if name == DEFAULT:
        return views
    system = _system(name)
    for view in views:
        kind = view.get('view')
        if kind == 'spectrum':
            x = _axis(view['x'], system)
            y = _axis(view['y'], system)
            for line in view['series']:
                if x is not None:
                    line['x'] = _values(line['x'], x)
                    if 'edges' in line:
                        line['edges'] = _values(line['edges'], x)
                if y is not None:
                    line['y'] = _values(line['y'], y)
            if x is not None:
                for region in view.get('regions', []):
                    region['x0'] = float(_values(region['x0'], x))
                    region['x1'] = float(_values(region['x1'], x))
        elif kind == 'reading':
            for row in view['rows']:
                change = _change(row.get('unit'), system)
                # words are in the unit they were written in, and a label
                # moved without them would say a different number
                if change is None or isinstance(row['value'], str):
                    continue
                row['unit'] = change[2]
                row['value'] = float(_values(float(row['value']), change))
        elif kind == 'table':
            _table(view, system)
        elif kind == 'shape3d':
            _shape(view, system)
        elif kind == 'waterfall':
            x = _axis(view['x'], system)
            z = _axis(view['z'], system)
            if x is not None:
                view['x_values'] = _values(view['x_values'], x)
                view['x_range'] = [float(_values(v, x)) for v in view['x_range']]
            if x is not None:
                for region in view.get('regions', []):
                    region['x0'] = float(_values(region['x0'], x))
                    region['x1'] = float(_values(region['x1'], x))
            if z is not None:
                view['z_values'] = _values(view['z_values'], z)
                if 'z_range' in view:
                    view['z_range'] = [float(_values(v, z)) for v in view['z_range']]
            for zone in view.get('zones', []):
                if x is not None:
                    zone['x'] = _values(zone['x'], x)
                if z is not None:
                    for key in ('inner', 'outer'):
                        if key in zone:
                            zone[key] = _values(zone[key], z)
            for hit in view.get('exceed', []):
                if x is not None:
                    hit['x0'], hit['x1'] = _values(hit['x0'], x), _values(hit['x1'], x)
                if z is not None:
                    hit['z'] = _values(hit['z'], z)
            if x is not None and 'walls' in view:
                view['walls'] = [_values(wall, x) for wall in view['walls']]
    return views
