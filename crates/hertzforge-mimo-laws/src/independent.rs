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
//! 2. **The constant per line.** `c` is the parameter times the largest
//!    diagonal element of `A^T A` on each line, each line solved on its own as
//!    Mayes and Rohe process each line separately.
//! 3. **The negative rule.** A negative solved level is set to zero, the rule
//!    of Hunter, Cross and Nelson and of Schultz. Mayes and Rohe set it to "a
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
//!    3 x 3 structure with the test harness's 1 percent noise, over loops 10 to
//!    30 the drive changed 1.70 dB a loop on average and up to 3.01 dB (the
//!    rise limit) while the worst control moved 0.11 dB; with `convergence`
//!    0.5 the drive changed 0.74 dB on average and up to 1.25 dB
//!    (`tests/independent.rs`).
//! 7. **The frequency response.** Both laws use the frequency response the
//!    core hands them on each call. `S+` is formed again when it changes (the
//!    FRF updated during the run). The nearly independent reference is formed
//!    once, at the seed, from the frequency response handed then; later updates
//!    reach the closed loop, as for `min_drive`.
//! 8. **The reference.** Both laws read the specified reference's autospectra
//!    only; its cross terms are not used.
//! 9. **The trial response.** For the nearly independent reference the levels
//!    are solved with the published negative rule (zero) and the trial response
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
//!     says how to close the loop on the method.
//! 12. **Shapes.** Both laws take all three shapes: with more controls than
//!     drives the solve is least squares, with fewer it is regularized minimum
//!     norm; nothing in either method needs a square system.
//! 13. **Outside the band** every drive is zero and nothing is counted.

use crate::factor::{FactorLoop, LoopSettings};
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
/// `A_kd = |H_kd|^2` (Mayes and Rohe, Eq. 3); `S+ = (A^T A + c I)^-1 A^T`
/// (their Eq. 4) with `c = constant * max_d (A^T A)_dd` (their section 7.2),
/// formed as `V diag(s / (s^2 + c)) U^T` from the singular value decomposition
/// after dropping singular values at or below the numerical floor.
fn squared_inverse(frf_band: &MatrixStack, constant: f64) -> Result<Vec<f64>, LawError> {
    let (nb, nc, nd) = frf_band.dims();
    let mut a = MatrixStack::zeros(nb, nc, nd);
    for (x, h) in a.data_mut().iter_mut().zip(frf_band.data()) {
        *x = C64::new(h.norm_sqr(), 0.0);
    }
    let svd = linalg::svd(&a)?;
    let k = svd.k();
    let mut plus = vec![0.0; nb * nd * nc];
    for i in 0..nb {
        let line = a.line(i);
        let normal_max = (0..nd)
            .map(|d| (0..nc).map(|r| line[r * nd + d].re.powi(2)).sum::<f64>())
            .fold(0.0f64, f64::max);
        let c = constant * normal_max;
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
    }
    Ok(plus)
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
    /// Per line, how many drives' solved levels were negative and were set by
    /// the negative rule; zero outside the band.
    pub negative: Vec<f64>,
}

/// The independent drive levels for `reference` through `frf` on the lines
/// of `band`: per line `g = S+ r` with `r` the reference autospectra and `S+`
/// the regularized inverse of the squared magnitudes of `frf` (Mayes and
/// Rohe, Eqs. 3 to 5), each negative level set to `negative_floor` times the
/// line's largest (0: zero).
pub fn independent_levels(
    frf: &MatrixStack,
    reference: &HermitianStack,
    band: (usize, usize),
    regularization_constant: f64,
    negative_floor: f64,
) -> Result<IndependentLevels, LawError> {
    check_dims(frf, reference, band)?;
    let plus = squared_inverse(&band_matrix(frf, band), regularization_constant)?;
    Ok(levels_from(
        &plus,
        reference,
        frf.dims(),
        band,
        negative_floor,
    ))
}

fn levels_from(
    plus: &[f64],
    reference: &HermitianStack,
    (lines, nc, nd): (usize, usize, usize),
    band: (usize, usize),
    negative_floor: f64,
) -> IndependentLevels {
    let mut levels = vec![0.0; lines * nd];
    let mut negative = vec![0.0; lines];
    for (i, line) in (band.0..band.1).enumerate() {
        let r = autospectra(reference, line);
        let g = &mut levels[line * nd..(line + 1) * nd];
        // Mayes and Rohe, Eq. 5: |v|^2 = S+ |x|^2.
        apply(&plus[i * nd * nc..(i + 1) * nd * nc], &r, nd, nc, g);
        negative[line] = negative_rule(g, negative_floor) as f64;
    }
    IndependentLevels { levels, negative }
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
    let levels = independent_levels(frf, reference, band, regularization_constant, 0.0)?;
    let (lines, nc, nd) = frf.dims();
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
        help: "The Tikhonov constant of the independent-drive solve, relative to the largest \
               diagonal element of the normal matrix of squared frequency-response \
               magnitudes on each line. 0.01 is the published value; 0 solves by the plain \
               pseudo-inverse.",
        kind: ParamKindDecl::Number {
            unit: "",
            min: 0.0,
            max: 1.0,
            step: 1e-3,
            default: 0.01,
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
    /// The Tikhonov constant relative to the largest diagonal of `A^T A`.
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
}

impl Default for IndependentConfig {
    fn default() -> Self {
        IndependentConfig {
            regularization_constant: 0.01,
            negative_floor: 0.0,
            limit_up_percent: 100.0,
            limit_down_percent: 67.0,
            convergence: 1.0,
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
            other => return Err(LawError(format!("parameter `{other}` is not declared"))),
        }
        Ok(())
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
    /// The in-band frequency response `plus` was formed from.
    key: Vec<C64>,
    plus: Vec<f64>,
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
            plus: Vec::new(),
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
        if self.plus.is_empty() || self.key.as_slice() != inband {
            self.plus = squared_inverse(&band_matrix(frf, band), self.cfg.regularization_constant)?;
            self.key = inband.to_vec();
        }
        Ok(())
    }

    fn outputs(
        &mut self,
        drive: HermitianStack,
        negative: &[f64],
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
                negative_values(negative),
                value("limited", DiagnosticData::Scalar(limited as f64)),
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
        let lv = levels_from(
            &self.plus,
            &s.reference,
            (s.lines, s.controls, s.drives),
            s.band,
            self.cfg.negative_floor,
        );
        let drive = diagonal_drive(&lv.levels, s.lines, s.drives);
        Ok(self.outputs(drive, &lv.negative, 0))
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
        let mut levels = vec![0.0; lines * nd];
        let mut negative = vec![0.0; lines];
        let mut limited = 0usize;
        let mut err = vec![0.0; nc];
        let mut step = vec![0.0; nd];
        for (i, line) in (band.0..band.1).enumerate() {
            let want = autospectra(&self.setup.reference, line);
            let got = inputs.response.diagonal(line);
            for k in 0..nc {
                err[k] = want[k] - got[k];
            }
            // Mayes and Rohe, Eq. 6: |v_d|^2 = |v_i|^2 + S+ (|x_d|^2 - |x_i|^2),
            // times the convergence factor (1: the whole step).
            apply(
                &self.plus[i * nd * nc..(i + 1) * nd * nc],
                &err,
                nd,
                nc,
                &mut step,
            );
            let g = &mut levels[line * nd..(line + 1) * nd];
            let last: Vec<f64> = played
                .diagonal(line)
                .into_iter()
                .map(|v| v.max(0.0))
                .collect();
            for d in 0..nd {
                g[d] = last[d] + self.cfg.convergence * step[d];
            }
            negative[line] = negative_rule(g, self.cfg.negative_floor) as f64;
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
        let drive = diagonal_drive(&levels, lines, nd);
        Ok(self.outputs(drive, &negative, limited))
    }
}

impl BuiltinMimoLaw for IndependentDrivesLaw {
    fn spec() -> LawSpec {
        LawSpec {
            name: "independent drives",
            version: "1.0.0",
            author: "Hertzforge",
            summary: "Uncorrelated drives, a diagonal drive matrix on every line. The seed solves \
                      the real system of squared frequency-response magnitudes for the drive \
                      autospectra that produce the reference autospectra, by a Tikhonov \
                      pseudo-inverse, with negative levels set to zero (Mayes and Rohe; \
                      Schultz; Hunter, Cross and Nelson). Each loop adds the same solve of the \
                      autospectrum error to the levels last played, each drive limited to \
                      +100 / -67 percent a loop (Mayes and Rohe); a convergence factor below \
                      1 takes part of the step. It matches autospectra only: \
                      the cross terms are whatever the structure makes of uncorrelated drives.",
            params: vec![
                regularization_constant_param(),
                ParamDecl {
                    key: "negative_floor",
                    label: "Negative level floor",
                    help: "A solved drive autospectrum below zero is set to this fraction of \
                           the line's largest solved level. 0 sets it to zero, the published \
                           rule.",
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
}

impl Default for NearlyIndependentConfig {
    fn default() -> Self {
        NearlyIndependentConfig {
            regularization_constant: 0.01,
            convergence: 0.5,
        }
    }
}

impl NearlyIndependentConfig {
    /// Set the parameter `key`.
    pub fn set(&mut self, key: &str, v: &ParamVal) -> Result<(), LawError> {
        match key {
            "regularization_constant" => self.regularization_constant = v.number(key)?,
            "convergence" => self.convergence = v.number(key)?,
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
    /// The seed's negative-level counts, reported on every loop.
    negative: Vec<f64>,
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
        Ok(NearlyIndependentLaw {
            setup: setup.clone(),
            cfg: cfg.clone(),
            inner: None,
            negative: Vec::new(),
        })
    }

    fn with_count(&self, mut out: MimoLoopOutputs) -> MimoLoopOutputs {
        out.diagnostics.push(negative_values(&self.negative));
        out
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
        let (target, levels) = nearly_independent_reference(
            inputs.frf,
            &self.setup.reference,
            self.setup.band,
            self.cfg.regularization_constant,
        )?;
        let settings = LoopSettings {
            convergence: self.cfg.convergence,
            cap_up_db: 3.5,
            cap_down_db: 6.0,
            rank_tol: 1e-6,
        };
        let mut inner = FactorLoop::new(&self.setup, &target, pareto::loop_rule(), settings)?;
        let out = inner.seed(inputs.frf)?.outputs(true);
        self.inner = Some(inner);
        self.negative = levels.negative;
        Ok(self.with_count(out))
    }

    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
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
            version: "1.0.0",
            author: "Hertzforge",
            summary: "Keeps the specified autospectra and takes each pair's coherence and phase \
                      from the response that independent drives would produce: the drive \
                      autospectra solved from the squared frequency-response magnitudes, \
                      negative levels set to zero, their response through the frequency \
                      response, rescaled to the specified autospectra (Smallwood's independent \
                      drive algorithm as Cap, Norman and Smallwood state it; the nearly \
                      independent drive algorithm of Hunter, Cross and Nelson). The drives \
                      are coupled just enough to reach that reference. Then runs the default \
                      law's closed loop on it.",
            params: vec![regularization_constant_param(), convergence_param()],
            diagnostics: vec![
                diag_condition(),
                diag_kept(),
                diag_correction(),
                diag_clamped(),
                DiagDecl {
                    help: "How many drives' independent levels were solved below zero on \
                           each line when the reference was formed at the seed, and were set \
                           to zero; the same on every loop. 0 out of band.",
                    ..diag_negative()
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
