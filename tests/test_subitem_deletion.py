"""Deleting the thing that was selected, not the thing that contains it.

Sub-items — records, modes, channels — used to be undeletable: picking one
and pressing Delete walked up to the owner and deleted the whole object. Now
the selection is what goes: records from the tree or a grid, channels and
modes as whole rows of their tables, whole objects only when the object
itself is picked.
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge.core.data import TimeHistory


def test_delete_records_takes_everything_a_record_owns():
    data = TimeHistory(abscissa=np.arange(4.0),
                       ordinate=np.arange(12.0).reshape(3, 4),
                       response_dof=['1Z+', '2Z+', '3Z+'],
                       block=['a', 'b', 'c'],
                       ordinate_dim='acceleration', ordinate_unit='m/s**2',
                       comment=['one', 'two', 'three'])
    data.delete_records([1])
    assert data.response_dof == ['1Z+', '3Z+']
    assert data.block == ['a', 'c']
    assert data.comment == ['one', 'three']
    assert np.allclose(data.ordinate[:, 0].real, [0.0, 8.0])


def test_a_specification_keeps_its_limits_aligned():
    """The limit curves ride per record; deleting a record without its rows
    would bound the wrong channels."""
    spec = hertzforge.import_file(fixture_path('plate',
                                         'random_spectra.nc4'))['Random_specification']
    # the fixture's limit curves are identical across channels, so value
    # comparison alone proves nothing — tag each row before deleting
    for name in spec.limits:
        spec.limits[name] = spec.limits[name] * np.arange(
            1.0, spec.num_records + 1)[:, None]
    kept_dof = spec.response_dof[2]
    kept_limit = spec.limits['abort_upper'][2].copy()
    spec.delete_records([0, 1])
    assert spec.response_dof[0] == kept_dof
    for name, values in spec.limits.items():
        assert values.shape[0] == spec.num_records, name
    assert np.allclose(spec.limits['abort_upper'][0], kept_limit,
                       equal_nan=True)


@pytest.mark.parametrize('path,attribute,method', [
    ('shapes.npy', 'num_shapes', 'delete_modes'),
    ('modal_spectra.nc4', 'num_channels', 'delete_channels'),
])
def test_the_last_sub_item_is_refused(path, attribute, method):
    """An empty object is not a state anything else can show; delete the
    object instead."""
    out = hertzforge.import_file(fixture_path('plate', path))
    obj = out['channel_table'] if isinstance(out, dict) else out
    with pytest.raises(ValueError, match='delete the'):
        getattr(obj, method)(range(getattr(obj, attribute)))
