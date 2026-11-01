"""The averaging drawn on the record it will be cut from.

Five numbers in a table do not say whether the analysis sits where you
meant it to. The marks do, so what these hold is that they are where the
frames are, that a drag lands on whole frames, and that the overlay
takes every one of its items away again.

The rail above the trace carries one window per frame; the band under
each falls to the foot of the plot from that frame's own slot, so the
height says which level a frame is on and an overlap has a shape rather
than only a shade.
"""

from __future__ import annotations

import numpy as np

RATE = 1024.0

SAMPLES = 8192

# a GraphicsLayoutWidget that goes out of scope takes its items' C++
# objects with it, and touching one afterwards raises "already deleted"
_ALIVE = []


def fill_color(item):
    return item.opts['fillBrush'].color()


def band_gradient(band):
    """([0..1 across the frame], alpha at each) for a band's shading."""
    stops = band.opts['fillBrush'].gradient().stops()
    return [at for at, _color in stops], [c.alpha() for _at, c in stops]


def bands_of(overlaid):
    """[(low, high)] each band covers, as an array: pytest.approx
    compares arrays of pairs, and refuses a list of tuples since 9.2."""
    return np.array([(item.getData()[0][0], item.getData()[0][-1])
                     for item in overlaid.bands])


def band_tops(overlaid):
    """The height each band rises to — its frame's slot on the rail."""
    return [float(item.getData()[1][0]) for item in overlaid.bands]


def glyphs_of(overlaid):
    """[(low, high, baseline, peak)] for every window on the rail."""
    out = []
    for item in overlaid.glyphs:
        x, y = item.getData()
        out.append((float(x.min()), float(x.max()),
                    float(item.opts['fillLevel']), float(y.max())))
    return out


def drag(overlaid, low, high):
    """The gesture, start to finish.

    setRegion emits both sigRegionChanged and sigRegionChangeFinished,
    which is what a drag and its release do.
    """
    overlaid.region.setRegion((low, high))
    return overlaid.averaging
