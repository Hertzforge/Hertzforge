"""Coherence reads two ways, and the plot lets you pick.

A line plot answers "is this channel any good". It cannot answer that for 339
channels at once — they overlay into a band and the curve budget means most
are never drawn. The map answers it for all of them: a poor channel is a dark
row, a poor band a dark column.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.data import MultipleCoherence


def channels(count, samples=32):
    """`count` coherence channels, each a ramp from 0 to 1 along frequency."""
    ordinate = np.tile(np.linspace(0.0, 1.0, samples), (count, 1))
    return MultipleCoherence(abscissa=np.linspace(0, 128, samples),
                             ordinate=ordinate,
                             response_dof=[f'{100 + i}Z+' for i in range(count)])


def image_of(plot):
    return next(item for item in plot.items
                if type(item).__name__ == 'ImageItem')
