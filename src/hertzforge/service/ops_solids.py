"""Solid modeling on the connection's project, as Visual Dynamics' Model
screen builds a part: a new geometry, the solids it is made of (blocks,
cylinders, cones, spheres, sections, plates, disks and holes), the
booleans and patterns of them, Mesh Solids, Mate Surfaces and the view a
geometry opens on.

Each operation runs the `Project` verb VD's panes run (`add_solid_block`,
`combine_solids`, `mesh_solids` and the rest) and answers with the
geometry drawn, its meshed elements and its parts' surfaces as the
kernel tessellates them (`core.solids.shape_triangles`), and the solids
table VD lists under the geometry in its tree. Solids are named by id
from that table ("1, 3") where VD picks them in the tree or the view.
Lengths are typed in meters and angles in degrees about the geometry's
X, then Y, then Z axes.
"""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

import numpy as np

from . import views as v
from .datasets import _mesh
from .descriptors import Operation, Option, Param
from .protocol import refused

GEOMETRY = Param('geometry', 'Geometry', 'enum', options_from='objects:geometry')
NAME = Param('name', 'Name', 'string', default='')
SOLIDS = Param('solids', 'Solids (ids, as "1, 3")', 'string', default='')
PARTS = Param('solids', 'Solids (ids, as "1, 3"; empty for every part)', 'string', default='')


def _length(name: str, label: str, default: float, low: float | None = None) -> Param:
    return Param(name, label, 'number', default=default, unit='m', min=low, step=0.01)


def _angle(axis: str, prefix: str = 'angle', label: str = 'Angle') -> Param:
    return Param(f'{prefix}_{axis}', f'{label} about {axis.upper()}', 'number', default=0.0,
                 unit='deg', step=1.0)


CENTER = tuple(_length(axis, f'Center {axis.upper()}', 0.0) for axis in 'xyz')
ANGLES = tuple(_angle(axis) for axis in 'xyz')
WIDTHS = tuple(_length(f'width_{axis}', f'Width along {axis.upper()}', 1.0, 0.0)
               for axis in 'xyz')
CORNER = _length('corner', 'Corner radius', 0.0, 0.0)


def _section_shapes() -> tuple[Option, ...]:
    from hertzforge.core.solids import SECTIONS

    return tuple(Option(shape, f'{shape[:1].upper()}{shape[1:]}') for shape in SECTIONS)


#: a section's dimensions as VD's pane opens on them, in the order
#: `core.solids.SECTIONS` names them
SECTION_DEFAULTS = {'I-beam': (1.0, 0.5, 0.1, 0.1), 'channel': (1.0, 0.5, 0.1, 0.1),
                    'tee': (1.0, 1.0, 0.1, 0.1), 'angle': (1.0, 1.0, 0.1),
                    'rectangular tube': (1.0, 0.5, 0.1), 'round tube': (1.0, 0.1)}


def _kind_label(kind: str) -> str:
    return {'subtract': 'Subtraction', 'intersect': 'Intersection'}.get(
        kind, f'{kind[:1].upper()}{kind[1:]}')


def _label(item: Any) -> str:
    """A solid as VD's tree names it: its name, or its kind and id."""
    return item.name.strip() or f'{_kind_label(item.kind)} {item.id}'


def _geometry(project: Any, params: dict[str, Any]) -> tuple[str, Any]:
    name = params['geometry']
    return name, project[name]


def _numbers(text: str, what: str) -> list[float]:
    try:
        return [float(part) for part in text.replace(' ', '').split(',') if part]
    except ValueError:
        raise refused(f'{what} is numbers separated by commas, not {text!r}') from None


def _ids(geometry: Any, text: str, every: list[int] | None = None) -> list[int]:
    """The solids `text` names by id ("1, 3"), each one the geometry has;
    `every` when it names none and that is allowed."""
    known = [s.id for s in geometry.solids]
    if not text.strip():
        if every is not None and every:
            return every
        if not known:
            raise refused('the geometry has no solids yet; add one first')
        raise refused(f'name the solids by id, as "1, 3"; the geometry has '
                      f'{", ".join(map(str, known))}')
    chosen = []
    for part in text.replace(' ', '').split(','):
        try:
            number = int(part)
        except ValueError:
            raise refused(f'{part!r} is not a solid id') from None
        if number not in known:
            raise refused(f'the geometry has solids {", ".join(map(str, known)) or "none"}; '
                          f'not {number}')
        chosen.append(number)
    return chosen


def _parts(geometry: Any) -> list[int]:
    from hertzforge.core import solids

    return [s.id for s in solids.top_level(geometry)]


def _call(make: Callable[[], Any]) -> Any:
    """What `make` returns, a refusal of the core's said in its own words."""
    try:
        return make()
    except (ValueError, KeyError) as error:
        text = error.args[0] if isinstance(error, KeyError) and error.args else str(error)
        raise refused(str(text)) from None


# ---- the geometry drawn, and its solids listed ---------------------------------

def _f(value: float) -> str:
    return f'{float(value):.4g}'


def _dimensions(geometry: Any, item: Any) -> str:
    """A solid's size as typed, in a few words."""
    from hertzforge.core import solids

    unit = ' m' if geometry.units_defined else ''
    kind = item.kind
    if kind == 'block':
        corner = f', corner {_f(item.corner)}' if item.corner else ''
        return ' × '.join(_f(w) for w in item.widths) + unit + corner
    if kind in ('cylinder', 'sphere', 'disk'):
        long = f', length {_f(item.length)}' if kind == 'cylinder' else ''
        return f'radius {_f(item.radius)}{long}{unit}'
    if kind == 'cone':
        return (f'base radius {_f(item.radius)}, top radius {_f(solids._top(item))}, '
                f'height {_f(item.length)}{unit}')
    if kind == 'plate':
        return ' × '.join(_f(w) for w in item.widths[:2]) + unit
    if kind in solids.SECTIONS:
        named = ', '.join(f'{n} {_f(d)}' for n, d in zip(solids.SECTIONS[kind], item.dimensions,
                                                          strict=True))
        return f'{named}, length {_f(item.length)}{unit}'
    if kind == solids.HOLE:
        diameter, depth, wide, deep = item.dimensions
        said = f'diameter {_f(diameter)}, ' + (f'depth {_f(depth)}' if depth else 'through')
        if wide:
            said += f', counterbore {_f(wide)} across {_f(deep)} deep'
        return said + unit
    if kind == solids.PATTERN:
        return (' × '.join(str(c) for c in item.counts) + ', spacing '
                + ', '.join(_f(w) for w in item.widths) + unit)
    return ''


def _solids_table(name: str, geometry: Any) -> dict:
    """The solids as VD's tree lists them under the geometry: each part,
    and under a boolean or a pattern the solids it is made of."""
    from hertzforge.core import solids

    used = solids.used_by(geometry)
    unit = ' m' if geometry.units_defined else ''
    rows: list[list[str]] = []

    def add(item: Any, depth: int) -> None:
        primitive = item.kind in solids.PRIMITIVES
        if item.operands:
            operation = f'{item.kind} of {", ".join(map(str, item.operands))}'
        else:
            operation = 'part'
        if item.id in used:
            operation += f', in {used[item.id]}'
        rows.append([str(item.id), '  ' * depth + _label(item), _kind_label(item.kind),
                     _dimensions(geometry, item),
                     f'({", ".join(_f(c) for c in item.center)}){unit}' if primitive else '',
                     f'({", ".join(_f(a) for a in item.angles)})°'
                     if primitive or item.kind == solids.PATTERN else '',
                     operation])
        for operand in item.operands:
            add(solids.solid(geometry, operand), depth + 1)

    for item in solids.top_level(geometry):
        add(item, 0)
    return v.table(f'Solids of {name}', ['Id', 'Solid', 'Kind', 'Dimensions', 'Center',
                                         'Angles', 'Operation'], rows)


def _drawn(title: str, geometry: Any, defined: bool | None = None
           ) -> tuple[dict, list[dict]]:
    """The geometry as a `shape3d`: its elements as `view_geometry` draws
    them, and each part's surface as the kernel tessellates it
    (`solids.shape_triangles`), which is how VD draws a solid. A part the
    kernel cannot make is left out and said in a row. `defined` says
    whether its lengths are meters, when it is a mesh not yet joined to
    the geometry it was made from."""
    from hertzforge.core import solids

    used, triangles, lines = _mesh(geometry)
    nodes = [np.asarray(geometry.node_xyz, dtype=float).reshape(-1, 3)[used]]
    faces = [triangles.reshape(-1, 3)]
    start = len(used)
    notes = []
    for item in solids.top_level(geometry):
        try:
            soup = solids.shape_triangles(solids.shape(geometry, item.id))
        except ValueError as error:
            notes.append(v.row(_label(item), f'not drawn: {error}', None, 'warn'))
            continue
        count = 3 * len(soup)
        nodes.append(soup.reshape(-1, 3))
        faces.append(np.arange(start, start + count).reshape(-1, 3))
        start += count
    defined = geometry.units_defined if defined is None else defined
    view = v.shape3d(title, 'm' if defined else 'undefined',
                     np.concatenate(nodes), [], triangles=np.concatenate(faces), lines=lines)
    return view, notes


def _result(name: str, geometry: Any, title: str, rows: list[dict]) -> list[dict]:
    """What a solids operation answers with: what it did, the geometry
    drawn, and its solids table."""
    view, notes = _drawn(name, geometry)
    return [v.reading(title, rows + notes), view, _solids_table(name, geometry)]


def _parts_row(geometry: Any) -> dict:
    return v.row('Parts', float(len(_parts(geometry))), '', 'info')


# ---- New Geometry and Solids ---------------------------------------------------

GEOMETRY_NAME = Param('name', 'Name', 'string', default='Geometry')
UNIT = Param('unit', 'Built in', 'enum', default='m', options=tuple(
    Option(unit, label) for unit, label in (('m', 'm'), ('mm', 'mm'), ('in', 'in'),
                                            ('ft', 'ft'))))


def new_geometry(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.new_geometry`: an empty geometry to build a part in from
    solids (`Project.new_geometry`), recording the length unit it is built
    in; its coordinates are held in SI either way."""
    added = _call(lambda: project.new_geometry(params['name'].strip() or 'Geometry',
                                               unit=params['unit']))
    geometry = project[added]
    return [v.reading(f'New geometry {added}', [
        v.row('Added', added, None, 'pass'),
        v.row('Built in', params['unit'], None, 'info'),
        v.row('Next', 'add a plate, a block or another solid to it', None, 'info')]),
        _solids_table(added, geometry)]


def view_solids(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.solids`: a geometry's solids listed as VD's tree lists
    them, with the geometry drawn; it changes nothing."""
    name, geometry = _geometry(project, params)
    return _result(name, geometry, f'Solids of {name}', [_parts_row(geometry)])


# ---- adding a solid -------------------------------------------------------------

def _center(params: dict[str, Any]) -> tuple[float, float, float]:
    return (params['x'], params['y'], params['z'])


def _angles(params: dict[str, Any], prefix: str = 'angle') -> tuple[float, float, float]:
    return tuple(params[f'{prefix}_{axis}'] for axis in 'xyz')


def _add(project: Any, params: dict[str, Any], make: Callable[[str], int]) -> list[dict]:
    """Add the solid `make` makes in the picked geometry, and say it as
    VD's pane does: what it is, and its volume, or a flat one's area."""
    from hertzforge.core import solids

    name, geometry = _geometry(project, params)
    new = _call(lambda: make(name))
    item = solids.solid(geometry, new)
    rows = [v.row('Added', f'{_label(item)}, solid {new}', None, 'pass')]
    try:
        if item.kind in solids.FLAT:
            rows.append(v.row('Area', solids.area(geometry, new),
                              'm²' if geometry.units_defined else None, 'info'))
            rows.append(v.row('Thickness', 'set on the element group once it is meshed '
                              'into shells', None, 'info'))
        else:
            rows.append(v.row('Volume', solids.volume(geometry, new),
                              'm³' if geometry.units_defined else None, 'info'))
    except ValueError as error:
        rows.append(v.row('Volume', f'not read: {error}', None, 'warn'))
    rows.append(_parts_row(geometry))
    return _result(name, geometry, f'Added to {name}', rows)


def add_solid_block(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.add_solid_block`: a block, its widths along its own axes,
    centered and turned (`Project.add_solid_block`)."""
    return _add(project, params, lambda name: project.add_solid_block(
        name, _center(params), [params[f'width_{a}'] for a in 'xyz'], _angles(params),
        params['name'], corner=params['corner']))


def add_solid_cylinder(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.add_solid_cylinder`: a cylinder about its own Z
    (`Project.add_solid_cylinder`)."""
    return _add(project, params, lambda name: project.add_solid_cylinder(
        name, _center(params), params['radius'], params['length'], _angles(params),
        params['name']))


def add_solid_cone(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.add_solid_cone`: a cone or frustum along its own Z, its
    base below (`Project.add_solid_cone`)."""
    return _add(project, params, lambda name: project.add_solid_cone(
        name, _center(params), params['radius'], params['top_radius'], params['height'],
        _angles(params), params['name']))


def add_solid_sphere(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.add_solid_sphere`: a sphere about its center
    (`Project.add_solid_sphere`)."""
    return _add(project, params, lambda name: project.add_solid_sphere(
        name, _center(params), params['radius'], params['name']))


def add_solid_section(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.add_solid_section`: a structural section along its own Z
    (`Project.add_solid_section`), its dimensions typed in the order
    `core.solids.SECTIONS` names them, or VD's opening ones when none are
    typed."""
    from hertzforge.core.solids import SECTIONS

    shape = params['shape']
    typed = params['dimensions']
    dimensions = (_numbers(typed, 'the dimensions') if typed.strip()
                  else SECTION_DEFAULTS[shape])
    if len(dimensions) != len(SECTIONS[shape]):
        raise refused(f'{"an" if shape[0] in "aeiouI" else "a"} {shape} takes '
                      f'{", ".join(SECTIONS[shape])}: {len(SECTIONS[shape])} numbers')
    return _add(project, params, lambda name: project.add_solid_section(
        name, shape, _center(params), dimensions, params['length'], _angles(params),
        params['name']))


def add_solid_plate(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.add_solid_plate`: a flat rectangle in its own XY plane,
    meshed later into shells (`Project.add_solid_plate`)."""
    return _add(project, params, lambda name: project.add_solid_plate(
        name, _center(params), (params['width_x'], params['width_y']), _angles(params),
        params['name']))


def add_solid_disk(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.add_solid_disk`: a flat circle in its own XY plane,
    meshed later into shells (`Project.add_solid_disk`)."""
    return _add(project, params, lambda name: project.add_solid_disk(
        name, _center(params), params['radius'], _angles(params), params['name']))


def add_solid_hole(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.add_solid_hole`: a hole entering at its center and running
    down its own Z, through when its depth is 0, counterbored when given
    one (`Project.add_solid_hole`); subtract it from a part to cut it."""
    bore = (params['counterbore_diameter'], params['counterbore_depth'])
    return _add(project, params, lambda name: project.add_solid_hole(
        name, _center(params), params['diameter'], params['depth'] or None,
        _angles(params), params['name'], counterbore=bore if any(bore) else None))


# ---- changing solids ------------------------------------------------------------

SOLID = Param('solid', 'Solid (id)', 'integer', default=1, min=1, step=1)
KEEP_NOTE = 'empty keeps it'


def _field(name: str, label: str) -> Param:
    return Param(name, f'{label} ({KEEP_NOTE})', 'string', default='')


EDIT_FIELDS = (
    _field('center', 'Center, x, y, z [m]'), _field('widths', 'Widths [m]'),
    _field('angles', 'Angles about X, Y, Z [deg]'), _field('radius', 'Radius [m]'),
    _field('top_radius', 'Top radius [m]'), _field('length', 'Length or height [m]'),
    _field('corner', 'Corner radius [m]'), _field('dimensions', 'Dimensions [m]'),
    _field('counts', 'Counts along X, Y, Z'), _field('spacing', 'Spacing [m]'),
    _field('name', 'Name'))


def edit_solid(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.edit_solid`: a solid changed after it is made, only the
    fields typed (`Project.edit_solid`); a boolean it is part of changes
    with it, and a mesh made from it waits for Mesh Solids again."""
    from hertzforge.core import solids

    name, geometry = _geometry(project, params)
    solid_id = params['solid']
    changes: dict[str, Any] = {}
    for key in ('center', 'widths', 'angles', 'dimensions', 'spacing'):
        if params[key].strip():
            changes[key] = _numbers(params[key], f'the {key}')
    for key in ('radius', 'top_radius', 'length', 'corner'):
        if params[key].strip():
            values = _numbers(params[key], f'the {key.replace("_", " ")}')
            if len(values) != 1:
                raise refused(f'the {key.replace("_", " ")} is one number')
            changes[key] = values[0]
    if params['counts'].strip():
        changes['counts'] = [int(c) for c in _numbers(params['counts'], 'the counts')]
    if params['name'].strip():
        changes['name'] = params['name'].strip()
    if not changes:
        raise refused('type what changes: a field left empty keeps it')
    _call(lambda: project.edit_solid(name, solid_id, **changes))
    item = solids.solid(geometry, solid_id)
    rows = [v.row('Changed', f'{_label(item)}, solid {solid_id}: {", ".join(changes)}',
                  None, 'pass')]
    if geometry.num_nodes:
        rows.append(v.row('Mesh', 'Mesh Solids again to mesh the change', None, 'info'))
    return _result(name, geometry, f'Edited {name}', rows)


OFFSET = tuple(_length(f'offset_{axis}', f'Move along {axis.upper()}', 0.0) for axis in 'xyz')
TURN = tuple(_angle(axis, 'turn', 'Turn') for axis in 'xyz')


def move_solids(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.move_solids`: parts moved whole, each with every solid it
    is made of, turned about the middle of their extent and then moved
    (`Project.move_solids`): VD's Move Part, its gizmo's drag typed."""
    name, geometry = _geometry(project, params)
    ids = _ids(geometry, params['solids'])
    offset = tuple(params[f'offset_{a}'] for a in 'xyz')
    turn = _angles(params, 'turn')
    if not any(offset) and not any(turn):
        raise refused('type how far to move it or how far to turn it')
    moved = _call(lambda: project.move_solids(name, ids, offset, turn))
    return _result(name, geometry, f'Moved in {name}', [
        v.row('Moved', f'{", ".join(map(str, ids))}: {moved} solids with what they are '
              'made of', None, 'pass')])


INTO = Param('into', 'Into geometry', 'enum', options_from='objects:geometry')


def copy_solids(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.copy_solids`: solids copied, recipe and all, into the same
    geometry or another (`Project.copy_solids`): VD's Copy and Paste on
    solid rows."""
    name, geometry = _geometry(project, params)
    ids = _ids(geometry, params['solids'])
    into = params['into']
    made = _call(lambda: project.copy_solids(name, ids, into=into))
    target = project[into]
    return _result(into, target, f'Copied into {into}', [
        v.row('Copied', f'{", ".join(map(str, ids))} as {", ".join(map(str, made))}',
              None, 'pass'), _parts_row(target)])


WITHIN = Param('within', 'Within (a boolean\'s or pattern\'s id; 0 for the parts)',
               'integer', default=0, min=0, step=1)


def reorder_solids(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.reorder_solids`: the parts, or one boolean's solids, put in
    a new order, every one of them named (`Project.reorder_solids`): VD's
    drag in the tree. A subtraction cuts the rest from the first."""
    name, geometry = _geometry(project, params)
    ids = _ids(geometry, params['solids'])
    within = params['within'] or None
    _call(lambda: project.reorder_solids(name, ids, within=within))
    where = f'solid {within}\'s solids' if within else 'the parts'
    return _result(name, geometry, f'Reordered in {name}', [
        v.row('Order', f'{where}: {", ".join(map(str, ids))}', None, 'pass')])


OPERATION = Param('operation', 'Operation', 'enum', default='union', options=(
    Option('union', 'Union'), Option('subtract', 'Subtract (the rest from the first)'),
    Option('intersect', 'Intersect')))


def combine_solids(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.combine_solids`: two or more parts made one by a union, a
    subtraction of the rest from the first, or an intersection
    (`Project.combine_solids`)."""
    from hertzforge.core import solids

    name, geometry = _geometry(project, params)
    ids = _ids(geometry, params['solids'])
    new = _call(lambda: project.combine_solids(name, params['operation'], ids,
                                               params['name'].strip()))
    rows = [v.row('Made', f'{_label(solids.solid(geometry, new))}, solid {new}, of '
                  f'{", ".join(map(str, ids))}', None, 'pass')]
    try:
        rows.append(v.row('Volume', solids.volume(geometry, new),
                          'm³' if geometry.units_defined else None, 'info'))
    except ValueError as error:
        rows.append(v.row('Volume', f'not read: {error}', None, 'warn'))
    return _result(name, geometry, f'Combined in {name}', [*rows, _parts_row(geometry)])


def separate_solids(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.separate_solids`: booleans and patterns taken apart, the
    solids they were made of parts again (`Project.separate_solids`):
    VD's Take Apart."""
    name, geometry = _geometry(project, params)
    ids = _ids(geometry, params['solids'])
    freed = _call(lambda: project.separate_solids(name, ids))
    return _result(name, geometry, f'Taken apart in {name}', [
        v.row('Taken apart', f'{", ".join(map(str, ids))} into {freed} solids', None,
              'pass'), _parts_row(geometry)])


def delete_solids(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.delete_solids`: solids deleted, each with every solid it is
    made of (`Project.delete_solids`); a mesh made from them stays."""
    name, geometry = _geometry(project, params)
    ids = _ids(geometry, params['solids'])
    gone = _call(lambda: project.delete_solids(name, ids))
    return _result(name, geometry, f'Deleted from {name}', [
        v.row('Deleted', f'{gone} solids', None, 'pass'), _parts_row(geometry)])


COUNTS = tuple(Param(f'count_{axis}', f'How many along {axis.upper()}', 'integer',
                     default=2 if axis == 'x' else 1, min=1, step=1) for axis in 'xyz')
SPACING = tuple(_length(f'spacing_{axis}', f'Spacing along {axis.upper()}',
                        1.0 if axis == 'x' else 0.0) for axis in 'xyz')


def pattern_solids(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.pattern_solids`: parts copied in rows, columns and layers
    along X, Y and Z, the originals counted, as one solid whose copies
    follow them (`Project.pattern_solids`)."""
    name, geometry = _geometry(project, params)
    ids = _ids(geometry, params['solids'])
    counts = [params[f'count_{a}'] for a in 'xyz']
    copies = counts[0] * counts[1] * counts[2] - 1
    if not copies:
        raise refused('one of each along every axis: no copies yet')
    made = _call(lambda: project.pattern_solids(
        name, ids, counts, [params[f'spacing_{a}'] for a in 'xyz']))
    total = len(ids) * copies
    return _result(name, geometry, f'Patterned in {name}', [
        v.row('Made', f'pattern {made}', None, 'pass'),
        v.row('Copies', float(total), '', 'info'), _parts_row(geometry)])


# ---- Mesh Solids ----------------------------------------------------------------

ELEMENTS = Param('elements', 'Elements', 'enum', default='hexes', options=(
    Option('hexes', 'Hexes'), Option('tetras', 'Tetras'),
    Option('quads', 'Shells (quads)'), Option('triangles', 'Shells (triangles)')))
SIZE = Param('size', 'Element size (0 for the size it is likely to hold at)', 'number',
             default=0.0, unit='m', min=0.0, step=0.001)
APART = Param('apart', 'Keep apart from other groups', 'boolean', default=False)
PARTS_APART = Param('parts_apart', 'Keep the parts apart from each other', 'boolean',
                    default=False)
ANYWAY = Param('anyway', 'Mesh even where the fit is likely to fold', 'boolean',
               default=False)
MESH_KEEP = Param('keep', 'Mesh: add the elements (else the preview)', 'boolean',
                  default=False)

#: past these VD counts the preview and does not draw it
PREVIEW_BRICKS = 50_000
PREVIEW_TETS = 5_000
PREVIEW_FITTED = 1_500


def _likely_size(geometry: Any, ids: list[int], elements: str) -> float:
    """The size VD's pane opens on: an extrusion's half the thinnest
    feature for bricks, a flat part's across its plane for shells, and
    otherwise `hexmesh.suggested_size`."""
    from hertzforge.core import hexmesh, shells, solids

    if elements in ('quads', 'triangles'):
        return shells.mesh_size(geometry, ids)
    if elements == 'hexes':
        try:
            solids.extrusion_axis(geometry, ids)
            return solids.mesh_size(geometry, ids)
        except solids.NotAnExtrusion:
            pass
    return hexmesh.suggested_size(geometry, ids, elements)


def _fold_check(geometry: Any, ids: list[int], size: float) -> str:
    """VD's question before a fit is paid for, as words, or '' when the
    bricks are swept or the fit is likely to hold
    (`hexmesh.likely_to_fold` on each fitted piece)."""
    from hertzforge.core import hexmesh, solids

    try:
        solids.extrusion_axis(geometry, ids)
        return ''
    except solids.NotAnExtrusion:
        pass
    for entry in solids.piece_plan(geometry, ids, size):
        if entry['method'] != 'overlay':
            continue
        piece = ids if entry['id'] is None else [entry['id']]
        with solids.meshed_as(geometry, piece[0], entry['cuts']) as read_as:
            shown = [read_as, *piece[1:]]
            read = hexmesh.likely_to_fold(geometry, shown, entry['size'])
            if not read['likely']:
                continue
            bricks = solids.brick_count(geometry, shown, read['suggested'])
            cells = hexmesh.grid_cells(geometry, shown, read['suggested'], piece=True)
        why = []
        if np.isfinite(read['across']) and read['across'] < hexmesh.ACROSS - 0.5:
            why.append(f'the smallest round feature is {read["across"]:.1f} bricks across '
                       f'its radius, and fits have held at {hexmesh.ACROSS:g} or more')
        if np.isfinite(read['through']) and read['through'] < hexmesh.THROUGH - 0.5:
            why.append(f'the thinnest wall is {read["through"]:.1f} bricks through, '
                       f'and a fit needs {hexmesh.THROUGH:g}')
        if read['flat']:
            why.append(f'{read["flat"]:,} bricks are flat before any fitting')
        said = (f'{entry["name"]}: at {entry["size"]:.4g} m the fit is likely to fold: '
                f'{"; ".join(why)}. {read["suggested"]:.3g} m is likely to hold')
        if bricks > solids.MESH_LIMIT or cells > solids.GRID_LIMIT:
            said += (', but that is too many bricks: the part wants a finer size on its '
                     'thin piece than on the rest, which one fit cannot give; Tetras '
                     'can mesh it')
        return said
    return ''


def _made_rows(made: dict, earlier: int) -> list[dict]:
    """What a mesh holds, as VD's Mesh Solids pane says it."""
    rows = []
    method = made.get('method')
    if 'tets' in made:
        rows += [v.row('Ten-node tetrahedra', float(made['tets']), '', 'pass'),
                 v.row('Nodes', float(made['nodes']), '', 'info'),
                 v.row('Settled in', float(made['steps']), 'steps', 'info'),
                 v.row('Worst radius ratio', float(made['quality']), '',
                       'warn' if made['poor'] else 'pass'),
                 v.row('Under 0.3', float(made['poor']), '', 'info')]
    elif method == 'shells':
        rows += [v.row('Shells', float(made['elements']), '', 'pass'),
                 v.row('Nodes', float(made['nodes']), '', 'info'),
                 v.row('Where', ' and '.join(w for w, n in (
                     ('on their plane', made['flat']),
                     ('on the outer surface', made['surfaces'])) if n), None, 'info'),
                 v.row('Thickness', 'set on the element group', None, 'info')]
    else:
        rows += [v.row('Bricks', float(made['bricks']), '', 'pass'),
                 v.row('Nodes', float(made['nodes']), '', 'info')]
        if method == 'pieces':
            for piece in made['pieces']:
                how = ('fitted' if piece['method'] == 'overlay' else
                       'swept along its own axis' if piece.get('turned') else
                       f'swept along {"XYZ"[piece["axis"]]}')
                rows.append(v.row(piece['name'], f'{how} at {piece["size"]:.4g} m, '
                                  f'{piece["bricks"]:,} bricks', None, 'info'))
            rows.append(v.row('Shared at the seams', float(made['seam_shared']), 'nodes',
                              'info'))
            if made['ties']:
                rows.append(v.row('Joined rigidly at the seams', float(made['ties']),
                                  'nodes', 'info'))
        elif method == 'overlay':
            rows += [v.row('Method', 'a grid fitted to the shape (not an extrusion)',
                           None, 'info'),
                     v.row('Worst scaled Jacobian', float(made['quality']), '',
                           'warn' if made['poor'] else 'pass'),
                     v.row('Under 0.3', float(made['poor']), '', 'info')]
        else:
            rows.append(v.row('Method', f'extruded along {"XYZ"[made["axis"]]}', None,
                              'info'))
            if made.get('rounded'):
                rows.append(v.row('Nodes on circles', float(made['rounded']), '', 'info'))
    if 'shortest' in made:
        rows.append(v.row('Shortest edge', float(made['shortest']), 'm', 'info'))
    if earlier:
        rows.append(v.row('Replaces', f'the {earlier:,} elements meshed before', None,
                          'warn'))
    return rows


def _preview(geometry: Any, ids: list[int], size: float, elements: str,
             parts_apart: bool) -> tuple[Any, list[dict]]:
    """The mesh Mesh Solids would make, unjoined, and what it holds; or
    no mesh and its count past VD's preview budgets."""
    from hertzforge.core import shells, solids, tetmesh

    if elements in ('quads', 'triangles'):
        flat = [i for i in ids if solids.is_flat(geometry, i)]
        rest = [i for i in ids if i not in flat]
        expected = sum(solids.area(geometry, i) for i in flat) / size ** 2
        if expected > PREVIEW_BRICKS:
            return None, [v.row('Plates', f'about {int(expected):,} at this size; Mesh '
                                'makes them', None, 'info')]
        if rest:
            inside, budget, what = (
                (tetmesh.tet_count(geometry, rest, size), PREVIEW_TETS, 'tetrahedra')
                if elements == 'triangles' else
                (solids.brick_count(geometry, rest, size), PREVIEW_BRICKS, 'bricks'))
            if inside > budget:
                return None, [v.row(what.title(), f'about {inside:,} under the surface at '
                                    'this size; Mesh makes them', None, 'info')]
        part, made = shells.shell_mesh(geometry, ids, size,
                                       triangles=elements == 'triangles')
    elif elements == 'tetras':
        expected = tetmesh.tet_count(geometry, ids, size)
        if expected > PREVIEW_TETS:
            return None, [v.row('Tetrahedra', f'about {expected:,} at this size; Mesh '
                                'makes them', None, 'info')]
        part, made = tetmesh.tet_mesh(geometry, ids, size)
    else:
        plan = solids.piece_plan(geometry, ids, size)
        bricks = sum(entry['bricks'] for entry in plan)
        budget = (PREVIEW_FITTED if any(e['method'] == 'overlay' for e in plan)
                  else PREVIEW_BRICKS)
        if bricks > budget:
            return None, [v.row('Bricks', f'about {bricks:,} at this size; Mesh makes '
                                'them', None, 'info')]
        part, made = solids.brick_mesh(geometry, ids, size, parts_apart)
    earlier = len(solids.meshed_elements(geometry, part.group_name))
    return part, _made_rows(made, earlier)


def mesh_solids(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.mesh_solids`: parts meshed into hexes, tetras or shells at
    an element size, each part an element group (`Project.mesh_solids`).
    Without `keep` it is VD's live preview: the mesh the core's mesher
    makes (`solids.brick_mesh`, `tetmesh.tet_mesh`, `shells.shell_mesh`)
    drawn on its own and said, the project unchanged. A fitted brick mesh
    likely to fold (`hexmesh.likely_to_fold`) is said in the preview and
    made only with `anyway`, where VD asks. The size sets the time: a
    fine size on a large part is seconds to minutes."""
    from hertzforge.core import solids

    name, geometry = _geometry(project, params)
    ids = _ids(geometry, params['solids'], every=_parts(geometry))
    elements = params['elements']
    shell = elements in ('quads', 'triangles')
    flat = [i for i in ids if solids.is_flat(geometry, i)]
    if flat and not shell:
        raise refused(f'{solids.piece_name(geometry, flat[0])} is flat: a plate or a disk '
                      'is meshed into shells, quads or triangles')
    size = params['size'] or _call(lambda: _likely_size(geometry, ids, elements))
    fold = (_call(lambda: _fold_check(geometry, ids, size)) if elements == 'hexes'
            else '')
    said = [v.row('Solids', ', '.join(_label(solids.solid(geometry, i)) for i in ids),
                  None, 'info'),
            v.row('Element size', float(size), 'm', 'info')]
    if fold:
        said.append(v.row('Fit', fold, None, 'warn'))
    if not params['keep']:
        part, rows = _call(lambda: _preview(geometry, ids, size, elements,
                                            params['parts_apart']))
        out = [v.reading(f'Mesh Solids preview of {name}', said + rows)]
        if part is not None:
            out.append(_drawn(f'Mesh preview of {name}', part,
                              geometry.units_defined)[0])
        return out
    if fold and not params['anyway']:
        raise refused(f'{fold}. Mesh at the size suggested, or ask to mesh anyway')
    found = _call(lambda: project.mesh_solids(
        name, ids, size, apart=params['apart'],
        parts_apart=params['parts_apart'] and elements == 'hexes', elements=elements))
    rows = _made_rows(found, 0)
    if found['replaced']:
        rows.append(v.row('Replaced', f'the {found["replaced"]:,} elements meshed before',
                          None, 'info'))
    rows += [v.row('Nodes added', float(found['added']), '', 'info'),
             v.row('Nodes shared with the geometry', float(found['shared']), '', 'info'),
             v.row('Nodes in all', float(geometry.num_nodes), '', 'info')]
    rows += [v.row(f'Group {group}', f'{held} taken off: solids take a material alone',
                   None, 'warn') for group, held in (found.get('cleared') or {}).items()]
    return _result(name, geometry, f'Meshed {name}', said + rows)


# ---- Mate Surfaces --------------------------------------------------------------

GROUPS = Param('groups', 'Element groups (empty for every group of plates or solids)',
               'string', default='')
MATE_TOLERANCE = Param('tolerance', 'Tolerance (0 for a quarter of the finest spacing)',
                       'number', default=0.0, unit='m', min=0.0, step=0.0001)
HELD = tuple(Option(mode, label) for mode, label in (
    ('rigid', 'Rigid'), ('stiffness', 'Stiffness'), ('free', 'Free')))
NORMAL = Param('normal', 'Normal', 'enum', default='rigid', options=HELD)
SHEAR = Param('shear', 'Shear', 'enum', default='rigid', options=HELD)
NORMAL_STIFFNESS = Param('normal_stiffness', 'Normal stiffness', 'number', default=1e6,
                         unit='N/m', min=0.0)
SHEAR_STIFFNESS = Param('shear_stiffness', 'Shear stiffness', 'number', default=1e5,
                        unit='N/m', min=0.0)
PATCHES = Param('patches', 'Patches to keep (empty for every one)', 'string', default='')
MATE_KEEP = Param('keep', 'Keep: add the patches to the Mated Surfaces', 'boolean',
                  default=False)


def _group_ids(geometry: Any, text: str) -> list[int] | None:
    known = [int(g) for g in geometry.group_id]
    if not text.strip():
        return None
    chosen = []
    for part in text.replace(' ', '').split(','):
        try:
            group = int(part)
        except ValueError:
            raise refused(f'{part!r} is not an element group number') from None
        if group not in known:
            raise refused(f'the geometry has element groups '
                          f'{", ".join(map(str, known)) or "none"}; not {group}')
        chosen.append(group)
    return chosen


def _group_name(geometry: Any, group: int) -> str:
    names = dict(zip((int(g) for g in geometry.group_id), geometry.group_name, strict=True))
    return str(names.get(int(group), '')).strip() or f'element group {group}'


def _held(params: dict[str, Any]) -> dict[str, Any]:
    return {'normal_mode': params['normal'], 'shear_mode': params['shear'],
            'normal_stiffness': params['normal_stiffness']
            if params['normal'] == 'stiffness' else 0.0,
            'shear_stiffness': params['shear_stiffness']
            if params['shear'] == 'stiffness' else 0.0}


def _mates_table(name: str, geometry: Any) -> dict:
    """The geometry's Mated Surfaces, as VD lists them under it."""
    rows = []
    for mated in geometry.mates:
        how = mated.describe()
        springs = [f'{what} {value:.4g} N/m' for what, mode, value in (
            ('normal', mated.normal_mode, mated.normal_stiffness),
            ('shear', mated.shear_mode, mated.shear_stiffness)) if mode == 'stiffness']
        first, second = (_group_name(geometry, g) for g in mated.groups)
        rows.append([str(mated.id), mated.name, f'{first} on {second}',
                     f'{len(mated.nodes_a):,} on {len(mated.nodes_b):,}',
                     how + (f' ({", ".join(springs)})' if springs else '')])
    return v.table(f'Mated Surfaces of {name}', ['Id', 'Name', 'Groups', 'Nodes', 'Held'],
                   rows)


def mate_surfaces(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.mate_surfaces`: the patches where element groups' surfaces
    mate (`Project.detect_mated_surfaces`, `core.mates.detect`), listed;
    with `keep`, each picked patch added to the geometry's Mated Surfaces,
    held along the normal and in the plane rigidly, by a stiffness or not
    at all (`Project.add_mated_surfaces`). The tolerance opens on VD's: a
    quarter of the groups' finest node spacing."""
    name, geometry = _geometry(project, params)
    groups = _group_ids(geometry, params['groups'])
    tolerance = params['tolerance']
    if not tolerance:
        spacing = geometry.node_spacing(groups)
        low, high = geometry.extent
        tolerance = (0.25 * spacing if spacing
                     else 1e-6 * float(np.linalg.norm(high - low) or 1.0))
    found = _call(lambda: project.detect_mated_surfaces(name, groups, tolerance))
    if not found:
        raise refused(f'no surface of these element groups lies on another within '
                      f'{tolerance:.4g} m')
    rows = [v.row('Tolerance', float(tolerance), 'm', 'info'),
            v.row('Patches found', float(len(found)), '', 'pass' if found else 'warn')]
    patches = v.table(f'Patches of {name}', ['Patch', 'First side (follows)', 'Second side',
                                             'Nodes'], [
        [str(k + 1), _group_name(geometry, p['groups'][0]),
         _group_name(geometry, p['groups'][1]),
         f'{len(p["nodes_a"]):,} on {len(p["nodes_b"]):,}'] for k, p in enumerate(found)])
    if params['keep']:
        if params['patches'].strip():
            picked = [int(n) for n in _numbers(params['patches'], 'the patches')]
            wrong = [k for k in picked if not 1 <= k <= len(found)]
            if wrong:
                raise refused(f'{len(found)} patches were found; there is no patch {wrong[0]}')
        else:
            picked = list(range(1, len(found) + 1))
        held = _held(params)
        kept = []
        for k in picked:
            patch = found[k - 1]
            kept.append(_call(lambda patch=patch: project.add_mated_surfaces(
                name, patch['groups'], patch['nodes_a'], patch['nodes_b'], **held)))
        rows.append(v.row('Kept', ', '.join(m.name for m in geometry.mates if m.id in kept),
                          None, 'pass'))
    view, _notes = _drawn(name, geometry)
    return [v.reading(f'Mate Surfaces of {name}', rows), patches, view,
            _mates_table(name, geometry)]


MATE = Param('mate', 'Mated surfaces (id)', 'integer', default=1, min=1, step=1)
MATE_NAME = Param('name', 'Name (empty keeps it)', 'string', default='')


def edit_mated_surfaces(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.edit_mated_surfaces`: a kept set's name and how it is held
    changed (`Project.edit_mated_surfaces`): VD's Save on a set's pane."""
    name, geometry = _geometry(project, params)
    changes = _held(params)
    if params['name'].strip():
        changes['name'] = params['name'].strip()
    _call(lambda: project.edit_mated_surfaces(name, params['mate'], **changes))
    (mated,) = [m for m in geometry.mates if m.id == params['mate']]
    return [v.reading(f'Mated surfaces of {name}', [
        v.row('Saved', f'{mated.name} ({mated.describe()})', None, 'pass')]),
        _mates_table(name, geometry)]


MATES = Param('mates', 'Mated surfaces (ids, as "1, 3")', 'string', default='')


def delete_mated_surfaces(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.delete_mated_surfaces`: kept sets deleted by id
    (`Project.delete_mated_surfaces`)."""
    name, geometry = _geometry(project, params)
    known = [m.id for m in geometry.mates]
    ids = [int(i) for i in _numbers(params['mates'], 'the mated surfaces')]
    if not ids:
        raise refused(f'name the mated surfaces by id; the geometry has '
                      f'{", ".join(map(str, known)) or "none"}')
    wrong = [i for i in ids if i not in known]
    if wrong:
        raise refused(f'the geometry has mated surfaces {", ".join(map(str, known)) or "none"}; '
                      f'not {wrong[0]}')
    gone = project.delete_mated_surfaces(name, ids)
    return [v.reading(f'Mated surfaces of {name}', [
        v.row('Deleted', float(gone), 'sets', 'pass')]), _mates_table(name, geometry)]


# ---- Set Default View -----------------------------------------------------------

EYE = tuple(Param(f'eye_{axis}', f'Eye toward {axis.upper()}', 'number',
                  default=0.0, step=1.0) for axis in 'xyz')
UP = tuple(Param(f'up_{axis}', f'Up along {axis.upper()}', 'number',
                 default=1.0 if axis == 'z' else 0.0, step=1.0) for axis in 'xyz')


def set_view(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_view`: the view a geometry opens on, where the eye is
    seen from the model's center and which way is up (`Project.set_view`
    with a `View`); an eye of 0, 0, 0 returns to the default isometric
    view. VD captures the view on screen; here it is typed."""
    from hertzforge.core.geometry import View

    name, geometry = _geometry(project, params)
    eye = tuple(params[f'eye_{a}'] for a in 'xyz')
    up = tuple(params[f'up_{a}'] for a in 'xyz')
    view = None if not any(eye) else _call(lambda: View(eye=eye, up=up))
    project.set_view(name, view)
    held = geometry.view
    if held is None:
        rows = [v.row('View', 'the default: from +X+Y+Z, Z up', None, 'pass')]
    else:
        rows = [v.row('Eye', ', '.join(_f(c) for c in held.eye), None, 'pass'),
                v.row('Up', ', '.join(_f(c) for c in held.up), None, 'info')]
    return [v.reading(f'Default view of {name}', rows), _drawn(name, geometry)[0]]


OPERATIONS = (
    Operation(
        id='project.new_geometry', title='New Geometry', workspace='model',
        accepts='project',
        summary='An empty geometry to build a part in from solids, recording the length '
                'unit it is built in.',
        params=(GEOMETRY_NAME, UNIT), outputs=('reading', 'table'), run=new_geometry),
    Operation(
        id='project.solids', title='Solids', workspace='model', accepts='project',
        summary='A geometry\'s solids, each part and what it is made of, with their ids '
                'for the other solid tools, and the geometry drawn.',
        params=(GEOMETRY,), outputs=('reading', 'shape3d', 'table'), run=view_solids),
    Operation(
        id='project.add_solid_block', title='Add Block', workspace='model', accepts='project',
        summary='A block, its widths along its own axes, centered and turned; a corner radius '
                'rounds its four edges along its own Z.',
        params=(GEOMETRY, *CENTER, *WIDTHS, *ANGLES, CORNER, NAME),
        outputs=('reading', 'shape3d', 'table'), run=add_solid_block, edits=True),
    Operation(
        id='project.add_solid_cylinder', title='Add Cylinder', workspace='model',
        accepts='project',
        summary='A cylinder of a radius about its own Z and a length along it, centered and '
                'turned; subtracted from a part it makes a hole.',
        params=(GEOMETRY, *CENTER, _length('radius', 'Radius', 0.25, 0.0),
                _length('length', 'Length', 1.0, 0.0), *ANGLES, NAME),
        outputs=('reading', 'shape3d', 'table'), run=add_solid_cylinder, edits=True),
    Operation(
        id='project.add_solid_cone', title='Add Cone', workspace='model', accepts='project',
        summary='A cone or frustum along its own Z, a radius at its base and its top (either '
                'zero for a point), centered and turned.',
        params=(GEOMETRY, *CENTER, _length('radius', 'Base radius', 0.5, 0.0),
                _length('top_radius', 'Top radius', 0.25, 0.0),
                _length('height', 'Height', 1.0, 0.0), *ANGLES, NAME),
        outputs=('reading', 'shape3d', 'table'), run=add_solid_cone, edits=True),
    Operation(
        id='project.add_solid_sphere', title='Add Sphere', workspace='model',
        accepts='project', summary='A sphere of a radius about its center.',
        params=(GEOMETRY, *CENTER, _length('radius', 'Radius', 0.5, 0.0), NAME),
        outputs=('reading', 'shape3d', 'table'), run=add_solid_sphere, edits=True),
    Operation(
        id='project.add_solid_section', title='Add Section', workspace='model',
        accepts='project',
        summary='A structural section along its own Z. Dimensions in order: I-beam and '
                'channel depth, flange width, flange thickness, web thickness; tee the same; '
                'angle long leg, short leg, thickness; rectangular tube width, height, wall; '
                'round tube outer diameter, wall. Empty for the shape\'s opening ones.',
        params=(GEOMETRY, Param('shape', 'Shape', 'enum', default='I-beam',
                                options=_section_shapes()),
                Param('dimensions', 'Dimensions [m], in order', 'string', default=''),
                _length('length', 'Length', 4.0, 0.0), *CENTER, *ANGLES, NAME),
        outputs=('reading', 'shape3d', 'table'), run=add_solid_section, edits=True),
    Operation(
        id='project.add_solid_plate', title='Add Plate', workspace='model', accepts='project',
        summary='A flat rectangle in its own XY plane, centered and turned; it is meshed '
                'into shells, whose element group takes the thickness.',
        params=(GEOMETRY, *CENTER, *WIDTHS[:2], *ANGLES, NAME),
        outputs=('reading', 'shape3d', 'table'), run=add_solid_plate, edits=True),
    Operation(
        id='project.add_solid_disk', title='Add Disk', workspace='model', accepts='project',
        summary='A flat circle in its own XY plane, centered and turned; it is meshed into '
                'shells, whose element group takes the thickness.',
        params=(GEOMETRY, *CENTER, _length('radius', 'Radius', 0.5, 0.0), *ANGLES, NAME),
        outputs=('reading', 'shape3d', 'table'), run=add_solid_disk, edits=True),
    Operation(
        id='project.add_solid_hole', title='Add Hole', workspace='model', accepts='project',
        summary='A hole entering at its center and running down its own Z, a depth of 0 '
                'through the part, a counterbore diameter of 0 none; subtract it from a part.',
        params=(GEOMETRY, *CENTER, _length('diameter', 'Diameter', 0.25, 0.0),
                _length('depth', 'Depth (0 through)', 0.0, 0.0),
                _length('counterbore_diameter', 'Counterbore diameter (0 none)', 0.0, 0.0),
                _length('counterbore_depth', 'Counterbore depth', 0.0, 0.0), *ANGLES, NAME),
        outputs=('reading', 'shape3d', 'table'), run=add_solid_hole, edits=True),
    Operation(
        id='project.edit_solid', title='Edit Solid', workspace='model', accepts='project',
        summary='Change a solid after it is made: only the fields typed change, lists as '
                '"0, 0, 0.1". A boolean it is part of changes with it; Mesh Solids again to '
                'mesh the change.',
        params=(GEOMETRY, SOLID, *EDIT_FIELDS), outputs=('reading', 'shape3d', 'table'),
        run=edit_solid, edits=True),
    Operation(
        id='project.move_solids', title='Move Part', workspace='model', accepts='project',
        summary='Move parts whole, each with every solid it is made of: turned about the '
                'middle of their extent, then moved.',
        params=(GEOMETRY, SOLIDS, *OFFSET, *TURN), outputs=('reading', 'shape3d', 'table'),
        run=move_solids, edits=True),
    Operation(
        id='project.copy_solids', title='Copy Solids', workspace='model', accepts='project',
        summary='Copy solids, recipe and all, into the same geometry or another; a boolean '
                'or pattern comes with every solid it is made of.',
        params=(GEOMETRY, SOLIDS, INTO), outputs=('reading', 'shape3d', 'table'),
        run=copy_solids, edits=True),
    Operation(
        id='project.reorder_solids', title='Reorder Solids', workspace='model',
        accepts='project',
        summary='Put the parts, or one boolean\'s solids, in a new order, naming every one; '
                'a subtraction cuts the rest from the first.',
        params=(GEOMETRY, SOLIDS, WITHIN), outputs=('reading', 'shape3d', 'table'),
        run=reorder_solids, edits=True),
    Operation(
        id='project.combine_solids', title='Combine Solids', workspace='model',
        accepts='project',
        summary='Make two or more parts one: their union, the rest subtracted from the '
                'first, or their intersection.',
        params=(GEOMETRY, OPERATION, SOLIDS, NAME), outputs=('reading', 'shape3d', 'table'),
        run=combine_solids, edits=True),
    Operation(
        id='project.separate_solids', title='Take Apart', workspace='model',
        accepts='project',
        summary='Undo booleans or patterns: the solids they were made of stand on their own '
                'again.',
        params=(GEOMETRY, SOLIDS), outputs=('reading', 'shape3d', 'table'),
        run=separate_solids, edits=True),
    Operation(
        id='project.delete_solids', title='Delete Solids', workspace='model',
        accepts='project',
        summary='Delete solids, each with every solid it is made of; a mesh made from them '
                'stays.',
        params=(GEOMETRY, SOLIDS), outputs=('reading', 'shape3d', 'table'),
        run=delete_solids, edits=True),
    Operation(
        id='project.pattern_solids', title='Pattern', workspace='model', accepts='project',
        summary='Copy parts in rows, columns and layers along X, Y and Z, the originals '
                'counted, as one solid whose copies follow them.',
        params=(GEOMETRY, SOLIDS, *COUNTS, *SPACING), outputs=('reading', 'shape3d', 'table'),
        run=pattern_solids, edits=True),
    Operation(
        id='project.mesh_solids', title='Mesh Solids', workspace='model', accepts='project',
        summary='Mesh parts into hexes, tetras or shells, each part an element group; the '
                'preview until Mesh is ticked. The size sets the time: a fine size is slow.',
        params=(GEOMETRY, PARTS, ELEMENTS, SIZE, APART, PARTS_APART, ANYWAY, MESH_KEEP),
        outputs=('reading', 'shape3d', 'table'), run=mesh_solids, edits=True),
    Operation(
        id='project.mate_surfaces', title='Mate Surfaces', workspace='model',
        accepts='project',
        summary='Find where element groups\' surfaces mate; Keep ties each patch, along the '
                'normal and in the plane, rigidly, by a stiffness or not at all.',
        params=(GEOMETRY, GROUPS, MATE_TOLERANCE, NORMAL, NORMAL_STIFFNESS, SHEAR,
                SHEAR_STIFFNESS, PATCHES, MATE_KEEP),
        outputs=('reading', 'table', 'shape3d', 'table'), run=mate_surfaces, edits=True),
    Operation(
        id='project.edit_mated_surfaces', title='Edit Mated Surfaces', workspace='model',
        accepts='project', summary='Change how a kept set of mated surfaces is held, or its '
                                   'name.',
        params=(GEOMETRY, MATE, NORMAL, NORMAL_STIFFNESS, SHEAR, SHEAR_STIFFNESS, MATE_NAME),
        outputs=('reading', 'table'), run=edit_mated_surfaces, edits=True),
    Operation(
        id='project.delete_mated_surfaces', title='Delete Mated Surfaces', workspace='model',
        accepts='project', summary='Delete kept sets of mated surfaces, by id.',
        params=(GEOMETRY, MATES), outputs=('reading', 'table'), run=delete_mated_surfaces,
        edits=True),
    Operation(
        id='project.set_view', title='Set Default View', workspace='model', accepts='project',
        summary='The view a geometry opens on: where the eye is, seen from the model\'s '
                'center, and which way is up. An eye of 0, 0, 0 is the default isometric view.',
        params=(GEOMETRY, *EYE, *UP), outputs=('reading', 'shape3d'), run=set_view,
        edits=True),
)
