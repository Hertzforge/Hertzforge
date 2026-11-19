"""Two shape sets on one geometry: the cross-MAC drives an overlay.

Click a MAC cell and its two modes animate over each other on the
geometry, the second phase-aligned to the first so an arbitrary sign or
rotation cannot make matching shapes move against each other.
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.core.shapes import ShapeSet, aligned_mode


def test_a_flipped_shape_aligns_back(  ):
    a = ShapeSet([10.0], [0.01], ['1X+', '2X+', '3X+'],
                 [[1.0, -2.0, 0.5]])
    b = ShapeSet([10.2], [0.01], ['1X+', '2X+', '3X+'],
                 [[-1.1, 2.2, -0.55]])
    aligned = aligned_mode(a, 0, b, 0)
    assert np.isrealobj(aligned)
    assert np.vdot(a.shape_matrix[0], aligned) > 0, 'moving together now'
    assert np.allclose(aligned, -b.shape_matrix[0])


def test_a_rotated_complex_shape_aligns_back():
    base = np.array([1.0 + 0.2j, -0.5 + 0.1j, 0.3 - 0.4j])
    a = ShapeSet([10.0], [0.01], ['1X+', '2X+', '3X+'], [base])
    b = ShapeSet([10.0], [0.01], ['1X+', '2X+', '3X+'],
                 [base * np.exp(1j * 2.1)])
    aligned = aligned_mode(a, 0, b, 0)
    inner = np.vdot(a.shape_matrix[0], aligned)
    assert abs(np.angle(inner)) < 1e-9, 'the rotation is undone'


def test_alignment_factor_applies_to_a_third_vector():
    """Measured between a set and its projection, applied to the dense
    original: a sign flip in the projection flips the raw set too."""
    from hertzforge.core.shapes import alignment_factor, apply_alignment

    a = ShapeSet([10.0], [0.01], ['1X+', '2X+'], [[1.0, 2.0]])
    projected = ShapeSet([10.0], [0.01], ['1X+', '2X+'],
                         [[-1.0, -2.0]])
    factor = alignment_factor(a, 0, projected, 0)
    dense = np.array([3.0, -1.0, 0.5])
    flipped = apply_alignment(dense, factor)
    assert np.isrealobj(flipped)
    assert np.allclose(flipped, -dense)


def test_disjoint_sets_are_left_alone():
    a = ShapeSet([10.0], [0.01], ['1X+'], [[1.0]])
    b = ShapeSet([10.0], [0.01], ['9X+'], [[-2.0]])
    assert np.allclose(aligned_mode(a, 0, b, 0), b.shape_matrix[0])


def test_alignment_phases_are_the_factors_of_every_pair():
    """One matrix product gives what `alignment_factor` gives pair by
    pair: on shared DOFs in either order, for real and complex modes, with
    a zero inner product left unrotated."""
    from hertzforge.core.shapes import alignment_factor, alignment_phases

    a = ShapeSet([10.0, 20.0, 30.0], [0.01, 0.01, 0.01], ['1X+', '2X+', '3X+'],
                 [[1.0, 2.0, 0.5], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]])
    b = ShapeSet([11.0, 19.0], [0.02, 0.02], ['3X+', '9X+', '1X+'],
                 [[0.5, 7.0, -1.0], [2j, 7.0, 1.0 + 1j]])
    phases = alignment_phases(a, [0, 2, 1], b, [1, 0])
    assert phases.shape == (3, 2)
    for r, i in enumerate([0, 2, 1]):
        for c, j in enumerate([1, 0]):
            assert phases[r, c] == pytest.approx(np.angle(alignment_factor(a, i, b, j)))
    zero = ShapeSet([5.0], [0.01], ['1X+', '3X+'], [[0.0, 0.0]])
    assert alignment_phases(a, [0], zero, [0]) == pytest.approx(0.0)
    disjoint = ShapeSet([5.0], [0.01], ['9X+'], [[1.0]])
    assert np.array_equal(alignment_phases(a, [0, 1], disjoint, [0]), np.zeros((2, 1)))
    assert alignment_phases(a, [], b, [0]).shape == (0, 1)


def test_a_large_set_pairs_in_seconds():
    """The Frame-Wing's 393,510 DOFs hung the service for good in the auto-
    MAC's overlay phases (2026-11-05): `list.index` per DOF per pair. The
    lookup is linear now, so twelve modes over 400,000 DOFs pair at once."""
    import time

    from hertzforge.core.shapes import alignment_phases, cross_mac

    n = 400_000
    dofs = [f'{k // 3 + 1}{"XYZ"[k % 3]}+' for k in range(n)]
    rng = np.random.default_rng(1)
    a = ShapeSet(np.arange(1.0, 13.0), np.full(12, 0.01), dofs, rng.standard_normal((12, n)))
    started = time.perf_counter()
    phases = alignment_phases(a, range(12), a, range(12))
    matrix = cross_mac(a, a)
    assert time.perf_counter() - started < 10.0
    assert phases.shape == (12, 12) and np.allclose(np.diag(phases), 0.0)
    assert np.allclose(np.diag(matrix), 1.0)
