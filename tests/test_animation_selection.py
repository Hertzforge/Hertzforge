"""Which records a deflection animation shows.

An animation gives every node one trajectory, so it can use at most one
record per DOF. Before this, a whole 20-average time object animated the
*sum* of its averages — a waveform nobody measured — a grid subset could
never animate at all (each picked cell became its own series entry and the
animator refuses more than one), and picking cells deselected the geometry.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.data import TimeHistory


def blocked(dofs=('1Z+', '2Z+'), averages=3):
    dof_list, blocks = [], []
    for dof in dofs:
        for average in range(averages):
            dof_list.append(dof)
            blocks.append(f'avg {average + 1}')
    n = len(dof_list)
    return TimeHistory(abscissa=np.arange(4.0),
                       ordinate=np.arange(float(n * 4)).reshape(n, 4),
                       response_dof=dof_list, block=blocks,
                       ordinate_dim='acceleration', ordinate_unit='m/s**2')


def test_the_headless_time_animation_takes_one_quantity():
    """The script's call follows the same rule as the window: the most
    common quantity by default, another by name, and a quantity no
    record measures refused."""
    from hertzforge.deform import first_capture, quantity_records

    mixed = TimeHistory(np.arange(4.0), np.ones((3, 4)),
                        response_dof=['1Z+', '2Z+', '3Z+'],
                        ordinate_dim=['acceleration', 'force',
                                      'acceleration'])
    assert quantity_records(mixed) == ([0, 2], 'acceleration', 1)
    assert quantity_records(mixed, None, 'force') == ([1], 'force', 2)
    # a tie goes to the first record's quantity, the same every run
    assert quantity_records(mixed, [1, 2], None)[1] == 'force'
    assert quantity_records(mixed, None, 'velocity') == ([], 'velocity', 3)
    indices, note = first_capture(blocked(), None)
    assert indices == [0, 3] and 'avg 1 of 3' in note
    assert first_capture(blocked(), [1, 4]) == ([1, 4], '')
