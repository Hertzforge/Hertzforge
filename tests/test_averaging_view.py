"""The averaging view: a button on the bar, a table beside the plot.

Clicking a time history shows the trace as it always did. The button
adds the frames — shaded on the plot, listed in the table — and takes
them away again. Nothing here computes: the calculator in the tree is
still what turns a history into a PSD, with whatever is set at the
moment it is clicked.
"""

from __future__ import annotations

MODAL = ('plate', 'modal.nc4')

SPLIT = ('plate', 'modal_spectra.nc4')
