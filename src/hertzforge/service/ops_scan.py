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
from hertzforge.core.scan_sensors import CHOICES, assemble, resolve
from hertzforge.core.sensor_catalog import CATALOG
from hertzforge.core.sensors import place
from hertzforge.drawing.viz.sensors import sensor_bodies

from . import views as v
from .descriptors import Operation, Option, Param
from .ops_geometry import arrows
from .protocol import refused

MEASURED = Param('geometry', 'Measured points', 'enum', options_from='verb:create_sensors')
KEEP = Param('keep', 'Create the geometry and channel table', 'boolean', default=False)
ROW = Param('row', 'Row', 'integer', default=1, min=1, step=1)
COLUMN = Param('column', 'Column', 'enum', default='model', options=tuple(
    Option(key, key.title()) for key in CHOICES))
VALUE = Param('value', 'Value', 'string', default='')

#: the bodies are drawn this much under size, so the points measured on
#: their faces show outside them
UNDER_SIZE = 0.97

#: the color of a point no surface was fitted to (sRGB)
UNFITTED = (139, 148, 158)

#: a fitted point's colors (sRGB): on its surface, at the roughness limit, at twice it.
#: Green to red and not the colormap, whose low end is too dark to see on a dark body
ON_SURFACE, AT_LIMIT, FAR = (63, 185, 80), (210, 153, 34), (248, 81, 73)


def _colors(distance: np.ndarray, limit: float) -> np.ndarray:
    """A color per point from how far it lies from its surface: green on
    it, amber at `limit`, red at twice `limit` and beyond."""
    stops = np.asarray((ON_SURFACE, AT_LIMIT, FAR), dtype=float)
    at = np.clip(np.abs(distance) / (2 * limit), 0.0, 1.0) * (len(stops) - 1)
    lower = np.minimum(at.astype(int), len(stops) - 2)
    part = (at - lower)[:, None]
    return np.rint(stops[lower] * (1 - part) + stops[lower + 1] * part).astype(np.uint8)


def _fit_row(fit: Any, scale: float) -> list[str]:
    """One sensor's row of the fits table; `scale` takes its lengths to millimeters."""
    surfaces = [*fit.planes, *([CYLINDER_LABEL] if fit.cylinder is not None else [])]
    counts = [plane.count for plane in fit.planes.values()]
    rms = [plane.rms for plane in fit.planes.values()]
    if fit.cylinder is not None:
        counts.append(fit.cylinder.count)
        rms.append(fit.cylinder.rms)
    return [fit.sensor, ', '.join(surfaces) or 'none fitted', str(sum(counts)),
            f'{max(rms) * scale:.4f}' if rms else '',
            f'{fit.moved:.2f}' if fit.frame is not None else '',
            f'{fit.extent * scale:.2f}' if fit.extent is not None else '',
            '; '.join(f'{flag.level}: {flag.text}' for flag in fit.flags)]


def _length(value: float | None) -> str:
    return '' if value is None else f'{value:.6g}'


def _sensors_table(name: str, resolved: list, fits: dict, defined: bool) -> dict:
    """The table of sensors, typed into: a node, a model from the catalog
    or any name, and a size and a length, which go out in the display
    system's length when the geometry's unit is declared."""
    unit = ' [m]' if defined else ''
    def fit_cell(one: Any) -> str:
        # placed or why not, then what the fit warns of
        warned = [flag.text for flag in fits[one.sensor].flags if flag.level == 'warn']
        return '; '.join([one.problem or 'placed', *warned])

    rows = [[one.sensor, '' if one.node is None else str(one.node), one.model, one.shape,
             _length(one.size), _length(one.length), fit_cell(one)]
            for one in resolved]
    models = [entry.model for entry in CATALOG if entry.kind in ('triax', 'uniaxial', 'force')]
    return v.table(
        f'Sensors of {name}',
        ['Sensor', 'Node', 'Model', 'Shape', f'Size{unit}', f'Length{unit}', 'Fit'], rows,
        edit=v.editing('project.set_sensor', {'geometry': name},
                       [None, 'node', 'model', None, 'size', 'length', None],
                       choices={'model': models},
                       kinds={'node': 'number', 'model': 'text', 'size': 'number',
                              'length': 'number'}))


def create_sensors(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.create_sensors`: the accelerometers a geometry's measured
    points locate (`Project.fit_sensors`), drawn over the points: each
    point colored by how far it lies from the surface fitted to it, each
    accelerometer's body where it would be placed, a little under size
    so the points on its faces show, and an arrow along each channel.
    The first table is the sensors, typed into (`project.set_sensor`): a
    node, a model, a size and a length each; the second reads each fit.
    Create adds the geometry and the channel table
    (`Project.create_sensors`); without it nothing changes."""
    name = params['geometry']
    scan = project[name]
    try:
        fits, notes = project.fit_sensors(name)
    except (TypeError, ValueError) as error:
        raise refused(str(error)) from None
    defined = scan.units_defined
    scale = 1e3 if defined else 1.0
    known = {fit.sensor for fit in fits}
    choices = {sensor: choice for sensor, choice in scan.sensor_choices.items()
               if sensor in known}
    resolved, said = resolve(fits, choices)
    placed = [one for one in resolved if one.placed]
    problem = None
    if not defined:
        problem = f'{name} has no length unit; declare it before the accelerometers are sized'
    elif not placed:
        problem = 'no accelerometer is located by its measured points'
    made = None if problem else assemble(fits, scan.length_unit, choices=choices)

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
        geometry, table, _all, _said = made
        sensors, _drawn_notes = place(table, geometry)
        bodies, _body_notes = sensor_bodies(sensors, geometry, lambda _node: None, UNDER_SIZE)
        if bodies is not None:
            view['bodies'] = bodies
        centers = np.asarray(geometry.node_xyz, dtype=float)
        drawn, labels = [], []
        for row, one in enumerate(placed):
            frame = by_sensor[one.sensor].frame
            axes = (0, 1, 2) if one.kind == 'triax' else (
                'XYZ'.index(by_sensor[one.sensor].direction[0]),)
            for axis in axes:
                drawn.append((False, (row, frame[axis], axis)))
                labels.append(f'{one.node}{"XYZ"[axis]}+')
        view['arrows'] = arrows(centers, drawn, 1.5 * max(max(one.body) for one in placed),
                                labels)

    unit = 'mm' if defined else 'file units'
    readings = [v.row('Sensors', f'{len(placed)} placed of {len(fits)} measured', None,
                      'pass' if fits and len(placed) == len(fits) else 'warn')]
    if problem:
        readings.append(v.row('Not placed', problem, None, 'warn'))
    readings += [v.row('Left out', f'sensor {one.sensor}: {one.problem}', None, 'warn')
                 for one in resolved if one.problem]
    readings += [v.row('Note', note, None, 'warn') for note in [*notes, *said]]
    if params['keep']:
        if not made:
            raise refused(problem or 'no accelerometer is located by its measured points')
        try:
            added = project.create_sensors(name)
        except (TypeError, ValueError) as error:
            raise refused(str(error)) from None
        readings.append(v.row('Created', f'{added["geometry"]} and {added["channels"]}', None,
                              'pass'))
    else:
        readings.append(v.row('Not created', 'Apply adds the geometry and the channel table',
                              None, 'info'))
    return [view, _sensors_table(name, resolved, by_sensor, defined),
            v.table(f'Fits of {name}',
                    ['Sensor', 'Surfaces', 'Points', f'Worst RMS [{unit}]', 'Squared by [°]',
                     f'Measured size [{unit}]', 'Flags'],
                    [_fit_row(fit, scale) for fit in fits]),
            v.reading(f'Sensors of {name}', readings)]


def set_sensor(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_sensor`: one cell of the sensors table typed over, by
    row (from 1) and column: a node, a model, a size or a length
    (`Project.set_sensor`). An empty value takes the choice back. A
    value its column cannot hold is refused and the cell is unchanged.
    The preview is drawn again with it."""
    name = params['geometry']
    try:
        fits, _notes = project.fit_sensors(name)
    except (TypeError, ValueError) as error:
        raise refused(str(error)) from None
    if params['row'] > len(fits):
        raise refused(f'{name} has {len(fits)} sensors; there is no row {params["row"]}')
    sensor, column, text = fits[params['row'] - 1].sensor, params['column'], params['value'].strip()
    value: Any = None
    if text and column == 'model':
        value = text
    elif text:
        try:
            value = float(text)
        except ValueError:
            raise refused(f'a {column} is a number; {text!r} is not') from None
        value = int(value) if column == 'node' and value == int(value) else value
    try:
        project.set_sensor(name, sensor, **{column: value})
    except (TypeError, ValueError) as error:
        raise refused(str(error)) from None
    return create_sensors(project, {'geometry': name, 'keep': False})


def _kept(params: dict[str, Any]) -> bool:
    return params['keep']


OPERATIONS = (
    Operation(
        id='project.create_sensors', title='Fit Sensors', workspace='geometry',
        summary='Accelerometers fitted to points measured on their faces with a measuring arm: '
                'a plane to each face, a cylinder to each round side. Preview draws each body '
                'on its points with its channels\' arrows and lists the sensors; type a node, '
                'a model, a size or a length into the table and the preview is drawn again. '
                'Apply adds the test geometry and channel table.',
        params=(MEASURED, KEEP), outputs=('shape3d', 'table', 'reading'),
        run=create_sensors, edits_when=_kept, commits='keep'),
    Operation(
        id='project.set_sensor', title='Set Sensor', workspace='geometry',
        summary='One cell of Fit Sensors\' table typed over: a sensor\'s node, model, size or '
                'length. The table edits through it.',
        params=(MEASURED, ROW, COLUMN, VALUE), outputs=('shape3d', 'table', 'reading'),
        run=set_sensor, edits=True),
)
