"""The specification sheet in the window: one reading, two doors.

On a channel table the toggle on the table bar makes a specification
at its control channels and opens the sheet on it; on a specification,
Edit on the plot bar opens the sheet on the object. A shape set's door,
at its modal coordinates, came off the bar (Brandon, 2026-10-22); a
script still writes one through `author_specification`. There is no button: every
edit lands on the specification as it is made (Brandon, 2026-09-06).
The sheet sits beside the plot, which previews the autospectra.
"""

from __future__ import annotations

#: the control channels the sheet opens at: two tri-axial points
CONTROLS = ['101X+', '101Y+', '101Z+', '102X+', '102Y+', '102Z+']


def _rows(view):
    return view.model().rowCount()


def _columns(view):
    return view.model().columnCount()
