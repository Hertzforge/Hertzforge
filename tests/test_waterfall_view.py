"""The 2D/3D toggle: the default, what it swaps, what the camera keeps.

The scene's own claims — one mesh, one color scale, log mapping — are
`test_waterfall.py`'s. This file holds the window's half: the 3-D
reading is the default wherever plain curves draw (a single record
included), unchecking asks for the flat plot, the surfaces swap without
losing each other's state, and the camera obeys the standing rule
(placed when the displayed object changes, never on a reread).
"""

import numpy as np

from hertzforge.core.data import Psd, TimeHistory

PSD_DIM = 'acceleration**2/frequency'

PSD_UNIT = '(m/s**2)**2/Hz'


def psd(rows=6, dims=None, units=None):
    rng = np.random.default_rng(5)
    f = np.linspace(0.0, 2000.0, 257)
    values = 1e-4 * (1 + rng.random((rows, 257)))
    return Psd(f, values, response_dof=[f'{100 + i}Z+' for i in range(rows)],
               ordinate_dim=dims or [PSD_DIM] * rows,
               ordinate_unit=units or [PSD_UNIT] * rows)


def history(rows=3):
    t = np.linspace(0.0, 1.0, 4096)
    return TimeHistory(t, np.tile(np.sin(60.0 * t), (rows, 1)),
                       response_dof=[f'{i}Z+' for i in range(1, rows + 1)],
                       ordinate_dim=['acceleration'] * rows,
                       ordinate_unit=['m/s**2'] * rows)


def camera_of(plotter):
    return np.asarray([list(point) for point in plotter.camera_position])


def in_3d(pane):
    return (pane.waterfall_plotter is not None
            and not pane.graphics.isVisibleTo(pane)
            and 'waterfall' in pane.waterfall_plotter.renderer.actors)


def mixed_psd():
    return psd(rows=5,
               dims=[PSD_DIM] * 3 + ['voltage**2/frequency'] * 2,
               units=[PSD_UNIT] * 3 + ['V**2/Hz'] * 2)


def big_psd(rows=40, samples=4001):
    rng = np.random.default_rng(6)
    f = np.linspace(0.0, 2000.0, samples)
    return Psd(f, 1e-4 * (1 + rng.random((rows, samples))),
               response_dof=[f'{100 + i}Z+' for i in range(rows)],
               ordinate_dim=[PSD_DIM] * rows, ordinate_unit=[PSD_UNIT] * rows)
