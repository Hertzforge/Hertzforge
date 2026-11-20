"""Fit Sensors: the accelerometers a geometry's measured points locate,
shown before the test geometry and channel table are made of them."""

from __future__ import annotations

from typing import Any

import numpy as np

from hertzforge.core.scan_fit import (
    CYLINDER_LABEL,
    CYLINDER_RMS,
    PLANE_RMS,
    SURFACES,
    measured_features,
    parse_feature,
    residuals,
    surface_of,
)
from hertzforge.core.scan_sensors import TYPED, assemble, resolve
from hertzforge.core.sensor_catalog import CATALOG
from hertzforge.core.sensors import place
from hertzforge.drawing.viz.geometry import dof_arrow_solids
from hertzforge.drawing.viz.sensors import sensor_bodies

from . import views as v
from .descriptors import Operation, Option, Param
from .protocol import refused

MEASURED = Param('geometry', 'Measured points', 'enum', options_from='verb:create_sensors')
KEEP = Param('keep', 'Create the geometry and channel table', 'boolean', default=False)
ROW = Param('row', 'Row', 'integer', default=1, min=1, step=1)
COLUMN = Param('column', 'Column', 'enum', default='model', options=tuple(
    Option(key, key.title()) for key in TYPED))
VALUE = Param('value', 'Value', 'string', default='')
SET_COLUMN = Param('column', 'Column', 'enum', default='surface', options=(
    Option('sensor', 'Sensor'), Option('surface', 'Surface')))

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


def _set_rows(scan: Any, fits: dict, scale: float) -> list[tuple[str | None, list[str]]]:
    """Every set of measured points, in the geometry's order: the sensor it
    is taken for (None when its name says none), and its row: its name,
    that sensor, its surface, its points and how far they lie from the
    surface fitted to them (RMS, in millimeters when the unit is known)."""
    xyz = np.asarray(scan.node_xyz, dtype=float)
    features = measured_features(scan)
    cylinders = {parsed[0] for name in features
                 if (parsed := parse_feature(name)) and parsed[1] == CYLINDER_LABEL}
    out = []
    for name, rows in features.items():
        parsed = parse_feature(name)
        sensor = parsed[0] if parsed else None
        distance = (residuals(fits[sensor], parsed[1], xyz[rows])
                    if parsed and sensor in fits else None)
        rms = '' if distance is None else f'{float(np.sqrt(np.mean(distance ** 2))) * scale:.4f}'
        out.append((sensor, [name, sensor or '', surface_of(name, sensor in cylinders),
                             str(len(rows)), rms]))
    return out


def _sets_table(title: str, name: str, rows: list[tuple[int, list[str]]], unit: str) -> dict:
    """A table of scan sets, typed into: the sensor each belongs to and
    which of its surfaces it is (`project.set_scan_set`). `rows` pairs
    each row with its set's place among the geometry's sets, from 1."""
    return v.table(
        title, ['Scan set', 'Sensor', 'Surface', 'Points', f'RMS ({unit})'],
        [row for _place, row in rows],
        edit={**v.editing('project.set_scan_set', {'geometry': name},
                          [None, 'sensor', 'surface', None, None],
                          choices={'surface': list(SURFACES.values())},
                          kinds={'sensor': 'text', 'surface': 'choice'}),
              'rows': [place for place, _row in rows]})


def _sensors_table(name: str, resolved: list, fits: dict, defined: bool,
                   sets: list[tuple[str | None, list[str]]]) -> dict:
    """The table of sensors, typed into: a node, a model from the catalog
    or any name, and a length (a cube's side or a round body's length)
    and a diameter (a round body's), which go out in the display
    system's length when the geometry's unit is declared."""
    unit = ' [m]' if defined else ''
    def fit_cell(one: Any) -> str:
        # placed or why not, then what the fit warns of
        warned = [flag.text for flag in fits[one.sensor].flags if flag.level == 'warn']
        return '; '.join([one.problem or 'placed', *warned])

    def sizes(one: Any) -> list[str]:
        # a cube's size is its length; a round body's size is its diameter
        if one.kind == 'triax':
            return [_length(one.size), '']
        return [_length(one.length), _length(one.size)]

    rows = [[one.sensor, '' if one.node is None else str(one.node), one.model, one.shape,
             *sizes(one), fit_cell(one)]
            for one in resolved]
    models = [entry.model for entry in CATALOG if entry.kind in ('triax', 'uniaxial', 'force')]
    # under each sensor, the sets of points it was fitted to; a sensor that is not placed
    # opens on them, since one of them is most likely what is wrong
    nested = [_sets_table(f'Scan sets of sensor {one.sensor}', name,
                          [(place, row) for place, (sensor, row) in enumerate(sets, start=1)
                           if sensor == one.sensor], 'mm' if defined else 'file units')
              for one in resolved]
    table = v.table(
        f'Sensors of {name}',
        ['Sensor', 'Node', 'Model', 'Shape', f'Length{unit}', f'Diameter{unit}', 'Fit'], rows,
        edit=v.editing('project.set_sensor', {'geometry': name},
                       [None, 'node', 'model', None, 'length', 'diameter', None],
                       choices={'model': models},
                       kinds={'node': 'number', 'model': 'text', 'length': 'number',
                              'diameter': 'number'}))
    table['nested'] = nested
    table['open'] = [row for row, one in enumerate(resolved, start=1) if not one.placed]
    return table


def create_sensors(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.create_sensors`: the accelerometers a geometry's measured
    points locate (`Project.fit_sensors`), drawn over the points: each
    point colored by how far it lies from the surface fitted to it, each
    accelerometer's body where it would be placed, a little under size
    so the points on its faces show, and an arrow along each channel.
    The first table is the sensors, typed into (`project.set_sensor`): a
    node, a model, a length and a diameter each; the second reads each
    fit. Create adds the geometry and the channel table
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
            for axis in ('XYZ'.index(direction[0]) for direction in one.directions):
                drawn.append((False, (row, frame[axis], axis)))
                labels.append(f'{one.node}{"XYZ"[axis]}+')
        view['arrows'] = dof_arrow_solids(centers, drawn, 1.5 * max(max(one.body) for one in placed),
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
    sets = _set_rows(scan, by_sensor, scale)
    stray = [(place, row) for place, (sensor, row) in enumerate(sets, start=1)
             if sensor not in by_sensor]
    return [view, _sensors_table(name, resolved, by_sensor, defined, sets),
            *([_sets_table(f'Scan sets of {name} with no sensor', name, stray, unit)]
              if stray else []),
            v.table(f'Fits of {name}',
                    ['Sensor', 'Surfaces', 'Points', f'Worst RMS [{unit}]', 'Squared by [°]',
                     f'Measured size [{unit}]', 'Flags'],
                    [_fit_row(fit, scale) for fit in fits]),
            v.reading(f'Sensors of {name}', readings)]


def set_sensor(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_sensor`: one cell of the sensors table typed over, by
    row (from 1) and column: a node, a model, a length or a diameter
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


def set_scan_set(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_scan_set`: one cell of a table of scan sets typed over,
    by the set's place among the geometry's sets (from 1) and column: the
    sensor it belongs to, or which of its surfaces it is
    (`Project.set_scan_set`). The set is renamed, and exchanges with the
    set that held the place. The preview is drawn again with it."""
    name = params['geometry']
    try:
        project.fit_sensors(name)
    except (TypeError, ValueError) as error:
        raise refused(str(error)) from None
    sets = list(measured_features(project[name]))
    if params['row'] > len(sets):
        raise refused(f'{name} has {len(sets)} scan sets; there is no row {params["row"]}')
    text = params['value'].strip()
    if not text:
        raise refused(f'a scan set needs a {params["column"]}')
    try:
        project.set_scan_set(name, sets[params['row'] - 1], **{params['column']: text})
    except (TypeError, ValueError) as error:
        raise refused(str(error)) from None
    return create_sensors(project, {'geometry': name, 'keep': False})


def _kept(params: dict[str, Any]) -> bool:
    return params['keep']


OPERATIONS = (
    Operation(
        id='project.create_sensors', title='Fit Sensors', workspace='geometry',
        summary='Accelerometers fitted to points measured on their faces with a measuring arm: '
                'a plane to each face, a cylinder to each round side. It opens on its preview, '
                'each body drawn on its points with its channels\' arrows and the sensors '
                'listed; type a node, a model, a length or a diameter into the table and the '
                'preview is drawn again. Apply adds the test geometry and channel table.',
        params=(MEASURED, KEEP), outputs=('shape3d', 'table', 'reading'),
        run=create_sensors, edits_when=_kept, commits='keep', opens_previewed=True),
    Operation(
        id='project.set_sensor', title='Set Sensor', workspace='geometry',
        summary='One cell of Fit Sensors\' table typed over: a sensor\'s node, model, length '
                'or diameter. The table edits through it.',
        params=(MEASURED, ROW, COLUMN, VALUE), outputs=('shape3d', 'table', 'reading'),
        run=set_sensor, edits=True),
    Operation(
        id='project.set_scan_set', title='Set Scan Set', workspace='geometry',
        summary='One cell of a scan set under a sensor in Fit Sensors\' table typed over: the '
                'sensor the set belongs to, or which surface it is. The set is renamed, and '
                'exchanges with the set that held the place. The table edits through it.',
        params=(MEASURED, ROW, SET_COLUMN, VALUE), outputs=('shape3d', 'table', 'reading'),
        run=set_scan_set, edits=True),
)
