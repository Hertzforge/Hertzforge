"""The Model workspace's tools on the connection's project: a geometry seen
on its own, its element groups listed, set, merged, deleted and tied,
parts meshed apart aligned and their coincident nodes merged, the steps
between importing a mesh and Solve Modes, as Visual Dynamics' Model
screen takes them.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import group_cells
from . import views as v
from .datasets import _mesh
from .descriptors import Operation, Option, Param
from .ops_geometry import entity_table
from .protocol import refused

GEOMETRY = Param('geometry', 'Geometry', 'enum', options_from='objects:geometry')
GROUPS = Param('groups', 'Element groups', 'string', default='')


def _materials() -> tuple[Option, ...]:
    from hertzforge.core.fem import MATERIALS, RIGID

    return tuple(Option(name, name) for name in MATERIALS if name != RIGID.name)


MATERIAL = Param('material', 'Material', 'enum', default='6061-T6', options=_materials())
THICKNESS = Param('thickness', 'Plate thickness', 'number', default=3.0, unit='mm',
                  min=0.0, step=0.5)
GROUPS_TO_MERGE = Param('groups', 'Element groups', 'string', default='')
ELEMENTS = Param('elements', 'Elements', 'string', default='')
TIE_TO = Param('to', 'Tie to', 'string', default='')
REACH = Param('reach', 'Reach', 'number', default=0.0, unit='mm', min=0.0, step=0.1)
TOLERANCE = Param('tolerance', 'Tolerance', 'number', default=0.0, unit='m', min=0.0,
                  step=0.0001)


def view_geometry(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.view_geometry`: a geometry on its own, nodes and elements."""
    name = params['geometry']
    geometry = project[name]
    rows, triangles, lines = _mesh(geometry)
    return [v.shape3d(name, 'm' if geometry.units_defined else 'undefined',
                      np.asarray(geometry.node_xyz, dtype=float)[rows], [],
                      triangles=triangles, lines=lines,
                      node_ids=np.asarray(geometry.node_id)[rows])]


_family = group_cells.family


def _groups(geometry: Any, text: str) -> list[int]:
    known = [int(g) for g in geometry.groups.ids]
    if not text.strip():
        return known
    chosen = []
    for part in text.replace(' ', '').split(','):
        try:
            group = int(part)
        except ValueError:
            raise refused(f'{part!r} is not an element group number') from None
        if group not in known:
            raise refused(f'the geometry has element groups {", ".join(map(str, known))}; '
                          f'not {group}')
        chosen.append(group)
    return chosen


def _properties_reading(name: str, geometry: Any) -> dict:
    rows = []
    for group in (int(g) for g in geometry.groups.ids):
        held = geometry.group_properties.get(group)
        family = _family(geometry, group)
        if held is None:
            said = 'not set'
        elif held.thickness is not None:
            said = f'{held.material.name}, {held.thickness * 1000:g} mm'
        elif held.section is not None:
            said = f'{held.material.name}, {held.section.shape or "custom"} section'
        else:
            said = held.material.name if held.material else held.kind
        rows.append(v.row(f'Group {group} ({family or "empty"})', said, None,
                          'info' if held is not None else 'warn'))
    return v.reading(f'Element groups of {name}', rows)


def group_properties(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.group_properties`: what the chosen element groups are made
    of (`fem.GroupProperties`), many at once: a library material, and for
    plates a thickness. Beams and points take more than that, a section, a
    mass, a stiffness or a ground, in the Element Groups table; with no group
    named they are left alone, and one named is refused with where to go."""
    from hertzforge.core.fem import GroupProperties, material

    name = params['geometry']
    geometry = project[name]
    chosen = _groups(geometry, params['groups'])
    if not chosen:
        raise refused(f'{name} has no element groups')
    if not params['groups'].strip():
        taking = [g for g in chosen if _family(geometry, g) in ('face', 'volume')]
        if taking:
            chosen = taking
    made = material(params['material'])
    thickness = params['thickness'] / 1000.0
    set_now = {}
    for group in chosen:
        family = _family(geometry, group)
        if family == 'face':
            if not thickness > 0:
                raise refused(f'group {group} is plates, which take a thickness above 0')
            set_now[group] = GroupProperties(made, thickness)
        elif family == 'volume':
            set_now[group] = GroupProperties(made)
        elif family == 'line':
            raise refused(f'group {group} is beams, which take a section: set it in the '
                          'Element Groups table')
        elif family == 'point':
            raise refused(f'group {group} is points, which take a mass, a stiffness or a '
                          'ground: set it in the Element Groups table')
        else:
            raise refused(f'group {group} holds {family or "no"} elements; give it one '
                          'kind of element first')
    geometry.group_properties = {**geometry.group_properties, **set_now}
    return [_properties_reading(name, geometry)]


def merge_coincident_nodes(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.merge_coincident_nodes`: each set of a geometry's coincident
    nodes merged into one, the elements reconnected to the lowest id
    (`Project.merge_coincident_nodes`)."""
    name = params['geometry']
    tolerance = params['tolerance'] or None
    before = len(project[name].node_id)
    try:
        project.merge_coincident_nodes(name, tolerance=tolerance)
    except ValueError as error:
        raise refused(str(error)) from None
    after = len(project[name].node_id)
    return [v.reading(f'Coincident nodes of {name}', [
        v.row('Nodes before', float(before), '', 'info'),
        v.row('Nodes merged away', float(before - after), '', 'info'),
        v.row('Nodes now', float(after), '', 'info')])]


def _ids(text: str, what: str) -> list[int]:
    """The ids '3, 7-9' names, in the order named."""
    out: list[int] = []
    for part in text.replace(' ', '').split(','):
        if not part:
            continue
        first, dash, last = part.partition('-')
        try:
            low, high = int(first), int(last) if dash else int(first)
        except ValueError:
            raise refused(f'{part!r} is not {what} or a range such as 3-6') from None
        out.extend(range(low, high + 1))
    return list(dict.fromkeys(out))


def element_groups(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.element_groups`: a geometry's element groups as Visual
    Dynamics' Element Groups table lists them, and takes them typed in place
    (`group_cells`): each group's id, name and elements, and what it is
    made of."""
    name = params['geometry']
    return [entity_table(project[name], 'groups', f'Element groups of {name}', name)]


def merge_element_groups(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.merge_element_groups`: two or more of a geometry's element
    groups merged into the first named, which keeps its id, name and
    properties (`Project.merge_groups`)."""
    name = params['geometry']
    chosen = _groups(project[name], params['groups'])
    if len(chosen) < 2:
        raise refused('name two or more element groups to merge, such as 1, 3')
    try:
        project.merge_groups(name, chosen)
    except ValueError as error:
        raise refused(str(error)) from None
    return element_groups(project, params)


def delete_element_groups(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.delete_element_groups`: element groups deleted with their
    elements and the nodes only they used, except nodes where data in the
    project is measured or solved (`Project.delete_element_groups`)."""
    name = params['geometry']
    geometry = project[name]
    if not params['groups'].strip():
        raise refused('name the element groups to delete, such as 2, 4')
    chosen = _groups(geometry, params['groups'])
    if len(chosen) == len(geometry.groups.ids):
        raise refused('that is every element group: delete the geometry instead')
    try:
        project.delete_element_groups(name, chosen)
    except ValueError as error:
        raise refused(str(error)) from None
    return element_groups(project, params)


def tie_elements(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.tie_elements`: a patch of a geometry's elements tied
    rigidly to the part under it, each node by a rigid link to the
    nearest node of an element group or a second patch
    (`Project.tie_elements`), as for a bolted joint modeled from the
    elements its washer covers."""
    name = params['geometry']
    geometry = project[name]
    patch = _ids(params['elements'], 'an element id')
    if not patch:
        raise refused('name the elements of the patch, such as 101-104')
    target_text = params['to'].strip()
    if not target_text:
        raise refused('name what the patch ties to: an element group, by name or '
                      'number, or a second patch of elements')
    names = list(geometry.groups.names)
    if target_text in names:
        target: Any = target_text
    else:
        numbers = _ids(target_text, 'an element group or element id')
        target = numbers[0] if len(numbers) == 1 and numbers[0] in geometry.groups.ids \
            else numbers
    try:
        found = project.tie_elements(name, patch, target)
    except (ValueError, KeyError) as error:
        raise refused(str(error).strip("'")) from None
    return [v.reading(f'Tie on {name}', [
        v.row('Links added', float(found['links']), '', 'info'),
        v.row('Nodes already shared', float(found['shared']), '', 'info'),
        v.row('Links in group', str(found['group']), None, 'info')])]


def align_meshes(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.align_meshes`: where parts meshed apart meet, the coarser
    mesh's nodes moved onto the finer mesh's nearest nodes within a
    reach (`Project.align_meshes`), so Merge Coincident Nodes can join
    them next. 0 reaches half the coarsest piece's spacing."""
    name = params['geometry']
    geometry = project[name]
    groups = _groups(geometry, params['groups']) if params['groups'].strip() else None
    reach = params['reach'] / 1000.0 or None
    try:
        found = project.align_meshes(name, reach, groups)
    except ValueError as error:
        raise refused(str(error)) from None
    return [v.reading(f'Aligned meshes of {name}', [
        v.row('Nodes moved', float(found['moved']), '', 'info'),
        v.row('Farthest move', float(found['farthest']) * 1000.0, 'mm', 'info')])]


OPERATIONS = (
    Operation(
        id='project.view_geometry', title='View Geometry', workspace='model',
        accepts='project', summary='A geometry on its own: its nodes and elements.',
        params=(GEOMETRY,), outputs=('shape3d',), run=view_geometry),
    Operation(
        id='project.group_properties', title='Element Group Properties',
        workspace='model', accepts='project',
        summary='What element groups are made of, for Solve Modes: a library '
                'material, and for plates a thickness. Every group when none '
                'are named, or "1, 3".',
        params=(GEOMETRY, GROUPS, MATERIAL, THICKNESS), outputs=('reading',),
        run=group_properties, edits=True),
    Operation(
        id='project.merge_coincident_nodes', title='Merge Coincident Nodes',
        workspace='model', accepts='project',
        summary='Each set of coincident nodes merged into one, so parts meshed '
                'apart are joined; 0 for the core\'s own tolerance.',
        params=(GEOMETRY, TOLERANCE), outputs=('reading',),
        run=merge_coincident_nodes, edits=True),
    Operation(
        id='project.element_groups', title='Element Groups', workspace='model',
        accepts='project',
        summary='A geometry\'s element groups: each one\'s name, what its elements are, '
                'how many, and what it is made of.',
        params=(GEOMETRY,), outputs=('table',), run=element_groups),
    Operation(
        id='project.merge_element_groups', title='Merge Element Groups', workspace='model',
        accepts='project',
        summary='Two or more element groups merged into the first named, which keeps its '
                'name and properties, such as "1, 3".',
        params=(GEOMETRY, GROUPS_TO_MERGE), outputs=('table',), run=merge_element_groups,
        edits=True),
    Operation(
        id='project.delete_element_groups', title='Delete Element Groups', workspace='model',
        accepts='project',
        summary='Element groups deleted with their elements and the nodes only they used, '
                'keeping nodes where the project has data.',
        params=(GEOMETRY, GROUPS_TO_MERGE), outputs=('table',), run=delete_element_groups,
        edits=True),
    Operation(
        id='project.tie_elements', title='Tie', workspace='model', accepts='project',
        summary='A patch of elements tied rigidly to the part under it, each node to the '
                'nearest node of an element group or a second patch.',
        params=(GEOMETRY, ELEMENTS, TIE_TO), outputs=('reading',), run=tie_elements,
        edits=True),
    Operation(
        id='project.align_meshes', title='Align Meshes', workspace='model', accepts='project',
        summary='Where parts meshed apart meet, the coarser mesh\'s nodes moved onto the '
                'finer one\'s, so Merge Coincident Nodes can join them; reach 0 for half '
                'the coarsest spacing.',
        params=(GEOMETRY, GROUPS, REACH), outputs=('reading',), run=align_meshes,
        edits=True),
)
