"""The sine view: the toggle, the panel, the sampled preview, the
verb (Brandon, 2026-09-30). The extraction's settings ride the record
the way the filter's do — a taskbar toggle, a panel beside the plot,
the Extract button on it — with the smoothing chosen from the data
unless set, and the readings sampled at it before anything is made.
"""

from __future__ import annotations


def _cancellable(stopped):
    import time

    def verb(name, progress=None):
        for k in range(200):
            progress(k, 200)
            time.sleep(0.02)
        stopped.append('ran to the end')
        return name
    return verb
