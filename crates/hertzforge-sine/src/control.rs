//! Sine control core: the scalar per-tick control seam of the
//! closed-loop sine engine.
//!
//! Everything here is a PURE computation on per-tick scalars - no I/O, no
//! engine coupling. It transposes the hertzforge-random composite / notch-arbitration
//! patterns to the sine world, where exactly ONE frequency is excited at a
//! time, so every per-bin PSD rule becomes a scalar-at-current-f rule.
//!
//! # The tick pipeline (who calls what, in order)
//!
//! Per control tick (~10 ms nominal) the sine service runs:
//!
//! 1. [`composite_amplitude`] - fold the per-channel TRACKED amplitudes into
//!    the one control amplitude (weighted avg / min / max).
//! 2. The amplitude servo ([`hertzforge_dsp::sine::AmplitudeServo`]) produces the
//!    feedback step in dB (see the normalized-target convention below).
//! 3. [`BlendLaw::step`] adds the `w`-scaled feedforward SLOPE prediction from
//!    the sysID [`FeedforwardTable`] (delta form, BINDING) and clamps
//!    the total per-tick move.
//! 4. [`NotchGovernor`] caps the requested drive scale so no limit channel
//!    exceeds its bound (BINDING). While the ceiling binds, the
//!    caller MUST tick the servo with
//!    [`hertzforge_dsp::sine::AmplitudeServo::hold_tick`] - the scalar hold rule.
//! 5. [`DriveGuard::check`] gives the absolute drive-voltage verdict;
//!    [`tracked_velocity_ips_pk`] / [`tracked_displacement_in_pp`] are the
//!    analytic kinematic monitor helpers.
//!
//! # Normalized-target servo convention (BINDING)
//!
//! The service does NOT retarget the servo per frequency or per ladder rung.
//! It builds ONE `AmplitudeServo` with `target = 1.0` and feeds it
//!
//! ```text
//!   feed = measured_composite / (profile_target_accel(f) * rung_scale)
//! ```
//!
//! each tick. Frequency moves and rung moves are FEED changes, never servo
//! retargets, so the servo's fault threshold (`target * fault_fraction`)
//! stays semantically correct by construction across the whole profile.
//!
//! # Drive-unit reference (LOAD-BEARING)
//!
//! Throughout this module `|H|` means: RESPONSE ENGINEERING UNITS PER UNIT
//! NCO DRIVE COMMAND. "Unit NCO drive command" is a drive scale of `1.0`
//! applied to the unit-amplitude sine NCO
//! ([`hertzforge_dsp::sweep::SweepOscillator`] emits `sin` in `[-1, 1]`), i.e. the
//! full-scale drive waveform BEFORE any DAC volts mapping. The sysID pass
//! must therefore record the drive in NCO-command units (drive scale), not in
//! volts and not in plant EU, or every feedforward prediction in
//! [`FeedforwardTable::ff_db`] is silently offset by the unit mismatch.
//! `ff_db(f) = target_db(f) - H_db(f)` is then the predicted drive SCALE in
//! dB re 1.0 (unit NCO command) that puts the response on target at `f`.

use crate::profile::{displacement_in_pp_from_accel, velocity_ips_pk_from_accel};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Errors from constructing or feeding the sine control core. Library paths
/// return these instead of panicking (a running test must not be killable by
/// a panic in the control math). Local to hertzforge-sine by design: the
/// hertzforge-random `ControlError` is the same shape but is NOT imported (the
/// crates stay decoupled; only the pattern transposes).
#[derive(Debug, Clone, PartialEq)]
pub enum ControlError {
    /// A config knob was out of its valid range.
    BadConfig(String),
    /// A per-tick input had the wrong shape or a nonsensical value.
    BadInputs(String),
    /// A composite was requested over zero channels, with a channel/weight
    /// count mismatch, or with a non-positive weight sum.
    BadComposite(String),
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ControlError::BadConfig(s) => write!(f, "sine control config error: {s}"),
            ControlError::BadInputs(s) => write!(f, "sine control input error: {s}"),
            ControlError::BadComposite(s) => write!(f, "sine composite error: {s}"),
        }
    }
}

impl std::error::Error for ControlError {}

// =============================================================================
// Composite - scalar transpose of hertzforge-random composite_psd
// =============================================================================

/// How multiple control channels' tracked amplitudes combine into the single
/// composite amplitude the servo controls to. Weights are normalized to sum 1
/// before use, so they express RELATIVE emphasis, not absolute scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompositeMode {
    /// Weighted average: `sum_i w_i * a_i` (weights normalized). The standard
    /// multi-point average control.
    WeightedAvg,
    /// MIN of the weighted amplitudes: `min_i (w_i * a_i)`. Controls so the
    /// QUIETEST (weighted) point reaches target - keeps pushing until the
    /// weakest control point is satisfied.
    Min,
    /// MAX of the weighted amplitudes: `max_i (w_i * a_i)`. The protective
    /// mode - backs off as soon as any (weighted) control point hits target.
    Max,
}

/// Combine per-channel TRACKED amplitudes into one composite control
/// amplitude - the scalar transpose of hertzforge-random's `composite_psd` (same
/// normalization and weighted-min/max semantics, applied to scalars at the
/// current frequency instead of per-bin PSDs).
///
/// Weights are normalized so `sum w_i == 1` (empty inputs, a length mismatch,
/// or a non-positive/NaN weight sum are rejected). Min/Max operate on the
/// WEIGHTED values `w_i * a_i`, not the raw ones, so weighting still expresses
/// emphasis in those modes; WeightedAvg is the dot product with the normalized
/// weights.
pub fn composite_amplitude(
    amps: &[f64],
    weights: &[f64],
    mode: CompositeMode,
) -> Result<f64, ControlError> {
    if amps.is_empty() {
        return Err(ControlError::BadComposite("no control channels".into()));
    }
    if amps.len() != weights.len() {
        return Err(ControlError::BadComposite(format!(
            "{} amplitudes but {} weights",
            amps.len(),
            weights.len()
        )));
    }
    let wsum: f64 = weights.iter().sum();
    if wsum <= 0.0 || wsum.is_nan() {
        return Err(ControlError::BadComposite(format!(
            "weights must sum to a positive value (got {wsum})"
        )));
    }
    let out = match mode {
        CompositeMode::WeightedAvg => amps
            .iter()
            .zip(weights.iter())
            .map(|(&a, &w)| (w / wsum) * a)
            .sum(),
        CompositeMode::Min => amps
            .iter()
            .zip(weights.iter())
            .map(|(&a, &w)| (w / wsum) * a)
            .fold(f64::INFINITY, f64::min),
        CompositeMode::Max => amps
            .iter()
            .zip(weights.iter())
            .map(|(&a, &w)| (w / wsum) * a)
            .fold(f64::NEG_INFINITY, f64::max),
    };
    Ok(out)
}

// =============================================================================
// Feedforward table (sysID -> Hinv drive prediction, trust-gated)
// =============================================================================

/// Trust-mask knobs for [`FeedforwardTable`]. A sysID bin is TRUSTED only when
/// its coherence is at/above `coherence_floor` AND its `|H|` is at/above
/// `h_floor_frac` of the grid's peak `|H|` (mirrors hertzforge-random's trust gate).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FeedforwardConfig {
    /// Coherence floor, `[0, 1]`. Default 0.5.
    pub coherence_floor: f64,
    /// `|H|` floor as a fraction of the peak grid `|H|`, `[0, 1)`. Default 0.05.
    pub h_floor_frac: f64,
}

impl Default for FeedforwardConfig {
    fn default() -> Self {
        FeedforwardConfig {
            coherence_floor: 0.5,
            h_floor_frac: 0.05,
        }
    }
}

impl FeedforwardConfig {
    fn validate(&self) -> Result<(), ControlError> {
        if self.coherence_floor.is_nan() || !(0.0..=1.0).contains(&self.coherence_floor) {
            return Err(ControlError::BadConfig(format!(
                "coherence_floor must be in [0,1] (got {})",
                self.coherence_floor
            )));
        }
        if self.h_floor_frac.is_nan() || !(0.0..1.0).contains(&self.h_floor_frac) {
            return Err(ControlError::BadConfig(format!(
                "h_floor_frac must be in [0,1) (got {})",
                self.h_floor_frac
            )));
        }
        Ok(())
    }
}

/// Hinv feedforward table built from a sysID pass: a frequency grid, the
/// measured `|H|` per bin, and a per-bin trust mask.
///
/// # Unit reference (LOAD-BEARING)
///
/// `h_mag[k]` MUST be in response engineering units PER UNIT NCO DRIVE
/// COMMAND (drive scale 1.0 on the unit-amplitude NCO) - see the module docs.
/// The target must be in the SAME response EU (for the sine service: the
/// profile's acceleration target in g pk, with the sysID `|H|` in
/// g-pk-per-unit-drive). [`FeedforwardTable::ff_db`] then predicts the drive
/// SCALE in dB re 1.0 that puts the response on target.
///
/// # Trust mask
///
/// A bin is trusted when `coherence >= coherence_floor` AND
/// `|H| >= h_floor_frac * max|H|` (grid peak) AND the target at that bin
/// exists and is positive. Queries at untrusted bins - or interpolations
/// whose EITHER bracketing bin is untrusted - return `None`, and the caller's
/// [`BlendLaw`] treats a `None` as "prediction term 0" (benign boundary).
#[derive(Debug, Clone, PartialEq)]
pub struct FeedforwardTable {
    freqs: Vec<f64>,
    /// `target_db(f_k) - H_db(f_k)` at trusted bins; 0.0 (never read) at
    /// untrusted ones.
    ff_db: Vec<f64>,
    trusted: Vec<bool>,
}

impl FeedforwardTable {
    /// Build from a sysID pass with the target supplied as a closure (e.g.
    /// `|f| profile.target_accel_g_pk(f)` scaled to the run level). The
    /// closure returning `None` (out of profile band) or a non-positive value
    /// marks the bin untrusted.
    ///
    /// `freqs` must be strictly increasing and positive; all slices must have
    /// equal, nonzero length.
    pub fn from_sysid<F>(
        freqs: &[f64],
        h_mag: &[f64],
        coherence: &[f64],
        target_amp: F,
        cfg: FeedforwardConfig,
    ) -> Result<Self, ControlError>
    where
        F: Fn(f64) -> Option<f64>,
    {
        cfg.validate()?;
        let n = freqs.len();
        if n == 0 {
            return Err(ControlError::BadInputs("empty sysID grid".into()));
        }
        if h_mag.len() != n || coherence.len() != n {
            return Err(ControlError::BadInputs(format!(
                "ragged sysID grid: freqs {n}, h_mag {}, coherence {}",
                h_mag.len(),
                coherence.len()
            )));
        }
        for (i, w) in freqs.windows(2).enumerate() {
            if w[1] <= w[0] || w[1].is_nan() {
                return Err(ControlError::BadInputs(format!(
                    "freqs must be strictly increasing (freqs[{}]={} vs freqs[{}]={})",
                    i,
                    w[0],
                    i + 1,
                    w[1]
                )));
            }
        }
        if freqs[0] <= 0.0 || freqs[0].is_nan() {
            return Err(ControlError::BadInputs(format!(
                "freqs must be positive (freqs[0]={})",
                freqs[0]
            )));
        }
        // Grid peak |H| over finite positive bins (NaN/non-positive lose).
        let h_max = h_mag
            .iter()
            .copied()
            .filter(|h| h.is_finite() && *h > 0.0)
            .fold(0.0f64, f64::max);
        let h_floor = cfg.h_floor_frac * h_max;

        let mut ff = vec![0.0f64; n];
        let mut trusted = vec![false; n];
        for k in 0..n {
            let h = h_mag[k];
            if h <= 0.0 || h.is_nan() || !h.is_finite() {
                continue;
            }
            if coherence[k].is_nan() || coherence[k] < cfg.coherence_floor {
                continue;
            }
            if h < h_floor {
                continue;
            }
            let Some(t) = target_amp(freqs[k]) else {
                continue;
            };
            if t <= 0.0 || t.is_nan() || !t.is_finite() {
                continue;
            }
            ff[k] = 20.0 * (t / h).log10();
            trusted[k] = true;
        }
        Ok(FeedforwardTable {
            freqs: freqs.to_vec(),
            ff_db: ff,
            trusted,
        })
    }

    /// Build from a sysID pass with a precomputed per-bin target array (same
    /// grid as `freqs`; a non-positive / NaN target marks the bin untrusted).
    pub fn from_sysid_targets(
        freqs: &[f64],
        h_mag: &[f64],
        coherence: &[f64],
        target_amps: &[f64],
        cfg: FeedforwardConfig,
    ) -> Result<Self, ControlError> {
        if target_amps.len() != freqs.len() {
            return Err(ControlError::BadInputs(format!(
                "target grid length {} != freq grid {}",
                target_amps.len(),
                freqs.len()
            )));
        }
        // Positional lookup: freqs is validated strictly increasing inside
        // from_sysid, so an exact binary search per closure call is safe; a
        // simple position scan keeps this correct even before that validation
        // runs (the closure is only called after validation passes).
        Self::from_sysid(
            freqs,
            h_mag,
            coherence,
            |f| {
                freqs
                    .iter()
                    .position(|&g| g == f)
                    .map(|i| target_amps[i])
                    .filter(|t| *t > 0.0 && !t.is_nan())
            },
            cfg,
        )
    }

    /// The predicted drive level in dB re unit NCO drive command at `f_hz`:
    /// `target_db(f) - H_db(f)`, linearly interpolated in `ln f` between grid
    /// bins. Returns `None` out of the grid span, at an untrusted bin, or when
    /// EITHER bracketing bin of an interpolation is untrusted (the trust
    /// boundary is treated as unknown, which the [`BlendLaw`] makes benign).
    pub fn ff_db(&self, f_hz: f64) -> Option<f64> {
        if f_hz.is_nan() {
            return None;
        }
        let n = self.freqs.len();
        if f_hz < self.freqs[0] || f_hz > self.freqs[n - 1] {
            return None;
        }
        // First index with freqs[i] > f_hz; f lies in [freqs[i-1], freqs[i]).
        let i = self.freqs.partition_point(|&g| g <= f_hz);
        let lo = i - 1; // i >= 1 because f >= freqs[0]
        if self.freqs[lo] == f_hz {
            return self.trusted[lo].then_some(self.ff_db[lo]);
        }
        let hi = i; // in range: f < freqs[n-1] here (== case took the branch above)
        if !self.trusted[lo] || !self.trusted[hi] {
            return None;
        }
        let (fl, fh) = (self.freqs[lo], self.freqs[hi]);
        let t = (f_hz.ln() - fl.ln()) / (fh.ln() - fl.ln());
        Some(self.ff_db[lo] + t * (self.ff_db[hi] - self.ff_db[lo]))
    }

    /// Shift every trusted bin's prediction by `db` - the NEGATIVE CONTROL for a
    /// closed-loop accuracy assertion.
    ///
    /// A flat unity plant (the jumpered AO->AI bench) makes the feedforward
    /// prediction and the servo's converged answer the SAME number, so a level
    /// row on that plant passes whether or not the servo is closing anything. It
    /// is a result that cannot fail. Offsetting the table by a
    /// known error makes the two disagree by exactly that much: the row then
    /// passes only if the FEEDBACK path walked the drive back onto target.
    ///
    /// Deliberately not `pub(crate)`: the chassis gate lives in
    /// `hertzforge-station/examples`, outside this crate. It is an instrument, never a
    /// run-time control - nothing in the service calls it.
    pub fn offset_db(&mut self, db: f64) {
        if !db.is_finite() {
            return;
        }
        for (v, t) in self.ff_db.iter_mut().zip(self.trusted.iter()) {
            if *t {
                *v += db;
            }
        }
    }

    /// The grid frequencies (for display / tests).
    pub fn freqs(&self) -> &[f64] {
        &self.freqs
    }

    /// The per-bin trust mask (for display / tests).
    pub fn trusted(&self) -> &[bool] {
        &self.trusted
    }
}

// =============================================================================
// Blend law (DELTA FORM, BINDING)
// =============================================================================

/// Knobs for [`BlendLaw`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BlendConfig {
    /// Prediction weight `w` in `[0, 1]` (the operator knob). `0` = pure
    /// feedback; `1` = full Hinv slope prediction PLUS the servo (the servo is
    /// never bypassed - see [`BlendLaw`]). Default 0.5.
    pub w: f64,
    /// Clamp on the TOTAL per-tick drive move (prediction + servo), dB.
    /// Default 1.0 dB/tick (at a 10 ms tick this allows up to 100 dB/s of
    /// combined move; the servo's own `max_rate_db_per_s` is the sustained
    /// limiter, this clamp bounds single-tick transients).
    pub max_step_db_per_tick: f64,
}

impl Default for BlendConfig {
    fn default() -> Self {
        BlendConfig {
            w: 0.5,
            max_step_db_per_tick: 1.0,
        }
    }
}

impl BlendConfig {
    fn validate(&self) -> Result<(), ControlError> {
        if self.w.is_nan() || !(0.0..=1.0).contains(&self.w) {
            return Err(ControlError::BadConfig(format!(
                "blend w must be in [0,1] (got {})",
                self.w
            )));
        }
        if self.max_step_db_per_tick <= 0.0 || self.max_step_db_per_tick.is_nan() {
            return Err(ControlError::BadConfig(format!(
                "max_step_db_per_tick must be > 0 (got {})",
                self.max_step_db_per_tick
            )));
        }
        Ok(())
    }
}

/// The delta-form feedforward/feedback blend (BINDING).
///
/// Per control tick the drive moves by
///
/// ```text
///   drive_db(f_k) = drive_db(f_{k-1})
///                 + w * [ ff_db(f_k) - ff_db(f_{k-1}) ]   (prediction term)
///                 + servo_step_db                          (feedback term)
/// ```
///
/// with the TOTAL clamped to `+- max_step_db_per_tick`. [`BlendLaw::step`]
/// returns that clamped delta; the caller owns `drive_db` itself.
///
/// Properties the delta form buys (why the original absolute-mix law was
/// reshaped into this):
///
/// - **The servo ALWAYS runs** - `w` scales only the Hinv slope prediction
///   along the sweep, never the feedback. `w = 1` is still closed-loop.
/// - **Dwell at `w = 1` still tracks plant drift**: with `f` fixed the
///   prediction delta is 0 and the whole move is `servo_step_db`.
/// - **Trust boundaries are benign**: where [`FeedforwardTable::ff_db`] is
///   `None` at EITHER end of the tick, the prediction term is 0 (no step is
///   emitted at the boundary). After an untrusted gap the previous-ff anchor
///   is re-seeded from the first trusted query WITHOUT emitting a jump, so
///   re-entering the trusted region costs nothing.
#[derive(Debug, Clone)]
pub struct BlendLaw {
    cfg: BlendConfig,
    /// `ff_db` at the previous tick's frequency, `None` before the first
    /// trusted query and across untrusted gaps.
    ff_prev_db: Option<f64>,
}

impl BlendLaw {
    /// Build a blend law; validates the config.
    pub fn new(cfg: BlendConfig) -> Result<Self, ControlError> {
        cfg.validate()?;
        Ok(BlendLaw {
            cfg,
            ff_prev_db: None,
        })
    }

    /// The current prediction weight.
    pub fn w(&self) -> f64 {
        self.cfg.w
    }

    /// Move the operator knob `w` (validated to `[0, 1]`). Changing `w`
    /// mid-run is continuous by construction: only future slope predictions
    /// are scaled; no absolute-drive re-mix happens.
    pub fn set_w(&mut self, w: f64) -> Result<(), ControlError> {
        if w.is_nan() || !(0.0..=1.0).contains(&w) {
            return Err(ControlError::BadConfig(format!(
                "blend w must be in [0,1] (got {w})"
            )));
        }
        self.cfg.w = w;
        Ok(())
    }

    /// Drop the previous-ff anchor (e.g. at a test restart). The next call to
    /// [`BlendLaw::step`] emits a prediction term of 0 and re-anchors.
    pub fn reset(&mut self) {
        self.ff_prev_db = None;
    }

    /// One control tick: returns the TOTAL drive delta in dB (prediction +
    /// servo), clamped to `+- max_step_db_per_tick`. `servo_step_db` is the
    /// feedback move the amplitude servo produced this tick (the caller
    /// derives it from the servo's before/after drive scale, or 0 while the
    /// servo is held).
    pub fn step(&mut self, f_now_hz: f64, servo_step_db: f64, ff: &FeedforwardTable) -> f64 {
        let ff_now = ff.ff_db(f_now_hz);
        let prediction = match (self.ff_prev_db, ff_now) {
            (Some(prev), Some(now)) => self.cfg.w * (now - prev),
            // Either end untrusted / out of grid: benign - prediction 0.
            _ => 0.0,
        };
        // Re-anchor (or clear) for the next tick. Coming OUT of an untrusted
        // gap this sets the anchor without having emitted a jump this tick.
        self.ff_prev_db = ff_now;
        let servo = if servo_step_db.is_nan() {
            0.0
        } else {
            servo_step_db
        };
        (prediction + servo).clamp(
            -self.cfg.max_step_db_per_tick,
            self.cfg.max_step_db_per_tick,
        )
    }
}

// =============================================================================
// Notch governor (BINDING)
// =============================================================================

/// Knobs for [`NotchGovernor`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NotchGovernorConfig {
    /// Drive floor as a fraction of full drive scale (default 0.01 = 1%).
    /// At/below this drive the measured ratio `tracked_amp / drive` is too
    /// noise-dominated to trust; the sysID `|H|` fallback is used instead.
    pub drive_floor: f64,
    /// One-pole smoothing time constant for the per-channel sensitivity, in
    /// control ticks (default 3.0). Smoothing keeps a single noisy amplitude
    /// read from slamming the ceiling around.
    pub smoothing_ticks: f64,
    /// The release ramp rate the CALLER must honor when the ceiling stops
    /// binding, dB/s. Default 20.0 = the Slow compression preset rate; using
    /// the servo's own `max_rate_db_per_s` as the resume limiter satisfies
    /// this automatically.
    pub release_ramp_db_per_s: f64,
}

impl Default for NotchGovernorConfig {
    fn default() -> Self {
        NotchGovernorConfig {
            drive_floor: 0.01,
            smoothing_ticks: 3.0,
            release_ramp_db_per_s: 20.0,
        }
    }
}

impl NotchGovernorConfig {
    fn validate(&self) -> Result<(), ControlError> {
        if self.drive_floor <= 0.0 || self.drive_floor.is_nan() {
            return Err(ControlError::BadConfig(format!(
                "drive_floor must be > 0 (got {})",
                self.drive_floor
            )));
        }
        if self.smoothing_ticks <= 0.0 || self.smoothing_ticks.is_nan() {
            return Err(ControlError::BadConfig(format!(
                "smoothing_ticks must be > 0 (got {})",
                self.smoothing_ticks
            )));
        }
        if self.release_ramp_db_per_s <= 0.0 || self.release_ramp_db_per_s.is_nan() {
            return Err(ControlError::BadConfig(format!(
                "release_ramp_db_per_s must be > 0 (got {})",
                self.release_ramp_db_per_s
            )));
        }
        Ok(())
    }
}

/// One limit channel's per-tick observation for [`NotchGovernor::observe`].
///
/// The governor stores NO limit profile: the caller is the bound provider and
/// evaluates its limit profile at the current frequency every tick
/// (`limit_amp_at_f`), which is what lets N channels share one profile or
/// carry their own without the governor knowing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimitChannelTick {
    /// Tracking-filter amplitude on this limit channel this tick (response EU).
    pub tracked_amp: f64,
    /// The channel's limit bound at the current frequency (same response EU).
    /// Non-finite or non-positive = no limit on this channel this tick.
    pub limit_amp_at_f: f64,
    /// SysID `|H|` for this channel at the current frequency (response EU per
    /// unit NCO drive command - the module-level unit reference), used as the
    /// sensitivity fallback below the drive floor. `None` = no fallback
    /// available; the channel is skipped this tick when the drive is below
    /// the floor.
    pub sysid_h_mag: Option<f64>,
}

/// The scalar notch limiter (BINDING): a per-tick ceiling on
/// the drive scale so that no limit channel exceeds its bound at the current
/// frequency. Limit-down ALWAYS beats control-up (the hertzforge-random notch
/// arbitration, transposed to scalars).
///
/// # Per-tick logic
///
/// For each limit channel: `sensitivity = tracked_amp / drive` when
/// `drive > drive_floor`, else the sysID `|H|` fallback (`None` fallback =
/// the channel's sensitivity is NOT updated this tick). The sensitivity is
/// one-pole smoothed over `smoothing_ticks`; the channel's allowed scale is
/// `limit_amp_at_f / sensitivity`; the governor ceiling is the MIN over
/// channels; [`NotchGovernor::apply`] clamps the requested scale to it.
///
/// A channel with no fresh sensitivity sample this tick KEEPS contributing
/// its last smoothed sensitivity (holding the ceiling steady) rather than
/// dropping out - dropping out would lift the ceiling to infinity for a tick
/// and let the drive punch through the limit.
///
/// # The caller contract while binding (BINDING)
///
/// While [`NotchGovernor::binding`] is true, the caller MUST tick the
/// amplitude servo with [`hertzforge_dsp::sine::AmplitudeServo::hold_tick`], NOT
/// `update` - the servo's error is unrealizable while the ceiling holds the
/// drive down, and integrating it would wind the level up against the clamp
/// (the classic post-release overshoot). On release, the caller ramps back
/// up at the compression rate - [`NotchGovernorConfig::release_ramp_db_per_s`]
/// (default = the Slow compression rate); the servo's own
/// `max_rate_db_per_s` provides exactly this when the resume path simply
/// returns to `update`.
///
/// # Anti-resonance direction is SAFE by construction - do NOT "fix" it
///
/// When a limit channel sits at an anti-resonance its sensitivity tends to 0,
/// so its allowed scale `limit/sensitivity` tends to INFINITY and no notch
/// engages. That is CORRECT: a channel the drive cannot move cannot breach
/// its bound through this drive, so it must not constrain the drive. A
/// well-meaning "guard against divide-by-small" that floors the allowed
/// scale would introduce phantom notching at anti-resonances. (Sensitivity
/// `<= 0` skips the channel's ceiling contribution entirely - infinity, not
/// NaN.)
#[derive(Debug, Clone)]
pub struct NotchGovernor {
    cfg: NotchGovernorConfig,
    /// Per-channel one-pole smoothed sensitivity; `None` until the first
    /// valid sample (seeded directly, not smoothed up from zero - smoothing
    /// up from zero would understate sensitivity and briefly allow a
    /// limit-violating drive).
    smoothed_sens: Vec<Option<f64>>,
    /// Current ceiling on the drive scale (min over channels), `+inf` = none.
    ceiling: f64,
    /// Whether the most recent [`NotchGovernor::apply`] clamped the request.
    binding: bool,
}

impl NotchGovernor {
    /// Build a governor over `n_channels` limit channels.
    pub fn new(cfg: NotchGovernorConfig, n_channels: usize) -> Result<Self, ControlError> {
        cfg.validate()?;
        if n_channels == 0 {
            return Err(ControlError::BadConfig(
                "notch governor needs at least one limit channel".into(),
            ));
        }
        Ok(NotchGovernor {
            cfg,
            smoothed_sens: vec![None; n_channels],
            ceiling: f64::INFINITY,
            binding: false,
        })
    }

    /// Feed this tick's observations: the drive scale that was APPLIED for
    /// the interval the amplitudes were tracked over, plus one
    /// [`LimitChannelTick`] per channel (same order/length as construction).
    /// Updates the smoothed sensitivities and recomputes the ceiling.
    pub fn observe(
        &mut self,
        current_drive_scale: f64,
        channels: &[LimitChannelTick],
    ) -> Result<(), ControlError> {
        if channels.len() != self.smoothed_sens.len() {
            return Err(ControlError::BadInputs(format!(
                "{} channel ticks for a {}-channel governor",
                channels.len(),
                self.smoothed_sens.len()
            )));
        }
        let alpha = 1.0 - (-1.0 / self.cfg.smoothing_ticks).exp();
        let mut ceiling = f64::INFINITY;
        for (ch, sens_slot) in channels.iter().zip(self.smoothed_sens.iter_mut()) {
            // Fresh sensitivity sample: measured ratio above the drive floor,
            // sysID fallback below it. No sample -> no update.
            let sample = if current_drive_scale > self.cfg.drive_floor {
                let s = ch.tracked_amp / current_drive_scale;
                if s.is_finite() && s >= 0.0 {
                    Some(s)
                } else {
                    None
                }
            } else {
                ch.sysid_h_mag.filter(|h| h.is_finite() && *h >= 0.0)
            };
            if let Some(x) = sample {
                *sens_slot = Some(match *sens_slot {
                    Some(prev) => prev + alpha * (x - prev),
                    None => x, // seed directly (see field docs)
                });
            }
            // Ceiling contribution from the (possibly held-over) smoothed
            // sensitivity. Sensitivity <= 0 => anti-resonance => no notch
            // (SAFE direction - see type docs). Non-positive/non-finite
            // limit => no limit on this channel.
            if let Some(s) = *sens_slot {
                let lim = ch.limit_amp_at_f;
                if s > 0.0 && lim.is_finite() && lim > 0.0 {
                    ceiling = ceiling.min(lim / s);
                }
            }
        }
        self.ceiling = ceiling;
        Ok(())
    }

    /// Clamp the requested drive scale to the governor ceiling. Returns
    /// `(applied_scale, binding)`; `binding` is true when the ceiling cut the
    /// request (also latched into [`NotchGovernor::binding`] until the next
    /// call). A NaN request is treated as a request for zero drive (safe).
    pub fn apply(&mut self, requested_scale: f64) -> (f64, bool) {
        let req = if requested_scale.is_nan() {
            0.0
        } else {
            requested_scale
        };
        if req > self.ceiling {
            self.binding = true;
            (self.ceiling, true)
        } else {
            self.binding = false;
            (req, false)
        }
    }

    /// Whether the most recent [`NotchGovernor::apply`] clamped its request.
    /// While true, the caller MUST use `hold_tick` on the servo (see type
    /// docs).
    pub fn binding(&self) -> bool {
        self.binding
    }

    /// The current ceiling on the drive scale (`+inf` = no limit).
    pub fn ceiling(&self) -> f64 {
        self.ceiling
    }

    /// The release ramp the caller must honor when the ceiling stops binding.
    pub fn release_ramp_db_per_s(&self) -> f64 {
        self.cfg.release_ramp_db_per_s
    }

    /// The smoothed sensitivity for channel `ch` (for tests / UI display).
    pub fn smoothed_sensitivity(&self, ch: usize) -> Option<f64> {
        self.smoothed_sens.get(ch).copied().flatten()
    }

    /// Clear all smoothing state and the ceiling (e.g. at a new run).
    pub fn reset(&mut self) {
        for s in self.smoothed_sens.iter_mut() {
            *s = None;
        }
        self.ceiling = f64::INFINITY;
        self.binding = false;
    }
}

// =============================================================================
// Drive guard + analytic kinematic monitor helpers
// =============================================================================

/// Verdict from [`DriveGuard::check`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DriveVerdict {
    /// Drive within the ceiling.
    Ok,
    /// Drive above the ceiling but below the abort trip: apply the carried
    /// clamped value instead.
    Clamped(f64),
    /// Drive at/above the abort trip (or non-finite): the service must abort.
    Abort,
}

/// Absolute drive-voltage guard: a hard ceiling and an abort trip
/// on the commanded drive, in the same unit the caller commands the DAC with
/// (volts peak for a hardware backend, drive scale for Sim - the guard only
/// compares numbers; the caller owns the unit and must pass both bounds and
/// the checked value in it).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DriveGuard {
    /// Clamp ceiling: a drive above this is clamped to it.
    pub vpk_ceiling: f64,
    /// Abort trip: a drive at/above this aborts the run. Must be `>=`
    /// `vpk_ceiling`.
    pub vpk_abort: f64,
}

impl DriveGuard {
    /// Build a guard; `vpk_ceiling` must be positive and `vpk_abort >=
    /// vpk_ceiling`.
    pub fn new(vpk_ceiling: f64, vpk_abort: f64) -> Result<Self, ControlError> {
        if vpk_ceiling <= 0.0 || vpk_ceiling.is_nan() {
            return Err(ControlError::BadConfig(format!(
                "vpk_ceiling must be > 0 (got {vpk_ceiling})"
            )));
        }
        if vpk_abort < vpk_ceiling || vpk_abort.is_nan() {
            return Err(ControlError::BadConfig(format!(
                "vpk_abort must be >= vpk_ceiling (got {vpk_abort} < {vpk_ceiling})"
            )));
        }
        Ok(DriveGuard {
            vpk_ceiling,
            vpk_abort,
        })
    }

    /// Check a commanded drive. NaN / non-finite drives return
    /// [`DriveVerdict::Abort`] (a non-finite command is a control-math fault,
    /// not something to clamp and keep running on).
    pub fn check(&self, drive_vpk: f64) -> DriveVerdict {
        if drive_vpk.is_nan() || !drive_vpk.is_finite() {
            return DriveVerdict::Abort;
        }
        if drive_vpk >= self.vpk_abort {
            DriveVerdict::Abort
        } else if drive_vpk > self.vpk_ceiling {
            DriveVerdict::Clamped(self.vpk_ceiling)
        } else {
            DriveVerdict::Ok
        }
    }
}

/// Analytic peak velocity (`in/s`) of the TRACKED response at `f_hz`: exact
/// for a sine, `v = a * G / (2 pi f)`. Thin wrapper over
/// [`crate::profile::velocity_ips_pk_from_accel`] so the kinematic monitor
/// and the profile evaluation cannot drift apart.
pub fn tracked_velocity_ips_pk(amp_g_pk: f64, f_hz: f64) -> f64 {
    velocity_ips_pk_from_accel(amp_g_pk, f_hz)
}

/// Analytic peak-to-peak displacement (`in`) of the TRACKED response at
/// `f_hz`: exact for a sine, `d_pp = 2 * a * G / (2 pi f)^2`.
/// Thin wrapper over [`crate::profile::displacement_in_pp_from_accel`].
pub fn tracked_displacement_in_pp(amp_g_pk: f64, f_hz: f64) -> f64 {
    displacement_in_pp_from_accel(amp_g_pk, f_hz)
}

// =============================================================================
// Compression presets ((ki, max_rate) PAIRS, BINDING)
// =============================================================================

/// Compression-speed presets for the amplitude servo. These are pinned as
/// (ki, max_rate) PAIRS, not max_rate alone: the accuracy bar (+-1 dB
/// through Q=50 at 1 oct/min) fails on the default `ki = 2.0` with a raised
/// rate alone (~1.7 dB of servo lag on pure feedback), so Fast raises BOTH.
///
/// Usage with the normalized-target convention:
///
/// ```ignore
/// let (ki, rate) = preset.gains();
/// let servo = AmplitudeServo::new(1.0, rate).with_gains(0.3, ki);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum CompressionPreset {
    /// `(ki, max_rate) = (2.0, 20 dB/s)`.
    Slow,
    /// `(ki, max_rate) = (6.0, 60 dB/s)`.
    Fast,
    /// Explicit pair for expert tuning.
    Custom { ki: f64, max_rate_db_per_s: f64 },
}

impl CompressionPreset {
    /// The `(ki, max_rate_db_per_s)` pair for this preset.
    pub fn gains(&self) -> (f64, f64) {
        match *self {
            CompressionPreset::Slow => (2.0, 20.0),
            CompressionPreset::Fast => (6.0, 60.0),
            CompressionPreset::Custom {
                ki,
                max_rate_db_per_s,
            } => (ki, max_rate_db_per_s),
        }
    }
}
