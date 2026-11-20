"""The modal test's analysis core (`core.modal_test`): the fixed-reference form by
reciprocity, the exponential window's damping, and a measurement estimated again."""

from __future__ import annotations

import math

import numpy as np
import pytest

from hertzforge.core.averaging import Averaging, exponential_tau
from hertzforge.core.data import Coherence, Frf, MultipleCoherence, TimeHistory
from hertzforge.core.modal_test import (
    TRANSPOSED,
    fixed_reference,
    fixed_reference_refusal,
    frf_tau,
    is_transposed,
    less_window_damping,
    note_window_damping,
    reprocess,
    roving_excitation,
    window_corrected,
    window_damping,
    window_note,
)
from hertzforge.core.shapes import ShapeSet

FS = 1024.0
LINES = np.arange(0.0, 200.0, 1.0)


def _roving(n: int = 4, dim: str = 'acceleration/force') -> Frf:
    """A roving hammer set: one accelerometer at 1X, the hammer at n points."""
    values = np.array([np.full(len(LINES), k + 1.0 + 0.5j) for k in range(n)])
    return Frf(LINES, values, response_dof=['1X+'] * n,
               reference_dof=[f'{k + 1}X+' for k in range(n)], ordinate_dim=[dim] * n,
               comment=[f'H1, point {k + 1}' for k in range(n)])


def test_modal_test_fixed_reference_turns_a_roving_set_round():
    frf = _roving()
    frf.averaging = Averaging(frame_length=1024, window='exponential', window_parameter=0.05)
    assert roving_excitation(frf)
    out = fixed_reference(frf)
    assert isinstance(out, Frf) and out is not frf
    assert out.response_dof == ['1X+', '2X+', '3X+', '4X+']
    assert out.reference_dof == ['1X+'] * 4
    np.testing.assert_array_equal(out.ordinate, frf.ordinate)
    assert out.ordinate_dim == frf.ordinate_dim
    assert is_transposed(out) and not is_transposed(frf)
    assert out.comment[1] == f'{TRANSPOSED}: measured as 1X+ against 2X+; H1, point 2'
    assert out.averaging == frf.averaging
    assert not roving_excitation(out)
    # the measured set is left as it was
    assert frf.response_dof == ['1X+'] * 4


def test_modal_test_fixed_reference_turns_an_ordinary_coherence_and_a_subset():
    coherence = Coherence(LINES, np.full((3, len(LINES)), 0.9), response_dof=['1Z'] * 3,
                          reference_dof=['1Z', '2Z', '3Z'])
    out = fixed_reference(coherence, records=[2, 0])
    assert isinstance(out, Coherence)
    assert out.response_dof == ['3Z+', '1Z+'] and out.reference_dof == ['1Z+', '1Z+']


@pytest.mark.parametrize('make, words', [
    (lambda: MultipleCoherence(LINES, np.ones((1, len(LINES))), response_dof=['1X']),
     'multiple coherence belongs'),
    (lambda: TimeHistory(LINES, np.ones((1, len(LINES))), response_dof=['1X']),
     'only an FRF or an ordinary coherence'),
    (lambda: _roving(2, 'force/force'), 'force per force'),
    (lambda: _roving(2, 'acceleration/acceleration'), 'acceleration per acceleration'),
    (lambda: Frf(LINES, np.ones((1, len(LINES))), response_dof=['1'], reference_dof=['2X'],
                 ordinate_dim=['acceleration/force']), 'names no direction'),
    (lambda: Frf(LINES, np.ones((2, len(LINES))), response_dof=['1X', '1X'],
                 reference_dof=['2X', '2X'], ordinate_dim=['acceleration/force'] * 2),
     'records 0 and 1 would both be'),
    (lambda: fixed_reference(_roving()), 'turned by reciprocity already'),
])
def test_modal_test_fixed_reference_refuses_what_reciprocity_does_not_turn(make, words):
    obj = make()
    assert words in fixed_reference_refusal(obj)
    with pytest.raises(ValueError, match=words.split()[-1]):
        fixed_reference(obj)


def test_modal_test_fixed_reference_refuses_no_records_and_absent_ones():
    frf = _roving()
    assert 'no records' in fixed_reference_refusal(frf, [])
    assert 'there is no record 7' in fixed_reference_refusal(frf, [0, 7])
    assert fixed_reference_refusal(frf, [0, 1]) == ''


def test_modal_test_window_damping_is_one_over_two_pi_f_tau():
    assert window_damping(10.0, 0.5) == pytest.approx(1.0 / (2.0 * math.pi * 5.0))
    # a mode of 50 Hz at 2 %: the window moves its pole left by exactly 1 / tau
    fn, zeta, tau = 50.0, 0.02, 0.2
    wn = 2 * math.pi * fn
    decay, damped = zeta * wn + 1.0 / tau, wn * math.sqrt(1 - zeta ** 2)
    fitted_wn = math.hypot(decay, damped)
    f, z = less_window_damping(fitted_wn / (2 * math.pi), decay / fitted_wn, tau)
    assert float(f) == pytest.approx(fn, rel=1e-12)
    assert float(z) == pytest.approx(zeta, rel=1e-12)
    # to first order the fitted damping is high by window_damping
    assert decay / fitted_wn - zeta == pytest.approx(window_damping(fn, tau), rel=0.01)
    # a mode the window accounts for whole has no damping of its own to give
    f, z = less_window_damping(50.0, window_damping(50.0, tau) * 0.5, tau)
    assert math.isnan(float(z))
    with pytest.raises(ValueError, match='above zero'):
        less_window_damping(50.0, 0.02, 0.0)


def test_modal_test_frf_tau_reads_the_averaging_and_survives_its_end_value():
    frf = _roving()
    assert frf_tau(frf) is None
    # a 1024 block at 1 Hz lines is 1024 samples a second
    end = math.exp(-(1024 - 1) / (0.25 * 1024))
    frf.averaging = Averaging(frame_length=1024, window='exponential', window_parameter=end)
    assert frf_tau(frf) == pytest.approx(0.25, rel=1e-12)
    frf.averaging = Averaging(frame_length=1024, window='hann')
    assert frf_tau(frf) is None


def _shapes(frequency, damping, comment=None) -> ShapeSet:
    n = len(frequency)
    return ShapeSet(np.asarray(frequency, float), np.asarray(damping, float),
                    ['1X+', '2X+'], np.ones((n, 2)), comment=comment or [''] * n)


def test_modal_test_note_window_damping_writes_the_mode_less_the_window():
    frf = _roving()
    end = math.exp(-(1024 - 1) / (0.25 * 1024))
    frf.averaging = Averaging(frame_length=1024, window='exponential', window_parameter=end)
    shapes = _shapes([50.0, 120.0], [0.03, 0.02], comment=['bending', ''])
    note_window_damping(shapes, frf)
    assert shapes.comment[0].startswith('bending | exponential window tau 0.25 s')
    # the fit's values are left as the fit made them
    assert list(shapes.damping) == [0.03, 0.02]
    expected = [tuple(float(v) for v in less_window_damping(f, z, 0.25))
                for f, z in ((50.0, 0.03), (120.0, 0.02))]
    for (f, z), (ef, ez) in zip(window_corrected(shapes), expected, strict=True):
        assert f == pytest.approx(ef, rel=1e-5) and z == pytest.approx(ez, rel=1e-3)
    # publishing again replaces the note rather than adding a second
    note_window_damping(shapes, frf)
    assert shapes.comment[0].count('exponential window') == 1
    assert shapes.comment[0].startswith('bending | ')
    # an FRF with no exponential window leaves the set alone
    plain = _shapes([50.0], [0.03])
    note_window_damping(plain, _roving())
    assert plain.comment == [''] and window_corrected(plain) == [None]
    assert 'not known' in window_note(50.0, 0.001, 0.25)


# ---- estimating again ------------------------------------------------------

FN, ZETA, BLOCK, PRE = 50.0, 0.02, 1024, 32


def _impact(hits: int = 4, noise: float = 0.0, seed: int = 3) -> TimeHistory:
    """An impact measurement of one mode: a hammer at 2Z+ and an accelerometer
    at 1Z+, `hits` frames, each with its pre-trigger samples at negative times."""
    rng = np.random.default_rng(seed)
    t = (np.arange(BLOCK) - PRE) / FS
    wn = 2 * np.pi * FN
    wd = wn * np.sqrt(1 - ZETA ** 2)
    rows, dofs, dims, labels = [], [], [], []
    for k in range(hits):
        pulse = np.where((t >= 0) & (t < 0.004), np.sin(np.pi * t / 0.004), 0.0) * (1 + k)
        # the receptance's impulse response, differentiated twice for acceleration
        h = np.where(t >= 0, np.exp(-ZETA * wn * t) * np.sin(wd * t) / wd, 0.0)
        x = np.convolve(pulse, h)[:BLOCK] / FS
        a = np.gradient(np.gradient(x, 1 / FS), 1 / FS)
        a = a + noise * rng.standard_normal(BLOCK) * np.abs(a).max()
        rows += [pulse, a]
        dofs += ['2Z+', '1Z+']
        dims += ['force', 'acceleration']
        labels += [f'hit {k + 1}'] * 2
    history = TimeHistory(t, np.asarray(rows), response_dof=dofs, ordinate_dim=dims,
                          ordinate_unit=['N', 'm/s^2'] * hits, block=labels)
    history.roles = {('2Z+', 'force'): 'reference', ('1Z+', 'acceleration'): 'response'}
    return history


def test_modal_test_reprocess_estimates_a_measurement_again_with_coherence():
    history = _impact(noise=0.01)
    frf, coherence = reprocess(history, method='H1')
    assert isinstance(frf, Frf) and isinstance(coherence, Coherence)
    assert frf.response_dof == ['1Z+'] and frf.reference_dof == ['2Z+']
    assert coherence.reference_dof == ['2Z+']
    # the peak sits at the damped frequency, within a line
    peak = frf.abscissa[np.argmax(np.abs(frf.ordinate[0, 1:])) + 1]
    assert abs(peak - FN) <= 1.0
    assert '4 frames' in frf.comment[0]
    assert frf.averaging.frames == 4 and frf.averaging.window == 'boxcar'
    near = np.abs(coherence.abscissa - FN) < 3
    assert coherence.ordinate[0, near].min() > 0.95


def test_modal_test_reprocess_carries_its_windows_and_a_subset_of_frames():
    history = _impact()
    frf, _ = reprocess(history, force=0.05, exponential=0.01, block=512, captures=[0, 2],
                       method='H1')
    assert len(frf.abscissa) == 512 // 2 + 1
    assert frf.averaging.frame_length == 512 and frf.averaging.frames == 2
    assert frf.averaging.window == 'exponential'
    assert frf_tau(frf) == pytest.approx(exponential_tau(0.01, 512) / FS, rel=1e-9)
    assert 'captures 1, 3' in frf.comment[0]
    assert 'force window 0.05 of the block on the references' in frf.comment[0]
    # the windowed estimate differs from the plain one: the windows were applied
    plain, _ = reprocess(history, block=512, captures=[0, 2])
    assert not np.allclose(plain.ordinate, frf.ordinate)
    shaker, _ = reprocess(history, shaker_window='hann', block=256)
    assert shaker.averaging.window == 'hann'


@pytest.mark.parametrize('kwargs, words', [
    ({'block': 1}, 'from 2 samples'),
    ({'block': BLOCK + 1}, 'from 2 samples'),
    ({'captures': []}, 'no frames are picked'),
    ({'captures': [0, 9]}, 'no capture 9'),
    ({'shaker_window': 'hann', 'force': 0.1}, 'not used with the force'),
    ({'method': 'H1', 'captures': [0]}, 'average'),
])
def test_modal_test_reprocess_refuses(kwargs, words):
    with pytest.raises(ValueError, match=words):
        reprocess(_impact(), **kwargs)


def test_modal_test_reprocess_refuses_a_frame_that_is_not_finite_and_an_empty_history():
    history = _impact()
    history.ordinate[1, 100] = np.nan
    with pytest.raises(ValueError, match='finite'):
        reprocess(history)
    dead = _impact()
    dead.ordinate[2] = 0.0
    with pytest.raises(ValueError, match='dead reference'):
        reprocess(dead)
    # leaving the dead hit out estimates from the rest
    frf, _ = reprocess(dead, captures=[0, 2, 3])
    assert np.isfinite(frf.ordinate).all() and np.abs(frf.ordinate).max() > 0
