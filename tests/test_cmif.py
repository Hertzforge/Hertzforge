"""The Complex Mode Indicator Function: the FRF matrix as singular values.

At every frequency line the records assemble into the response x reference
matrix they are and get an SVD; the singular values plot over frequency.
The largest peaks at every mode, and the second peaking too is how a
repeated root shows itself. With a modal model selected beside the FRFs,
the synthesis's CMIF draws dashed over the measurement's, color for color.
"""

from __future__ import annotations

import numpy as np

import hertzforge
from hertzforge.drawing.plot import cmif_curves


def test_the_cmif_is_the_singular_values_of_the_matrix(survey):
    """Checked against a hand-assembled matrix and numpy's own SVD — not
    through cmif_curves' plumbing, so the complex matrix reaching the SVD
    is part of what is checked (an |H| matrix is a plausible wrong CMIF
    that agrees with itself)."""
    _shapes, frfs = survey
    singular, _x = cmif_curves(frfs)
    references = sorted(set(frfs.reference_dof))
    assert singular.shape == (len(references), len(frfs.abscissa))
    assert (np.diff(singular, axis=0) <= 1e-12).all(), 'sorted descending'
    responses = sorted(set(frfs.response_dof))
    lines = [10, 100, 200]
    for line in lines:
        matrix = np.zeros((len(responses), len(references)),
                          dtype=np.complex128)
        for i in range(frfs.num_records):
            matrix[responses.index(frfs.response_dof[i]),
                   references.index(frfs.reference_dof[i])] = (
                frfs.ordinate[i, line])
        expected = np.linalg.svd(matrix, compute_uv=False)
        assert np.allclose(sorted(singular[:, line], reverse=True),
                           expected)


def test_a_line_of_the_frf_not_a_number_has_no_singular_values():
    """The singular values at every line: a line holding an
    entry that is not a number is left out as NaN, and the lines around
    it are decomposed as before, the largest first."""
    from hertzforge.core.data import Frf

    f = np.array([10.0, 20.0, 30.0])
    plant = np.array([[2.0, 0.0], [0.0, 1.0]], dtype=complex)
    rows = [np.full(3, plant[i, j]) for i in range(2) for j in range(2)]
    rows[1] = rows[1].copy()
    rows[1][1] = np.nan
    frf = Frf(f, np.array(rows), response_dof=['1Z+', '1Z+', '2Z+', '2Z+'],
              reference_dof=['1X+', '2X+', '1X+', '2X+'],
              ordinate_dim='acceleration/voltage')
    singular, _x = cmif_curves(frf, unit_system=hertzforge.SI)
    assert np.isnan(singular[:, 1]).all()
    np.testing.assert_allclose(singular[:, [0, 2]], [[2.0, 2.0], [1.0, 1.0]])


def test_a_line_not_a_number_leaves_the_modal_report_its_cmif(project):
    """One FRF entry that is not a number, as one such sample in a
    record makes: the modal report renders, its measured CMIF has a gap
    at that line and only there, and the resynthesis is whole."""
    import json

    from hertzforge.core.report import modal_template
    from hertzforge.report import render_html

    project['FRF'].ordinate[0, 50] = np.nan
    payload = json.loads(render_html(modal_template(project), project).split(
        'type="application/json">')[1].split('</script>')[0])
    cmif = next(block for block in payload['blocks']
                if block.get('caption', '').startswith(
                    'Complex Mode Indicator Function'))
    measured = [c for c in cmif['curves'] if not c.get('dash')]
    synthesized = [c for c in cmif['curves'] if c.get('dash')]
    assert measured and synthesized
    for curve in measured:
        gaps = [k for k, value in enumerate(curve['y']) if value is None]
        assert gaps == [50]
    assert all(value is not None
               for curve in synthesized for value in curve['y'])
