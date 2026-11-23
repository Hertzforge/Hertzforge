"""Strain and stress shapes in the window: reading a solved model's modes
element by element (`Project.element_shapes`), and drawing one component
of a set on its model, each element its own color.

The view is the model's mode shape, animated, with the elements read
drawn over it in the colormap by one component at their centroids
(`element_values`): strain and stress jump from one element to the next,
so each element keeps its own flat color rather than a value averaged at
the nodes. The rest of the model is drawn gray.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .descriptors import Operation, Option, Param
from .mesh import drawn_mesh
from .mode_pairs import translations
from .parsing import parse_id_runs, parse_optional_picks
from .protocol import refused

QUANTITY = Param('quantity', 'Quantity', 'enum', default='stress',
                 options=(Option('stress', 'Stress'), Option('strain', 'Strain')))


def _component_options() -> tuple[Option, ...]:
    from hertzforge.core.element_shapes import COMPONENTS

    return tuple(Option(c, c) for c in COMPONENTS)


COMPONENT = Param('component', 'Component', 'enum', default='largest',
                  options=(Option('largest', 'Largest'), *_component_options()))
SOURCE = Param('source', 'Shape set', 'enum', options_from='verb:element_shapes')
GROUPS = Param('groups', 'Element groups, blank for every solid group', 'string', default='')
MODES = Param('modes', 'Modes, blank for all', 'string', default='')
OBJECT = Param('object', 'Strain and stress', 'enum', options_from='objects:element_shape_set')

#: what each quantity is in, in SI: strain has no unit, stress is a pressure
UNITS = {'strain': '', 'stress': 'Pa'}


def element_skin(geometry: Any, elements: Any) -> tuple[np.ndarray, np.ndarray]:
    """The outside of a set of solid elements, as triangles of node rows
    (u32 [T, 3]) and each triangle's element, as its index in `elements`
    (u32 [T]): every face no second element of the set shares, fanned
    from its first corner."""
    from hertzforge.core.geometry import solid_faces

    where = {int(e): k for k, e in enumerate(np.asarray(geometry.elem_id))}
    index = {int(n): k for k, n in enumerate(np.asarray(geometry.node_id))}
    faces: dict[tuple[int, ...], tuple[tuple[int, ...], int]] = {}
    for owner, element in enumerate(elements):
        conn = [int(n) for n in geometry.elem_conn[where[int(element)]]]
        for face in solid_faces(len(conn)):
            corners = tuple(index[conn[k]] for k in face)
            key = tuple(sorted(corners))
            if key in faces:
                del faces[key]
            else:
                faces[key] = (corners, owner)
    triangles, owners = [], []
    for corners, owner in faces.values():
        for k in range(1, len(corners) - 1):
            triangles.append((corners[0], corners[k], corners[k + 1]))
            owners.append(owner)
    return (np.asarray(triangles, dtype=np.int64).reshape(-1, 3),
            np.asarray(owners, dtype=np.int64))


def largest_component(shapes: Any, quantity: str) -> str:
    """The component that reaches furthest at the elements' centroids over
    every mode: what a set is drawn by until another is chosen."""
    from hertzforge.core.element_shapes import COMPONENTS

    held = np.abs(getattr(shapes, quantity)[:, shapes.point == 0])
    if not held.size:
        return COMPONENTS[0]
    return COMPONENTS[int(np.argmax(held.max(axis=(0, 1))))]


def _source_modes(project: Any, name: str, shapes: Any) -> tuple[Any, list[int] | None]:
    """The shape set an element shape set was read from and the index there
    of each of its modes, or (None, None) when that set is gone or no
    longer has them."""
    from hertzforge.core.shapes import ShapeSet

    record = (getattr(project, 'provenance', {}) or {}).get(name) or {}
    source = record.get('source')
    if record.get('verb') != 'element_shapes' or source not in project:
        return None, None
    modal = project[source]
    if not isinstance(modal, ShapeSet):
        return None, None
    chosen = (record.get('params') or {}).get('modes')
    picks = list(chosen) if chosen is not None else list(range(shapes.num_shapes))
    if len(picks) != shapes.num_shapes or max(picks, default=-1) >= modal.num_shapes:
        return None, None
    if not np.allclose(modal.frequency[picks], shapes.frequency):
        return None, None
    return modal, picks


def element_view(project: Any, name: str, picked: list[int] | None = None,
                 quantity: str = 'stress', component: str = 'largest',
                 title: str | None = None) -> dict:
    """An element shape set on its model: the modes picked (all when
    None), each animated by the shape set it was read from while that set
    is still in the project, with the elements read colored by one
    component at their centroids."""
    from hertzforge.core.element_shapes import COMPONENTS, QUANTITIES

    shapes = project[name]
    if quantity not in QUANTITIES:
        raise refused(f'{quantity!r} is not strain or stress')
    if component == 'largest':
        component = largest_component(shapes, quantity)
    if component not in COMPONENTS:
        raise refused(f'{component!r} is not a component: {", ".join(COMPONENTS)}')
    found = project.geometry_for(name)
    if found is None:
        raise refused(f'{name} has no geometry to be drawn on: link it with its model')
    geometry_name, geometry = found
    elements = shapes.elements
    have = set(np.asarray(geometry.elem_id).tolist())
    missing = [int(e) for e in elements if int(e) not in have]
    if missing:
        raise refused(f'{geometry_name} no longer has element {missing[0]}, which {name} '
                      'was read at: read it again')
    rows, triangles, lines = drawn_mesh(geometry)
    skin, owner = element_skin(geometry, elements)
    # the skin of a group inside the model has nodes the model's own
    # drawing leaves out: drawn too, and the drawing's indices moved
    every = np.union1d(rows, skin.ravel())
    remap = np.full(geometry.num_nodes, -1, dtype=np.int64)
    remap[every] = np.arange(every.size)
    triangles, lines = remap[rows][triangles], remap[rows][lines]
    skin = remap[skin]
    picks = list(picked) if picked else list(range(shapes.num_shapes))
    modal, source_modes = _source_modes(project, name, shapes)
    if modal is not None:
        moved = translations(geometry, modal,
                             modal.shape_matrix[[source_modes[k] for k in picks]], every)
    else:
        moved = np.zeros((len(picks), every.size, 3))
    modes = [v.mode(k + 1, float(shapes.frequency[k]), float(shapes.damping[k]), moved[i])
             for i, k in enumerate(picks)]
    values = np.array([shapes.values(quantity, component, k) for k in picks],
                      dtype=np.float32).reshape(len(picks), len(elements))
    # a rigid-body mode strains nothing, and what it holds is round-off: drawn as
    # the zero it is, not spread over the whole colormap by its own peak
    rigid = [i for i, k in enumerate(picks) if shapes.frequency[k] == 0.0]
    values[rigid] = 0.0
    label = f'{quantity.capitalize()} {component}'
    out = v.shape3d(title or f'{label} of {name} on {geometry_name}',
                    'm' if geometry.units_defined else 'undefined',
                    np.asarray(geometry.node_xyz, dtype=float)[every], modes,
                    triangles=triangles, lines=lines,
                    node_ids=np.asarray(geometry.node_id)[every],
                    normalization='mass')
    out['element_values'] = {
        'triangles': np.ascontiguousarray(skin, np.uint32),
        'owner': np.ascontiguousarray(owner, np.uint32),
        'values': np.ascontiguousarray(values, np.float32),
        'label': label, 'unit': UNITS[quantity]}
    notes = []
    if rigid:
        notes.append('a rigid-body mode at 0 Hz strains nothing, and is drawn at zero')
    if modal is None:
        notes.append(f'the shape set {name} was read from is gone, so its modes are drawn '
                     'still')
    if notes:
        out['note'] = '; '.join(notes)
    return out


def _read_rows(name: str, shapes: Any) -> list[dict]:
    energy = shapes.strain_energy()
    return [v.row('Elements', float(len(shapes.elements)), '', 'info'),
            v.row('Stress points', float(shapes.num_points), '', 'info'),
            v.row('Modes', float(shapes.num_shapes), '', 'info'),
            v.row('Largest strain energy', float(energy.max()) if len(energy) else 0.0,
                  'J', 'info')]


def element_shapes(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.element_shapes`: a solved model's modes read as strain
    and stress at each solid element's centroid and rule points
    (`Project.element_shapes`), drawn on the model by the component
    chosen."""
    from hertzforge.core.geometry import Geometry

    name = params['source']
    modal = project[name]
    found = project.geometry_for(name)
    if found is None:
        raise refused(f'{name} has no geometry: link it with the model it was solved on')
    geometry: Geometry = found[1]
    groups = None
    if params['groups'].strip():
        known = [int(g) for g in geometry.group_id]
        groups = parse_id_runs(params['groups'], 'an element group number')
        for group in groups:
            if group not in known:
                raise refused(f'{found[0]} has element groups {", ".join(map(str, known))}; '
                              f'not {group}')
    modes = parse_optional_picks(params['modes'], modal.num_shapes, 'mode')
    try:
        added = project.element_shapes(name, groups=groups, modes=modes)
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    shapes = project[added]
    view = element_view(project, added, None, params['quantity'], params['component'])
    return [view, v.reading(f'{added}: strain and stress at {len(shapes.elements):,} elements',
                            [v.row('Added', added, None, 'info'), *_read_rows(added, shapes)])]


def color_element_shapes(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.color_element_shapes`: an element shape set drawn on its
    model by another quantity or component."""
    name = params['object']
    picked = parse_optional_picks(params.get('modes', ''), project[name].num_shapes, 'mode')
    return [element_view(project, name, picked, params['quantity'], params['component'])]


OPERATIONS = (
    Operation(
        id='project.element_shapes', title='Strain and Stress', workspace='modal',
        summary='A solved model\'s modes read as strain and stress at each solid element\'s '
                'centroid and integration points, drawn on the model by one component.',
        params=(SOURCE, GROUPS, MODES, QUANTITY, COMPONENT), outputs=('shape3d', 'reading'),
        run=element_shapes, previews=True),
    Operation(
        id='project.color_element_shapes', title='Color By', workspace='modal',
        summary='An element shape set drawn on its model, each element colored by one '
                'component of its strain or stress at its centroid.',
        params=(OBJECT, QUANTITY, COMPONENT, MODES), outputs=('shape3d',),
        run=color_element_shapes),
)
