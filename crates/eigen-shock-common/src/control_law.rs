//! The shock control-law seam and the trivial
//! default [`PerBandGainLaw`].
//!
//! # Why shock gets its OWN control-law trait
//!
//! Random's [`eigen_random::control::ControlLaw`] is a CONTINUOUS PSD trait: it
//! iterates every control loop against a fresh averaged spectrum, owning a scalar
//! level servo plus a mean-preserving per-bin shape trim. Shock is EVENT-DRIVEN: a
//! pulse fires, gets captured, gets an SRS computed, and only THEN does the law
//! produce a per-component gain update and a level scalar for the NEXT pulse. There is
//! no continuous spectrum and no per-loop servo. Forcing shock through Random's trait
//! would misrepresent the timing and the quantities, so shock gets its own trait,
//! [`ShockControlLaw`], with a fire -> capture -> SRS -> update boundary.
//!
//! The SHARED posture (carried over from Random) still holds: this trait is the
//! INTERNAL loop boundary, chosen so a predictive-limiting law can slot in
//! without a signature break. It is NOT the frozen, versioned plugin ABI a third party
//! compiles against - that is deferred to the module-ABI decision so
//! MIMO/limiting can inform its shape first. Do not treat this trait as that ABI.
//!
//! # The one shipped law: [`PerBandGainLaw`]
//!
//! The classic SRS iteration: at each natural frequency (band), the drive is scaled by
//! the target/measured SRS ratio, clamped to a max dB-per-iteration so a bad band
//! cannot blow up. This converges the control SRS to the target the way every
//! commercial SRS controller's baseline loop does. The PREDICTIVE-NOTCH limiting that
//! caps component gains where a limit-channel prediction would exceed bounds
//! (Coleman/Smallwood) lives in `eigen-srs`
//! (`SrsSynthesisLaw::with_limits` + the `limit` module - it slots into this trait
//! with no signature break, as intended); this baseline law leaves the
//! `predicted_limit_srs` input unused and documents that.

/// Everything a [`ShockControlLaw`] sees after one pulse fires and is analyzed.
/// All spectra are on the SAME natural-frequency grid `band_freqs` (length `N`),
/// the SRS analysis grid.
///
/// Borrowed slices (no allocation to call a law), mirroring Random's `LoopInputs`
/// shape.
pub struct ShockLawInputs<'a> {
    /// The target SRS on the band grid (the acceleration SRS the control channel must
    /// match), in g. Length `N`.
    pub target_srs: &'a [f64],
    /// The measured control-channel SRS from the just-fired pulse, in g. Length `N`.
    pub measured_srs: &'a [f64],
    /// The natural-frequency grid the SRS are on, Hz. Length `N` (the octave grid from
    /// [`eigen_dsp::srs::octave_freqs`], typically).
    pub band_freqs: &'a [f64],
    /// The synthesis-component natural frequencies, Hz. The law updates a gain PER
    /// COMPONENT (there may be more or fewer components than SRS bands - the three
    /// grids are distinct). Length `M`.
    pub component_freqs: &'a [f64],
    /// OPTIONAL predicted limit-channel SRS from measured FRFs, one per limit channel,
    /// each on the `band_freqs` grid. The shipped [`PerBandGainLaw`]
    /// IGNORES this (it carries no limit bounds to compare against); the limiting
    /// law (`eigen-srs::SrsSynthesisLaw::with_limits`) caps component gains where a
    /// prediction exceeds its bounds (limit-down beats control-up).
    pub predicted_limit_srs: Option<&'a [Vec<f64>]>,
    /// The controller's current overall level scale (linear, `10^(level_db/20)`),
    /// applied on top of the per-component gains. The law returns the NEXT level
    /// scale command in [`ShockLawOutputs::level_scale_cmd`].
    pub current_scale: f64,
}

impl ShockLawInputs<'_> {
    /// Number of SRS band grid points.
    pub fn nbands(&self) -> usize {
        self.band_freqs.len()
    }

    /// Number of synthesis components.
    pub fn ncomponents(&self) -> usize {
        self.component_freqs.len()
    }
}

/// One law's output after a pulse: the per-component gain updates to
/// apply before the next pulse, the level scalar command, and diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct ShockLawOutputs {
    /// The MULTIPLICATIVE per-component gain update to apply to each synthesis
    /// component before the next pulse (length `M`, matching
    /// [`ShockLawInputs::component_freqs`]). `1.0` = leave a component unchanged.
    pub component_gain_updates: Vec<f64>,
    /// The overall level scale to command for the next pulse (linear). The service's
    /// level ladder ([`crate::ladder::ShockLevelSchedule`]) owns the rung; a law that
    /// only shapes returns `current_scale` unchanged.
    pub level_scale_cmd: f64,
    /// Human-readable per-iteration diagnostics for the run log / UI.
    pub diagnostics: ShockLawDiagnostics,
}

/// Per-pulse control diagnostics, surfaced to the run log / UI.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShockLawDiagnostics {
    /// The worst per-band SRS error this pulse, in dB (`20*log10(measured/target)`);
    /// `None` before any valid measured band.
    pub worst_band_err_db: Option<f64>,
    /// The band index of [`Self::worst_band_err_db`].
    pub worst_band_index: Option<usize>,
    /// How many component gain updates hit the per-iteration dB clamp this pulse (a
    /// high count means the loop is rate-limited, not converged).
    pub clamped_components: usize,
}

/// A pluggable shock control law. Event-driven: [`Self::init`] seeds the
/// component gains before the first pulse; [`Self::on_pulse`] advances one iteration
/// given a fresh captured+analyzed pulse.
///
/// # NOT the frozen module ABI
///
/// This is the internal loop boundary, shaped so a predictive-limiting law can
/// slot in without a signature break. The user-supplied, versioned pluggable-law ABI
/// is DEFERRED to the module-ABI decision. Do not treat this trait as
/// that ABI; it will move as limiting and MIMO inform its shape.
pub trait ShockControlLaw {
    /// Seed the law before the first pulse: return the initial per-component gains
    /// (length `M`, matching `inputs.component_freqs`) and the starting level scale.
    /// The `measured_srs` on the first call is typically zeros (no pulse yet); a law
    /// may seed from the target shape.
    fn init(&mut self, inputs: &ShockLawInputs) -> ShockLawOutputs;

    /// Advance one iteration after a pulse fired and was analyzed: update the
    /// per-component gains and the level command from the measured SRS, and return
    /// them for the next pulse.
    fn on_pulse(&mut self, inputs: &ShockLawInputs) -> ShockLawOutputs;

    /// A short stable identifier for the law (run logs / UI selection).
    fn name(&self) -> &str;
}

/// Errors from constructing or configuring a [`ShockControlLaw`]. Library paths return
/// these instead of panicking (a real run must not be aborted by a panic in the
/// control math).
#[derive(Debug, Clone, PartialEq)]
pub enum ControlLawError {
    /// A config knob was out of its valid range. Carries the knob name and a note.
    BadConfig(String),
}

impl std::fmt::Display for ControlLawError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlLawError::BadConfig(s) => write!(f, "shock control law config error: {s}"),
        }
    }
}

impl std::error::Error for ControlLawError {}

/// The default `max dB / iteration` per-component gain clamp for [`PerBandGainLaw`].
/// Bounds any single-pulse transient so a badly-mismatched band cannot blow the drive
/// up in one step; the classic SRS-iteration safety limit.
pub const DEFAULT_MAX_GAIN_DB_PER_ITER: f64 = 3.0;

/// The trivial shipped shock control law: the classic per-band SRS ratio
/// iteration.
///
/// For each synthesis component it looks up the NEAREST SRS band, forms the linear
/// ratio `target_srs / measured_srs` at that band, and multiplies the component's gain
/// by that ratio - CLAMPED to `+/- max_gain_db_per_iter` in dB so no single pulse can
/// move a component more than that. This is the baseline loop every commercial SRS
/// controller ships: drive up where you are low, back off where you are high, one
/// bounded step per pulse.
///
/// This law does NOT limit: [`ShockLawInputs::predicted_limit_srs`] is IGNORED here.
/// Predictive-notch limiting that caps component gains where a limit-channel
/// prediction would exceed bounds (Coleman/Smallwood) is the SEPARATE law,
/// `eigen-srs::SrsSynthesisLaw::with_limits`. This law also does not move the level:
/// it returns `current_scale` unchanged (the level ladder owns the rung).
#[derive(Debug, Clone, PartialEq)]
pub struct PerBandGainLaw {
    max_gain_db_per_iter: f64,
    ncomponents: usize,
}

impl PerBandGainLaw {
    /// Build a per-band gain law for `ncomponents` synthesis components with the
    /// default per-iteration clamp ([`DEFAULT_MAX_GAIN_DB_PER_ITER`]).
    pub fn new(ncomponents: usize) -> Result<Self, ControlLawError> {
        Self::with_clamp(ncomponents, DEFAULT_MAX_GAIN_DB_PER_ITER)
    }

    /// Build a per-band gain law with an explicit per-iteration dB clamp. The clamp
    /// must be positive and finite.
    pub fn with_clamp(
        ncomponents: usize,
        max_gain_db_per_iter: f64,
    ) -> Result<Self, ControlLawError> {
        if ncomponents == 0 {
            return Err(ControlLawError::BadConfig("ncomponents must be > 0".into()));
        }
        if !(max_gain_db_per_iter.is_finite()) || max_gain_db_per_iter <= 0.0 {
            return Err(ControlLawError::BadConfig(format!(
                "max_gain_db_per_iter must be finite and > 0 (got {max_gain_db_per_iter})"
            )));
        }
        Ok(PerBandGainLaw {
            max_gain_db_per_iter,
            ncomponents,
        })
    }

    /// The per-iteration dB clamp this law uses.
    pub fn max_gain_db_per_iter(&self) -> f64 {
        self.max_gain_db_per_iter
    }

    /// Index of the band whose frequency is nearest `f_hz` in `band_freqs`. Returns 0
    /// for an empty grid (guarded by the caller). Linear scan (the grids are small).
    fn nearest_band(band_freqs: &[f64], f_hz: f64) -> usize {
        let mut best = 0usize;
        let mut best_d = f64::INFINITY;
        for (i, &fb) in band_freqs.iter().enumerate() {
            let d = (fb - f_hz).abs();
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        best
    }
}

impl ShockControlLaw for PerBandGainLaw {
    fn init(&mut self, inputs: &ShockLawInputs) -> ShockLawOutputs {
        // Seed every component at unity gain and hold the level (the ladder owns the
        // rung). No measurement yet, so no diagnostics.
        ShockLawOutputs {
            component_gain_updates: vec![1.0; inputs.ncomponents()],
            level_scale_cmd: inputs.current_scale,
            diagnostics: ShockLawDiagnostics::default(),
        }
    }

    fn on_pulse(&mut self, inputs: &ShockLawInputs) -> ShockLawOutputs {
        let m = inputs.ncomponents();
        let nb = inputs.nbands();
        let max_lin = 10.0_f64.powf(self.max_gain_db_per_iter / 20.0);
        let min_lin = 1.0 / max_lin;

        let mut gains = vec![1.0f64; m];
        let mut clamped = 0usize;

        if nb > 0 {
            for (j, &fc) in inputs.component_freqs.iter().enumerate() {
                let b = Self::nearest_band(inputs.band_freqs, fc);
                let tgt = inputs.target_srs.get(b).copied().unwrap_or(0.0);
                let meas = inputs.measured_srs.get(b).copied().unwrap_or(0.0);
                // The classic SRS-iteration ratio. Guard a dead/zero measured band:
                // with no response we cannot form a trusted ratio, so hold the gain
                // (unity) rather than command an unbounded push into noise. A
                // nonpositive target likewise means "no demand here", hold.
                let ratio = if meas > 0.0 && tgt > 0.0 {
                    tgt / meas
                } else {
                    1.0
                };
                let clamped_ratio = ratio.clamp(min_lin, max_lin);
                if (clamped_ratio - ratio).abs() > 1e-12 {
                    clamped += 1;
                }
                gains[j] = clamped_ratio;
            }
        }

        // Diagnostics: the worst per-band SRS error over the band grid.
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
        "per-band-gain"
    }
}
