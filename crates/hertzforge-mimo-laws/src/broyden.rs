//! Law 9: the Broyden congruence law. It specifies autospectra only.
//!
//! The first drive is the inverse of the reference, `H^+ G_ref H^+^H`. After
//! that the law never builds a drive again: on each loop and each line it
//! multiplies each drive by its own gain, and with phase steering turns its
//! phase, `X_new = D X D^H` with `D = diag(e^(u_i + j theta_i))`. A congruence
//! by a diagonal matrix leaves the coherence between every pair of drives as
//! it was and keeps a positive semi-definite drive positive semi-definite.
//! The gains come from a sensitivity matrix of the log response to the log
//! gain, which starts as the model's (from the frequency response and the
//! current drive) and is corrected each loop from what the last step did, by
//! a damped Broyden secant update.
//!
//! # Credit
//!
//! The law is N. Hunter's, from his unpublished note "The Broyden Congruence
//! Law, Step by Step" (17 November 2026), a private communication, used with
//! his permission. This implementation was written from the note; the section
//! numbers in the comments below are the note's. The secant update is C. G.
//! Broyden's, "A class of methods for solving nonlinear simultaneous
//! equations", Mathematics of Computation 19 (1965).
//!
//! # The loop, as the note orders it
//!
//! The first drive (section 3): the inverse of the reference with the
//! crate's truncated pseudo-inverse (`regularization` `truncated`,
//! `regularization_amount` 1e-3: singular values at or below 1e-3 times the
//! largest are dropped, the note's cutoff); lines with no specification get
//! no drive; the floor, set here for the run (Choice 14; with the default
//! regularization it lifts nothing on the first drive); the drive-coherence
//! cap; and, when `start_cap` is on, the note's level cap of the first drive.
//! The controller owns the level, so the cap is off by default and the first
//! drive is at full level; `start_cap` on with `start_cap_db` -9 is the note's.
//!
//! One feedback cycle (section 4), per line:
//!
//! 1. The base drive: the law's last command if the drive as played is within
//!    `realize_tol_db` of it (the median over the band of the trace ratio in
//!    decibels), otherwise the drive as played; and, line by line, the drive
//!    as played on a line where it is more than `realize_tol_db` from the
//!    command (Choice 19, Hertzforge's).
//! 2. The log error `eps_m = ln(t_m / y_m)`; a control measured at zero, below
//!    zero or not a number, or with no specification, is left out of every sum.
//! 3. The model sensitivity: shares `a_mi = H_mi (X H^H)_im`,
//!    `S_mi = 2 Re(a_mi) / y_m`, each row summing to 2; with phase steering
//!    the phase columns `-2 Im(a_mi) / y_m`, each row summing to 0.
//! 4. The Broyden update of the learned sensitivity with its trust weight
//!    `w_m = p_m^2 / (p_m^2 + 2/n)`, its skip rule and its three corrections.
//! 5. The anchor toward the model, `(1 - a) S + a S_model`.
//! 6. The coherence weights `(gamma_m^2)^p`.
//! 7. The common level split out first.
//! 8. The ridge solve.
//! 9. The loop gain (`convergence`) and the resonance schedule.
//! 10. The step limit with its three predictions, repeated up to six times.
//! 11. The congruence.
//! 12. The ceiling, the floor and the drive-coherence cap, in that order.
//! 13. The state kept, after them.
//!
//! # Choices This Implementation Makes
//!
//! Where the note leaves a step open, it is decided here. Every item is to
//! confirm with the author. "The table" is the note's fourteen-cycle table,
//! "the phase table" its phase steering on and off table.
//!
//! 1. **Learning off keeps the anchor** (to confirm with the author). With
//!    `broyden_gain` 0 the update does nothing and the anchor still acts, so
//!    the sensitivity in use starts at the first loop's model and moves 5
//!    percent a loop toward the current model. Decided by the table: this
//!    reading reproduces its learning-off column to 0.005 dB; the model's
//!    sensitivity taken whole every loop misses it by 0.03 dB. With
//!    `model_anchor` 1 the sensitivity in use is the model's every loop. The
//!    note's wording in one place reads the other way (learning off "freezes"
//!    the sensitivity at the model); the table decides for this reading.
//! 2. **The table was run without phase steering** (to confirm with the
//!    author). On the note's real example the phase columns are zero, and
//!    the only difference phase steering makes is `P = 2N` in the ridge
//!    constant; the table's numbers are those of `P = N`. The default here is
//!    the note's, phase steering on.
//! 3. **The loop gain and the resonance gain scale the phases too** (to
//!    confirm with the author). Decided by the phase table: with the phases
//!    scaled it is reproduced to the printed figure; unscaled, cycle 10 reads
//!    0.02 dB where the note prints 0.06.
//! 4. **The 15 degree rule** (to confirm with the author) joins the step
//!    limit's ratio: the largest phase step over `max_phase_step_deg` is a
//!    fourth ratio, and the whole step (gains and phases) is scaled by the
//!    largest, so the direction of the solve is kept. The phase table does
//!    not decide it. So a line whose phase step is limited also has its level
//!    correction slowed on that loop, because the one scale is applied to
//!    both.
//! 5. **Phases are per-loop increments** (to confirm with the author): each
//!    loop's phases turn the base drive; they are not accumulated. The first
//!    loop's phase change in the update is the phases of the loop before,
//!    zero for the first drive. Solved phases are shifted to zero mean over
//!    the drives that reach a control, as the note asks; in exact arithmetic
//!    the shift does nothing, since every phase row of the sensitivity sums
//!    to zero and so do the solved phases.
//! 6. **The model prediction with phases** (to confirm with the author) is
//!    `(H D X D^H H^H)_mm` with the complex `D`; the ceiling's expected
//!    response `y_m e^((S u)_m)` uses the whole `u`, gains and phases.
//! 7. **The skip rule** (to confirm with the author): the update is skipped
//!    on a line where the largest drive gain change of the last step,
//!    `20/ln 10 max_i |du_i|` dB, is below `min_du_db`. Phases do not count.
//!    The three corrections and the anchor run every loop from the second,
//!    whether or not the update ran. The table does not decide either.
//! 8. **The Broyden denominator** `du^T du + delta` uses `delta` 1e-12
//!    (to confirm with the author); the skip rule keeps `du^T du` above
//!    3e-5 at the default.
//! 9. **The entry bound** `|S_mi| <= max(2, 2 |S_model_mi|)` (to confirm
//!    with the author) uses the model of the current base drive, on the phase
//!    columns too. The phase rows are shifted to add to zero by subtracting
//!    their mean.
//! 10. **The number of averages** `n` is half the response's degrees of
//!     freedom, capped by `effective_averages` when that is above 0 (to
//!     confirm with the author). A response with no degrees of freedom
//!     gives no learning.
//! 11. **Channels left out** (to confirm with the author): a control with a
//!     zero (or negative, or not a number) target, a measurement that is not
//!     good, or a model response of zero (a zero row of the frequency
//!     response) is left out of the update, the level split, the solve, the
//!     learned prediction and the expected response of the ceiling. A row is
//!     updated only where the control was good on this loop and the last. A
//!     line with every control left out takes no step; its drive is the base
//!     drive after the ceiling, the floor and the cap.
//! 12. **A drive that reaches no control** (a zero column of the model
//!     sensitivity) is not stepped: its gain and phase are 0. A drive pushed
//!     very far down on a line is the near case of the same thing, and does
//!     not come back on its own (see "Limits of the method as built").
//! 13. **Zero denominators**: the level split with `sum w r^2 = 0` takes no
//!     level (`c = 0`); a ridge system that is not positive definite (only
//!     with every weight zero) takes no step; a shrinking command level of
//!     zero gives a gain change of zero.
//! 14. **The floor is relative to a drive, not to the specification, and is
//!     set once, at the first drive** (to confirm with the author). The note
//!     lifts a line whose drive trace is below 1e-8 times the specification's
//!     trace. That compares volts squared with response units squared, so
//!     whether it acts depends on the units of the frequency response: with
//!     the response 1e3 times larger in its units, the note's floor held most
//!     lines up and overdrove the structure by 14 dB, at 1e4 by 35 dB. Here
//!     the floor of a line is `FLOOR_REL` (1e-8) times the trace of the model
//!     drive `H^+ S_ref H^+^H`, with `H` the frequency response given to the
//!     first drive and `H^+` the truncated pseudo-inverse at a fixed relative
//!     cutoff of 1e-3 (`FLOOR_RCOND`), whatever `regularization` is; it is
//!     kept for the run and formed again only by a new first drive. Where
//!     that trace is zero or not finite the line has no floor. A drive with a
//!     positive trace is scaled up to the floor; an all-zero drive becomes
//!     uncorrelated drives at it. The truncated inverse whatever the
//!     regularization: in review, with `regularization` `none` on a structure
//!     of condition 1e6, a floor from the user's inverse lifted every line and
//!     left one control 32.66 dB over its target. Fixed at the first drive:
//!     in review, with the estimate falling to 1e-5 of the true one on a
//!     single call, a floor formed on each call lifted the drive 20 dB for
//!     that loop. The bound: the truncated inverse has no singular value
//!     above 1e3 over the largest of `H`, so the floor's trace is at most
//!     `FLOOR_REL` 1e6 `tr S_ref` over the square of that largest singular
//!     value, and a drive at the floor answers at the controls, by the first
//!     drive's frequency response, with a total of at most `FLOOR_REL` 1e6
//!     (1e-2) of the target's total: 20 dB under it. As in the note, the floor
//!     comes after the ceiling and has the last word over it, since letting
//!     the ceiling overrule it lets a reading stuck high drive the line toward
//!     zero, from where the step limit needs one loop per `max_step_db` to
//!     come back.
//! 15. **The drive-coherence cap**: a pair whose coherence is above
//!     `max_drive_coherence` has its cross spectrum shrunk to it, phase kept;
//!     if that leaves a negative eigenvalue (only with three drives or more),
//!     negative eigenvalues are set to zero by a Jacobi eigendecomposition and
//!     the matrix is rebuilt, not capped again (to confirm with the author).
//! 16. **The first drive's level cap**, when on, compares the summed targets
//!     with the summed model response of the drive after its floor and cap,
//!     line by line, as the note's reason for putting the cap last says.
//! 17. **The ceiling** (to confirm with the author) compares the expected
//!     response of each control left in with its limit and the model response
//!     of each specified control, left in or not, with the limit 20 dB higher.
//! 18. **The resonance baseline is Hertzforge's, not the author's** (8.1 item
//!     2; decided with the maintainers). The note names a half-power peak
//!     detector it does not define. Here, for each control, the size of its
//!     row of the frequency response (the root sum of squares over the
//!     drives) is divided by its running median over the in-band lines in a
//!     window centered on the line: `resonance_halfwidth_oct` octaves on each
//!     side, made symmetric in lines (as many lines above as below, the
//!     smaller count of the two sides), so a smooth trend leaves the ratio at
//!     one, and shrinking symmetrically near the band's edges. With fewer than
//!     five lines in the window, or a zero baseline, the ratio is one, so no
//!     cut is made on the lowest lines of the band (below about line 8 at the
//!     default width). `p` is
//!     the largest ratio over the controls specified on the line, and the step
//!     is multiplied by `max(resonance_min_gain, 1 / (1 + s max(0, p - 1)))`.
//!     It is formed again whenever the estimate changes.
//! 19. **The base drive line by line is Hertzforge's, not the note's** (to
//!     confirm with the author). In addition to the note's median over the
//!     band, a line whose played drive's trace is more than `realize_tol_db`
//!     from its command's builds on the drive as played (a line where either
//!     trace is zero or not finite is left to the median); `base_played_lines`
//!     counts them. The core holds lines back (drive ceilings, limit
//!     channels), and building on a command the core did not play winds the
//!     command up on those lines: in a test with five lines held 10 dB down
//!     for ten loops the command rose 39 dB above the drive played and the
//!     first loop after the hold was released was 30 dB high there. With this
//!     test the command stayed within 6 dB of the drive played, and the first
//!     loop after the release was 4 dB below the targets there. The rule
//!     assumes the controller holds a line by an absolute limit or by its
//!     change cap, which is how the controller does it; a played drive that
//!     stayed a fixed ratio beyond `realize_tol_db` under the command on every
//!     loop would walk the command down (31.4 dB in ten loops at a fixed 6 dB,
//!     measured in review), and no path in the controller is known to do that.
//! 20. **The row-sum clip** adds its correction to the `N` magnitude entries
//!     of the row only (the note says "every entry of the row", and divides
//!     by `N`), so the phase entries are not moved by it (to confirm with the
//!     author).
//! 21. **The step limit's predictions** run over different controls (to
//!     confirm with the author): the learned one over the controls left in;
//!     the model's exact one over every specified control with a positive
//!     model response, whether or not its measurement is good; the drive's
//!     own over the drives.
//! 22. **The median of the base rule** is over the specified in-band lines
//!     where the played and the commanded traces are both positive and
//!     finite; a band with no such line builds on the command (to confirm
//!     with the author).
//!
//! # What the note says of its own limits
//!
//! The note calls the law a prototype. In the author's runs on a nonlinear
//! structure it settled 3.2 to 3.6 dB above the best a matched drive could
//! do. Phase steering's benefit mostly disappears with realistic 30-average
//! estimates (about 0.15 dB better where the response is noisy, 0.1 to 0.6 dB
//! worse where the frequency response is wrong), and it relies on the
//! hardware reproducing the phase between drives.
//!
//! # Limits of the method as built
//!
//! The law moves each drive by a factor, so a drive whose level on a line has
//! been pushed very far down has almost no share in any response, the solve
//! gives it almost no step, and it does not come back on its own (the
//! reviewer's reading of what was measured). Measured in review, with a
//! control reading 40 dB high on four lines for 60 loops: 140 loops after the
//! reading returned, the worst control there was still 3.09 dB off at the
//! defaults, against 0.09 dB in a run without the fault; with phase steering
//! off it was 5.15 dB off, against 0.21 dB without the fault, and one drive
//! was 67 dB down on one of those lines. With phase steering off a fault of
//! 10 loops recovered. This is a limit of the method as built and a point for
//! the author.
//!
//! # Ideas Not Built
//!
//! - The note's dither (a small random gain each loop): off by its own
//!   default, and a law here is deterministic.
//! - The two offline start-up seeds (the buzz seed and the fast-law seed):
//!   the note does not define them enough to build, and it says they are not
//!   in its law.
//!
//! # Parity
//!
//! The law file runs this code as WebAssembly. Logarithms, exponentials,
//! sines and cosines are evaluated on every line of every loop, and the
//! system libraries of the two targets need not agree in the last bit; so
//! they are evaluated here by [`detmath`], with arithmetic and square roots
//! only, which take the same steps on both. The first drive and the floor,
//! both formed at the first drive, use the crate's pseudo-inverse, as every
//! other law's first drive does. The coherence cap's repair with three drives or more uses the shared
//! Jacobi solver, which calls the library's `hypot`: on that path parity is
//! measured by the tests, not guaranteed by construction.

// Index loops over lines, controls and drives mirror the matrix algebra.
#![allow(clippy::needless_range_loop)]

use crate::pareto;
use crate::pinv::ALL_SHAPES;
use crate::spec::{DiagDecl, DiagKind, LawSpec, ParamDecl, ParamKindDecl, ParamVal};
use crate::support::{
    band_hermitian, band_matrix, convergence_param, diag_correction, largest_change_db,
    regularization, regularization_params, ZERO,
};
use crate::BuiltinMimoLaw;
use hertzforge_sdm::law::{
    DiagnosticData, DiagnosticValue, LawError, MimoControlLaw, MimoInput, MimoLoopInputs,
    MimoLoopOutputs, MimoSetup, MimoShape,
};
use hertzforge_sdm::linalg;
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

const NEEDS: [MimoInput; 1] = [MimoInput::LastDrive];

/// `delta` in the Broyden denominator `du^T du + delta` (Choice 8).
pub const BROYDEN_DELTA: f64 = 1e-12;
/// The floor of a specified line's drive trace, relative to the trace of its
/// model drive `H^+ S_ref H^+^H` (note 4.12; Choice 14).
pub const FLOOR_REL: f64 = 1e-8;
/// The relative cutoff of the truncated pseudo-inverse the floor's model
/// drive is formed with, whatever the law's regularization (Choice 14).
pub const FLOOR_RCOND: f64 = 1e-3;
/// The model check of the ceiling is this much looser than the ceiling: 20 dB
/// (note 4.12).
pub const MODEL_CEILING_FACTOR: f64 = 100.0;
/// The step limit scales the step at most this many times (note 4.10).
pub const STEP_REPEATS: usize = 6;
/// A resonance window with fewer lines than this gives a ratio of one.
pub const MIN_WINDOW_LINES: usize = 5;

// ---------------------------------------------------------------------------
// Elementary functions with the same steps on every target
// ---------------------------------------------------------------------------

/// `ln`, `exp`, `sin` and `cos` by arithmetic and square roots only.
///
/// The native law and the law file compile the same code for two targets; a
/// library logarithm may differ between them in the last bit, and the law
/// takes a logarithm and an exponential of every control on every line of
/// every loop, so those differences would feed back. These functions use only
/// IEEE operations, which round the same way everywhere. Each is accurate to a
/// few units in the last place, which the tests measure against the standard
/// library.
pub mod detmath {
    // The split constants by their bits, so no decimal rounding stands
    // between them and the values the algorithms need.
    /// `ln 2`, high part, 6.93147180369123816490e-01.
    const LN2_HI: f64 = f64::from_bits(0x3fe6_2e42_fee0_0000);
    /// `ln 2`, low part, 1.90821492927058770002e-10.
    const LN2_LO: f64 = f64::from_bits(0x3dea_39ef_3579_3c76);
    const INV_LN2: f64 = std::f64::consts::LOG2_E;
    const SQRT2: f64 = std::f64::consts::SQRT_2;
    /// `ln 10`.
    pub const LN10: f64 = std::f64::consts::LN_10;
    /// `pi / 2`, first 33 bits, 1.57079632673412561417e+00.
    const PIO2_1: f64 = f64::from_bits(0x3ff9_21fb_5440_0000);
    /// `pi / 2 - PIO2_1`, 6.07710050650619224932e-11.
    const PIO2_1T: f64 = f64::from_bits(0x3dd0_b461_1a62_6331);
    const TWO_OVER_PI: f64 = std::f64::consts::FRAC_2_PI;

    /// `2^k` for `k` in the normal range.
    fn pow2(k: i64) -> f64 {
        f64::from_bits(((k + 1023) as u64) << 52)
    }

    /// `e^x`.
    pub fn exp(x: f64) -> f64 {
        if x.is_nan() {
            return x;
        }
        if x > 709.782_712_893_384 {
            return f64::INFINITY;
        }
        if x < -745.2 {
            return 0.0;
        }
        // x = k ln 2 + r, |r| <= ln 2 / 2, with ln 2 in two parts so r is exact
        // to the rounding of one product.
        let k = (x * INV_LN2 + 0.5).floor();
        let r = (x - k * LN2_HI) - k * LN2_LO;
        // Taylor series to r^16/16!: below 1e-19 at |r| = 0.35.
        let mut p = 1.0 / 20_922_789_888_000.0;
        for d in [
            1_307_674_368_000.0,
            87_178_291_200.0,
            6_227_020_800.0,
            479_001_600.0,
            39_916_800.0,
            3_628_800.0,
            362_880.0,
            40_320.0,
            5_040.0,
            720.0,
            120.0,
            24.0,
            6.0,
            2.0,
            1.0,
            1.0,
        ] {
            p = p * r + 1.0 / d;
        }
        let k = k as i64;
        // Two factors keep each power of two normal, down to the subnormals.
        let (a, b) = if k < -1000 {
            (k + 600, -600)
        } else if k > 1000 {
            (k - 100, 100)
        } else {
            (k, 0)
        };
        p * pow2(a) * pow2(b)
    }

    /// `ln x`: not a number below zero, minus infinity at zero.
    pub fn ln(x: f64) -> f64 {
        if x.is_nan() || x < 0.0 {
            return f64::NAN;
        }
        if x == 0.0 {
            return f64::NEG_INFINITY;
        }
        if x.is_infinite() {
            return x;
        }
        let (mut x, mut e) = (x, 0i64);
        if x < f64::MIN_POSITIVE {
            x *= pow2(54);
            e = -54;
        }
        let bits = x.to_bits();
        e += ((bits >> 52) & 0x7ff) as i64 - 1023;
        let mut m = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | (1023u64 << 52));
        if m > SQRT2 {
            m *= 0.5;
            e += 1;
        }
        // ln m = 2 atanh(s), s = (m - 1)/(m + 1), |s| <= 0.1716.
        let s = (m - 1.0) / (m + 1.0);
        let s2 = s * s;
        let mut p = 0.0;
        for k in (0..12).rev() {
            p = p * s2 + 1.0 / (2 * k + 1) as f64;
        }
        let e = e as f64;
        e * LN2_HI + (2.0 * s * p + e * LN2_LO)
    }

    /// `(sin x, cos x)`, accurate for `|x|` up to about 1e5.
    pub fn sin_cos(x: f64) -> (f64, f64) {
        if !x.is_finite() {
            return (f64::NAN, f64::NAN);
        }
        let k = (x * TWO_OVER_PI + 0.5).floor();
        let r = (x - k * PIO2_1) - k * PIO2_1T;
        let r2 = r * r;
        // Taylor series to r^19 and r^18: below 1e-18 at |r| = pi/4.
        let mut s = 0.0;
        let mut c = 0.0;
        for n in (0..10).rev() {
            let (two_n, two_n1) = ((2 * n) as f64, (2 * n + 1) as f64);
            // s accumulates (-1)^n r^(2n)/(2n+1)!, c (-1)^n r^(2n)/(2n)!.
            s = 1.0 - s * r2 / ((two_n1 + 1.0) * (two_n1 + 2.0));
            c = 1.0 - c * r2 / ((two_n + 1.0) * (two_n + 2.0));
        }
        let sin_r = r * s;
        let cos_r = c;
        match (k as i64).rem_euclid(4) {
            0 => (sin_r, cos_r),
            1 => (cos_r, -sin_r),
            2 => (-sin_r, -cos_r),
            _ => (-cos_r, sin_r),
        }
    }

    /// `10^(db/10)`.
    pub fn from_db(db: f64) -> f64 {
        exp(db * LN10 / 10.0)
    }

    /// `|z|` by a square root of the sum of squares (no library `hypot`).
    pub fn abs(re: f64, im: f64) -> f64 {
        (re * re + im * im).sqrt()
    }
}

use detmath::{exp, ln};

/// Decibels of a power ratio per natural log unit: `10 / ln 10`.
const DB_PER_NEPER_POWER: f64 = 10.0 / detmath::LN10;

// ---------------------------------------------------------------------------
// One line's arithmetic
// ---------------------------------------------------------------------------

/// The settings one line's step reads.
#[derive(Clone, Debug)]
struct Knobs {
    phase: bool,
    gain: f64,
    anchor: f64,
    min_du_db: f64,
    coherence_power: f64,
    split_level: bool,
    ridge: f64,
    loop_gain: f64,
    max_step: f64,
    max_phase: f64,
    ceiling: f64,
    max_coherence: f64,
}

/// What a line carries from one loop to the next (note 4.13).
#[derive(Clone, Debug)]
struct LineMemory {
    /// The learned sensitivity in use last loop, controls by unknowns.
    s_hat: Vec<f64>,
    /// The measured autospectra last loop.
    y: Vec<f64>,
    /// Which controls were left in last loop.
    usable: Vec<bool>,
    /// The phases applied last loop (zero without phase steering).
    theta: Vec<f64>,
}

/// Every intermediate value of one line's last call, for the tests and for a
/// reader checking the law against the note by hand. Recorded only for the
/// line named by [`BroydenCongruenceLaw::record_line`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineRecord {
    /// The grid line.
    pub line: usize,
    /// 0 for the first drive, then the feedback cycle.
    pub cycle: u64,
    // The first drive.
    /// The inverse of the reference, drives by drives, before any adjustment.
    pub raw: Vec<C64>,
    /// The model response of the first drive before its level cap.
    pub predicted: Vec<f64>,
    /// The first drive's level cap factor `s` (1 with the cap off).
    pub start_scale: f64,
    /// The floor of the line's drive trace, set at the first drive (Choice
    /// 14); 0 for none.
    pub floor: f64,
    // The feedback cycle.
    /// Whether the base drive was the drive as played.
    pub base_played: bool,
    /// The base drive, drives by drives.
    pub base: Vec<C64>,
    /// The measured autospectra.
    pub measured: Vec<f64>,
    /// Which controls are left in.
    pub usable: Vec<bool>,
    /// The shares `a_mi`, controls by drives.
    pub shares: Vec<C64>,
    /// The model response of the base drive.
    pub model_response: Vec<f64>,
    /// The model sensitivity, controls by unknowns.
    pub s_model: Vec<f64>,
    /// The change of the log gains (and phases) the update learned from.
    pub du: Vec<f64>,
    /// The change of the log responses.
    pub dy: Vec<f64>,
    /// The prediction `S du`.
    pub prediction: Vec<f64>,
    /// The trust weights.
    pub trust: Vec<f64>,
    /// Whether the update ran.
    pub updated: bool,
    /// The sensitivity after the update, before its corrections.
    pub s_updated: Vec<f64>,
    /// After the corrections, before the anchor.
    pub s_corrected: Vec<f64>,
    /// The sensitivity in use (after the anchor).
    pub s_hat: Vec<f64>,
    /// The coherence weights.
    pub weights: Vec<f64>,
    /// The log errors.
    pub eps: Vec<f64>,
    /// The common level `c`.
    pub level: f64,
    /// The log errors with the level split out.
    pub eps_split: Vec<f64>,
    /// `S^T W S`, unknowns by unknowns.
    pub normal: Vec<f64>,
    /// `S^T W eps'`.
    pub rhs: Vec<f64>,
    /// The ridge constant.
    pub lambda: f64,
    /// The ridge solution `z`.
    pub z: Vec<f64>,
    /// The step before the loop gain: `z` with `c` added to the gains.
    pub u_solved: Vec<f64>,
    /// The resonance gain.
    pub resonance_gain: f64,
    /// The three predictions of the step limit for the first step tried:
    /// learned, model exact, drive.
    pub predictions: [f64; 3],
    /// The first ratio of the step limit.
    pub ratio: f64,
    /// The step applied.
    pub u: Vec<f64>,
    /// `D X D^H`, before the ceiling.
    pub applied: Vec<C64>,
    /// The expected responses of the ceiling.
    pub expected: Vec<f64>,
    /// The ceiling's `o`.
    pub overshoot: f64,
    /// The drive returned.
    pub drive: Vec<C64>,
}

/// What one line's step produced.
struct LineOutcome {
    drive: Vec<C64>,
    /// The drive kept as the law's command: the one returned, after the
    /// ceiling, the floor and the cap (note 4.13).
    kept: Vec<C64>,
    memory: LineMemory,
    updated: bool,
    step_scale: f64,
    ceiling_factor: f64,
    /// The floor lifted the line.
    floor_lifted: bool,
}

/// The full `n` by `n` matrix of one line.
fn full_line(h: &HermitianStack, line: usize) -> Vec<C64> {
    let n = h.n();
    let mut x = vec![ZERO; n * n];
    for r in 0..n {
        for c in 0..n {
            x[r * n + c] = h.get(line, r, c);
        }
    }
    x
}

/// Write the upper triangle of `x` into `out` at `line`.
fn put_line(out: &mut HermitianStack, line: usize, x: &[C64]) {
    let n = out.n();
    for r in 0..n {
        for c in r..n {
            out.set(line, r, c, x[r * n + c]);
        }
    }
}

/// The shares `a_mi = H_mi (X H^H)_im` (note 4.3), controls by drives, and
/// the model response `y_m = sum_i a_mi` (real).
fn shares(h: &[C64], x: &[C64], nc: usize, nd: usize) -> (Vec<C64>, Vec<f64>) {
    let mut a = vec![ZERO; nc * nd];
    let mut y = vec![0.0; nc];
    for m in 0..nc {
        let mut sum = 0.0;
        for i in 0..nd {
            let mut xh = ZERO;
            for j in 0..nd {
                xh += x[i * nd + j] * h[m * nd + j].conj();
            }
            let v = h[m * nd + i] * xh;
            a[m * nd + i] = v;
            sum += v.re;
        }
        y[m] = sum;
    }
    (a, y)
}

/// The model response `(H X H^H)_mm` of every control.
fn model_response(h: &[C64], x: &[C64], nc: usize, nd: usize) -> Vec<f64> {
    shares(h, x, nc, nd).1
}

/// The model sensitivity `d ln y_m / d u_i = 2 Re(a_mi) / y_m` and, with
/// phase steering, `d ln y_m / d theta_i = -2 Im(a_mi) / y_m` (note 4.3, 5.3),
/// controls by `p` unknowns. A control whose model response is not positive
/// has a zero row.
fn sensitivity(a: &[C64], y: &[f64], nc: usize, nd: usize, p: usize) -> Vec<f64> {
    let mut s = vec![0.0; nc * p];
    for m in 0..nc {
        if !(y[m] > 0.0 && y[m].is_finite()) {
            continue;
        }
        for i in 0..nd {
            let v = a[m * nd + i];
            s[m * p + i] = 2.0 * v.re / y[m];
            if p > nd {
                s[m * p + nd + i] = -2.0 * v.im / y[m];
            }
        }
    }
    s
}

/// `D X D^H` with `d_i = e^(u_i) (cos theta_i + j sin theta_i)` (note 4.11,
/// 5.2).
fn congruence(x: &[C64], u: &[f64], nd: usize, phase: bool) -> Vec<C64> {
    let d: Vec<C64> = (0..nd)
        .map(|i| {
            let g = exp(u[i]);
            if phase {
                let (s, c) = detmath::sin_cos(u[nd + i]);
                C64::new(g * c, g * s)
            } else {
                C64::new(g, 0.0)
            }
        })
        .collect();
    let mut out = vec![ZERO; nd * nd];
    for i in 0..nd {
        for j in 0..nd {
            out[i * nd + j] = d[i] * x[i * nd + j] * d[j].conj();
        }
    }
    out
}

fn trace(x: &[C64], n: usize) -> f64 {
    (0..n).map(|i| x[i * n + i].re).sum()
}

/// The floor for a line whose model drive has trace `model_trace` (Choice
/// 14): 0, no floor, where that trace is zero or not finite.
fn floor_of(model_trace: f64) -> f64 {
    if model_trace > 0.0 && model_trace.is_finite() {
        FLOOR_REL * model_trace
    } else {
        0.0
    }
}

/// The floor (note 4.12): a specified line's drive trace is lifted to
/// `floor`; an all-zero drive becomes uncorrelated drives at that trace.
fn apply_floor(x: &mut [C64], n: usize, floor: f64) {
    let tr = trace(x, n);
    if tr >= floor || floor <= 0.0 {
        return;
    }
    if tr > 0.0 && tr.is_finite() {
        let k = floor / tr;
        for v in x.iter_mut() {
            *v *= k;
        }
    } else {
        for v in x.iter_mut() {
            *v = ZERO;
        }
        for i in 0..n {
            x[i * n + i] = C64::new(floor / n as f64, 0.0);
        }
    }
}

/// The drive-coherence cap (note 4.12, Choice 15).
fn cap_coherence(x: &mut [C64], n: usize, cmax: f64) {
    if cmax >= 1.0 {
        return;
    }
    let mut capped = false;
    for i in 0..n {
        for j in (i + 1)..n {
            let (xi, xj) = (x[i * n + i].re.max(0.0), x[j * n + j].re.max(0.0));
            let v = x[i * n + j];
            let mag = detmath::abs(v.re, v.im);
            if mag == 0.0 {
                continue;
            }
            let lim = cmax * (xi * xj).sqrt();
            if mag > lim {
                let k = lim / mag;
                x[i * n + j] = v * k;
                x[j * n + i] = (v * k).conj();
                capped = true;
            }
        }
    }
    if capped && n >= 3 {
        let (values, vectors) = pareto::eigh_jacobi(x, n);
        if values.iter().any(|&l| l < 0.0) {
            for r in 0..n {
                for c in 0..n {
                    let mut s = ZERO;
                    for k in 0..n {
                        if values[k] > 0.0 {
                            s += vectors[r * n + k] * values[k] * vectors[c * n + k].conj();
                        }
                    }
                    x[r * n + c] = s;
                }
            }
            for r in 0..n {
                x[r * n + r] = C64::new(x[r * n + r].re, 0.0);
                for c in (r + 1)..n {
                    // Hermitian to the bit.
                    let v = (x[r * n + c] + x[c * n + r].conj()) * 0.5;
                    x[r * n + c] = v;
                    x[c * n + r] = v.conj();
                }
            }
        }
    }
}

/// Solve `(A + lambda I) z = b`, `A` symmetric positive semi-definite, by
/// Cholesky. `None` when the matrix is not positive definite.
fn ridge_solve(a: &[f64], lambda: f64, b: &[f64], p: usize) -> Option<Vec<f64>> {
    let mut l = vec![0.0; p * p];
    for i in 0..p {
        for j in 0..=i {
            let mut s = a[i * p + j] + if i == j { lambda } else { 0.0 };
            for k in 0..j {
                s -= l[i * p + k] * l[j * p + k];
            }
            if i == j {
                if !(s > 0.0 && s.is_finite()) {
                    return None;
                }
                l[i * p + i] = s.sqrt();
            } else {
                l[i * p + j] = s / l[j * p + j];
            }
        }
    }
    let mut z = b.to_vec();
    for i in 0..p {
        let mut s = z[i];
        for k in 0..i {
            s -= l[i * p + k] * z[k];
        }
        z[i] = s / l[i * p + i];
    }
    for i in (0..p).rev() {
        let mut s = z[i];
        for k in (i + 1)..p {
            s -= l[k * p + i] * z[k];
        }
        z[i] = s / l[i * p + i];
    }
    Some(z)
}

/// The specification of one line: targets, which controls have one, and the
/// reference trace.
struct LineSpec {
    t: Vec<f64>,
    specified: Vec<bool>,
    trace: f64,
}

fn line_spec(reference: &HermitianStack, line: usize) -> LineSpec {
    let t = reference.diagonal(line);
    let specified: Vec<bool> = t.iter().map(|&v| v > 0.0 && v.is_finite()).collect();
    let trace = t
        .iter()
        .zip(&specified)
        .filter(|(_, &s)| s)
        .map(|(v, _)| *v)
        .sum();
    LineSpec {
        t,
        specified,
        trace,
    }
}

/// The inputs of one line's feedback step.
struct LineInputs<'a> {
    h: &'a [C64],
    spec: &'a LineSpec,
    measured: &'a [f64],
    coherence: &'a [f64],
    base: Vec<C64>,
    /// The diagonals of the last command and the one before it.
    command: Vec<f64>,
    previous: Option<Vec<f64>>,
    memory: Option<&'a LineMemory>,
    averages: f64,
    resonance_gain: f64,
    /// The floor of the drive trace on this line (Choice 14); 0 for none.
    floor: f64,
}

/// One feedback cycle on one line (note 4, steps 2 to 12).
fn step_line(
    k: &Knobs,
    nc: usize,
    nd: usize,
    li: &LineInputs<'_>,
    rec: Option<&mut LineRecord>,
) -> LineOutcome {
    let p = if k.phase { 2 * nd } else { nd };
    let x = &li.base;
    let t = &li.spec.t;

    // Step 3: the shares and the model sensitivity of the base drive (4.3).
    let (a, y_model) = shares(li.h, x, nc, nd);
    let s_model = sensitivity(&a, &y_model, nc, nd, p);

    // Step 2: the log error, and the controls left out (4.2; Choice 11).
    let usable: Vec<bool> = (0..nc)
        .map(|m| {
            li.spec.specified[m]
                && li.measured[m] > 0.0
                && li.measured[m].is_finite()
                && y_model[m] > 0.0
                && y_model[m].is_finite()
        })
        .collect();
    let eps: Vec<f64> = (0..nc)
        .map(|m| {
            if usable[m] {
                ln(t[m] / li.measured[m])
            } else {
                0.0
            }
        })
        .collect();

    // Step 4: learn (4.4).
    let mut s_hat;
    let mut updated = false;
    let mut du = vec![0.0; p];
    let mut dy = vec![0.0; nc];
    let mut pred = vec![0.0; nc];
    let mut trust = vec![0.0; nc];
    let mut s_updated = Vec::new();
    let mut s_corrected = Vec::new();
    match (li.memory, &li.previous) {
        (Some(mem), Some(prev)) => {
            s_hat = mem.s_hat.clone();
            // 4.4.2: du from the commanded levels; with phase steering the
            // phases applied last loop (5.4).
            for i in 0..nd {
                let (now, before) = (li.command[i], prev[i]);
                du[i] = if now > 0.0 && before > 0.0 {
                    0.5 * ln(now / before)
                } else {
                    0.0
                };
                if k.phase {
                    du[nd + i] = mem.theta[i];
                }
            }
            let gain_db =
                2.0 * DB_PER_NEPER_POWER * du[..nd].iter().fold(0.0f64, |m, v| m.max(v.abs()));
            let dudu: f64 = du.iter().map(|v| v * v).sum();
            // 4.4.6: skip a line whose gains barely moved (Choice 7).
            if gain_db >= k.min_du_db && k.gain > 0.0 && gain_db.is_finite() {
                let noise = if li.averages > 0.0 {
                    2.0 / li.averages
                } else {
                    f64::INFINITY
                };
                for m in 0..nc {
                    if !(usable[m] && mem.usable[m] && mem.y[m] > 0.0) {
                        continue;
                    }
                    // 4.4.3, 4.4.4: the observed change and its prediction.
                    dy[m] = ln(li.measured[m] / mem.y[m]);
                    let row = &s_hat[m * p..(m + 1) * p];
                    pred[m] = row.iter().zip(&du).map(|(s, d)| s * d).sum();
                    // 4.4.5: the trust weight.
                    let p2 = pred[m] * pred[m];
                    let den = p2 + noise;
                    trust[m] = if den > 0.0 && den.is_finite() {
                        p2 / den
                    } else {
                        0.0
                    };
                    // 4.4.6: the damped secant update of the row.
                    let mult = k.gain * trust[m] * (dy[m] - pred[m]) / (dudu + BROYDEN_DELTA);
                    for j in 0..p {
                        s_hat[m * p + j] += mult * du[j];
                    }
                    // Counted only where a row changed (a zero trust weight,
                    // or nothing to correct, learns nothing).
                    updated |= mult != 0.0;
                }
            }
            s_updated = s_hat.clone();
            // 4.4.7: the three corrections, in order (Choice 9).
            for m in 0..nc {
                for j in 0..p {
                    let bound = 2.0f64.max(2.0 * s_model[m * p + j].abs());
                    let v = &mut s_hat[m * p + j];
                    *v = v.clamp(-bound, bound);
                }
                let r: f64 = s_hat[m * p..m * p + nd].iter().sum();
                let shift = (r.clamp(0.0, 2.0) - r) / nd as f64;
                for j in 0..nd {
                    s_hat[m * p + j] += shift;
                }
                if k.phase {
                    let mean: f64 = s_hat[m * p + nd..(m + 1) * p].iter().sum::<f64>() / nd as f64;
                    for j in nd..p {
                        s_hat[m * p + j] -= mean;
                    }
                }
            }
            s_corrected = s_hat.clone();
            // Step 5: the model anchor (4.5).
            for j in 0..nc * p {
                s_hat[j] = (1.0 - k.anchor) * s_hat[j] + k.anchor * s_model[j];
            }
        }
        // The first feedback cycle: nothing to learn from (4.4).
        _ => s_hat = s_model.clone(),
    }

    // Step 6: coherence weights (4.6).
    let weights: Vec<f64> = (0..nc)
        .map(|m| {
            if !usable[m] {
                return 0.0;
            }
            let g = li.coherence[m];
            let g = if g.is_finite() {
                g.clamp(0.0, 1.0)
            } else {
                0.0
            };
            if k.coherence_power == 0.0 {
                1.0
            } else if k.coherence_power == 1.0 {
                g
            } else if g == 0.0 {
                0.0
            } else {
                exp(k.coherence_power * ln(g))
            }
        })
        .collect();

    // Step 7: the common level split out first (4.7; magnitude columns).
    let r: Vec<f64> = (0..nc)
        .map(|m| s_hat[m * p..m * p + nd].iter().sum())
        .collect();
    let mut level = 0.0;
    if k.split_level {
        let (mut num, mut den) = (0.0, 0.0);
        for m in 0..nc {
            num += weights[m] * r[m] * eps[m];
            den += weights[m] * r[m] * r[m];
        }
        if den > 0.0 && den.is_finite() {
            level = num / den;
        }
    }
    let eps_split: Vec<f64> = (0..nc).map(|m| eps[m] - r[m] * level).collect();

    // Step 8: the ridge solve (4.8).
    let mut normal = vec![0.0; p * p];
    let mut rhs = vec![0.0; p];
    let mut model_trace = 0.0;
    for m in 0..nc {
        let w = weights[m];
        if w == 0.0 {
            continue;
        }
        let row = &s_hat[m * p..(m + 1) * p];
        for i in 0..p {
            rhs[i] += row[i] * w * eps_split[m];
            for j in 0..p {
                normal[i * p + j] += row[i] * w * row[j];
            }
            model_trace += w * s_model[m * p + i] * s_model[m * p + i];
        }
    }
    let hat_trace: f64 = (0..p).map(|i| normal[i * p + i]).sum();
    let lambda = k.ridge * hat_trace.max(model_trace) / p as f64;
    let z = if hat_trace > 0.0 || model_trace > 0.0 {
        ridge_solve(&normal, lambda, &rhs, p).unwrap_or_else(|| vec![0.0; p])
    } else {
        vec![0.0; p]
    };
    let mut u = z.clone();
    let any_usable = usable.iter().any(|&b| b);
    for i in 0..nd {
        u[i] += level;
    }
    if !any_usable {
        u.iter_mut().for_each(|v| *v = 0.0);
    }
    // Choice 12: a drive that reaches no control left in is not stepped.
    let live: Vec<bool> = (0..nd)
        .map(|i| {
            (0..nc).any(|m| {
                usable[m]
                    && (s_model[m * p + i] != 0.0 || (k.phase && s_model[m * p + nd + i] != 0.0))
            })
        })
        .collect();
    for i in 0..nd {
        if !live[i] {
            u[i] = 0.0;
            if k.phase {
                u[nd + i] = 0.0;
            }
        }
    }
    // 5.3: the solved phases shifted to zero mean (Choice 5). Redundant in
    // exact arithmetic (every phase row of the sensitivity sums to zero, so
    // the solved phases already do); kept because the note asks for it.
    if k.phase {
        let count = live.iter().filter(|&&b| b).count();
        if count > 0 {
            let mean: f64 =
                (0..nd).filter(|&i| live[i]).map(|i| u[nd + i]).sum::<f64>() / count as f64;
            for i in 0..nd {
                if live[i] {
                    u[nd + i] -= mean;
                }
            }
        }
    }
    let u_solved = u.clone();

    // Step 9: the loop gain and the resonance schedule, on gains and phases
    // (4.9; Choice 3).
    let gain = k.loop_gain * li.resonance_gain;
    for v in u.iter_mut() {
        *v *= gain;
    }

    // Step 10: the step limit (4.10, 5.4; Choices 4 and 21). After the sixth
    // scaling the step is not checked again, as the note has it: only the
    // exact prediction can then still be above the limit.
    let mut step_scale = 1.0;
    let mut predictions = [0.0; 3];
    let mut first_ratio = 0.0;
    for rep in 0..STEP_REPEATS {
        let applied = congruence(x, &u, nd, k.phase);
        let y_new = model_response(li.h, &applied, nc, nd);
        let mut learned: f64 = 0.0;
        let mut exact: f64 = 0.0;
        for m in 0..nc {
            if usable[m] {
                let su: f64 = s_hat[m * p..(m + 1) * p]
                    .iter()
                    .zip(&u)
                    .map(|(s, v)| s * v)
                    .sum();
                learned = learned.max(su.abs());
            }
            if li.spec.specified[m] && y_model[m] > 0.0 {
                let e = if y_new[m] > 0.0 {
                    ln(y_new[m] / y_model[m]).abs()
                } else {
                    f64::INFINITY
                };
                exact = exact.max(e);
            }
        }
        let drive = 2.0 * u[..nd].iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let mut ratio = learned.max(exact).max(drive) / k.max_step;
        if k.phase {
            let ph = u[nd..].iter().fold(0.0f64, |m, v| m.max(v.abs()));
            ratio = ratio.max(ph / k.max_phase);
        }
        if rep == 0 {
            predictions = [learned, exact, drive];
            first_ratio = ratio;
        }
        if !ratio.is_finite() {
            // A step the model cannot predict is not taken.
            u.iter_mut().for_each(|v| *v = 0.0);
            step_scale = 0.0;
            break;
        }
        if ratio <= 1.0 {
            break;
        }
        for v in u.iter_mut() {
            *v /= ratio;
        }
        step_scale /= ratio;
    }

    // Step 11: the congruence (4.11).
    let applied = congruence(x, &u, nd, k.phase);
    let mut drive = applied.clone();

    // Step 12: the ceiling (4.12; Choice 17).
    let y_after = model_response(li.h, &drive, nc, nd);
    let mut overshoot: f64 = 0.0;
    let mut expected = vec![0.0; nc];
    for m in 0..nc {
        if !li.spec.specified[m] {
            continue;
        }
        let limit = t[m] * k.ceiling;
        if usable[m] {
            let su: f64 = s_hat[m * p..(m + 1) * p]
                .iter()
                .zip(&u)
                .map(|(s, v)| s * v)
                .sum();
            expected[m] = li.measured[m] * exp(su);
            overshoot = overshoot.max(expected[m] / limit);
        }
        overshoot = overshoot.max(y_after[m] / (MODEL_CEILING_FACTOR * limit));
    }
    let mut ceiling_factor = 1.0;
    if overshoot > 1.0 && overshoot.is_finite() {
        ceiling_factor = 1.0 / overshoot;
        for v in drive.iter_mut() {
            *v *= ceiling_factor;
        }
    }
    // The floor (Choice 14), after the ceiling, which it overrules.
    let mut floor_lifted = false;
    let tr_now = trace(&drive, nd);
    if li.floor > 0.0 && li.floor.is_finite() && (tr_now < li.floor || tr_now.is_nan()) {
        apply_floor(&mut drive, nd, li.floor);
        floor_lifted = true;
    }
    // The drive-coherence cap.
    cap_coherence(&mut drive, nd, k.max_coherence);

    let theta = if k.phase {
        u[nd..].to_vec()
    } else {
        vec![0.0; nd]
    };
    if let Some(rec) = rec {
        rec.base = x.clone();
        rec.floor = li.floor;
        rec.measured = li.measured.to_vec();
        rec.usable = usable.clone();
        rec.shares = a;
        rec.model_response = y_model;
        rec.s_model = s_model;
        rec.du = du;
        rec.dy = dy;
        rec.prediction = pred;
        rec.trust = trust;
        rec.updated = updated;
        rec.s_updated = s_updated;
        rec.s_corrected = s_corrected;
        rec.s_hat = s_hat.clone();
        rec.weights = weights;
        rec.eps = eps;
        rec.level = level;
        rec.eps_split = eps_split;
        rec.normal = normal;
        rec.rhs = rhs;
        rec.lambda = lambda;
        rec.z = z;
        rec.u_solved = u_solved;
        rec.resonance_gain = li.resonance_gain;
        rec.predictions = predictions;
        rec.ratio = first_ratio;
        rec.u = u.clone();
        rec.applied = applied;
        rec.expected = expected;
        rec.overshoot = overshoot;
        rec.drive = drive.clone();
    }
    // Step 13: what is kept is stored after the ceiling, floor and cap.
    let kept = drive.clone();
    LineOutcome {
        drive,
        kept,
        memory: LineMemory {
            s_hat,
            y: li.measured.to_vec(),
            usable,
            theta,
        },
        updated,
        step_scale,
        ceiling_factor,
        floor_lifted,
    }
}

// ---------------------------------------------------------------------------
// The resonance schedule
// ---------------------------------------------------------------------------

/// The resonance gain `g` of every in-band line (Choice 18).
///
/// `h` is the in-band frequency response (`nb` lines), `first` the grid
/// index of its first line, `specified` which controls have a specification
/// on each in-band line (`nb * nc`).
pub fn resonance_gains(
    h: &MatrixStack,
    first: usize,
    specified: &[bool],
    halfwidth_oct: f64,
    sensitivity: f64,
    min_gain: f64,
) -> Vec<f64> {
    let (nb, nc, nd) = h.dims();
    if nb == 0 {
        return Vec::new();
    }
    // Each control's row size on each line.
    let mut size = vec![0.0; nb * nc];
    for i in 0..nb {
        let line = h.line(i);
        for c in 0..nc {
            let s: f64 = line[c * nd..(c + 1) * nd]
                .iter()
                .map(|v| v.re * v.re + v.im * v.im)
                .sum();
            size[i * nc + c] = s.sqrt();
        }
    }
    // The symmetric half-window in lines of each in-band line: the lines
    // within the half-width in log frequency, the smaller count of the two
    // sides, so a smooth trend has its own value as the window's median.
    let factor = exp(halfwidth_oct * std::f64::consts::LN_2);
    let half: Vec<usize> = (0..nb)
        .map(|i| {
            let f = (first + i) as f64;
            if f <= 0.0 {
                return 0;
            }
            let lo = f / factor;
            let hi = f * factor;
            let below = (0..i)
                .rev()
                .take_while(|&j| (first + j) as f64 >= lo)
                .count();
            let above = ((i + 1)..nb)
                .take_while(|&j| (first + j) as f64 <= hi)
                .count();
            below.min(above)
        })
        .collect();
    let mut ratio = vec![1.0f64; nb];
    let mut window: Vec<f64> = Vec::new();
    for c in 0..nc {
        for i in 0..nb {
            let k = half[i];
            if 2 * k + 1 < MIN_WINDOW_LINES || !specified[i * nc + c] {
                continue;
            }
            window.clear();
            window.extend((i - k..=i + k).map(|j| size[j * nc + c]));
            window.sort_by(|a, b| a.total_cmp(b));
            let base = window[k];
            if base > 0.0 && base.is_finite() {
                let rt = size[i * nc + c] / base;
                if rt.is_finite() {
                    ratio[i] = ratio[i].max(rt);
                }
            }
        }
    }
    ratio
        .iter()
        .map(|&p| {
            let g = 1.0 / (1.0 + sensitivity * (p - 1.0).max(0.0));
            g.max(min_gain)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// The settings of [`BroydenCongruenceLaw`]. Defaults are the note's, except
/// the first drive's level cap (off: the controller owns the level).
#[derive(Clone, Debug, PartialEq)]
pub struct BroydenConfig {
    /// The first drive's inverse: `truncated`, `tikhonov` or `none`.
    pub regularization: String,
    /// Relative to the largest singular value: the note's `rcond`.
    pub regularization_amount: f64,
    /// The drive-coherence cap.
    pub max_drive_coherence: f64,
    /// Whether the first drive is capped in level as the note does.
    pub start_cap: bool,
    /// The level of the first drive's cap, dB.
    pub start_cap_db: f64,
    /// The loop gain `K_i`.
    pub convergence: f64,
    /// The most a response or a drive moves per loop, dB.
    pub max_step_db: f64,
    /// The ridge weight `rho`.
    pub ridge: f64,
    /// The resonance sensitivity `s`.
    pub resonance_sensitivity: f64,
    /// The resonance window's half-width, octaves.
    pub resonance_halfwidth_oct: f64,
    /// The smallest resonance gain.
    pub resonance_min_gain: f64,
    /// The overshoot ceiling, dB.
    pub ceiling_db: f64,
    /// The learning rate `g`.
    pub broyden_gain: f64,
    /// The power of the coherence weights.
    pub coherence_power: f64,
    /// The pull toward the model each loop.
    pub model_anchor: f64,
    /// The smallest gain change learned from, dB.
    pub min_du_db: f64,
    /// Take the common level out first.
    pub split_level: bool,
    /// Build on the command when it was played as commanded.
    pub base_on_command: bool,
    /// Above this the base is the drive as played, dB.
    pub realize_tol_db: f64,
    /// Steer the drives' phases too.
    pub phase_steering: bool,
    /// The most a phase moves per loop, degrees.
    pub max_phase_step_deg: f64,
    /// A cap on the number of averages; 0, no cap.
    pub effective_averages: f64,
}

impl Default for BroydenConfig {
    fn default() -> Self {
        BroydenConfig {
            regularization: "truncated".into(),
            regularization_amount: 1e-3,
            max_drive_coherence: 0.95,
            start_cap: false,
            start_cap_db: -9.0,
            convergence: 0.5,
            max_step_db: 3.0,
            ridge: 0.1,
            resonance_sensitivity: 0.5,
            resonance_halfwidth_oct: 1.0 / 3.0,
            resonance_min_gain: 0.0,
            ceiling_db: 12.0,
            broyden_gain: 0.6,
            coherence_power: 1.0,
            model_anchor: 0.05,
            min_du_db: 0.05,
            split_level: true,
            base_on_command: true,
            realize_tol_db: 3.0,
            phase_steering: true,
            max_phase_step_deg: 15.0,
            effective_averages: 0.0,
        }
    }
}

impl BroydenConfig {
    /// Set the parameter `key`.
    pub fn set(&mut self, key: &str, v: &ParamVal) -> Result<(), LawError> {
        match key {
            "regularization" => self.regularization = v.choice(key)?.to_string(),
            "regularization_amount" => self.regularization_amount = v.number(key)?,
            "max_drive_coherence" => self.max_drive_coherence = v.number(key)?,
            "start_cap" => self.start_cap = v.toggle(key)?,
            "start_cap_db" => self.start_cap_db = v.number(key)?,
            "convergence" => self.convergence = v.number(key)?,
            "max_step_db" => self.max_step_db = v.number(key)?,
            "ridge" => self.ridge = v.number(key)?,
            "resonance_sensitivity" => self.resonance_sensitivity = v.number(key)?,
            "resonance_halfwidth_oct" => self.resonance_halfwidth_oct = v.number(key)?,
            "resonance_min_gain" => self.resonance_min_gain = v.number(key)?,
            "ceiling_db" => self.ceiling_db = v.number(key)?,
            "broyden_gain" => self.broyden_gain = v.number(key)?,
            "coherence_power" => self.coherence_power = v.number(key)?,
            "model_anchor" => self.model_anchor = v.number(key)?,
            "min_du_db" => self.min_du_db = v.number(key)?,
            "split_level" => self.split_level = v.toggle(key)?,
            "base_on_command" => self.base_on_command = v.toggle(key)?,
            "realize_tol_db" => self.realize_tol_db = v.number(key)?,
            "phase_steering" => self.phase_steering = v.toggle(key)?,
            "max_phase_step_deg" => self.max_phase_step_deg = v.number(key)?,
            "effective_averages" => self.effective_averages = v.number(key)?,
            other => return Err(LawError(format!("parameter `{other}` is not declared"))),
        }
        Ok(())
    }

    /// Every value checked against its declaration.
    fn check(&self) -> Result<(), LawError> {
        let spec = BroydenCongruenceLaw::spec();
        for d in &spec.params {
            let v = match d.key {
                "regularization" => ParamVal::Choice(self.regularization.clone()),
                "start_cap" => ParamVal::Toggle(self.start_cap),
                "split_level" => ParamVal::Toggle(self.split_level),
                "base_on_command" => ParamVal::Toggle(self.base_on_command),
                "phase_steering" => ParamVal::Toggle(self.phase_steering),
                key => ParamVal::Number(self.number(key)),
            };
            d.check(&v)?;
        }
        Ok(())
    }

    fn number(&self, key: &str) -> f64 {
        match key {
            "regularization_amount" => self.regularization_amount,
            "max_drive_coherence" => self.max_drive_coherence,
            "start_cap_db" => self.start_cap_db,
            "convergence" => self.convergence,
            "max_step_db" => self.max_step_db,
            "ridge" => self.ridge,
            "resonance_sensitivity" => self.resonance_sensitivity,
            "resonance_halfwidth_oct" => self.resonance_halfwidth_oct,
            "resonance_min_gain" => self.resonance_min_gain,
            "ceiling_db" => self.ceiling_db,
            "broyden_gain" => self.broyden_gain,
            "coherence_power" => self.coherence_power,
            "model_anchor" => self.model_anchor,
            "min_du_db" => self.min_du_db,
            "realize_tol_db" => self.realize_tol_db,
            "max_phase_step_deg" => self.max_phase_step_deg,
            "effective_averages" => self.effective_averages,
            _ => f64::NAN,
        }
    }

    fn knobs(&self) -> Knobs {
        Knobs {
            phase: self.phase_steering,
            gain: self.broyden_gain,
            anchor: self.model_anchor,
            min_du_db: self.min_du_db,
            coherence_power: self.coherence_power,
            split_level: self.split_level,
            ridge: self.ridge,
            loop_gain: self.convergence,
            max_step: self.max_step_db / DB_PER_NEPER_POWER,
            max_phase: self.max_phase_step_deg * std::f64::consts::PI / 180.0,
            ceiling: detmath::from_db(self.ceiling_db),
            max_coherence: self.max_drive_coherence,
        }
    }
}

fn number(
    key: &'static str,
    label: &'static str,
    help: &'static str,
    unit: &'static str,
    (min, max, step, default): (f64, f64, f64, f64),
) -> ParamDecl {
    ParamDecl {
        key,
        label,
        help,
        kind: ParamKindDecl::Number {
            unit,
            min,
            max,
            step,
            default,
        },
    }
}

fn toggle(key: &'static str, label: &'static str, help: &'static str, default: bool) -> ParamDecl {
    ParamDecl {
        key,
        label,
        help,
        kind: ParamKindDecl::Toggle(default),
    }
}

fn per_line(key: &'static str, label: &'static str, help: &'static str) -> DiagDecl {
    DiagDecl {
        key,
        label,
        unit: "",
        help,
        kind: DiagKind::PerLine,
    }
}

// ---------------------------------------------------------------------------
// The law
// ---------------------------------------------------------------------------

/// What the law keeps between loops.
struct State {
    /// The last drive returned.
    command: HermitianStack,
    /// The one before it.
    previous: Option<HermitianStack>,
    /// Per in-band line.
    memory: Vec<Option<LineMemory>>,
    /// Feedback cycles done.
    cycle: u64,
    /// The floor of each in-band line's drive trace (Choice 14), set by the
    /// first drive and kept until the next.
    floors: Vec<f64>,
}

/// The Broyden congruence law for one run.
pub struct BroydenCongruenceLaw {
    setup: MimoSetup,
    cfg: BroydenConfig,
    state: Option<State>,
    /// The in-band frequency response the resonance gains were formed from.
    key: Vec<C64>,
    resonance: Vec<f64>,
    updates: u64,
    record_line: Option<usize>,
    record: Option<LineRecord>,
}

impl BroydenCongruenceLaw {
    /// The law for `setup` with `cfg`.
    pub fn new(setup: &MimoSetup, cfg: &BroydenConfig) -> Result<Self, LawError> {
        setup.check()?;
        cfg.check()?;
        Ok(BroydenCongruenceLaw {
            setup: setup.clone(),
            cfg: cfg.clone(),
            state: None,
            key: Vec::new(),
            resonance: Vec::new(),
            updates: 0,
            record_line: None,
            record: None,
        })
    }

    /// Record every intermediate value of `line` on each call from now on
    /// (`None` stops recording).
    pub fn record_line(&mut self, line: Option<usize>) {
        self.record_line = line;
        self.record = None;
    }

    /// The intermediate values of the recorded line on the last call.
    pub fn line_record(&self) -> Option<&LineRecord> {
        self.record.as_ref()
    }

    /// The learned sensitivity in use on `line` after the last call,
    /// controls by unknowns, row-major.
    pub fn sensitivity_in_use(&self, line: usize) -> Option<&[f64]> {
        let (lo, hi) = self.setup.band;
        if line < lo || line >= hi {
            return None;
        }
        self.state
            .as_ref()
            .and_then(|s| s.memory[line - lo].as_ref())
            .map(|m| m.s_hat.as_slice())
    }

    fn check_frf(&self, frf: &MatrixStack) -> Result<(), LawError> {
        let (lines, nc, nd) = (self.setup.lines, self.setup.controls, self.setup.drives);
        if frf.dims() != (lines, nc, nd) {
            return Err(LawError(format!(
                "the frequency response is {:?}, expected ({lines}, {nc}, {nd})",
                frf.dims()
            )));
        }
        if !frf.data()[self.setup.band.0 * nc * nd..self.setup.band.1 * nc * nd]
            .iter()
            .all(|v| v.re.is_finite() && v.im.is_finite())
        {
            return Err(LawError(
                "the frequency response is not finite in the band".into(),
            ));
        }
        Ok(())
    }

    /// The in-band lines of `frf`.
    fn inband<'a>(&self, frf: &'a MatrixStack) -> &'a [C64] {
        let (nc, nd) = (self.setup.controls, self.setup.drives);
        let band = self.setup.band;
        &frf.data()[band.0 * nc * nd..band.1 * nc * nd]
    }

    /// The resonance gains for `frf`: those kept when its in-band lines are
    /// the ones they were formed from, otherwise formed again.
    fn resonance_for(&self, frf: &MatrixStack) -> Vec<f64> {
        if !self.resonance.is_empty() && self.key.as_slice() == self.inband(frf) {
            return self.resonance.clone();
        }
        let band = self.setup.band;
        let specified: Vec<bool> = (band.0..band.1)
            .flat_map(|l| line_spec(&self.setup.reference, l).specified)
            .collect();
        resonance_gains(
            &band_matrix(frf, band),
            band.0,
            &specified,
            self.cfg.resonance_halfwidth_oct,
            self.cfg.resonance_sensitivity,
            self.cfg.resonance_min_gain,
        )
    }

    /// Keep the resonance gains for `frf`, formed again only when its in-band
    /// lines changed.
    fn refresh(&mut self, frf: &MatrixStack) {
        if self.resonance.is_empty() || self.key.as_slice() != self.inband(frf) {
            self.resonance = self.resonance_for(frf);
            self.key = self.inband(frf).to_vec();
        }
    }

    fn outputs(
        &mut self,
        drive: HermitianStack,
        diags: PerLineDiags,
        previous: Option<&HermitianStack>,
    ) -> MimoLoopOutputs {
        let correction_db = previous
            .map(|last| largest_change_db(last, &drive, self.setup.band))
            .unwrap_or(0.0);
        let value = |key: &str, data| DiagnosticValue {
            key: key.into(),
            data,
        };
        let mut resonance = vec![1.0; self.setup.lines];
        let (lo, hi) = self.setup.band;
        if self.resonance.len() == hi - lo {
            resonance[lo..hi].copy_from_slice(&self.resonance);
        }
        MimoLoopOutputs {
            drive,
            diagnostics: vec![
                value("correction_db", DiagnosticData::Scalar(correction_db)),
                value("resonance_gain", DiagnosticData::PerLine(resonance)),
                value("step_scale", DiagnosticData::PerLine(diags.step_scale)),
                value(
                    "ceiling_factor",
                    DiagnosticData::PerLine(diags.ceiling_factor),
                ),
                value("broyden_updated", DiagnosticData::PerLine(diags.updated)),
                value("base_played", DiagnosticData::PerLine(diags.base_played)),
                value("floor_lifted", DiagnosticData::PerLine(diags.floor_lifted)),
                value(
                    "base_played_lines",
                    DiagnosticData::Scalar(diags.base_played_lines as f64),
                ),
                value(
                    "broyden_updates",
                    DiagnosticData::Scalar(self.updates as f64),
                ),
            ],
        }
    }
}

/// The per-line diagnostics of one call.
struct PerLineDiags {
    step_scale: Vec<f64>,
    ceiling_factor: Vec<f64>,
    updated: Vec<f64>,
    base_played: Vec<f64>,
    floor_lifted: Vec<f64>,
    /// Lines on the played drive by the line-by-line test (Choice 19).
    base_played_lines: usize,
}

impl PerLineDiags {
    fn new(lines: usize) -> Self {
        PerLineDiags {
            step_scale: vec![1.0; lines],
            ceiling_factor: vec![1.0; lines],
            updated: vec![0.0; lines],
            base_played: vec![0.0; lines],
            floor_lifted: vec![0.0; lines],
            base_played_lines: 0,
        }
    }
}

/// The median of `|10 log10(tr played / tr command)|` over the specified
/// in-band lines where both are positive (note 4.1); `None` with no such line.
fn realize_db(
    played: &HermitianStack,
    command: &HermitianStack,
    reference: &HermitianStack,
    band: (usize, usize),
) -> Option<f64> {
    let mut v: Vec<f64> = (band.0..band.1)
        .filter(|&l| line_spec(reference, l).trace > 0.0)
        .filter_map(|l| {
            let (a, b) = (played.trace(l), command.trace(l));
            (a > 0.0 && b > 0.0 && a.is_finite() && b.is_finite())
                .then(|| (DB_PER_NEPER_POWER * ln(a / b)).abs())
        })
        .collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    })
}

impl MimoControlLaw for BroydenCongruenceLaw {
    fn needs(&self) -> &[MimoInput] {
        &NEEDS
    }

    fn shapes(&self) -> &[MimoShape] {
        &ALL_SHAPES
    }

    /// The first drive (note section 3).
    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        self.check_frf(inputs.frf)?;
        let (lines, nc, nd, band) = (
            self.setup.lines,
            self.setup.controls,
            self.setup.drives,
            self.setup.band,
        );
        let reg = regularization(&self.cfg.regularization, self.cfg.regularization_amount)?;
        let hb = band_matrix(inputs.frf, band);
        // 3.1: X_raw = H^+ S_spec H^+^H, the only place the cross terms of the
        // reference are read.
        let (plus, _) = linalg::pinv(&hb, reg)?;
        let reference = band_hermitian(&self.setup.reference, band);
        let raw = linalg::congruence(&plus, &reference)?;
        // The floor of each line (Choice 14): from the model drive of the
        // truncated inverse, whatever the regularization, set here once.
        let floor_reg = linalg::Regularization::Truncated { rcond: FLOOR_RCOND };
        let floor_model = if reg == floor_reg {
            raw.clone()
        } else {
            linalg::congruence(&linalg::pinv(&hb, floor_reg)?.0, &reference)?
        };
        let floors: Vec<f64> = (0..band.1 - band.0)
            .map(|i| floor_of(trace(&full_line(&floor_model, i), nd)))
            .collect();
        let mut drive = HermitianStack::zeros(lines, nd);
        let mut record = None;
        for (i, line) in (band.0..band.1).enumerate() {
            let spec = line_spec(&self.setup.reference, line);
            let mut rec = (self.record_line == Some(line)).then(|| LineRecord {
                line,
                ..LineRecord::default()
            });
            let mut x = full_line(&raw, i);
            if let Some(r) = rec.as_mut() {
                r.raw = x.clone();
                r.floor = floors[i];
            }
            // 3.2 (a): no specification, no drive; almost zero, the floor.
            if spec.trace <= 0.0 {
                if let Some(r) = rec {
                    record = Some(r);
                }
                continue;
            }
            if !x.iter().all(|v| v.re.is_finite() && v.im.is_finite()) {
                return Err(LawError(format!(
                    "the first drive is not finite on line {line}"
                )));
            }
            // The floor (Choice 14): with the default regularization it
            // lifts nothing here, being 80 dB below this very drive.
            apply_floor(&mut x, nd, floors[i]);
            // (b) the drive-coherence cap.
            cap_coherence(&mut x, nd, self.cfg.max_drive_coherence);
            // (c) the level cap, last because (a) and (b) can raise the level.
            let h = inputs.frf.line(line);
            let y = model_response(h, &x, nc, nd);
            let mut s = 1.0;
            if self.cfg.start_cap {
                let sum_y: f64 = y.iter().sum();
                if sum_y > 0.0 && sum_y.is_finite() {
                    s = (detmath::from_db(self.cfg.start_cap_db) * spec.trace / sum_y).min(1.0);
                }
                for v in x.iter_mut() {
                    *v *= s;
                }
            }
            if let Some(mut r) = rec {
                r.predicted = y;
                r.start_scale = s;
                r.drive = x.clone();
                record = Some(r);
            }
            put_line(&mut drive, line, &x);
        }
        if !drive.is_finite() {
            return Err(LawError("the first drive is not finite".into()));
        }
        // Kept only now, so a seed that fails leaves the law as it was.
        self.refresh(inputs.frf);
        self.record = record;
        self.updates = 0;
        self.state = Some(State {
            command: drive.clone(),
            previous: None,
            memory: vec![None; band.1 - band.0],
            cycle: 0,
            floors,
        });
        let diags = PerLineDiags::new(lines);
        Ok(self.outputs(drive, diags, None))
    }

    /// One feedback cycle (note section 4). A call that fails leaves the law
    /// as it was: the new state is built aside and kept only when every line
    /// succeeded.
    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        let state = self
            .state
            .as_ref()
            .ok_or_else(|| LawError("iterate was called before seed".into()))?;
        let next = self.cycle(inputs, state)?;
        if self.key.as_slice() != self.inband(inputs.frf) {
            self.key = self.inband(inputs.frf).to_vec();
        }
        self.resonance = next.resonance;
        self.updates += next.updates;
        self.record = next.record;
        let last = self.state.replace(next.state).expect("seeded").command;
        let outputs = self.outputs(next.drive, next.diags, Some(&last));
        if let Some(state) = self.state.as_mut() {
            state.previous = Some(last);
        }
        Ok(outputs)
    }
}

/// What one feedback cycle produced, kept only if it succeeded.
struct Cycle {
    drive: HermitianStack,
    diags: PerLineDiags,
    /// The new state, `previous` not yet set.
    state: State,
    updates: u64,
    record: Option<LineRecord>,
    /// The resonance gains of this call's frequency response.
    resonance: Vec<f64>,
}

impl BroydenCongruenceLaw {
    fn cycle(&self, inputs: &MimoLoopInputs<'_>, state: &State) -> Result<Cycle, LawError> {
        let (lines, nc, nd, band) = (
            self.setup.lines,
            self.setup.controls,
            self.setup.drives,
            self.setup.band,
        );
        let played = inputs
            .last_drive
            .ok_or_else(|| LawError("the core did not pass the last drive".into()))?;
        if played.lines() != lines || played.n() != nd {
            return Err(LawError(format!(
                "the last drive is {} lines by {}, expected {lines} by {nd}",
                played.lines(),
                played.n()
            )));
        }
        if inputs.response.lines() != lines || inputs.response.n() != nc {
            return Err(LawError(format!(
                "the response is {} lines by {}, expected {lines} by {nc}",
                inputs.response.lines(),
                inputs.response.n()
            )));
        }
        if inputs.multiple_coherence.len() != lines * nc {
            return Err(LawError(format!(
                "the multiple coherence has {} values, expected {}",
                inputs.multiple_coherence.len(),
                lines * nc
            )));
        }
        self.check_frf(inputs.frf)?;
        // The resonance gains for this call's frequency response, formed
        // aside (the law keeps them only if the call succeeds).
        let resonance = self.resonance_for(inputs.frf);
        let cycle = state.cycle + 1;

        // Step 1: the base drive (4.1), for the band by its median, and line
        // by line (Choice 19).
        let on_command = self.cfg.base_on_command
            && match realize_db(played, &state.command, &self.setup.reference, band) {
                Some(db) => db <= self.cfg.realize_tol_db,
                None => true,
            };

        // The number of averages (4.4.5; Choice 10).
        let mut averages = inputs.response_dof / 2.0;
        if !averages.is_finite() || averages < 0.0 {
            averages = 0.0;
        }
        if self.cfg.effective_averages > 0.0 {
            averages = averages.min(self.cfg.effective_averages);
        }
        let knobs = self.cfg.knobs();
        let mut drive = HermitianStack::zeros(lines, nd);
        let mut kept = HermitianStack::zeros(lines, nd);
        let mut memory: Vec<Option<LineMemory>> = vec![None; band.1 - band.0];
        let mut diags = PerLineDiags::new(lines);
        let mut updates = 0u64;
        let mut record = None;
        for (i, line) in (band.0..band.1).enumerate() {
            let spec = line_spec(&self.setup.reference, line);
            let line_played = on_command
                && line_off_command(
                    played.trace(line),
                    state.command.trace(line),
                    self.cfg.realize_tol_db,
                );
            let from_played = !on_command || line_played;
            diags.base_played[line] = if from_played { 1.0 } else { 0.0 };
            if spec.trace <= 0.0 {
                continue;
            }
            diags.base_played_lines += line_played as usize;
            let base_stack = if from_played { played } else { &state.command };
            let base = full_line(base_stack, line);
            if !base.iter().all(|v| v.re.is_finite() && v.im.is_finite()) {
                return Err(LawError(format!(
                    "the base drive is not finite on line {line}"
                )));
            }
            let measured = inputs.response.diagonal(line);
            let li = LineInputs {
                h: inputs.frf.line(line),
                spec: &spec,
                measured: &measured,
                coherence: &inputs.multiple_coherence[line * nc..(line + 1) * nc],
                base,
                command: state.command.diagonal(line),
                previous: state.previous.as_ref().map(|p| p.diagonal(line)),
                memory: state.memory[i].as_ref(),
                averages,
                resonance_gain: resonance[i],
                floor: state.floors[i],
            };
            let mut rec = (self.record_line == Some(line)).then(|| LineRecord {
                line,
                cycle,
                base_played: from_played,
                start_scale: 1.0,
                ..LineRecord::default()
            });
            let out = step_line(&knobs, nc, nd, &li, rec.as_mut());
            if rec.is_some() {
                record = rec;
            }
            if !out
                .drive
                .iter()
                .all(|v| v.re.is_finite() && v.im.is_finite())
            {
                return Err(LawError(format!("the drive is not finite on line {line}")));
            }
            put_line(&mut drive, line, &out.drive);
            put_line(&mut kept, line, &out.kept);
            diags.step_scale[line] = out.step_scale;
            diags.ceiling_factor[line] = out.ceiling_factor;
            diags.floor_lifted[line] = if out.floor_lifted { 1.0 } else { 0.0 };
            if out.updated {
                diags.updated[line] = 1.0;
                updates += 1;
            }
            memory[i] = Some(out.memory);
        }
        Ok(Cycle {
            drive,
            diags,
            state: State {
                command: kept,
                previous: None,
                memory,
                cycle,
                floors: state.floors.clone(),
            },
            updates,
            record,
            resonance,
        })
    }
}

/// The line-by-line test of the base drive (Choice 19): the played drive's
/// trace more than `tol_db` from the command's. Lines where either trace is
/// zero or not finite are left to the band's median.
fn line_off_command(played: f64, command: f64, tol_db: f64) -> bool {
    played > 0.0
        && command > 0.0
        && played.is_finite()
        && command.is_finite()
        && (DB_PER_NEPER_POWER * ln(played / command)).abs() > tol_db
}

impl BuiltinMimoLaw for BroydenCongruenceLaw {
    fn spec() -> LawSpec {
        let mut params = regularization_params();
        params.extend([
            number(
                "max_drive_coherence",
                "Largest drive coherence",
                "No two drives may be more coherent than this on a line: a cross spectrum \
                 above it is shrunk to it. 1 turns the cap off.",
                "",
                (0.5, 1.0, 0.01, 0.95),
            ),
            toggle(
                "start_cap",
                "Cap the first drive",
                "Scale the first drive so its predicted response is start_cap_db below the \
                 specification, as the author's note does. Off, the first drive is at full \
                 level and the controller's own level schedule applies.",
                false,
            ),
            number(
                "start_cap_db",
                "First drive cap",
                "With start_cap on: the summed predicted response of the first drive is held \
                 this far below the summed specification. The note's value is -9 dB.",
                "dB",
                (-40.0, 0.0, 0.5, -9.0),
            ),
            convergence_param(),
            number(
                "max_step_db",
                "Largest step",
                "The most any control's response, predicted by the learned and the model \
                 sensitivity, or any drive's level may change in one loop.",
                "dB",
                (0.1, 20.0, 0.1, 3.0),
            ),
            number(
                "ridge",
                "Ridge weight",
                "Damps the solve for the gains, relative to the size of the sensitivity. \
                 Larger values take smaller, steadier steps in weak directions.",
                "",
                (1e-6, 10.0, 0.01, 0.1),
            ),
            number(
                "resonance_sensitivity",
                "Resonance sensitivity",
                "How much the step is cut where a control's frequency response stands above \
                 its running median over neighboring lines. 0 turns the cut off.",
                "",
                (0.0, 10.0, 0.1, 0.5),
            ),
            number(
                "resonance_halfwidth_oct",
                "Resonance window",
                "Half-width of the window, in octaves on each side of a line, over which the \
                 running median of the frequency response is taken. Where the window holds \
                 fewer than 5 lines (the lowest lines of the band at the default width) no \
                 cut is made.",
                "oct",
                (0.05, 2.0, 0.05, 1.0 / 3.0),
            ),
            number(
                "resonance_min_gain",
                "Smallest resonance gain",
                "The resonance cut never takes the step below this fraction. 0 sets no floor.",
                "",
                (0.0, 1.0, 0.05, 0.0),
            ),
            number(
                "ceiling_db",
                "Overshoot ceiling",
                "If a control's expected response would rise this far above its specification, \
                 the whole line's drive is scaled down at once, whatever the step limit.",
                "dB",
                (0.0, 40.0, 0.5, 12.0),
            ),
            number(
                "broyden_gain",
                "Learning rate",
                "How much of the difference between what the last step did and what the \
                 sensitivity predicted is learned each loop. 0 learns nothing.",
                "",
                (0.0, 1.0, 0.05, 0.6),
            ),
            number(
                "coherence_power",
                "Coherence weighting",
                "Each control's error is weighted by its multiple coherence to this power. 0 \
                 weights every control alike.",
                "",
                (0.0, 4.0, 0.5, 1.0),
            ),
            number(
                "model_anchor",
                "Model anchor",
                "The fraction by which the learned sensitivity is pulled toward the model's \
                 each loop. 1 uses the model's sensitivity alone.",
                "",
                (0.0, 1.0, 0.01, 0.05),
            ),
            number(
                "min_du_db",
                "Smallest step learned from",
                "A line whose drive gains changed less than this last loop learns nothing from \
                 it, since the change it would learn from is mostly noise.",
                "dB",
                (0.0, 3.0, 0.01, 0.05),
            ),
            toggle(
                "split_level",
                "Split off the common level",
                "Solve first for one gain common to every drive, then for the shares. It keeps \
                 the ridge from holding back an overall level error.",
                true,
            ),
            toggle(
                "base_on_command",
                "Build on the command",
                "Step from the law's own last drive when it was played as commanded, so \
                 estimation noise in the drive does not accumulate. Off, every step starts \
                 from the drive as played.",
                true,
            ),
            number(
                "realize_tol_db",
                "Played drive tolerance",
                "When the drive as played differs from the command by more than this (the \
                 median over the band), the next step starts from the drive as played; and \
                 on any line where it differs by more than this, that line does.",
                "dB",
                (0.0, 40.0, 0.5, 3.0),
            ),
            toggle(
                "phase_steering",
                "Steer phases",
                "Turn each drive's phase as well as its gain, which can reach autospectra the \
                 gains alone cannot. It relies on the system reproducing the phase between \
                 drives.",
                true,
            ),
            number(
                "max_phase_step_deg",
                "Largest phase step",
                "With phase steering: the most any drive's phase may turn in one loop.",
                "deg",
                (1.0, 90.0, 1.0, 15.0),
            ),
            number(
                "effective_averages",
                "Effective averages",
                "Caps the number of averages the learning trusts, for exponential averaging \
                 (2 / alpha - 1). 0 uses half the response's degrees of freedom.",
                "",
                (0.0, 1e6, 1.0, 0.0),
            ),
        ]);
        LawSpec {
            name: "Broyden congruence",
            version: "1.0.0",
            author: "N. Hunter",
            summary: "N. Hunter's law, from his note \"The Broyden Congruence Law, Step by Step\" \
                      (2026), used with his permission. It specifies autospectra only. The \
                      first drive is the inverse of the reference; after that each loop \
                      multiplies each drive by its own gain and, with phase steering, turns its \
                      phase (a congruence, which keeps the coherence between drives). The gains \
                      solve the log error through a sensitivity of the log response to the log \
                      gain, which starts as the model's and is corrected each loop from what \
                      the last step did by a damped Broyden secant update (C. G. Broyden, \
                      Mathematics of Computation, 1965), with a step limit, a cut at \
                      resonances and an overshoot ceiling.",
            params,
            diagnostics: vec![
                diag_correction(),
                per_line(
                    "resonance_gain",
                    "Resonance gain",
                    "The factor the resonance schedule multiplies each line's step by: 1 \
                     where the frequency response is not above its running median.",
                ),
                per_line(
                    "step_scale",
                    "Step limit factor",
                    "The factor the step limit scaled each line's step by this loop: 1 where \
                     the step was within the limit.",
                ),
                per_line(
                    "ceiling_factor",
                    "Ceiling factor",
                    "The factor the overshoot ceiling scaled each line's drive by this loop: \
                     1 where it did not act.",
                ),
                per_line(
                    "broyden_updated",
                    "Sensitivity updated",
                    "1 on a line where the update changed the learned sensitivity this loop; \
                     0 where it was skipped or learned nothing (every trust weight zero, or \
                     nothing to correct).",
                ),
                per_line(
                    "base_played",
                    "Built on the played drive",
                    "1 on a line this loop stepped from the drive as played, 0 where it \
                     stepped from the law's own command: the band's median decides for every \
                     line, and a line whose played drive is more than realize_tol_db from its \
                     command is stepped from the played drive.",
                ),
                per_line(
                    "floor_lifted",
                    "Lifted to the floor",
                    "1 on a line whose drive was lifted to the floor this loop: 1e-8 of the \
                     trace of the model drive, formed at the first drive from its frequency \
                     response, truncated at 1e-3, and the reference.",
                ),
                DiagDecl {
                    key: "base_played_lines",
                    label: "Lines built on the played drive",
                    unit: "",
                    help: "Lines this loop stepped from the drive as played because their played \
                           drive was more than realize_tol_db from the command, while the \
                           band's median kept the others on the command.",
                    kind: DiagKind::Scalar,
                },
                DiagDecl {
                    key: "broyden_updates",
                    label: "Sensitivity updates",
                    unit: "",
                    help: "Line updates that changed the learned sensitivity since the first \
                           drive.",
                    kind: DiagKind::Scalar,
                },
            ],
            shapes: ALL_SHAPES.to_vec(),
            needs: NEEDS.to_vec(),
        }
    }

    fn build(setup: &MimoSetup, params: &[(String, ParamVal)]) -> Result<Self, LawError> {
        let mut cfg = BroydenConfig::default();
        for (k, v) in params {
            cfg.set(k, v)?;
        }
        Self::new(setup, &cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::detmath;

    fn rel(a: f64, b: f64) -> f64 {
        if a == b {
            0.0
        } else {
            (a - b).abs() / a.abs().max(b.abs())
        }
    }

    #[test]
    fn the_elementary_functions_agree_with_the_library_to_a_few_ulps() {
        let mut worst: [f64; 4] = [0.0; 4];
        let mut x = 1e-300f64;
        while x < 1e300 {
            worst[0] = worst[0].max(rel(detmath::ln(x), x.ln()));
            x *= 1.37;
        }
        let mut x = -700.0f64;
        while x < 700.0 {
            worst[1] = worst[1].max(rel(detmath::exp(x), x.exp()));
            x += 0.0731;
        }
        let mut x = -20.0f64;
        while x < 20.0 {
            let (s, c) = detmath::sin_cos(x);
            worst[2] = worst[2].max((s - x.sin()).abs());
            worst[3] = worst[3].max((c - x.cos()).abs());
            x += 0.00917;
        }
        assert!(worst[0] < 4e-16, "ln {}", worst[0]);
        assert!(worst[1] < 4e-16 * 700.0, "exp {}", worst[1]);
        assert!(worst[2] < 4e-16 && worst[3] < 4e-16, "sin cos {worst:?}");
        // Edges.
        assert_eq!(detmath::ln(1.0), 0.0);
        assert_eq!(detmath::exp(0.0), 1.0);
        assert!(detmath::ln(-1.0).is_nan());
        assert_eq!(detmath::ln(0.0), f64::NEG_INFINITY);
        assert_eq!(detmath::exp(-1000.0), 0.0);
        assert_eq!(detmath::exp(1000.0), f64::INFINITY);
        assert!(rel(detmath::ln(5e-320), 5e-320f64.ln()) < 1e-15);
        assert!(rel(detmath::exp(-740.0), (-740.0f64).exp()) < 1e-3);
    }
}
