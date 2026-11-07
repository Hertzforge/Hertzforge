"""Declaring the units a file did not state, as Visual Dynamics' Imported
Units pane does: by channel, not by record.

A geometry has one length unit and a shape set one mass unit. A data
object's records are grouped into channels by (DOF, quantity), since a
drive point's accelerometer and load cell share a DOF and are two
channels, and one channel's unit applies to every record it is in. An FRF
has two tables, its response channels and its reference channels; a
cross-spectral matrix has one, the channels on either side of its terms.
A record converts when every channel it is made of is named, and one only
half named waits, its named half held on the object until the other comes
"""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any

from hertzforge.core.data import UNKNOWN, DataArray, Psd, channel_quantities
from hertzforge.core.geometry import Geometry
from hertzforge.core.shapes import ShapeSet
from hertzforge.core.unit_choices import (
    LENGTH_UNITS,
    MASS_UNITS,
    REFERENCE_UNITS,
    shown_dimension,
    units_for,
)
from hertzforge.io.stated_units import unit_problem
from hertzforge.units import dimension_of, normalize_unit

from .protocol import bad_request, not_found, refused


def _quantities(data: DataArray, record: int) -> tuple[str | None, str | None]:
    """(response, reference) quantity of a record: its dimension's, or the
    file's claim when its units are not declared."""
    dim = data.ordinate_dim[record]
    if dim == UNKNOWN:
        dim = (data.dimension_hint[record] or '') if data.dimension_hint else ''
    if not dim:
        return None, None
    response, reference = channel_quantities(dim)
    return response or None, reference or None


def _sides(data: DataArray) -> list[str]:
    """The tables a data object's channels are declared in."""
    if data.reference_dof is None:
        return ['channels']
    if isinstance(data, Psd):
        return ['channels']
    return ['response', 'reference']


def _key(dof: str, quantity: str | None) -> str:
    return f'{dof}|{quantity or ""}'


def tables(project: Any, name: str) -> dict[str, Any]:
    """`project.units`: the tables an object's units are declared in, each
    row a channel (or the object itself) with its unit, None while it is
    not declared, and the units offered first for it."""
    if name not in project:
        raise not_found(f'the project has no object named {name!r}')
    obj = project[name]
    if isinstance(obj, Geometry):
        rows = [_row('length', 'Coordinates', 'length',
                     obj.length_unit if obj.units_defined else None, LENGTH_UNITS)]
        return {'name': name, 'tables': [{'side': 'object', 'rows': rows}]}
    if isinstance(obj, ShapeSet):
        rows = [_row('mass', 'Normalized against', 'mass',
                     obj.mass_unit if obj.units_defined else None, MASS_UNITS)]
        return {'name': name, 'tables': [{'side': 'object', 'rows': rows}]}
    if not isinstance(obj, DataArray):
        raise refused(f'{name} carries no units to declare')
    found: dict[str, dict[str, dict[str, Any]]] = {side: {} for side in _sides(obj)}
    held = {**(obj.pending_units or {}), **_held(obj)}
    for channels in _channels(obj):
        for side, dof, quantity in channels:
            key = _key(dof, quantity)
            if key in found[side]:
                continue
            offered = (units_for(quantity, REFERENCE_UNITS) if side == 'reference'
                       and quantity is None else units_for(quantity))
            found[side][key] = _row(key, dof, quantity, held.get((side, key)), offered)
    return {'name': name, 'tables': [{'side': side, 'rows': list(rows.values())}
                                     for side, rows in found.items()]}


def _channels(data: DataArray) -> list[list[tuple[str, str, str | None]]]:
    """Each record's channels as (side, DOF, quantity). A record not yet
    converted says no quantity, and one of its channels may already say it on
    another record: a CPSD's diagonal converts before its cross term. It is
    the same channel when the DOF has only that one quantity on that side, or
    one sensor would read as two rows."""
    records = []
    said: dict[tuple[str, str], set[str]] = {}
    for i in range(data.num_records):
        response, reference = _quantities(data, i)
        pairs = [(_side(data, 'response'), data.response_dof[i], response)]
        if data.reference_dof is not None:
            pairs.append((_side(data, 'reference'), data.reference_dof[i], reference))
        for side, dof, quantity in pairs:
            if quantity:
                said.setdefault((side, dof), set()).add(quantity)
        records.append(pairs)

    def named(side: str, dof: str, quantity: str | None) -> str | None:
        known = said.get((side, dof), ())
        return next(iter(known)) if not quantity and len(known) == 1 else quantity

    return [[(side, dof, named(side, dof, quantity)) for side, dof, quantity in pairs]
            for pairs in records]


def _held(data: DataArray) -> dict[tuple[str, str], str]:
    """Each channel's declared unit, from the records already converted: a
    channel's unit applies to every record it is in."""
    out: dict[tuple[str, str], str] = {}
    for i, channels in enumerate(_channels(data)):
        if data.ordinate_dim[i] == UNKNOWN:
            continue
        side, dof, quantity = channels[0]
        out.setdefault((side, _key(dof, quantity)), data.ordinate_unit[i])
        if len(channels) > 1 and data.reference_unit[i]:
            side, dof, quantity = channels[1]
            out.setdefault((side, _key(dof, quantity)), data.reference_unit[i])
    return out


def _side(data: DataArray, which: str) -> str:
    return 'channels' if _sides(data) == ['channels'] else which


def _row(key: str, label: str, quantity: str | None, unit: str | None,
         choices: Any) -> dict[str, Any]:
    return {'key': key, 'label': label,
            'quantity': shown_dimension(quantity) if quantity else None,
            'unit': unit, 'choices': list(choices)}


def define(project: Any, name: str, given: Any) -> list[int]:
    """`project.define_units`: the units named, by table and row, declared
    on the object; the records converted. Every unit is read before
    anything changes, so a unit that does not parse leaves the object as
    it was. A data record converts when each of its channels is named
    (here or already), and waits otherwise."""
    if name not in project:
        raise not_found(f'the project has no object named {name!r}')
    if not isinstance(given, list) or not given or not all(
            isinstance(entry, Mapping) and isinstance(entry.get('side'), str)
            and isinstance(entry.get('key'), str) and isinstance(entry.get('unit'), str)
            and entry['unit'].strip() for entry in given):
        raise bad_request('project.define_units "units" is a list of '
                          '{"side", "key", "unit"}, each unit a name')
    known = {(table['side'], row['key']) for table in tables(project, name)['tables']
             for row in table['rows']}
    named: dict[tuple[str, str], str] = {}
    for entry in given:
        where = (entry['side'], entry['key'])
        if where not in known:
            raise bad_request(f'{name} has no {entry["side"]} row {entry["key"]!r}')
        unit = entry['unit'].strip()
        if dimension_of(normalize_unit(unit)) is None:
            raise refused(f'{unit!r} is not a unit Hertzforge can read')
        if isinstance(project[name], DataArray) and (problem := unit_problem(unit)):
            raise refused(f'{unit!r} {problem}')
        named[where] = unit
    obj = project[name]
    try:
        if isinstance(obj, (Geometry, ShapeSet)):
            unit = named[('object', 'length' if isinstance(obj, Geometry) else 'mass')]
            obj.define_units(unit)
            project.record_call(name, 'define_units', unit)
            return []
        return _define_records(project, name, obj, named)
    except ValueError as error:
        raise refused(str(error)) from None


def _define_records(project: Any, name: str, data: DataArray,
                    named: dict[tuple[str, str], str]) -> list[int]:
    # named before and not yet converted: no record holds the unit, so it
    # waits with the object for the call that names the channel beside it
    held = {**(data.pending_units or {}), **_held(data)}
    units: dict[int, str] = {}
    references: dict[int, str] = {}
    channels = _channels(data)
    keys_of = [[(side, _key(dof, quantity)) for side, dof, quantity in pairs]
               for pairs in channels]
    for i, keys in enumerate(keys_of):
        found = [named.get(key, held.get(key)) for key in keys]
        # a record converts when this call names one of its channels and
        # every one of them is named, now or before
        if not any(key in named for key in keys) or not all(found):
            continue
        units[i] = found[0]
        if len(found) > 1:
            references[i] = found[1]
    if units:
        data.define_units(units, reference_units=references or None)
        project.record_call(name, 'define_units', units,
                            reference_units=references or None)
    waiting = {key for i, keys in enumerate(keys_of) if i not in units
               and data.ordinate_dim[i] == UNKNOWN for key in keys}
    data.pending_units = {key: unit
                          for key, unit in {**(data.pending_units or {}), **named}.items()
                          if key in waiting} or None
    return sorted(units)
