"""Convert to PSD/CPSD on a specification written at breakpoints: the
frequency spacing is chosen there, and the result is on lines.

Brandon, 2026-10-30: a specification at breakpoints converted to a CPSD
kept its breakpoints, and Compute Modal Responses answered it at those
frequencies alone. A density to drive or predict with is read line by
line, so the conversion reads the power law onto lines, with the
sheet's own reading (`to_lines`), at a spacing the user sets; it is
never guessed. A specification already on lines converts as it did.
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.data import Psd, Specification

BREAKS = np.array([20.0, 100.0, 1000.0])

CHANNELS = ['1X+', '3X+']

COHERENCE, PHASE = 0.5, 30.0


def _breakpoints(varying=False):
    """Two force channels at three breakpoints, one constant coherence
    and phase between them (a cross term that varies with `varying`)."""
    a = np.array([1e-3, 4e-3, 4e-3])
    b = np.array([2e-3, 2e-3, 5e-4])
    phase = np.radians(PHASE) * (np.arange(3) + 1 if varying else np.ones(3))
    cross = np.sqrt(COHERENCE * a * b) * np.exp(1j * phase)
    return Specification(
        BREAKS, np.array([a, cross, np.conj(cross), b]),
        response_dof=['1X+', '1X+', '3X+', '3X+'],
        reference_dof=['1X+', '3X+', '1X+', '3X+'],
        ordinate_dim=['force**2/frequency', 'force*force/frequency',
                      'force*force/frequency', 'force**2/frequency'])


def _project(spec=None):
    project = hertzforge.Project()
    project['Spec'] = _breakpoints() if spec is None else spec
    project.journal.clear()
    return project


def _record(psd, response, reference):
    return next(i for i in range(psd.num_records)
                if psd.response_dof[i] == response
                and psd.reference_dof[i] == reference)


def test_a_breakpoint_specification_converts_onto_lines_at_the_spacing():
    project = _project()
    assert project['Spec'].interpolation == 'log_log'
    project.convert_to_psd('Spec', spacing=5.0)
    psd = project['Spec']
    assert type(psd) is Psd and psd.interpolation == 'bin'
    np.testing.assert_allclose(psd.abscissa, np.arange(20.0, 1000.1, 5.0))
    assert project.journal[-1] == "project.convert_to_psd('Spec', spacing=5.0)"
    # the power law between 20 and 100 Hz, read at 50 Hz
    at = int(np.flatnonzero(psd.abscissa == 50.0)[0])
    slope = np.log(4.0) / np.log(5.0)
    auto = np.real(psd.ordinate[_record(psd, '1X+', '1X+'), at])
    assert auto == pytest.approx(1e-3 * (50.0 / 20.0) ** slope, rel=1e-9)
    # the cross term carried as one coherence and phase at every line
    s11 = np.real(psd.ordinate[_record(psd, '1X+', '1X+')])
    s22 = np.real(psd.ordinate[_record(psd, '3X+', '3X+')])
    s12 = psd.ordinate[_record(psd, '1X+', '3X+')]
    np.testing.assert_allclose(np.abs(s12) ** 2 / (s11 * s22), COHERENCE,
                               rtol=1e-6)
    np.testing.assert_allclose(np.degrees(np.angle(s12)), PHASE, atol=1e-6)


def test_the_spacing_is_never_guessed_and_the_sheets_is_the_default():
    project = _project()
    assert project.remembered_spacing('Spec') is None
    with pytest.raises(ValueError, match='breakpoints.*spacing'):
        project.convert_to_psd('Spec')
    assert project['Spec'].interpolation == 'log_log', 'nothing changed'
    # the sheet keeps its draft, spacing included, in the provenance
    project.provenance['Spec'] = {
        'verb': 'author_specification', 'source': 'Table',
        'params': {'draft': {'spacing': 10.0}}, 'state': None}
    assert project.remembered_spacing('Spec') == 10.0
    project.convert_to_psd('Spec')
    assert np.diff(project['Spec'].abscissa) == pytest.approx(10.0)


def test_a_specification_on_lines_converts_as_it_is():
    lines = np.arange(20.0, 101.0, 5.0)
    spec = Specification(lines, np.ones((1, lines.size)) * 1e-3,
                         response_dof=['1X+'],
                         ordinate_dim='force**2/frequency')
    spec.interpolation = 'bin'
    project = _project(spec)
    with pytest.raises(ValueError, match='already on lines'):
        project.convert_to_psd('Spec', spacing=5.0)
    project.convert_to_psd('Spec')
    assert type(project['Spec']) is Psd
    np.testing.assert_array_equal(project['Spec'].abscissa, lines)


def test_a_cross_term_that_varies_is_refused_rather_than_lost():
    project = _project(_breakpoints(varying=True))
    with pytest.raises(ValueError, match='vary with frequency.*1X\\+–3X\\+'):
        project.convert_to_psd('Spec', spacing=5.0)


def test_converted_it_drives_compute_modal_responses():
    """The reason for the spacing: the result answers line by line."""
    from scipy.linalg import eigh

    from hertzforge.core.shapes import ShapeSet

    m = np.diag([2.0, 1.0, 1.5])
    k = 1e6 * np.array([[2.0, -1.0, 0.0], [-1.0, 2.0, -1.0], [0.0, -1.0, 1.0]])
    lam, phi = eigh(k, m)
    project = _project()
    project['Shapes'] = ShapeSet(np.sqrt(lam) / (2 * np.pi), [0.02] * 3,
                                 ['1X+', '2X+', '3X+'], phi.T, mass_unit='kg')
    with pytest.raises(ValueError, match='breakpoints'):
        project.compute_modal_responses('Spec', 'Shapes')
    project.convert_to_psd('Spec', spacing=5.0)
    made = project.compute_modal_responses('Spec', 'Shapes')
    assert len(project[made].abscissa) == len(project['Spec'].abscissa) > 100
