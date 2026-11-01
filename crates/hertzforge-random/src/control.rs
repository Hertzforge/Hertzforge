//! The Random closed-loop control core: the [`ControlLaw`] trait, the
//! shipped [`HinvLaw`], the control composite, and the notch limiter with the
//! safety arbitration.
//!
//! # The level/shape split - WHY the loop is factored this way
//!
//! A random controller has two orthogonal jobs that MUST NOT fight each other:
//!
//! 1. **Level** - hold the overall in-band RMS on target. This is a scalar servo in
//!    the dB domain ([`hertzforge_dsp::sine::AmplitudeServo`] semantics: velocity-form PI,
//!    rate-clamped, with a fault-hold on a dead measurement and an absolute drive
//!    ceiling). It owns the ONE number that scales the whole drive spectrum.
//! 2. **Shape** - flatten the per-bin response error so the measured PSD matches the
//!    target SHAPE, while PRESERVING the in-band mean (so it never moves the level).
//!    This is the mean-preserving per-bin trim.
//!
//! `hertzforge-demo`'s `RandomController` proved this split converges: the servo owns the
//! mean, the trim owns the shape, and because the trim renormalizes to hold the mean
//! it cannot wind the servo up or down. This crate keeps that split but moves BOTH
//! loops INSIDE a single [`ControlLaw`] ([`HinvLaw`]) that reports the level command
//! separately (the law owns both, level is surfaced). This is what lets a
//! future MIMO law swap in a matrix-valued update without changing the loop boundary.
//!
//! # What this crate adds on top of the demo trim (the production gates)
//!
//! - **Measurement trust gate.** A bin whose coherence OR estimated
//!   plant gain is too low is UNTRUSTED: its shape-trim update is skipped this
//!   iteration (drive shape held there) rather than chased into noise. This is
//!   distinct from Tikhonov regularization, which SOFTENS the inversion (attenuates
//!   low-gain bins) but never "holds" - see [`HinvConfig`] for the reconciliation.
//! - **Per-iteration correction clamp.** Every per-bin trim step is
//!   clamped to `max_correction_db_per_iter`, bounding all transients.
//! - **Divergence accounting.** A bin whose trim clamps at the max for
//!   `divergence_n` consecutive iterations is reported as divergent; the caller aborts
//!   when divergent bins persist for `divergence_loops` consecutive loops.
//! - **Notch exclusion contract.** Notched bins and untrusted bins
//!   are EXCLUDED from the mean-preserving renormalization set, so the shape loop may
//!   not re-inflate a bin the notch is holding down or a bin it does not trust.

use hertzforge_dsp::frf::{h_inverse_regularized, suggest_epsilon};
use hertzforge_dsp::sine::AmplitudeServo;
use serde::{Deserialize, Serialize};

/// Errors from constructing or configuring a control law or a notch set. Library
/// paths return these instead of panicking (a real run cannot be aborted by a
/// panic in the control math).
#[derive(Debug, Clone, PartialEq)]
pub enum ControlError {
    /// A config knob was out of its valid range. Carries the knob name and a note.
    BadConfig(String),
    /// A per-loop input vector had the wrong length or an empty grid.
    BadInputs(String),
    /// A composite was requested over zero channels, or with a channel/weight count
    /// mismatch, or ragged channel PSD lengths.
    BadComposite(String),
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlError::BadConfig(s) => write!(f, "control law config error: {s}"),
            ControlError::BadInputs(s) => write!(f, "control loop input error: {s}"),
            ControlError::BadComposite(s) => write!(f, "control composite error: {s}"),
        }
    }
}

impl std::error::Error for ControlError {}

// =============================================================================
// The loop boundary
// =============================================================================

/// Everything a control law sees on one loop iteration. The REAL loop boundary:
/// the target and the measured composite control PSD on the line grid,
/// the plant estimate, per-bin coherence, and the current overall level.
///
/// Borrowed slices (no allocation to call a law). All spectra share the SAME line
/// grid: length `N` bins spaced `df_hz` apart, `g^2/Hz`, index `k` at frequency
/// `k*df_hz` (the [`hertzforge_breakpoints::LineSpectrum`] convention with `f0 = 0`).
///
/// # Why `h_mag2` and not complex `H`
///
/// For a SISO shape-trim the only thing the update needs from the plant is the
/// per-bin MAGNITUDE: the H-inverse seed is `drive = target / |H|^2` (Tikhonov
/// softens the divide), and the mean-preserving trim compares measured vs target
/// PSD (magnitudes). Phase does not enter the single-output drive-PSD synthesis at
/// all - the COLA synth draws random phase per bin regardless. Passing `|H|^2`
/// (rather than complex `H`) keeps the SISO boundary honest about what it uses.
/// The MIMO generalization is a matrix-valued `H` (cross-channel phase matters
/// there); this boundary is shaped so that becomes a type change on
/// the inputs, not a rewrite of the trait.
pub struct LoopInputs<'a> {
    /// Target control PSD on the line grid, `g^2/Hz`. Zero out-of-band.
    pub target_psd: &'a [f64],
    /// Measured composite control PSD (post-[`composite_psd`]), same grid, `g^2/Hz`.
    pub measured_psd: &'a [f64],
    /// Per-bin plant gain magnitude-squared `|H|^2` estimate (drive->response).
    pub h_mag2: &'a [f64],
    /// Per-bin ordinary coherence `gamma^2` in `[0,1]` from
    /// [`hertzforge_dsp::frf::h_estimate_hv_with_coherence`].
    pub coherence: &'a [f64],
    /// The controller's current overall level in dB (0 dB = full shaped target).
    pub current_level_db: f64,
    /// Line-grid bin width (Hz), for RMS integration and level bookkeeping.
    pub df_hz: f64,
    /// Bins the notch limiter is HOLDING DOWN this loop. A law MUST
    /// exclude these from its mean-preserving renormalization set so the shape loop
    /// cannot re-inflate a notch. Empty on the first loop (nothing notched yet); the
    /// caller feeds back the previous loop's [`NotchReport::notched`] mask. Length
    /// `N` or empty (empty = none notched).
    pub notched_bins: &'a [bool],
    /// The half-open in-band window `[lo, hi)` in line-grid bins. The level servo and
    /// the mean-preserving trim operate over this window only; bins outside carry no
    /// control energy. Derived by the caller from the target profile's support.
    pub band: (usize, usize),
}

impl LoopInputs<'_> {
    /// Number of line-grid bins (from the target).
    pub fn nbins(&self) -> usize {
        self.target_psd.len()
    }

    /// True if a bin index is currently notch-held (safe for an empty mask).
    #[inline]
    fn is_notched(&self, k: usize) -> bool {
        self.notched_bins.get(k).copied().unwrap_or(false)
    }

    /// Validate the per-loop invariants a law relies on. Returns [`ControlError`] on
    /// a ragged grid or a nonsensical band, rather than letting a slice index panic.
    fn validate(&self) -> Result<(), ControlError> {
        let n = self.target_psd.len();
        if n == 0 {
            return Err(ControlError::BadInputs("empty target grid".into()));
        }
        if self.measured_psd.len() != n || self.h_mag2.len() != n || self.coherence.len() != n {
            return Err(ControlError::BadInputs(format!(
                "ragged grid: target {n}, measured {}, h_mag2 {}, coherence {}",
                self.measured_psd.len(),
                self.h_mag2.len(),
                self.coherence.len()
            )));
        }
        if !self.notched_bins.is_empty() && self.notched_bins.len() != n {
            return Err(ControlError::BadInputs(format!(
                "notched mask length {} != grid {n}",
                self.notched_bins.len()
            )));
        }
        let (lo, hi) = self.band;
        if lo >= hi || hi > n {
            return Err(ControlError::BadInputs(format!(
                "bad band [{lo},{hi}) for grid of {n}"
            )));
        }
        // `<= 0.0 || is_nan()` (not `!(x > 0.0)`) so a NaN df is caught without a
        // negated partial-order comparison (clippy neg_cmp_op_on_partial_ord), the
        // hertzforge-breakpoints convention.
        if self.df_hz <= 0.0 || self.df_hz.is_nan() {
            return Err(ControlError::BadInputs(format!(
                "df_hz must be positive (got {})",
                self.df_hz
            )));
        }
        Ok(())
    }
}

/// Per-loop diagnostics a law reports alongside its drive. The caller surfaces these
/// to the UI / run log and uses [`Self::divergent_bins`] for the divergence abort.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoopDiagnostics {
    /// Bins skipped this iteration by the measurement trust gate:
    /// coherence below floor OR `|H|` below the floor fraction of peak `|H|`.
    pub untrusted_bins: Vec<usize>,
    /// Bins whose per-iteration trim step was clamped at
    /// `max_correction_db_per_iter` this iteration.
    pub clamped_bins: Vec<usize>,
    /// Bins that have clamped for `divergence_n` CONSECUTIVE iterations: the
    /// per-bin divergence signal. Empty when healthy. The caller counts
    /// consecutive nonempty loops and aborts at `divergence_loops`.
    pub divergent_bins: Vec<usize>,
    /// In-band RMS error (dB) between measured and target on this loop, or `None`
    /// before any valid measurement.
    pub inband_err_db: Option<f64>,
    /// Realized in-band RMS (g) integrated over the band from the measured PSD.
    pub inband_rms_g: Option<f64>,
    /// Whether the level servo latched a fault-hold this loop (dead measurement).
    pub servo_faulted: bool,
}

/// One law's output for a loop: the drive PSD to synthesize (before notch limiting -
/// the caller applies the notch AFTER), the level command in dB the
/// law is holding, and the diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopOutputs {
    /// Drive PSD on the line grid, `g^2/Hz`, that the COLA synth should realize
    /// (BEFORE the notch ceiling is applied). Zero out-of-band.
    pub drive_psd: Vec<f64>,
    /// The overall level the law is commanding, in dB (0 dB = full shaped target).
    /// Surfaced separately so a UI shows the servo state distinct from the shape.
    pub level_cmd_db: f64,
    /// Diagnostics for this loop.
    pub diagnostics: LoopDiagnostics,
}

/// A pluggable random control law.
///
/// The law owns BOTH the scalar level servo and the per-bin shape trim; it reports
/// the level command separately in [`LoopOutputs::level_cmd_db`]. [`Self::init`]
/// seeds the shape from the plant inverse (the low-level system-ID handoff);
/// [`Self::iterate`] advances one loop given a fresh measurement.
///
/// # This trait is NOT the frozen module ABI
///
/// This is the internal loop boundary, chosen so a MIMO law (matrix-valued `H`,
/// cross-channel drive) can slot in without a signature break. The user-supplied
/// pluggable-law ABI - the stable, versioned boundary a third party compiles
/// against - is DEFERRED until MIMO informs its shape. Do not treat
/// this trait as that ABI; it will move as the internal design learns from MIMO.
pub trait ControlLaw {
    /// Seed the law from the first (low-level, system-ID) measurement: estimate the
    /// plant, seed the drive shape from its regularized inverse so the RESPONSE (not
    /// the drive) matches the target, and return the initial drive.
    fn init(&mut self, inputs: &LoopInputs) -> LoopOutputs;

    /// Advance one control loop: update the level servo and the shape trim from the
    /// latest measurement, and return the next drive.
    fn iterate(&mut self, inputs: &LoopInputs) -> LoopOutputs;

    /// A short stable identifier for the law (for run logs / UI selection).
    fn name(&self) -> &str;
}

// =============================================================================
// Control composite (per-channel weighting, min/max/avg modes)
// =============================================================================

/// How multiple control channels combine into the single composite PSD the law
/// controls to. Weights are normalized to sum 1 before use, so
/// they express RELATIVE emphasis, not absolute scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompositeMode {
    /// Weighted average per bin: `sum_i w_i * psd_i[k]` (weights normalized). The
    /// standard multi-point average control: the loop drives the weighted MEAN of the
    /// control points to target.
    WeightedAvg,
    /// Per-bin MIN of the weighted channel PSDs: `min_i (w_i * psd_i[k])`. Controls
    /// so the QUIETEST (weighted) point reaches target - the loop keeps pushing until
    /// the weakest control point is satisfied (drives everything else higher). The
    /// weighting is applied to each channel's PSD BEFORE the min so a de-emphasized
    /// (small weight) channel is less likely to be the binding minimum.
    Min,
    /// Per-bin MAX of the weighted channel PSDs: `max_i (w_i * psd_i[k])`. Controls so
    /// the LOUDEST (weighted) point does not exceed target - the protective mode that
    /// backs off as soon as any control point is at target. Weighting applied before
    /// the max, so an up-weighted channel dominates the max sooner.
    Max,
}

/// Combine `channels` (each a per-bin control PSD on the SAME line grid) into one
/// composite control PSD using `mode` and per-channel `weights`.
///
/// # Semantics (precise)
///
/// Weights are normalized so `sum w_i == 1` (a zero-sum or all-negative weight set is
/// rejected). Let `p_i[k]` be channel `i`'s PSD at bin `k`.
///
/// - [`CompositeMode::WeightedAvg`]: `composite[k] = sum_i w_i * p_i[k]`.
/// - [`CompositeMode::Min`]: `composite[k] = min_i (w_i * p_i[k])`.
/// - [`CompositeMode::Max`]: `composite[k] = max_i (w_i * p_i[k])`.
///
/// Min/Max operate on the WEIGHTED channel PSDs (`w_i * p_i[k]`), not the raw ones,
/// so the weighting still expresses emphasis in those modes: down-weighting a channel
/// makes it a less likely binding min and a less likely dominating max.
///
/// Returns [`ControlError::BadComposite`] on zero channels, a channel/weight count
/// mismatch, a ragged grid, or a non-positive normalizer.
pub fn composite_psd(
    channels: &[Vec<f64>],
    weights: &[f64],
    mode: CompositeMode,
) -> Result<Vec<f64>, ControlError> {
    if channels.is_empty() {
        return Err(ControlError::BadComposite("no control channels".into()));
    }
    if channels.len() != weights.len() {
        return Err(ControlError::BadComposite(format!(
            "{} channels but {} weights",
            channels.len(),
            weights.len()
        )));
    }
    let n = channels[0].len();
    if n == 0 {
        return Err(ControlError::BadComposite("empty channel grid".into()));
    }
    for (i, ch) in channels.iter().enumerate() {
        if ch.len() != n {
            return Err(ControlError::BadComposite(format!(
                "channel {i} length {} != channel 0 length {n}",
                ch.len()
            )));
        }
    }
    // Normalize weights (relative emphasis). Reject a non-positive normalizer so a
    // zeroed or all-negative weight set cannot silently produce a zero composite.
    let wsum: f64 = weights.iter().sum();
    if wsum <= 0.0 || wsum.is_nan() {
        return Err(ControlError::BadComposite(format!(
            "weights must sum to a positive value (got {wsum})"
        )));
    }
    let wnorm: Vec<f64> = weights.iter().map(|&w| w / wsum).collect();

    let mut out = vec![0.0f64; n];
    match mode {
        CompositeMode::WeightedAvg => {
            for (ch, &w) in channels.iter().zip(wnorm.iter()) {
                for (o, &p) in out.iter_mut().zip(ch.iter()) {
                    *o += w * p;
                }
            }
        }
        CompositeMode::Min => {
            for (k, o) in out.iter_mut().enumerate() {
                let mut m = f64::INFINITY;
                for (ch, &w) in channels.iter().zip(wnorm.iter()) {
                    m = m.min(w * ch[k]);
                }
                *o = m;
            }
        }
        CompositeMode::Max => {
            for (k, o) in out.iter_mut().enumerate() {
                let mut m = f64::NEG_INFINITY;
                for (ch, &w) in channels.iter().zip(wnorm.iter()) {
                    m = m.max(w * ch[k]);
                }
                *o = m;
            }
        }
    }
    Ok(out)
}

// =============================================================================
// Notch limiting with the safety arbitration
// =============================================================================

/// A per-bin drive-PSD ceiling set. Built from per-channel limit
/// profiles: an [`hertzforge_breakpoints::Envelope`]-style response ceiling (or a force
/// spec) mapped through the channel's measured-response-to-drive relation into a
/// ceiling on the DRIVE PSD. Applied AFTER the law synthesizes its drive:
/// `drive[k] = min(drive[k], ceiling[k])`. Limit-down ALWAYS beats control-up.
///
/// # Building the ceiling (the response->drive map)
///
/// A limit is authored as a RESPONSE ceiling `resp_max[k]` (`g^2/Hz` for an accel
/// channel, or a force spec for a force channel). To turn it into a drive-PSD ceiling
/// it is divided by the per-bin drive->response power gain `|H|^2`: the drive PSD that
/// would produce exactly the ceiling response is `resp_max[k] / |H|^2[k]`. Where
/// `|H|^2` is unknown or ~0 (a bin that cannot be driven) the ceiling is left at
/// `+inf` (no limit - a bin the drive cannot move cannot breach a response limit
/// through this actuator). Multiple channels contribute the MINIMUM ceiling per bin
/// (the tightest limit wins), which is the safety-monotone choice.
///
/// # Force-channel limiting is the SAME mechanism
///
/// A force-limit profile is just another response ceiling whose `|H|^2` is the
/// drive->force power gain. Random force protection is PREDICTIVE/NOTCH only:
/// this notch, plus the frequency-domain envelope alarm, NOT a reactive force
/// abort axis. Build force ceilings with [`NotchSet::add_channel_ceiling`] exactly
/// like an accel ceiling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotchSet {
    /// Per-bin drive-PSD ceiling, `g^2/Hz`. `f64::INFINITY` = no limit at that bin.
    ceiling: Vec<f64>,
}

/// The result of applying a [`NotchSet`] to a drive: which bins were clamped, and the
/// per-bin mask the law needs next loop to exclude notched bins from renormalization.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NotchReport {
    /// Line-grid bins clamped to their ceiling this loop.
    pub notched_lines: Vec<usize>,
    /// Per-bin boolean: `true` where the drive was clamped. Fed back into
    /// [`LoopInputs::notched_bins`] next loop (the notch exclusion seam).
    pub notched: Vec<bool>,
}

impl NotchSet {
    /// An all-`+inf` (no-limit) notch set over `n` bins.
    pub fn unlimited(n: usize) -> Self {
        NotchSet {
            ceiling: vec![f64::INFINITY; n],
        }
    }

    /// Add a channel's response ceiling, mapping it through the channel's per-bin
    /// drive->response power gain `h_mag2` into a drive-PSD ceiling and taking the
    /// per-bin MINIMUM with the existing ceilings (tightest limit wins).
    ///
    /// `resp_ceiling[k]` is the maximum allowed response PSD at bin `k` (`g^2/Hz`, or
    /// a force spec for a force channel). `h_mag2[k]` is the drive->response power
    /// gain `|H|^2`. Where `h_mag2[k]` is `<= h_floor` (a bin the drive cannot move),
    /// the ceiling contribution is `+inf` (no limit): the response there is not
    /// reachable through this drive, so it cannot be limited through it.
    ///
    /// Lengths must match the set's grid (returns [`ControlError::BadInputs`]).
    pub fn add_channel_ceiling(
        &mut self,
        resp_ceiling: &[f64],
        h_mag2: &[f64],
        h_floor: f64,
    ) -> Result<(), ControlError> {
        let n = self.ceiling.len();
        if resp_ceiling.len() != n || h_mag2.len() != n {
            return Err(ControlError::BadInputs(format!(
                "notch channel grid mismatch: set {n}, ceiling {}, h_mag2 {}",
                resp_ceiling.len(),
                h_mag2.len()
            )));
        }
        let floor = h_floor.max(0.0);
        for k in 0..n {
            let rc = resp_ceiling[k];
            // A non-positive or infinite response ceiling means "no limit" at k.
            let drive_ceiling = if !rc.is_finite() || rc < 0.0 {
                f64::INFINITY
            } else if h_mag2[k] > floor && h_mag2[k] > 0.0 {
                rc / h_mag2[k]
            } else {
                // Un-driveable bin: cannot breach a response limit through this drive.
                f64::INFINITY
            };
            if drive_ceiling < self.ceiling[k] {
                self.ceiling[k] = drive_ceiling;
            }
        }
        Ok(())
    }

    /// Set an explicit per-bin drive-PSD ceiling directly (min with the existing).
    /// For callers that already have the drive-domain ceiling in hand (e.g. a fixed
    /// drive-voltage-derived cap). Length must match the grid.
    pub fn set_drive_ceiling(&mut self, drive_ceiling: &[f64]) -> Result<(), ControlError> {
        let n = self.ceiling.len();
        if drive_ceiling.len() != n {
            return Err(ControlError::BadInputs(format!(
                "notch drive-ceiling length {} != grid {n}",
                drive_ceiling.len()
            )));
        }
        for (c, &d) in self.ceiling.iter_mut().zip(drive_ceiling.iter()) {
            if d < *c {
                *c = d;
            }
        }
        Ok(())
    }

    /// Number of bins in the set.
    pub fn len(&self) -> usize {
        self.ceiling.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ceiling.is_empty()
    }

    /// The per-bin ceiling (for tests / UI display). `+inf` = unlimited.
    pub fn ceiling(&self) -> &[f64] {
        &self.ceiling
    }

    /// Apply the ceiling to a drive PSD in place: `drive[k] = min(drive[k],
    /// ceiling[k])` (limit-down beats control-up ALWAYS). Returns the
    /// [`NotchReport`] with the clamped bins and the per-bin mask to feed back into
    /// [`LoopInputs::notched_bins`] next loop.
    ///
    /// A drive length shorter than the set clamps only the overlapping prefix; the
    /// report mask is sized to the drive. This is the ONE place the notch is applied,
    /// so the arbitration lives in exactly one seam.
    pub fn apply(&self, drive: &mut [f64]) -> NotchReport {
        let mut report = NotchReport {
            notched_lines: Vec::new(),
            notched: vec![false; drive.len()],
        };
        for (k, d) in drive.iter_mut().enumerate() {
            let Some(&c) = self.ceiling.get(k) else {
                break;
            };
            if *d > c {
                *d = c;
                report.notched_lines.push(k);
                report.notched[k] = true;
            }
        }
        report
    }
}

// =============================================================================
// HinvLaw - the one law shipped now
// =============================================================================

/// Knobs for [`HinvLaw`]. Defaults reproduce a conservative, stable loop close to the
/// converged `hertzforge-demo` trim, with the trust/clamp/divergence gates active.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HinvConfig {
    /// Tikhonov epsilon for the H-inverse seed. `None` = use the
    /// [`hertzforge_dsp::frf::suggest_epsilon`] `(beta*Hmax)^2` policy with
    /// [`Self::suggest_beta`]. `Some(eps)` uses `eps` directly (Tikhonov
    /// owns inversion softening).
    pub lambda: Option<f64>,
    /// The `beta` fed to [`hertzforge_dsp::frf::suggest_epsilon`] when `lambda` is `None`
    /// (the fraction of peak `|H|` below which the plant is treated as un-driveable).
    pub suggest_beta: f64,
    /// Measurement TRUST gate: coherence floor. A bin with coherence
    /// below this SKIPS its shape-trim update this iteration (drive shape held).
    /// Default 0.5.
    pub trust_coherence_floor: f64,
    /// Measurement TRUST gate: `|H|` floor as a fraction of the peak in-band `|H|`.
    /// A bin whose `|H|` is below `trust_h_floor_frac * max|H|` SKIPS
    /// its trim update (untrusted low-gain bin). Default 0.05. A bin failing EITHER
    /// floor is untrusted.
    pub trust_h_floor_frac: f64,
    /// Maximum per-bin shape-trim step magnitude, dB per iteration.
    /// Every trim step is clamped to `+/- this`. Default 2.0.
    pub max_correction_db_per_iter: f64,
    /// Truncated-SVD retained-singular-value count. Accepted and validated; MIMO-
    /// future (for SISO this is the rank-1 degenerate case, no behavior
    /// beyond validation). `Some(k)` with `k == 0` is rejected.
    pub svd_keep: Option<usize>,
    /// Correction damping 0..=10 (operator knob, 2026-08-02 spec; default 4).
    ///
    /// The per-bin trim gain is `1 / (1 + damping)`: 0 corrects at full step
    /// every loop, 10 spreads a correction across roughly ten loops. Why it
    /// exists: the loop iterates on a FINITE-DOF Welch estimate. At nAvg 4 a
    /// line has 8 DOF (sigma ~2.2 dB), and a gain-1.0 trim writes the full
    /// inverse of that scatter into the drive shape every loop - measured on
    /// the jumpered cDAQ 2026-08-03 as 114-123 of 991 lines outside a +/-3 dB
    /// alarm at steady level, and an envelope abort at the 10-line budget on a
    /// PERFECT wire. Damping trades convergence speed for immunity to
    /// estimator noise; it works WITH `max_correction_db_per_iter` (the dB cap
    /// bounds one step, the gain sets how much of the observed error a step
    /// takes).
    pub damping: f64,
    /// Consecutive iterations a bin must clamp at the max correction before it is
    /// reported divergent. Default 10.
    pub divergence_n: u32,
    /// Scalar level servo: maximum drive slew, dB/s
    /// ([`hertzforge_dsp::sine::AmplitudeServo`]). Default 1.0.
    pub servo_rate_db_per_s: f64,
    /// Level-servo fault fraction: a measured in-band RMS below `target *
    /// fault_fraction` is treated as a dead-measurement fault (drive held). Default
    /// 1e-3.
    pub servo_fault_fraction: f64,
    /// Level-servo absolute drive-scale ceiling (safety bound). Default 64.0
    /// (~+36 dB over the 0 dB shaped level), matching hertzforge-demo's headroom.
    pub servo_drive_ceiling: f64,
}

impl Default for HinvConfig {
    fn default() -> Self {
        HinvConfig {
            lambda: None,
            suggest_beta: 0.05,
            trust_coherence_floor: 0.5,
            trust_h_floor_frac: 0.05,
            max_correction_db_per_iter: 2.0,
            svd_keep: None,
            damping: 4.0,
            divergence_n: 10,
            servo_rate_db_per_s: 1.0,
            servo_fault_fraction: 1e-3,
            servo_drive_ceiling: 64.0,
        }
    }
}

impl HinvConfig {
    /// Validate the knob ranges. Returns [`ControlError::BadConfig`] on any invalid
    /// value so a misconfigured law is rejected up front, not mid-run.
    pub fn validate(&self) -> Result<(), ControlError> {
        // Range checks are written as POSITIVE failure conditions (`x < 0`, not
        // `!(x >= 0)`) with an explicit `is_nan()` guard, so a NaN knob is rejected
        // without a negated partial-order comparison (clippy
        // neg_cmp_op_on_partial_ord) - the hertzforge-breakpoints convention.
        if let Some(l) = self.lambda {
            if l < 0.0 || l.is_nan() {
                return Err(ControlError::BadConfig(format!(
                    "lambda must be >= 0 (got {l})"
                )));
            }
        }
        let in_open01 = |x: f64| x > 0.0 && x < 1.0;
        if !in_open01(self.suggest_beta) {
            return Err(ControlError::BadConfig(format!(
                "suggest_beta must be in (0,1) (got {})",
                self.suggest_beta
            )));
        }
        if self.trust_coherence_floor.is_nan() || !(0.0..=1.0).contains(&self.trust_coherence_floor)
        {
            return Err(ControlError::BadConfig(format!(
                "trust_coherence_floor must be in [0,1] (got {})",
                self.trust_coherence_floor
            )));
        }
        if self.trust_h_floor_frac.is_nan() || !(0.0..1.0).contains(&self.trust_h_floor_frac) {
            return Err(ControlError::BadConfig(format!(
                "trust_h_floor_frac must be in [0,1) (got {})",
                self.trust_h_floor_frac
            )));
        }
        if self.max_correction_db_per_iter <= 0.0 || self.max_correction_db_per_iter.is_nan() {
            return Err(ControlError::BadConfig(format!(
                "max_correction_db_per_iter must be > 0 (got {})",
                self.max_correction_db_per_iter
            )));
        }
        if let Some(k) = self.svd_keep {
            if k == 0 {
                return Err(ControlError::BadConfig(
                    "svd_keep must be >= 1 when set (rank-1 SISO degenerate)".into(),
                ));
            }
        }
        if self.divergence_n == 0 {
            return Err(ControlError::BadConfig("divergence_n must be >= 1".into()));
        }
        if self.servo_rate_db_per_s <= 0.0 || self.servo_rate_db_per_s.is_nan() {
            return Err(ControlError::BadConfig(format!(
                "servo_rate_db_per_s must be > 0 (got {})",
                self.servo_rate_db_per_s
            )));
        }
        if !in_open01(self.servo_fault_fraction) {
            return Err(ControlError::BadConfig(format!(
                "servo_fault_fraction must be in (0,1) (got {})",
                self.servo_fault_fraction
            )));
        }
        if self.servo_drive_ceiling <= 0.0 || self.servo_drive_ceiling.is_nan() {
            return Err(ControlError::BadConfig(format!(
                "servo_drive_ceiling must be > 0 (got {})",
                self.servo_drive_ceiling
            )));
        }
        Ok(())
    }
}

/// SISO H-inverse random control law. The one law shipped now.
///
/// # Structure (the demo trim, hardened)
///
/// - **Seed** ([`Self::init`]): from the low-level system-ID measurement, seed the
///   per-bin drive shape from the Tikhonov-regularized plant inverse so the RESPONSE
///   matches the target: `shape[k] = target[k] / |H|^2[k]` (Tikhonov softens the
///   divide at anti-resonances). Only in-band bins are ever driven.
/// - **Level** ([`Self::iterate`]): a scalar [`hertzforge_dsp::sine::AmplitudeServo`]
///   holds the in-band RMS on target - velocity-form dB PI, rate-clamped, with the
///   fault-hold on a dead measurement and an absolute drive ceiling. It owns the
///   overall level; the level command is surfaced in [`LoopOutputs::level_cmd_db`].
///   The servo integrates the REALIZED in-band RMS the caller measured (the
///   post-notch RMS, so it cannot wind up against a notch).
/// - **Shape** ([`Self::iterate`]): the mean-preserving per-bin trim, EXCLUDING
///   notched bins and untrusted bins from the
///   renormalization set, with every step clamped to `max_correction_db_per_iter`
///   and divergence tracked per bin.
///
/// # Trust gate vs Tikhonov (the reconciliation)
///
/// These are DIFFERENT mechanisms and both are active:
///
/// - **Tikhonov** (`lambda`) softens the INVERSION: at a low-`|H|` bin it attenuates
///   the seed drive toward zero (never "holds"). This is the one-shot seed policy.
/// - **The trust gate** (`trust_coherence_floor`, `trust_h_floor_frac`) decides each
///   ITERATION whether a bin's shape-trim UPDATE applies: an untrusted bin's drive
///   shape is HELD (not chased into noise). The scalar servo still owns level with
///   its own fault-hold, so a globally dead measurement still freezes the level.
pub struct HinvLaw {
    cfg: HinvConfig,
    target: Vec<f64>,
    /// Per-bin drive-PSD shape at unit servo level: `drive_psd[k] = shape[k] *
    /// scale^2`. Seeded from the FRF inverse, then trimmed. Length = grid.
    shape: Vec<f64>,
    servo: AmplitudeServo,
    target_inband_rms: f64,
    /// Consecutive-clamp counter per bin (divergence accounting).
    /// SIGNED per-bin clamp streak: positive counts consecutive UP-clamps,
    /// negative consecutive DOWN-clamps. Direction-aware on purpose (root cause of
    /// false Divergence aborts, 2026-08-01): a per-bin Welch estimate with a handful
    /// of averages wobbles ~+/-1 dB per loop, so most bins clamp SOMEWHERE every
    /// loop - in RANDOM directions. An unsigned any-direction streak matures everywhere
    /// on a long dwell and flagged 235/239 bins divergent on a CONVERGED wire
    /// run (measured composite dead on target). True divergence pushes one way,
    /// loop after loop; noise flips sign and restarts the count.
    clamp_streak: Vec<i32>,
    band: (usize, usize),
    initialized: bool,
    /// The scheduled level as an AMPLITUDE scale (1.0 = full level), mirrored
    /// from `retarget_level`'s feedforward scale. THE SHAPE TRIM'S REFERENCE:
    /// `self.target` is the FULL-LEVEL line, the measurement is at the
    /// SCHEDULED level, and comparing them raw reads a healthy -12 dB rung as
    /// "+12 dB low on every bin" - which matured one-directional clamp streaks
    /// across the whole band and aborted every long-dwell run as Divergence
    /// (the actual root cause of the false Divergence aborts, found on the real
    /// chassis 2026-08-01; the composite was dead on target the entire time). Power
    /// compares use its square.
    level_scale: f64,
    /// The damped per-bin trim gain, `1 / (1 + cfg.damping)`, precomputed at
    /// construction (see `HinvConfig::damping`).
    trim_gain: f64,
}

impl HinvLaw {
    /// Build the law for a `target` PSD line grid and in-band window `band = [lo,hi)`.
    /// Validates the config; returns [`ControlError`] on a bad knob or band.
    pub fn new(
        cfg: HinvConfig,
        target: &[f64],
        band: (usize, usize),
        df_hz: f64,
    ) -> Result<Self, ControlError> {
        cfg.validate()?;
        let n = target.len();
        let (lo, hi) = band;
        if lo >= hi || hi > n {
            return Err(ControlError::BadConfig(format!(
                "bad band [{lo},{hi}) for target of {n}"
            )));
        }
        if df_hz <= 0.0 || df_hz.is_nan() {
            return Err(ControlError::BadConfig(format!(
                "df_hz must be > 0 (got {df_hz})"
            )));
        }
        let target_inband_rms = Self::inband_rms(target, band, df_hz);
        if target_inband_rms <= 0.0 || target_inband_rms.is_nan() {
            return Err(ControlError::BadConfig(
                "target has no in-band energy".into(),
            ));
        }
        let servo = AmplitudeServo::new(target_inband_rms, cfg.servo_rate_db_per_s)
            // Mostly-integral, low proportional: one update per noisy loop, so a big
            // proportional kick would chase measurement noise (hertzforge-demo tuning).
            .with_gains(0.0, 0.4)
            .with_drive_ceiling(cfg.servo_drive_ceiling)
            .with_fault_fraction(cfg.servo_fault_fraction);
        let trim_gain = 1.0 / (1.0 + cfg.damping.clamp(0.0, 10.0));
        Ok(HinvLaw {
            cfg,
            target: target.to_vec(),
            shape: vec![0.0; n],
            servo,
            target_inband_rms,
            level_scale: 1.0,
            clamp_streak: vec![0; n],
            band,
            initialized: false,
            trim_gain,
        })
    }

    /// The in-band target RMS (g): `sqrt( sum_band target * df )`.
    fn inband_rms(psd: &[f64], band: (usize, usize), df_hz: f64) -> f64 {
        let (lo, hi) = band;
        let hi = hi.min(psd.len());
        if lo >= hi {
            return 0.0;
        }
        (psd[lo..hi].iter().sum::<f64>() * df_hz).max(0.0).sqrt()
    }

    /// The target in-band RMS the servo holds to (for tests / callers).
    pub fn target_inband_rms(&self) -> f64 {
        self.target_inband_rms
    }

    /// The current per-bin drive shape at unit level (for tests / UI display).
    pub fn shape(&self) -> &[f64] {
        &self.shape
    }

    /// The current servo drive scale (`drive_scale = 10^(level_db/20)`).
    pub fn drive_scale(&self) -> f64 {
        self.servo.drive_scale()
    }

    /// The current drive PSD (`shape[k] * drive_scale^2`) - the run service reads this
    /// each loop to synthesize the drive stream before applying the notch (notch seam).
    /// Additive accessor over the private [`Self::drive_from_shape`]; behavior identical.
    pub fn drive_psd(&self) -> Vec<f64> {
        self.drive_from_shape()
    }

    /// Re-target the scalar level servo to a NEW in-band RMS target and seed its
    /// feedforward scale (the schedule/level split). The SERVICE owns the schedule
    /// level (the ladder rung); it calls this to move
    /// the rung. The law keeps owning the servo TRIM around that target and the shape.
    ///
    /// This rebuilds the internal [`hertzforge_dsp::sine::AmplitudeServo`] at `target_rms`
    /// with the SAME configured gains / drive ceiling / fault fraction as construction
    /// (the servo has no public re-target, so a fresh servo seeded to the new rung is
    /// the additive way to move the target without a DSP-crate signature change), then
    /// applies `feedforward_scale` so the drive starts AT the rung and the servo only
    /// trims residual plant-estimate error. The per-bin shape (the H-inverse
    /// feedforward) is preserved - only the overall level target moves.
    pub fn retarget_level(&mut self, target_rms: f64, feedforward_scale: f64) {
        if target_rms <= 0.0 || target_rms.is_nan() {
            return;
        }
        self.target_inband_rms = target_rms;
        let mut servo = AmplitudeServo::new(target_rms, self.cfg.servo_rate_db_per_s)
            .with_gains(0.0, 0.4)
            .with_drive_ceiling(self.cfg.servo_drive_ceiling)
            .with_fault_fraction(self.cfg.servo_fault_fraction);
        if feedforward_scale > 0.0 {
            servo.set_feedforward_scale(feedforward_scale);
            self.level_scale = feedforward_scale;
        }
        self.servo = servo;
        // RETARGETING RESETS DIVERGENCE TRACKING (root cause found
        // 2026-08-01 on the real chassis). The clamp streaks measure "pushing
        // with no progress at a STATIONARY target"; a level step moves the
        // target several dB at once, so every bin's next few corrections clamp
        // by DESIGN (max_correction_db_per_iter < the step) - that is
        // convergence, not divergence. Without this reset the streaks ride
        // through the ladder: the first hardware full-ladder run flagged
        // essentially EVERY in-band bin during each transition and aborted 5
        // loops after the final step, at full level, on a perfectly coherent
        // wire plant. A bin that is GENUINELY diverging re-earns its streak
        // within divergence_n loops at the new level and still aborts.
        for s in self.clamp_streak.iter_mut() {
            *s = 0;
        }
    }

    /// The current level in dB (`20*log10(drive_scale)`), surfaced as the level cmd.
    fn level_db(&self) -> f64 {
        20.0 * self.servo.drive_scale().log10()
    }

    /// Compute the untrusted-bin set for this measurement: a bin is
    /// untrusted if coherence < floor OR `|H| < h_floor_frac * max_inband|H|`.
    /// `|H|` is `sqrt(h_mag2)`; the peak is taken over the in-band window only.
    fn untrusted_mask(&self, inputs: &LoopInputs) -> Vec<bool> {
        let (lo, hi) = self.band;
        let hi = hi.min(inputs.h_mag2.len());
        let peak_h2 = inputs.h_mag2[lo..hi].iter().copied().fold(0.0f64, f64::max);
        let h_floor2 = (self.cfg.trust_h_floor_frac * self.cfg.trust_h_floor_frac) * peak_h2;
        let n = inputs.nbins();
        let mut mask = vec![false; n];
        for (k, m) in mask.iter_mut().enumerate().take(hi).skip(lo) {
            let low_coh = inputs.coherence[k] < self.cfg.trust_coherence_floor;
            let low_h = inputs.h_mag2[k] < h_floor2;
            *m = low_coh || low_h;
        }
        mask
    }

    /// Seed the drive shape from the Tikhonov-regularized plant inverse over the
    /// in-band window (Tikhonov owns the inversion softening).
    fn seed_shape(&mut self, inputs: &LoopInputs) {
        let (lo, hi) = self.band;
        let hi = hi.min(inputs.h_mag2.len());
        // Build a complex H with zero phase from |H|^2 (phase is irrelevant to the
        // magnitude-only SISO seed; h_inverse_regularized only uses |H|^2 in its
        // denominator, and for a real (re,0) input returns a real inverse).
        let h: Vec<(f64, f64)> = inputs.h_mag2[lo..hi]
            .iter()
            .map(|&h2| (h2.max(0.0).sqrt(), 0.0))
            .collect();
        // Epsilon: explicit lambda, else the suggest_epsilon (beta*Hmax)^2 policy on
        // the IN-BAND H only (out-of-band bins carry no drive so their |H| is noise-
        // dominated and would blow up a whole-spectrum epsilon - hertzforge-demo lesson).
        let eps = match self.cfg.lambda {
            Some(l) => l.max(1e-30),
            None => suggest_epsilon(&h, self.cfg.suggest_beta).max(1e-30),
        };
        let hinv = h_inverse_regularized(&h, eps);
        for s in self.shape.iter_mut() {
            *s = 0.0;
        }
        // shape[k] = target[k] * |Hinv[k]|^2  (=> response = |H|^2 * drive ~ target).
        for (i, k) in (lo..hi).enumerate() {
            let (re, im) = hinv[i];
            self.shape[k] = (self.target[k] * (re * re + im * im)).max(0.0);
        }
    }

    /// Mean-preserving per-bin shape trim. Updates only TRUSTED,
    /// non-notched in-band bins; renormalizes over the trusted+non-notched set to
    /// preserve its mean; clamps each step to `max_correction_db_per_iter`; tracks
    /// per-bin consecutive-clamp streaks for divergence.
    ///
    /// Returns `(untrusted, clamped, divergent)` bin index lists for the diagnostics.
    fn trim_shape(
        &mut self,
        inputs: &LoopInputs,
        untrusted: &[bool],
    ) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
        let (lo, hi) = self.band;
        let hi = hi.min(inputs.measured_psd.len());
        let step_max = self.cfg.max_correction_db_per_iter;
        // Multiplicative step limit: |10*log10(step)| <= max_db  =>  step in
        // [10^(-max/10), 10^(+max/10)].
        let ratio_hi = 10f64.powf(step_max / 10.0);
        let ratio_lo = 1.0 / ratio_hi;

        let mut untrusted_bins = Vec::new();
        let mut clamped_bins = Vec::new();
        let mut divergent_bins = Vec::new();

        // The renormalization SET is the trusted, non-notched in-band bins (the
        // notch exclusion contract). Capture its mean BEFORE trimming.
        let in_set = |k: usize| -> bool { !untrusted[k] && !inputs.is_notched(k) };
        let mut set_sum_before = 0.0;
        let mut set_count = 0usize;
        for (k, &s) in self.shape.iter().enumerate().take(hi).skip(lo) {
            if in_set(k) {
                set_sum_before += s;
                set_count += 1;
            }
        }

        // Index-based: this loop reads five parallel per-bin arrays (untrusted,
        // notched mask, measured_psd, target) and mutates two more (shape,
        // clamp_streak) at the same index k. A zipped-iterator rewrite would be less
        // readable than the shared index, so the range-loop lint is allowed here.
        #[allow(clippy::needless_range_loop)]
        for k in lo..hi {
            if untrusted[k] {
                untrusted_bins.push(k);
                // Held: no trim, streak resets (not clamping this iter).
                self.clamp_streak[k] = 0;
                continue;
            }
            if inputs.is_notched(k) {
                // Notch-held: drive shape unchanged here, excluded from renorm.
                self.clamp_streak[k] = 0;
                continue;
            }
            let meas = inputs.measured_psd[k].max(1e-30);
            // The SCHEDULED per-bin target: the stored line is full-level, the
            // measurement is at the rung - see `level_scale`.
            let tgt = (self.target[k] * self.level_scale * self.level_scale).max(1e-30);
            // Move the drive shape toward flattening the response error at the
            // DAMPED gain (see `HinvConfig::damping`), then clamp the
            // multiplicative step to the per-iter dB limit. `(tgt/meas)^g` is
            // the fractional-dB step: g of the observed error, per loop.
            let raw = (tgt / meas).powf(self.trim_gain);
            let step = raw.clamp(ratio_lo, ratio_hi);
            if step == ratio_hi || step == ratio_lo {
                clamped_bins.push(k);
                let dir: i32 = if step == ratio_hi { 1 } else { -1 };
                // Same direction extends the streak; a flip RESTARTS it at 1 in
                // the new direction - see the field doc for why.
                self.clamp_streak[k] = if self.clamp_streak[k].signum() == dir {
                    self.clamp_streak[k].saturating_add(dir)
                } else {
                    dir
                };
                if self.clamp_streak[k].unsigned_abs() >= self.cfg.divergence_n {
                    divergent_bins.push(k);
                    if std::env::var_os("HERTZFORGE_DIVERGE_DEBUG").is_some()
                        && divergent_bins.len() <= 6
                    {
                        eprintln!(
                            "[diverge-debug]   bin {k}: streak {} tgt {:.3e} meas {:.3e}                              err {:+.2} dB",
                            self.clamp_streak[k],
                            tgt,
                            meas,
                            10.0 * (tgt / meas).log10()
                        );
                    }
                }
            } else {
                self.clamp_streak[k] = 0;
            }
            self.shape[k] = (self.shape[k] * step).max(0.0);
        }

        // Renormalize the trusted+non-notched set to preserve its mean (shape-only;
        // level stays the servo's). A held/notched bin is NOT touched.
        if set_count > 0 {
            let mut set_sum_after = 0.0;
            for k in lo..hi {
                if in_set(k) {
                    set_sum_after += self.shape[k];
                }
            }
            if set_sum_after > 0.0 && set_sum_before > 0.0 {
                let renorm = set_sum_before / set_sum_after;
                for k in lo..hi {
                    if in_set(k) {
                        self.shape[k] *= renorm;
                    }
                }
            }
        }

        (untrusted_bins, clamped_bins, divergent_bins)
    }

    /// Build the drive PSD from the current shape and servo scale.
    fn drive_from_shape(&self) -> Vec<f64> {
        let scale2 = self.servo.drive_scale().powi(2);
        self.shape.iter().map(|&s| s * scale2).collect()
    }
}

impl ControlLaw for HinvLaw {
    fn init(&mut self, inputs: &LoopInputs) -> LoopOutputs {
        // A malformed grid cannot advance the loop; return a zero drive with an
        // error surfaced as an empty measurement (the caller validates upstream in
        // the service, but the law must not panic).
        if inputs.validate().is_err() {
            return LoopOutputs {
                drive_psd: vec![0.0; self.shape.len()],
                level_cmd_db: self.level_db(),
                diagnostics: LoopDiagnostics::default(),
            };
        }
        self.seed_shape(inputs);
        // Seed the servo at unit level (0 dB = full shaped target); the shape already
        // carries the H-inverse feedforward, so the level starts at the answer and
        // the servo only trims residual plant-estimate error (hertzforge-demo handoff).
        self.servo.set_feedforward_scale(1.0);
        self.initialized = true;
        LoopOutputs {
            drive_psd: self.drive_from_shape(),
            level_cmd_db: self.level_db(),
            diagnostics: LoopDiagnostics::default(),
        }
    }

    fn iterate(&mut self, inputs: &LoopInputs) -> LoopOutputs {
        if inputs.validate().is_err() {
            return LoopOutputs {
                drive_psd: self.drive_from_shape(),
                level_cmd_db: self.level_db(),
                diagnostics: LoopDiagnostics::default(),
            };
        }
        // Compute the measurement trust mask first: it gates BOTH the
        // shape trim AND the level servo's RMS. A sub-band whose coherence has
        // dropped out is UNTRUSTED, and an untrusted measurement must not drive the
        // loop - neither its shape nor the overall level. Excluding untrusted bins
        // from the servo RMS is what stops a mid-run coherence dropout on a band
        // from whipsawing the whole level (and then over-correcting on recovery);
        // the adversarial fixture (ii) gates exactly that.
        let untrusted = self.untrusted_mask(inputs);

        // Realized in-band RMS over the TRUSTED bins from the (post-notch)
        // measured PSD. Notched bins stay in the RMS because their
        // post-notch drive is real and realized; only UNTRUSTED bins are dropped.
        // If every in-band bin is untrusted (a globally dead measurement), fall back
        // to the whole-band RMS so the servo's own fault-hold can latch.
        let inband_rms_g = self.trusted_inband_rms(inputs, &untrusted);
        let inband_err_db = self.inband_err_db(inputs);

        // Level servo: dt is one loop; wall-clock loop period is the service's concern.
        // Use a unit dt so the rate clamp acts per-loop, matching hertzforge-demo's
        // per-window clamp.
        let dt = 1.0;
        self.servo.update(inband_rms_g, dt);
        let servo_faulted = self.servo.faulted();

        // Shape trim over trusted, non-notched bins.
        let (untrusted_bins, clamped_bins, divergent_bins) = self.trim_shape(inputs, &untrusted);

        LoopOutputs {
            drive_psd: self.drive_from_shape(),
            level_cmd_db: self.level_db(),
            diagnostics: LoopDiagnostics {
                untrusted_bins,
                clamped_bins,
                divergent_bins,
                inband_err_db,
                inband_rms_g: Some(inband_rms_g),
                servo_faulted,
            },
        }
    }

    fn name(&self) -> &str {
        "hinv"
    }
}

impl HinvLaw {
    /// The realized in-band RMS (g) over the TRUSTED bins of the measured PSD
    /// (untrusted measurements do not drive the level servo). Notched
    /// bins stay in the sum (their post-notch drive is realized). If every in-band
    /// bin is untrusted, fall back to the whole-band RMS so the servo's own
    /// dead-measurement fault-hold can latch instead of silently reading zero.
    fn trusted_inband_rms(&self, inputs: &LoopInputs, untrusted: &[bool]) -> f64 {
        let (lo, hi) = self.band;
        let hi = hi.min(inputs.measured_psd.len());
        if lo >= hi {
            return 0.0;
        }
        let mut sum = 0.0;
        let mut any_trusted = false;
        for k in lo..hi {
            if !untrusted.get(k).copied().unwrap_or(false) {
                sum += inputs.measured_psd[k];
                any_trusted = true;
            }
        }
        if !any_trusted {
            // Everything untrusted: whole-band RMS so the servo fault-hold triggers.
            return Self::inband_rms(inputs.measured_psd, self.band, inputs.df_hz);
        }
        (sum * inputs.df_hz).max(0.0).sqrt()
    }

    /// In-band RMS error (dB): the RMS over band bins of `10*log10(meas/target)`.
    fn inband_err_db(&self, inputs: &LoopInputs) -> Option<f64> {
        let (lo, hi) = self.band;
        let hi = hi.min(inputs.measured_psd.len());
        if lo >= hi {
            return None;
        }
        let mut sq = 0.0;
        let mut nb = 0usize;
        for k in lo..hi {
            let meas = inputs.measured_psd[k].max(1e-30);
            // Scheduled reference, same rule as the shape trim (`level_scale`).
            let tgt = (self.target[k] * self.level_scale * self.level_scale).max(1e-30);
            let e = 10.0 * (meas / tgt).log10();
            sq += e * e;
            nb += 1;
        }
        if nb == 0 {
            None
        } else {
            Some((sq / nb as f64).sqrt())
        }
    }
}

#[cfg(test)]
mod tests;
