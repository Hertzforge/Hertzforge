"""What an object holds, for the project tree's expansions.

The shell's tree opens an object into what it contains from this answer
(`project.children`). A
geometry opens into its nodes, its coordinate systems, its elements by
family with the element groups under each, its solids and its mated
surfaces; its Nodes and Coordinate systems rows and each element group's
row list what they hold, the first `LISTED` of a large one. A data object
opens into a grid, even one column wide: rows are a DOF and a quantity,
columns are what tells one record from another (the reference, the capture,
or nothing), and each cell names its record, whose icon says what it
measures. A shape set opens into its modes, a channel table and a sine
object into their channels, photographs into their names, matched modes
into their pairs, and any other object that holds a structure into `items`
(a set of predictions, a report's blocks). `opens` says whether an object
opens at all, which is the shell's only word on it.

Nothing here is drawn. The icon of a record is a key the shell draws
from: a quantity ('acceleration'), two as a fraction, response over
reference ('acceleration/force'), or '' for the object's own icon.
"""

from __future__ import annotations

from collections import Counter
from typing import Any

import numpy as np

#: the quantities the shell has a glyph for, as `known_dim` names them
QUANTITIES = frozenset((
    'length', 'strain', 'pressure', 'velocity', 'acceleration', 'force', 'temperature',
    'voltage', 'angle', 'angular_velocity', 'angular_acceleration', 'moment',
    'modal_length', 'modal_velocity', 'modal_acceleration', 'modal_force'))

SOLID_LABELS = {'block': 'Block', 'cylinder': 'Cylinder', 'cone': 'Cone', 'sphere': 'Sphere',
                'plate': 'Plate', 'disk': 'Disk', 'union': 'Union', 'subtract': 'Subtraction',
                'intersect': 'Intersection', 'pattern': 'Pattern', 'hole': 'Hole'}


def record_icon(data: Any, index: int) -> str:
    """What record `index` measures, as the shell's icon key: a quantity,
    a fraction of two ('acceleration/force', response over reference) or
    '' when the quantities have no glyph (a coherence, an undeclared
    channel), which leaves the object's own icon. A `dimension_hint` is
    enough to choose: a source that said "force" without a unit has still
    said what the record is."""
    from hertzforge.core.data import channel_quantities

    response, reference = channel_quantities(data.known_dim(index))
    if response in QUANTITIES and reference in QUANTITIES and response != reference:
        return f'{response}/{reference}'
    if response in QUANTITIES:
        return response
    return ''


def grid(data: Any) -> dict[str, Any]:
    """A data object's records. A row is a DOF and a quantity:
    two channels can sit at one point, but a volt and a newton never share
    a row. When even that cannot tell channels apart (nothing declared, so
    every quantity is unknown) the channel order breaks the tie, and the
    row's label says so ('101Z+ #2'). Columns are the reference DOFs of a
    matrix of measurements, the captures of a repeated one, or one blank
    column, which is also what records that are each their own reference
    (ASDs) get. A set that does not fill its rectangle keeps its holes (-1).
    Time data has a role per row."""
    from hertzforge.core.data import channel_quantities

    count = int(data.num_records)
    factors = [channel_quantities(data.known_dim(i)) for i in range(count)]
    quantities = [(str(dof), factors[i][0]) for i, dof in enumerate(data.response_dof)]
    # every record its own response against itself (an ASD, a specification of autos):
    # the references tell nothing apart, so they make no columns. A column a reference
    # laid eight ASDs out as an eight by eight diagonal (Brandon, 2026-11-17).
    autos = (data.reference_dof is not None and count > 0
             and all(str(response) == str(reference) for response, reference
                     in zip(data.response_dof, data.reference_dof, strict=True)))
    if autos:
        identities: list[Any] = [''] * count
        labels = {'': ''}
    elif data.reference_dof is not None:
        identities = [(str(dof), factors[i][1])
                      for i, dof in enumerate(data.reference_dof)]
        labels = {identity: identity[0] for identity in identities}
    else:
        keys = data.column_keys()
        identities = [str(k) for k in keys] if keys is not None else [''] * count
        labels = {identity: identity for identity in identities}
    seen: Counter = Counter()
    row_keys = []
    for pair, column in zip(quantities, identities, strict=True):
        row_keys.append((*pair, seen[(pair, column)]))
        seen[(pair, column)] += 1
    rows: list[tuple] = []
    columns: list[Any] = []
    row_index: dict[tuple, int] = {}
    column_index: dict[Any, int] = {}
    cells: dict[tuple[int, int], int] = {}
    for i, (key, identity) in enumerate(zip(row_keys, identities, strict=True)):
        if key not in row_index:
            row_index[key] = len(rows)
            rows.append(key)
        if identity not in column_index:
            column_index[identity] = len(columns)
            columns.append(identity)
        cells[(row_index[key], column_index[identity])] = i
    table = [[cells.get((r, c), -1) for c in range(len(columns))] for r in range(len(rows))]
    out: dict[str, Any] = {
        'kind': 'grid',
        'rows': [{'label': dof + (f' #{occurrence + 1}' if occurrence else ''),
                  'dof': dof, 'quantity': quantity}
                 for dof, quantity, occurrence in rows],
        'columns': [labels[identity] for identity in columns],
        'cells': table,
        'icons': [record_icon(data, i) for i in range(count)],
        'undefined': [i for i in range(count) if data.ordinate_dim[i] == 'unknown'],
    }
    if hasattr(data, 'channel_roles'):
        # a role is the channel's, the same in every capture, so any
        # record in the row names it
        resolved = data.channel_roles()
        by_row: dict[int, str] = {}
        for (row, _column), index in cells.items():
            by_row.setdefault(row, resolved[(data.response_dof[index],
                                             data.ordinate_dim[index])])
        for r, row in enumerate(out['rows']):
            row['role'] = by_row.get(r, 'response')
    return out


#: the most entries a row of a geometry lists under it: the nodes of its Nodes row, the
#: elements of an element group
LISTED = 2000


def listed_elements(geometry: Any) -> dict[int, list[int]]:
    """The elements each element group's row lists, by group id: the rows of the first
    `LISTED` of its elements, in the geometry's order. The tree lists them and the 3-D
    view marks them one at a time, so both read them here."""
    listed: dict[int, list[int]] = {}
    for row, group in enumerate(geometry.elem_group):
        held = listed.setdefault(int(group), [])
        if len(held) < LISTED:
            held.append(row)
    return listed


def _geometry(geometry: Any) -> dict[str, Any]:
    """A geometry's parts as the tree lists them: nodes and coordinate
    systems with their counts, the nodes with `ids` (the first `LISTED`) and
    the systems with `ids` and `rows`, a line a system; every element family,
    the six always and points too (a ground or a spring to ground starts from
    one), pyramids when there are any, each with the element groups holding
    elements of it, a group with the `elements` its row lists; the solids, a
    boolean's operands under it; the mated surfaces."""
    from hertzforge.core import solids
    from hertzforge.core.geometry import (
        CS_TYPES,
        FAMILIES,
        FAMILY_LABELS,
        RARE_FAMILIES,
        element_family,
    )

    families: dict[str, dict[int, int]] = {}
    for code, group in zip(geometry.elem_type, geometry.elem_group, strict=True):
        held = families.setdefault(element_family(int(code)), {})
        held[int(group)] = held.get(int(group), 0) + 1
    # an empty element group is listed under the family it was made for
    for group in (int(g) for g in geometry.group_id):
        made_for = geometry.group_family.get(group)
        if made_for and not any(group in held for held in families.values()):
            families.setdefault(made_for, {})[group] = 0
    # each group's elements, the first of a large one, listed when its row is opened
    members = {group: [int(geometry.elem_id[row]) for row in rows]
               for group, rows in listed_elements(geometry).items()}
    systems = [f'CS {int(cs)}' + (f', {str(name).strip()}' if str(name).strip() else '')
               + f' ({CS_TYPES.get(int(kind), "cartesian")})'
               for cs, name, kind in zip(geometry.cs_id, geometry.cs_name, geometry.cs_type,
                                         strict=True)]
    listed = [*FAMILIES, *(f for f in RARE_FAMILIES if f == 'points' or f in families)]
    names = {int(g): str(n).strip() for g, n in
             zip(geometry.group_id, geometry.group_name, strict=True)}
    family_rows = []
    for family in listed:
        groups = families.get(family, {})
        family_rows.append({
            'id': family, 'label': FAMILY_LABELS[family],
            'count': sum(groups.values()),
            'groups': [{'id': group, 'label': names.get(group) or f'Element group {group}',
                        'count': groups[group], 'elements': members.get(group, [])}
                       for group in (int(g) for g in geometry.group_id) if group in groups]})

    def solid_row(solid: Any) -> dict[str, Any]:
        label = solid.name.strip() or f'{SOLID_LABELS.get(solid.kind, solid.kind.capitalize())} {solid.id}'
        return {'id': int(solid.id), 'label': label, 'kind': solid.kind,
                'children': [solid_row(solids.solid(geometry, operand))
                             for operand in solid.operands]}

    return {
        'kind': 'geometry',
        'parts': [{'id': 'nodes', 'label': 'Nodes', 'count': len(geometry.node_id),
                   # listed under the row when it is opened; a model's hundred thousand
                   # are read in its table
                   'ids': [int(n) for n in geometry.node_id[:LISTED]]},
                  {'id': 'coordinate_systems', 'label': 'Coordinate systems',
                   'count': len(geometry.cs_id), 'rows': systems,
                   'ids': [int(cs) for cs in geometry.cs_id]}],
        'families': family_rows,
        'solids': [solid_row(s) for s in solids.top_level(geometry)],
        'mates': [{'id': int(m.id), 'label': f'{m.name} ({m.describe()})', 'name': m.name}
                  for m in getattr(geometry, 'mates', ())],
    }


def children(project: Any, name: str) -> dict[str, Any]:
    """What the object `name` holds, by what it is; `{'kind': 'none'}` for
    one that opens into nothing (an empty report, a run log extract)."""
    from hertzforge.core.channel_table import ChannelTable
    from hertzforge.core.data import DataArray
    from hertzforge.core.geometry import Geometry
    from hertzforge.core.matches import MatchedModes
    from hertzforge.core.photos import Photos
    from hertzforge.core.shapes import ShapeSet

    obj = project[name]
    if isinstance(obj, Geometry):
        return _geometry(obj)
    if isinstance(obj, DataArray):
        return grid(obj)
    if isinstance(obj, ShapeSet):
        descriptions = list(getattr(obj, 'description', None) or [])
        return {'kind': 'modes', 'modes': [
            {'index': k + 1, 'frequency': float(obj.frequency[k]),
             'damping': float(obj.damping[k]),
             'description': str(descriptions[k]) if k < len(descriptions) else ''}
            for k in range(int(obj.num_shapes))]}
    if isinstance(obj, ChannelTable):
        types = [t if isinstance(t, str) else '' for t in obj.frame['channel_type']]
        return {'kind': 'channels', 'channels': [
            {'dof': dof, 'quantity': quantity if quantity in QUANTITIES else ''}
            for dof, quantity in zip(obj.dof_strings(), types, strict=True)]}
    if isinstance(obj, Photos):
        return {'kind': 'photos', 'photos': list(obj.names)}
    from hertzforge.core.sine import SineLevelSet, SineSweepSpecification

    if isinstance(obj, (SineSweepSpecification, SineLevelSet)):
        # the rows under a sine object are its control DOFs (2026-09-25:
        # the tones were the rows once, which inverted the rest of the tree)
        from .ops_sine_shock import sine_dofs

        quantity = (obj.ordinate_dim if isinstance(obj, SineSweepSpecification)
                    else next((str(level.ordinate_dim[0]) for level in obj.levels
                               if level.num_records), ''))
        return {'kind': 'channels', 'channels': [
            {'dof': dof, 'quantity': quantity if quantity in QUANTITIES else ''}
            for dof in sine_dofs(obj)]}
    if isinstance(obj, MatchedModes):
        return {'kind': 'matches', 'matches': [
            {'first': int(first) + 1, 'second': int(second) + 1, 'mac': float(mac)}
            for (first, second), mac in zip(obj.pairs, obj.macs, strict=True)]}
    from hertzforge.core.prediction import NOUN, Predictions
    from hertzforge.core.report import Report

    if isinstance(obj, Predictions) and len(obj.predictions):
        return {'kind': 'items', 'items': [
            {'label': str(each.channel), 'detail': NOUN.get(each.quantity, each.quantity)}
            for each in obj.predictions]}
    if isinstance(obj, Report) and obj.blocks:
        return {'kind': 'items', 'items': [_block_row(block) for block in obj.blocks]}
    return {'kind': 'none'}


def _block_row(block: Any) -> dict[str, str]:
    """A report's block as its row in the tree: its kind, and its caption, the object it
    draws or its first words."""
    words = block.get('caption') or block.get('source') or ' '.join(
        str(block.get('text', '')).split())
    return {'label': str(block.get('kind', 'block')).capitalize(),
            'detail': words if len(words) <= 60 else words[:57].rstrip() + '...'}


def opens(obj: Any) -> bool:
    """Whether an object opens in the tree: it holds a structure of items, which
    `children` lists. The rule is the object's, not a list of kinds the shell keeps: one
    with items beneath it opens, whatever it is (Brandon, 2026-11-17).

    Parameters
    ----------
    obj : object
        An object of the project.

    Returns
    -------
    bool
        True when `children` answers anything but `{'kind': 'none'}` for it.
    """
    from hertzforge.core.channel_table import ChannelTable
    from hertzforge.core.data import DataArray
    from hertzforge.core.geometry import Geometry
    from hertzforge.core.matches import MatchedModes
    from hertzforge.core.photos import Photos
    from hertzforge.core.prediction import Predictions
    from hertzforge.core.report import Report
    from hertzforge.core.shapes import ShapeSet
    from hertzforge.core.sine import SineLevelSet, SineSweepSpecification

    if isinstance(obj, Predictions):
        return bool(len(obj.predictions))
    if isinstance(obj, Report):
        return bool(obj.blocks)
    return isinstance(obj, (Geometry, DataArray, ShapeSet, ChannelTable, Photos,
                            SineSweepSpecification, SineLevelSet, MatchedModes))


def banded(obj: Any) -> bool:
    """Whether a PSD or a specification is on proportional bands, which
    changes its icon as it changes its shape."""
    bandwidth = getattr(obj, 'bandwidth', None)
    return bandwidth is not None and not (isinstance(bandwidth, float) and np.isnan(bandwidth))
