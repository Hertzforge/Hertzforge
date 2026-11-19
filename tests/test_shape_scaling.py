"""Two shape sets overlaid: each is drawn to its own peak, and said so.

Overlaying scales every shape to its own peak. That is what makes a
sparse test set and a dense model swing comparably, and it is the whole
point of the picture — but it also means a reader cannot see that one
set is thirty times the other, which is exactly the thing they would
want to know. A constant factor is a mass unit or a normalization
convention; a factor that changes mode by mode is two different
normalizations. Neither is visible in the scene, so both are said in
words.
"""

from __future__ import annotations

import copy

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge.core.shapes import SCALE_SPREAD, compare_scaling


@pytest.fixture(scope='module')
def modes():
    """The survey's modes, mass-normalized and declared in kg."""
    shapes = hertzforge.import_file(fixture_path('plate', 'shapes.npy'))
    shapes.define_units('kg')
    return shapes


def scaled(shapes, factor):
    other = copy.deepcopy(shapes)
    other.shape_matrix = other.shape_matrix * factor
    return other


def unit_normalized(shapes):
    """Each mode's peak set to 1 — the other common convention, and the
    one that does not compare with mass normalization at all."""
    other = copy.deepcopy(shapes)
    other.shape_matrix = other.shape_matrix / np.abs(
        other.shape_matrix).max(axis=1, keepdims=True)
    return other


def test_a_set_against_itself_is_scaled_alike(modes):
    comparison = compare_scaling(modes, copy.deepcopy(modes))
    assert comparison.factor == pytest.approx(1.0)
    assert comparison.spread == pytest.approx(1.0)
    assert comparison.equal
    assert comparison.message() is None, 'nothing to say'


def test_a_constant_factor_is_reported_as_one(modes):
    """Every mode off by the same amount is a unit or a convention, not
    the structure — and the number is the thing to report."""
    comparison = compare_scaling(modes, scaled(modes, 31.6227766))
    assert comparison.factor == pytest.approx(31.6227766)
    assert comparison.spread == pytest.approx(1.0)
    assert comparison.consistent, 'one scaling, applied throughout'
    assert not comparison.equal
    message = comparison.message('Test', 'FEM')
    assert '31.62' in message
    assert 'constant factor' in message
    assert '31.6' in message, 'and what a thousandfold mass unit looks like'


def test_a_ratio_that_moves_mode_by_mode_is_a_different_normalization(modes):
    comparison = compare_scaling(modes, unit_normalized(modes))
    assert comparison.spread > SCALE_SPREAD
    assert not comparison.consistent
    message = comparison.message('Test', 'FEM')
    assert 'not normalized the same way' in message
    assert 'Unit-normalized' in message
    assert 'mode by mode' in message


def test_the_two_faults_do_not_wear_each_others_words(modes):
    constant = compare_scaling(modes, scaled(modes, 8.0)).message()
    varying = compare_scaling(modes, unit_normalized(modes)).message()
    assert 'constant factor' in constant
    assert 'constant factor' not in varying
    assert 'mode by mode' in varying
    assert 'mode by mode' not in constant


def test_a_sparse_set_and_a_dense_one_compare_on_shared_dofs(modes):
    """Otherwise a set covering a tenth of the DOFs would read as a
    third of the size for no reason but the count."""
    sparse = copy.deepcopy(modes)
    keep = list(range(0, modes.num_dofs, 4))
    sparse.coordinate = [modes.coordinate[i] for i in keep]
    sparse.shape_matrix = modes.shape_matrix[:, keep]
    comparison = compare_scaling(modes, sparse)
    assert comparison.factor == pytest.approx(1.0)
    assert comparison.equal


def test_sets_sharing_no_dofs_say_nothing(modes):
    stranger = copy.deepcopy(modes)
    # node ids far above the plate's 101..1313 — a 9xx prefix collided
    # with the plate's own ninth row of survey nodes
    stranger.coordinate = [f'{90000 + i}X+' for i in range(modes.num_dofs)]
    comparison = compare_scaling(modes, stranger)
    assert not comparison.ratios.size
    assert comparison.message() is None, (
        'no shared DOFs is no comparison, not a fault')


def test_it_reads_the_pairs_it_is_given(modes):
    """The matched pairs, when there are any: mode 3 against mode 5 is
    the comparison someone committed, and comparing 3 with 3 instead
    would measure a different thing."""
    other = scaled(modes, 4.0)
    picked = [(0, 2), (1, 5), (2, 9)]
    comparison = compare_scaling(modes, other, picked)
    assert len(comparison.ratios) == 3
    for k, (row, column) in enumerate(picked):
        wanted = (np.linalg.norm(other.shape_matrix[column])
                  / np.linalg.norm(modes.shape_matrix[row]))
        assert comparison.ratios[k] == pytest.approx(wanted)


def test_different_declared_mass_units_are_named(modes):
    grams = copy.deepcopy(modes).define_units('gram')
    message = compare_scaling(modes, grams).message('Test', 'FEM')
    assert 'kg' in message and 'gram' in message
    assert 'square root of mass' in message


def test_one_declared_and_one_not_says_which_to_define(modes):
    bare = copy.deepcopy(modes).undefine_units()
    message = compare_scaling(modes, bare).message('Test', 'FEM')
    assert 'Define the units on FEM' in message
    other_way = compare_scaling(bare, modes).message('Test', 'FEM')
    assert 'Define the units on Test' in other_way


def test_an_unscaled_set_says_its_size_means_nothing(modes):
    """No drive point, no scale: the shapes are right and their
    magnitudes are arbitrary, so a ratio against them is meaningless
    and the message says that rather than reporting one."""
    unscaled = scaled(modes, 17.0)
    unscaled.unscaled = True
    message = compare_scaling(modes, unscaled).message('Test', 'FEM')
    assert 'unscaled' in message
    assert 'arbitrary scale' in message
    assert '17' not in message, 'a number here would be a false lead'
