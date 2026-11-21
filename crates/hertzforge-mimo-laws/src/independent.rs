//! Laws 7 and 8: independent drives, and nearly independent drives.
//!
//! **Independent drives** ([`IndependentDrivesLaw`]): the drive matrix is
//! diagonal on every line. With uncorrelated drives each control's
//! autospectrum is the sum of the drive autospectra weighted by the squared
//! magnitudes of the frequency response, `y = A g` with `A_kd = |H_kd|^2`, a
//! real system. The seed solves it for the drive autospectra `g` from the
//! reference autospectra; each loop then adds the solve of the error between
//! the reference and the measured autospectra to the levels last played, with
//! a limit on the change per loop.
//!
//! **Nearly independent drives** ([`NearlyIndependentLaw`]): the reference
//! keeps its specified autospectra and takes every pair's coherence and phase
//! from the response the independent drives would produce through the
//! frequency response. The default law's closed loop ([`crate::factor`]) then
//! holds that reference, as `buzz_match` and `min_drive` hold theirs.
//!
//! By default both laws carry N. Hunter's refinements (see "Refinements by
//! N. Hunter" below), except the weighting by the target, which is off by
//! default in `independent` (Choice 21); each one's parameter sets the law
//! back to the published method. `nearly_independent` also has an open form, his law as his note
//! runs it: the same drive on every call, with no correction from the
//! response.
//!
//! # Sources
//!
//! - R. L. Mayes and D. P. Rohe, "Physical Vibration Simulation of an Acoustic
//!   Environment with Six Shakers on an Industrial Structure", IMAC XXXIV
//!   (2016), DOI 10.1007/978-3-319-30087-0_4: the real system of squared
//!   magnitudes for uncorrelated drives (Eqs. 2 and 3), its Tikhonov
//!   pseudo-inverse `S+ = (S^T S + c I)^-1 S^T` (Eq. 4) with `c` 0.01 times the
//!   largest diagonal element of `S^T S` (section 7.2), the first solve (Eq. 5),
//!   the feedback iteration `|v_d|^2 = |v_i|^2 + S+ (|x_d|^2 - |x_i|^2)` (Eq. 6)
//!   and its limit of +100 and -67 percent per iteration (section 7.4).
//! - R. Schultz, "A Demonstration of Force Estimation and Regularization
//!   Methods for Multi-Shaker Testing", IMAC XXXVII (2019), DOI
//!   10.1007/978-3-030-12676-6_21, section 6.3: independent drives from the
//!   target autospectra (Eq. 15, read as the element-wise product with a
//!   pseudo-inverse, as Mayes and Rohe state it), negative levels set to zero,
//!   and the rescale of the target from the estimated responses followed by a
//!   second solve, credited to D. O. Smallwood.
//! - J. S. Cap, S. Norman and D. O. Smallwood, "The Derivation of
//!   Multiple-Input-Multiple Output (MIMO) Acoustic Test Specifications to
//!   Simulate a Missile Flight", 86th Shock and Vibration Symposium (2015),
//!   Appendix A, the independent drive algorithm: the independent-drive
//!   response `S_YYI = H S_XXI H'` (Eq. 4), its rescale to the target
//!   autospectra keeping its phase and coherence (prose, p. 12), and the drive
//!   that produces the rescaled response by a regularized pseudo-inverse
//!   (Eq. 5). They credit the method to D. O. Smallwood, "A Revised Algorithm
//!   for Minimum Input Trace to a Multiple-Input / Multiple-Output System
//!   (MIMO) to Match the Output Autospectral Densities", 85th Shock and
//!   Vibration Symposium (2014), which was not available to read.
//! - N. Hunter, K. Cross and G. Nelson, "The Cross Spectrum in Multiple Input
//!   Multiple Response Vibration Testing", IMAC XXXVI (2018), DOI
//!   10.1007/978-3-319-74700-2_10: the name "nearly independent drive
//!   algorithm", the diagonal solve (Eq. 11) and the rule that a drive whose
//!   solved autospectrum is negative is set to zero (p. 6).
//!
//! Hunter, Cross and Nelson print the second stage for a single spectrum
//! vector (Eqs. 12 to 15). As printed those equations scale by the trial over
//! the desired autospectra, apply the scale twice, and give a rank-one response
//! of unit coherence; they are not built here. The spectral-density form of the
//! same idea, Cap, Norman and Smallwood's rescale of the whole response
//! matrix, is the one implemented.
//!
//! # Refinements by N. Hunter
//!
//! The refinements below are N. Hunter's, from his unpublished note "The
//! Independent Drive Law, Step by Step" (18 November 2026), a private
//! communication, and are used with his permission. No published paper gives
//! them. They are the defaults, except the weighting in `independent`
//! (Choice 21); the parameter named after each sets it back to the published
//! form.
//!
//! 1. **Weighting by the target** (his step 2). Each control's row of `A` and
//!    its target autospectrum are divided by that target, so every control's
//!    error counts in proportion to its own level, and a control with a zero
//!    target takes no part in the solve on that line. Both laws declare it;
//!    the default in `nearly_independent`, off by default in `independent`
//!    (Choice 21). `solve_weighting` `none` is the published form.
//! 2. **Non-negative least squares** (his steps 3 and 4). Where the plain
//!    solve gives a negative level, the levels are the minimizer of the same
//!    objective over levels of zero or more, instead of the plain levels with
//!    the negative ones set to zero. Both laws; `solve` `set_to_zero` is the
//!    published form.
//! 3. **The exactness test** (his step 5). Where the plain least-squares
//!    levels with no regularization are all zero or more and leave a relative
//!    residual at or below 1e-10, independent drives meet the reference
//!    exactly: those are the line's levels, and the loop starts there from the
//!    diagonal drive where it can hold it (Choice 11). `nearly_independent`
//!    only; `exact_tolerance` 0 is the
//!    published form.
//! 4. **The level set** (his step 10). On a line that is not exact, the
//!    first drive is scaled by one number, the sum of the targets over the sum
//!    of the autospectra the frequency response predicts for it, so the total
//!    predicted power equals the total specified. In the closed loop it sets
//!    only the first drive's level; the loop then sets the level from the
//!    response. `nearly_independent` only; `level_set` off is the published
//!    form.
//!
//! His note's law is open loop: after the first command it sends the same
//! drive on every call. `loop` `open` is that law (Choice 20); `closed`, the
//! default, closes the default law's loop around the same reference.
//!
//! # Choices This Implementation Makes
//!
//! Where the sources leave a step open, it is decided by Hertzforge's own
//! conventions:
//!
//! 1. **The solve.** `S+` is formed per line from the singular value
//!    decomposition of `A = |H|^2`, `S+ = V diag(s / (s^2 + c)) U^T`, which is
//!    Mayes and Rohe's Eq. 4 exactly for `c > 0`. Singular values at or below
//!    `max(rows, cols) * eps * s_max` are dropped first, the numerical floor of
//!    `hertzforge_sdm::linalg::pinv`, so with `c = 0` it is the minimum-norm
//!    least-squares pseudo-inverse and a rank-deficient or all-zero system has a
//!    defined result. A control no drive reaches (a zero row) adds nothing; a
//!    drive that reaches no control (a zero column) gets level zero, to
//!    rounding (under 1e-12 of the line's largest level).
//! 2. **The constant per line.** Each line is solved on its own, as Mayes and
//!    Rohe process each line separately. With `solve_weighting` `none`, `c` is
//!    the parameter times the largest diagonal element of `A^T A` on the line,
//!    as published. With `target`, the term is scaled drive by drive: drive
//!    `d`'s is the parameter times its own diagonal element `(A^T A)_dd` of
//!    the weighted matrix, the scaling of D. W. Marquardt ("An Algorithm for
//!    Least-Squares Estimation of Nonlinear Parameters", J. SIAM 11, 1963),
//!    which is ridge regression on columns of equal size. Divided by their
//!    targets, the columns differ by the square of the level each drive needs,
//!    so one constant for the line over-regularizes every drive that needs a
//!    high level; scaled, every drive is regularized by the same relative
//!    amount (on a decoupled structure, every level is short by `c / (1 + c)`
//!    at the seed and by that factor less on each loop). A drive whose column
//!    is zero has no term and gets level zero.
//! 3. **The negative rule** (with `solve` `set_to_zero`). A negative solved
//!    level is set to zero, the rule of Hunter, Cross and Nelson and of
//!    Schultz. Mayes and Rohe set it to "a
//!    small positive value" and give no value; `negative_floor` sets it to that
//!    fraction of the line's largest solved level, 0 by default. The rule is
//!    applied once, with no second solve over the remaining drives.
//! 4. **The levels last played.** The loop starts each step from the diagonal
//!    of the drive the core played last loop (the `last_drive` input), after the
//!    core's change cap, ceilings and limit channels, as `band_average` does:
//!    Eq. 6's "last voltage autospectrum" is what was played, and a level the
//!    core scaled down is not assumed to have been played at the law's value.
//! 5. **The change limit.** Mayes and Rohe "did not let the voltage at a
//!    specific line change more than +100 percent, -67 percent" (7.4). The
//!    page's word is voltage; the limit is applied here to the autospectrum,
//!    the quantity Eq. 6 updates: +100 / -67 percent of the autospectrum is
//!    +3.0 / -4.8 dB. Read as amplitude it would be +300 / -89 percent of the
//!    autospectrum (+6.0 / -9.6 dB), looser than the controller's own per-loop
//!    cap of +3.5 / -6 dB, so the core's cap would govern. Either reading can
//!    be set: `limit_up_percent` 300 and `limit_down_percent` 89 are inside
//!    their ranges. Each drive's autospectrum on each line may rise by at most
//!    `limit_up_percent` and fall by at most `limit_down_percent` from its level
//!    played last loop, applied after the negative rule, on every loop but the
//!    seed ("on subsequent iterations"). A rise is measured from at least
//!    60 dB below that drive's in-band mean as played, the floor of the
//!    controller's own change cap; with no floor a level played at zero would
//!    have no base to measure a rise from and would rise unlimited. So a level
//!    zeroed on one line while its drive is non-zero elsewhere comes back by
//!    doubling once a loop from the floor, about 19 loops for 60 dB, at the
//!    default limit; `negative_floor` above zero (Mayes and Rohe's "small
//!    positive value") shortens this. A drive that is zero on every in-band
//!    line has no level to measure a rise from: the law applies no rise limit
//!    to it, and the core's change cap governs what is played.
//! 6. **The step.** The default, `convergence` 1, is Eq. 6 whole, the
//!    published step. The factor `e` in `g + e S+ (r - x)` is a Hertzforge
//!    addition, declared as every other built-in law declares one; no source
//!    gives one. The whole step passes measurement noise to the drive on any
//!    structure, because `A = |H|^2` has only non-negative entries and is
//!    often badly conditioned where `H` is not. On a feasible, well-conditioned
//!    3 x 3 structure with the test harness's 1 percent noise and the published
//!    solve and weighting, over loops 10 to
//!    30 the drive changed 1.70 dB a loop on average and up to 3.01 dB (the
//!    rise limit) while the worst control moved 0.11 dB; with `convergence`
//!    0.5 the drive changed 0.74 dB on average and up to 1.25 dB
//!    (`tests/independent.rs`). On an infeasible 3 x 3 structure with
//!    1 percent noise, over loops 10 to 30, the defaults (`solve`
//!    `nonnegative`, `solve_weighting` `none`) moved the worst control
//!    0.27 dB and the drive 0.45 dB a loop on average, against 0.30 dB and
//!    0.48 dB published. With both refinements (`solve_weighting` `target`
//!    as well), where levels are held at zero, the noise moves the loop more:
//!    0.63 dB and 2.65 dB, and the number of levels held at zero changed on
//!    4 of those 21 loops (on none in the other settings); with
//!    `convergence` 0.5, 0.41 dB and 1.43 dB (`tests/independent.rs`).
//! 7. **The frequency response.** Both laws use the frequency response the
//!    core hands them on each call. `S+` is formed again when it changes (the
//!    FRF updated during the run). The nearly independent reference is formed
//!    once, at the seed, from the frequency response handed then; later updates
//!    reach the closed loop, as for `min_drive`.
//! 8. **The reference.** Both laws read the specified reference's autospectra
//!    only; its cross terms are not used.
//! 9. **The trial response.** For the nearly independent reference the levels
//!    are solved as `solve` and `solve_weighting` say (with `set_to_zero`, the
//!    negative rule at zero; `negative_floor` is not declared by this law), the
//!    exactness test is applied, and the trial response
//!    `H diag(g) H^H` is formed as the Gram matrix of `H diag(sqrt(g))`, so it
//!    is positive semi-definite by construction, and so is its rescale, a
//!    congruence by a non-negative diagonal.
//! 10. **A zero trial autospectrum.** A control whose trial autospectrum is
//!     zero (no drive with a level reaches it) has no coherence to take: it
//!     keeps its specified autospectrum and is left independent of the others,
//!     as `buzz_match` treats a pair with no identification response. The
//!     result stays positive semi-definite.
//! 11. **The second solve and the loop.** Cap, Norman and Smallwood's Eq. 5 is
//!     the closed loop's seed, `H^+ G H^+^H`, with the default law's truncated
//!     pseudo-inverse (1e-3), its caps (+3.5 / -6 dB) and its rank rule (1e-6);
//!     they name a Tikhonov pseudo-inverse with no parameter, and no source
//!     says how to close the loop on the method. On an exact line the
//!     reference is the response of the exact levels, `H diag(g) H^H`, and the
//!     first drive is `diag(g)` itself, handed to the loop with
//!     `FactorLoop::seed_with`, wherever the loop can hold it: where its
//!     truncated pseudo-inverse keeps every drive direction (`H^+ H = I`: as
//!     many controls as drives or more, and nothing below the 1e-3
//!     truncation). Elsewhere (more drives than controls, or the inverse
//!     truncated on the line) the loop cannot hold a diagonal drive, and
//!     playing it first would change the drive at the next loop, by up to
//!     16 dB on a wide structure, with no change in the response; so in the
//!     closed loop the first drive there is the loop's own, the least drive
//!     that gives the same response, and the scalar diagnostic
//!     `exact_not_held` counts those lines. The open form plays `diag(g)` on
//!     every exact line (Choice 20). With no exact line and the level set off,
//!     the loop's own seed is used, as before.
//! 12. **Shapes.** Both laws take all three shapes: with more controls than
//!     drives the solve is least squares, with fewer it is regularized minimum
//!     norm; nothing in either method needs a square system.
//! 13. **Outside the band** every drive is zero and nothing is counted.
//! 14. **The weighting's reach.** With `solve_weighting` `target`, row `k` of
//!     `A` and its right side are divided by the target autospectrum `t_k`;
//!     a control with `t_k` zero gets a zero row and a zero right side. In the
//!     feedback step of `independent` the error `r - x` is weighted the same
//!     way, so the step solves the weighted system. Unlike the shared
//!     `weighting` of the default law, which leaves a square system's solution
//!     unchanged, this weighting changes the levels of a square system too
//!     wherever `c > 0` or a level is held at zero, so it has its own name.
//!     It also changes the trial levels, and so the coherence of the nearly
//!     independent reference: with more controls than drives the loop can
//!     settle further from the specified autospectra than the published form
//!     does, or closer. On the crate's 6 x 4 test structure the defaults
//!     settle 0.04 dB closer (3.134 against 3.174 dB, worst control, loop 30);
//!     on two other 6 x 4 structures, 0.52 dB further and 0.57 dB closer.
//! 15. **The non-negative objective.** The seed (and the trial levels) solve
//!     `min ||A g - t||^2 + sum_d c_d g_d^2` over `g >= 0`; each feedback step
//!     of `independent` solves `min ||A g - (A g_last + e W (r - x))||^2 +
//!     sum_d c_d (g_d - g_last,d)^2` over `g >= 0`, the objective whose
//!     unconstrained minimizer is the published step, with `W` the weighting
//!     and `c_d` the constant of Choice 2 (`c` for every drive unweighted).
//!     Both are posed as one
//!     least-squares system with the rows `diag(sqrt(c_d))` appended and handed to
//!     the active-set solver ([`nonnegative_least_squares`], Lawson and
//!     Hanson's method, written from the 1974 book). The solver runs only on a
//!     line whose unconstrained levels include a negative one, so elsewhere
//!     the levels are the published levels bit for bit. The change limit then
//!     applies as before; `negative_floor` has no effect with this solve.
//! 16. **The solver's bound.** At most `3 n` least-squares subproblems for
//!     `n` drives (Lawson and Hanson's bound), each by Householder QR. If the
//!     bound is reached, a subproblem's columns are dependent, an input is not
//!     finite (or its norm overflows), or a step toward a subproblem's
//!     solution is not finite, the line takes the published rule (negative
//!     levels to zero) and the scalar diagnostic `solve_fallbacks` counts it. The solver uses only arithmetic
//!     and square roots in a fixed order, so the law file takes the same steps
//!     as the native law (see the parity note in [`crate::pareto`]).
//! 17. **The exactness residual.** `||A g - t|| / ||t||`, with the weighting
//!     if it is on (then `t` is 1 for every control with a target, the
//!     note's `||A~ s - 1|| / sqrt(M)`), from the plain levels with `c = 0`,
//!     whatever the regularization constant: with any regularization the
//!     residual is never that small. The per-line diagnostic `exact` marks
//!     the lines; `held_at_zero` counts the levels the non-negative solve held
//!     at zero; `negative_levels` keeps its meaning, the levels of the plain
//!     solve below zero.
//! 18. **The level set's sums.** The targets are the specified autospectra
//!     and the predicted autospectra those of `H X H^H` through the whole
//!     estimate, not the loop's truncated inverse. On a square or wide
//!     structure of full rank every reference is reachable, the predicted
//!     power is the specified, and the factor is 1 to rounding: the level set
//!     acts where there are more controls than drives (or the inverse drops
//!     a direction). A zero or non-finite sum gives the factor 1, counted in
//!     `level_set_guarded`. The factor is reported per line as a ratio,
//!     `level_set_factor`, not in decibels: a factor within rounding of 1 has
//!     a logarithm that is all rounding, and the native law and the law file
//!     round differently (the reason `min_drive` reports a ratio too).
//! 19. **The level set in the closed loop.** The scaled drive is handed to the
//!     loop with `FactorLoop::seed_with`, which takes it as the drive last
//!     played (it lies in the loop's range, so it is held as given); from the
//!     next loop on the loop is the shared loop, unchanged.
//!     On a 6 x 4 structure with an exact estimate the run with the level set
//!     reaches the run without it to 4e-14 by loop 40 (`tests/level_set.rs`).
//! 20. **The open form.** On every call the law returns its own drive for the
//!     frequency response it is handed: the diagonal drive on an exact line,
//!     whether or not the closed loop could hold it, otherwise the loop's first drive `H^+ G H^+^H` scaled by the level set
//!     when that is on. It does not read the response. While the estimate is
//!     held, bit for bit, on the band, it returns the same drive (and reports
//!     a change of 0 dB); when the estimate changes, the reference and the
//!     drive are formed again from it. The open form does not correct from
//!     the response, so its error is whatever the model's error is; the
//!     core's level, limits, ceilings and aborts act on it as on any law.
//!     `convergence` has no meaning in it.
//! 21. **The weighting is off by default in `independent`.** Least squares
//!     on the relative error, where the targets cannot all be met, settles
//!     low, because an overshoot of a control costs more than the same
//!     undershoot. In N. Hunter's law the weighted solve gives only the trial
//!     drives, whose levels are then replaced by the specified autospectra
//!     (and set by his level step where that is not exact), so the bias does
//!     no harm; in `independent` the solved levels are the drive that is
//!     played, and nothing after the solve sets the level. On the benchmark's
//!     tuning structures, with the weighting on, `independent` ran 3 to 7 dB
//!     low on every case, and the non-negative solve without the weighting
//!     had the level right. This reading and the default are Hertzforge's,
//!     to be confirmed with the author; `nearly_independent` keeps his
//!     weighting by default.
//!
//! # Ideas Not Built
//!
//! - From N. Hunter's note: the rank rule by a gap in the singular values
//!   (his step 8), the coherence cap (step 9, off by its own default), the
//!   first-command ceiling (step 11) and the running ceiling of his open
//!   loop. The shared loop's rank rule (in both forms here) and the core's
//!   level, ceilings and start-up level own those.

use crate::factor::{FactorLoop, LoopSettings, Step};
use crate::pareto;
use crate::pinv::ALL_SHAPES;
use crate::spec::{DiagDecl, DiagKind, LawSpec, ParamDecl, ParamKindDecl, ParamVal};
use crate::support::{
    band_matrix, convergence_param, diag_clamped, diag_condition, diag_correction, diag_kept,
    gram_packed, largest_change_db, ZERO,
};
use crate::BuiltinMimoLaw;
use hertzforge_sdm::law::{
    DiagnosticData, DiagnosticValue, LawError, MimoControlLaw, MimoInput, MimoLoopInputs,
    MimoLoopOutputs, MimoSetup, MimoShape,
};
use hertzforge_sdm::linalg;
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

const NEEDS: [MimoInput; 1] = [MimoInput::LastDrive];

/// A rise is measured from at least this fraction of the drive's in-band mean
/// (-60 dB), the floor of the controller's own per-loop change cap.
const LIMIT_FLOOR: f64 = 1e-6;

// ---------------------------------------------------------------------------
// The mathematics
// ---------------------------------------------------------------------------

/// The regularized inverse `S+` of the squared-magnitude matrix on each
/// in-band line, drives by controls, real, row-major (`nb * nd * nc`).
///
/// `A_kd = |H_kd|^2` (Mayes and Rohe, Eq. 3); `S+ = (A^T A + C)^-1 A^T`
/// (their Eq. 4), formed from the singular value decomposition after
/// dropping singular values at or below the numerical floor.
///
/// Unscaled (`scaled` false), `C = c I` with `c = constant * max_d (A^T A)_dd`
/// (their section 7.2), and `S+ = V diag(s / (s^2 + c)) U^T`.
///
/// Scaled (`scaled` true, the weighting by the target), `C = constant *
/// diag(A^T A)`: each drive's own diagonal element (Marquardt's scaling). With
/// `N = diag(n_d)`, `n_d` the norm of column `d`, and `A' = A N^-1`,
/// `S+ = N^-1 (A'^T A' + constant I)^-1 A'^T`, the same formula on columns of
/// unit norm. A zero column has no term and a zero row of `S+`.
///
/// `a` holds the (weighted) squared magnitudes, real in the real parts. Also
/// returns each drive's constant on each line (`nb * nd`): the term
/// `C_dd` that multiplies `(g_d - prior_d)^2`.
fn inverse_of(
    a: &MatrixStack,
    constant: f64,
    scaled: bool,
) -> Result<(Vec<f64>, Vec<f64>), LawError> {
    let (nb, nc, nd) = a.dims();
    let mut norms = vec![1.0; nb * nd];
    let unit;
    let a_svd = if scaled {
        let mut u = a.clone();
        for i in 0..nb {
            let line = u.line_mut(i);
            for d in 0..nd {
                let n = (0..nc)
                    .map(|r| line[r * nd + d].re.powi(2))
                    .sum::<f64>()
                    .sqrt();
                norms[i * nd + d] = n;
                for r in 0..nc {
                    line[r * nd + d] = if n > 0.0 {
                        C64::new(line[r * nd + d].re / n, 0.0)
                    } else {
                        C64::new(0.0, 0.0)
                    };
                }
            }
        }
        unit = u;
        &unit
    } else {
        a
    };
    let svd = linalg::svd(a_svd)?;
    let k = svd.k();
    let mut plus = vec![0.0; nb * nd * nc];
    let mut constants = vec![0.0; nb * nd];
    for i in 0..nb {
        let line = a.line(i);
        let c = if scaled {
            for d in 0..nd {
                let n = norms[i * nd + d];
                constants[i * nd + d] = constant * n * n;
            }
            constant
        } else {
            let normal_max = (0..nd)
                .map(|d| (0..nc).map(|r| line[r * nd + d].re.powi(2)).sum::<f64>())
                .fold(0.0f64, f64::max);
            let c = constant * normal_max;
            constants[i * nd..(i + 1) * nd].fill(c);
            c
        };
        let s = &svd.s[i * k..(i + 1) * k];
        let smax = s.first().copied().unwrap_or(0.0);
        let floor = nc.max(nd) as f64 * f64::EPSILON * smax;
        let (u, v) = (svd.u.line(i), svd.v.line(i));
        let out = &mut plus[i * nd * nc..(i + 1) * nd * nc];
        for j in 0..k {
            if s[j] <= floor || s[j] <= 0.0 {
                continue;
            }
            let f = s[j] / (s[j] * s[j] + c);
            for d in 0..nd {
                let vd = v[d * k + j] * f;
                for r in 0..nc {
                    // A is real, so U diag V^T is real up to rounding.
                    out[d * nc + r] += (vd * u[r * k + j].conj()).re;
                }
            }
        }
        if scaled {
            for d in 0..nd {
                let n = norms[i * nd + d];
                let back = if n > 0.0 { 1.0 / n } else { 0.0 };
                for r in 0..nc {
                    out[d * nc + r] *= back;
                }
            }
        }
    }
    Ok((plus, constants))
}

/// The system the levels are solved from, on the in-band lines.
struct System {
    /// `nb` lines of `nc` by `nd`: `|H_kd|^2`, divided by the control's
    /// reference autospectrum when weighted (zero where that is zero).
    a: MatrixStack,
    /// `nb * nc`: the right side, the reference autospectra, or 1 (0 where
    /// the reference is zero) when weighted.
    rhs: Vec<f64>,
    /// `nb * nc`: the weight of each control's row (1 unweighted).
    weight: Vec<f64>,
    /// `S+`, and each drive's constant on each line (`nb * nd`).
    plus: Vec<f64>,
    constants: Vec<f64>,
}

fn system(
    frf: &MatrixStack,
    reference: &HermitianStack,
    band: (usize, usize),
    weighted: bool,
    constant: f64,
) -> Result<System, LawError> {
    let frf_band = band_matrix(frf, band);
    let (nb, nc, nd) = frf_band.dims();
    let mut a = MatrixStack::zeros(nb, nc, nd);
    let mut rhs = vec![0.0; nb * nc];
    let mut weight = vec![1.0; nb * nc];
    for i in 0..nb {
        let t = autospectra(reference, band.0 + i);
        for k in 0..nc {
            if weighted {
                // Hunter's step 2: the row and its target divided by the
                // target; a control with no target takes no part.
                weight[i * nc + k] = if t[k] > 0.0 { 1.0 / t[k] } else { 0.0 };
                rhs[i * nc + k] = if t[k] > 0.0 { 1.0 } else { 0.0 };
            } else {
                rhs[i * nc + k] = t[k];
            }
        }
    }
    for (idx, (x, h)) in a.data_mut().iter_mut().zip(frf_band.data()).enumerate() {
        let m = h.norm_sqr();
        let row = idx / nd;
        *x = C64::new(
            if weighted {
                let (i, k) = (row / nc, row % nc);
                let t = reference.get(band.0 + i, k, k).re;
                if t > 0.0 {
                    m / t
                } else {
                    0.0
                }
            } else {
                m
            },
            0.0,
        );
    }
    let (plus, constants) = inverse_of(&a, constant, weighted)?;
    Ok(System {
        a,
        rhs,
        weight,
        plus,
        constants,
    })
}

/// How a level the plain solve makes negative is treated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelSolve {
    /// The minimizer of the same objective over levels of zero or more
    /// (N. Hunter's refinement), by the active-set method.
    NonNegative,
    /// The published rule: set the negative ones to zero, keep the rest.
    SetToZero,
}

/// How the independent levels are solved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolveOptions {
    /// Divide each control's row and target by its target (N. Hunter's
    /// refinement).
    pub weighted: bool,
    /// The treatment of negative levels.
    pub solve: LevelSolve,
    /// The Tikhonov constant: relative to each drive's own diagonal element
    /// of `A^T A` when weighted, to the largest one otherwise.
    pub regularization_constant: f64,
    /// With [`LevelSolve::SetToZero`], a negative level is set to this
    /// fraction of the line's largest.
    pub negative_floor: f64,
}

impl SolveOptions {
    /// The published form: unweighted, negative levels set to zero.
    pub fn published(regularization_constant: f64, negative_floor: f64) -> Self {
        SolveOptions {
            weighted: false,
            solve: LevelSolve::SetToZero,
            regularization_constant,
            negative_floor,
        }
    }
}

/// The result of [`nonnegative_least_squares`].
#[derive(Clone, Debug, PartialEq)]
pub struct Nnls {
    /// The minimizer, every entry zero or more.
    pub x: Vec<f64>,
    /// Least-squares subproblems solved.
    pub iterations: usize,
}

/// The bound on the subproblems [`nonnegative_least_squares`] solves, per
/// unknown: Lawson and Hanson's `3 n`.
pub const NNLS_ITERATIONS_PER_UNKNOWN: usize = 3;

/// The minimizer of `||M x - y||` over `x >= 0`, `M` `rows` by `cols`,
/// row-major: Lawson and Hanson's active-set method ("Solving Least Squares
/// Problems", 1974, chapter 23). From `x = 0`, the free variable with the
/// largest positive pull `w = M^T (y - M x)` is freed; the least-squares
/// solution over the free set, by Householder QR, is taken if every free
/// entry is positive, otherwise the step toward it is cut where the first
/// free entry reaches zero and that entry is fixed at zero. It stops when no
/// fixed variable pulls by more than `10 eps ||M||_F ||y||`.
///
/// `None` if an entry of `M` or `y` is not finite or their norms overflow, if
/// more than `3 * cols`
/// subproblems are needed, if a subproblem's columns are dependent, or if a
/// step toward a subproblem's solution is not finite: the caller then falls
/// back to the published rule and counts it.
/// Only arithmetic and square roots, in a fixed order, so the native law and
/// the law file take the same steps.
pub fn nonnegative_least_squares(m: &[f64], y: &[f64], rows: usize, cols: usize) -> Option<Nnls> {
    if !m.iter().chain(y).all(|v| v.is_finite()) {
        return None;
    }
    let mut x = vec![0.0; cols];
    let mut free = vec![false; cols];
    let norm_m = m.iter().map(|v| v * v).sum::<f64>().sqrt();
    let norm_y = y.iter().map(|v| v * v).sum::<f64>().sqrt();
    let tol = 10.0 * f64::EPSILON * norm_m * norm_y;
    if !tol.is_finite() {
        // The norms overflow: every pull would fall below the tolerance and
        // the zero levels would be returned as if they were the minimizer.
        return None;
    }
    let bound = NNLS_ITERATIONS_PER_UNKNOWN * cols;
    let mut iterations = 0usize;
    let mut w = vec![0.0; cols];
    loop {
        pull(m, y, &x, rows, cols, &mut w);
        let mut best: Option<usize> = None;
        for j in 0..cols {
            if !free[j] && w[j] > tol && best.is_none_or(|b| w[j] > w[b]) {
                best = Some(j);
            }
        }
        let Some(j) = best else {
            return Some(Nnls { x, iterations });
        };
        free[j] = true;
        loop {
            iterations += 1;
            if iterations > bound {
                return None;
            }
            let z = subset_least_squares(m, y, rows, cols, &free)?;
            if (0..cols).all(|p| !free[p] || z[p] > 0.0) {
                x = z;
                break;
            }
            // Step toward z as far as the first free entry reaching zero.
            let mut alpha = f64::INFINITY;
            let mut first = 0;
            for p in 0..cols {
                if free[p] && z[p] <= 0.0 {
                    let a = x[p] / (x[p] - z[p]);
                    if a < alpha {
                        alpha = a;
                        first = p;
                    }
                }
            }
            if !alpha.is_finite() {
                return None;
            }
            for p in 0..cols {
                if free[p] {
                    x[p] += alpha * (z[p] - x[p]);
                }
            }
            x[first] = 0.0;
            for p in 0..cols {
                if free[p] && x[p] <= 0.0 {
                    x[p] = 0.0;
                    free[p] = false;
                }
            }
            if !free.iter().any(|&f| f) {
                break;
            }
        }
    }
}

/// `w = M^T (y - M x)`.
fn pull(m: &[f64], y: &[f64], x: &[f64], rows: usize, cols: usize, w: &mut [f64]) {
    w.fill(0.0);
    for r in 0..rows {
        let mut res = y[r];
        for c in 0..cols {
            res -= m[r * cols + c] * x[c];
        }
        for c in 0..cols {
            w[c] += m[r * cols + c] * res;
        }
    }
}

/// The least-squares solution over the columns marked `free` (zero
/// elsewhere), by Householder QR. `None` if those columns are dependent to
/// rounding.
fn subset_least_squares(
    m: &[f64],
    y: &[f64],
    rows: usize,
    cols: usize,
    free: &[bool],
) -> Option<Vec<f64>> {
    let idx: Vec<usize> = (0..cols).filter(|&c| free[c]).collect();
    let p = idx.len();
    if p > rows {
        return None;
    }
    // Column-major copy of the free columns.
    let mut q = vec![0.0; rows * p];
    for (j, &c) in idx.iter().enumerate() {
        for r in 0..rows {
            q[j * rows + r] = m[r * cols + c];
        }
    }
    let mut b = y.to_vec();
    let mut diag = vec![0.0; p];
    for k in 0..p {
        let col = &q[k * rows..(k + 1) * rows];
        let norm = col[k..].iter().map(|v| v * v).sum::<f64>().sqrt();
        if norm == 0.0 {
            return None;
        }
        let alpha = if col[k] >= 0.0 { -norm } else { norm };
        let mut v = col[k..].to_vec();
        v[0] -= alpha;
        let vv = v.iter().map(|t| t * t).sum::<f64>();
        if vv == 0.0 {
            return None;
        }
        for j in (k + 1)..p {
            let cj = &mut q[j * rows + k..(j + 1) * rows];
            let s = v.iter().zip(cj.iter()).map(|(a, b)| a * b).sum::<f64>();
            let f = 2.0 * s / vv;
            for (t, a) in cj.iter_mut().zip(&v) {
                *t -= f * a;
            }
        }
        let s = v.iter().zip(&b[k..]).map(|(a, b)| a * b).sum::<f64>();
        let f = 2.0 * s / vv;
        for (t, a) in b[k..].iter_mut().zip(&v) {
            *t -= f * a;
        }
        diag[k] = alpha;
    }
    let top = diag.iter().fold(0.0f64, |a, d| a.max(d.abs()));
    if diag
        .iter()
        .any(|d| d.abs() <= rows.max(p) as f64 * f64::EPSILON * top)
    {
        return None;
    }
    // Back substitution: R holds diag on its diagonal, q above it.
    let mut z = vec![0.0; p];
    for k in (0..p).rev() {
        let mut t = b[k];
        for j in (k + 1)..p {
            t -= q[j * rows + k] * z[j];
        }
        z[k] = t / diag[k];
    }
    let mut out = vec![0.0; cols];
    for (j, &c) in idx.iter().enumerate() {
        out[c] = z[j];
    }
    Some(out)
}

/// What one line's solve did.
#[derive(Clone, Copy, Debug, Default)]
struct LineSolve {
    /// Levels of the plain solve below zero.
    negative: usize,
    /// Levels the non-negative solve held at zero.
    held: usize,
    /// The non-negative solve did not finish and the published rule was used.
    fallback: bool,
}

/// One line's levels: `g` holds the unconstrained minimizer of
/// `||A g - b||^2 + sum_d c_d (g_d - prior_d)^2` on entry and the levels on return.
/// The plain levels' negatives are counted; with the non-negative solve and
/// any negative, `g` is the minimizer over `g >= 0` (`b` given by `rhs`),
/// otherwise the published rule applies.
#[allow(clippy::too_many_arguments)]
fn settle_line(
    g: &mut [f64],
    a: &[C64],
    b: &[f64],
    prior: &[f64],
    c: &[f64],
    nc: usize,
    nd: usize,
    opts: &SolveOptions,
) -> LineSolve {
    let negative = g.iter().filter(|v| **v < 0.0).count();
    if negative == 0 || opts.solve == LevelSolve::SetToZero {
        negative_rule(g, opts.negative_floor);
        return LineSolve {
            negative,
            ..LineSolve::default()
        };
    }
    // [A; diag(sqrt(c))] g ~ [b; diag(sqrt(c)) prior].
    let rows = nc + nd;
    let mut m = vec![0.0; rows * nd];
    let mut y = vec![0.0; rows];
    for k in 0..nc {
        for d in 0..nd {
            m[k * nd + d] = a[k * nd + d].re;
        }
        y[k] = b[k];
    }
    for d in 0..nd {
        let rc = c[d].sqrt();
        m[(nc + d) * nd + d] = rc;
        y[nc + d] = rc * prior[d];
    }
    match nonnegative_least_squares(&m, &y, rows, nd) {
        Some(sol) => {
            g.copy_from_slice(&sol.x);
            LineSolve {
                negative,
                held: sol.x.iter().filter(|v| **v == 0.0).count(),
                fallback: false,
            }
        }
        None => {
            negative_rule(g, 0.0);
            LineSolve {
                negative,
                held: 0,
                fallback: true,
            }
        }
    }
}

/// `S+ y` on one line: `plus` is `nd` by `nc`.
fn apply(plus: &[f64], y: &[f64], nd: usize, nc: usize, out: &mut [f64]) {
    for d in 0..nd {
        out[d] = (0..nc).map(|r| plus[d * nc + r] * y[r]).sum();
    }
}

/// The published rule for negative levels, on one line: each negative level
/// is set to `floor_rel` times the line's largest level (0: set to zero, as
/// Hunter, Cross and Nelson, p. 6, and Schultz, section 6.3, state it).
/// Returns how many were negative.
fn negative_rule(levels: &mut [f64], floor_rel: f64) -> usize {
    let top = levels.iter().copied().fold(0.0f64, f64::max);
    let mut count = 0;
    for g in levels.iter_mut() {
        if *g < 0.0 {
            *g = floor_rel * top;
            count += 1;
        }
    }
    count
}

/// The reference's autospectra on one line, negatives read as zero.
fn autospectra(reference: &HermitianStack, line: usize) -> Vec<f64> {
    reference
        .diagonal(line)
        .into_iter()
        .map(|v| v.max(0.0))
        .collect()
}

fn check_dims(
    frf: &MatrixStack,
    reference: &HermitianStack,
    band: (usize, usize),
) -> Result<(), LawError> {
    let (lines, nc, _) = frf.dims();
    if reference.lines() != lines || reference.n() != nc {
        return Err(LawError(format!(
            "the reference is {} lines by {}, the frequency response {lines} lines by {nc} \
             controls",
            reference.lines(),
            reference.n()
        )));
    }
    if band.0 >= band.1 || band.1 > lines {
        return Err(LawError(format!(
            "band [{}, {}) is not inside 0..{lines}",
            band.0, band.1
        )));
    }
    Ok(())
}

/// Independent drive levels over the whole grid.
#[derive(Clone, Debug, PartialEq)]
pub struct IndependentLevels {
    /// Drive autospectra, `lines * drives`, line-major; zero outside the band.
    pub levels: Vec<f64>,
    /// Per line, how many drives' levels the plain (unconstrained) solve made
    /// negative; zero outside the band.
    pub negative: Vec<f64>,
    /// Per line, how many levels the non-negative solve held at zero.
    pub held_at_zero: Vec<f64>,
    /// Per line, 1 where the line was found exact (independent drives meet
    /// the autospectra; nearly independent reference only), else 0.
    pub exact: Vec<f64>,
    /// Lines where the non-negative solve did not finish and the published
    /// rule was used.
    pub fallbacks: usize,
}

/// The independent drive levels for `reference` through `frf` on the lines
/// of `band`, in the published form: per line `g = S+ r` with `r` the
/// reference autospectra and `S+` the regularized inverse of the squared
/// magnitudes of `frf` (Mayes and Rohe, Eqs. 3 to 5), each negative level set
/// to `negative_floor` times the line's largest (0: zero).
pub fn independent_levels(
    frf: &MatrixStack,
    reference: &HermitianStack,
    band: (usize, usize),
    regularization_constant: f64,
    negative_floor: f64,
) -> Result<IndependentLevels, LawError> {
    independent_levels_with(
        frf,
        reference,
        band,
        &SolveOptions::published(regularization_constant, negative_floor),
    )
}

/// The independent drive levels solved as `opts` says: the system weighted
/// by the targets or not, negative levels set to zero or the non-negative
/// minimizer of the same objective.
pub fn independent_levels_with(
    frf: &MatrixStack,
    reference: &HermitianStack,
    band: (usize, usize),
    opts: &SolveOptions,
) -> Result<IndependentLevels, LawError> {
    check_dims(frf, reference, band)?;
    let sys = system(
        frf,
        reference,
        band,
        opts.weighted,
        opts.regularization_constant,
    )?;
    Ok(levels_from(&sys, frf.dims(), band, opts))
}

/// The squared-magnitude matrix of `frf` on the lines of `band`, each
/// control's row divided by its reference autospectrum when `weighted` (zero
/// where that is zero): `nb` lines of controls by drives, real parts.
pub fn squared_magnitudes(
    frf: &MatrixStack,
    reference: &HermitianStack,
    band: (usize, usize),
    weighted: bool,
) -> Result<MatrixStack, LawError> {
    check_dims(frf, reference, band)?;
    Ok(system(frf, reference, band, weighted, 0.0)?.a)
}

fn levels_from(
    sys: &System,
    (lines, nc, nd): (usize, usize, usize),
    band: (usize, usize),
    opts: &SolveOptions,
) -> IndependentLevels {
    let mut levels = vec![0.0; lines * nd];
    let mut negative = vec![0.0; lines];
    let mut held_at_zero = vec![0.0; lines];
    let mut fallbacks = 0;
    let prior = vec![0.0; nd];
    for (i, line) in (band.0..band.1).enumerate() {
        let r = &sys.rhs[i * nc..(i + 1) * nc];
        let g = &mut levels[line * nd..(line + 1) * nd];
        // Mayes and Rohe, Eq. 5: |v|^2 = S+ |x|^2.
        apply(&sys.plus[i * nd * nc..(i + 1) * nd * nc], r, nd, nc, g);
        let out = settle_line(
            g,
            sys.a.line(i),
            r,
            &prior,
            &sys.constants[i * nd..(i + 1) * nd],
            nc,
            nd,
            opts,
        );
        negative[line] = out.negative as f64;
        held_at_zero[line] = out.held as f64;
        fallbacks += out.fallback as usize;
    }
    IndependentLevels {
        levels,
        negative,
        held_at_zero,
        exact: vec![0.0; lines],
        fallbacks,
    }
}

/// The diagonal drive of `levels` (`lines * drives`).
fn diagonal_drive(levels: &[f64], lines: usize, nd: usize) -> HermitianStack {
    let mut g = HermitianStack::zeros(lines, nd);
    for line in 0..lines {
        for d in 0..nd {
            let v = levels[line * nd + d];
            if v != 0.0 {
                g.set(line, d, d, C64::new(v, 0.0));
            }
        }
    }
    g
}

/// The nearly independent reference for `reference` through `frf` on the lines
/// of `band` (zero outside), and the independent levels it was formed from.
///
/// Per line: the independent levels `g` (with negative levels set to zero),
/// the trial response `S_YYI = H diag(g) H^H` (Cap, Norman and Smallwood,
/// Eq. 4), and its rescale `S_YYIC(i,j) = S_YYI(i,j) sqrt(r_i r_j) /
/// sqrt(S_YYI(i,i) S_YYI(j,j))` to the reference autospectra `r` (their
/// Appendix A, p. 12), which keeps every pair's coherence and phase. A control
/// with no trial autospectrum keeps its specified one and is left independent.
/// Positive semi-definite by construction.
pub fn nearly_independent_reference(
    frf: &MatrixStack,
    reference: &HermitianStack,
    band: (usize, usize),
    regularization_constant: f64,
) -> Result<(HermitianStack, IndependentLevels), LawError> {
    nearly_independent_reference_with(
        frf,
        reference,
        band,
        &SolveOptions::published(regularization_constant, 0.0),
        0.0,
    )
}

/// The nearly independent reference with the trial levels solved as `opts`
/// says, and N. Hunter's exactness test when `exact_tolerance > 0`: where the
/// plain least-squares levels with no regularization are all zero or more and
/// leave a residual at most `exact_tolerance` of the right side, relative,
/// those are the line's levels and the line is marked exact.
pub fn nearly_independent_reference_with(
    frf: &MatrixStack,
    reference: &HermitianStack,
    band: (usize, usize),
    opts: &SolveOptions,
    exact_tolerance: f64,
) -> Result<(HermitianStack, IndependentLevels), LawError> {
    let opts = SolveOptions {
        negative_floor: 0.0,
        ..*opts
    };
    check_dims(frf, reference, band)?;
    let sys = system(
        frf,
        reference,
        band,
        opts.weighted,
        opts.regularization_constant,
    )?;
    let mut levels = levels_from(&sys, frf.dims(), band, &opts);
    let (lines, nc, nd) = frf.dims();
    if exact_tolerance > 0.0 {
        let (plain, _) = inverse_of(&sys.a, 0.0, false)?;
        let mut s = vec![0.0; nd];
        for (i, line) in (band.0..band.1).enumerate() {
            let r = &sys.rhs[i * nc..(i + 1) * nc];
            let norm = r.iter().map(|v| v * v).sum::<f64>().sqrt();
            if norm <= 0.0 {
                continue;
            }
            apply(&plain[i * nd * nc..(i + 1) * nd * nc], r, nd, nc, &mut s);
            if s.iter().any(|v| *v < 0.0) {
                continue;
            }
            let a = sys.a.line(i);
            let mut res = 0.0;
            for k in 0..nc {
                let mut e = -r[k];
                for d in 0..nd {
                    e += a[k * nd + d].re * s[d];
                }
                res += e * e;
            }
            if res.sqrt() <= exact_tolerance * norm {
                levels.levels[line * nd..(line + 1) * nd].copy_from_slice(&s);
                levels.held_at_zero[line] = 0.0;
                levels.exact[line] = 1.0;
            }
        }
    }
    let mut target = HermitianStack::zeros(lines, nc);
    let per = HermitianStack::packed_len(nc);
    let mut b = vec![ZERO; nc * nd];
    let mut trial = vec![ZERO; per];
    for line in band.0..band.1 {
        let h = frf.line(line);
        let g = &levels.levels[line * nd..(line + 1) * nd];
        for r in 0..nc {
            for d in 0..nd {
                b[r * nd + d] = h[r * nd + d] * g[d].sqrt();
            }
        }
        // S_YYI = (H diag(sqrt g)) (H diag(sqrt g))^H = H diag(g) H^H.
        gram_packed(&b, nc, nd, &mut trial);
        let at = |r: usize, c: usize| trial[HermitianStack::packed_index(nc, r, c)];
        let want = autospectra(reference, line);
        let scale: Vec<f64> = (0..nc)
            .map(|i| {
                let t = at(i, i).re;
                if t > 0.0 {
                    (want[i] / t).sqrt()
                } else {
                    0.0
                }
            })
            .collect();
        for r in 0..nc {
            target.set(line, r, r, C64::new(want[r], 0.0));
            for c in (r + 1)..nc {
                target.set(line, r, c, at(r, c) * (scale[r] * scale[c]));
            }
        }
    }
    Ok((target, levels))
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

fn regularization_constant_param() -> ParamDecl {
    ParamDecl {
        key: "regularization_constant",
        label: "Regularization constant",
        help: "The Tikhonov constant of the independent-drive solve. With the weighting by \
               the target it multiplies each drive's own diagonal element of the weighted \
               normal matrix of squared frequency-response magnitudes on each line \
               (Marquardt's scaling); with no weighting, the largest diagonal element, as \
               published. 0.01 is the published value; 0 solves by the plain pseudo-inverse.",
        kind: ParamKindDecl::Number {
            unit: "",
            min: 0.0,
            max: 1.0,
            step: 1e-3,
            default: 0.01,
        },
    }
}

const SOLVES: &[&str] = &["nonnegative", "set_to_zero"];
const SOLVE_WEIGHTINGS: &[&str] = &["target", "none"];

fn level_solve(choice: &str) -> Result<LevelSolve, LawError> {
    match choice {
        "nonnegative" => Ok(LevelSolve::NonNegative),
        "set_to_zero" => Ok(LevelSolve::SetToZero),
        other => Err(LawError(format!(
            "solve `{other}` is not one of {}",
            SOLVES.join(", ")
        ))),
    }
}

fn solve_weighting(choice: &str) -> Result<bool, LawError> {
    match choice {
        "target" => Ok(true),
        "none" => Ok(false),
        other => Err(LawError(format!(
            "solve weighting `{other}` is not one of {}",
            SOLVE_WEIGHTINGS.join(", ")
        ))),
    }
}

fn solve_param() -> ParamDecl {
    ParamDecl {
        key: "solve",
        label: "Level solve",
        help: "Where the plain solve gives a drive a negative level: nonnegative (the default, \
               N. Hunter's refinement) solves the same problem again over levels of zero or \
               more; set_to_zero is the published rule, the negative levels set to zero and \
               the rest kept.",
        kind: ParamKindDecl::Choice {
            options: SOLVES,
            default: "nonnegative",
        },
    }
}

/// `solve_weighting`: `none` by default in `independent`, `target` in
/// `nearly_independent` (Choice 21).
fn solve_weighting_param(default: &'static str) -> ParamDecl {
    let help = if default == "target" {
        "target (the default, N. Hunter's refinement) divides each control's row of the \
         squared-magnitude system and its target by that target, so each control's error \
         counts in proportion to its own level and a control with no target on a line \
         takes no part there; none is the published, unweighted solve. Unlike the \
         shared error weighting, it changes the result on a square system too."
    } else {
        "none (the default) is the published, unweighted solve; target (N. Hunter's \
         refinement) divides each control's row of the squared-magnitude system and its \
         target by that target, so each control's error counts in proportion to its own \
         level and a control with no target on a line takes no part there. Caution: the \
         solved levels are the drive played, and where the targets cannot all be met the \
         weighted solve settles low."
    };
    ParamDecl {
        key: "solve_weighting",
        label: "Solve weighting",
        help,
        kind: ParamKindDecl::Choice {
            options: SOLVE_WEIGHTINGS,
            default,
        },
    }
}

fn diag_held() -> DiagDecl {
    DiagDecl {
        key: "held_at_zero",
        label: "Levels held at zero",
        unit: "",
        help: "How many drive levels the non-negative solve held at zero on each line. 0 with \
               the set-to-zero solve and out of band.",
        kind: DiagKind::PerLine,
    }
}

fn diag_fallbacks() -> DiagDecl {
    DiagDecl {
        key: "solve_fallbacks",
        label: "Solve fallbacks",
        unit: "",
        help: "Lines where the non-negative solve did not finish within its bound (3 \
               subproblems per drive) and the published set-to-zero rule was used instead.",
        kind: DiagKind::Scalar,
    }
}

const LOOP_FORMS: &[&str] = &["closed", "open"];

/// `true` for the open form.
fn loop_form(choice: &str) -> Result<bool, LawError> {
    match choice {
        "closed" => Ok(false),
        "open" => Ok(true),
        other => Err(LawError(format!(
            "loop `{other}` is not one of {}",
            LOOP_FORMS.join(", ")
        ))),
    }
}

fn level_set_param() -> ParamDecl {
    ParamDecl {
        key: "level_set",
        label: "Level set",
        help: "On (the default, N. Hunter's refinement): on each line that is not exact, the \
               first drive is scaled by one number, the sum of the target autospectra over the \
               sum of the autospectra the frequency response predicts for it, so the total \
               predicted power equals the total specified. In the closed loop it sets only the \
               first drive's level; the loop then sets the level from the response. Off is the \
               published form.",
        kind: ParamKindDecl::Toggle(true),
    }
}

fn loop_param() -> ParamDecl {
    ParamDecl {
        key: "loop",
        label: "Loop",
        help: "closed (the default): the default law's closed loop holds the reference, \
               correcting from the response every loop. open: N. Hunter's law as his note \
               runs it; on every call the law returns its own drive for the frequency response \
               it is handed (the diagonal drive on an exact line, otherwise the inverse of the \
               reference, scaled by the level set when that is on) and does not read the \
               response. The open form does not correct from the response, so its error is \
               whatever the model's error is; the core's level, limits, ceilings and aborts act \
               on it as on any law.",
        kind: ParamKindDecl::Choice {
            options: LOOP_FORMS,
            default: "closed",
        },
    }
}

/// The shared convergence factor, defaulting to 1: Eq. 6 whole.
fn independent_convergence_param() -> ParamDecl {
    let shared = convergence_param();
    let kind = match shared.kind {
        ParamKindDecl::Number {
            unit,
            min,
            max,
            step,
            ..
        } => ParamKindDecl::Number {
            unit,
            min,
            max,
            step,
            default: 1.0,
        },
        other => other,
    };
    ParamDecl {
        help: "The fraction of each loop's step applied. 1, the default, is the published \
               whole step, which passes measurement noise to the drive; smaller values \
               converge more slowly and average the noise.",
        kind,
        ..shared
    }
}

fn diag_negative() -> DiagDecl {
    DiagDecl {
        key: "negative_levels",
        label: "Negative drive levels",
        unit: "",
        help: "How many drives' solved autospectra were below zero on each line, counted \
               before the negative rule and the change limit. At the seed a count marks a line \
               where independent drives cannot meet the reference autospectra. On later loops \
               it also counts a step that overshoots below zero, and a counted level may still \
               be played above zero where the fall limit holds it. 0 out of band.",
        kind: DiagKind::PerLine,
    }
}

fn negative_values(negative: &[f64]) -> DiagnosticValue {
    DiagnosticValue {
        key: "negative_levels".into(),
        data: DiagnosticData::PerLine(negative.to_vec()),
    }
}

// ---------------------------------------------------------------------------
// Independent drives
// ---------------------------------------------------------------------------

/// The settings of [`IndependentDrivesLaw`].
#[derive(Clone, Debug, PartialEq)]
pub struct IndependentConfig {
    /// The Tikhonov constant: relative to each drive's own diagonal element
    /// of `A^T A` when weighted, to the largest one otherwise.
    pub regularization_constant: f64,
    /// A negative level is set to this fraction of the line's largest.
    pub negative_floor: f64,
    /// The largest rise per loop of a drive autospectrum, percent.
    pub limit_up_percent: f64,
    /// The largest fall per loop of a drive autospectrum, percent.
    pub limit_down_percent: f64,
    /// The fraction of the Eq. 6 step applied: 1, the default, is the
    /// published whole step.
    pub convergence: f64,
    /// Weight each control's row by its target (N. Hunter's refinement); off
    /// by default in this law (Choice 21).
    pub weighted: bool,
    /// Negative levels: the non-negative solve (N. Hunter's refinement) or
    /// the published set-to-zero rule.
    pub solve: LevelSolve,
}

impl Default for IndependentConfig {
    fn default() -> Self {
        IndependentConfig {
            regularization_constant: 0.01,
            negative_floor: 0.0,
            limit_up_percent: 100.0,
            limit_down_percent: 67.0,
            convergence: 1.0,
            weighted: false,
            solve: LevelSolve::NonNegative,
        }
    }
}

impl IndependentConfig {
    /// Set the parameter `key`.
    pub fn set(&mut self, key: &str, v: &ParamVal) -> Result<(), LawError> {
        match key {
            "regularization_constant" => self.regularization_constant = v.number(key)?,
            "negative_floor" => self.negative_floor = v.number(key)?,
            "limit_up_percent" => self.limit_up_percent = v.number(key)?,
            "limit_down_percent" => self.limit_down_percent = v.number(key)?,
            "convergence" => self.convergence = v.number(key)?,
            "solve" => self.solve = level_solve(v.choice(key)?)?,
            "solve_weighting" => self.weighted = solve_weighting(v.choice(key)?)?,
            other => return Err(LawError(format!("parameter `{other}` is not declared"))),
        }
        Ok(())
    }

    fn options(&self) -> SolveOptions {
        SolveOptions {
            weighted: self.weighted,
            solve: self.solve,
            regularization_constant: self.regularization_constant,
            negative_floor: self.negative_floor,
        }
    }

    fn check(&self) -> Result<(), LawError> {
        let bad = |key: &str, v: f64, lo: f64, hi: f64| {
            if v.is_finite() && v >= lo && v <= hi {
                Ok(())
            } else {
                Err(LawError(format!(
                    "parameter `{key}` must be in [{lo}, {hi}], got {v}"
                )))
            }
        };
        bad(
            "regularization_constant",
            self.regularization_constant,
            0.0,
            1.0,
        )?;
        bad("negative_floor", self.negative_floor, 0.0, 0.1)?;
        bad("limit_up_percent", self.limit_up_percent, 1.0, 1000.0)?;
        bad("limit_down_percent", self.limit_down_percent, 1.0, 99.0)?;
        bad("convergence", self.convergence, 0.1, 1.0)
    }
}

/// The independent-drives law for one run.
pub struct IndependentDrivesLaw {
    setup: MimoSetup,
    cfg: IndependentConfig,
    /// The in-band frequency response `sys` was formed from.
    key: Vec<C64>,
    sys: Option<System>,
    last: Option<HermitianStack>,
}

impl IndependentDrivesLaw {
    /// The law for `setup` with `cfg`.
    pub fn new(setup: &MimoSetup, cfg: &IndependentConfig) -> Result<Self, LawError> {
        setup.check()?;
        cfg.check()?;
        Ok(IndependentDrivesLaw {
            setup: setup.clone(),
            cfg: cfg.clone(),
            key: Vec::new(),
            sys: None,
            last: None,
        })
    }

    /// `S+` for `frf`, formed again only when its in-band lines changed.
    fn refresh(&mut self, frf: &MatrixStack) -> Result<(), LawError> {
        let (lines, nc, nd) = (self.setup.lines, self.setup.controls, self.setup.drives);
        if frf.dims() != (lines, nc, nd) {
            return Err(LawError(format!(
                "the frequency response is {:?}, expected ({lines}, {nc}, {nd})",
                frf.dims()
            )));
        }
        let band = self.setup.band;
        let inband = &frf.data()[band.0 * nc * nd..band.1 * nc * nd];
        if self.sys.is_none() || self.key.as_slice() != inband {
            self.sys = Some(system(
                frf,
                &self.setup.reference,
                band,
                self.cfg.weighted,
                self.cfg.regularization_constant,
            )?);
            self.key = inband.to_vec();
        }
        Ok(())
    }

    fn outputs(
        &mut self,
        drive: HermitianStack,
        lv: &IndependentLevels,
        limited: usize,
    ) -> MimoLoopOutputs {
        let correction_db = self
            .last
            .as_ref()
            .map(|last| largest_change_db(last, &drive, self.setup.band))
            .unwrap_or(0.0);
        self.last = Some(drive.clone());
        let value = |key: &str, data| DiagnosticValue {
            key: key.into(),
            data,
        };
        MimoLoopOutputs {
            drive,
            diagnostics: vec![
                value("correction_db", DiagnosticData::Scalar(correction_db)),
                negative_values(&lv.negative),
                value("limited", DiagnosticData::Scalar(limited as f64)),
                value(
                    "held_at_zero",
                    DiagnosticData::PerLine(lv.held_at_zero.clone()),
                ),
                value(
                    "solve_fallbacks",
                    DiagnosticData::Scalar(lv.fallbacks as f64),
                ),
            ],
        }
    }
}

impl MimoControlLaw for IndependentDrivesLaw {
    fn needs(&self) -> &[MimoInput] {
        &NEEDS
    }

    fn shapes(&self) -> &[MimoShape] {
        &ALL_SHAPES
    }

    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        self.last = None;
        self.refresh(inputs.frf)?;
        let s = &self.setup;
        let sys = self.sys.as_ref().expect("formed by refresh");
        let lv = levels_from(
            sys,
            (s.lines, s.controls, s.drives),
            s.band,
            &self.cfg.options(),
        );
        let drive = diagonal_drive(&lv.levels, s.lines, s.drives);
        Ok(self.outputs(drive, &lv, 0))
    }

    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        if self.last.is_none() {
            return Err(LawError("iterate was called before seed".into()));
        }
        let played = inputs
            .last_drive
            .ok_or_else(|| LawError("the core did not pass the last drive".into()))?;
        self.refresh(inputs.frf)?;
        let (lines, nc, nd, band) = (
            self.setup.lines,
            self.setup.controls,
            self.setup.drives,
            self.setup.band,
        );
        if played.lines() != lines || played.n() != nd {
            return Err(LawError(format!(
                "the last drive is {} lines by {}, expected {lines} by {nd}",
                played.lines(),
                played.n()
            )));
        }
        let up = 1.0 + self.cfg.limit_up_percent / 100.0;
        let down = 1.0 - self.cfg.limit_down_percent / 100.0;
        // Each drive's in-band mean as played: the base a rise from zero is
        // measured from.
        let means: Vec<f64> = (0..nd)
            .map(|d| {
                let s: f64 = (band.0..band.1)
                    .map(|l| played.get(l, d, d).re.max(0.0))
                    .sum();
                s / (band.1 - band.0) as f64
            })
            .collect();
        let sys = self.sys.as_ref().expect("formed by refresh");
        let opts = self.cfg.options();
        let e = self.cfg.convergence;
        let mut lv = IndependentLevels {
            levels: vec![0.0; lines * nd],
            negative: vec![0.0; lines],
            held_at_zero: vec![0.0; lines],
            exact: vec![0.0; lines],
            fallbacks: 0,
        };
        let mut limited = 0usize;
        let mut err = vec![0.0; nc];
        let mut step = vec![0.0; nd];
        let mut b = vec![0.0; nc];
        for (i, line) in (band.0..band.1).enumerate() {
            let want = autospectra(&self.setup.reference, line);
            let got = inputs.response.diagonal(line);
            let weight = &sys.weight[i * nc..(i + 1) * nc];
            for k in 0..nc {
                // Weighted by the target when weighting is on (1 otherwise).
                err[k] = (want[k] - got[k]) * weight[k];
            }
            // Mayes and Rohe, Eq. 6: |v_d|^2 = |v_i|^2 + S+ (|x_d|^2 - |x_i|^2),
            // times the convergence factor (1: the whole step).
            apply(
                &sys.plus[i * nd * nc..(i + 1) * nd * nc],
                &err,
                nd,
                nc,
                &mut step,
            );
            let g = &mut lv.levels[line * nd..(line + 1) * nd];
            let last: Vec<f64> = played
                .diagonal(line)
                .into_iter()
                .map(|v| v.max(0.0))
                .collect();
            for d in 0..nd {
                g[d] = last[d] + e * step[d];
            }
            // The same objective over levels of zero or more: the residual
            // against the response the last levels would give plus the
            // weighted error, and c times the distance from the last levels.
            let a = sys.a.line(i);
            for k in 0..nc {
                let mut t = e * err[k];
                for d in 0..nd {
                    t += a[k * nd + d].re * last[d];
                }
                b[k] = t;
            }
            let out = settle_line(
                g,
                a,
                &b,
                &last,
                &sys.constants[i * nd..(i + 1) * nd],
                nc,
                nd,
                &opts,
            );
            lv.negative[line] = out.negative as f64;
            lv.held_at_zero[line] = out.held as f64;
            lv.fallbacks += out.fallback as usize;
            // Mayes and Rohe, section 7.4: at most +100 and -67 percent a loop.
            for d in 0..nd {
                let base = last[d].max(LIMIT_FLOOR * means[d]);
                if base > 0.0 && g[d] > base * up {
                    g[d] = base * up;
                    limited += 1;
                } else if last[d] > 0.0 && g[d] < last[d] * down {
                    g[d] = last[d] * down;
                    limited += 1;
                }
            }
        }
        let drive = diagonal_drive(&lv.levels, lines, nd);
        Ok(self.outputs(drive, &lv, limited))
    }
}

impl BuiltinMimoLaw for IndependentDrivesLaw {
    fn spec() -> LawSpec {
        LawSpec {
            name: "independent drives",
            version: "1.1.0",
            author: "Hertzforge; refinements by N. Hunter",
            summary: "Uncorrelated drives, a diagonal drive matrix on every line. The seed solves \
                      the real system of squared frequency-response magnitudes for the drive \
                      autospectra that produce the reference autospectra, by a Tikhonov \
                      pseudo-inverse, with negative levels set to zero (Mayes and Rohe; \
                      Schultz; Hunter, Cross and Nelson). Each loop adds the same solve of the \
                      autospectrum error to the levels last played, each drive limited to \
                      +100 / -67 percent a loop (Mayes and Rohe); a convergence factor below \
                      1 takes part of the step. It matches autospectra only: \
                      the cross terms are whatever the structure makes of uncorrelated drives. \
                      By default it carries N. Hunter's non-negative least squares, used with \
                      his permission, where the plain solve gives a negative level; his \
                      weighting of each control by its own target can be set, and is off by \
                      default because, with the solved levels played directly, it settles low \
                      where the targets cannot all be met. The level solve set to set_to_zero \
                      gives the published method.",
            params: vec![
                regularization_constant_param(),
                ParamDecl {
                    key: "negative_floor",
                    label: "Negative level floor",
                    help: "With the set_to_zero solve only: a solved drive autospectrum below \
                           zero is set to this fraction of the line's largest solved level. 0 \
                           sets it to zero, the published rule. No effect with the \
                           nonnegative solve.",
                    kind: ParamKindDecl::Number {
                        unit: "",
                        min: 0.0,
                        max: 0.1,
                        step: 1e-3,
                        default: 0.0,
                    },
                },
                ParamDecl {
                    key: "limit_up_percent",
                    label: "Largest rise per loop",
                    help: "The most a drive's autospectrum may rise on a line in one loop, \
                           percent of its level played last loop. The controller's own cap \
                           still applies.",
                    kind: ParamKindDecl::Number {
                        unit: "%",
                        min: 1.0,
                        max: 1000.0,
                        step: 1.0,
                        default: 100.0,
                    },
                },
                ParamDecl {
                    key: "limit_down_percent",
                    label: "Largest fall per loop",
                    help: "The most a drive's autospectrum may fall on a line in one loop, \
                           percent of its level played last loop. The controller's own cap \
                           still applies.",
                    kind: ParamKindDecl::Number {
                        unit: "%",
                        min: 1.0,
                        max: 99.0,
                        step: 1.0,
                        default: 67.0,
                    },
                },
                independent_convergence_param(),
                solve_param(),
                solve_weighting_param("none"),
            ],
            diagnostics: vec![
                diag_correction(),
                diag_negative(),
                DiagDecl {
                    key: "limited",
                    label: "Limited drive levels",
                    unit: "",
                    help: "Drive autospectra, over all lines, held to the per-loop change \
                           limit this loop. 0 at the seed.",
                    kind: DiagKind::Scalar,
                },
                diag_held(),
                diag_fallbacks(),
            ],
            shapes: ALL_SHAPES.to_vec(),
            needs: NEEDS.to_vec(),
        }
    }

    fn build(setup: &MimoSetup, params: &[(String, ParamVal)]) -> Result<Self, LawError> {
        let mut cfg = IndependentConfig::default();
        for (k, v) in params {
            cfg.set(k, v)?;
        }
        Self::new(setup, &cfg)
    }
}

// ---------------------------------------------------------------------------
// Nearly independent drives
// ---------------------------------------------------------------------------

/// The settings of [`NearlyIndependentLaw`].
#[derive(Clone, Debug, PartialEq)]
pub struct NearlyIndependentConfig {
    /// The Tikhonov constant of the independent-drive solve.
    pub regularization_constant: f64,
    /// The convergence factor `e` of the closed loop.
    pub convergence: f64,
    /// Weight each control's row by its target (N. Hunter's refinement).
    pub weighted: bool,
    /// Negative trial levels: the non-negative solve (N. Hunter's
    /// refinement) or the published set-to-zero rule.
    pub solve: LevelSolve,
    /// N. Hunter's exactness test's relative residual; 0 turns it off.
    pub exact_tolerance: f64,
    /// N. Hunter's level set on the first drive of a line that is not exact.
    pub level_set: bool,
    /// The open form: the law's own drive on every call, no feedback.
    pub open: bool,
}

impl Default for NearlyIndependentConfig {
    fn default() -> Self {
        NearlyIndependentConfig {
            regularization_constant: 0.01,
            convergence: 0.5,
            weighted: true,
            solve: LevelSolve::NonNegative,
            exact_tolerance: 1e-10,
            level_set: true,
            open: false,
        }
    }
}

impl NearlyIndependentConfig {
    /// Set the parameter `key`.
    pub fn set(&mut self, key: &str, v: &ParamVal) -> Result<(), LawError> {
        match key {
            "regularization_constant" => self.regularization_constant = v.number(key)?,
            "convergence" => self.convergence = v.number(key)?,
            "solve" => self.solve = level_solve(v.choice(key)?)?,
            "solve_weighting" => self.weighted = solve_weighting(v.choice(key)?)?,
            "exact_tolerance" => self.exact_tolerance = v.number(key)?,
            "level_set" => self.level_set = v.toggle(key)?,
            "loop" => self.open = loop_form(v.choice(key)?)?,
            other => return Err(LawError(format!("parameter `{other}` is not declared"))),
        }
        Ok(())
    }
}

/// The nearly-independent-drives law for one run. The reference is formed at
/// the seed, from the frequency response the core hands the law.
pub struct NearlyIndependentLaw {
    setup: MimoSetup,
    cfg: NearlyIndependentConfig,
    inner: Option<FactorLoop>,
    /// The seed's levels and counts, reported on every loop.
    levels: Option<IndependentLevels>,
    /// The level-set factor per line, a ratio (1 on an exact line, out of
    /// band, and with the level set off), and the lines where it was guarded.
    level_factor: Vec<f64>,
    level_guarded: usize,
    /// The exact lines the closed loop could not hold as diagonal, which
    /// start from the loop's own drive.
    not_held: usize,
    /// The open form: the estimate the drive was formed for, and the outputs
    /// returned for it.
    open_frf: Option<MatrixStack>,
    open_out: Option<MimoLoopOutputs>,
}

/// The first drive of [`NearlyIndependentLaw`] for one estimate.
struct FirstDrive {
    inner: FactorLoop,
    step: Step,
    levels: IndependentLevels,
    level_factor: Vec<f64>,
    level_guarded: usize,
    not_held: usize,
}

impl NearlyIndependentLaw {
    /// The law for `setup` with `cfg`.
    pub fn new(setup: &MimoSetup, cfg: &NearlyIndependentConfig) -> Result<Self, LawError> {
        setup.check()?;
        let c = cfg.regularization_constant;
        if !(c.is_finite() && (0.0..=1.0).contains(&c)) {
            return Err(LawError(format!(
                "parameter `regularization_constant` must be in [0, 1], got {c}"
            )));
        }
        let t = cfg.exact_tolerance;
        if !(t.is_finite() && (0.0..=1e-3).contains(&t)) {
            return Err(LawError(format!(
                "parameter `exact_tolerance` must be in [0, 0.001], got {t}"
            )));
        }
        Ok(NearlyIndependentLaw {
            setup: setup.clone(),
            cfg: cfg.clone(),
            inner: None,
            levels: None,
            level_factor: vec![1.0; setup.lines],
            level_guarded: 0,
            not_held: 0,
            open_frf: None,
            open_out: None,
        })
    }

    /// The reference, the loop and its first drive for the estimate `frf`.
    /// The loop's own first drive is `H^+ G H^+^H`. On an exact line it is
    /// replaced by the diagonal drive of the exact levels where the loop can
    /// hold it, and always in the open form; in the closed loop, an exact
    /// line it cannot hold keeps the loop's own drive and is counted. On any
    /// other line,
    /// with the level set on, it is scaled by `sum t / trace(H X H^H)`, the
    /// targets' sum over the predicted autospectra's (N. Hunter's step 10).
    /// A zero or non-finite sum gives a factor of 1 and is counted. With
    /// neither change on any line the loop's own seed is used as it is.
    #[allow(clippy::needless_range_loop)]
    fn first(&self, frf: &MatrixStack) -> Result<FirstDrive, LawError> {
        let opts = SolveOptions {
            weighted: self.cfg.weighted,
            solve: self.cfg.solve,
            regularization_constant: self.cfg.regularization_constant,
            negative_floor: 0.0,
        };
        let (target, levels) = nearly_independent_reference_with(
            frf,
            &self.setup.reference,
            self.setup.band,
            &opts,
            self.cfg.exact_tolerance,
        )?;
        let settings = LoopSettings {
            convergence: self.cfg.convergence,
            cap_up_db: 3.5,
            cap_down_db: 6.0,
            rank_tol: 1e-6,
        };
        let mut inner = FactorLoop::new(&self.setup, &target, pareto::loop_rule(), settings)?;
        let step = inner.seed(frf)?;
        let mut level_factor = vec![1.0; self.setup.lines];
        let mut level_guarded = 0;
        let any_exact = levels.exact.iter().any(|e| *e > 0.0);
        if !self.cfg.level_set && !any_exact {
            return Ok(FirstDrive {
                inner,
                step,
                levels,
                level_factor,
                level_guarded,
                not_held: 0,
            });
        }
        let (_, nc, nd) = frf.dims();
        let mut x = step.drive.clone();
        for line in self.setup.band.0..self.setup.band.1 {
            if levels.exact[line] > 0.0 {
                for r in 0..nd {
                    for c in r..nd {
                        let v = if r == c {
                            levels.levels[line * nd + r]
                        } else {
                            0.0
                        };
                        x.set(line, r, c, C64::new(v, 0.0));
                    }
                }
                continue;
            }
            if !self.cfg.level_set {
                continue;
            }
            let wanted: f64 = autospectra(&self.setup.reference, line).iter().sum();
            let h = frf.line(line);
            let mut predicted = 0.0;
            for m in 0..nc {
                let mut y = ZERO;
                for a in 0..nd {
                    for b in 0..nd {
                        y += h[m * nd + a] * x.get(line, a, b) * h[m * nd + b].conj();
                    }
                }
                predicted += y.re;
            }
            let k = wanted / predicted;
            let k = if wanted > 0.0 && predicted > 0.0 && k.is_finite() {
                k
            } else {
                level_guarded += 1;
                1.0
            };
            level_factor[line] = k;
            for r in 0..nd {
                for c in r..nd {
                    x.set(line, r, c, x.get(line, r, c) * k);
                }
            }
        }
        let (mut first, differs) = inner.seed_with(frf, &x)?;
        let mut not_held = 0;
        if self.cfg.open {
            // The open form plays the diagonal drive on every exact line.
            first.drive = x;
        } else {
            // Closed: on an exact line the loop cannot hold as diagonal, the
            // first drive is the loop's own, the least drive with the same
            // response through its inverse.
            for line in self.setup.band.0..self.setup.band.1 {
                if levels.exact[line] > 0.0 && differs[line] > 0.0 {
                    not_held += 1;
                    for r in 0..nd {
                        for c in r..nd {
                            x.set(line, r, c, step.drive.get(line, r, c));
                        }
                    }
                }
            }
            if not_held > 0 {
                first = inner.seed_with(frf, &x)?.0;
            }
        }
        Ok(FirstDrive {
            inner,
            step: first,
            levels,
            level_factor,
            level_guarded,
            not_held,
        })
    }

    fn with_count(&self, mut out: MimoLoopOutputs) -> MimoLoopOutputs {
        if let Some(lv) = &self.levels {
            let value = |key: &str, data| DiagnosticValue {
                key: key.into(),
                data,
            };
            out.diagnostics.push(negative_values(&lv.negative));
            out.diagnostics.push(value(
                "held_at_zero",
                DiagnosticData::PerLine(lv.held_at_zero.clone()),
            ));
            out.diagnostics
                .push(value("exact", DiagnosticData::PerLine(lv.exact.clone())));
            out.diagnostics.push(value(
                "solve_fallbacks",
                DiagnosticData::Scalar(lv.fallbacks as f64),
            ));
            out.diagnostics.push(value(
                "level_set_factor",
                DiagnosticData::PerLine(self.level_factor.clone()),
            ));
            out.diagnostics.push(value(
                "level_set_guarded",
                DiagnosticData::Scalar(self.level_guarded as f64),
            ));
            out.diagnostics.push(value(
                "exact_not_held",
                DiagnosticData::Scalar(self.not_held as f64),
            ));
        }
        out
    }

    /// Keep the first drive's state and return its outputs.
    fn take_first(&mut self, first: FirstDrive) -> MimoLoopOutputs {
        self.levels = Some(first.levels);
        self.level_factor = first.level_factor;
        self.level_guarded = first.level_guarded;
        self.not_held = first.not_held;
        let out = first.step.outputs(true);
        self.inner = Some(first.inner);
        self.with_count(out)
    }

    /// Whether `frf` is, on the band, the estimate the open form's drive was
    /// formed for.
    fn same_estimate(&self, frf: &MatrixStack) -> bool {
        self.open_frf.as_ref().is_some_and(|old| {
            old.dims() == frf.dims()
                && (self.setup.band.0..self.setup.band.1).all(|l| old.line(l) == frf.line(l))
        })
    }
}

impl MimoControlLaw for NearlyIndependentLaw {
    fn needs(&self) -> &[MimoInput] {
        &[]
    }

    fn shapes(&self) -> &[MimoShape] {
        &ALL_SHAPES
    }

    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        let first = self.first(inputs.frf)?;
        let out = self.take_first(first);
        if self.cfg.open {
            self.open_frf = Some(inputs.frf.clone());
            self.open_out = Some(out.clone());
        }
        Ok(out)
    }

    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        if self.cfg.open {
            // The open form: the response is not read. The same drive while
            // the estimate is held; the drive for the new estimate when it
            // changes.
            let Some(prev) = self.open_out.clone() else {
                return Err(LawError("iterate was called before seed".into()));
            };
            if self.same_estimate(inputs.frf) {
                let mut out = prev;
                for d in &mut out.diagnostics {
                    if d.key == "correction_db" || d.key == "clamped" {
                        d.data = DiagnosticData::Scalar(0.0);
                    }
                }
                return Ok(out);
            }
            let first = self.first(inputs.frf)?;
            let mut out = self.take_first(first);
            let change = largest_change_db(&prev.drive, &out.drive, self.setup.band);
            for d in &mut out.diagnostics {
                if d.key == "correction_db" {
                    d.data = DiagnosticData::Scalar(change);
                }
            }
            self.open_frf = Some(inputs.frf.clone());
            self.open_out = Some(out.clone());
            return Ok(out);
        }
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| LawError("iterate was called before seed".into()))?;
        let out = inner.iterate(inputs.frf, inputs.response)?.outputs(true);
        Ok(self.with_count(out))
    }
}

impl BuiltinMimoLaw for NearlyIndependentLaw {
    fn spec() -> LawSpec {
        LawSpec {
            name: "nearly independent drives",
            version: "1.1.0",
            author: "Hertzforge; refinements by N. Hunter",
            summary: "Keeps the specified autospectra and takes each pair's coherence and phase \
                      from the response that independent drives would produce: the drive \
                      autospectra solved from the squared frequency-response magnitudes, \
                      negative levels set to zero, their response through the frequency \
                      response, rescaled to the specified autospectra (Smallwood's independent \
                      drive algorithm as Cap, Norman and Smallwood state it; the nearly \
                      independent drive algorithm of Hunter, Cross and Nelson). The drives \
                      are coupled just enough to reach that reference. Then runs the default \
                      law's closed loop on it. By default it carries N. Hunter's refinements, \
                      used with his permission: the weighting by the target, non-negative \
                      least squares, the exactness test that keeps the independent levels on \
                      a line they meet exactly, and the level set that scales the first drive \
                      to the total specified power. The level solve set to set_to_zero, the \
                      solve weighting to none, the exactness tolerance to 0 and the level set \
                      off give the published method. The open loop form is his law as his \
                      note runs it: the same drive on every call, with no correction from the \
                      response.",
            params: vec![
                regularization_constant_param(),
                ParamDecl {
                    help: "The fraction of each loop's correction applied in the closed loop. \
                           1 applies it whole; smaller values converge more slowly and average \
                           estimation noise. It has no meaning in the open form.",
                    ..convergence_param()
                },
                solve_param(),
                solve_weighting_param("target"),
                level_set_param(),
                loop_param(),
                ParamDecl {
                    key: "exact_tolerance",
                    label: "Exactness tolerance",
                    help: "N. Hunter's exactness test: where the plain least-squares levels, \
                           with no regularization, are all zero or more and leave a relative \
                           residual at most this, independent drives meet the autospectra on \
                           that line and those levels form its reference. 0 turns the test \
                           off, the published form.",
                    kind: ParamKindDecl::Number {
                        unit: "",
                        min: 0.0,
                        max: 1e-3,
                        step: 1e-10,
                        default: 1e-10,
                    },
                },
            ],
            diagnostics: vec![
                diag_condition(),
                diag_kept(),
                diag_correction(),
                diag_clamped(),
                DiagDecl {
                    help: "How many drives' independent levels the plain solve made negative \
                           on each line when the reference was formed at the seed, before the \
                           set-to-zero rule or the non-negative solve; the same on every loop. \
                           0 out of band.",
                    ..diag_negative()
                },
                diag_held(),
                DiagDecl {
                    key: "exact",
                    label: "Exact lines",
                    unit: "",
                    help: "1 on a line where the exactness test found that independent \
                           drives meet the autospectra, else 0; set at the seed.",
                    kind: DiagKind::PerLine,
                },
                diag_fallbacks(),
                DiagDecl {
                    key: "level_set_factor",
                    label: "Level-set factor",
                    unit: "",
                    help: "The level set's factor on each line's first drive, as a power \
                           ratio (10 log10 of it is the change in decibels): 1 on an exact \
                           line, out of band and with the level set off. Set when the first \
                           drive is formed (in the open form, again when the frequency response \
                           changes). A ratio, not decibels: a factor within rounding of 1 has a \
                           logarithm that is all rounding, and the two builds round differently.",
                    kind: DiagKind::PerLine,
                },
                DiagDecl {
                    key: "level_set_guarded",
                    label: "Level-set guards",
                    unit: "",
                    help: "Lines where the level set's sums were zero or not finite and the \
                           factor 1 was used.",
                    kind: DiagKind::Scalar,
                },
                DiagDecl {
                    key: "exact_not_held",
                    label: "Exact lines not held",
                    unit: "",
                    help: "Exact lines on which the closed loop cannot hold the diagonal drive \
                           (more drives than controls, or the inverse truncated on the line), \
                           so the first drive there is the loop's own, the least drive with the \
                           same response. 0 in the open form, which plays the diagonal drive.",
                    kind: DiagKind::Scalar,
                },
            ],
            shapes: ALL_SHAPES.to_vec(),
            needs: Vec::new(),
        }
    }

    fn build(setup: &MimoSetup, params: &[(String, ParamVal)]) -> Result<Self, LawError> {
        let mut cfg = NearlyIndependentConfig::default();
        for (k, v) in params {
            cfg.set(k, v)?;
        }
        Self::new(setup, &cfg)
    }
}
