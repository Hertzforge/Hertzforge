"""Creating geometry by clicking in the 3D view.

The one gesture in hertzforge that *makes* something out of nothing: the
`+` on the toolbar arms it, and after that a click in the view is a
node, a coordinate system, or the next corner of an element. It is also
the part with the most state — what is being edited, what the cursor is
over, which nodes are half-picked — and it was covered only by the
legacy end-to-end script, twenty seconds in a subprocess whose failures
name a line number in a 1900-line file.

The clicks go in through `_add_at` and `hover_at`, which is how a click
arrives from VTK anyway; `conftest`'s offscreen plotter gives a real
projector, so the screen positions are the ones a mouse would land on.
"""

from __future__ import annotations


def _drawn(icon):
    """An icon's pixels, to tell the pencil on from the pencil off."""
    image = icon.pixmap(64, 64).toImage()
    return bytes(image.constBits())
