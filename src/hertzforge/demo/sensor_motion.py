"""Responses of the free BARC at a set of sensors nobody has measured the
positions of, to try Infer Geometry on.

Fourteen accelerometers are put on the BARC of plates (`barc.geometry`):
five tri-axial, four with two axes and five with one, each turned its
own way on its surface and three of them tipped off it. The structure
hangs free and is shaken by random forces at the box's four bottom
corners, and the responses are computed the way a person would in the
app: the modes solved, projected onto the drives and onto the sensors'
own axes, each mode's response to the forces computed in time
(`Project.compute_modal_responses`) and expanded to the sensors
(`Project.expand`). White noise is then added to every channel.

    from hertzforge.demo import sensor_motion

    project = sensor_motion.project()
    project.geometry_from_motion('BARC Random', known='BARC Sensors (true)')

`project()` makes one test with all twelve drives running, which moves
the BARC in all six rigid motions. `project(sensor_motion.AXIS_TESTS)`
makes three tests instead, each driving along one global axis, which is
three rigid motions apiece: none of them places the sensors alone, and
the three together do.

The true positions are kept in the project as 'BARC Sensors (true)', to
check a result against and to give as the known geometry.
"""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any

import numpy as np

from . import barc, rc_plate

#: {node: (point in, outward normal, axes measured, turned about the normal °, tipped °)}.
#: A point is taken to the plate model's nearest node.
SENSORS: dict[int, tuple[tuple[float, float, float], tuple[int, int, int], str, float, float]] = {
    1: ((-2.0, 5.0625, 1.0), (0, 1, 0), 'XYZ', 25.0, 0.0),
    2: ((2.875, -1.5, 2.5), (1, 0, 0), 'XYZ', -40.0, 0.0),
    3: ((-1.5, -2.875, 0.5), (0, -1, 0), 'XYZ', 70.0, 0.0),
    4: ((-2.5, 4.0, 1.5), (-1, 0, 0), 'XYZ', 10.0, 20.0),
    5: ((0.5, 2.875, 2.75), (0, 1, 0), 'XYZ', 130.0, 0.0),
    6: ((2.0, 5.0625, 2.0), (0, 1, 0), 'XY', -15.0, 0.0),
    7: ((1.5, 2.875, 0.25), (0, 1, 0), 'XZ', 55.0, 0.0),
    8: ((-2.875, 1.0, 0.5), (-1, 0, 0), 'YZ', 35.0, 0.0),
    9: ((2.5, 4.0, 1.5), (1, 0, 0), 'XZ', -60.0, 15.0),
    10: ((0.0, 5.0625, 1.5), (0, 1, 0), 'Z', 0.0, 0.0),
    11: ((2.875, 2.0, 0.5), (1, 0, 0), 'Z', 0.0, 0.0),
    12: ((2.0, -2.875, 2.5), (0, -1, 0), 'Z', 0.0, 0.0),
    13: ((-2.875, -2.0, 2.5), (-1, 0, 0), 'Z', 0.0, 30.0),
    14: ((-2.5, 3.5, 0.5), (-1, 0, 0), 'X', 45.0, 0.0),
}
#: the drives' node numbers, one node a corner of `barc.DRIVES`
DRIVE_IDS = 201
#: {test: the global axes its drives push along}
RANDOM_TEST = {'BARC Random': 'XYZ'}
AXIS_TESTS = {'BARC Shake X': 'X', 'BARC Shake Y': 'Y', 'BARC Shake Z': 'Z'}
#: the forces: flat at `barc.FORCE_LEVEL`, lbf^2/Hz, over this band, Hz
FORCE_BAND = (5.0, 800.0)
#: the modes kept, to this frequency, Hz
SOLVE_TO = 1500.0
#: the noise added to every channel, g/sqrt(Hz)
NOISE = 1e-4
G = 9.80665
TRUE = 'BARC Sensors (true)'


def _turn(axis: np.ndarray, degrees: float) -> np.ndarray:
    """The rotation by `degrees` about `axis`."""
    axis = np.asarray(axis, dtype=float) / np.linalg.norm(axis)
    angle = np.radians(degrees)
    k = np.array([[0.0, -axis[2], axis[1]], [axis[2], 0.0, -axis[0]], [-axis[1], axis[0], 0.0]])
    return np.eye(3) + np.sin(angle) * k + (1.0 - np.cos(angle)) * k @ k


def frame(normal: Any, turned: float, tipped: float) -> np.ndarray:
    """A sensor's axes, rows X, Y and Z in the global system: the surface's
    frame (`instrumentation.surface_frame`) turned about the normal, then
    tipped about the sensor's own X."""
    from hertzforge.core.instrumentation import surface_frame

    axes = surface_frame(normal)
    axes = axes @ _turn(axes[2], turned).T
    return axes @ _turn(axes[0], tipped).T


def _nearest(geometry: Any, point: tuple[float, float, float]) -> int:
    gap = np.linalg.norm(geometry.node_xyz / barc.INCH - np.asarray(point), axis=1)
    return int(geometry.node_id[int(gap.argmin())])


def _force_psd(drives: list[str]) -> Any:
    """Every drive flat at `barc.FORCE_LEVEL` over `FORCE_BAND`, uncorrelated."""
    from hertzforge.core.data import Psd

    freq = np.arange(0.0, 1024.5, 1.0)
    low, high = FORCE_BAND
    level = np.where((freq >= low) & (freq <= high), barc.FORCE_LEVEL * barc.LBF ** 2, 0.0)
    return Psd(freq, np.tile(level, (len(drives), 1)), response_dof=drives,
               reference_dof=drives, ordinate_dim='force**2/frequency', ordinate_unit='N')


def project(tests: Mapping[str, str] | None = None, *, seconds: float = 20.0,
            sample_rate: float = 2048.0, noise: float = NOISE, seed: int = 0) -> Any:
    """The BARC, its sensors and a time history of their responses for each
    test.

    Parameters
    ----------
    tests : mapping, optional
        {name: the global axes its drives push along}; `RANDOM_TEST`, one
        test with every drive, when omitted. `AXIS_TESTS` is three tests
        of one axis each.
    seconds : float, default 20.0
        The length of each test.
    sample_rate : float, default 2048.0
        Samples per second.
    noise : float, default `NOISE`
        The white noise added to every channel, g/sqrt(Hz).
    seed : int, default 0
        Seeds the forces and the noise.

    Returns
    -------
    Project
        'BARC' (the plate model), 'BARC Sensors (true)' (the sensors
        where they really are, each in its own system), 'BARC Channels',
        the modes at the drives and at the sensors, and a time history
        of accelerations named for each test.
    """
    from hertzforge import ChannelTable
    from hertzforge.core.signals import random_history
    from hertzforge.project import Project

    tests = dict(RANDOM_TEST if tests is None else tests)
    model = barc.geometry()
    rows, ids, fe_nodes, frames = [], [], [], []
    for node, (label, point) in enumerate(barc.DRIVES.items(), start=DRIVE_IDS):
        ids.append(node)
        fe_nodes.append(_nearest(model, point))
        frames.append(None)
        rows += [{'node': node, 'direction': direction, 'role': 'reference',
                  'channel_type': 'force', 'unit': 'lbf', 'comment': label}
                 for direction in barc.inward(point)]
    for node, (point, normal, axes, turned, tipped) in SENSORS.items():
        ids.append(node)
        fe_nodes.append(_nearest(model, point))
        frames.append(frame(normal, turned, tipped))
        rows += [{'node': node, 'direction': f'{axis}+', 'role': 'response',
                  'channel_type': 'acceleration', 'unit': 'g',
                  'triax_dof': axis if len(axes) == 3 else ''} for axis in axes]
    names = ('node', 'direction', 'role', 'channel_type', 'unit', 'triax_dof', 'comment')
    columns: dict[str, list] = {name: [row.get(name, '') for row in rows] for name in names}
    columns['channel'] = list(range(1, len(rows) + 1))

    out = Project('BARC Sensor Motion')
    out.add('BARC', model)
    truth = rc_plate.test_geometry(model, ids, fe_nodes, frames)
    rc_plate.draw(truth, {})
    out.add(TRUE, truth)
    out.add('BARC Channels', ChannelTable(columns))
    out.set_basis(TRUE, 'BARC Channels')
    modes = out.solve_modes('BARC', maximum_frequency=SOLVE_TO, damping=barc.DAMPING)
    drive_rows = [k for k, row in enumerate(rows) if row['role'] == 'reference']
    out.project_onto_basis(modes, onto='BARC Channels', channels=drive_rows,
                           name='BARC Modes @ Force DOFs')
    out.project_onto_basis(modes, onto='BARC Channels',
                           channels=[k for k in range(len(rows)) if k not in drive_rows],
                           name='BARC Modes @ Response DOFs')
    rng = np.random.default_rng(seed)
    for test, axes in tests.items():
        drives = [f"{rows[k]['node']}{rows[k]['direction']}" for k in drive_rows
                  if rows[k]['direction'][0] in axes]
        forces = out.add(f'{test} Forces', random_history(
            _force_psd(drives), duration=seconds, sample_rate=sample_rate, seed=rng))
        modal = out.compute_modal_responses(forces, 'BARC Modes @ Force DOFs')
        made = out.expand(modal, 'BARC Modes @ Response DOFs', name=test)
        history = out[made]
        history.ordinate = history.ordinate + (
            rng.standard_normal(history.ordinate.shape) * noise * G * np.sqrt(sample_rate / 2))
        out.remove(modal)
        out.remove(forces)
    return out
