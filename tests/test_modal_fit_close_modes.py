"""Fitting modes whose half-power widths overlap, and fits whose
references are not all measured as responses.

The plant is synthetic: seven modes at 1 % damping, three of them close
enough that their half-power widths overlap, over twelve response-only
DOFs and three drive points. The FRFs are its exact accelerance, so the
fit is judged against the modal model that made them.
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.data import Frf
from hertzforge.core.modal_fit import ModalFitSession
from hertzforge.core.shapes import ShapeSet, cross_mac, shared_columns

#: 150 and 2800 Hz lie outside the fitted band and leave their tails in it
FREQUENCIES = (150.0, 418.1, 1125.4, 1190.7, 1216.0, 1950.5, 2800.0)
CLOSE = (1125.4, 1190.7, 1216.0)
ALONE = (418.1, 1950.5)
RESPONSE_ONLY = [f'{100 + n}Z+' for n in range(1, 13)]
DRIVES = ['201Z+', '202Z+', '203Z+']
BOUNDS = (300.0, 2400.0)


def _plant():
    """(truth, FRF): every DOF against every drive, at 1 Hz."""
    rng = np.random.default_rng(3)
    dofs = RESPONSE_ONLY + DRIVES
    truth = ShapeSet(FREQUENCIES, np.full(len(FREQUENCIES), 0.01), dofs,
                     rng.standard_normal((len(FREQUENCIES), len(dofs))))
    lines = np.arange(200.0, 3001.0, 1.0)
    responses = [dof for dof in dofs for _ in DRIVES]
    references = [ref for _ in dofs for ref in DRIVES]
    rows = truth.synthesize_frf(lines, responses, references, power=2)
    return truth, Frf(lines, rows, response_dof=responses, reference_dof=references)


def _against(fit, truth, frequency):
    """(fitted mode, truth's amplitude over the fit's, MAC) for the
    fitted mode nearest `frequency`, on the DOFs both sets hold."""
    i = int(np.argmin(np.abs(fit.frequency - frequency)))
    j = int(np.argmin(np.abs(truth.frequency - frequency)))
    ours, theirs, signs = shared_columns(fit, truth)
    a, b = fit.shape_matrix[i, ours], truth.shape_matrix[j, theirs] * signs
    return i, abs(float(b @ a)) / float(a @ a), float(a @ b) ** 2 / float((a @ a) * (b @ b))


def _project():
    truth, frf = _plant()
    project = hertzforge.Project('close modes')
    project.add('FRF', frf)
    project.add('Truth', truth)
    return project, truth


def test_refine_finds_the_poles_of_overlapping_modes():
    """Each mode's pole is searched alone on the residual, leashed to
    1.5 lines and kept out of every confirmed mode's half-power width, so
    the poles of modes whose widths overlap come out biased, and their
    amplitudes with them. Refine searches the poles of overlapping modes
    together, and the cluster comes back on the truth while the modes
    that stand alone are left as they were."""
    project, truth = _project()
    sequential = project[project.fit_modes('FRF', bounds=BOUNDS, limit=10, name='Sequential')]
    worst = max(abs(sequential.damping[_against(sequential, truth, f)[0]] - 0.01) for f in CLOSE)
    assert worst > 0.002, 'the one-at-a-time poles are visibly off, or this checks nothing'

    fit = project[project.fit_modes('FRF', bounds=BOUNDS, limit=10, refine=3)]
    for frequency in CLOSE:
        i, amplitude, mac = _against(fit, truth, frequency)
        assert fit.frequency[i] == pytest.approx(frequency, abs=0.1)
        assert fit.damping[i] == pytest.approx(0.01, abs=0.001), f'{frequency} Hz'
        assert amplitude == pytest.approx(1.0, abs=0.05), f'{frequency} Hz'
        assert mac > 0.999
    for frequency in ALONE:
        i, amplitude, _mac = _against(fit, truth, frequency)
        k, before, _ = _against(sequential, truth, frequency)
        assert fit.frequency[i] == sequential.frequency[k], 'a mode alone keeps its pole'
        assert fit.damping[i] == sequential.damping[k]
        assert abs(amplitude - 1.0) <= abs(before - 1.0) + 1e-9


def test_the_joint_pole_search_holds_what_it_cannot_move():
    """One channel buried in noise, which its coherence says to distrust:
    the weighted loop confirms the close modes and, around them, modes
    that are only noise, some at the least damping a fit allows and
    within a line of each other, some at half of critical. Those are
    held at their poles while the real modes of their cluster are
    searched, so the noise does not cost the cluster its refinement."""
    from hertzforge.core.data import MultipleCoherence

    truth, frf = _plant()
    rng = np.random.default_rng(11)
    noisy = frf.ordinate.copy()
    bad = [i for i, dof in enumerate(frf.response_dof) if dof == '104Z+']
    level = np.median(np.abs(noisy[bad]))
    noisy[bad] += level * (rng.standard_normal(noisy[bad].shape)
                                 + 1j * rng.standard_normal(noisy[bad].shape))
    frf = Frf(frf.abscissa, noisy, response_dof=frf.response_dof,
              reference_dof=frf.reference_dof)
    dofs = RESPONSE_ONLY + DRIVES
    gamma = np.full((len(dofs), len(frf.abscissa)), 0.98)
    gamma[dofs.index('104Z+')] = 0.05
    coherence = MultipleCoherence(frf.abscissa, gamma, response_dof=dofs)
    project = hertzforge.Project('weighted')
    project.add('FRF', frf)
    fit = project[project.fit_modes('FRF', bounds=BOUNDS, limit=10, refine=3,
                                    coherence=coherence)]
    for frequency in CLOSE:
        i, _amplitude, _mac = _against(fit, truth, frequency)
        assert fit.frequency[i] == pytest.approx(frequency, abs=0.2)
        assert fit.damping[i] == pytest.approx(0.01, abs=0.001), f'{frequency} Hz'


def test_a_refined_pole_keeps_its_place_in_the_replay():
    """Refine moves the poles of overlapping modes, and the replay list
    holds the poles they were confirmed at: taking a refined mode back,
    typing its damping and replaying the fit must still find its pick."""
    from hertzforge.service.fitting import delete, edit

    _truth, frf = _plant()
    session = ModalFitSession(frf)
    session.suggest(BOUNDS)
    for count in range(5):
        session.confirm(description=f'mode {count + 1}')
        session.suggest(BOUNDS, modes_only=True)
    picks = list(session.confirmed)
    session.refine_residues()
    moved = [k for k, mode in enumerate(session.modes)
             if (mode['frequency'], mode['damping']) not in picks]
    assert moved, 'Refine moved a pole, or this checks nothing'
    k = moved[0]
    replay = session.replay()
    assert {pick[2] for pick in replay['at'] if len(pick) == 3} == {
        f'mode {n}' for n in range(1, 6)}, 'every description rides its pick'

    edit(session, {'mode': k, 'damping': 0.02})
    assert session.modes[k]['damping'] == 0.02
    assert len(session.confirmed) == 5
    assert sum(pick[1] == 0.02 for pick in session.confirmed) == 1

    delete(session, [k])
    assert len(session.confirmed) == 4, 'the refined mode left the replay list too'
    assert all(pick[1] != 0.02 for pick in session.confirmed)


def test_reference_only_dofs_stay_out_of_an_unscaled_mac():
    """Fitted without a drive point, a set's reference DOFs hold the
    references' participation, on a scale of its own: the products of
    response and reference values are measured, their split is not. A
    MAC over the response DOFs and those together reads a good fit as a
    poor one, so the MAC leaves them out of an unscaled set."""
    project, truth = _project()
    frf = project['FRF']
    rows = [i for i, dof in enumerate(frf.response_dof) if dof in RESPONSE_ONLY]
    name = project.fit_modes('FRF', bounds=(300.0, 1000.0), limit=1, records=rows)
    fit = project[name]
    assert fit.unscaled
    assert fit.reference_only == DRIVES
    assert set(fit.coordinate) == set(RESPONSE_ONLY + DRIVES), (
        'the participation is kept, for synthesis and Edit Fit')

    mac = project.comparison_mac(name, 'Truth')
    assert mac[0, FREQUENCIES.index(418.1)] > 0.999
    together = ShapeSet(fit.frequency, fit.damping, fit.coordinate, fit.shape_matrix)
    assert cross_mac(together, truth)[0, 1] < 0.95, (
        'with the participation in, the MAC is visibly wrong, or this checks nothing')


def test_a_scaled_fit_keeps_its_reference_dofs_in_the_mac():
    """With a drive point the scale is measured, and the value at a
    reference-only DOF is the mode shape there."""
    project, truth = _project()
    frf = project['FRF']
    rows = [i for i, dof in enumerate(frf.response_dof) if dof in RESPONSE_ONLY + ['201Z+']]
    fit = project[project.fit_modes('FRF', bounds=(300.0, 1000.0), limit=1, records=rows)]
    assert not fit.unscaled
    assert fit.reference_only == ['202Z+', '203Z+']
    j = FREQUENCIES.index(418.1)
    sign = np.sign(fit.shape_matrix[0, 0] * truth.shape_matrix[j, 0])
    for dof in fit.reference_only:
        assert sign * fit.shape_matrix[0, fit.coordinate.index(dof)] == pytest.approx(
            truth.shape_matrix[j, truth.coordinate.index(dof)], rel=0.01)
    assert cross_mac(fit, truth)[0, j] > 0.9999


def test_a_mac_on_nothing_but_participation_is_refused():
    fit = ShapeSet([10.0], [0.01], ['1Z+', '2Z+'], [[1.0, 2.0]],
                   unscaled=True, reference_only=['2Z+'])
    other = ShapeSet([10.0], [0.01], ['2Z+', '3Z+'], [[1.0, 1.0]])
    with pytest.raises(ValueError, match='reference'):
        cross_mac(fit, other)


def test_reference_only_rides_the_project_file(tmp_path):
    project = hertzforge.Project('file')
    project.add('Fit', ShapeSet([10.0], [0.01], ['1Z+', '2Z-'], [[1.0, 2.0]],
                                unscaled=True, reference_only=['2Z-']))
    project.add('Plain', ShapeSet([10.0], [0.01], ['1Z+'], [[1.0]]))
    project.save(tmp_path / 'fit.hforge')
    back = hertzforge.Project.open(tmp_path / 'fit.hforge')
    assert back['Fit'].reference_only == ['2Z-']
    assert back['Plain'].reference_only == []
