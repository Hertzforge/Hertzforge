"""Averaging frames and shock windows drawn on the waterfall stage.

The marks read the same numbers the flat overlays read —
`Averaging.frame_bounds`, `window_shape`, the shocks' own windows —
so the assertions pin positions against those numbers, not against
the drawing."""

from __future__ import annotations

from hertzforge.core.averaging import Averaging

# a nonzero origin on purpose: a record cut to start at half a second
# must have its marks shifted with it, and extents starting at zero
# would let a dropped origin term pass
EXTENTS = (0.5, 8.5, -1.0, 1.0)


def _averaging(frames=5, length=1024, rate=1024.0):
    return Averaging(frame_length=length, frames=frames, overlap=0.5,
                     window='hann'), rate


def _drawn_x_labels(pane):
    """The x labels as the screen draws them — GetXAxisRange can read
    the truth while the rendered strings are stale, which is exactly
    the failure this measures."""
    axes = pane.waterfall_plotter.renderer.cube_axes_actor
    labels = axes.GetAxisLabels(0)
    out = []
    for k in range(labels.GetNumberOfValues()):
        try:
            out.append(float(labels.GetValue(k)))
        except ValueError:
            pass
    return out
