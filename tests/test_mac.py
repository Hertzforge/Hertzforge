"""The Modal Assurance Criterion, as numbers and as the grid beside a
mode table.

MAC_ij = |phi_i^H psi_j|^2 / ((phi_i^H phi_i)(psi_j^H psi_j)): 1 for the
same shape in any scaling, near 0 for independent ones. The auto-MAC's
off-diagonals say how distinct a set's own modes are, which belongs
beside the list of them — when reading a shape set, and while fitting one.
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge.core.shapes import mac_matrix


def test_mac_is_one_for_the_same_shape_in_any_scaling():
    a = np.array([1.0, -2.0, 3.0, 0.5])
    matrix = mac_matrix(np.vstack([a, -2.5 * a]))
    assert np.allclose(matrix, 1.0)


def test_mac_is_zero_for_orthogonal_shapes():
    matrix = mac_matrix(np.array([[1.0, 0.0], [0.0, 1.0]]))
    assert np.allclose(matrix, np.eye(2))


def test_the_plate_auto_mac_is_nearly_identity():
    """Elastic modes only: the six rigid vectors are an arbitrary basis
    of the null space — the solver may hand back any mixture of them,
    and the MAC between two members of an arbitrary basis is not a
    statement about anything."""
    shapes = hertzforge.import_file(fixture_path('plate', 'shapes.npy'))
    elastic = np.flatnonzero(shapes.frequency > 0.0)
    matrix = shapes.auto_mac()[np.ix_(elastic, elastic)]
    assert np.allclose(np.diag(matrix), 1.0)
    off = matrix - np.diag(np.diag(matrix))
    assert off.max() < 0.6, 'mass-normalized modes are nearly independent'


def test_cross_mac_aligns_shared_dofs_whatever_their_order():
    """The sets need not cover the same DOFs or order them the same way;
    a positional comparison would scramble everything."""
    from hertzforge.core.shapes import ShapeSet, cross_mac

    a = ShapeSet(frequency=[10.0, 20.0], damping=[0.01, 0.01],
                 coordinate=['1X+', '2X+', '3X+'],
                 shape_matrix=[[1.0, 2.0, 3.0], [3.0, -1.0, 0.5]])
    # the same two shapes, DOFs reversed and one extra the first lacks
    b = ShapeSet(frequency=[10.5, 19.5], damping=[0.01, 0.01],
                 coordinate=['3X+', '9X+', '2X+', '1X+'],
                 shape_matrix=[[3.0, 7.0, 2.0, 1.0],
                               [0.5, -4.0, -1.0, 3.0]])
    matrix = cross_mac(a, b)
    assert matrix.shape == (2, 2)
    assert np.allclose(np.diag(matrix), 1.0), 'same shapes, aligned'
    assert matrix[0, 1] < 0.5 and matrix[1, 0] < 0.5


def test_cross_mac_refuses_disjoint_sets():
    from hertzforge.core.shapes import ShapeSet, cross_mac

    a = ShapeSet([10.0], [0.01], ['1X+'], [[1.0]])
    b = ShapeSet([10.0], [0.01], ['2X+'], [[1.0]])
    with pytest.raises(ValueError, match='share no DOFs'):
        cross_mac(a, b)
