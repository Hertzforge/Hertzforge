"""A converted Modal Test run's FRFs and coherence go to the modal fit unchanged
(`fit_refusal`, `ModalFitSession`, `fit.start`), and the fit recovers the structure the
run measured: three modes over six points (`run_folders.modal_test_run`), measured with
one reference, and with the hammer roving and the set turned round by reciprocity. A fit
of FRFs estimated with an exponential window notes on each mode the damping the window
added and the mode less it.

Tolerances: each natural frequency within 0.5 % and each damping ratio within 10 % of the
structure's; the errors measured are asserted far tighter (`ACHIEVED`), so a fit that
drifts is seen long before it leaves the brief's bounds."""

from __future__ import annotations

import numpy as np
import pytest
import run_folders as rf

from hertzforge import Project
from hertzforge.core.modal_fit import ModalFitSession, fit_refusal
from hertzforge.core.modal_test import is_transposed, window_corrected
from hertzforge.io import run_convert
from hertzforge.service import fitting

FREQUENCY, DAMPING = 0.005, 0.10
#: the largest relative errors measured on this branch, with margin: frequency, damping
ACHIEVED = (1e-4, 1e-2)
#: the same, of the damping less an exponential window: taking the window's share out of
#: the fitted damping leaves the fit's own error on a smaller number
LESS_WINDOW = (1e-4, 2e-2)


def _project(tmp_path, layout, **kwargs):
    folder = rf.modal_test_run(str(tmp_path / layout), layout, **kwargs)
    return Project.open(run_convert.convert(folder).path)


def _fit(project, frf):
    """The fit as the fitting screen runs it: find, confirm, three times; publish."""
    fit = fitting.start(project, {'frf': frf})
    assert fit.coherence is not None, 'the run\'s coherence weights the fit'
    for _ in range(3):
        fitting.act(fit, 'fit.find', {'low': 5.0, 'high': 140.0})
        fitting.act(fit, 'fit.confirm', {})
    return project[fitting.publish(project, fit, {})]


def _errors(shapes, truth):
    f = np.sort(np.asarray(shapes.frequency))
    order = np.argsort(shapes.frequency)
    z = np.asarray(shapes.damping)[order]
    ft, zt = (np.array(v) for v in zip(*truth, strict=True))
    return np.abs(f / ft - 1).max(), np.abs(z / zt - 1).max()


def _check(shapes, truth=None):
    truth = truth or list(zip(rf.MODAL_FREQUENCIES, rf.MODAL_DAMPING, strict=True))
    assert shapes.num_shapes == 3
    df, dz = _errors(shapes, truth)
    assert df < FREQUENCY and dz < DAMPING
    assert df < ACHIEVED[0] and dz < ACHIEVED[1], (df, dz)
    return df, dz


def test_modal_test_single_reference_run_goes_to_the_fit_and_recovers_the_structure(
        tmp_path):
    project = _project(tmp_path, 'fixed_hammer')
    frf = project['modal_frf']
    assert fit_refusal(frf) == ''
    ModalFitSession(frf, coherence=project['modal_coherence'])
    assert project.fit_coherence('modal_frf') == 'modal_coherence'
    shapes = _fit(project, 'modal_frf')
    _check(shapes)
    assert shapes.source == 'SIM'
    # a drive point at 1Z+ scales the shapes: they are the structure's own
    assert not shapes.unscaled
    k = list(shapes.coordinate).index('2Z+')
    order = np.argsort(shapes.frequency)
    ratio = np.real(shapes.shape_matrix[order, k] / shapes.shape_matrix[order, 0])
    np.testing.assert_allclose(ratio, rf.MODAL_SHAPES[:, 1] / rf.MODAL_SHAPES[:, 0],
                               rtol=1e-2)


def test_modal_test_roving_hammer_turned_round_goes_to_the_fit(tmp_path):
    project = _project(tmp_path, 'roving_hammer')
    turned = project['modal_frf fixed reference']
    assert is_transposed(turned)
    assert fit_refusal(turned) == ''
    shapes = _fit(project, 'modal_frf fixed reference')
    _check(shapes)
    assert sorted(shapes.coordinate) == [f'{k}Z+' for k in range(1, 7)]
    assert not shapes.unscaled


def test_modal_test_two_shakers_go_to_the_fit_with_multiple_coherence(tmp_path):
    project = _project(tmp_path, 'two_shakers')
    assert fit_refusal(project['modal_frf']) == ''
    _check(_fit(project, 'modal_frf'))


def test_modal_test_exponential_window_damping_is_noted_and_taken_out(tmp_path):
    tau = 0.5
    project = _project(tmp_path, 'fixed_hammer', tau_s=tau)
    shapes = _fit(project, 'modal_frf')
    order = np.argsort(shapes.frequency)
    # the fit reads the windowed poles: its damping is the window's as well as the
    # structure's, so it is out of the brief's bounds as fitted
    _check(shapes, rf.modal_poles(1.0 / tau))
    with pytest.raises(AssertionError):
        _check(shapes)
    corrected = [window_corrected(shapes)[m] for m in order]
    assert all(c is not None for c in corrected)
    for (f, z), fn, zeta in zip(corrected, rf.MODAL_FREQUENCIES, rf.MODAL_DAMPING,
                                strict=True):
        assert abs(f / fn - 1) < FREQUENCY and abs(z / zeta - 1) < DAMPING
        assert abs(f / fn - 1) < LESS_WINDOW[0] and abs(z / zeta - 1) < LESS_WINDOW[1]
    # the fitted values are left as the fit made them
    assert shapes.comment[order[0]].startswith('exponential window tau 0.5 s adds about')


def test_modal_test_fit_modes_notes_the_window_too(tmp_path):
    project = _project(tmp_path, 'fixed_hammer', tau_s=0.5)
    name = project.fit_modes('modal_frf', bounds=(5.0, 140.0), limit=3,
                             coherence='modal_coherence')
    shapes = project[name]
    assert shapes.num_shapes == 3
    assert all(c is not None for c in window_corrected(shapes))
    plain = _project(tmp_path / 'plain', 'fixed_hammer')
    name = plain.fit_modes('modal_frf', bounds=(5.0, 140.0), limit=3)
    assert window_corrected(plain[name]) == [None] * 3


def test_modal_test_run_takes_the_modal_report_and_it_builds_and_renders(tmp_path):
    """A Modal Test project picks the modal report, and that report builds from the converted
    run with its fit and renders with the run's FRFs, coherence and modes bound."""
    from hertzforge.core.report import TEMPLATE_BUILDERS, TEST_TEMPLATES
    from hertzforge.report import render_html
    from hertzforge.service.reports import run_template

    project = _project(tmp_path, 'fixed_hammer')
    _fit(project, 'modal_frf')
    assert TEST_TEMPLATES['modal_test'] == 'modal'
    assert run_template(project) == 'modal'
    report = TEMPLATE_BUILDERS[run_template(project)](project, project.object_groups)
    page = render_html(report, project, None, object_groups=project.object_groups)
    assert 'Modal Test Report' in page
    # the identified parameters table holds the fit of the run's FRFs
    assert '"rows": [["1", "20.0000", "3.000", ""]' in page


def test_modal_test_three_triaxial_accelerometers_and_a_roving_hammer_are_turned(tmp_path):
    """Nine fixed responses and six hammer points: fewer references than responses, so the
    set's shape alone would not say the excitation roved; the roving plan does."""
    project = _project(tmp_path, 'triaxial_roving_hammer')
    measured = project['modal_frf']
    assert len(set(measured.response_dof)) == 9 and len(set(measured.reference_dof)) == 6
    turned = project['modal_frf fixed reference']
    assert sorted(set(turned.reference_dof)) == sorted(set(measured.response_dof))
    assert fit_refusal(turned) == ''
    fit = fitting.start(project, {'frf': 'modal_frf fixed reference'})
    assert fit.coherence == 'modal_coherence fixed reference'
    # the record of 5Z+ for a force at 2X+ is the structure's own, turned from 2X+ per 5Z+
    k = next(i for i in range(turned.num_records)
             if (turned.response_dof[i], turned.reference_dof[i]) == ('5Z+', '2X+'))
    np.testing.assert_allclose(turned.ordinate[k],
                               rf.modal_accelerance(turned.abscissa, (4, 'Z+'), (1, 'X+')),
                               rtol=1e-12)
    _check(_fit(project, 'modal_frf fixed reference'))


def test_modal_test_turning_carries_each_hammer_direction_in_the_values(tmp_path):
    """Hammer hits from below (Z-) at points 2 and 5: the turned records' values carry the
    sign, and the fit's shapes on those points are the structure's turned over."""
    project = _project(tmp_path, 'roving_hammer', directions={1: 'Z-', 4: 'Z-'})
    turned = project['modal_frf fixed reference']
    assert set(turned.response_dof) == {'1Z+', '2Z-', '3Z+', '4Z+', '5Z-', '6Z+'}
    for dof, point, sign in (('2Z-', 1, -1.0), ('3Z+', 2, 1.0), ('5Z-', 4, -1.0)):
        np.testing.assert_allclose(turned.ordinate[turned.response_dof.index(dof)],
                                   sign * rf.modal_accelerance(turned.abscissa, point, 0),
                                   rtol=1e-12)
    shapes = _fit(project, 'modal_frf fixed reference')
    _check(shapes)
    order = np.argsort(shapes.frequency)
    row = shapes.shape_matrix[order]
    for dof, point, sign in (('2Z-', 1, -1.0), ('5Z-', 4, -1.0)):
        k = list(shapes.coordinate).index(dof)
        ratio = np.real(row[:, k] / row[:, list(shapes.coordinate).index('1Z+')])
        np.testing.assert_allclose(ratio, sign * rf.MODAL_SHAPES[:, point]
                                   / rf.MODAL_SHAPES[:, 0], rtol=1e-2)


#: two modes 3 % apart, each with a half-power bandwidth larger than their spacing's half
CLOSE = ((20.0, 45.0, 46.35), (0.02, 0.02, 0.02))
#: measured on this branch through the turned set with one refine, with margin: the
#: largest frequency error 7.8e-6, damping error 9.6e-4, shape-ratio error 1.7e-3
CLOSE_ACHIEVED = (5e-5, 5e-3, 1e-2)


def test_modal_test_close_modes_in_noise_are_separated_by_the_fit(tmp_path):
    """Modes at 45 and 46.35 Hz, 2 % damped, overlap: each one's half-power band is 1.8 Hz
    and they are 1.35 Hz apart. With noise 40 dB under each FRF, a roving hammer's set turned
    round goes through the fitting screen's path and one refine, and the fit recovers all
    three modes and the shapes on Z."""
    project = _project(tmp_path, 'roving_hammer', modes=CLOSE, noise_db=-40)
    turned = project['modal_frf fixed reference']
    clean = np.array([rf.modal_accelerance(turned.abscissa, j, 0, modes=CLOSE)
                      for j in range(6)])
    # the noise is there, at about the level asked for
    error = np.sqrt(np.mean(np.abs(turned.ordinate - clean) ** 2, axis=-1)
                    / np.mean(np.abs(clean) ** 2, axis=-1))
    assert np.all((error > 10 ** (-42 / 20)) & (error < 10 ** (-38 / 20))), error
    fit = fitting.start(project, {'frf': 'modal_frf fixed reference'})
    for _ in range(3):
        fitting.act(fit, 'fit.find', {'low': 5.0, 'high': 140.0})
        fitting.act(fit, 'fit.confirm', {})
    fitting.act(fit, 'fit.refine', {})
    shapes = project[fitting.publish(project, fit, {})]
    assert shapes.num_shapes == 3 and not shapes.unscaled
    df, dz = _errors(shapes, list(zip(*CLOSE, strict=True)))
    assert df < CLOSE_ACHIEVED[0] and dz < CLOSE_ACHIEVED[1], (df, dz)
    order = np.argsort(shapes.frequency)
    columns = [list(shapes.coordinate).index(f'{k}Z+') for k in range(1, 7)]
    for row, m in enumerate(order):
        ratio = np.real(shapes.shape_matrix[m, columns] / shapes.shape_matrix[m, columns[0]])
        np.testing.assert_allclose(ratio, rf.MODAL_SHAPES[row] / rf.MODAL_SHAPES[row, 0],
                                   rtol=0, atol=CLOSE_ACHIEVED[2])
