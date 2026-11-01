"""Solids in the app (Brandon, 2026-10-11): the Solids row under every
geometry, Add Block and Add Cylinder in the add pane, the
booleans on picked parts, Take Apart, Delete, and the parts drawn
see-through in the view.

The geometry's own verbs are pinned in `test_solids.py`; here, that the
window reaches them as a person would and shows what they did.
"""

from __future__ import annotations

INCH = 0.0254


def _shown(widget):
    return not widget.isHidden()
