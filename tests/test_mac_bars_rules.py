"""The MAC bars as solids: each bar a closed box standing where its
cell is, as tall as its value, every outline and checker on it
(2026-10-11).

`tools/mutate.py` over viz/mac_bars.py caught 97 of 229 against
test_mac_bars.py, which checks the bars through pictures, picks and
counts; 132 survivors changed a corner index, a winding or an offset,
and a picture of a field of bars does not say which corner is where.
A plotter that keeps what it is given stands in for the window, and
the meshes are read for their geometry.
"""

from __future__ import annotations

import numpy as np


class _Kept:
    """A plotter that keeps every mesh by name and draws nothing."""

    def __init__(self):
        self.meshes, self.labels, self.camera_position = {}, None, None

    def add_mesh(self, mesh, name=None, **_style):
        self.meshes[name] = mesh

    def add_point_labels(self, points, names, **_style):
        self.labels = (np.asarray(points), list(names))

    def remove_actor(self, name):
        self.meshes.pop(name, None)

    def show_bounds(self, **_style):
        pass


MAC = np.array([[1.0, 0.25, 0.0], [0.1, 0.9, 0.5]])


def _faces(mesh):
    return mesh.faces.reshape(-1, 5)[:, 1:]
