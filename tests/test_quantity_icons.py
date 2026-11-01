"""A record's icon says what it measures.

One object routinely mixes quantities — a test's time data holds
accelerations, forces, voltages and temperatures side by side — so the
record, not the object, is what has to carry the answer. Putting it in the
row label instead cost twice the width and had to be read; a shape is read at
a glance.

Displacement, velocity and acceleration share one trace and are told apart by
Newton's dots, which is the notation a dynamicist already has.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.data import TimeHistory


def mixed():
    """What a real test's time data looks like."""
    dims = ['acceleration', 'force', 'voltage', 'temperature']
    units = ['m/s**2', 'N', 'V', 'K']
    return TimeHistory(abscissa=np.arange(4.0), ordinate=np.zeros((4, 4)),
                       response_dof=['1Z+', '1Z+', '2Z+', '3Z+'],
                       ordinate_dim=dims, ordinate_unit=units)
