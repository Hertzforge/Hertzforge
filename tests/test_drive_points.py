"""The drive-point button: filter an FRF plot to the grid's diagonal.

A drive-point FRF has its response and reference at one DOF. The button does
not filter the plot behind the selection's back — it *is* a selection: the
diagonal cells of the FRF grid light up as if picked by hand, so the tree,
the grid and the plot keep telling one story, and the checked state is
derived from whether the selection equals the diagonal rather than stored.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.data import Frf


def with_drive_points():
    """3 responses x 2 references, complete, with 2 drive points."""
    return Frf(abscissa=np.linspace(0, 128, 64),
               ordinate=np.ones((6, 64), dtype=complex),
               response_dof=['1Z+', '1Z+', '2Z+', '2Z+', '3Z+', '3Z+'],
               reference_dof=['1Z+', '2Z+'] * 3,
               ordinate_dim='acceleration/force',
               ordinate_unit='m/s**2', reference_unit='N')
