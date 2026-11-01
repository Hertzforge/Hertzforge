"""Drawing decisions from Visual Dynamics' `plot.time_statistics`: what a figure needs
to know, with none of the code that draws it."""

from __future__ import annotations

import numpy as np

#: the bars of the histogram. Odd, so the middle bar is centered on
#: the record's middle value; enough that the bell's shoulders and a
#: clipped record's spikes at each edge are separate bars
BINS = 81


def histogram(values: np.ndarray, bins: int = BINS
              ) -> tuple[np.ndarray, np.ndarray]:
    """(edges, density) of `values`: the histogram scaled so its area
    is one, over the span the values cover.

    The edges run from the smallest value to the largest, so a clipped
    record's samples on the rail fall in the outermost bars and stand
    there as spikes.
    """
    values = values[np.isfinite(values)]
    low, high = float(values.min()), float(values.max())
    if not high > low:
        high = low + 1.0
    density, edges = np.histogram(values, bins=bins, range=(low, high),
                                  density=True)
    return edges, density
