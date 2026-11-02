"""FRFs re-synthesized from a modal model, overlaid on the measurement.

Selecting a shape set (or picked modes in its grid) beside measured FRFs
asks the modal model to predict them: the residue form
H_jk = sum_r phi_jr phi_kr / (m_r (w_r^2 - w^2 + 2i z_r w_r w)),
scaled to each measurement's quantity by powers of iw. The survey
fixtures were generated from exactly this modal model, so the full-mode
synthesis must reproduce the measured FRFs to machine precision.

**That is a consistency check, not a verification.** The fixture FRFs
were made by this same function (`testdata/generate_plate.py`), so a
convention error shared by the two would pass here at machine
precision. What verifies the synthesis is elsewhere: exact physics in
`test_frf_synthesis_exact.py` and sdynpy's own synthesis in
`test_modal_frf_oracle.py`. This file is kept for what only it covers
— signed DOFs, mode subsets, rigid modes at 0 Hz and the overlay.
"""

from __future__ import annotations

import numpy as np
import pytest


def test_full_synthesis_reproduces_the_fixture_frfs(survey):
    shapes, frfs = survey
    receptance = shapes.synthesize_frf(
        frfs.abscissa, frfs.response_dof[:5], frfs.reference_dof[:5])
    accelerance = -(2.0 * np.pi * frfs.abscissa) ** 2 * receptance
    for row in range(5):
        measured = frfs.ordinate[row]
        scale = np.abs(measured).max()
        assert np.abs(accelerance[row] - measured).max() < 1e-9 * scale


def test_a_signed_dof_flips_the_shape(survey):
    shapes, frfs = survey
    plus = shapes.synthesize_frf(frfs.abscissa, ['101X+'], ['101Z+'])
    minus = shapes.synthesize_frf(frfs.abscissa, ['101X-'], ['101Z+'])
    assert np.allclose(minus, -plus)


def test_a_mode_subset_synthesizes_only_those_modes(survey):
    shapes, frfs = survey
    # an out-of-plane pair: the plate's in-plane DOFs answer at the
    # level of round-off, where any truncation is invisible
    full = shapes.synthesize_frf(frfs.abscissa, ['107Z+'], ['101Z+'])
    truncated = shapes.synthesize_frf(frfs.abscissa, ['107Z+'], ['101Z+'],
                                      modes=range(10))
    assert not np.allclose(truncated, full), 'truncation must show'
    rest = shapes.synthesize_frf(frfs.abscissa, ['107Z+'], ['101Z+'],
                                 modes=range(10, shapes.num_shapes))
    assert np.allclose(truncated + rest, full), 'modes sum independently'


def test_rigid_body_modes_at_zero_hertz_raise_no_warnings(survey):
    """The user's data starts at 0 Hz and the model has rigid-body modes:
    that bin was 0/0, and every render printed a screenful of
    RuntimeWarnings. The power applies inside the sum now, where the
    rigid accelerance at DC cancels exactly to the residue."""
    import warnings

    shapes, frfs = survey
    freq = np.concatenate([[0.0], frfs.abscissa])
    assert (shapes.frequency == 0.0).any(), 'the fixture has rigid modes'
    with warnings.catch_warnings():
        warnings.simplefilter('error')
        accelerance = shapes.synthesize_frf(freq, ['101X+'], ['101Z+'],
                                            power=2)
        receptance = shapes.synthesize_frf(freq, ['101X+'], ['101Z+'])
    rigid = shapes.frequency == 0.0
    lookup = {c: i for i, c in enumerate(shapes.coordinate)}
    expected_dc = np.sum(
        shapes.shape_matrix[rigid, lookup['101X+']]
        * shapes.shape_matrix[rigid, lookup['101Z+']]
        / shapes.modal_mass[rigid])
    assert np.isfinite(accelerance[0, 0])
    assert accelerance[0, 0] == pytest.approx(expected_dc)
    assert not np.isfinite(receptance[0, 0]), (
        'a free body has no static deflection to report')


def test_the_power_matches_the_external_multiply_off_dc(survey):
    shapes, frfs = survey
    receptance = shapes.synthesize_frf(frfs.abscissa, ['101X+'], ['101Z+'])
    accelerance = shapes.synthesize_frf(frfs.abscissa, ['101X+'], ['101Z+'],
                                        power=2)
    outside = -(2.0 * np.pi * frfs.abscissa) ** 2 * receptance
    assert np.allclose(accelerance, outside)


def test_a_dof_outside_the_shapes_is_refused(survey):
    shapes, frfs = survey
    assert not shapes.covers('99999X+')
    with pytest.raises(ValueError, match='99999X'):
        shapes.synthesize_frf(frfs.abscissa, ['99999X+'], ['101Z+'])
