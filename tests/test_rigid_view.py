"""The rigid-body reading: six shapes previewed where they are set.

The toggle on the 3-D view's bar is offered to one whole geometry and
nothing else; its pane seeds the centroid in the display units and
hands every edit to the geometry as a `MassProperties` (which is what
the journal records and the generated set's badge fingerprints); the
scene animates the proposed set through the same animator a selected
set uses, with the reference point marked; and the button makes the
set through the project verb, linked into the geometry's group.
"""

from __future__ import annotations

import numpy as np

import hertzforge


def _geometry(unit='m'):
    rng = np.random.default_rng(3)
    return hertzforge.Geometry(np.arange(101, 108),
                                   rng.uniform(-1.0, 1.0, (7, 3)),
                                   length_unit=unit)
