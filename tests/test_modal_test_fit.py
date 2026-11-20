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
