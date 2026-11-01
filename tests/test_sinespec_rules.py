"""Where the sine stage puts a specification, read off its meshes
(2026-10-11).

`tools/mutate.py` over viz/sinespec.py caught 65 of 168 against its
tests, 72 of the survivors in `add_sine_stage`: frequency along,
time in depth, amplitude up, each onto `STAGE`, and a mutant that
moved one of them drew a stage the tests still counted right. A
plotter that keeps what it is given stands in for the window.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.sine import SineSweepSpecification, SineTone
from hertzforge.drawing.viz.waterfall import STAGE

SX, SY, SZ = STAGE


class _Kept:
    def __init__(self):
        self.meshes, self.labels, self.bounds = {}, None, None

    def add_mesh(self, mesh, name=None, **_style):
        self.meshes[name] = mesh

    def add_point_labels(self, points, names, **_style):
        self.labels = (np.asarray(points), list(names))

    def show_bounds(self, **style):
        self.bounds = style['axes_ranges']


def _spec():
    """Two tones on one channel: 100 to 400 Hz from 1 s at 2 m/s^2 and
    400 down to 200 Hz from 4 s at 1 m/s^2, the first with an upper
    abort at 4."""
    up = SineTone('Up', 1.0, [100.0, 400.0], [[2.0], [2.0]], [0], [120.0])
    down = SineTone('Down', 4.0, [400.0, 200.0], [[1.0], [1.0]], [0], [-120.0])
    up.limits['abort_upper'] = np.array([[4.0], [4.0]])
    return SineSweepSpecification([up, down], ['1X+'], ordinate_unit='m/s**2')
