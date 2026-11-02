"""The MAC's 3D reading: the default, the toggle, and picking parity.

The auto-MAC beside a mode table, the fit's MAC, and the cross-MAC all
draw through the same pair of surfaces, and the bars are the default —
made so once they could be picked like the grid. Picking runs through
`_pick_mac_pair`, one implementation for both readings, so the bars
cannot select differently from the grid; the marks (outline, boldest
outline, stripes) are drawn by the scene and asserted by actor.
"""

import numpy as np

from hertzforge.core.shapes import ShapeSet


def shapes(n=3, offset=0.0):
    rng = np.random.default_rng(4)
    return ShapeSet(np.linspace(10.0, 90.0, n) + offset,
                    np.full(n, 0.01), ['1Z+', '2Z+', '3Z+', '4Z+'],
                    rng.standard_normal((n, 4)))
