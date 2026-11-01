"""Two shape sets on one geometry: the cross-MAC drives an overlay.

Click a MAC cell and its two modes animate over each other on the
geometry, the second phase-aligned to the first so an arbitrary sign or
rotation cannot make matching shapes move against each other.
"""

from __future__ import annotations

import numpy as np

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
