"""Matrix data expands into a grid, and the grid is what gets plotted.

Two things are worth guarding. First that only real matrices get a grid — a
partial set would come out as a grid of holes, and the rule that decides is
easy to loosen by accident. Second that the grid's selection is the record
selection: it is the only copy of that state, so if the plot ever stopped
reading it the two would silently disagree.
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge.core.data import TimeHistory


def dofs_of(rows):
    """grid_axes returns RowKeys; often only the DOF matters."""
    return [key.dof for key in rows]


@pytest.fixture(scope='module')
def spectra():
    return (hertzforge.import_file(fixture_path('plate', 'modal_spectra.nc4'))
            | hertzforge.import_file(fixture_path('plate',
                                            'random_spectra.nc4')))


def two_at_one_point():
    """A drive and the accelerometer beside it: same node, same direction,
    different quantity. A modal survey that measures its own drive points
    has one such pair per shaker."""

    dofs, dims, blocks = [], [], []
    for dof, dim in (('1Z+', 'acceleration'), ('2Z+', 'acceleration'),
                     ('2Z+', 'force')):
        for average in range(3):
            dofs.append(dof)
            dims.append(dim)
            blocks.append(f'avg {average + 1}')
    return TimeHistory(abscissa=np.arange(4.0),
                       ordinate=np.arange(9 * 4.0).reshape(9, 4),
                       response_dof=dofs, block=blocks, ordinate_dim=dims,
                       ordinate_unit=['m/s**2' if d == 'acceleration' else 'N'
                                      for d in dims])


def three_types_at_one_dof():
    """A volt, a newton and a meter per second squared, all at 5Z+."""

    dofs, dims, units, blocks = [], [], [], []
    for dim, unit in (('acceleration', 'm/s**2'), ('force', 'N'),
                      ('voltage', 'V')):
        for average in range(2):
            dofs.append('5Z+')
            dims.append(dim)
            units.append(unit)
            blocks.append(f'avg {average + 1}')
    return TimeHistory(abscissa=np.arange(4.0),
                       ordinate=np.arange(6 * 4.0).reshape(6, 4),
                       response_dof=dofs, block=blocks,
                       ordinate_dim=dims, ordinate_unit=units)


def repeated(**kwargs):
    """Four records, one DOF, two averages — so two channels at one point."""

    return TimeHistory(abscissa=np.arange(4.0), ordinate=np.arange(16.0).reshape(4, 4),
                       response_dof=['1Z+'] * 4,
                       block=['avg 1', 'avg 2'] * 2, **kwargs)
