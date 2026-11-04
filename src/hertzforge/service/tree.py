"""What an object holds, for the project tree's expansions.

Visual Dynamics' tree opens an object into what it contains, and the
shell's tree does the same from this answer (`project.children`). A
geometry opens into its nodes, its coordinate systems, its elements by
family with the element groups under each, its solids and its mated
surfaces (`gui.main_window._build_children`). Every object that holds
more than one thing opens into a grid, even one column wide
(`gui.record_grid`): rows are a DOF and a quantity, columns are what
tells one record from another (the reference, the capture, or nothing),
and each cell names its record, whose icon says what it measures
(`gui.icons.record_icon`). A shape set opens into its modes, a channel
table into its channels, photographs into their names.

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
    """A data object's records as Visual Dynamics' grid plans them
    (`gui.record_grid.row_keys`, `plan`). A row is a DOF and a quantity:
    two channels can sit at one point, but a volt and a newton never share
    a row. When even that cannot tell channels apart (nothing declared, so
    every quantity is unknown) the channel order breaks the tie, and the
    row's label says so ('101Z+ #2'). Columns are the reference DOFs of a
    matrix of measurements, the captures of a repeated one, or one blank
    column. A set that does not fill its rectangle keeps its holes (-1).
    Time data has a role per row."""
    from hertzforge.core.data import channel_quantities

    count = int(data.num_records)
    factors = [channel_quantities(data.known_dim(i)) for i in range(count)]
    quantities = [(str(dof), factors[i][0]) for i, dof in enumerate(data.response_dof)]
    if data.reference_dof is not None:
        identities: list[Any] = [(str(dof), factors[i][1])
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


def _geometry(geometry: Any) -> dict[str, Any]:
    """A geometry's parts as the tree lists them: nodes and coordinate
    systems with their counts; every element family, the six always and
    points too (a ground or a spring to ground starts from one), pyramids
    when there are any, each with the element groups holding elements of
    it; the solids, a boolean's operands under it; the mated surfaces."""
    from hertzforge.core import solids
    from hertzforge.core.geometry import FAMILIES, FAMILY_LABELS, RARE_FAMILIES, element_family

    families: dict[str, dict[int, int]] = {}
    for code, group in zip(geometry.elem_type, geometry.elem_group, strict=True):
        held = families.setdefault(element_family(int(code)), {})
        held[int(group)] = held.get(int(group), 0) + 1
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
                        'count': groups[group]}
                       for group in (int(g) for g in geometry.group_id) if group in groups]})

    def solid_row(solid: Any) -> dict[str, Any]:
        label = solid.name.strip() or f'{SOLID_LABELS.get(solid.kind, solid.kind.capitalize())} {solid.id}'
        return {'id': int(solid.id), 'label': label, 'kind': solid.kind,
                'children': [solid_row(solids.solid(geometry, operand))
                             for operand in solid.operands]}

    return {
        'kind': 'geometry',
        'parts': [{'id': 'nodes', 'label': 'Nodes', 'count': len(geometry.node_id)},
                  {'id': 'coordinate_systems', 'label': 'Coordinate systems',
                   'count': len(geometry.cs_id)}],
        'families': family_rows,
        'solids': [solid_row(s) for s in solids.top_level(geometry)],
        'mates': [{'id': int(m.id), 'label': f'{m.name} ({m.describe()})'}
                  for m in getattr(geometry, 'mates', ())],
    }


def children(project: Any, name: str) -> dict[str, Any]:
    """What the object `name` holds, by what it is; `{'kind': 'none'}` for
    one that opens into nothing (a report, predictions)."""
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
    if isinstance(obj, MatchedModes):
        return {'kind': 'matches', 'matches': [
            {'first': int(first) + 1, 'second': int(second) + 1, 'mac': float(mac)}
            for (first, second), mac in zip(obj.pairs, obj.macs, strict=True)]}
    return {'kind': 'none'}


def banded(obj: Any) -> bool:
    """Whether a PSD or a specification is on proportional bands, which
    changes its icon as it changes its shape (`gui.icons.object_icon`)."""
    bandwidth = getattr(obj, 'bandwidth', None)
    return bandwidth is not None and not (isinstance(bandwidth, float) and np.isnan(bandwidth))
