"""The notch prediction in the window and on its own: a transmissibility
from a control channel selected with the specification written there
offers a Notch Prediction toggle, which draws the control with its
predicted notch hatched and each response channel against its limit
profile, with the verdict, the notches in a table and the reading in
words in the panel beside it.

A reading of objects that exist, so a toggle with no act, offered for a
transmissibility from the channel a selected specification is written
at, and for nothing else.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.data import Frf, Specification

G = 9.80665

F = np.arange(0.0, 2001.0, 1.0)


def _frf(gain=4.0, dim='acceleration/acceleration', reference='1Z+'):
    t = np.ones(len(F), dtype=complex)
    t[(F >= 240) & (F <= 260)] = gain
    return Frf(F, t[None], response_dof=['2Z+'], reference_dof=[reference],
               ordinate_dim=dim)


def _spec(profile=0.18, control='1Z+'):
    level = 0.04 * G ** 2
    values = np.array([[level, level], [profile * G ** 2] * 2])
    nan = np.full(2, np.nan)
    return Specification(
        np.array([20.0, 2000.0]), values, response_dof=[control, '2Z+'],
        ordinate_dim='acceleration**2/frequency',
        abort_lower=np.array([values[0] / 2, nan]),
        abort_upper=np.array([values[0] * 2, nan]))


VERDICT = ('1 notch in the control at full level, the deepest 5.5 dB at '
           '240 Hz for 2Z+; it takes the control outside the abort '
           'tolerance')
