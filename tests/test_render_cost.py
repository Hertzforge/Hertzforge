"""Drawing a curve must not cost the size of the object it came from.

A 1356-record FRF took 43 seconds to plot one reference column and half a
second to plot eight cells. Neither number was about drawing: the conversion
to display units ran over every record in the object however few were asked
for, and re-derived the same scale factor from pint each time — 2713 round
trips to draw one curve. Both halves are guarded here.
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.core.data import Frf
from hertzforge.units import IN_LBF_S, parse_dimension, si_transform


@pytest.fixture
def big():
    """Shaped like the large modal set: many records, two dimensions."""
    records = 400
    return Frf(abscissa=np.linspace(0, 128, 64),
               ordinate=np.ones((records, 64), dtype=complex),
               response_dof=[f'{100 + i // 4}Z+' for i in range(records)],
               reference_dof=[f'{210 + i % 4}Z+' for i in range(records)],
               ordinate_dim='acceleration/force',
               ordinate_unit='m/s**2', reference_unit='N')


def test_a_subset_converts_to_the_same_numbers_as_the_whole(big):
    system = IN_LBF_S
    whole = big.display_ordinate(system)
    wanted = [3, 17, 200]
    assert np.allclose(big.display_ordinate(system, wanted), whole[wanted])


class Counting:
    """A unit system that records what it was asked to convert.

    A wrapper rather than a patch: UnitSystem is a frozen dataclass, so its
    methods cannot be replaced on an instance.
    """

    def __init__(self, system):
        self.system = system
        self.calls = []

    def from_si(self, values, dimension):
        self.calls.append((dimension, np.shape(values)[0]))
        return self.system.from_si(values, dimension)

    def __getattr__(self, name):
        return getattr(self.system, name)


def test_converting_a_subset_does_not_touch_the_rest(big):
    """The cost of one curve must not grow with the object holding it."""
    system = Counting(IN_LBF_S)
    big.display_ordinate(system, [5])
    assert [rows for _dimension, rows in system.calls] == [1], \
        f'converted {system.calls} to draw one curve'


def test_one_conversion_per_dimension_not_per_record(big):
    """Records sharing a dimension convert as one block."""
    system = Counting(IN_LBF_S)
    big.display_ordinate(system, range(big.num_records))
    assert system.calls == [('acceleration/force', big.num_records)], \
        f'{len(system.calls)} conversions for {big.num_records} records'


def test_the_unit_lookups_are_memoized():
    """They are pure functions of short strings, and the plot asks for the
    same handful once per record."""
    assert hasattr(si_transform, 'cache_info')
    si_transform('mm', 'length')
    before = si_transform.cache_info()
    si_transform('mm', 'length')
    assert si_transform.cache_info().hits > before.hits

    parse_dimension('acceleration**2/frequency')
    from hertzforge.units import _parse_dimension
    before = _parse_dimension.cache_info()
    parse_dimension('acceleration**2/frequency')
    assert _parse_dimension.cache_info().hits > before.hits


def test_parse_dimension_still_returns_a_fresh_list():
    """It is memoized behind a tuple; handing out the cached object would let
    a caller mutate every future answer."""
    first = parse_dimension('acceleration/force')
    first.append(('bogus', 1))
    assert parse_dimension('acceleration/force') == [('acceleration', 1),
                                                     ('force', -1)]
