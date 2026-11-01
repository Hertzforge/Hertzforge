"""Where the banded stage puts things, read off its meshes
(2026-10-11).

`tools/mutate.py` over viz/banded.py caught 60 of 109 against the
stage's tests, which count stations and pixels. The stage is a
normalization — frequency along, channel in depth, level up, each onto
`STAGE` — and 35 survivors moved something along one of those axes.
A plotter that keeps what it is given stands in for the window.
"""

from __future__ import annotations

import numpy as np

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

    def remove_actor(self, name):
        self.meshes.pop(name, None)
