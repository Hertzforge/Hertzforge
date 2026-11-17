"""Fit Sensors: the accelerometers a geometry's measured points locate,
shown before the test geometry and channel table are made of them."""

from __future__ import annotations

from typing import Any

import numpy as np

from hertzforge.core.scan_fit import (
    CYLINDER_LABEL,
    CYLINDER_RMS,
    PLANE_RMS,
    measured_features,
    parse_feature,
    residuals,
)
from hertzforge.core.scan_sensors import assemble
from hertzforge.core.sensors import place
from hertzforge.drawing.viz.sensors import sensor_bodies

from . import views as v
from .descriptors import Operation, Param
from .ops_geometry import arrows
from .protocol import refused

MEASURED = Param('geometry', 'Measured points', 'enum', options_from='verb:create_sensors')
MODELS = Param('models', 'Models (34=356A02, 12=Cylinder 16 x 9 mm; blank: guessed)', 'string',
               default='')
NODES = Param('nodes', 'Nodes (34=134; blank: the sensor\'s number)', 'string', default='')
NAME = Param('name', 'Name (blank: the geometry\'s, with Sensors)', 'string', default='')
KEEP = Param('keep', 'Create the geometry and channel table', 'boolean', default=False)

#: the bodies are drawn this much under size, so the points measured on
#: their faces show outside them
UNDER_SIZE = 0.97

#: the color of a point no surface was fitted to (sRGB)
UNFITTED = (139, 148, 158)

#: a fitted point's colors (sRGB): on its surface, at the roughness limit, at twice it.
#: Green to red and not the colormap, whose low end is too dark to see on a dark body
ON_SURFACE, AT_LIMIT, FAR = (63, 185, 80), (210, 153, 34), (248, 81, 73)


def _pairs(text: str, what: str) -> dict[str, str]:
    """'34=356A02, 12=Cylinder 16 x 9 mm' as {sensor: value}."""
    out: dict[str, str] = {}
    for part in text.replace(';', ',').split(','):
        if not part.strip():
            continue
        sensor, mark, value = part.partition('=')
        if not mark or not sensor.strip() or not value.strip():
            raise refused(f'{what} are written sensor=value, parted by commas; '
                          f'{part.strip()!r} is not')
        out[sensor.strip()] = value.strip()
    return out


def _colors(distance: np.ndarray, limit: float) -> np.ndarray:
    """A color per point from how far it lies from its surface: green on
    it, amber at `limit`, red at twice `limit` and beyond."""
    stops = np.asarray((ON_SURFACE, AT_LIMIT, FAR), dtype=float)
    at = np.clip(np.abs(distance) / (2 * limit), 0.0, 1.0) * (len(stops) - 1)
    lower = np.minimum(at.astype(int), len(stops) - 2)
    part = (at - lower)[:, None]
    return np.rint(stops[lower] * (1 - part) + stops[lower + 1] * part).astype(np.uint8)


def _fit_row(fit: Any, placed: Any, scale: float) -> list[str]:
    """One sensor's row of the fits table; `scale` takes its lengths to millimeters."""
    surfaces = [*fit.planes, *([CYLINDER_LABEL] if fit.cylinder is not None else [])]
    counts = [plane.count for plane in fit.planes.values()]
    rms = [plane.rms for plane in fit.planes.values()]
    if fit.cylinder is not None:
        counts.append(fit.cylinder.count)
        rms.append(fit.cylinder.rms)
    return [fit.sensor, 'tri-axial' if fit.kind == 'triax' else 'uni-axial',
            ', '.join(surfaces) or 'none fitted', str(sum(counts)),
            f'{max(rms) * scale:.4f}' if rms else '',
            f'{fit.moved:.2f}' if fit.frame is not None else '',
            f'{fit.extent * scale:.2f}' if fit.extent is not None else '',
            placed.model if placed else '', str(placed.node) if placed else '',
            ' '.join(str(c) for c in placed.channels) if placed else '',
            '; '.join(f'{flag.level}: {flag.text}' for flag in fit.flags)]


def create_sensors(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.create_sensors`: the accelerometers a geometry's measured
    points locate (`Project.fit_sensors`), drawn over the points: each
    point colored by how far it lies from the surface fitted to it, each
    accelerometer's body where it would be placed, a little under size
    so the points on its faces show, and an arrow along each channel.
    The table reads each fit. Create adds the geometry and the channel
    table (`Project.create_sensors`); without it nothing changes."""
    name = params['geometry']
    scan = project[name]
    try:
        fits, notes = project.fit_sensors(name)
    except (TypeError, ValueError) as error:
        raise refused(str(error)) from None
    models, nodes = _pairs(params['models'], 'models'), _pairs(params['nodes'], 'nodes')
    try:
        nodes_given = {sensor: int(value) for sensor, value in nodes.items()}
    except ValueError:
        raise refused('a node is a whole number') from None
    defined = scan.units_defined
    scale = 1e3 if defined else 1.0
    made = None
    problem = None if defined else (f'{name} has no length unit; declare it before the '
                                    'accelerometers are sized')
    if defined:
        try:
            made = assemble(fits, scan.length_unit, models=models, nodes=nodes_given)
        except ValueError as error:
            problem = str(error)
    placed = {one.sensor: one for one in made[2]} if made else {}

    # the points, each colored by its distance from its own surface
    xyz = np.asarray(scan.node_xyz, dtype=float)
    colors = np.tile(np.uint8(UNFITTED), (len(xyz), 1))
    by_sensor = {fit.sensor: fit for fit in fits}
    for feature, rows in measured_features(scan).items():
        parsed = parse_feature(feature)
        if parsed is None or parsed[0] not in by_sensor:
            continue
        distance = residuals(by_sensor[parsed[0]], parsed[1], xyz[rows])
        if distance is not None:
            limit = CYLINDER_RMS if parsed[1] == CYLINDER_LABEL else PLANE_RMS
            # with no unit the distance cannot be judged: every fitted point at the low end
            colors[rows] = _colors(distance if defined else np.zeros_like(distance), limit)
    view = v.shape3d(f'Sensors fitted to {name}', 'm' if defined else 'undefined', xyz, [],
                     node_ids=np.asarray(scan.node_id))
    view['points'] = [{'xyz': np.ascontiguousarray(xyz, np.float32), 'colors': colors,
                       'size': 5.0}]
    if made:
        geometry, table, _placed, said = made
        notes = [*notes, *said]
        sensors, _drawn_notes = place(table, geometry)
        bodies, _body_notes = sensor_bodies(sensors, geometry, lambda _node: None, UNDER_SIZE)
        if bodies is not None:
            view['bodies'] = bodies
        centers = np.asarray(geometry.node_xyz, dtype=float)
        drawn, labels = [], []
        for row, one in enumerate(made[2]):
            frame = by_sensor[one.sensor].frame
            axes = (0, 1, 2) if one.kind == 'triax' else (
                'XYZ'.index(by_sensor[one.sensor].direction[0]),)
            for axis in axes:
                drawn.append((False, (row, frame[axis], axis)))
                labels.append(f'{one.node}{"XYZ"[axis]}+')
        length = 1.5e-3 * max(max(one.size) for one in made[2])
        view['arrows'] = arrows(centers, drawn, length, labels)

    rows = [_fit_row(fit, placed.get(fit.sensor), scale) for fit in fits]
    unit = 'mm' if defined else 'file units'
    readings = [v.row('Sensors', f'{len(placed)} placed of {len(fits)} measured', None,
                      'pass' if fits and len(placed) == len(fits) else 'warn')]
    if problem:
        readings.append(v.row('Not placed', problem, None, 'warn'))
    readings += [v.row('Note', note, None, 'warn') for note in notes]
    if params['keep']:
        if not made:
            raise refused(problem or 'no accelerometer is located by its measured points')
        try:
            added = project.create_sensors(name, models or None, None, nodes_given or None,
                                           params['name'].strip() or None)
        except (TypeError, ValueError) as error:
            raise refused(str(error)) from None
        readings.append(v.row('Created', f'{added["geometry"]} and {added["channels"]}', None,
                              'pass'))
    else:
        readings.append(v.row('Not created', 'Apply adds the geometry and the channel table',
                              None, 'info'))
    return [view,
            v.table(f'Fits of {name}',
                    ['Sensor', 'Kind', 'Surfaces', 'Points', f'Worst RMS [{unit}]',
                     'Squared by [°]', f'Measured size [{unit}]', 'Model', 'Node', 'Channels',
                     'Flags'], rows),
            v.reading(f'Sensors of {name}', readings)]


def _kept(params: dict[str, Any]) -> bool:
    return params['keep']


OPERATIONS = (
    Operation(
        id='project.create_sensors', title='Fit Sensors', workspace='geometry',
        summary='Accelerometers fitted to points measured on their faces with a measuring arm: '
                'a plane to each face, a cylinder to each round side. Preview draws each body '
                'on its points with its channels\' arrows and reads the fits; Apply adds the '
                'test geometry and channel table. A model or a size (Block 17.5 mm, Cylinder '
                '16 x 9 mm) may be named for a sensor.',
        params=(MEASURED, MODELS, NODES, NAME, KEEP), outputs=('shape3d', 'table', 'reading'),
        run=create_sensors, edits_when=_kept, commits='keep'),
)
