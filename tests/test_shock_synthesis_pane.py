"""SRS synthesis in the Generate Signal pane: the synthesis iterated off
the window's thread once the settings rest, held for them, drawn as
`plot_shock_synthesis` draws it, and made by Generate on the worker.
"""

from __future__ import annotations


def _shown(panel):
    return {k for k, e in panel._editors.items() if e.isVisibleTo(panel)}
