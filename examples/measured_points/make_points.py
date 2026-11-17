"""A made-up file of points probed on four accelerometers, in the layout a
measuring arm's software exports ("Feature Measured Points Table").

    python examples/measured_points/make_points.py probed_points.csv

Two tri-axial accelerometers and two uni-axial ones on a made-up part.
Nothing here was measured: every point is computed from a body placed at
a known pose, with a few micrometers of scatter, so what Fit Sensors
finds can be checked against `SENSORS`.

Two things are wrong in it on purpose, as they go wrong at the arm:

- sensor 21 was probed on its X- face, and named so (`21_-1`);
- the tops of the two uni-axial accelerometers were named for each
  other's sensor (`31_3` is on sensor 32 and `32_3` on sensor 31).

`docs/sensor-positions.md` walks through importing the file, correcting
the second, and making the test geometry and channel table.
"""

from __future__ import annotations

import sys

import numpy as np
from scipy.spatial.transform import Rotation

#: the accelerometers, by sensor: its kind, its size in millimeters (a
#: cube's side, or a round body's diameter and length), where its center
#: is in millimeters, and how it is turned (degrees about X, Y, then Z)
SENSORS = {
    11: ('cube', (10.2,), (120.0, 40.0, 25.1), (0.0, 0.0, 30.0)),
    21: ('cube', (6.3,), (310.0, 85.0, 23.15), (0.0, 0.0, -75.0)),
    31: ('cylinder', (7.0, 5.0), (60.0, 150.0, 22.5), (0.0, 0.0, 0.0)),
    32: ('cylinder', (16.0, 9.0), (255.0, 190.0, 12.0), (0.0, 90.0, 0.0)),
}

#: the faces probed on each cube, and how many points on each surface
FACES = {11: (1, 2, 3), 21: (-1, 2, 3)}
SCATTER_MM = 0.004


def frame_of(angles: tuple[float, float, float]) -> np.ndarray:
    """A body's X, Y and Z as rows, turned by `angles` in degrees about
    the fixed X, then Y, then Z."""
    return Rotation.from_euler('xyz', angles, degrees=True).as_matrix().T


def features(seed: int = 7) -> dict[str, np.ndarray]:
    """Each feature's points in millimeters, by the name it was probed
    under: the wrong names among them."""
    rng = np.random.default_rng(seed)
    out: dict[str, np.ndarray] = {}
    for sensor, (kind, size, center, angles) in SENSORS.items():
        frame, center = frame_of(angles), np.asarray(center)
        if kind == 'cube':
            side = size[0]
            for label in FACES[sensor]:
                k, sign = abs(label) - 1, np.sign(label)
                spread = rng.uniform(-0.38, 0.38, (5, 3)) * side
                spread[:, k] = sign * side / 2 + rng.normal(0, SCATTER_MM, 5)
                out[f'{sensor}_{label}'] = center + spread @ frame
            continue
        diameter, length = size
        angle = np.radians(rng.uniform(0, 200, 30))
        along = rng.uniform(-0.1, 0.35, 30) * length
        reach = diameter / 2 + rng.normal(0, SCATTER_MM, 30)
        out[f'{sensor}_c'] = (center + np.outer(along, frame[2])
                              + (np.cos(angle) * reach)[:, None] * frame[0]
                              + (np.sin(angle) * reach)[:, None] * frame[1])
        spot = rng.uniform(-0.55, 0.55, (5, 2)) * diameter / 2
        top = (center + (length / 2 + rng.normal(0, SCATTER_MM, 5))[:, None] * frame[2]
               + np.outer(spot[:, 0], frame[0]) + np.outer(spot[:, 1], frame[1]))
        # named for the other uni-axial accelerometer, as happened at the arm
        out[f'{ {31: 32, 32: 31}[sensor] }_3'] = top
    return out


def table_text(points: dict[str, np.ndarray]) -> str:
    """The features as the arm's software writes them: a header, then a
    block a feature, its points listed by deviation with their index."""
    lines = ['Feature Measured Points Table,,,,,', 'Length Units,Millimeters,,,,',
             'Coordinate System,world,,,,', 'Data Alignments,original,,,,']
    for name, xyz in points.items():
        middle = xyz.mean(axis=0)
        if name.endswith('_c'):
            deviation = np.zeros(len(xyz))                 # the arm's own fit is not made here
        else:
            normal = np.linalg.svd(xyz - middle)[2][2]
            deviation = (xyz - middle) @ normal
        order = np.argsort(-deviation)
        lines += [',,,,,', f'Name,{name},,,,', f'No. Pts,{len(xyz)},,,,',
                  f'Min Dev,{deviation.min():.3f},,,,', f'Max Dev,{deviation.max():.3f},,,,',
                  ',Index,Dev,X,Y,Z']
        lines += [f'Feature,{k + 1},{deviation[k]:.3f},{xyz[k, 0]:.3f},{xyz[k, 1]:.3f},'
                  f'{xyz[k, 2]:.3f}' for k in order]
    return '\r\n'.join(lines) + '\r\n'


def write(path: str, seed: int = 7) -> None:
    """Write the made-up table to `path`."""
    with open(path, 'w', newline='') as handle:
        handle.write(table_text(features(seed)))


if __name__ == '__main__':
    write(sys.argv[1] if len(sys.argv) > 1 else 'probed_points.csv')
