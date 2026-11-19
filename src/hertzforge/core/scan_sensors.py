"""A geometry and a channel table from accelerometers fitted to measured
points (`core.scan_fit`).

Each accelerometer becomes one node at the center of its body, measured
in a coordinate system of its own whose axes are the accelerometer's.
A tri-axial accelerometer gives three channels, X+, Y+ and Z+, whatever
faces were measured; a uni-axial one gives one, on the axis its top was
labelled with. A uni-axial accelerometer that is a block (a catalog
model whose body is a box) is measured by its flat faces as a tri-axial
one is, with X along its length toward its cable and Z its sensing
axis, and gives the one channel Z+.

The center is half the body's size back from each measured face, so it
needs the size. What is chosen for a sensor is its node, its model, and
its size: a cube's side, or a round body's diameter and length. A size
typed is used. Otherwise the size is the model's, when the model is in
the catalog or is itself a size (`Block 17.5 mm`); otherwise that of the
smallest tri-axial catalog model the measured points fit in; otherwise
what was measured, a block's least side or a cylinder's diameter for
both its diameter and its length. A sensor with no model named takes
its size written where its model would be, which is how the 3-D view
draws it at that size.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any

import numpy as np

from .channel_table import ChannelTable
from .geometry import Geometry
from .scan_fit import SensorFit
from .sensor_catalog import CATALOG, SensorModel, lookup, size_text, sized

#: a block whose points reach this far past a model's side still takes
#: that model, in millimeters: the reach is measured from a fitted corner
MODEL_SLACK = 0.2

#: what a body's shape is called in the table of sensors
SHAPES = {'box': 'Cube', 'puck': 'Cylinder', 'hex': 'Hex', 'teardrop': 'Teardrop',
          'rounded': 'Rounded'}

#: what may be chosen for a sensor
CHOICES = ('node', 'model', 'size', 'length')


@dataclass(frozen=True)
class Placed:
    """One accelerometer as it would be placed, or why it is not.

    Attributes:
        sensor: The sensor's name in the features.
        kind: 'triax' or 'uniaxial', as it was measured.
        node: Its node in the geometry; None when it has none.
        model: The model named or guessed for it, '' for none.
        table_model: What its rows name as the model: `model`, or its
            size written as one.
        shape: 'Cube', 'Cylinder', or a catalog body's shape.
        size: A cube's side or a round body's diameter, in the fits'
            unit; None when nothing was measured.
        length: A round body's length along its sensing axis; None for a
            cube.
        body: The body's size along its X, Y and Z, in the fits' unit.
        sized_by: Where the size came from: 'given' (typed), 'model',
            'guess' (the catalog model the points fit in) or 'measured';
            '' when it has no size.
        channel_type: 'acceleration', or 'force' for a force transducer.
        problem: Why it is not placed; '' when it is.
        channels: Its channel numbers in the table, once assembled.
        directions: The channels it gives: X+, Y+ and Z+ for a tri-axial
            accelerometer, one for a uni-axial one.
    """

    sensor: str
    kind: str
    node: int | None
    model: str
    table_model: str
    shape: str
    size: float | None
    length: float | None
    body: tuple[float, float, float] | None
    sized_by: str
    channel_type: str
    problem: str
    channels: tuple[int, ...] = ()
    directions: tuple[str, ...] = ()

    @property
    def placed(self) -> bool:
        """Whether the sensor becomes a node and channels."""
        return not self.problem


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


def _resolve_one(fit: SensorFit, choice: Mapping[str, Any], meters: float
                 ) -> tuple[dict[str, Any], list[str]]:
    """What one sensor's row holds before its node is settled, and notes."""
    notes: list[str] = []
    to_units = 1.0 / (meters * 1e3)                 # millimeters to the fits' unit
    model = str(choice.get('model') or '').strip()
    typed_size, typed_length = choice.get('size'), choice.get('length')
    measured = fit.extent
    problem = next((flag.text for flag in fit.flags if flag.level == 'error'), '')
    kinds = ('triax',) if fit.kind == 'triax' else ('uniaxial', 'force')
    entry: SensorModel | None = None
    if model:
        entry = lookup(model)
        if entry is None:
            entry = next((found for kind in kinds if (found := sized(model, kind))), None)
        elif entry.kind not in kinds and not (
                fit.kind == 'triax' and entry.kind == 'uniaxial' and entry.shape == 'box'):
            # a uni-axial block is measured by its faces, as a cube is
            problem = problem or (f'{entry.model} is a {entry.kind} model, and this sensor was '
                                  f'measured as a {"cube" if fit.kind == "triax" else "cylinder"}')
            entry = None
    sized_by = 'given' if (typed_size is not None or typed_length is not None) else 'model'
    if fit.kind == 'triax':
        guessed = None if (model or typed_size is not None) else guess_model(fit, meters)
        if typed_size is not None:
            body = (float(typed_size),) * 3
        elif entry is not None:
            body = tuple(v * to_units for v in entry.size)
        elif guessed is not None:
            body, model, entry, sized_by = (tuple(v * to_units for v in guessed.size),
                                            guessed.model, guessed, 'guess')
        elif measured is not None:
            body, sized_by = (measured,) * 3, 'measured'
            notes.append(f'sensor {fit.sensor}: its side is taken as the least it can be, '
                         f'{measured * meters * 1e3:.1f} mm, the furthest its points reach')
        else:
            body = None
        cube = body is not None and body[0] == body[1] == body[2]
        shape = 'Cube' if cube or body is None else 'Block'
        size, length = (None if body is None else body[0]), None
        written = size_text('block', [v * meters * 1e3 for v in body]) if body else ''
    else:
        if entry is not None:
            body = tuple(v * to_units for v in entry.size)
            shape = SHAPES.get(entry.shape, entry.shape.title())
        elif measured is not None:
            body, shape = (measured, measured, measured), 'Cylinder'
            if typed_length is None:
                sized_by = 'measured'
                notes.append(f'sensor {fit.sensor}: its length is not measured and is taken '
                             f'as its diameter, {measured * meters * 1e3:.1f} mm')
        else:
            body, shape = None, 'Cylinder'
        if body is not None and (typed_size is not None or typed_length is not None):
            across = float(typed_size) if typed_size is not None else body[0]
            along = float(typed_length) if typed_length is not None else body[2]
            body = (across, across, along)
        size, length = (None, None) if body is None else (body[0], body[2])
        written = size_text('cylinder', [v * meters * 1e3 for v in body]) if body else ''
    if body is None:
        # nothing measured to size it by
        sized_by = ''
        problem = problem or 'no surface was fitted'
    force = entry is not None and entry.kind == 'force'
    if fit.kind != 'triax':
        directions = (fit.direction,) if fit.direction else ()
    elif entry is not None and entry.kind == 'uniaxial':
        directions = ('Z+',)
    else:
        directions = ('X+', 'Y+', 'Z+')
    return ({'sensor': fit.sensor, 'kind': fit.kind, 'model': model,
             'table_model': model or written, 'shape': shape, 'size': size, 'length': length,
             'body': body, 'sized_by': sized_by, 'directions': directions,
             'channel_type': 'force' if force else 'acceleration', 'problem': problem},
            notes)


def resolve(fits: Sequence[SensorFit], choices: Mapping[Any, Mapping[str, Any]] | None = None,
            meters: float = 1.0) -> tuple[list[Placed], list[str]]:
    """Each measured sensor as it would be placed: its node, model, shape
    and size, from what was chosen for it and what its fit gives.

    Parameters
    ----------
    fits : sequence of SensorFit
        The fits.
    choices : mapping, optional
        By the sensor's name, a mapping with any of `CHOICES`: 'node' (a
        whole number), 'model' (a catalog model, a size written as one,
        or any name), 'size' (a cube's side or a round body's diameter)
        and 'length' (a round body's length), the last two in the fits'
        unit.
    meters : float, default 1.0
        How many meters one unit of the fits' lengths is.

    Returns
    -------
    placed : list of Placed
        One per fit, in order. One with a `problem` is not placed: its
        fit has an error, its model is the other kind, or its node is
        another sensor's.
    notes : list of str
        The sizes that were assumed.

    Raises
    ------
    ValueError
        When `choices` names a sensor the fits do not hold.
    """
    given = {str(sensor): dict(choice) for sensor, choice in (choices or {}).items()}
    known = {fit.sensor for fit in fits}
    for stray in sorted(set(given) - known):
        raise ValueError(f'sensor {stray} is not among the measured points')
    rows, notes = [], []
    for fit in fits:
        row, said = _resolve_one(fit, given.get(fit.sensor, {}), meters)
        rows.append(row)
        notes += said if not row['problem'] else []
    # the nodes: the one chosen, else the sensor's name when that is a
    # whole number, else the next number free
    taken: dict[int, str] = {}
    for row in rows:
        chosen = given.get(row['sensor'], {}).get('node')
        row['node'] = (int(chosen) if chosen is not None
                       else int(row['sensor']) if row['sensor'].isdigit() and int(row['sensor']) > 0
                       else None)
    free = max((row['node'] for row in rows if row['node'] is not None), default=0) + 1
    for row in rows:
        if row['node'] is None:
            row['node'], free = free, free + 1
        if row['problem']:
            continue
        if row['node'] in taken:
            row['problem'] = f'node {row["node"]} is already sensor {taken[row["node"]]}\'s'
        else:
            taken[row['node']] = row['sensor']
    return [Placed(**row) for row in rows], notes


def assemble(fits: Sequence[SensorFit], length_unit: str, *, meters: float = 1.0,
             choices: Mapping[Any, Mapping[str, Any]] | None = None
             ) -> tuple[Geometry, ChannelTable, list[Placed], list[str]]:
    """The geometry and channel table of fitted accelerometers.

    Parameters
    ----------
    fits : sequence of SensorFit
        The fits; those that cannot be placed are left out and named in
        the notes.
    length_unit : str
        The length unit of the geometry made, the one the points were
        measured in.
    meters : float, default 1.0
        How many meters one unit of the fits' lengths is.
    choices : mapping, optional
        What was chosen for each sensor (`resolve`).

    Returns
    -------
    geometry : Geometry
        One node per accelerometer at the center of its body, placed in
        the global system and measured in a cartesian system of its own
        at the node, named ``Sensor <name>``, whose rows are the
        accelerometer's X+, Y+ and Z+.
    channels : ChannelTable
        Channels numbered from 1 in the sensors' order: X+, Y+, Z+ for a
        tri-axial accelerometer and the one labelled axis for a
        uni-axial one, acceleration in g, or force in N for a force
        transducer, with the make and model.
    placed : list of Placed
        Every sensor measured, placed or not, with its channels.
    notes : list of str
        The sensors left out and the sizes that were assumed.

    Raises
    ------
    ValueError
        When nothing can be placed, or `choices` names a sensor the fits
        do not hold.
    """
    resolved, notes = resolve(fits, choices, meters)
    notes = [*(f'sensor {one.sensor} is left out: {one.problem}'
               for one in resolved if one.problem), *notes]
    by_sensor = {fit.sensor: fit for fit in fits}
    kept = [one for one in resolved if one.placed]
    if not kept:
        raise ValueError('no accelerometer is located by its measured points')
    centers, frames, rows, channels = [], [], [], {}
    for one in kept:
        fit = by_sensor[one.sensor]
        assert one.body is not None
        # a round body's length is its size along its sensing axis,
        # whichever body axis the top was labelled with
        body = one.body if one.kind == 'triax' else (one.body[2],) * 3
        centers.append(fit.center(body))
        frames.append(fit.frame)
        triax = len(one.directions) == 3
        first = len(rows) + 1
        entry = lookup(one.model) if one.model else None
        for direction in one.directions:
            rows.append({'channel': len(rows) + 1, 'node': one.node,
                         'direction': direction, 'role': 'response',
                         'channel_type': one.channel_type,
                         'unit': 'N' if one.channel_type == 'force' else 'g',
                         'make': entry.make if entry else '', 'model': one.table_model,
                         'triax_dof': direction[0] if triax else ''})
        channels[one.sensor] = tuple(range(first, len(rows) + 1))
    count = len(kept)
    cs_matrix = np.zeros((count + 1, 4, 3))
    cs_matrix[0, :3] = np.eye(3)
    cs_matrix[1:, :3] = np.array(frames)
    cs_matrix[1:, 3] = np.array(centers) * meters
    geometry = Geometry(
        node_id=[one.node for one in kept],
        node_xyz=np.array(centers) * meters,
        node_def_cs=np.ones(count, dtype=np.int64),
        node_disp_cs=np.arange(2, count + 2),
        cs_id=np.arange(1, count + 2),
        cs_name=['', *(f'Sensor {one.sensor}' for one in kept)],
        cs_type=np.zeros(count + 1, dtype=np.int64),
        cs_matrix=cs_matrix, length_unit=length_unit)
    table = ChannelTable({column: [row[column] for row in rows] for column in rows[0]})
    import dataclasses

    placed = [dataclasses.replace(one, channels=channels.get(one.sensor, ()))
              for one in resolved]
    return geometry, table, placed, notes
