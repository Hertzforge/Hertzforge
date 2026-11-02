"""The two matrix checks in the window: Conditioning on an FRF from
several references or a CSD, Realizability on a CSD or specification,
each drawn as a figure of its own with a cursor read in its pane, and
the specification sheet's refusal of entries that cannot hold.

Readings of objects that exist, so toggles on the bar with no act, one
up at a time; Realizability's pane carries the one apply, Make
Realizable, offered only while there is something to repair.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.data import Frf, Specification

F = np.linspace(10.0, 1000.0, 100)


def _plant(references=('1Z+', '2Z+')):
    """Two modes, at 100 Hz moving both drives together and at 400 Hz
    against each other, read at two responses: the columns are one at
    the first mode, where the matrix is singular."""
    w = 2 * np.pi * F
    h = np.zeros((len(F), 2, 2), dtype=complex)
    for fn, phi in ((100.0, np.array([1.0, 1.0])),
                    (400.0, np.array([1.0, -1.0]))):
        wn = 2 * np.pi * fn
        h += np.outer(phi, phi)[None] / (
            wn ** 2 - w[:, None, None] ** 2
            + 2j * 0.02 * wn * w[:, None, None])
    rows, resp, refs = [], [], []
    for i, r in enumerate(('101Z+', '102Z+')):
        for j, d in enumerate(references):
            rows.append(h[:, i, j])
            resp.append(r)
            refs.append(d)
    return Frf(F, np.asarray(rows), response_dof=resp, reference_dof=refs,
               ordinate_dim=['acceleration/force'] * len(rows))


def _specification(phase_13=180.0, unstated=False):
    """Three channels at 0.01 g²/Hz, coherence one between every pair,
    in step between 1-2 and 2-3 and at `phase_13` between 1-3; with
    `unstated` the 1-3 pair is left out."""
    channels = ['1Z+', '2Z+', '3Z+']
    phases = {(0, 1): 0.0, (1, 2): 0.0, (0, 2): phase_13}
    rows, resp, refs = [], [], []
    for i in range(3):
        for j in range(3):
            if i == j:
                value = 1.0 + 0j
            else:
                key = (min(i, j), max(i, j))
                if unstated and key == (0, 2):
                    continue
                value = np.exp(1j * np.radians(phases[key])
                               * (1 if i < j else -1))
            rows.append(np.full(len(F), value * 0.01 * 9.80665 ** 2))
            resp.append(channels[i])
            refs.append(channels[j])
    return Specification(F, np.asarray(rows), response_dof=resp,
                         reference_dof=refs,
                         ordinate_dim=['acceleration**2/frequency']
                         * len(rows))
