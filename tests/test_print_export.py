"""Plots written for print keep their proportions.

pyqtgraph's ImageExporter, asked for more pixels than a plot's logical
width, scaled its items unevenly: at three times the width the legend's
labels came out a third of their set size and QFont tick labels half
again too large (the band-average paper's print figures, 2026-09-26).
`plot.save_image` paints the laid-out plot at a device ratio instead, so
everything scales together, and says its resolution in the file.
"""

from __future__ import annotations

import numpy as np

_ALIVE = []


def _red_height(image):
    """The tallest red glyph in an image, in pixels."""
    from scipy import ndimage

    image = np.asarray(image).astype(int)
    red = (image[..., 0] > 150) & (image[..., 1] < 100) & (image[..., 2] < 100)
    labeled, _count = ndimage.label(red)
    heights = [piece[0].stop - piece[0].start
               for piece in ndimage.find_objects(labeled)
               if 3 < piece[0].stop - piece[0].start < 300
               and 2 < piece[1].stop - piece[1].start < 300]
    return max(heights, default=0)
