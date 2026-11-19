//! [`SrsSynthesisLaw`] - the run-time counterpart of the offline SRS-matching
//! iteration, implementing the [`hertzforge_shock_common::ShockControlLaw`] seam, WITH the
//! predictive-limit cap wired in.
//!
//! # Offline iteration vs run-time law
//!
//! [`crate::iterate::synthesize_to_target`] is the OFFLINE loop: it synthesizes,
//! scores its OWN synthesized waveform with [`hertzforge_dsp::srs::srs_analyze`], and
//! re-synthesizes until the modeled SRS matches. The run-time law does the SAME
//! per-component nearest-band ratio update, but from the MEASURED control-channel SRS
//! of a just-fired pulse (fire -> capture -> SRS -> update), so the drive converges on
//! the real plant, not the model. This is the event-driven counterpart the
//! [`hertzforge_shock_common::ShockControlLaw`] trait exists for.
//!
//! # Relationship to [`hertzforge_shock_common::PerBandGainLaw`]
//!
//! `PerBandGainLaw` is the generic per-band gain law shipped in the shared crate.
//! `SrsSynthesisLaw` is the SRS-synthesis-specific law: it applies the identical
//! nearest-band `target/measured` ratio (clamped per iteration), and its distinct
//! identity is that it is the run-time face of THIS crate's synthesis engines - a
//! service pairs it with a WAVSYN or SEDS component table and re-synthesizes the drive
//! from the returned per-component gains. It carries its component frequencies at
//! construction so the law is self-contained (it does not depend on the caller passing
//! the same `component_freqs` every pulse, though it honors them if the grids match).
//!
//! # Limiting (wired): [`ShockLawInputs::predicted_limit_srs`]
//!
//! [`ShockLawInputs::predicted_limit_srs`] carries each limit channel's PREDICTED
//! response SRS (via the reduced-level FRFs - Skousen/Coleman/Smallwood,
//! SAND2016-1335C; computed by the service, e.g. through
//! [`crate::limit::predict_response_srs`]) for the drive as of the just-fired pulse.
//! A law built with [`SrsSynthesisLaw::with_limits`] folds the predictive-notch cap
//! into every per-component gain update:
//!
//! - Component `j`'s cap is `min` over every limit channel `c` and every band `b`
//!   within [`LIMIT_CAP_WINDOW_OCTAVES`] of the component frequency of
//!   `allowed[c][b] / predicted[c][b]` (with
//!   `allowed = bound * 10^(-headroom_db/20)`): the gain may rise only as far as the
//!   prediction says the tightest limit channel IN THE COMPONENT'S SPECTRAL REACH
//!   can absorb. (Nearest-band-only capping lets an off-band component pump a
//!   binding band back up - the window is the fix; see the constant's docs.)
//! - When a prediction ALREADY exceeds its allowed bound, that cap is `< 1` and it is
//!   NOT rate-clamped: limit-down beats control-up and beats the per-iteration dB
//!   clamp (the Random arbitration rule; safety is monotone, rate limiting applies to
//!   control moves only).
//! - A cap `> 1` (headroom available) permits an up-move, DAMPED by
//!   [`LIMIT_UP_MOVE_DAMPING`] so simultaneous multi-component rises converge to the
//!   bound from below instead of collectively overshooting it.
//! - Bounds configured but NO prediction supplied on a pulse: the law falls back to
//!   safety-monotone HOLDS - no component gain may exceed `1.0` (nothing may be
//!   driven up without a fresh prediction saying it is safe). Documented fallback,
//!   not an error: the ladder still owns the overall level.
//!
//! A law built WITHOUT limits ([`SrsSynthesisLaw::new`] / `with_clamp`) has no bounds
//! to compare a prediction against and IGNORES `predicted_limit_srs`, exactly like
//! [`hertzforge_shock_common::PerBandGainLaw`] does.

use hertzforge_shock_common::control_law::ShockLawDiagnostics;
use hertzforge_shock_common::{ControlLawError, ShockControlLaw, ShockLawInputs, ShockLawOutputs};
use serde::{Deserialize, Serialize};

/// The built-in law's arithmetic, `srs_ratio`: the per-component ratio update
/// without the limit cap. The run's core calls it through
/// [`crate::control::SrsRatioFactory`]; [`SrsSynthesisLaw`] wraps the same
/// arithmetic with the cap.
pub use hertzforge_shock_law_math::srs as ratio;

/// The default per-iteration per-component gain clamp, dB (matches
/// [`hertzforge_shock_common::DEFAULT_MAX_GAIN_DB_PER_ITER`]).
pub const DEFAULT_MAX_GAIN_DB_PER_ITER: f64 = 3.0;

/// Half-width (octaves) of the band WINDOW a component's limit cap is taken over.
///
/// A synthesis component's spectral support spans several analysis bands (the
/// grids are distinct), so capping a component by its NEAREST band
/// alone lets a component just off a binding band pump energy INTO that band and
/// defeat the limit (observed as a growing limit cycle in the closed-loop
/// fixture). The cap is therefore the MINIMUM headroom ratio over all bands within
/// `+/- LIMIT_CAP_WINDOW_OCTAVES` of the component frequency - the run-time,
/// per-pulse conservative counterpart of the offline drive-spectrum notch + LS
/// re-fit in [`crate::limit`], which resolves the same coupling exactly. Half an
/// octave comfortably covers the spectral half-width of the synthesis components
/// (a WAVSYN `N = 5` wavelet spans roughly `+/- f/5`, about 0.26 octave).
pub const LIMIT_CAP_WINDOW_OCTAVES: f64 = 0.5;

/// Extra depth (dB) applied to a BINDING down-cap (`cap < 1`), mirroring
/// [`crate::limit::OVER_NOTCH_DB`]: the exact `allowed/predicted` ratio only reaches
/// the bound asymptotically while components just outside the window keep pumping
/// the band, so an exact-ratio loop hovers slightly OVER the bound. A small fixed
/// over-cut makes the down-moves win and the equilibrium settle UNDER the bound.
pub const LIMIT_DOWN_OVERCUT_DB: f64 = 0.5;

/// Damping exponent applied to prediction-capped UP-moves: an up-move is capped at
/// `headroom_ratio.powf(LIMIT_UP_MOVE_DAMPING)` instead of the full ratio.
///
/// When several coupled components all rise by their FULL predicted headroom in the
/// same pulse, each band's realized response rises by MORE than any single
/// component's factor (the neighbors add), overshooting the bound and setting up a
/// down/up limit cycle. Halving the up-move in dB turns that overshoot into a
/// converging approach from below. DOWN-caps are NOT damped: limit-down stays exact
/// and always wins (the Random arbitration rule).
pub const LIMIT_UP_MOVE_DAMPING: f64 = 0.5;

/// Per-band response bounds for the law's limit channels. One curve per limit
/// channel, each on the SAME analysis band grid as
/// [`ShockLawInputs::band_freqs`] / `predicted_limit_srs`, in the channel's response
/// units (g for accel). `f64::INFINITY` = no bound at that band.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawLimits {
    /// Per-channel, per-band SRS bounds. Channel order must match the order the
    /// service packs `predicted_limit_srs` in.
    pub bounds: Vec<Vec<f64>>,
    /// Prediction headroom, dB (>= 0): predictions must stay this far BELOW the
    /// bound (`allowed = bound * 10^(-headroom_db/20)`). Budgets the zero-phase
    /// FRF-prediction slack (see [`crate::limit`]'s honesty notes).
    pub headroom_db: f64,
}

/// What the limit cap did on the last pulse (diagnostics beyond the shared
/// [`ShockLawDiagnostics`], which is a pinned cross-crate type).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct LawLimitStatus {
    /// How many component gain updates the limit cap actually bound (cap below the
    /// control update) on the last pulse.
    pub limited_components: usize,
    /// Worst predicted exceedance over all channels/bands on the last pulse, dB
    /// (`20*log10(predicted/allowed)`; positive = over). `None` when no comparable
    /// prediction was supplied.
    pub worst_predicted_exceedance_db: Option<f64>,
    /// True when bounds are configured but the pulse carried NO prediction, so the
    /// safety-monotone hold (no up-moves) was applied.
    pub held_no_prediction: bool,
}

/// The run-time SRS-synthesis control law, with the predictive-limit cap. Applies the
/// classic per-band ratio update to each synthesis component from the measured SRS,
/// clamped per pulse; when built [`SrsSynthesisLaw::with_limits`], caps every update by
/// the predicted limit-channel headroom (limit-down beats control-up). Does NOT move
/// the level (the level ladder owns the rung).
#[derive(Debug, Clone, PartialEq)]
pub struct SrsSynthesisLaw {
    /// The synthesis component natural frequencies (Hz), in table order. The law maps
    /// each to its nearest analysis band.
    component_freqs: Vec<f64>,
    /// Per-iteration per-component gain clamp, dB (positive, finite).
    max_gain_db_per_iter: f64,
    /// Optional limit bounds. `None` = non-limiting law.
    limits: Option<LawLimits>,
    /// What the limit cap did on the last `on_pulse` (zeroed by `init`).
    last_limit_status: LawLimitStatus,
}

impl SrsSynthesisLaw {
    /// Build the law for the given synthesis-component frequencies with the default
    /// per-iteration clamp. The frequency list must be non-empty.
    pub fn new(component_freqs: Vec<f64>) -> Result<Self, ControlLawError> {
        Self::with_clamp(component_freqs, DEFAULT_MAX_GAIN_DB_PER_ITER)
    }

    /// Build the law with an explicit per-iteration dB clamp (positive, finite).
    pub fn with_clamp(
        component_freqs: Vec<f64>,
        max_gain_db_per_iter: f64,
    ) -> Result<Self, ControlLawError> {
        if component_freqs.is_empty() {
            return Err(ControlLawError::BadConfig(
                "component_freqs must be non-empty".into(),
            ));
        }
        if !max_gain_db_per_iter.is_finite() || max_gain_db_per_iter <= 0.0 {
            return Err(ControlLawError::BadConfig(format!(
                "max_gain_db_per_iter must be finite and > 0 (got {max_gain_db_per_iter})"
            )));
        }
        Ok(SrsSynthesisLaw {
            component_freqs,
            max_gain_db_per_iter,
            limits: None,
            last_limit_status: LawLimitStatus::default(),
        })
    }

    /// Build the LIMITING law: as [`Self::with_clamp`], plus per-band response
    /// bounds for the limit channels the service predicts through its reduced-level
    /// FRFs. See the module docs for the cap semantics (limit-down beats control-up;
    /// no-prediction pulses hold).
    pub fn with_limits(
        component_freqs: Vec<f64>,
        max_gain_db_per_iter: f64,
        limits: LawLimits,
    ) -> Result<Self, ControlLawError> {
        let mut law = Self::with_clamp(component_freqs, max_gain_db_per_iter)?;
        if limits.bounds.is_empty() {
            return Err(ControlLawError::BadConfig(
                "limits.bounds must have at least one channel".into(),
            ));
        }
        for (c, ch) in limits.bounds.iter().enumerate() {
            if ch.is_empty() {
                return Err(ControlLawError::BadConfig(format!(
                    "limits.bounds[{c}] is empty"
                )));
            }
            if ch.iter().any(|&b| b.is_nan() || b <= 0.0) {
                return Err(ControlLawError::BadConfig(format!(
                    "limits.bounds[{c}] entries must be > 0 (use +inf for 'no bound')"
                )));
            }
        }
        if !limits.headroom_db.is_finite() || limits.headroom_db < 0.0 {
            return Err(ControlLawError::BadConfig(format!(
                "limits.headroom_db must be finite and >= 0 (got {})",
                limits.headroom_db
            )));
        }
        law.limits = Some(limits);
        Ok(law)
    }

    /// The per-iteration dB clamp this law uses.
    pub fn max_gain_db_per_iter(&self) -> f64 {
        self.max_gain_db_per_iter
    }

    /// The component frequencies the law updates (table order).
    pub fn component_freqs(&self) -> &[f64] {
        &self.component_freqs
    }

    /// The configured limit bounds, if this is a limiting law.
    pub fn limits(&self) -> Option<&LawLimits> {
        self.limits.as_ref()
    }

    /// What the limit cap did on the most recent [`ShockControlLaw::on_pulse`]
    /// (all-default after `init` or for a non-limiting law).
    pub fn last_limit_status(&self) -> LawLimitStatus {
        self.last_limit_status
    }

    /// Which component-frequency list to iterate: prefer the inputs' `component_freqs`
    /// when its length matches the law's own (a service that re-synthesizes may reorder
    /// or refine the grid), else fall back to the law's stored list. Either way the
    /// output length matches `inputs.ncomponents()` so the seam contract holds.
    fn freqs_for<'a>(&'a self, inputs: &'a ShockLawInputs) -> &'a [f64] {
        if inputs.component_freqs.len() == self.component_freqs.len() {
            inputs.component_freqs
        } else {
            &self.component_freqs
        }
    }

    /// The predictive-limit gain CAP for a component at `fc`: the tightest
    /// `allowed / predicted` ratio over every limit channel and every band within
    /// [`LIMIT_CAP_WINDOW_OCTAVES`] of `fc` (see that constant for why a window and
    /// not just the nearest band), or `+inf` when nothing constrains the component
    /// (no bound in reach, or no predicted response to compare).
    ///
    /// `predictions` is the per-channel predicted SRS from
    /// [`ShockLawInputs::predicted_limit_srs`]. Channel `c`'s bound curve is
    /// `limits.bounds[c]`; a prediction channel without a configured bound curve (or
    /// vice versa) contributes nothing - channel order is the pairing, and a shorter
    /// curve simply caps only the bands it covers.
    fn limit_cap_for_component(
        limits: &LawLimits,
        predictions: &[Vec<f64>],
        band_freqs: &[f64],
        fc: f64,
    ) -> f64 {
        let headroom_lin = 10.0_f64.powf(-limits.headroom_db / 20.0);
        let window = std::f64::consts::LN_2 * LIMIT_CAP_WINDOW_OCTAVES;
        let mut cap = f64::INFINITY;
        for (b, &fb) in band_freqs.iter().enumerate() {
            if fc <= 0.0 || fb <= 0.0 || (fb / fc).ln().abs() > window {
                continue;
            }
            for (bounds, preds) in limits.bounds.iter().zip(predictions) {
                let (Some(&bound), Some(&pred)) = (bounds.get(b), preds.get(b)) else {
                    continue;
                };
                if !bound.is_finite() || pred <= 0.0 {
                    continue; // no bound here, or nothing predicted to compare
                }
                let allowed = bound * headroom_lin;
                let ratio = allowed / pred;
                if ratio < cap {
                    cap = ratio;
                }
            }
        }
        cap
    }
}

/// The predictive-limit cap on the gain update of the component at `fc`, as
/// the run applies it after the law (limit-down beats control-up): the
/// tightest `allowed / predicted` ratio over the limit channels and the bands
/// within [`LIMIT_CAP_WINDOW_OCTAVES`], an up-move damped by
/// [`LIMIT_UP_MOVE_DAMPING`], a binding cap cut a further
/// [`LIMIT_DOWN_OVERCUT_DB`]. With no prediction the cap is 1 (nothing rises
/// without a prediction) and the second value is `true`.
pub fn predictive_cap(
    limits: &LawLimits,
    predictions: Option<&[Vec<f64>]>,
    band_freqs: &[f64],
    fc: f64,
) -> (f64, bool) {
    match predictions {
        Some(preds) => {
            let c = SrsSynthesisLaw::limit_cap_for_component(limits, preds, band_freqs, fc);
            let cap = if c > 1.0 && c.is_finite() {
                // Damped approach to the bound from below.
                c.powf(LIMIT_UP_MOVE_DAMPING)
            } else if c < 1.0 {
                // Binding: exact ratio plus the fixed over-cut so the down-move
                // wins against window-edge pumping.
                c * 10.0_f64.powf(-LIMIT_DOWN_OVERCUT_DB / 20.0)
            } else {
                c
            };
            (cap, false)
        }
        // Bounds configured, no prediction this pulse: safety-monotone hold.
        None => (1.0, true),
    }
}

/// `min(update, cap)`: the gain the run applies, and whether the cap bound it.
pub fn apply_cap(update: f64, cap: f64) -> (f64, bool) {
    if cap < update {
        (cap.max(0.0), true)
    } else {
        (update, false)
    }
}

/// The worst predicted exceedance over every channel and band, dB over the
/// allowed level (`20 log10(predicted / allowed)`; positive is over). `None`
/// when no comparable prediction was supplied.
pub fn worst_predicted_exceedance_db(limits: &LawLimits, predictions: &[Vec<f64>]) -> Option<f64> {
    let headroom_lin = 10.0_f64.powf(-limits.headroom_db / 20.0);
    let mut worst: Option<f64> = None;
    for (bounds, pred) in limits.bounds.iter().zip(predictions) {
        for (&bound, &p) in bounds.iter().zip(pred) {
            if !bound.is_finite() || p <= 0.0 {
                continue;
            }
            let db = 20.0 * (p / (bound * headroom_lin)).log10();
            worst = Some(worst.map_or(db, |w: f64| w.max(db)));
        }
    }
    worst
}

impl ShockControlLaw for SrsSynthesisLaw {
    fn init(&mut self, inputs: &ShockLawInputs) -> ShockLawOutputs {
        self.last_limit_status = LawLimitStatus::default();
        // Seed every component at unity and hold the level (the ladder owns the rung).
        ShockLawOutputs {
            component_gain_updates: vec![1.0; inputs.ncomponents()],
            level_scale_cmd: inputs.current_scale,
            diagnostics: ShockLawDiagnostics::default(),
        }
    }

    fn on_pulse(&mut self, inputs: &ShockLawInputs) -> ShockLawOutputs {
        let m = inputs.ncomponents();
        let nb = inputs.nbands();
        let freqs = self.freqs_for(inputs).to_vec();
        let mut gains = vec![1.0f64; m];
        let mut clamped = 0usize;
        let mut status = LawLimitStatus::default();

        if nb > 0 {
            // The ratio update is the built-in law's; a grid the arithmetic
            // cannot take (mismatched lengths) holds every gain.
            let params = ratio::SrsRatioParams {
                max_db_per_pulse: self.max_gain_db_per_iter,
            };
            let step = ratio::SrsRatio::new(params, inputs.band_freqs, &freqs, None)
                .and_then(|mut r| r.iterate(inputs.target_srs, inputs.measured_srs));
            if let Ok(step) = step {
                clamped = step.report.clamped_components;
                gains = step.gains;
            }
            // The predictive-limit cap (module docs).
            if let Some(limits) = &self.limits {
                for (j, gain) in gains.iter_mut().enumerate() {
                    let fc = freqs.get(j).copied().unwrap_or(0.0);
                    let (cap, held) =
                        predictive_cap(limits, inputs.predicted_limit_srs, inputs.band_freqs, fc);
                    status.held_no_prediction |= held;
                    let (g, bound) = apply_cap(*gain, cap);
                    *gain = g;
                    status.limited_components += usize::from(bound);
                }
            }
        }

        // Worst predicted exceedance for the limit status (diagnostic).
        if let (Some(limits), Some(preds)) = (&self.limits, inputs.predicted_limit_srs) {
            status.worst_predicted_exceedance_db = worst_predicted_exceedance_db(limits, preds);
        }
        self.last_limit_status = status;

        // Diagnostics: worst per-band SRS error over the grid.
        let mut worst_db: Option<f64> = None;
        let mut worst_idx: Option<usize> = None;
        for b in 0..nb {
            let tgt = inputs.target_srs.get(b).copied().unwrap_or(0.0);
            let meas = inputs.measured_srs.get(b).copied().unwrap_or(0.0);
            if tgt > 0.0 && meas > 0.0 {
                let db = 20.0 * (meas / tgt).log10();
                if worst_db.map(|w| db.abs() > w.abs()).unwrap_or(true) {
                    worst_db = Some(db);
                    worst_idx = Some(b);
                }
            }
        }

        ShockLawOutputs {
            component_gain_updates: gains,
            // This law does not move the level; the ladder owns the rung.
            level_scale_cmd: inputs.current_scale,
            diagnostics: ShockLawDiagnostics {
                worst_band_err_db: worst_db,
                worst_band_index: worst_idx,
                clamped_components: clamped,
            },
        }
    }

    fn name(&self) -> &str {
        "srs-synthesis"
    }
}
