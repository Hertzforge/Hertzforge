"""Ordinary coherence and multiple coherence are different objects.

UFF dataset 58 gives them different function types — 6 and 26 — and sdynpy
has a class for each. hertzforge had one class declaring 6 while holding what
Rattlesnake reports, which is multiple coherence: one curve per response,
references summed over. Exported, that claimed a reference DOF per record
that was never there.
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge.core.data import Coherence, MultipleCoherence


def ordinary():
    return Coherence(abscissa=np.linspace(0, 10, 5),
                     ordinate=np.full((2, 5), 0.8),
                     response_dof=['1Z+', '1Z+'],
                     reference_dof=['3Z+', '4Z+'])


def multiple():
    return MultipleCoherence(abscissa=np.linspace(0, 10, 5),
                             ordinate=np.full((2, 5), 0.9),
                             response_dof=['1Z+', '2Z+'])


def test_they_carry_the_function_types_the_standard_gives_them():
    assert Coherence.function_type == 6
    assert MultipleCoherence.function_type == 26


def test_ordinary_coherence_is_meaningless_without_a_reference():
    """It is one response against one reference; there is no such thing as
    an ordinary coherence with nothing to be coherent with."""
    with pytest.raises(ValueError, match='requires reference_dof'):
        Coherence(abscissa=np.arange(3.0), ordinate=np.ones((1, 3)),
                  response_dof=['1Z+'])


def test_multiple_coherence_has_no_reference_of_its_own():
    """The references are summed over, which is the whole point of it."""
    assert multiple().reference_dof is None
    assert MultipleCoherence.needs_reference is False


@pytest.mark.parametrize('build', [ordinary, multiple], ids=['ordinary', 'multiple'])
def test_either_kind_needs_no_unit_declared(build):
    """A ratio of spectra is dimensionless by construction."""
    data = build()
    assert data.units_defined
    assert data.undefined_records == []
    assert set(data.ordinate_dim) == {'dimensionless'}


@pytest.mark.parametrize('build', [ordinary, multiple], ids=['ordinary', 'multiple'])
def test_either_kind_plots_linear_against_its_own_bounds(build):
    data = build()
    assert type(data).log_ordinate is False
    assert type(data).ordinate_limits == (0.0, 1.05)


@pytest.mark.parametrize('build,expected', [(ordinary, Coherence),
                                            (multiple, MultipleCoherence)],
                         ids=['ordinary', 'multiple'])
def test_a_unv_round_trip_comes_back_as_the_same_kind(build, expected, tmp_path):
    """The point of the split. Written as type 6, a multiple coherence comes
    back promising a reference DOF that does not exist."""
    source = build()
    path = str(tmp_path / 'c.unv')
    hertzforge.export_file(source, path)
    back = hertzforge.import_file(path)
    assert isinstance(back, expected)
    assert np.allclose(back.ordinate.real, source.ordinate.real)
    assert back.reference_dof == source.reference_dof


def test_rattlesnake_reports_multiple_coherence():
    """One curve per response channel, against all the shakers at once."""
    out = hertzforge.import_file(fixture_path('plate', 'modal_spectra.nc4'))
    coherence = out['Modal_coherence']
    assert isinstance(coherence, MultipleCoherence)
    assert not isinstance(coherence, Coherence), 'not the ordinary kind'
    assert coherence.reference_dof is None
