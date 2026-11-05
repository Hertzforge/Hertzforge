"""Planning a modal test and handing its model on: Visual Dynamics'
Plan Instrumentation, Export State-Space Model, Project onto Basis DOFs
narrowed to picked channels, and Animate the Projection.

Plan Instrumentation chooses accelerometers on a finite element model
by Effective Independence (`core.instrumentation`), as the pane's
preview does, and with Keep adds the plan's geometry, channel table and
shapes (`Project.plan_instrumentation`). Export State-Space Model
previews a shape set at a channel table's channels as a modal
state-space model (`core.state_space.state_space_model`) and, given a
file, writes it (`Project.export_state_space`). The two projection
tools sample one shape set at another's DOFs (`core.correlate.project_shapes`).
"""

from __future__ import annotations

import os
import warnings
from typing import Any

import numpy as np

from . import views as v
from .descriptors import Operation, Option, Param
from .mesh import drawn_mesh
from .mode_pairs import paired, translations
from .ops_model import _family, _groups
from .ops_project import _shape_view
from .protocol import refused
from .waterfalls import unit_text


def _picks(text: str, count: int, what: str) -> list[int] | None:
    """The 0-based items '1-3, 7' names, numbered from 1; None when
    `text` is blank."""
    if not text.strip():
        return None
    out: list[int] = []
    for part in text.replace(' ', '').split(','):
        first, dash, last = part.partition('-')
        try:
            low, high = int(first), int(last) if dash else int(first)
        except ValueError:
            raise refused(f'{part!r} is not a {what} number or a range such as 3-6') from None
        if not 1 <= low <= high <= count:
            raise refused(f'{what}s run from 1 to {count}; {part} is outside them')
        out.extend(range(low - 1, high))
    return list(dict.fromkeys(out))


def _runs(indices: Any) -> str:
    """0-based indices named by number from 1, run together where they
    follow on: '7-9, 11'."""
    runs: list[list[int]] = []
    for n in sorted(int(m) + 1 for m in indices):
        if runs and n == runs[-1][-1] + 1:
            runs[-1].append(n)
        else:
            runs.append([n])
    return ', '.join(f'{r[0]}' if len(r) == 1 else f'{r[0]}-{r[-1]}' for r in runs)


def _numbers(text: str, what: str) -> set[int]:
    try:
        return {int(t) for t in text.replace(',', ' ').split()}
    except ValueError:
        raise refused(f'{text!r} is not a list of {what} numbers') from None


# ---- Plan Instrumentation ---------------------------------------------------------

PLANNED = Param('source', 'Shape set', 'enum', options_from='verb:plan_instrumentation')
MODES = Param('modes', 'Modes (blank: every mode above 0 Hz)', 'string', default='')
TRIAXIAL = Param('triaxial', 'Tri-axial (0 of both: a third of the modes, and one)',
                 'integer', default=0, min=0, step=1)
UNIAXIAL = Param('uniaxial', 'Uni-axial', 'integer', default=0, min=0, step=1)
AXES = {'normal': (2,), 'any': (0, 1, 2)}
UNIAXIAL_ALONG = Param('axes', 'Uni-axial along', 'enum', default='normal', options=(
    Option('normal', 'Surface normal (Z)'), Option('any', 'Any direction')))
GROUPS = Param('groups', 'Element groups (blank: all)', 'string', default='')
LEAVE_OUT = Param('leave_out', 'Leave out surfaces', 'string', default='')
PUT_BACK = Param('put_back', 'Put back surfaces', 'string', default='')
EDGE_ANGLE = Param('edge_angle', 'Surface edge', 'number', default=20.0, unit='°',
                   min=1.0, max=179.0, step=5.0)
EDGE_DISTANCE = Param('edge_distance', 'Away from edges', 'number', default=0.25 * 0.0254,
                      unit='m', min=0.0, step=0.001)
PLAN_NAME = Param('name', 'Name (blank: the set\'s, with Plan)', 'string', default='')
KEEP = Param('keep', 'Keep: add the geometry, channels and shapes', 'boolean', default=False)


def _plannable_groups(geometry: Any, text: str) -> tuple[list[int], bool]:
    """The element groups sensors may go on, and whether that is every
    group with solids, plates or beams: the groups Visual Dynamics'
    pane lists, all checked when opened."""
    offered = [g for g in (int(g) for g in geometry.groups.ids)
               if _family(geometry, g) in ('volume', 'face', 'line', 'mixed')]
    chosen = _groups(geometry, text) if text.strip() else offered
    return chosen, set(offered) <= set(chosen)


def _surface_center(geometry: Any, surface: Any) -> np.ndarray:
    xyz = np.asarray(geometry.node_xyz, dtype=float)
    row_of = {int(n): k for k, n in enumerate(geometry.node_id)}
    centers = np.array([xyz[[row_of[int(n)] for n in face]].mean(axis=0)
                        for face in surface.faces])
    weights = np.where(surface.areas > 0, surface.areas, 1.0)
    return (centers * weights[:, None]).sum(axis=0) / weights.sum()


def _surface_faces(geometry: Any, drawn: np.ndarray, found: Any,
                   offered: set[int]) -> tuple[np.ndarray, np.ndarray]:
    """The model's surfaces as triangles of the drawn nodes, each face fanned
    from its first node, and each triangle's surface: its id when sensors
    may be offered it (one of the groups planned on), -1 otherwise."""
    row_of = {int(n): k for k, n in enumerate(geometry.node_id)}
    at = {int(row): k for k, row in enumerate(drawn)}
    triangles, owners = [], []
    for surface in found:
        owner = surface.id if surface.id in offered else -1
        for face in surface.faces:
            corners = [at.get(row_of.get(int(n), -1)) for n in face]
            if any(c is None for c in corners):
                continue
            for k in range(1, len(corners) - 1):
                triangles.append((corners[0], corners[k], corners[k + 1]))
                owners.append(owner)
    return (np.asarray(triangles, dtype=np.int64).reshape(-1, 3),
            np.asarray(owners, dtype=np.int32))


def _sensor_view(title: str, geometry: Any, shapes: Any, modes: list[int],
                 places: Any, placement: Any, surfaces: Any = None) -> dict:
    """The model with its measured modes, and each chosen sensor drawn
    as short lines along the directions it measures: a tri-axial one
    its place's three axes, a uni-axial one its single axis. The legs
    are extra nodes and lines of the mesh that ride with the node the
    sensor sits on, since a `shape3d` has no markers of its own.

    `surfaces` is (found, offered ids, kept ids, params): the model's
    faces are then its surfaces', and a click on one leaves it out or puts
    it back, as a click does in Visual Dynamics' pane."""
    from hertzforge.drawing.viz.geometry import dof_arrow_length

    drawn, triangles, lines = drawn_mesh(geometry)
    xyz = np.asarray(geometry.node_xyz, dtype=float)
    row_of = {int(n): k for k, n in enumerate(geometry.node_id)}
    frames = places.frames
    legs = ([(k, axis) for k in placement.triaxial for axis in range(3)]
            + [(k, axis) for k, axis in placement.uniaxial])
    seats = list(dict.fromkeys(k for k, _axis in legs))
    seat_rows = np.array([row_of[int(places.source[k])] for k in seats], dtype=np.int64)
    leg_rows = np.array([row_of[int(places.source[k])] for k, _axis in legs], dtype=np.int64)
    points = xyz[drawn]
    extent = float(np.sqrt(((points - points.mean(axis=0)) ** 2).sum(axis=1).max())) or 1.0
    length = dof_arrow_length(xyz[seat_rows], extent)
    tips = xyz[leg_rows] + length * np.array([frames[k][axis] for k, axis in legs])
    seat_index = {k: len(drawn) + n for n, k in enumerate(seats)}
    first_tip = len(drawn) + len(seats)
    sensor_lines = np.array([(seat_index[k], first_tip + n) for n, (k, _a) in enumerate(legs)],
                            dtype=np.int64).reshape(-1, 2)
    rows = np.concatenate([drawn, seat_rows, leg_rows])
    nodes = np.vstack([points, xyz[seat_rows], tips])
    shown = [v.mode(m + 1, float(shapes.frequency[m]),
                    float(shapes.damping[m]) if shapes.damping[m] > 0 else None,
                    np.real(translations(geometry, shapes.coordinate,
                                          shapes.shape_matrix[m], rows)))
             for m in modes]
    extra: dict[str, Any] = {}
    if surfaces is not None:
        found, offered, kept, params = surfaces
        faces, owners = _surface_faces(geometry, drawn, found, offered)
        if len(faces):
            triangles = faces
            extra['pick'] = v.picking(
                'project.plan_instrumentation', {**params, 'keep': False}, 'faces',
                'Click a surface to leave it out of the places sensors may go, or put it back',
                faces=owners, kept=sorted(kept), out='leave_out', back='put_back')
    return v.shape3d(title, 'm' if geometry.units_defined else 'undefined', nodes, shown,
                     triangles=triangles, **extra,
                     lines=np.vstack([np.asarray(lines, dtype=np.int64).reshape(-1, 2),
                                      sensor_lines]),
                     normalization='unscaled' if getattr(shapes, 'unscaled', False)
                     else 'mass')


def plan_instrumentation(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.plan_instrumentation`: accelerometers chosen on a finite
    element model by Effective Independence, tri-axial ones first, as
    Visual Dynamics' Plan Instrumentation pane previews them: the
    model's surfaces (`core.instrumentation.skin`, small ones left out
    by `default_surfaces`), the places on them (`candidates`), the modes
    in each place's directions (`local_shapes`), the choice
    (`effective_independence`) and the plan's objects (`assemble`). Its
    reading is the pane's: the sensors, the channels, the worst
    off-diagonal auto-MAC at those channels and the condition number of
    the information matrix. Keep adds the experimental geometry, the
    channel table and the shapes (`Project.plan_instrumentation`). A
    click on a surface in the pane is a typed surface number here, from
    the surfaces table."""
    from hertzforge.core.instrumentation import (
        FewerDirectionsWarning,
        assemble,
        candidates,
        default_surfaces,
        effective_independence,
        elastic,
        local_shapes,
        lowest_first,
        skin,
    )

    source = params['source']
    shapes = project[source]
    geometry_name, geometry = project.geometry_for(source)
    modes = _picks(params['modes'], shapes.num_shapes, 'mode') or elastic(shapes)
    if not modes:
        raise refused('every mode is a rigid-body mode at 0 Hz: name the modes to measure')
    groups, every_group = _plannable_groups(geometry, params['groups'])
    if not groups:
        raise refused(f'{geometry_name} has no solids, plates or beams to place sensors on')
    angle = float(params['edge_angle'])
    found = skin(geometry, edge_angle=angle)
    numbers = {s.id for s in found}
    out, back = (_numbers(params[key], 'surface') for key in ('leave_out', 'put_back'))
    unknown = sorted((out | back) - numbers)
    if unknown:
        raise refused(f'the model has surfaces 1 to {len(found)} at a {angle:g}° edge; '
                      f'not {", ".join(map(str, unknown))}')
    if out & back:
        raise refused(f'surface {", ".join(map(str, sorted(out & back)))} is both left out '
                      'and put back; name it in one')
    default = set(default_surfaces(found))
    offered = [s for s in found if s.group in set(groups)]
    kept = [s.id for s in offered if (s.id in default or s.id in back) and s.id not in out]
    triaxial, uniaxial = params['triaxial'], params['uniaxial']
    opened = not triaxial and not uniaxial
    if opened:
        triaxial = -(-len(modes) // 3) + 1
    distance = float(params['edge_distance'])
    axes = AXES[params['axes']]
    places = candidates(geometry, groups, kept, edge_angle=angle, found=found,
                        edge_distance=distance)
    blocks, measured = local_shapes(shapes, modes, places, geometry)
    try:
        placement = effective_independence(blocks, triaxial, uniaxial, axes=axes,
                                           allowed=measured, sources=places.source,
                                           lowest=lowest_first(shapes, modes))
    except ValueError as error:
        raise refused(str(error)) from None
    plan = assemble(shapes, modes, places, blocks, placement)
    mac = plan.shapes.auto_mac()
    worst = float((mac - np.diag(np.diag(mac))).max()) if len(modes) > 1 else 0.0
    base = params['name'].strip() or f'{source} Plan'
    low, high = (float(shapes.frequency[m]) for m in (min(modes, key=shapes.frequency.__getitem__),
                                                      max(modes, key=shapes.frequency.__getitem__)))
    how = ('every mode above 0 Hz' if modes == elastic(shapes) else 'the modes named')
    rows = [v.row('Shape set', f'{source}, on {geometry_name}', None, 'info'),
            v.row('Measuring', f'{len(modes)} mode{"s" * (len(modes) != 1)} ({_runs(modes)}), '
                  f'{low:,.4g} to {high:,.4g} Hz: {how}', None, 'info'),
            v.row('Surfaces', f'sensors on {len(kept)} of {len(offered)}', None, 'info'),
            v.row('Sensors', f'{len(placement.triaxial)} tri-axial and '
                  f'{len(placement.uniaxial)} uni-axial'
                  + (', the tri-axial count a third of the modes, and one' if opened else ''),
                  None, 'info'),
            v.row('Channels', float(plan.channels.num_channels), '', 'info'),
            v.row('Places', f'{plan.geometry.num_nodes} of {len(places.source):,}', None, 'info'),
            v.row('Worst off-diagonal MAC', worst, '', 'info'),
            v.row('Condition number', placement.condition, '', 'info')]
    if len(placement.planned) < len(modes):
        left = [m for k, m in enumerate(modes) if k not in set(placement.planned.tolist())]
        rows.append(v.row('Warning', f'{len(placement.planned)} directions for {len(modes)} '
                          f'modes, planned for the lowest {len(placement.planned)}; modes '
                          f'{_runs(left)} cannot be told apart by these sensors', None, 'warn'))
    if params['keep']:
        with warnings.catch_warnings():
            warnings.simplefilter('ignore', FewerDirectionsWarning)
            try:
                added = project.plan_instrumentation(
                    source, triaxial, uniaxial, modes=modes,
                    groups=None if every_group else groups, surfaces=kept, edge_angle=angle,
                    axes=list(axes), name=base, edge_distance=distance, unit='m')
            except ValueError as error:
                raise refused(str(error)) from None
        rows.append(v.row('Kept', f'{added["geometry"]}, {added["channels"]} and '
                          f'{added["shapes"]}', None, 'pass'))
    else:
        rows.append(v.row('Not kept', 'run with Keep to add the geometry, the channel table '
                          'and the modes at those channels', None, 'info'))

    unit = ' [m]' if geometry.units_defined else ''
    frames = places.frames
    shares = list(placement.independence)
    seats = ([(k, 'X, Y, Z', 'tri-axial') for k in placement.triaxial]
             + [(k, 'XYZ'[axis], 'uni-axial') for k, axis in placement.uniaxial])
    position = np.asarray(places.geometry.node_xyz, dtype=float)
    sensors = [[f'{n} {kind}', str(int(places.geometry.node_id[k])), str(int(places.surface[k])),
                *(f'{c:.6g}' for c in position[k]), measures,
                ', '.join(f'{c:.3f}' for c in frames[k][2]), f'{share:.3f}']
               for n, ((k, measures, kind), share) in enumerate(zip(seats, shares, strict=True),
                                                               start=1)]
    sensor_table = v.table(
        f'{base}: sensors', ['Sensor', 'Node', 'Surface', f'X{unit}', f'Y{unit}', f'Z{unit}',
                             'Measures', 'Normal (Z)', 'Share of information'], sensors)
    area = ' [m²]' if geometry.units_defined else ''
    picked = set(groups)
    surfaces = []
    for s in found:
        if s.group not in picked:
            state = 'no: element group not picked'
        elif s.id in kept:
            state = 'yes'
        elif s.id in out:
            state = 'no: left out'
        else:
            state = 'no: under 1 % of the outside\'s area'
        center = _surface_center(geometry, s)
        surfaces.append([str(s.id), str(s.group), s.kind, f'{s.area:.4g}', str(len(s.faces)),
                         *(f'{c:.4g}' for c in center), state])
    surface_table = v.table(
        f'{geometry_name}: surfaces at a {angle:g}° edge',
        ['Surface', 'Element group', 'Kind', f'Area{area}', 'Faces', f'Center X{unit}',
         f'Center Y{unit}', f'Center Z{unit}', 'Sensors may go'], surfaces)

    def axis(name: str) -> dict:
        return {'label': name, 'ticks': [f'{shapes.frequency[m]:.1f} Hz' for m in modes]}

    matrix = v.matrix(f'Auto-MAC of {source} at the planned channels', axis(source),
                      axis(source), mac, 'MAC', (0.0, 1.0))
    view = _sensor_view(f'{base}: {len(placement.triaxial)} tri-axial, '
                        f'{len(placement.uniaxial)} uni-axial on {geometry_name}',
                        geometry, shapes, modes, places, placement,
                        (found, {s.id for s in offered}, set(kept), params))
    return [view, v.reading(f'Plan Instrumentation: {base}', rows), matrix, sensor_table,
            surface_table]


# ---- Export State-Space Model -----------------------------------------------------

STATE_SHAPES = Param('shapes', 'Shape set', 'enum', options_from='objects:shape_set')
TABLE = Param('table', 'Channel table', 'enum', options_from='objects:channel_table')
MAXIMUM = Param('maximum', 'Maximum (0 keeps every mode)', 'number', default=0.0, unit='Hz',
                min=0.0, step=100.0)
RIGID = Param('rigid', 'Rigid', 'number', default=1.0, unit='Hz', min=0.01, step=0.5)
PATH = Param('path', 'File (.npz; blank previews only)', 'string', default='')

#: lines in the preview's FRFs, as Visual Dynamics draws them
PREVIEW_LINES = 1600


def export_state_space(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.export_state_space`: a shape set at a channel table's
    channels as the modal state-space model Rattlesnake's state-space
    hardware loads (`core.state_space.state_space_model`), modes at and
    above the maximum frequency left out and those below the rigid
    frequency moved up to it. The preview is Visual Dynamics': each
    drive point's FRF from the model's own matrices, beside the whole
    set's synthesized FRF dashed (`ShapeSet.synthesize_frf`), and the
    pane's rows. Given a file, the model is written there
    (`Project.export_state_space`); a blank unit cell takes SI's unit."""
    from hertzforge.core.state_space import OUTPUT_POWER, state_space_model
    from hertzforge.units import si_factor

    set_name, table_name = params['shapes'], params['table']
    shapes, table = project[set_name], project[table_name]
    maximum = float(params['maximum']) or None
    rigid = float(params['rigid'])
    path = os.path.expanduser(params['path'].strip())
    if path and not os.path.isabs(path):
        raise refused(f'{params["path"]!r} is not a full path; name where to write the .npz')
    if path and not path.lower().endswith('.npz'):
        path += '.npz'
    try:
        model = state_space_model(shapes, table, maximum_frequency=maximum,
                                  rigid_frequency=rigid)
        if path:
            model = project.export_state_space(set_name, table_name, path,
                                               maximum_frequency=maximum,
                                               rigid_frequency=rigid)
    except (ValueError, TypeError, OSError) as error:
        raise refused(str(error)) from None
    rows = [v.row('Modes', f'{len(model.mode_frequency)} of {shapes.num_shapes}', None, 'info'),
            v.row('Moved up', float(model.lifted), '', 'info'),
            v.row('Left out', float(model.dropped), '', 'info'),
            v.row('Inputs', float(len(model.input_dof)), '', 'info'),
            v.row('Channels', float(len(model.channel_dof)), '', 'info'),
            v.row('Units', ', '.join(dict.fromkeys(model.channel_unit)), None, 'info')]
    if maximum is None:
        rows.append(v.row('Note', f'every mode kept, the highest at '
                          f'{shapes.frequency.max():.4g} Hz; set the controller\'s Nyquist '
                          'frequency to leave out the modes above it', None, 'warn'))
    rows.append(v.row('File', f'wrote {path}: {model.summary()}', None, 'pass') if path else
                v.row('File', 'not written: name a .npz to write the model', None, 'info'))

    kinds, dofs = table.types(), model.channel_dof
    inputs = [row for row, role in enumerate(table.roles()) if role == 'reference']
    responses = [row for row in range(len(dofs)) if row not in inputs]
    pairs = [(row, k) for k, i in enumerate(inputs) for row in responses
             if dofs[row].rstrip('+-') == dofs[i].rstrip('+-')]
    if not pairs and responses:
        pairs = [(responses[0], k) for k in range(len(inputs))]
    reading = v.reading(f'State-space model of {set_name} at {table_name}', rows)
    if not pairs:
        return [reading]
    top = 1.25 * (maximum or float(model.mode_frequency.max()))
    lines = np.linspace(top / PREVIEW_LINES, top, PREVIEW_LINES)
    h = model.frf(lines)
    series = []
    for row, k in pairs:
        si = si_factor(model.channel_unit[row], kinds[row]) / si_factor(
            model.channel_unit[inputs[k]], 'force')
        label = f'{dofs[row]} / {model.input_dof[k]}'
        series.append(v.series(f'{label}, model', 'measured', lines, np.abs(h[:, row, k]) * si))
        full = shapes.synthesize_frf(lines, [dofs[row]], [model.input_dof[k]],
                                     power=OUTPUT_POWER[kinds[row]])[0]
        series.append(v.series(f'{label}, every mode', 'reference', lines, np.abs(full),
                               follows=len(series) - 1))
    units = {unit_text(f'{kinds[row]}/force') for row, _k in pairs}
    view = v.spectrum(f'{set_name} model at the drive points', v.axis('Frequency', 'Hz', 'linear'),
                      v.axis('FRF magnitude', units.pop() if len(units) == 1 else None, 'log'),
                      series)
    return [view, reading]


# ---- Projection ---------------------------------------------------------------------

PROJECTED = Param('source', 'Shape set', 'enum', options_from='objects:shape_set')
ROWS = Param('rows', 'Channels (rows, blank: all)', 'string', default='')
TOLERANCE = Param('tolerance', 'Node match tolerance', 'number', default=2.0, unit='%',
                  min=0.01, max=100.0, step=0.5)
BASIS = Param('first', 'Basis', 'enum', options_from='objects:shape_set')
OTHER = Param('second', 'Projected', 'enum', options_from='objects:shape_set')


def _report_rows(report: dict, units_defined: bool) -> list[dict]:
    """What a projection matched, as Visual Dynamics' status line says it."""
    unit = 'm' if units_defined else 'model units'
    rows = [v.row('Nodes matched', f'{report["matched"]} of {report["total"]}', None,
                  'pass' if report['matched'] == report['total'] else 'warn'),
            v.row('Worst distance', float(report['worst']), unit, 'info'),
            v.row('Match limit', float(report['limit']), unit, 'info')]
    dropped = report['dropped']
    rows.append(v.row('Dropped', f'{len(dropped)}: {", ".join(map(str, dropped))}'
                      if dropped else 'none', None, 'warn' if dropped else 'pass'))
    return rows


def project_onto_channels(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.project_onto_channels`: a shape set sampled at some rows
    of a channel table, by the nearest node and its direction
    (`Project.project_onto_basis` with `channels`), as Visual Dynamics
    projects onto rows picked in the table's grid, with the node match
    tolerance its dialog asks for. The set is added, drawn on the
    table's geometry, beside what the projection matched."""
    source, table = params['source'], params['table']
    count = len(project[table].dof_strings())
    rows = _picks(params['rows'], count, 'row')
    try:
        added = project.project_onto_basis(source, onto=table, channels=rows,
                                           tolerance=params['tolerance'] / 100.0)
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    found = project.geometry_for(added)
    shapes = project[added]
    reading = v.reading(f'{added}: projection', _report_rows(
        shapes.projection_report, found is not None and found[1].units_defined))
    if found is None:
        return [reading]
    return [_shape_view(project, added, found[0]), reading]


def projection_overlay(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.projection_overlay`: two shape sets on two geometries,
    the second projected onto the basis set's DOFs
    (`core.correlate.project_shapes`), their MAC after projection
    (`core.shapes.cross_mac`, as `Project.comparison_mac` reads it), and
    the pair a cell picks drawn on the basis geometry, the projected
    mode over the basis mode: Visual Dynamics' Animate the Projection.
    Nothing is added."""
    from hertzforge.core.correlate import project_shapes
    from hertzforge.core.shapes import cross_mac

    first, second = params['first'], params['second']
    if first == second:
        raise refused('a set is projected onto another set, not itself')
    home, theirs = project.geometry_for(first), project.geometry_for(second)
    if home is None or theirs is None:
        raise refused(f'{first if home is None else second} has no geometry; link one to it')
    if home[1] is theirs[1]:
        raise refused(f'{first} and {second} are both on {home[0]}, so there is nothing to '
                      'project; MAC overlays them there')
    rows = project[first]
    try:
        projected, report = project_shapes(project[second], theirs[1], rows, home[1],
                                           tolerance=params['tolerance'] / 100.0)
        values = cross_mac(rows, projected)
    except ValueError as error:
        raise refused(str(error)) from None

    def axis(name: str, shapes: Any) -> dict:
        return {'label': name, 'ticks': [f'{f:.1f} Hz' for f in shapes.frequency]}

    matrix = v.matrix(f'MAC of {first} Against {second} on {home[0]}', axis(first, rows),
                      axis(f'{second}, projected', projected), values, 'MAC', (0.0, 1.0))
    views = paired(home[1], matrix, rows, range(rows.num_shapes), projected,
                   range(projected.num_shapes))
    return [*views, v.reading(f'{second} projected onto {first}',
                              _report_rows(report, home[1].units_defined))]


OPERATIONS = (
    Operation(
        id='project.plan_instrumentation', title='Plan Instrumentation', workspace='modal',
        summary='Choose accelerometers on a finite element model by Effective Independence, '
                'tri-axial ones first, and read how well they tell the modes apart; Keep adds '
                'the geometry, channel table and shapes. Surfaces are left out or put back by '
                'their number in the surfaces table.',
        params=(PLANNED, MODES, TRIAXIAL, UNIAXIAL, UNIAXIAL_ALONG, GROUPS, LEAVE_OUT,
                PUT_BACK, EDGE_ANGLE, EDGE_DISTANCE, PLAN_NAME, KEEP),
        outputs=('shape3d', 'reading', 'matrix', 'table'), run=plan_instrumentation),
    Operation(
        id='project.export_state_space', title='Export State-Space Model', workspace='modal',
        summary='A shape set at a channel table\'s channels as the modal state-space model '
                'Rattlesnake\'s state-space hardware loads, previewed at the drive points and '
                'written to a .npz when a file is named.',
        params=(STATE_SHAPES, TABLE, MAXIMUM, RIGID, PATH), outputs=('spectrum', 'reading'),
        run=export_state_space),
    Operation(
        id='project.project_onto_channels', title='Project onto Picked Channels',
        workspace='modal',
        summary='A shape set sampled at some rows of a channel table, numbered from 1 as its '
                'table shows them, by the nearest node and its direction; added to the project.',
        params=(PROJECTED, TABLE, ROWS, TOLERANCE), outputs=('shape3d', 'reading'),
        run=project_onto_channels),
    Operation(
        id='project.projection_overlay', title='Animate the Projection', workspace='modal',
        summary='A shape set on another geometry projected onto the basis set\'s DOFs, their '
                'MAC, and a pair drawn on the basis geometry, the projected mode over the basis '
                'mode.',
        params=(BASIS, OTHER, TOLERANCE), outputs=('matrix', 'shape3d', 'reading'),
        run=projection_overlay),
)
