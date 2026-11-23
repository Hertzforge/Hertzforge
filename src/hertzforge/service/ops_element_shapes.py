"""Strain and stress shapes in the window: reading a solved model's modes
element by element (`Project.element_shapes`), drawing a set on its model
each element its own color, and playing modal coordinates with the
elements colored as the model moves.

The view is the model's mode shape, or a clip of modal coordinates,
animated, with the elements read drawn over it in the colormap
(`element_values`). The service sends each mode's tensor at each
element's centroid once; the shell sums them with the modal coordinates
each frame and forms the scalar chosen (von Mises, a principal stress or
one component) from the summed tensor, so a clip is never stored element
by element in time. Strain and stress jump from one element to the next,
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


def _scalar_options() -> tuple[Option, ...]:
    from hertzforge.core.element_shapes import COMPONENTS

    return (Option('largest', 'Largest component'), Option('von_mises', 'Von Mises (stress)'),
            Option('tresca', 'Tresca (stress)'),
            Option('max_principal', 'Largest principal (stress)'),
            Option('mid_principal', 'Middle principal (stress)'),
            Option('min_principal', 'Smallest principal (stress)'),
            *(Option(c, c) for c in COMPONENTS))


COMPONENT = Param('component', 'Color by', 'enum', default='largest', options=_scalar_options())
COLOR_GROUPS = Param('color_groups', 'Colored element groups, blank for every element read',
                     'string', default='')
COLOR_MAX = Param('color_max', 'Top of the stress color scale, 0 for the largest shown', 'number',
                  default=0.0, unit='Pa', min=0.0, quantity='pressure')
DATA = Param('data', 'Modal coordinates', 'enum', options_from='objects:time_history')
DATA_RECORDS = Param('data_records', 'Modal coordinates picked, blank for all', 'string',
                     default='')
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


def color_scalar(shapes: Any, quantity: str, component: str) -> str:
    """The scalar a set is colored by: `component`, or the component that
    reaches furthest for 'largest'. Strain is colored by one component; von
    Mises, Tresca and the principal stresses are stresses."""
    from hertzforge.core.element_shapes import COMPONENTS, QUANTITIES
    from hertzforge.core.stress_scalars import SCALARS

    if quantity not in QUANTITIES:
        raise ValueError(f'{quantity!r} is not strain or stress')
    if component == 'largest':
        return largest_component(shapes, quantity)
    if component not in SCALARS:
        raise ValueError(f'{component!r} is not a component or a stress scalar: largest, '
                         + ', '.join(SCALARS))
    if quantity == 'strain' and component not in COMPONENTS:
        raise ValueError(f'{component.replace("_", " ")} is a stress; strain is colored by one '
                         'component: ' + ', '.join(COMPONENTS))
    return component


def colored_elements(geometry: Any, name: str, shapes: Any, groups: str) -> np.ndarray:
    """Which of the set's elements are colored (a mask over `shapes.elements`):
    those in the element groups `groups` names, or every one for blank."""
    elements = np.asarray(shapes.elements)
    if not groups.strip():
        return np.ones(elements.size, dtype=bool)
    wanted = parse_id_runs(groups, 'an element group number')
    group_of = dict(zip(np.asarray(geometry.elem_id).tolist(),
                        np.asarray(geometry.elem_group).tolist(), strict=True))
    held = np.array([group_of[int(e)] for e in elements], dtype=np.int64)
    mask = np.isin(held, wanted)
    if not mask.any():
        have = ', '.join(str(g) for g in sorted(set(held.tolist())))
        raise ValueError(f'{name} read no element of group {", ".join(map(str, wanted))}; '
                         f'it read groups {have}')
    return mask


def element_field(geometry: Any, name: str, shapes: Any, rows: list[int], quantity: str,
                  component: str, mask: np.ndarray, color_max: float = 0.0,
                  coordinates: Any = None) -> dict:
    """The `element_values` of a view: the set's modes `rows`, in the order the
    view plays them, as each element's tensor at its centroid (modes,
    elements, 6, in SI), with the outside faces of the elements `mask` keeps
    as rows of `geometry`'s nodes (the caller maps them to its own drawing).
    The top of the color scale is `color_max` (SI) when above 0, else the
    largest value shown: over the clip of `coordinates` (modes, samples), or
    over each mode's cycle for modes."""
    from hertzforge.core.stress_scalars import LABELS, clip_peak, mode_peaks

    scalar = color_scalar(shapes, quantity, component)
    elements = np.asarray(shapes.elements)
    have = set(np.asarray(geometry.elem_id).tolist())
    missing = [int(e) for e in elements[mask] if int(e) not in have]
    if missing:
        raise ValueError(f'the geometry no longer has element {missing[0]}, which {name} '
                         'was read at: read it again')
    tensors = np.asarray(getattr(shapes, quantity))[:, shapes.point == 0][rows][:, mask]
    tensors = np.array(tensors, dtype=np.float64)
    # a rigid-body mode strains nothing, and what it holds is round-off: drawn as
    # the zero it is, not spread over the whole colormap by its own peak
    tensors[np.asarray(shapes.frequency)[rows] == 0.0] = 0.0
    skin, owner = element_skin(geometry, elements[mask])
    if coordinates is None:
        found = [float(p) for p in mode_peaks(tensors, scalar)]
    else:
        found = [clip_peak(tensors, coordinates, scalar)]
    stated = bool(color_max and color_max > 0)
    if stated and quantity == 'strain':
        raise ValueError('a stated color scale is a stress; strain is scaled by its largest '
                         'value: leave the top of the scale at 0')
    out = {'quantity': quantity,
           'label': LABELS[scalar] if quantity == 'stress' else f'Strain {scalar}',
           'unit': UNITS[quantity], 'scalar': scalar, 'elements': int(mask.sum()),
           'triangles': skin, 'owner': np.ascontiguousarray(owner, np.uint32),
           'shapes': np.ascontiguousarray(tensors, np.float32),
           'max': [float(color_max)] * len(found) if stated else found}
    if stated:
        out['stated'] = True
    return out


def with_rows(rows: np.ndarray, triangles: np.ndarray, lines: np.ndarray, extra: np.ndarray,
              num_nodes: int) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    """A drawing (`drawn_mesh`'s rows, triangles and lines) with the nodes
    `extra` drawn too, as (rows, triangles, lines, each geometry node's
    drawn index or -1): the skin of a group inside the model has nodes the
    model's own drawing leaves out."""
    every = np.union1d(rows, np.asarray(extra, dtype=np.int64).ravel())
    remap = np.full(num_nodes, -1, dtype=np.int64)
    remap[every] = np.arange(every.size)
    return every, remap[rows][triangles], remap[rows][lines], remap


def element_view(project: Any, name: str, picked: list[int] | None = None,
                 quantity: str = 'stress', component: str = 'largest',
                 title: str | None = None, color_groups: str = '',
                 color_max: float = 0.0) -> dict:
    """An element shape set on its model: the modes picked (all when
    None), each animated by the shape set it was read from while that set
    is still in the project, with the elements read (those of the element
    groups `color_groups` when given) colored by one scalar of their tensor
    at their centroids through each mode's cycle."""
    shapes = project[name]
    found = project.geometry_for(name)
    if found is None:
        raise refused(f'{name} has no geometry to be drawn on: link it with its model')
    geometry_name, geometry = found
    picks = list(picked) if picked else list(range(shapes.num_shapes))
    try:
        mask = colored_elements(geometry, name, shapes, color_groups)
        field = element_field(geometry, name, shapes, picks, quantity, component, mask,
                              color_max)
    except ValueError as error:
        raise refused(str(error).replace('the geometry', geometry_name, 1)) from None
    rows, triangles, lines = drawn_mesh(geometry)
    every, triangles, lines, remap = with_rows(rows, triangles, lines, field['triangles'],
                                               geometry.num_nodes)
    field['triangles'] = np.ascontiguousarray(remap[field['triangles']], np.uint32)
    modal, source_modes = _source_modes(project, name, shapes)
    if modal is not None:
        moved = translations(geometry, modal,
                             modal.shape_matrix[[source_modes[k] for k in picks]], every)
    else:
        moved = np.zeros((len(picks), every.size, 3))
    modes = [v.mode(k + 1, float(shapes.frequency[k]), float(shapes.damping[k]), moved[i])
             for i, k in enumerate(picks)]
    label = field['label']
    out = v.shape3d(title or f'{label} of {name} on {geometry_name}',
                    'm' if geometry.units_defined else 'undefined',
                    np.asarray(geometry.node_xyz, dtype=float)[every], modes,
                    triangles=triangles, lines=lines,
                    node_ids=np.asarray(geometry.node_id)[every],
                    normalization='mass')
    out['element_values'] = field
    from .show import _home

    # the geometry's own view, so a model built with Y up opens upright, as `show` draws it
    _home(project, [out], [geometry_name])
    notes = []
    if any(shapes.frequency[k] == 0.0 for k in picks):
        notes.append('a rigid-body mode at 0 Hz strains nothing, and is drawn at zero')
    if modal is None:
        notes.append(f'the shape set {name} was read from is gone, so its modes are drawn '
                     'still')
    if notes:
        out['note'] = '; '.join(notes)
    return out


def played_rows(project: Any, name: str, basis: str, played: list[int]) -> list[int]:
    """The set's mode for each mode of `basis` a clip plays (`played`, from 0),
    in that order; ValueError when the set was not read from `basis` or does
    not hold one of them."""
    shapes = project[name]
    modal, picks = _source_modes(project, name, shapes)
    source = ((getattr(project, 'provenance', {}) or {}).get(name) or {}).get('source')
    if modal is None or source != basis:
        said = f'from {source}' if source else 'from no shape set still in the project'
        raise ValueError(f'{name} was read {said}, and the clip plays {basis}: read the strain '
                         f'and stress of {basis}')
    where = {s: k for k, s in enumerate(picks)}
    missing = [s for s in played if s not in where]
    if missing:
        raise ValueError(f'{name} holds no strain or stress for mode {missing[0] + 1} of '
                         f'{basis}, which the clip plays: read every mode it plays')
    return [where[s] for s in played]


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
    model by another quantity or scalar, on a stated scale when given."""
    name = params['object']
    picked = parse_optional_picks(params.get('modes', ''), project[name].num_shapes, 'mode')
    return [element_view(project, name, picked, params['quantity'], params['component'],
                         color_groups=params.get('color_groups', ''),
                         color_max=float(params.get('color_max') or 0.0))]


def play_element_shapes(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.play_element_shapes`: modal coordinates played on the model
    an element shape set was read on, through the shape set it was read
    from, with its elements colored by one scalar of the tensor summed each
    frame."""
    from .show import element_coloring, modal_motion_of

    name, data = params['object'], params['data']
    found = project.geometry_for(name)
    if found is None:
        raise refused(f'{name} has no geometry to be drawn on: link it with its model')
    source = ((getattr(project, 'provenance', {}) or {}).get(name) or {}).get('source')
    if not source or source not in project:
        raise refused(f'the shape set {name} was read from is gone, so there is nothing to '
                      'play its modal coordinates through')
    coloring = element_coloring(name, params['quantity'], params['component'],
                                params.get('color_groups', ''),
                                float(params.get('color_max') or 0.0))
    # the modal coordinates picked under the clip are the modes played, all when none are
    picks = parse_optional_picks(params.get('data_records', ''),
                                 len(project[data].response_dof), 'record') or []
    views, notes = modal_motion_of(project, found[0], data, picks, source, coloring)
    if not views:
        raise refused(notes[0] if notes else f'{data} holds no modal coordinates to play')
    if notes:
        views[0]['note'] = '; '.join(notes)
    return views


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
                'component of its strain, or by von Mises, Tresca, a principal stress or one '
                'component of its stress, at its centroid through each mode\'s cycle.',
        params=(OBJECT, QUANTITY, COMPONENT, MODES, COLOR_GROUPS, COLOR_MAX), outputs=('shape3d',),
        run=color_element_shapes),
    Operation(
        id='project.play_element_shapes', title='Play Strain and Stress', workspace='modal',
        summary='Modal coordinates played on the model through the shape set an element shape '
                'set was read from, each element colored by one scalar of its strain or stress '
                'summed over the modes at each instant, on the clip\'s largest value or a stated '
                'scale.',
        params=(OBJECT, DATA, DATA_RECORDS, QUANTITY, COMPONENT, COLOR_GROUPS, COLOR_MAX),
        outputs=('shape3d', 'spectrum'), run=play_element_shapes),
)
