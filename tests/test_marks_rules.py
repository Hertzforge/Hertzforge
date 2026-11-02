"""The stage marks' geometry by value, where a mutation pass found it
reached only through pictures (2026-10-11).

`tools/mutate.py` over viz/marks.py caught 74 of 235 against its three
test files. Most of what survived draws handles and previews into a
window; the pieces here are the plain geometry the app's stage and
the report's canvas share, and they can be held to numbers.
"""

from __future__ import annotations

from hertzforge.drawing.viz.waterfall import STAGE

SX, SY, SZ = STAGE
