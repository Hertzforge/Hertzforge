"""The drive prediction in the window: an FRF and a specification
selected together offer a Drive Prediction toggle, which draws the
drives, one channel against its target and every quantity against its
rating, with the settings and the verdict in the panel beside it.

A reading of two objects that exist, so a toggle with no act, offered
for exactly one FRF and one specification and for nothing else.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.data import Frf, Specification

G = 9.80665


def _frf(gain=2.0, dof='1Z+'):
    f = np.arange(0.0, 2001.0, 5.0)
    return Frf(f, np.full((1, len(f)), gain * G, dtype=complex),
               response_dof=dof, reference_dof='1Z+',
               ordinate_dim='acceleration/voltage')


def _spec(dof='1Z+', cls=Specification):
    return cls(np.array([20.0, 2000.0]), np.array([[0.04, 0.04]]) * G ** 2,
               response_dof=[dof], ordinate_dim='acceleration**2/frequency')
