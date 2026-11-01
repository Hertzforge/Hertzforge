"""The transform in the window: an act on the bar that makes the object.

A transform has no settings, so it is an act and not a reading
(Brandon, 2026-09-04): with one time history and one shape set
selected together, the plot's bar offers Transform to Modal Coordinates
— or Expand to Physical Responses, when the record reads as modal —
and pressing it runs the project verb, shows what it made and puts
the account on the status line. No toggle, no pane, no preview, and
nothing in the right-click menu: every act lives on the bar.
"""

from __future__ import annotations

import numpy as np

import hertzforge
from hertzforge.core.rigid import MassProperties, rigid_body_shapes

T = np.arange(1024) / 1024.0


def _fixtures():
    rng = np.random.default_rng(1)
    geometry = hertzforge.Geometry(np.arange(101, 106),
                                       rng.uniform(-1.0, 1.0, (5, 3)),
                                       length_unit='m')
    rigid = rigid_body_shapes(geometry, MassProperties((0, 0, 0)))
    q = np.zeros((6, len(T)))
    q[0] = np.sin(2 * np.pi * 5 * T)
    q[5] = 0.3 * np.cos(2 * np.pi * 3 * T)
    run = hertzforge.TimeHistory(T, rigid.shape_matrix.T @ q,
                                     response_dof=list(rigid.coordinate),
                                     ordinate_dim='acceleration')
    return geometry, rigid, run
