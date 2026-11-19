"""Turning and sliding a coordinate system.

A coordinate system is stored as four rows: three orthonormal basis
vectors and an origin. Turning it about one of its axes changes only the
basis, and the origin stays where it is; sliding it along one moves only
the origin.
"""

from __future__ import annotations

import numpy as np
from numpy.typing import ArrayLike


def rotation_about(axis: ArrayLike, angle: float) -> np.ndarray:
    """The 3x3 rotation of `angle` radians about `axis`, by Rodrigues'
    formula.

    `axis` is normalized here, so it need not be a unit vector. A
    positive angle turns counterclockwise looking down the axis toward
    the origin (the right-hand rule).
    """
    axis = np.asarray(axis, dtype=np.float64)
    axis = axis / np.linalg.norm(axis)
    cross = np.array([[0.0, -axis[2], axis[1]],
                      [axis[2], 0.0, -axis[0]],
                      [-axis[1], axis[0], 0.0]])
    return (np.eye(3) + np.sin(angle) * cross
            + (1.0 - np.cos(angle)) * (cross @ cross))


def rotate_frame(matrix: ArrayLike, axis: int, angle: float) -> np.ndarray:
    """A copy of `matrix` with its basis turned `angle` radians about one
    of its own axes.

    The axis moves with the frame, so turning about X twice by 45
    degrees is turning about the frame's X by 90 degrees, not about the
    world's X. The origin is unchanged.
    """
    matrix = np.array(matrix, dtype=np.float64)
    turned = np.array(matrix)
    turned[:3] = matrix[:3] @ rotation_about(matrix[axis], angle).T
    return turned


def identity_frame(matrix: ArrayLike) -> np.ndarray:
    """The frame with its rotation undone: the identity basis at the
    same origin."""
    matrix = np.asarray(matrix, dtype=np.float64)
    reset = np.zeros_like(matrix)
    reset[:3] = np.eye(3)
    reset[3] = matrix[3]
    return reset


# ---- sliding --------------------------------------------------------------

def translate_frame(matrix: ArrayLike, axis: int, distance: float) -> np.ndarray:
    """A copy of `matrix` with its origin moved `distance` along one of
    its own axes. The basis is unchanged; `rotate_frame` is the
    opposite case."""
    moved = np.array(matrix, dtype=np.float64)
    moved[3] = moved[3] + float(distance) * moved[axis]
    return moved


# ---- angles -----------------------------------------------------------------


def frame_from_angles(angles: ArrayLike, origin: ArrayLike = (0.0, 0.0, 0.0)
                      ) -> np.ndarray:
    """A frame at `origin` turned by `angles`, in degrees, about the
    geometry's fixed X, then Y, then Z axes, the order a person types
    them in. The first three rows are the frame's axes and the fourth is
    its origin, as in a coordinate system."""
    ax, ay, az = (np.radians(float(a)) for a in angles)
    turn = (rotation_about((0.0, 0.0, 1.0), az) @ rotation_about((0.0, 1.0, 0.0), ay)
            @ rotation_about((1.0, 0.0, 0.0), ax))
    frame = np.zeros((4, 3))
    frame[:3] = turn.T
    frame[3] = np.asarray(origin, dtype=np.float64)
    return frame


def angles_of(matrix: ArrayLike) -> tuple[float, float, float]:
    """The fixed-axis X, Y, Z angles in degrees that turn the geometry's
    axes into the frame's, the inverse of `frame_from_angles`. X and Z
    are in (-180, 180] and Y in [-90, 90]. At a quarter turn about Y the
    X and Z turns are about the same axis, and the angle is given as Z
    with X at zero."""
    turn = np.asarray(matrix, dtype=np.float64)[:3].T
    ay = float(np.arcsin(np.clip(-turn[2, 0], -1.0, 1.0)))
    if abs(np.cos(ay)) > 1e-9:
        ax = float(np.arctan2(turn[2, 1], turn[2, 2]))
        az = float(np.arctan2(turn[1, 0], turn[0, 0]))
    else:
        ax = 0.0
        az = float(np.arctan2(-turn[0, 1], turn[1, 1]))
    return tuple(float(np.round(np.degrees(a), 9)) + 0.0 for a in (ax, ay, az))
