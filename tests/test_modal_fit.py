"""Fitting real normal modes to measured FRFs, one mode at a time.

The survey fixtures are the truth serum again: the FRFs were synthesized
from a known modal model, so a fitter that works must find those very
modes — frequencies on the true values, shapes at MAC ≈ 1 against the
true shapes — and every confirmed mode must shrink the residual.
"""

from __future__ import annotations

import itertools

import numpy as np
import pytest

from hertzforge.core.modal_fit import ModalFitSession


def mac(a, b):
    return abs(a @ b) ** 2 / ((a @ a) * (b @ b))


def test_five_confirms_find_five_true_modes(survey):
    truth, frfs = survey
    session = ModalFitSession(frfs)
    # energy, not the pointwise max: the global maximum can sit away from
    # every fitted peak and jitter, but each mode removes energy
    residuals = [np.linalg.norm(session.residual_matrix())]
    for _ in range(5):
        session.confirm()
        residuals.append(np.linalg.norm(session.residual_matrix()))
        session.suggest()
    assert all(later < earlier for earlier, later
               in itertools.pairwise(residuals)), 'each mode helps'
    # The residual only has to *fall*, not fall by a quota. Suggestions
    # rank by prominence — the eye's order — rather than raw height,
    # so the loop fits the low-frequency modes an engineer would take
    # first; on accelerance those carry little linear energy, and a
    # bulk-energy quota would drag the ranking back to tall-cluster
    # chasing, which is the disease the prominence order cured.

    fitted = session.shape_set()
    columns = [truth.coordinate.index(dof) for dof in fitted.coordinate]
    df = frfs.abscissa[1] - frfs.abscissa[0]
    for m in range(5):
        nearest = int(np.argmin(np.abs(truth.frequency
                                       - fitted.frequency[m])))
        assert abs(truth.frequency[nearest]
                   - fitted.frequency[m]) < df, 'on a true mode'
        # the plate has degenerate pairs, and any rotation of a pair is
        # as true a mode as either member — so the shape is judged
        # against the *span* of the truth modes at that frequency,
        # which reduces to the plain MAC when the mode stands alone
        pack = np.abs(truth.frequency
                      - truth.frequency[nearest]) < 2.0 * df
        basis = truth.shape_matrix[pack][:, columns].T
        phi = fitted.shape_matrix[m]
        coefficients = np.linalg.lstsq(basis, phi, rcond=None)[0]
        assert mac(phi, basis @ coefficients) > 0.98
        assert fitted.damping[m] == pytest.approx(
            truth.damping[nearest], rel=0.5), 'half-power in the ballpark'


def test_the_fitted_modes_resynthesize_the_measurement(survey):
    """The point of the whole tool: the fit predicts the data. Around each
    fitted mode's peak the synthesis must sit on the measurement."""
    truth, frfs = survey
    session = ModalFitSession(frfs)
    for _ in range(4):
        session.confirm()
        session.suggest()
    synthesis = session.synthesis_matrix()
    df = frfs.abscissa[1] - frfs.abscissa[0]
    checked = 0
    for mode in session.modes:
        # a lone fit at a degenerate peak legitimately misses its
        # twin's share of the response, so only modes that stand alone
        # in the truth are held to the bound
        if np.sum(np.abs(truth.frequency
                         - mode['frequency']) < 2.0 * df) > 1:
            continue
        checked += 1
        line = int(np.argmin(np.abs(frfs.abscissa - mode['frequency'])))
        measured = session.matrix[line]
        error = np.abs(synthesis[line] - measured)
        # close true modes share a peak's skirt, so the one-mode fit is
        # not exact there — but it must carry most of the response
        assert error.max() < 0.5 * np.abs(measured).max()
    assert checked >= 3, 'the separated modes were all judged'


def test_the_suggestion_moves_after_each_confirm(survey):
    _truth, frfs = survey
    session = ModalFitSession(frfs)
    visited = []
    for _ in range(4):
        visited.append(session.pending['frequency'])
        session.confirm()
        session.suggest()
    assert len(set(visited)) == len(visited), 'a confirmed peak collapses'


def test_shapes_are_real(survey):
    _truth, frfs = survey
    session = ModalFitSession(frfs)
    mode = session.confirm()
    assert np.isrealobj(mode['shape']), 'real normal modes only'


def test_suggestions_stay_inside_the_view(survey):
    _truth, frfs = survey
    session = ModalFitSession(frfs)
    everywhere = session.pending['frequency']
    frequency, _damping = session.suggest((5.0, 30.0))
    assert 5.0 <= frequency <= 30.0
    assert frequency != everywhere, 'the global peak lies outside the view'


def test_adopting_a_shape_set_reconstructs_the_fit(survey):
    """A ShapeSet carries the whole fit state — frequency, damping, shape,
    description — so a session seeded from one behaves as if those modes
    had just been confirmed: they subtract from the residual."""
    _truth, frfs = survey
    fresh = ModalFitSession(frfs)
    for _ in range(3):
        fresh.confirm()
        fresh.suggest()
    published = fresh.shape_set()

    resumed = ModalFitSession(frfs)
    resumed.adopt(published)
    assert len(resumed.modes) == 3
    assert np.allclose(
        np.linalg.norm(resumed.residual_matrix()),
        np.linalg.norm(fresh.residual_matrix())), 'the same fit, resumed'


def test_a_foreign_set_maps_by_dof_name(survey):
    from hertzforge.core.shapes import ShapeSet

    _truth, frfs = survey
    session = ModalFitSession(frfs)
    foreign = ShapeSet([12.0], [0.02], ['99999X+', '101Z+'],
                       [[7.0, 3.5]])
    session.adopt(foreign)
    mode = session.modes[0]
    assert mode['shape'][session.coordinate.index('101Z+')] == 3.5
    assert 7.0 not in mode['shape'], 'a DOF this FRF never measured'


def test_adopting_rigid_body_modes_survives_a_zero_hertz_line(survey):
    """The user's data starts at 0 Hz and his shape sets carry rigid-body
    modes: their SDOF term there is 0/0, and the NaN it produced stopped
    the residual's SVD converging the moment such a set was adopted."""
    truth, _frfs = survey
    assert (truth.frequency == 0.0).any()
    freq = np.linspace(0.0, 60.0, 41)
    synthesized = truth.synthesize_frf(
        freq, ['101X+', '707Z+'], ['101Z+', '101Z+'], power=2)
    from hertzforge.core.data import Frf
    measured = Frf(freq, synthesized,
                   response_dof=['101X+', '707Z+'],
                   reference_dof=['101Z+', '101Z+'])
    session = ModalFitSession(measured)
    session.adopt(truth)
    residual = session.residual_matrix()
    assert np.isfinite(residual).all(), 'no NaN reaches the SVD'
    scale = np.abs(session.matrix).max()
    assert np.abs(residual).max() < 1e-6 * scale, (
        'the adopted model explains its own synthesis, 0 Hz included')
    session.suggest()   # the crashing call


def synthetic_frf(power=2, responses=('1X+', '2X+', '3X+'),
                  reference='9Z+', frequency=20.0, damping=0.02,
                  declared=True):
    """One known mode, synthesized exactly, no drive point measured.

    `declared` passes the quantity as the dimension (values are SI);
    False passes it as a hint only — the file said what, never how much.
    """
    from hertzforge.core.data import Frf

    shape = {'1X+': 1.0, '2X+': -0.6, '3X+': 0.3, '9Z+': 0.8}
    freq = np.linspace(0.0, 50.0, 201)
    omega = 2 * np.pi * freq
    omega_r = 2 * np.pi * frequency
    q = (1j * omega) ** power / (
        omega_r ** 2 - omega ** 2 + 2j * damping * omega_r * omega)
    ordinate = np.stack([shape[r] * shape[reference] * q
                         for r in responses])
    dims = {0: 'length', 1: 'velocity', 2: 'acceleration'}
    quantity = f'{dims[power]}/force'
    tagged = ({'ordinate_dim': quantity} if declared
              else {'dimension_hint': quantity})
    return Frf(freq, ordinate, response_dof=list(responses),
               reference_dof=[reference] * len(responses), **tagged)


def test_the_measured_quantity_sets_the_power():
    session = ModalFitSession(synthetic_frf(power=1))
    assert session.power == 1
    session.confirm()
    scale = np.abs(session.matrix).max()
    assert np.abs(session.residual_matrix()).max() < 1e-2 * scale, (
        'a mobility fits as a mobility')


def test_no_drive_point_still_resynthesizes():
    """The reference DOF is never measured as a response, so nothing pins
    the mass-normalized scale — but the residues, and therefore the
    resynthesis, must still be right."""
    session = ModalFitSession(synthetic_frf())
    assert not any(dof in session.responses for dof in session.references)
    session.confirm()
    scale = np.abs(session.matrix).max()
    assert np.abs(session.residual_matrix()).max() < 1e-2 * scale


def test_a_view_beyond_the_data_falls_back_to_everything():
    """Scrolled off the end of the record there is no mode to find, and
    an empty answer is worse than the whole range."""
    session = ModalFitSession(synthetic_frf())
    assert session.searched((900.0, 1000.0)) == session.span
    frequency, _damping = session.suggest((900.0, 1000.0))
    assert 0.0 <= frequency <= 50.0, 'an empty view cannot mean no answer'


def test_half_power_on_a_flat_curve_falls_back_to_nominal():
    from hertzforge.core.data import Frf

    flat = Frf(np.linspace(1.0, 10.0, 10),
               np.ones((1, 10), dtype=complex),
               response_dof=['1X+'], reference_dof=['1X+'])
    session = ModalFitSession(flat)
    assert session.estimate_damping(5.0) == pytest.approx(0.01), (
        'no crossings on either side: the nominal 1%')


def test_confirm_with_explicit_arguments_refits_there():
    session = ModalFitSession(synthetic_frf())
    mode = session.confirm(frequency=20.0, damping=0.02,
                           description='typed in')
    assert mode['frequency'] == 20.0
    assert mode['description'] == 'typed in'
    scale = np.abs(session.matrix).max()
    assert np.abs(session.residual_matrix()).max() < 1e-2 * scale


def test_defined_units_make_the_fitted_shapes_si():
    session = ModalFitSession(synthetic_frf(declared=True))
    session.confirm()
    assert session.shape_set().mass_unit == 'kg'
    undefined = ModalFitSession(synthetic_frf(declared=False))
    assert undefined.power == 2, 'the hint still names the quantity'
    undefined.confirm()
    assert undefined.shape_set().mass_unit is None, (
        'raw data cannot claim SI shapes')


@pytest.mark.parametrize('sign', [1.0, -1.0])
def test_residue_products_survive_the_svd_sign(sign):
    """An SVD's singular vector carries an arbitrary sign; whichever way
    it lands — forced both ways here by negating the data — the fitted
    shape must reproduce the residue products phi_response * phi_reference,
    signs included."""
    frf = synthetic_frf()
    frf.ordinate *= sign
    session = ModalFitSession(frf)
    mode = session.confirm()
    truth = {'1X+': 1.0, '2X+': -0.6, '3X+': 0.3, '9Z+': 0.8}
    reference = session.coordinate.index('9Z+')
    for dof in ('1X+', '2X+', '3X+'):
        product = (mode['shape'][session.coordinate.index(dof)]
                   * mode['shape'][reference])
        assert product == pytest.approx(sign * truth[dof] * truth['9Z+'],
                                        rel=0.05)


def test_a_backwards_drive_point_flips_scale_not_residues():
    """Exact data cannot make the median ratio negative (it is |phi|^2
    for either SVD sign), so the flip branch answers one thing only: an
    inconsistent measurement, like a drive-point gauge wired backwards.
    The other records' residue products must come through unharmed —
    the branch once negated the direction but not the residues."""
    frf = synthetic_frf(responses=('1X+', '2X+', '3X+', '9Z+'))
    drive = list(frf.response_dof).index('9Z+')
    frf.ordinate[drive] *= -1.0          # the backwards gauge
    session = ModalFitSession(frf)
    mode = session.confirm()
    truth = {'1X+': 1.0, '2X+': -0.6, '3X+': 0.3, '9Z+': 0.8}
    for dof in ('1X+', '2X+'):
        product = (mode['shape'][session.coordinate.index(dof)]
                   * mode['shape'][session.coordinate.index('9Z+')])
        expected = truth[dof] * truth['9Z+']
        assert abs(product) == pytest.approx(abs(expected), rel=0.2), (
            'the healthy records still resynthesize')


def test_the_view_narrows_the_search(survey):
    """Zooming to a region is the plainest way to say where the next mode
    is, and it should not have to be said twice by dragging the band."""
    _truth, frfs = survey
    session = ModalFitSession(frfs)
    everywhere = session.suggest()[0]
    # each window holds one mode with room around it: a window edge
    # on a peak's rising skirt gets an interpolated answer a line
    # outside the window, which is the mechanism working as meant
    for low, high in ((400.0, 500.0), (600.0, 700.0),
                      (900.0, 1300.0)):
        found = session.suggest((low, high))[0]
        assert low <= found <= high, f'{found} is outside {low}-{high}'
    assert session.suggest((600.0, 700.0))[0] != everywhere, (
        'a window that excludes the global peak has to move the answer')


def test_the_view_is_clamped_to_the_measured_range(survey):
    """A view hanging off either end still searches data."""
    _truth, frfs = survey
    session = ModalFitSession(frfs)
    low, high = session.span
    assert session.searched((low - 50.0, 40.0)) == (low, 40.0), 'hangs below'
    assert session.searched((30.0, high + 50.0)) == (30.0, high), 'hangs above'
    assert session.searched((30.0, 50.0)) == (30.0, 50.0), 'wholly inside'
    assert session.searched(None) == session.span, 'no view, no narrowing'
    # scrolled clean off the end there is nothing to find, and an empty
    # range would mean no answer at all
    assert session.searched((high + 10.0, high + 50.0)) == session.span


def test_a_window_between_two_lines_still_finds_something(survey):
    """Zoomed closer than the frequency resolution: no line falls in the
    view, and the whole range answers rather than nothing."""
    _truth, frfs = survey
    session = ModalFitSession(frfs)
    spacing = float(frfs.abscissa[1] - frfs.abscissa[0])
    middle = float(frfs.abscissa[10]) + spacing * 0.3
    found = session.suggest((middle, middle + spacing * 0.2))[0]
    assert session.span[0] <= found <= session.span[1]


def _sdof(truth, zeta, df=0.5, lines=400):
    """One mode, synthesized as the accelerance it is declared to be."""
    from hertzforge.core.data import Frf

    f = np.arange(1, lines + 1) * df
    w, wn = 2 * np.pi * f, 2 * np.pi * truth
    h = (1j * w) ** 2 / (wn ** 2 - w ** 2 + 2j * zeta * wn * w)
    return Frf(f, np.array([h, 0.7 * h, -0.3 * h]),
               response_dof=['1Z+', '2Z+', '3Z+'],
               reference_dof=['1Z+'] * 3,
               ordinate_dim='acceleration/force')


@pytest.mark.parametrize(('truth', 'zeta'), [
    (63.13, 0.02),      # between two lines
    (63.00, 0.02),      # exactly on one
    (120.37, 0.005),    # lightly damped, a few lines wide
    (40.62, 0.05),      # heavily damped, broad
])
def test_the_search_finds_a_mode_between_the_lines(truth, zeta):
    """A resonance falls where the structure puts it, and the odds of
    that being an analysis line are the line spacing over itself.

    The peak used to be reported as the largest *line*, which pinned
    every mode to the frequency resolution — 130 mHz of error on a
    half-hertz grid, where the residual minimum is inside a few.
    """
    from hertzforge.core.modal_fit import ModalFitSession

    session = ModalFitSession(_sdof(truth, zeta))
    found, damping = session.suggest()
    spacing = float(session.frequencies[1] - session.frequencies[0])
    assert abs(found - truth) < 0.02 * spacing, (
        f'{found} against {truth}, lines {spacing} apart')
    assert damping == pytest.approx(zeta, rel=0.05)


def test_the_damping_no_longer_comes_from_two_crossings():
    """Half power reads two points off a curve and assumes the peak is
    one isolated mode. Where two sit close, the width it measures is
    the pair's, so the damping comes back too high and nothing says so.
    """
    from hertzforge.core.data import Frf
    from hertzforge.core.modal_fit import ModalFitSession

    df, zeta = 0.25, 0.01
    f = np.arange(1, 601) * df
    w = 2 * np.pi * f
    pair = sum((1j * w) ** 2
               / ((2 * np.pi * hz) ** 2 - w ** 2
                  + 2j * zeta * (2 * np.pi * hz) * w) * weight
               for hz, weight in ((60.0, 1.0), (61.2, 0.9)))
    frf = Frf(f, np.array([pair, 0.6 * pair]),
              response_dof=['1Z+', '2Z+'], reference_dof=['1Z+'] * 2,
              ordinate_dim='acceleration/force')
    session = ModalFitSession(frf)
    _found, searched = session.suggest()
    index = int(np.argmin(np.abs(np.asarray(session.frequencies) - 60.0)))
    crossings = session._half_power(session._residual_cmif(), index)
    assert crossings > 2 * zeta, (
        'the close pair should fool half power, or this proves nothing')
    assert searched < crossings, (
        'the search should not be fooled the same way')


def test_a_dragged_fit_keeps_the_damping_its_parabola_shows():
    """The drag searches the damping at the cursor and draws it; the fit
    that follows each tick read the half-power width again, so on a
    close pair the mode a drag fitted took the pair's width, not the
    damping on screen (2026-10-13). The fit keeps the searched one."""
    from hertzforge.core.data import Frf
    from hertzforge.core.modal_fit import ModalFitSession

    df, zeta = 0.25, 0.01
    f = np.arange(1, 601) * df
    w = 2 * np.pi * f
    pair = sum((1j * w) ** 2
               / ((2 * np.pi * hz) ** 2 - w ** 2
                  + 2j * zeta * (2 * np.pi * hz) * w) * weight
               for hz, weight in ((60.0, 1.0), (61.2, 0.9)))
    frf = Frf(f, np.array([pair, 0.6 * pair]),
              response_dof=['1Z+', '2Z+'], reference_dof=['1Z+'] * 2,
              ordinate_dim='acceleration/force')
    session = ModalFitSession(frf)
    session.move_to(60.0)
    shown = session.pending['damping']
    index = int(np.argmin(np.abs(np.asarray(session.frequencies) - 60.0)))
    crossings = session._half_power(session._residual_cmif(), index)
    assert abs(crossings - shown) > 0.05 * shown, 'the two must differ, or this proves nothing'
    fitted = session.fit_pending()
    assert fitted['damping'] == pytest.approx(shown, rel=1e-9)
    assert session.confirm()['damping'] == pytest.approx(shown, rel=1e-9)


def test_the_search_is_quick_enough_to_sit_in_a_click():
    """Only the frequency and the damping are nonlinear; given them the
    residues are a linear least squares. So the two-parameter surface is
    walked directly, and everything that does not depend on the pair —
    the shape direction, the residual projected onto it — is worked out
    once."""
    import time

    from hertzforge.core.modal_fit import ModalFitSession

    session = ModalFitSession(_sdof(63.13, 0.02))
    session.suggest()                      # warm
    start = time.perf_counter()
    session.suggest()
    assert (time.perf_counter() - start) < 0.25


def test_the_residual_is_cached_against_the_modes_not_the_cursor():
    """What made dragging have to stay free: a full SVD every tick.
    It does not depend on where the cursor is."""
    from hertzforge.core.modal_fit import ModalFitSession

    session = ModalFitSession(_sdof(63.13, 0.02))
    first = session.residual_matrix()
    assert session.residual_matrix() is first, 'recomputed for nothing'
    session.suggest()
    session.confirm()
    assert session.residual_matrix() is not first, (
        'a confirmed mode changes it and the cache has to notice')


def test_dragging_says_what_damping_the_cursor_implies():
    """You used to drag blind and learn the damping only on Fit Mode."""
    from hertzforge.core.modal_fit import ModalFitSession

    session = ModalFitSession(_sdof(63.13, 0.02))
    session.move_to(63.13)
    assert session.pending['damping'] == pytest.approx(0.02, rel=0.1)
    session.move_to(50.0)
    assert session.pending['damping'] != pytest.approx(0.02, rel=1e-6), (
        'off the peak it should not still be reporting the peak')


def test_a_typed_damping_is_not_overwritten_by_a_drag():
    from hertzforge.core.modal_fit import ModalFitSession

    session = ModalFitSession(_sdof(63.13, 0.02))
    session.pending['damping'] = 0.037
    session.pending['overridden'] = True
    session.move_to(63.13)
    assert session.pending['damping'] == 0.037


def test_a_drag_can_still_ask_for_nothing():
    """The escape hatch for a caller already running behind."""
    from hertzforge.core.modal_fit import ModalFitSession

    session = ModalFitSession(_sdof(63.13, 0.02))
    before = session.pending['damping']
    session.move_to(63.13, damping=False)
    assert session.pending['damping'] == before
    assert session.pending['frequency'] == 63.13


def test_the_damping_estimate_is_a_pure_function_of_the_line(survey):
    """It was path-dependent: the per-tick search was seeded from the
    previous tick's answer, and the residual cost over damping often has
    two minima — a sharp fit to the narrow peak, and a fat one that also
    swallows the shoulders. The same line gave different dampings
    depending on where the cursor had come from, and the synthesis
    jumped between solutions mid-drag."""
    _truth, frfs = survey
    left = ModalFitSession(frfs)
    right = ModalFitSession(frfs)
    target = float(left.pending['frequency'])

    # approach the same frequency from opposite sides, through territory
    # chosen to bias any seed-carrying search differently
    for frequency in (target - 15.0, target - 5.0, target):
        left.move_to(frequency)
    for frequency in (target + 15.0, target + 5.0, target):
        right.move_to(frequency)
    assert left.pending['damping'] == pytest.approx(
        right.pending['damping'], rel=1e-9), (
        'the answer must not depend on where the drag came from')


def test_overridden_damping_survives_the_drag(survey):
    """The second axis of the search in the user's hand: where two
    solutions sit at nearly the same frequency they are told apart by
    damping, and holding it says which neighborhood the fit lands in."""
    _truth, frfs = survey
    session = ModalFitSession(frfs)
    session.override_damping(0.03)
    assert session.pending['damping'] == pytest.approx(0.03)
    assert session.pending['overridden']

    before = session.pending['damping']
    session.move_to(session.pending['frequency'] + 2.0)
    assert session.pending['damping'] == before, 'the drag does not retune it'

    fitted = session.fit_pending()
    assert fitted['damping'] == pytest.approx(0.03), 'the fit uses it'

    session.suggest()
    assert not session.pending['overridden'], 'Find Mode lets go'


def test_override_is_clamped_to_the_believable_range(survey):
    _truth, frfs = survey
    session = ModalFitSession(frfs)
    session.override_damping(5.0)
    assert session.pending['damping'] == pytest.approx(0.5)
    session.override_damping(0.0)
    assert session.pending['damping'] == pytest.approx(1e-4)


def test_the_pending_curve_shows_the_damping(survey):
    """The SDOF magnitude at the pending (frequency, damping), crown on
    the residual CMIF at the cursor — the damping made visible, which is
    what lets a vertical drag be aimed."""
    _truth, frfs = survey
    session = ModalFitSession(frfs)
    session.move_to(session.pending['frequency'])

    session.override_damping(0.01)
    frequencies, sharp = session.pending_curve()
    index = int(np.argmin(np.abs(
        session.frequencies - session.pending['frequency'])))
    height = float(session._residual_cmif()[index])
    assert sharp.max() == pytest.approx(height), 'crown at the CMIF'
    assert np.all(sharp > 0)

    def width(values, axis):
        above = axis[values >= values.max() / np.sqrt(2.0)]
        return float(above[-1] - above[0]) if len(above) else 0.0

    session.override_damping(0.08)
    wider_axis, wide = session.pending_curve()
    assert width(wide, wider_axis) > width(sharp, frequencies), (
        'more damping, fatter parabola')


def test_the_parabola_is_in_display_units_like_the_plot(survey):
    """The session works in SI; the plot draws display units. On
    in-lbf-s data the two are a couple of orders of magnitude apart,
    and the SI-valued parabola was drawn that far below the curves it
    was meant to hug — on screen, indistinguishable from not being
    drawn at all. It passed every test because the fixture's units were
    undefined, and undefined values display as they stand. This one
    defines them.
    """
    import hertzforge
    from hertzforge.drawing.plot import cmif_curves

    _truth, frfs = survey
    frfs = frfs.copy() if hasattr(frfs, 'copy') else frfs
    frfs.define_units('m/s**2', reference_units='N')
    session = ModalFitSession(frfs)
    us = hertzforge.IN_LBF_S

    _frequencies, values = session.pending_curve(us)
    index = int(np.argmin(np.abs(
        session.frequencies - session.pending['frequency'])))

    # the plotted CMIF at the cursor's line, exactly as the plot builds
    # it (no modes are fitted yet, so the residual is the measurement)
    singular, _x = cmif_curves(frfs, None, us)
    drawn = float(singular[0][index])
    assert values.max() == pytest.approx(drawn, rel=1e-6), (
        'the crown must touch the curve the user is looking at')

    # and the SI answer is far away, which is the whole bug
    _f, si_values = session.pending_curve()
    assert not np.isclose(values.max(), si_values.max(), rtol=0.5), (
        'display and SI differ by orders of magnitude on this system'
    )


def _two_close_modes():
    """An FRF synthesized from two known modes, close enough to couple:
    half-power widths of ~2 Hz against 3 Hz of separation."""
    from hertzforge.core.data import Frf

    frequencies = np.linspace(20.0, 80.0, 1201)
    omega = 2.0 * np.pi * frequencies
    truth = [(50.0, 0.02, np.array([1.0, 0.5, -0.5])),
             (53.0, 0.02, np.array([0.2, 1.0, 0.6]))]
    responses = ['1Z+', '2Z+', '3Z+']
    reference = '1Z+'                       # a drive point pins the scale
    rows = np.zeros((3, len(frequencies)), dtype=complex)
    for f_r, zeta, phi in truth:
        omega_r = 2.0 * np.pi * f_r
        q = ((1j * omega) ** 2
             / (omega_r ** 2 - omega ** 2 + 2j * zeta * omega_r * omega))
        rows += np.outer(phi * phi[0], q)
    frf = Frf(frequencies, rows, response_dof=responses,
              reference_dof=[reference] * 3)
    return frf, truth


def _alignment(fitted, phi):
    """|MAC| between a fitted shape's response part and the truth."""
    a = fitted[:3] / np.linalg.norm(fitted[:3])
    b = phi / np.linalg.norm(phi)
    return abs(float(a @ b))


def test_refine_all_stops_close_modes_borrowing():
    """Sequential peeling fits the first of a close pair on data still
    containing the second, so its residues absorb a piece of the
    neighbor — the pair's *sum* tracks the measurement while each
    shape is contaminated. Refine holds the poles and re-fits every
    mode's residues in one least squares with all the terms present,
    and the shapes come back to the truth."""
    frf, truth = _two_close_modes()
    session = ModalFitSession(frf)
    for f_r, zeta, _phi in truth:
        session.confirm(frequency=f_r, damping=zeta)

    before = [_alignment(mode['shape'], phi)
              for mode, (_f, _z, phi) in zip(session.modes, truth)]
    assert min(before) < 0.9999, (
        'the sequential fit is visibly contaminated on this pair — '
        'if it were not, this test would be checking nothing')

    refined = session.refine_residues()
    assert refined == 2
    after = [_alignment(mode['shape'], phi)
             for mode, (_f, _z, phi) in zip(session.modes, truth)]
    assert all(late >= early for early, late in zip(before, after))
    assert min(after) > 0.9999, 'both shapes recover the truth'
    # the poles were the user's and stayed exactly put
    assert [mode['frequency'] for mode in session.modes] == [50.0, 53.0]
    assert [mode['damping'] for mode in session.modes] == [0.02, 0.02]


def test_refine_invalidates_the_residual_cache():
    """The caches stamp on the mode *count*, which refine does not
    change — the shapes did, and a stale residual would go on drawing
    the unrefined world."""
    frf, truth = _two_close_modes()
    session = ModalFitSession(frf)
    for f_r, zeta, _phi in truth:
        session.confirm(frequency=f_r, damping=zeta)
    stale = session.residual_matrix().copy()
    session.refine_residues()
    fresh = session.residual_matrix()
    assert not np.allclose(stale, fresh), (
        'the refined shapes change what is left over')
    assert float(np.linalg.norm(fresh)) < float(np.linalg.norm(stale)), (
        'and together they explain more of the measurement')


def _ill_conditioned_noisy_pair(bad_channel=True):
    """Close poles with *similar* shapes and measurement noise: the
    configuration where the pure joint solve amplifies noise into
    amplitude swaps — shipped, and caught by eye on the drone data as
    'Refine All made the CMIF worse'. With `bad_channel`, one channel
    carries forty times the noise: the case where no refinement the
    measurement can justify exists, and the ladder must decline.
    Returns the session and the clean truth matrix, so improvement can
    be judged against ground truth rather than against the noise."""
    from hertzforge.core.data import Frf

    rng = np.random.default_rng(7)
    frequencies = np.linspace(10.0, 120.0, 2201)
    omega = 2.0 * np.pi * frequencies
    responses = [f'{n}Z+' for n in range(1, 9)]
    references = ['1Z+', '5Z+']
    truth = [(50.0, 0.02, rng.standard_normal(8)),
             (53.0, 0.02, rng.standard_normal(8)),
             (90.0, 0.015, rng.standard_normal(8))]
    truth = [(f, z, p / np.linalg.norm(p)) for f, z, p in truth]
    assert abs(truth[0][2] @ truth[1][2]) > 0.3, (
        'the close pair must be similar or this tests the easy case')
    rows, rdofs, refdofs = [], [], []
    clean_rows = []
    for ref_index in (0, 4):
        for j, dof in enumerate(responses):
            h = sum(p[j] * p[ref_index]
                    * ((1j * omega) ** 2
                       / ((2 * np.pi * f) ** 2 - omega ** 2
                          + 2j * z * (2 * np.pi * f) * omega))
                    for f, z, p in truth)
            clean_rows.append(h.copy())
            level = np.median(np.abs(h))
            sigma = 0.05 * level * (40.0 if bad_channel and j == 3
                                    else 1.0)
            h = h + sigma * (rng.standard_normal(len(h))
                             + 1j * rng.standard_normal(len(h)))
            rows.append(h)
            rdofs.append(dof)
            refdofs.append(references[0 if ref_index == 0 else 1])
    frf = Frf(frequencies, np.array(rows), response_dof=rdofs,
              reference_dof=refdofs)
    session = ModalFitSession(frf)
    for f, z, _p in truth:
        session.confirm(frequency=f, damping=z)
    clean = np.zeros_like(session.matrix)
    for k in range(2):
        for j in range(8):
            clean[:, j, k] = clean_rows[k * 8 + j]
    return session, clean


def _union_band(session):
    """The lines the refine solves and scores over."""
    df = session.frequencies[1] - session.frequencies[0]
    lines = set()
    for mode in session.modes:
        i = int(np.argmin(np.abs(session.frequencies
                                 - mode['frequency'])))
        reach = max(round(3.0 * mode['damping'] * mode['frequency'] / df),
                    2)
        lines.update(range(max(i - reach, 0),
                           min(i + reach + 1, len(session.frequencies))))
    return np.array(sorted(lines))


def _score_vs(session, matrix):
    """CMIF mismatch of the synthesis against `matrix`, on the bands."""
    band = _union_band(session)
    s_reference = np.linalg.svd(matrix[band], compute_uv=False)[:, 0]
    s_synthesis = np.linalg.svd(session.synthesis_matrix()[band],
                                compute_uv=False)[:, 0]
    return float(np.linalg.norm(s_synthesis - s_reference)
                 / np.linalg.norm(s_reference))


def test_refine_never_worsens_the_measured_cmif():
    """The pure joint solve on this pair walks the synthesized CMIF
    visibly away from the measurement — noise amplified into amplitude
    swaps between similar close modes. The ladder scores every
    candidate against the measured CMIF over the fitted bands, and the
    sequential answer is always on it, so refining cannot lose ground
    on the score the winner is chosen by."""
    session, _clean = _ill_conditioned_noisy_pair()
    before = _score_vs(session, session.matrix)
    session.refine_residues()
    assert _score_vs(session, session.matrix) <= before * 1.0000001


def test_refine_still_helps_where_it_can():
    """The guarantee must not come at the price of doing nothing.
    Without the garbage channel this is ordinary noisy data with a
    coupled close pair, and the refine visibly improves the measured
    CMIF *and* — which the synthetic fixture can check — the match to
    the clean truth."""
    session, clean = _ill_conditioned_noisy_pair(bad_channel=False)
    measured_before = _score_vs(session, session.matrix)
    truth_before = _score_vs(session, clean)
    session.refine_residues()
    assert _score_vs(session, session.matrix) < measured_before * 0.8, (
        'the improvement is visible on the curve the user watches')
    assert _score_vs(session, clean) < truth_before * 0.8, (
        'and it is real, not an artifact of hugging the noise')


def _coherence_for(session, bad=3, good=0.98, poor=0.25):
    from hertzforge.core.data import MultipleCoherence

    gamma = np.full((len(session.responses), len(session.frequencies)),
                    good)
    gamma[bad] = poor
    return MultipleCoherence(session.frequencies, gamma,
                             response_dof=session.responses)


def test_coherence_becomes_least_squares_weights():
    """The variance of an FRF estimate goes as (1 - g2)/g2; the weight
    is its inverse, capped and normalized to a maximum of one. A
    response the coherence does not cover weighs 1.0 — no statement is
    not a bad statement."""
    session, _clean = _ill_conditioned_noisy_pair(bad_channel=False)
    coherence = _coherence_for(session)
    weighted = ModalFitSession(session.frf, coherence=coherence)
    assert weighted.weights is not None
    assert weighted.weights.shape == (len(session.frequencies), 8)
    assert weighted.weights.max() == pytest.approx(1.0)
    assert np.all(weighted.weights[:, 3] < 0.01), (
        'a quarter-coherent channel is worth about a hundredth of a '
        'believed one')

    from hertzforge.core.data import MultipleCoherence

    elsewhere = MultipleCoherence(
        session.frequencies,
        np.full((2, len(session.frequencies)), 0.9),
        response_dof=['901X+', '902X+'])
    assert ModalFitSession(session.frf,
                           coherence=elsewhere).weights is None, (
        'a coherence covering none of the responses says nothing')


def _weighted_score(session):
    """The refine's own currency when coherence is present: the CMIF
    of the trust-scaled matrices, compared over the fitted bands."""
    band = _union_band(session)
    trust = np.sqrt(session.weights[band])[:, :, None]
    s_measured = np.linalg.svd(session.matrix[band] * trust,
                               compute_uv=False)[:, 0]
    s_synth = np.linalg.svd(session.synthesis_matrix()[band] * trust,
                            compute_uv=False)[:, 0]
    return float(np.linalg.norm(s_synth - s_measured))


def test_coherence_guards_the_guarantee_in_its_own_currency():
    """With coherence present, the refine solves, judges and
    guarantees in the trust-weighted world: the channel the coherence
    disbelieves stops counting as something worth hugging, and the
    never-worse guarantee holds on the weighted score. What the weights
    buy on real surveys is measured there — on the drone data the
    weighted clustered refine improved every fitted peak — and what a
    synthetic garbage channel can still cost, unseen by any
    measured-data score, is bounded by the ladder's sequential rung."""
    session, _clean = _ill_conditioned_noisy_pair()     # bad channel
    weighted = ModalFitSession(session.frf,
                               coherence=_coherence_for(session))
    for mode in session.modes:
        weighted.confirm(frequency=mode['frequency'],
                         damping=mode['damping'])
    before = _weighted_score(weighted)
    weighted.refine_residues()
    assert _weighted_score(weighted) <= before * 1.0000001


def _single_mode_frf(noise=0.02, seed=1):
    from hertzforge.core.data import Frf

    rng = np.random.default_rng(seed)
    frequencies = np.linspace(10.0, 100.0, 1801)
    omega = 2.0 * np.pi * frequencies
    phi = rng.standard_normal(8)
    phi /= np.linalg.norm(phi)
    omega_r = 2.0 * np.pi * 50.0
    q = ((1j * omega) ** 2
         / (omega_r ** 2 - omega ** 2 + 2j * 0.02 * omega_r * omega))
    responses = [f'{n}Z+' for n in range(1, 9)]
    rows, rdofs, refdofs = [], [], []
    for ref in (0, 4):
        for j in range(8):
            h = phi[j] * phi[ref] * q
            level = np.median(np.abs(h))
            h = h + noise * level * (rng.standard_normal(len(h))
                                     + 1j * rng.standard_normal(len(h)))
            rows.append(h)
            rdofs.append(responses[j])
            refdofs.append(responses[ref])
    return Frf(frequencies, np.array(rows), response_dof=rdofs,
               reference_dof=refdofs)


def _peak_ratio(session):
    """Synthesis over measurement, on the CMIF, at the tallest line."""
    measured = np.linalg.svd(session.matrix, compute_uv=False)[:, 0]
    index = int(np.argmax(measured))
    synthesis = np.linalg.svd(session.synthesis_matrix(),
                              compute_uv=False)[:, 0]
    return float(synthesis[index] / measured[index])


def test_the_fit_keeps_the_parabolas_promise():
    """The user drags the parabola until its crown sits on the CMIF and
    fits — and the synthesis used to land 15–25 % below that crown
    whenever the held damping was wider than the data's, because the
    band least squares traded the peak away to the shoulders. With the
    damping *held by hand*, the residues are scaled so the mode's CMIF
    meets the residual CMIF at the fitted line: what was aimed is what
    appears."""
    session = ModalFitSession(_single_mode_frf())
    session.move_to(50.0, damping=False)
    session.override_damping(0.04)                  # held 2x too wide
    session.confirm()
    assert _peak_ratio(session) == pytest.approx(1.0, abs=0.02), (
        'a mismatched damping must not push the synthesis under the peak')


def test_an_automatic_fit_never_pins():
    """The pin only knows what the user meant; an automatic fit has no
    user meaning to honor, and pinning it was the runaway that stacked
    fourteen inflated modes at one frequency of a dense cluster. Without
    a held damping, the least squares answer stands — under a
    deliberately wrong damping it undershoots, and that honesty is the
    point."""
    session = ModalFitSession(_single_mode_frf())
    session.confirm(frequency=50.0, damping=0.04)   # wrong, but not held
    assert _peak_ratio(session) < 0.9, (
        'the LSQ answer, unpinned: it only claims what its direction '
        'explains')


def test_the_promise_costs_nothing_when_the_damping_is_right():
    session = ModalFitSession(_single_mode_frf())
    session.move_to(50.0, damping=False)
    session.override_damping(0.02)
    session.confirm()
    assert _peak_ratio(session) == pytest.approx(1.0, abs=0.01)


def test_a_confirmed_peak_is_not_offered_again():
    """The subtraction of a fitted mode is exact only when the mode is
    alone; in a cluster it leaves ridge enough that the same line stays
    the tallest, and the loop once confirmed fourteen modes at one
    frequency. A confirmed mode's half-power width is spoken for."""
    from hertzforge.core.data import Frf

    rng = np.random.default_rng(3)
    frequencies = np.linspace(5.0, 120.0, 2301)
    omega = 2.0 * np.pi * frequencies
    responses = [f'{n}Z+' for n in range(1, 7)]
    # a dense cluster of four similar close modes, plus two isolated
    # modes the loop must still reach
    poles = [(50.0, 0.02), (51.5, 0.02), (53.0, 0.02), (54.5, 0.02),
             (20.0, 0.01), (90.0, 0.01)]
    shapes = [rng.standard_normal(6) for _ in poles]
    rows = []
    for j in range(6):
        h = sum(phi[j] * phi[0]
                * ((1j * omega) ** 2
                   / ((2 * np.pi * f) ** 2 - omega ** 2
                      + 2j * z * (2 * np.pi * f) * omega))
                for (f, z), phi in zip(poles, shapes))
        rows.append(h)
    frf = Frf(frequencies, np.array(rows), response_dof=responses,
              reference_dof=[responses[0]] * 6)
    session = ModalFitSession(frf)
    for _ in range(6):
        session.confirm()
        session.suggest()
    fitted = sorted(m['frequency'] for m in session.modes)
    # The disease was fourteen modes at ONE LINE, and the guard is
    # against that: every confirm lands on its own line, more than two
    # line spacings from any other. Not a fixed 0.3 Hz, which an
    # earlier version asserted and which pinned trajectory luck rather
    # than quality (2026-08-28): on this deliberately cruel cluster —
    # four modes, two ever found by anyone — the estimator before the
    # frequency walk spent its sixth confirm on a z=0.005 sliver at
    # 49.37 Hz, and the one after it on a z=0.017 fit at 51.28, inside
    # the first blend's own claim, where the ridge test had said the
    # leftover shape belonged to the unfound modes. Both sixths are
    # honest leftovers of a cluster neither resolves; only their
    # addresses differ, and 0.3 Hz was a ruling about the address.
    df = frequencies[1] - frequencies[0]
    for a, b in itertools.pairwise(fitted):
        assert b - a > 2.0 * df, f'stacked at one line: {a:.2f}/{b:.2f}'
    assert any(f < 25.0 for f in fitted), 'the isolated 20 Hz mode is fit'
    assert any(f > 85.0 for f in fitted), 'and the 90 Hz one'
    assert sum(45.0 < f < 60.0 for f in fitted) >= 3, (
        'the cluster is worked through tooth by tooth, not abandoned')


def test_a_repeated_roots_second_mode_is_still_offered():
    """The half-power exclusion cannot tell the ridge a subtraction
    leaves from a genuine neighbor standing at the same frequency — a
    symmetric structure's repeated root is exactly that, and the plate
    demo's 1142 Hz pair was shadowed by its own first half. Shape can
    tell them apart: ridge lies in the span of the confirmed modes, a
    real neighbor does not. The loop must take both teeth of the pair
    and then leave — not skip the second, and not stack a third.

    The pair is *exactly* repeated, which is the regime that matters: a
    split wider than the fitted width escapes the old exclusion on
    frequency alone, and a first version of this test passed with the
    shape test stubbed out for exactly that reason. The plate's pair is
    split 0.4 Hz at 1142 Hz — beneath its 2 Hz line spacing — and only
    shape can reach the second tooth of that."""
    from hertzforge.core.data import Frf

    rng = np.random.default_rng(11)
    frequencies = np.linspace(5.0, 120.0, 2301)
    omega = 2.0 * np.pi * frequencies
    responses = [f'{n}Z+' for n in range(1, 7)]
    # a repeated root, orthogonal shapes, plus one isolated mode; two
    # references so both halves of the pair are excited
    poles = [(50.0, 0.02), (50.0, 0.02), (90.0, 0.01)]
    shapes = [rng.standard_normal(6) for _ in poles]
    shapes[1] -= (shapes[1] @ shapes[0]) / (shapes[0] @ shapes[0]) \
        * shapes[0]
    rows, response_dof, reference_dof = [], [], []
    for ref in (0, 1):
        for j in range(6):
            h = sum(phi[j] * phi[ref]
                    * ((1j * omega) ** 2
                       / ((2 * np.pi * f) ** 2 - omega ** 2
                          + 2j * z * (2 * np.pi * f) * omega))
                    for (f, z), phi in zip(poles, shapes))
            rows.append(h)
            response_dof.append(responses[j])
            reference_dof.append(responses[ref])
    frf = Frf(frequencies, np.array(rows), response_dof=response_dof,
              reference_dof=reference_dof)
    session = ModalFitSession(frf)
    for _ in range(3):
        session.confirm()
        session.suggest()
    fitted = sorted(m['frequency'] for m in session.modes)
    # *on* the root, both of them: without the shape test the loop still
    # reaches the second tooth eventually — via the local maximum the
    # exclusion boundary manufactures on the skirt, a full linewidth off
    # the true frequency (51.0 for 50.0 on this data)
    assert sum(abs(f - 50.0) < 0.2 for f in fitted) == 2, (
        f'both teeth of the pair, at the root: {fitted}')
    assert any(f > 85.0 for f in fitted), f'and the 90 Hz mode: {fitted}'
    # the two pair fits are the two orthogonal shapes, not one twice
    pair = [m for m in session.modes if 49.0 < m['frequency'] < 51.5]
    assert mac(pair[0]['shape'][:6].real,
               pair[1]['shape'][:6].real) < 0.1, (
        'the second fit found the other shape')


def test_refine_leaves_distant_modes_alone():
    """An accelerance SDOF term tends to a constant far above its pole
    — the mass line — so in a *global* joint solve a low mode's basis
    column reaches across every higher cluster's lines, and the refine
    once inflated a 64 Hz fundamental fivefold to shim milli-errors
    across a 1400 Hz cluster. Modes couple through the solve only when
    their bands overlap; a distant mode's amplitude is not the
    cluster's to spend."""
    from hertzforge.core.data import Frf

    rng = np.random.default_rng(5)
    frequencies = np.linspace(5.0, 1500.0, 6001)
    omega = 2.0 * np.pi * frequencies
    responses = [f'{n}Z+' for n in range(1, 7)]
    poles = [(60.0, 0.01), (900.0, 0.02), (905.0, 0.02), (910.0, 0.02)]
    shapes = [rng.standard_normal(6) for _ in poles]
    # the drone's disparity, exaggerated to where the disease is vivid:
    # a very quiet fundamental under a loud cluster — the configuration
    # where a globally judged solve bought a 7.7x shim
    shapes = [shapes[0] * 0.1] + [phi * 6.0 for phi in shapes[1:]]
    rows = []
    for j in range(6):
        h = sum(phi[j] * phi[0]
                * ((1j * omega) ** 2
                   / ((2 * np.pi * f) ** 2 - omega ** 2
                      + 2j * z * (2 * np.pi * f) * omega))
                for (f, z), phi in zip(poles, shapes))
        level = np.median(np.abs(h))
        h = h + 0.005 * level * (rng.standard_normal(len(h))
                                 + 1j * rng.standard_normal(len(h)))
        rows.append(h)
    frf = Frf(frequencies, np.array(rows), response_dof=responses,
              reference_dof=[responses[0]] * 6)
    session = ModalFitSession(frf)
    for f, z in poles:
        session.confirm(frequency=f, damping=z)
    truth = float(np.linalg.norm(shapes[0]))
    low_before = next(np.linalg.norm(m['shape'])
                      for m in session.modes
                      if m['frequency'] < 100.0)
    session.refine_residues()
    low_after = next(np.linalg.norm(m['shape'])
                     for m in session.modes if m['frequency'] < 100.0)
    # untouched, exactly: a cluster of one has nobody to borrow from,
    # and every refit estimator tried on it — global, locally judged —
    # absorbed the loud cluster's tails or the noise into the quiet
    # mode. The sequential answer stands for solo modes.
    assert low_after == low_before, (
        'a lone mode is not refit — there is nothing to un-borrow')
    # the sequential answer itself is imperfect on so extreme a
    # disparity — the guarantee is that refine does not make it *worse*,
    # which every refit estimator tried here did
    assert abs(low_after - truth) <= abs(low_before - truth)
    del truth


def test_suggestions_rank_by_prominence_not_height():
    """Raw height and the eye disagree on accelerance: a broad tall
    high-frequency mode out-measures a sharp small fundamental by
    height, but the fundamental is a needle above its floor and the
    broad mode barely rises above its own shoulders. Ranked by height,
    ten Confirms on a real survey never visited the modes its engineer
    fit first; ranked by prominence they reproduced that hand-picked
    set exactly."""
    from hertzforge.core.data import Frf

    rng = np.random.default_rng(2)
    frequencies = np.linspace(5.0, 1500.0, 3001)
    omega = 2.0 * np.pi * frequencies
    responses = [f'{n}Z+' for n in range(1, 7)]
    poles = [(50.0, 0.005, 1.0), (1000.0, 0.08, 60.0)]
    rows = []
    for j in range(6):
        h = sum(a * ((1j * omega) ** 2
                     / ((2 * np.pi * f) ** 2 - omega ** 2
                        + 2j * z * (2 * np.pi * f) * omega))
                for f, z, a in poles) * (0.5 + 0.1 * j)
        level = np.median(np.abs(h))
        h = h + 0.02 * level * (rng.standard_normal(len(h))
                                + 1j * rng.standard_normal(len(h)))
        rows.append(h)
    frf = Frf(frequencies, np.array(rows), response_dof=responses,
              reference_dof=[responses[0]] * 6)
    session = ModalFitSession(frf)
    s1 = session._residual_cmif()
    i50 = int(np.argmin(np.abs(session.frequencies - 50.0)))
    i1000 = int(np.argmin(np.abs(session.frequencies - 1000.0)))
    assert s1[i1000 - 40:i1000 + 41].max() > 3 * s1[i50 - 5:i50 + 6].max(), (
        'the fixture must make height and prominence disagree')
    assert session.pending['frequency'] == pytest.approx(50.0, abs=2.0), (
        'the needle above its floor comes first, as an engineer reads it')


def test_every_fitted_mode_replicates_the_cmif_peak():
    """The plate's own survey: the 647 Hz mode sits on one shaker's
    node, and that drive point's noise-signed scale vote averaged the
    synthesis down to half the measured CMIF. Weighted by where the
    mode actually lives, every peak replicates."""
    from conftest import fixture_path

    import hertzforge

    frf = hertzforge.import_file(fixture_path('plate', 'frfs.npz'))
    session = ModalFitSession(frf)
    for _ in range(4):
        session.confirm()
        session.suggest()
    measured = np.linalg.svd(session.matrix, compute_uv=False)[:, 0]
    synth = np.linalg.svd(session.synthesis_matrix(),
                          compute_uv=False)[:, 0]
    for mode in session.modes:
        i = int(np.argmin(np.abs(session.frequencies
                                 - mode['frequency'])))
        ratio = synth[i] / measured[i]
        assert 0.9 < ratio < 1.1, (
            f"{mode['frequency']:.1f} Hz synthesizes {ratio:.3f} of "
            'the measured CMIF')


def _drive_point_session():
    """Three responses, two references, both drive points."""
    from hertzforge.core.data import Frf

    frequencies = np.linspace(5.0, 100.0, 96)
    responses = ['1Z+', '2Z+', '3Z+']
    rows, rdof, fdof = [], [], []
    for ref in ('1Z+', '2Z+'):
        for dof in responses:
            rows.append(np.ones_like(frequencies, dtype=complex))
            rdof.append(dof)
            fdof.append(ref)
    return ModalFitSession(Frf(frequencies, np.array(rows),
                               response_dof=rdof, reference_dof=fdof))


def test_scale_votes_are_weighted_by_where_the_mode_lives():
    """A drive point near a node has no standing on scale: its junk
    ratio must not drag the answer, discarded when its sign self-
    declares and out-weighted regardless."""
    session = _drive_point_session()
    direction = np.array([0.995, 0.1, 0.0])
    direction = direction / np.linalg.norm(direction)
    # ratios: +8 at the strong drive, -6 at the nodal one
    residues = np.array([8.0 * direction[0], -6.0 * direction[1]])
    shape, scaled = session._shape_from(direction, residues)
    assert scaled
    assert np.allclose(shape[:3], np.sqrt(8.0) * direction), (
        'the strong drive point alone pins the scale')


def test_a_junk_vote_is_discarded_not_averaged():
    """Equal weights, opposite signs: the negative is a physical
    impossibility (a drive-point residue is phi squared) and is
    discarded, not blended into the mean."""
    session = _drive_point_session()
    direction = np.array([1.0, 1.0, 0.0]) / np.sqrt(2.0)
    residues = np.array([-6.0 * direction[0], 8.0 * direction[1]])
    shape, scaled = session._shape_from(direction, residues)
    assert scaled
    assert np.allclose(shape[:3], np.sqrt(8.0) * direction), (
        'discarded means gone, not averaged down to sqrt(1)')


def test_an_all_negative_vote_is_a_sign_convention():
    """A shape is defined up to sign: every drive point voting negative
    means the SVD's sign was against the residues, and the fit flips
    and scales normally rather than discarding everything."""
    session = _drive_point_session()
    direction = np.array([0.8, 0.6, 0.0])
    residues = np.array([-9.0 * 0.8, -11.0 * 0.6])
    shape, scaled = session._shape_from(direction, residues)
    assert scaled
    assert shape[0] < 0, 'the flip happened'
    weighted = (9.0 * 0.64 + 11.0 * 0.36) / (0.64 + 0.36)
    assert np.allclose(np.abs(shape[:3]),
                       np.sqrt(weighted) * direction)


def test_no_usable_vote_publishes_unscaled():
    """Every drive point on a node: nothing has standing, the residue
    magnitudes stand in, and the mode says so."""
    session = _drive_point_session()
    direction = np.array([0.0, 0.0, 1.0])
    residues = np.array([2.0, 3.0])
    _shape, scaled = session._shape_from(direction, residues)
    assert not scaled, 'a convention, not physics — and it says so'


def test_fit_modes_trusts_the_sessions_own_judgment():
    """The scripted loop adds no second opinion: a proximity guard here
    once vetoed any suggestion within 1 Hz of a confirmed mode, which
    silently skipped the repeated pair's second tooth the session had
    deliberately offered. limit=5 on the plate takes all five,
    both teeth of the 1142 Hz pair included."""
    from conftest import fixture_path

    import hertzforge

    project = hertzforge.Project('check')
    project.add('FRF', hertzforge.import_file(
        fixture_path('plate', 'frfs.npz')))
    fitted = project[project.fit_modes('FRF', bounds=(300.0, 1300.0),
                                       limit=5)]
    frequencies = sorted(np.atleast_1d(fitted.frequency))
    assert len(frequencies) == 5
    teeth = [f for f in frequencies if 1100 < f < 1200]
    assert len(teeth) == 2, f'both teeth of the pair: {frequencies}'
    assert abs(teeth[1] - teeth[0]) < 1.5


def test_the_factored_cmif_equals_the_materialized_one(survey):
    """The dashed synthesis CMIF used to be drawn by materializing the
    whole synthesis — a two-gigabyte array on the hard drone survey,
    6.5 s per redraw — and decomposing every line. The factored path
    computes the same singular values from the QR of the shape
    factors. Same numbers, or it is a different curve on the screen."""
    _truth, frfs = survey
    session = ModalFitSession(frfs)
    for _ in range(3):
        session.confirm()
        session.suggest()
    session.fit_pending()        # a preview too, the state a drag is in

    fast = session.synthesis_singular_values()
    modes = list(session.modes) + [session.preview]
    slow = np.zeros_like(session.matrix)
    for mode in modes:
        slow += session._mode_term(mode)
    reference = np.linalg.svd(slow, compute_uv=False).T
    assert fast.shape == reference.shape
    scale = reference.max()
    assert np.allclose(fast, reference, atol=1e-9 * scale), (
        'the factored singular values are not the materialized ones')


def test_the_factored_cmif_pads_a_thin_fit_to_full_width(survey):
    """One confirmed mode is rank one, but the materialized SVD reports
    min(responses, references) singular values — the extras exactly
    zero. The plot counts rows, so the factored path pads."""
    _truth, frfs = survey
    session = ModalFitSession(frfs)
    session.confirm()
    singular = session.synthesis_singular_values(include_preview=False)
    assert singular.shape[0] == min(len(session.responses),
                                    len(session.references))
    assert np.all(singular[1:] < 1e-9 * singular.max()), \
        'everything past the first row is the padding'


def test_no_modes_means_no_curve():
    from conftest import fixture_path

    import hertzforge

    frfs = hertzforge.import_file(fixture_path('plate', 'frfs.npz'))
    session = ModalFitSession(frfs)
    assert session.synthesis_singular_values() is None


def test_the_fit_weights_by_the_coherence_covering_most_linked_first():
    """`Project.fit_coherence` is the fitting screen's rule, so a script
    asks the same question: the coherence covering most of the fit's
    responses, one in the FRF's own group winning a tie."""
    from conftest import fixture_path

    import hertzforge

    project = hertzforge.Project('Weights')
    project.import_file(fixture_path('plate', 'modal_spectra.nc4'))
    assert project.fit_coherence('FRF') == 'Multiple Coherence'
    project.unlink('Multiple Coherence')     # the import linked the run
    twin, = project.duplicate('Multiple Coherence')
    assert project.fit_coherence('FRF') == 'Multiple Coherence', \
        'a tie with neither linked keeps the first'
    project.link('FRF', twin)
    assert project.fit_coherence('FRF') == twin, 'the linked one wins a tie'
    frf = project['FRF']
    only = frf.response_dof[0]
    project[twin].delete_records(
        [i for i, dof in enumerate(project[twin].response_dof)
         if dof != only])
    assert project.fit_coherence('FRF') == 'Multiple Coherence', \
        'coverage outranks the link'
    assert project.fit_coherence('FRF', records=[0]) == twin, \
        'judged over the fitted records only'


def test_a_mode_claims_at_least_its_own_line():
    """A confirmed mode whose damping collapsed to the search's least
    claimed less than a line, so a one-line spike was offered and
    confirmed again and again. On the plate's FRFs, 2 Hz apart, a line
    planted forty times its neighbors at 530 Hz took all five of five
    automatic modes at 0.01 %; with each mode claiming at least half a
    line either side, the loop moves on to the plate's own modes."""
    from conftest import fixture_path

    import hertzforge

    frf = hertzforge.import_file(fixture_path('plate', 'frfs.npz'))
    lines = np.asarray(frf.abscissa)
    spike = int(np.argmin(np.abs(lines - 530.0)))
    frf.ordinate = np.array(frf.ordinate, copy=True)
    frf.ordinate[:, spike] *= 40.0
    project = hertzforge.Project()
    project.add('FRF', frf)
    found = project[project.fit_modes('FRF', bounds=(300.0, 1300.0),
                                      limit=5)].frequency
    elsewhere = [f for f in found if abs(f - lines[spike]) > 2.0]
    assert len(elsewhere) >= 3, found
    session = ModalFitSession(frf)
    narrow = {'frequency': 530.3, 'damping': 1e-4}
    assert session._claim(narrow) == pytest.approx(1.0)
    wide = {'frequency': 530.3, 'damping': 0.02}
    assert session._claim(wide) == pytest.approx(0.02 * 530.3)
