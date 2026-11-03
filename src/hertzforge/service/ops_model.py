"""The Model workspace's tools on the connection's project: a geometry seen
on its own, what its element groups are made of, and its coincident nodes
merged, the steps between importing a mesh and Solve Modes, as Visual
Dynamics' Model screen takes them.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .datasets import _mesh
from .descriptors import Operation, Option, Param
from .protocol import refused

GEOMETRY = Param('geometry', 'Geometry', 'enum', options_from='objects:geometry')
GROUPS = Param('groups', 'Element groups', 'string', default='')


def _materials() -> tuple[Option, ...]:
    from hertzforge.core.fem import MATERIALS, RIGID

    return tuple(Option(name, name) for name in MATERIALS if name != RIGID.name)


MATERIAL = Param('material', 'Material', 'enum', default='6061-T6', options=_materials())
THICKNESS = Param('thickness', 'Plate thickness', 'number', default=3.0, unit='mm',
                  min=0.0, step=0.5)
TOLERANCE = Param('tolerance', 'Tolerance', 'number', default=0.0, unit='m', min=0.0,
                  step=0.0001)


def view_geometry(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.view_geometry`: a geometry on its own, nodes and elements."""
    name = params['geometry']
    geometry = project[name]
    rows, triangles, lines = _mesh(geometry)
    return [v.shape3d(name, 'm' if geometry.units_defined else 'undefined',
                      np.asarray(geometry.node_xyz, dtype=float)[rows], [],
                      triangles=triangles, lines=lines)]


def _family(geometry: Any, group: int) -> str:
    """'face', 'volume', 'line' or 'point': what an element group's
    elements are; 'mixed' when they differ, '' when it has none."""
    from hertzforge.core.geometry import ELEMENT_TYPES

    ids = set(geometry.elements_in(group))
    families = {ELEMENT_TYPES.get(int(kind), ('', 0, ''))[2]
                for elem, kind in zip(geometry.elem_id, geometry.elem_type, strict=True)
                if int(elem) in ids}
    if not families:
        return ''
    return families.pop() if len(families) == 1 else 'mixed'


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
            said = f'{held.material.name}, a section'
        else:
            said = held.material.name if held.material else 'set'
        rows.append(v.row(f'Group {group} ({family or "empty"})', said, None,
                          'info' if held is not None else 'warn'))
    return v.reading(f'Element groups of {name}', rows)


def group_properties(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.group_properties`: what the chosen element groups are made
    of (`fem.GroupProperties`), as the Element Groups table sets it: a
    library material, and for plates a thickness. A group of beams takes a
    section, which a script sets; one is refused here by name."""
    from hertzforge.core.fem import GroupProperties, material

    name = params['geometry']
    geometry = project[name]
    chosen = _groups(geometry, params['groups'])
    if not chosen:
        raise refused(f'{name} has no element groups')
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
            raise refused(f'group {group} is beams, which take a section; set it from a '
                          'script (geometry.group_properties) for now')
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
)
