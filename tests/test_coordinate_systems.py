"""Drawing a coordinate system: named directions, angles as arcs."""

import numpy as np


def identity_frame(origin=(0.0, 0.0, 0.0)):
    matrix = np.zeros((4, 3))
    matrix[:3] = np.eye(3)
    matrix[3] = origin
    return matrix


def _label_actors(plotter):
    return [name for name in plotter.renderer.actors if 'label' in name.lower()]
