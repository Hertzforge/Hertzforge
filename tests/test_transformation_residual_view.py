"""The rigid-body residual reading in the window: on a density
transformed through six rigid-body shapes, the measured power at the
channels, the part the six motions explain and the residual, with the
transformation's conditioning and where it holds in the pane.

A reading of a transform that exists, so a toggle on the bar with no
act, offered on such a density and nowhere else.
"""

from __future__ import annotations

import numpy as np

import hertzforge
from hertzforge.core.averaging import Averaging
from hertzforge.core.rigid import MassProperties, rigid_body_shapes


def _objects(flex_above=500.0):
    rng = np.random.default_rng(5)
    geometry = hertzforge.Geometry(np.arange(101, 106),
                                       rng.uniform(-1.0, 1.0, (5, 3)),
                                       length_unit='m')
    rigid = rigid_body_shapes(geometry, MassProperties((0.0, 0.0, 0.0)))
    n, rate = 16384, 2048.0
    q = rng.standard_normal((6, n))
    noise = rng.standard_normal((15, n))
    spectrum = np.fft.rfft(noise, axis=1)
    spectrum[:, np.fft.rfftfreq(n, 1.0 / rate) < flex_above] = 0.0
    u = rigid.shape_matrix.T @ q + 2.0 * np.fft.irfft(spectrum, n, axis=1)
    run = hertzforge.TimeHistory(np.arange(n) / rate, u,
                                     response_dof=list(rigid.coordinate),
                                     ordinate_dim='acceleration')
    run.averaging = Averaging(frame_length=1024, frames=15)
    return rigid, run
