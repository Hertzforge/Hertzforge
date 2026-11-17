"""A scan drawn: its points as layers of a `shape3d` view.

A cloud of points is colored by height, so its shape reads without
shading; a scan with features is colored by feature. Scans picked
together, and a geometry picked with them, share one scene, which is
how probed points are checked against a laser scan of the same part.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from hertzforge.core.scan import UNASSIGNED, Scan
from hertzforge.drawing.theme import VIRIDIS

from . import views as v

#: the most points one layer draws; a larger scan is drawn every nth point
DRAWN_POINTS = 500_000

#: a scan of this many points or fewer is drawn with large points: it was
#: probed by hand, and each point matters
PROBED_POINTS = 5000

#: point sizes on screen, in pixels
CLOUD_SIZE, PROBED_SIZE = 2.0, 6.0

#: the colors features take in turn (sRGB), and the color of a point in none
FEATURE_COLORS = ((76, 146, 217), (255, 140, 43), (63, 185, 80), (219, 97, 162),
                  (163, 113, 247), (210, 153, 34), (57, 197, 207), (248, 81, 73))
UNASSIGNED_COLOR = (139, 148, 158)


def _by_height(points: np.ndarray) -> np.ndarray:
    """Each point's color from its height along the axis the points
    span least, the way a part lying on a table is read from above."""
    if not len(points):
        return np.empty((0, 3), np.uint8)
    spans = np.ptp(points, axis=0)
    height = points[:, int(np.argmin(spans))]
    low, high = float(height.min()), float(height.max())
    at = (height - low) / (high - low) if high > low else np.zeros(len(height))
    stops = np.asarray(VIRIDIS, dtype=float)
    place = at * (len(stops) - 1)
    lower = np.minimum(place.astype(int), len(stops) - 2)
    part = (place - lower)[:, None]
    return np.rint(stops[lower] * (1 - part) + stops[lower + 1] * part).astype(np.uint8)


def _by_feature(scan: Scan) -> np.ndarray:
    palette = np.asarray(FEATURE_COLORS, dtype=np.uint8)
    colors = palette[scan.feature_of % len(palette)]
    colors[scan.feature_of == UNASSIGNED] = UNASSIGNED_COLOR
    return colors


def layer(scan: Scan) -> tuple[dict[str, Any], str]:
    """One scan as a `points` layer of a `shape3d` view, and what to say
    when not every point is drawn ('' otherwise)."""
    step = -(-scan.num_points // DRAWN_POINTS) if scan.num_points > DRAWN_POINTS else 1
    colors = _by_feature(scan) if scan.features else _by_height(scan.points)
    note = (f'every {step}th point of {scan.num_points:,} is drawn' if step > 1 else '')
    return {'xyz': np.ascontiguousarray(scan.points[::step], np.float32),
            'colors': np.ascontiguousarray(colors[::step], np.uint8),
            'size': PROBED_SIZE if scan.num_points <= PROBED_POINTS else CLOUD_SIZE}, note


def drawn(project: Any, names: list[str], base: dict | None = None) -> tuple[dict, list[str]]:
    """The scans `names` in one `shape3d` view, over `base` (a geometry's
    view) when there is one; (view, notes)."""
    scans = [project[name] for name in names]
    layers, notes = [], []
    for name, scan in zip(names, scans, strict=True):
        made, note = layer(scan)
        layers.append(made)
        if note:
            notes.append(f'{name}: {note}')
    defined = {scan.units_defined for scan in scans}
    if base is None:
        title = names[0] if len(names) == 1 else ' and '.join(names)
        base = v.shape3d(title, 'm' if defined == {True} else 'undefined',
                         np.empty((0, 3), np.float32), [])
    else:
        defined.add(base['unit'] == 'm')
    if len(defined) > 1:
        notes.append('units are declared on some of these and not on others, so they are '
                     'not drawn to one scale')
    base['points'] = layers
    return base, notes
