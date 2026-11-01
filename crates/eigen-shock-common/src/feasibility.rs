//! Deterministic-kinematics pulse feasibility and the minimal double-sided
//! compensation helper (a placeholder).
//!
//! # A NEW feasibility path
//!
//! eigen-shaker-db's [`eigen_shaker_db::FeasibilityCalculator`] is the RANDOM path: it
//! takes a PSD, computes RMS kinematics, applies a crest factor, and compares against
//! `force_random`. Shock is DIFFERENT: the demand is a DETERMINISTIC pulse, the crest
//! factor is exactly 1.0 (the peak IS the peak, no statistical multiplier), and the
//! rating to compare against is `force_shock` - which NO existing code path consumes.
//! This module wires it: [`pulse_feasibility`] computes the pulse's kinematics with
//! the PURE integrator ([`eigen_dsp::shock::pulse_kinematics`] - no high-pass,
//! which would mask the residuals) and compares LIKE-FOR-LIKE against `force_shock`
//! (by its `RatingBasis`), `max_velocity_ips`, `max_displacement_in_pp`, and the
//! bare-armature acceleration.
//!
//! It REUSES eigen-shaker-db's report/item/verdict types so the UI and
//! report render random and shock feasibility identically.
//!
//! # The compensation-quality gate
//!
//! Because the kinematics are computed with the pure (no-HP) integrator, a pulse's
//! RESIDUAL velocity and displacement are the physical net changes it leaves. A
//! properly compensated pulse drives both to ~0; an UNcompensated pulse does not
//! (a half-sine leaves `residual_velocity = 2*A*T/pi`). [`pulse_feasibility`] adds a
//! `CompensationQuality` item that is `Exceeded` when a residual is above tolerance -
//! this is a PRE-ARM gate (an uncompensated pulse must not be fired on the host path).
//!
//! # Units
//!
//! The in/g/lbf system, matching eigen-shaker-db: acceleration g, velocity in/s,
//! displacement in, force lbf, mass lb (the `F[lbf] = m[lb] * a[g]` g-based shortcut).

use eigen_dsp::shock::pulse_kinematics;
use eigen_shaker_db::{
    FeasibilityItem, FeasibilityReport, MovingMass, Quantity, RatingBasis, RatingValue,
    ShakerRecord, Verdict,
};

/// Tolerances for the compensation-quality gate. A compensated pulse must
/// leave a residual velocity and displacement below these; above either, the pulse is
/// flagged as improperly compensated (a pre-arm hard block).
///
/// Defaults are a small fraction of typical pulse kinematics (in/s and in): tight
/// enough to catch an uncompensated half-sine (whose residual velocity is `2*A*T/pi`,
/// e.g. ~54 in/s for 20 g / 11 ms) while loose enough to pass a numerically-compensated
/// pulse whose residuals are float-noise small.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeasibilityTolerances {
    /// Max acceptable `|residual velocity|`, in/s.
    pub residual_velocity_ips: f64,
    /// Max acceptable `|residual displacement|`, in.
    pub residual_disp_in: f64,
}

impl Default for FeasibilityTolerances {
    fn default() -> Self {
        FeasibilityTolerances {
            // 0.1 in/s and 0.001 in are ~three orders of magnitude below an
            // uncompensated pulse's residual and well above float-integration noise
            // for a properly compensated one.
            residual_velocity_ips: 0.1,
            residual_disp_in: 0.001,
        }
    }
}

/// Deterministic pulse feasibility. Assess a COMPENSATED pulse
/// against a shaker record, comparing like-for-like with crest factor 1.0.
///
/// # Inputs
/// - `record`: the shaker's ratings. `force_shock` is the force rating consumed here
///   (NOT `force_random`, which the random path uses).
/// - `moving_mass`: the mass stack; `moving_mass.total()` is `m_total`.
/// - `compensated_pulse_g`: the acceleration waveform to fire, g (the host-path
///   compensated pulse; on the delegated path the device compensates - this gate is a
///   HostDac pre-arm check).
/// - `fs_hz`: the pulse sample rate.
/// - `tol`: the compensation-quality tolerances.
///
/// # Method (deterministic, crest = 1.0)
/// - Kinematics via [`eigen_dsp::shock::pulse_kinematics`] (PURE integrator): peak
///   accel = `max|a|`, peak velocity, pk-pk displacement.
/// - Peak accel vs `min(bare-armature accel rating, force_shock / m_total)`, comparing
///   `force_shock` by its `RatingBasis` (`Peak`: direct; `Rms`/`SineEquivalent`:
///   documented conversion / Advisory).
/// - Peak velocity vs `max_velocity_ips` (basis-aware).
/// - Pk-pk displacement vs `max_displacement_in_pp`.
/// - Residual velocity + displacement vs `tol` -> a `CompensationQuality`
///   acceleration-quantity item, `Exceeded` when the pulse is not properly compensated
///   (a pre-arm gate).
///
/// Assumptions are listed in the report (crest = 1.0 deterministic, in/g/lbf units,
/// static ratings only), matching eigen-shaker-db's convention.
pub fn pulse_feasibility(
    record: &ShakerRecord,
    moving_mass: &MovingMass,
    compensated_pulse_g: &[f64],
    fs_hz: f64,
    tol: FeasibilityTolerances,
) -> FeasibilityReport {
    let m_total = moving_mass.total();
    let kin = pulse_kinematics(compensated_pulse_g, fs_hz);

    let mut items: Vec<FeasibilityItem> = Vec::new();
    let mut assumptions: Vec<String> = vec![
        "Deterministic shock: crest factor = 1.00 (the pulse peak IS the demand; no statistical multiplier).".to_string(),
        format!(
            "Total moving mass = {m_total:.2} lb (armature + slip table + drive adapter + fixturing + test article)."
        ),
        "Units: in / g / lbf (F11). Force demand uses F[lbf] = m[lb] * a[g] (g-based shortcut).".to_string(),
        "Static ratings only; dynamic amplification and shaker-specific derations are not modeled.".to_string(),
        "Kinematics from the PURE trapezoidal double-integrator (no high-pass, finding F5) so residual velocity/displacement are the true net pulse changes.".to_string(),
        format!(
            "Pulse kinematics: peak accel {:.3} g, peak velocity {:.3} in/s, pk-pk displacement {:.4} in; residuals: velocity {:.4} in/s, displacement {:.5} in.",
            kin.peak_accel_g, kin.peak_velocity_ips, kin.pk_pk_disp_in, kin.residual_velocity_ips, kin.residual_disp_in
        ),
    ];

    // --- peak acceleration vs the mass-derated ceiling (consuming force_shock) ---
    items.push(accel_item(
        record,
        m_total,
        kin.peak_accel_g,
        &mut assumptions,
    ));

    // --- peak velocity vs max_velocity_ips (basis-aware) ---
    items.push(velocity_item(
        record.max_velocity_ips,
        kin.peak_velocity_ips,
    ));

    // --- pk-pk displacement vs the stroke limit ---
    items.push(displacement_item(
        record.max_displacement_in_pp,
        kin.pk_pk_disp_in,
    ));

    // --- compensation-quality gate ---
    items.push(compensation_quality_item(
        kin.residual_velocity_ips,
        kin.residual_disp_in,
        tol,
        &mut assumptions,
    ));

    let overall = worst_verdict(&items);
    FeasibilityReport {
        items,
        overall,
        assumptions,
    }
}

/// The mass-derated acceleration ceiling item, consuming `force_shock` like-for-like
/// by its `RatingBasis`. The usable ceiling is
/// `min(bare-armature rating, force_shock / m_total)`; the peak accel (crest = 1.0) is
/// compared against it.
fn accel_item(
    record: &ShakerRecord,
    m_total: f64,
    peak_accel_g: f64,
    assumptions: &mut Vec<String>,
) -> FeasibilityItem {
    let bare = record.max_accel_bare_g.map(|r| r.value);

    // force_shock -> an acceleration ceiling F/m, honoring the rating basis:
    //  - Peak: the shock force rating is already a peak; F_peak/m is the peak-accel
    //    ceiling directly.
    //  - Rms: convert the RMS force rating to a peak-equivalent for a DETERMINISTIC
    //    pulse. A single deterministic pulse has crest 1.0, so an RMS-basis force
    //    rating (a statistical number) is not a like-for-like peak bound; we document
    //    the conversion (treat the RMS value as the comparable magnitude, noting it is
    //    approximate) and cap the verdict at Advisory.
    //  - SineEquivalent: approximate mapping; compared as a peak but capped at
    //    Advisory (never a hard fail), matching eigen-shaker-db's treatment.
    let force_shock = record.force_shock.filter(|_| m_total > 0.0);
    let (force_ceiling, force_basis) = match force_shock {
        Some(RatingValue { value, basis }) => (Some(value / m_total), Some(basis)),
        None => (None, None),
    };

    let ceiling = match (bare, force_ceiling) {
        (Some(b), Some(fd)) => Some(b.min(fd)),
        (Some(b), None) => Some(b),
        (None, Some(fd)) => Some(fd),
        (None, None) => None,
    };

    // Whether the binding ceiling comes from the (possibly approximate-basis) force
    // rating, so an Rms/SineEquivalent force basis caps the verdict at Advisory.
    let ceiling_from_force = match (bare, force_ceiling) {
        (Some(b), Some(fd)) => fd <= b,
        (None, Some(_)) => true,
        _ => false,
    };
    let force_basis_is_approx = matches!(
        force_basis,
        Some(RatingBasis::Rms) | Some(RatingBasis::SineEquivalent)
    );

    match ceiling {
        None => FeasibilityItem {
            quantity: Quantity::Acceleration,
            label: "peak acceleration (g)".into(),
            computed_value: peak_accel_g,
            rating_value: None,
            rating_basis: None,
            margin_ratio: None,
            margin_db: None,
            verdict: Verdict::NotEvaluated,
        },
        Some(c) => {
            if let Some(basis) = force_basis {
                assumptions.push(format!(
                    "force_shock consumed on the {basis} basis -> accel ceiling F_shock/m_total. \
                     Deterministic peak-accel comparison uses crest = 1.0.{}",
                    if force_basis_is_approx && ceiling_from_force {
                        " Basis is approximate for a deterministic pulse; verdict capped at Advisory."
                    } else {
                        ""
                    }
                ));
            }
            assumptions.push(format!(
                "Acceleration ceiling = {c:.2} g = min(bare-armature rating, force_shock / m_total); peak demand = {peak_accel_g:.2} g."
            ));
            let (ratio, db) = margins(c, peak_accel_g);
            let verdict = if ceiling_from_force && force_basis_is_approx {
                Verdict::Advisory
            } else {
                verdict_from_ratio(ratio)
            };
            FeasibilityItem {
                quantity: Quantity::Acceleration,
                label: "peak acceleration (vs derated shock ceiling)".into(),
                computed_value: peak_accel_g,
                rating_value: Some(c),
                rating_basis: Some(RatingBasis::Peak),
                margin_ratio: Some(ratio),
                margin_db: Some(db),
                verdict,
            }
        }
    }
}

/// The peak-velocity item, basis-aware. For a deterministic pulse the
/// demand is the peak velocity (crest = 1.0); it is compared against `max_velocity_ips`
/// treating the rating as a physical peak bound (Peak/Rms compared directly;
/// SineEquivalent capped at Advisory).
fn velocity_item(rating: Option<RatingValue>, peak_velocity_ips: f64) -> FeasibilityItem {
    match rating {
        None => FeasibilityItem {
            quantity: Quantity::Velocity,
            label: "peak velocity (in/s)".into(),
            computed_value: peak_velocity_ips,
            rating_value: None,
            rating_basis: None,
            margin_ratio: None,
            margin_db: None,
            verdict: Verdict::NotEvaluated,
        },
        Some(r) => {
            let (ratio, db) = margins(r.value, peak_velocity_ips);
            // A velocity limit is a physical envelope (a max slew), compared as a peak
            // bound. Only a sine-equivalent basis (approximate) softens to Advisory.
            let verdict = match r.basis {
                RatingBasis::SineEquivalent => Verdict::Advisory,
                _ => verdict_from_ratio(ratio),
            };
            FeasibilityItem {
                quantity: Quantity::Velocity,
                label: "peak velocity (vs max velocity)".into(),
                computed_value: peak_velocity_ips,
                rating_value: Some(r.value),
                rating_basis: Some(r.basis),
                margin_ratio: Some(ratio),
                margin_db: Some(db),
                verdict,
            }
        }
    }
}

/// The pk-pk displacement item vs the stroke limit. The stroke rating is a physical
/// envelope, so it is a hard pk-pk bound regardless of the stored basis (matching
/// eigen-shaker-db's `displacement_item`).
fn displacement_item(rating: Option<RatingValue>, pk_pk_disp_in: f64) -> FeasibilityItem {
    match rating {
        None => FeasibilityItem {
            quantity: Quantity::Displacement,
            label: "displacement (pk-pk)".into(),
            computed_value: pk_pk_disp_in,
            rating_value: None,
            rating_basis: None,
            margin_ratio: None,
            margin_db: None,
            verdict: Verdict::NotEvaluated,
        },
        Some(r) => {
            let (ratio, db) = margins(r.value, pk_pk_disp_in);
            FeasibilityItem {
                quantity: Quantity::Displacement,
                label: "displacement (pk-pk)".into(),
                computed_value: pk_pk_disp_in,
                rating_value: Some(r.value),
                rating_basis: Some(r.basis),
                margin_ratio: Some(ratio),
                margin_db: Some(db),
                verdict: verdict_from_ratio(ratio),
            }
        }
    }
}

/// The compensation-quality gate item. `Exceeded` when a residual is above
/// tolerance (the pulse is not properly compensated - a pre-arm hard block); `Pass`
/// when both residuals are within tolerance. Carried as an `Acceleration`-quantity item
/// (the eigen-shaker-db `Quantity` enum has no dedicated variant; the label makes the
/// intent explicit and the report renders it like any other line).
fn compensation_quality_item(
    residual_velocity_ips: f64,
    residual_disp_in: f64,
    tol: FeasibilityTolerances,
    assumptions: &mut Vec<String>,
) -> FeasibilityItem {
    let v_abs = residual_velocity_ips.abs();
    let d_abs = residual_disp_in.abs();
    let v_ok = v_abs <= tol.residual_velocity_ips;
    let d_ok = d_abs <= tol.residual_disp_in;
    let verdict = if v_ok && d_ok {
        Verdict::Pass
    } else {
        Verdict::Exceeded
    };
    assumptions.push(format!(
        "Compensation-quality gate (F5): |residual velocity| {v_abs:.4} in/s vs tol {:.4}; |residual displacement| {d_abs:.5} in vs tol {:.5}. Exceeded = the pulse is not properly compensated (pre-arm block).",
        tol.residual_velocity_ips, tol.residual_disp_in
    ));
    // Report the worst residual as a normalized margin (tolerance / demand > 1 = pass),
    // choosing the binding residual so the margin reflects the reason for the verdict.
    let (rating, demand, label_extra) = if !v_ok
        || (v_abs / tol.residual_velocity_ips)
            >= (d_abs / tol.residual_disp_in.max(f64::MIN_POSITIVE))
    {
        (tol.residual_velocity_ips, v_abs, "residual velocity")
    } else {
        (tol.residual_disp_in, d_abs, "residual displacement")
    };
    let (ratio, db) = margins(rating, demand);
    FeasibilityItem {
        quantity: Quantity::Acceleration,
        label: format!("compensation quality ({label_extra} binding)"),
        computed_value: demand,
        rating_value: Some(rating),
        rating_basis: None,
        margin_ratio: Some(ratio),
        margin_db: Some(db),
        verdict,
    }
}

/// `(ratio, db)` margin: `ratio = rating / demand`, `db = 20*log10(ratio)`. Zero demand
/// yields infinite headroom. (Same convention as eigen-shaker-db's `margins`.)
fn margins(rating: f64, demand: f64) -> (f64, f64) {
    if demand <= 0.0 {
        return (f64::INFINITY, f64::INFINITY);
    }
    let ratio = rating / demand;
    let db = if ratio > 0.0 {
        20.0 * ratio.log10()
    } else {
        f64::NEG_INFINITY
    };
    (ratio, db)
}

/// Pass if the rating meets or exceeds the demand (ratio >= 1), else Exceeded.
fn verdict_from_ratio(ratio: f64) -> Verdict {
    if ratio >= 1.0 {
        Verdict::Pass
    } else {
        Verdict::Exceeded
    }
}

/// The overall verdict is the worst HARD verdict among items (same logic as
/// eigen-shaker-db's `worst_verdict`): any `Exceeded` -> Exceeded; else if any `Pass`
/// -> Pass; else Advisory if any Advisory, else NotEvaluated.
fn worst_verdict(items: &[FeasibilityItem]) -> Verdict {
    if items.iter().any(|i| i.verdict == Verdict::Exceeded) {
        Verdict::Exceeded
    } else if items.iter().any(|i| i.verdict == Verdict::Pass) {
        Verdict::Pass
    } else if items.iter().any(|i| i.verdict == Verdict::Advisory) {
        Verdict::Advisory
    } else {
        Verdict::NotEvaluated
    }
}

// =============================================================================
// Minimal double-sided compensation (a placeholder)
// =============================================================================

/// A minimal double-sided compensation helper (a placeholder). Given a main
/// acceleration pulse, prepend a rectangular PRE-shoulder and append a rectangular
/// POST-shoulder whose two amplitudes are solved in closed form so the FULL compensated
/// waveform has ZERO net velocity AND ZERO net displacement.
///
/// # Why this exists and what it is NOT
///
/// The feasibility fixtures need a compensated pulse to test the feasibility path and
/// the compensation-quality gate. A rectangular double-sided compensation is the
/// simplest waveform for which the zero-net-velocity + zero-net-displacement conditions
/// have a clean closed-form solution, so this is the stand-in. It is NOT the real
/// compensation library: full selectable-shape compensation (half-sine / rounded-rect
/// / displacement-optimum shoulders, stroke/velocity optimization, tolerance-band
/// compliance) is separate work.
///
/// # Geometry and closed form
///
/// Let the main pulse be `p[0..M]` at sample rate `fs`, `dt = 1/fs`. The pre-shoulder
/// occupies `n_pre = round(M * pre_frac)` samples before the pulse at amplitude
/// `A_pre`; the post-shoulder occupies `n_post = round(M * post_frac)` samples after
/// the pulse at amplitude `A_post`. `comp_amp_frac` is unused for the SOLVED amplitudes
/// (they are determined by the zero-net conditions) but is retained as the documented
/// knob (a real shape-selectable compensator caps the shoulder amplitude at
/// `comp_amp_frac * peak`); here it only scales the shoulder DURATIONS' starting guess
/// is not needed - see the note below.
///
/// The two zero-net conditions on the concatenated waveform `[pre | main | post]` are
/// linear in `(A_pre, A_post)`:
///
/// - **Net velocity** `= 0`: `A_pre*(n_pre*dt) + I1_main + A_post*(n_post*dt) = 0`,
///   where `I1_main = sum(main)*dt` is the main pulse's velocity change.
/// - **Net displacement** `= 0`: the double integral. Each rectangle contributes a
///   closed-form displacement moment about the waveform end; solving the 2x2 system
///   for `(A_pre, A_post)` gives the exact amplitudes.
///
/// The linear system is assembled by treating each shoulder as a constant-acceleration
/// block and integrating analytically (velocity area + displacement moment), so the
/// solution is exact for the rectangular geometry (no iteration). The returned waveform
/// is `[pre-shoulder | main pulse | post-shoulder]` in g.
///
/// # Verification
///
/// The feasibility fixtures re-integrate the returned waveform with
/// [`eigen_dsp::shock::pulse_kinematics`] and assert both residuals are ~0. A
/// zero-length shoulder (`*_frac <= 0`) degenerates the corresponding amplitude to 0
/// and the helper returns [`CompensationError::Degenerate`] if that leaves the system
/// unsolvable (both shoulders zero, or a singular geometry).
///
/// `comp_amp_frac` note: it is accepted for the forward-compatible signature but does NOT
/// override the solved amplitudes (the zero-net conditions fully determine them for a
/// fixed geometry). A future compensator uses it to CHOOSE the geometry (grow the
/// duration until the amplitude fits under `comp_amp_frac * peak`); here it is recorded
/// and validated (must be in `(0, 1]`) so callers already pass a sane value.
pub fn naive_double_sided_compensation(
    pulse: &[f64],
    pre_frac: f64,
    post_frac: f64,
    comp_amp_frac: f64,
) -> Result<Vec<f64>, CompensationError> {
    if pulse.is_empty() {
        return Err(CompensationError::EmptyPulse);
    }
    if !(comp_amp_frac.is_finite()) || comp_amp_frac <= 0.0 || comp_amp_frac > 1.0 {
        return Err(CompensationError::BadAmplitudeFraction(comp_amp_frac));
    }
    if !pre_frac.is_finite() || !post_frac.is_finite() || pre_frac < 0.0 || post_frac < 0.0 {
        return Err(CompensationError::BadFraction {
            pre_frac,
            post_frac,
        });
    }
    let m = pulse.len();
    let n_pre = (m as f64 * pre_frac).round().max(0.0) as usize;
    let n_post = (m as f64 * post_frac).round().max(0.0) as usize;
    if n_pre == 0 && n_post == 0 {
        return Err(CompensationError::Degenerate(
            "both shoulders have zero length; cannot zero net velocity and displacement".into(),
        ));
    }
    // We solve on a normalized grid (dt = 1); the amplitudes are dimensionless
    // (accel-g) and independent of dt because both zero-net conditions scale by the
    // same dt powers on both sides of each equation. Work in sample units.
    //
    // Waveform layout in samples (index n, 0-based over the concatenated length L):
    //   pre : n in [0, n_pre)
    //   main: n in [n_pre, n_pre + m)
    //   post: n in [n_pre + m, n_pre + m + n_post)
    let l = n_pre + m + n_post;

    // Velocity (first integral, trapezoidal with dt = 1) contribution coefficients.
    // For a constant block of amplitude A over k samples starting at sample s, its
    // trapezoidal velocity area and its displacement moment are assembled by directly
    // building the block and integrating; to keep the closed form robust we compute
    // the main pulse's contribution numerically (trapezoid) and the shoulders'
    // coefficients analytically as functions of A_pre and A_post.
    //
    // Rather than hand-derive the moment integrals (error-prone), we EXPLOIT LINEARITY:
    // the net velocity and net displacement are linear in (A_pre, A_post). We evaluate
    // the pure double-integral residuals at three probe points ((0,0),(1,0),(0,1)) and
    // solve the resulting 2x2 linear system. Each probe is one call to the same
    // trapezoidal double-integrator the verification uses, so the solved waveform is
    // consistent with the integrator that will check it (no transcription gap).
    let residuals = |a_pre: f64, a_post: f64| -> (f64, f64) {
        let wave = assemble(pulse, n_pre, n_post, a_pre, a_post, comp_amp_frac);
        double_integral_residuals(&wave)
    };

    let (v00, d00) = residuals(0.0, 0.0);
    let (v10, d10) = residuals(1.0, 0.0);
    let (v01, d01) = residuals(0.0, 1.0);

    // Linear model: [v; d] = [v00; d00] + M * [A_pre; A_post], where the columns of M
    // are the per-unit-amplitude deltas.
    let a11 = v10 - v00;
    let a12 = v01 - v00;
    let a21 = d10 - d00;
    let a22 = d01 - d00;
    // Solve M * x = -[v00; d00].
    let det = a11 * a22 - a12 * a21;
    if det.abs() <= f64::EPSILON * (a11.abs() + a12.abs() + a21.abs() + a22.abs()).max(1.0) {
        return Err(CompensationError::Degenerate(
            "compensation geometry is singular (e.g. only one shoulder present); pick nonzero pre and post fractions".into(),
        ));
    }
    let b1 = -v00;
    let b2 = -d00;
    let a_pre = (b1 * a22 - a12 * b2) / det;
    let a_post = (a11 * b2 - b1 * a21) / det;

    let _ = l; // length is implicit in the assembled waveform.
    Ok(assemble(pulse, n_pre, n_post, a_pre, a_post, comp_amp_frac))
}

/// Assemble the concatenated `[pre | main | post]` waveform (g). `comp_amp_frac` is
/// accepted for the forward-compatible signature but does not scale the (solved) amplitudes;
/// the rectangles are constant at `a_pre` / `a_post`.
fn assemble(
    pulse: &[f64],
    n_pre: usize,
    n_post: usize,
    a_pre: f64,
    a_post: f64,
    _comp_amp_frac: f64,
) -> Vec<f64> {
    let mut wave = Vec::with_capacity(n_pre + pulse.len() + n_post);
    wave.extend(std::iter::repeat_n(a_pre, n_pre));
    wave.extend_from_slice(pulse);
    wave.extend(std::iter::repeat_n(a_post, n_post));
    wave
}

/// Net velocity and net displacement (the END-of-record values) of a waveform under
/// the PURE trapezoidal double-integral with `dt = 1` (sample units). This is the
/// same integrator shape [`eigen_dsp::shock::pulse_kinematics`] uses (no high-pass),
/// evaluated in sample units so the solved amplitudes are dt-independent.
fn double_integral_residuals(accel: &[f64]) -> (f64, f64) {
    let n = accel.len();
    if n == 0 {
        return (0.0, 0.0);
    }
    let mut vel = vec![0.0f64; n];
    let mut running_v = 0.0f64;
    for i in 1..n {
        running_v += 0.5 * (accel[i - 1] + accel[i]);
        vel[i] = running_v;
    }
    let mut running_d = 0.0f64;
    for i in 1..n {
        running_d += 0.5 * (vel[i - 1] + vel[i]);
    }
    (vel[n - 1], running_d)
}

/// Errors from [`naive_double_sided_compensation`].
#[derive(Debug, Clone, PartialEq)]
pub enum CompensationError {
    /// The main pulse was empty.
    EmptyPulse,
    /// `comp_amp_frac` was not in `(0, 1]`.
    BadAmplitudeFraction(f64),
    /// A shoulder fraction was negative or non-finite.
    BadFraction { pre_frac: f64, post_frac: f64 },
    /// The geometry cannot zero both net velocity and displacement (e.g. only one
    /// shoulder present, or a singular system). Carries a note.
    Degenerate(String),
}

impl std::fmt::Display for CompensationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompensationError::EmptyPulse => write!(f, "compensation: main pulse is empty"),
            CompensationError::BadAmplitudeFraction(v) => {
                write!(f, "compensation: comp_amp_frac {v} must be in (0, 1]")
            }
            CompensationError::BadFraction { pre_frac, post_frac } => write!(
                f,
                "compensation: shoulder fractions must be finite and >= 0 (pre {pre_frac}, post {post_frac})"
            ),
            CompensationError::Degenerate(s) => write!(f, "compensation geometry degenerate: {s}"),
        }
    }
}

impl std::error::Error for CompensationError {}
