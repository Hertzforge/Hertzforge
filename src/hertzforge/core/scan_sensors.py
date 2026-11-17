"""A geometry and a channel table from accelerometers fitted to measured
points (`core.scan_fit`).

Each accelerometer becomes one node at the center of its body, measured
in a coordinate system of its own whose axes are the accelerometer's.
A tri-axial accelerometer gives three channels, X+, Y+ and Z+, whatever
faces were measured; a uni-axial one gives one, on the axis its top was
labelled with.

The center is half the body's size back from each measured face, so it
needs the size. That comes from the model: one named for the sensor, or
the smallest tri-axial model in the catalog the measured points fit in.
A sensor with neither takes a size given for it, or the measured one.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any

import numpy as np

from .channel_table import ChannelTable
from .geometry import Geometry
from .scan_fit import SensorFit
from .sensor_catalog import CATALOG, SensorModel, lookup

#: a block whose points reach this far past a model's side still takes
#: that model, in millimeters: the reach is measured from a fitted corner
MODEL_SLACK = 0.2


@dataclass(frozen=True)
class Placed:
    """One accelerometer as it was placed.

    Attributes:
        sensor: The sensor's name in the features.
        node: Its node in the geometry.
        kind: 'triax' or 'uniaxial'.
        model: The catalog model it was sized by, '' for none.
        size: The body's size along its X, Y and Z, in millimeters.
        sized_by: Where the size came from: 'model' (named for it),
            'guess' (the catalog model the points fit in), 'given' or
            'measured'.
        channels: Its channel numbers in the table.
    """

    sensor: str
    node: int
    kind: str
    model: str
    size: tuple[float, float, float]
    sized_by: str
    channels: tuple[int, ...]


def guess_model(fit: SensorFit, meters: float) -> SensorModel | None:
    """The smallest tri-axial model in the catalog a fitted block fits in.

    Parameters
    ----------
    fit : SensorFit
        A tri-axial accelerometer's fit.
    meters : float
        How many meters one unit of the fit's lengths is.

    Returns
    -------
    SensorModel or None
        The first model, in catalog order, of the smallest body whose
        every side is at least the furthest the measured points reach
        behind a face (less `MODEL_SLACK`). Models of one size cannot be
        told apart by their faces. None for a uni-axial accelerometer,
        and when the points reach further than the largest model.
    """
    if fit.kind != 'triax' or fit.extent is None:
        return None
    reach = fit.extent * meters * 1e3 - MODEL_SLACK
    fitting = [entry for entry in CATALOG if entry.kind == 'triax' and min(entry.size) >= reach]
    return min(fitting, key=lambda entry: max(entry.size), default=None)


def _size(fit: SensorFit, meters: float, model: str | None,
          given: Any) -> tuple[str, tuple[float, float, float], str, list[str]]:
    """(model, size in mm, where it came from, notes) for one sensor."""
    notes: list[str] = []
    if given is not None:
        size = tuple(float(v) for v in np.broadcast_to(np.asarray(given, dtype=float), (3,)))
        return (model or ''), size, 'given', notes                   # type: ignore[return-value]
    if model:
        entry = lookup(model)
        if entry is None:
            raise ValueError(f'sensor {fit.sensor}: {model!r} is not in the sensor catalog; '
                             'give its size')
        if entry.kind != fit.kind:
            raise ValueError(f'sensor {fit.sensor} is {fit.kind} and {entry.model} is '
                             f'{entry.kind}')
        return entry.model, entry.size, 'model', notes
    guessed = guess_model(fit, meters)
    if guessed is not None:
        return guessed.model, guessed.size, 'guess', notes
    assert fit.extent is not None
    measured = fit.extent * meters * 1e3
    if fit.kind == 'triax':
        notes.append(f'sensor {fit.sensor}: no catalog model fits a block of {measured:.1f} mm '
                     'or more, so its side is taken as that, the least it can be')
    else:
        notes.append(f'sensor {fit.sensor}: its height is not measured and is taken as its '
                     f'diameter, {measured:.1f} mm')
    return '', (measured, measured, measured), 'measured', notes


def _node_ids(fits: Sequence[SensorFit], nodes: Mapping[str, int]) -> dict[str, int]:
    """Each sensor's node: the one given, else its name when that is a
    whole number, else the next number free."""
    ids: dict[str, int] = {}
    for fit in fits:
        if fit.sensor in nodes:
            ids[fit.sensor] = int(nodes[fit.sensor])
        elif fit.sensor.isdigit() and int(fit.sensor) > 0:
            ids[fit.sensor] = int(fit.sensor)
    free = max(ids.values(), default=0) + 1
    for fit in fits:
        if fit.sensor not in ids:
            ids[fit.sensor] = free
            free += 1
    taken: dict[int, str] = {}
    for sensor, node in ids.items():
        if node in taken:
            raise ValueError(f'sensors {taken[node]} and {sensor} would both be node {node}')
        taken[node] = sensor
    return ids


def assemble(fits: Sequence[SensorFit], length_unit: str, *, meters: float = 1.0,
             models: Mapping[Any, str] | None = None,
             sizes: Mapping[Any, Any] | None = None,
             nodes: Mapping[Any, int] | None = None
             ) -> tuple[Geometry, ChannelTable, list[Placed], list[str]]:
    """The geometry and channel table of fitted accelerometers.

    Parameters
    ----------
    fits : sequence of SensorFit
        The fits; those that do not locate their accelerometer are left
        out and named in the notes.
    length_unit : str
        The length unit of the geometry made, the one the points were
        measured in.
    meters : float, default 1.0
        How many meters one unit of the fits' lengths is.
    models : mapping, optional
        A catalog model for a sensor, by the sensor's name.
    sizes : mapping, optional
        A body size for a sensor in millimeters, one number for a cube
        or three for X, Y and Z, by the sensor's name. It is used in
        place of the model's. For a uni-axial accelerometer the one
        number, or the third, is its height.
    nodes : mapping, optional
        A node number for a sensor, by the sensor's name. A sensor named
        with a whole number is that node otherwise.

    Returns
    -------
    geometry : Geometry
        One node per accelerometer at the center of its body, placed in
        the global system and measured in a cartesian system of its own
        at the node, named ``Sensor <name>``, whose rows are the
        accelerometer's X+, Y+ and Z+.
    channels : ChannelTable
        Acceleration channels in g, numbered from 1 in the sensors'
        order: X+, Y+, Z+ for a tri-axial accelerometer and the one
        labelled axis for a uni-axial one, with the make and model.
    placed : list of Placed
        What each accelerometer was sized by and where it went.
    notes : list of str
        The sensors left out and the sizes that were assumed.

    Raises
    ------
    ValueError
        When no fit locates an accelerometer, a model is not in the
        catalog or is the other kind, a name in `models`, `sizes` or
        `nodes` is no sensor's, or two sensors would share a node.
    """
    by_name = {
        what: {str(key): value for key, value in (given or {}).items()}
        for what, given in (('models', models), ('sizes', sizes), ('nodes', nodes))}
    known = {fit.sensor for fit in fits}
    for what, given in by_name.items():
        for stray in sorted(set(given) - known):
            raise ValueError(f'{what} names sensor {stray}, which the measured points do not '
                             'hold')
    notes = [f'sensor {fit.sensor} is left out: {flag.text}'
             for fit in fits if not fit.placed
             for flag in fit.flags if flag.level == 'error']
    kept = [fit for fit in fits if fit.placed]
    if not kept:
        raise ValueError('no accelerometer is located by its measured points')
    ids = _node_ids(kept, by_name['nodes'])
    centers, frames, placed, rows = [], [], [], []
    for fit in kept:
        model, size, sized_by, said = _size(fit, meters, by_name['models'].get(fit.sensor),
                                            by_name['sizes'].get(fit.sensor))
        notes += said
        # the catalog holds a uni-axial body with z its sensing axis
        # whichever axis that is here, so its height is its third size
        body = size if fit.kind == 'triax' else (size[2], size[2], size[2])
        centers.append(fit.center(np.asarray(body) / (meters * 1e3)))
        frames.append(fit.frame)
        directions = ('X+', 'Y+', 'Z+') if fit.kind == 'triax' else (fit.direction,)
        first = len(rows) + 1
        entry = lookup(model) if model else None
        for direction in directions:
            rows.append({'channel': len(rows) + 1, 'node': ids[fit.sensor],
                         'direction': direction, 'role': 'response',
                         'channel_type': 'acceleration', 'unit': 'g',
                         'make': entry.make if entry else '', 'model': model,
                         'triax_dof': direction[0] if fit.kind == 'triax' else ''})
        placed.append(Placed(fit.sensor, ids[fit.sensor], fit.kind, model, size, sized_by,
                             tuple(range(first, len(rows) + 1))))
    count = len(kept)
    cs_matrix = np.zeros((count + 1, 4, 3))
    cs_matrix[0, :3] = np.eye(3)
    cs_matrix[1:, :3] = np.array(frames)
    cs_matrix[1:, 3] = np.array(centers) * meters
    geometry = Geometry(
        node_id=[placed_one.node for placed_one in placed],
        node_xyz=np.array(centers) * meters,
        node_def_cs=np.ones(count, dtype=np.int64),
        node_disp_cs=np.arange(2, count + 2),
        cs_id=np.arange(1, count + 2),
        cs_name=['', *(f'Sensor {fit.sensor}' for fit in kept)],
        cs_type=np.zeros(count + 1, dtype=np.int64),
        cs_matrix=cs_matrix, length_unit=length_unit)
    table = ChannelTable({column: [row[column] for row in rows] for column in rows[0]})
    return geometry, table, placed, notes
