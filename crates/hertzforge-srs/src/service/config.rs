//! [`SrsTestConfig`] - everything an SRS-shock run needs, plus the log-octave
//! tolerance envelope.
//!
//! Mirrors `hertzforge-random::service::config`'s shape: a serde-serializable config with
//! an explicit `validate()`, seeded metadata, and capture rules. The
//! shock-specific pieces are:
//!
//! - the TARGET SRS on a fractional-octave band grid (1/1 .. 1/12 octave
//!   via [`hertzforge_dsp::srs::octave_freqs`], selectable through [`BandFraction`]);
//! - the per-band `+/- dB` alarm/abort envelope ON THAT LOG-OCTAVE GRID
//!   ([`SrsEnvelope`]); the velocity-change and pulse-substitution checks are
//!   classical-shock scope;
//! - the synthesis engine selection (WAVSYN | SEDS) + the
//!   [`SynthesisOpts`] iteration knobs;
//! - the [`ShockLevelSchedule`] level ladder (default
//!   `[-12, -9, -6, -3, 0]` dB, N pulses per level);
//! - the limit-channel set for predictive notching;
//! - the shaker + moving mass for the DETERMINISTIC feasibility gate (crest = 1.0);
//! - the [`DriveLimit`] peak ceilings (the honesty split: enforcement, not
//!   prediction);
//! - the optional reduced-level system ID (Path-B-only).

use crate::iterate::SynthesisOpts;
use hertzforge_dsp::srs::{octave_freqs, OctaveFraction};
use hertzforge_shaker_db::{MovingMass, ShakerRecord};
use hertzforge_shock_common::{DriveLimit, FeasibilityTolerances, ShockLevelSchedule};
use serde::{Deserialize, Serialize};

/// The default SDOF quality factor for the SRS analysis (`zeta = 1/(2 Q)`), matching
/// [`hertzforge_shock_common::pulse_store::DEFAULT_SRS_Q`].
pub const DEFAULT_Q: f64 = 10.0;

/// Default alarm envelope half-widths, dB `(low, high)`.
pub const DEFAULT_ALARM_DB: (f64, f64) = (3.0, 3.0);

/// Default abort envelope half-widths, dB `(low, high)`. MIL-STD-810 SRS tolerance
/// is typically `+6/-3 dB` at full level; the default here is symmetric and a little
/// looser on the low side because the iteration approaches the target from below
/// during the ladder walk (the envelope is level-normalized, but the first pulses at
/// a fresh level still sit under the target until the law trims them up).
pub const DEFAULT_ABORT_DB: (f64, f64) = (9.0, 6.0);

/// Default system-ID level, dB relative to full (a REDUCED level so the plant is
/// identified gently before any full-level pulse).
pub const DEFAULT_SYSID_LEVEL_DB: f64 = -20.0;

/// Default system-ID FFT size (bins are `fs / nfft`).
pub const DEFAULT_SYSID_NFFT: usize = 1024;

/// Default `|H|` trust floor fraction for the FRF feedforward seed: a component's
/// drive amplitude is divided by `max(|H(f_c)|, frac * max_inband |H|)`, so a dead
/// FRF bin cannot command an unbounded drive (the same guard shape as Random's
/// `trust_h_floor_frac`).
pub const DEFAULT_H_FLOOR_FRAC: f64 = 0.05;

/// Default ring-out window captured after each pulse's drive ends, seconds (lets the
/// fixture ring-down land in the analyzed record - resonant plants keep responding
/// after the drive stops).
pub const DEFAULT_RING_OUT_S: f64 = 0.05;

/// Default measured-over-ceiling abort factor for the drive monitors: a
/// measured monitor peak at or beyond `abort_factor * ceiling` ABORTS instead of
/// scaling the next pulse.
pub const DEFAULT_DRIVE_ABORT_FACTOR: f64 = 2.0;

/// A serde-friendly mirror of [`hertzforge_dsp::srs::OctaveFraction`]. The
/// dsp enum carries no serde derives (read-only crate), so the config carries its
/// own and converts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum BandFraction {
    /// Whole octave (`N = 1`).
    Whole,
    /// One-third octave (`N = 3`) - the classic default.
    Third,
    /// One-sixth octave (`N = 6`).
    Sixth,
    /// One-twelfth octave (`N = 12`).
    Twelfth,
    /// Arbitrary `N` (ratio `2^(1/N)`).
    Arbitrary(f64),
}

impl From<BandFraction> for OctaveFraction {
    fn from(b: BandFraction) -> Self {
        match b {
            BandFraction::Whole => OctaveFraction::Whole,
            BandFraction::Third => OctaveFraction::Third,
            BandFraction::Sixth => OctaveFraction::Sixth,
            BandFraction::Twelfth => OctaveFraction::Twelfth,
            BandFraction::Arbitrary(n) => OctaveFraction::Arbitrary(n),
        }
    }
}

/// The target SRS specification. Resolved onto the band grid by
/// [`SrsTestConfig::target_on_grid`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TargetSrs {
    /// A flat target: every band demands `accel_g`.
    Flat {
        /// The per-band SRS demand, g. Must be finite and > 0.
        accel_g: f64,
    },
    /// A breakpoint table `(freq_hz, accel_g)` interpolated LOG-LOG onto the band
    /// grid (the standard SRS-spec convention: straight lines on a log-log plot).
    /// Points must be ascending in frequency, positive in both axes, and must cover
    /// the analysis band (first point at or below `f_lo`, last at or above the last
    /// band center).
    Breakpoints { points: Vec<(f64, f64)> },
}

/// Which synthesis engine drives the run. The component tables default
/// to one component per band ([`crate::table::WaveletTable::default_over_band`] /
/// [`crate::table::DampedSineTable::default_over_band`]); a custom imported table
/// can be substituted through the service constructor later (the import parsers are
/// [`crate::table`]'s).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineKind {
    /// WAVSYN odd-half-cycle wavelets (self-nulling, no compensation).
    Wavsyn,
    /// SEDS decaying sinusoids with Smallwood-Nord compensation (Path B; on the
    /// delegated path the download is the RAW composite - the device compensates
    /// on-device).
    Seds,
}

/// One limit channel for predictive notching: a non-control
/// location whose PREDICTED response SRS must stay under a bound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitChannelSpec {
    /// Channel name. On Path B with system ID enabled, the FRF is measured against
    /// the plant tap whose channel-table `description` equals this name (or
    /// `"ch<index>"` for an unnamed channel).
    pub name: String,
    /// The per-band SRS bound, g (`f64::INFINITY` = no bound at a band).
    pub limit: LimitTarget,
    /// A user-supplied FRF magnitude drive -> this channel. `None` on Path B means
    /// the reduced-level system ID measures it; on the delegated path
    /// (or with system ID disabled) a missing FRF is a validation error at arm -
    /// there is nothing to predict through.
    pub frf: Option<FrfSpec>,
}

/// A limit bound resolved onto the band grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LimitTarget {
    /// Every band bounded at `accel_g`.
    Flat { accel_g: f64 },
    /// One bound per analysis band (must match the band-grid length).
    PerBand { bounds_g: Vec<f64> },
}

/// A sampled FRF magnitude `|H(f)|` (drive -> channel) on an ascending frequency
/// grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrfSpec {
    /// Sample frequencies, Hz, ascending.
    pub freqs_hz: Vec<f64>,
    /// `|H|` per sample (dimensionless for g-per-g).
    pub mag: Vec<f64>,
}

/// The limit set: the channels plus the prediction headroom.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitSet {
    pub channels: Vec<LimitChannelSpec>,
    /// Prediction headroom, dB (>= 0): predictions must stay this far BELOW the
    /// bound. Budgets the zero-phase FRF-prediction slack ([`crate::limit`]'s
    /// honesty notes).
    pub headroom_db: f64,
}

/// Reduced-level system-ID options. Path-B-only (on the
/// delegated path the DEVICE owns system ID at arm; Hertzforge observes).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SysIdSpec {
    /// Excitation level, dB relative to full (default [`DEFAULT_SYSID_LEVEL_DB`]).
    pub level_db: f64,
    /// The excitation shape.
    pub excitation: SysIdExcitation,
    /// FFT size for [`hertzforge_dsp::frf::h_estimate_hv_with_coherence`] (power of two).
    pub nfft: usize,
    /// `|H|` trust floor fraction for the feedforward seed
    /// ([`DEFAULT_H_FLOOR_FRAC`]).
    pub h_floor_frac: f64,
}

impl Default for SysIdSpec {
    fn default() -> Self {
        SysIdSpec {
            level_db: DEFAULT_SYSID_LEVEL_DB,
            excitation: SysIdExcitation::Shaped,
            nfft: DEFAULT_SYSID_NFFT,
            h_floor_frac: DEFAULT_H_FLOOR_FRAC,
        }
    }
}

/// The system-ID excitation shape ("shaped system ID + standard pulses").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SysIdExcitation {
    /// SHAPED: the target-converged synthesis waveform scaled to the sysID level -
    /// concentrates identification energy exactly where the test will drive.
    Shaped,
    /// STANDARD: a short low-level broadband pulse (a few samples wide), the
    /// classical low-level-pulse system ID.
    StandardPulse,
}

/// Per-pulse kinematic abort ceilings ("Random's safety features", shock
/// form): checked against every ingested pulse's measured kinematics
/// ([`hertzforge_shock_common::PulseRecord::kinematics`], the pure-integrator path).
/// `None` = no ceiling on that axis.
///
/// `#[serde(default)]` (2026-08-09): a missing axis is NO CEILING on that axis,
/// which is what `Default` already says, rather than a parse failure. The front end
/// sends this object under different key names (`ui/src/lib/shock/types.ts` calls
/// them `peak_accel_g` / `peak_velocity_ips` / `pk_pk_disp_in`), so today every
/// UI-sent config lands here on the default. That mismatch is a REAL gap - no
/// kinematic ceiling is settable from the panel - but a defaulted struct is
/// the same posture as omitting the field, and it is not the reason the whole
/// configure died. Pinned by `ui/src-tauri`'s
/// `the_front_ends_srs_config_json_deserializes`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct KinematicAbort {
    /// Peak measured acceleration ceiling, g.
    pub accel_g: Option<f64>,
    /// Peak measured velocity ceiling, in/s.
    pub velocity_ips: Option<f64>,
    /// Peak-to-peak measured displacement ceiling, in.
    pub disp_pp_in: Option<f64>,
}

/// How pulse capture is triggered (every pulse recorded).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShockCaptureTrigger {
    /// Capture off.
    Off,
    /// Every fired pulse's control time history is streamed to the capture sink.
    EveryPulse,
    /// Operator-toggled (`start_capture` / `stop_capture`); pulses fired while a
    /// capture is open are streamed.
    Manual,
}

/// The capture rules for a run.
///
/// `#[serde(default)]` (2026-08-09) - and this one was load-bearing, not hygiene.
///
/// SRS's capture rules are `{trigger, snapshot_on_stop}`; CLASSICAL's
/// ([`hertzforge_shock_host::CaptureRules`]) are `{auto_start, run_name}`. The UI has ONE
/// `ShockCaptureRules` TypeScript type (`ui/src/lib/shock/types.ts`) shaped like
/// classical's, and `srsStore.config()` sends it to the SRS configure verb. With no
/// defaults here, serde refused that object with `missing field 'trigger'`, so
/// **every local (Tauri) SRS configure failed until 2026-08-09** - inside
/// Tauri's argument deserialization, before `shock_srs_configure` was ever entered.
/// Nothing caught it because every Rust test built its config in Rust.
///
/// Defaulting is the SAFE resolution: [`Default`] is capture OFF plus no snapshot,
/// so a config that cannot express capture rules gets none, rather than inheriting
/// something it never asked for. Giving the operator real capture-trigger CONTROL is
/// the UI half of the fix. Pinned by `ui/src-tauri`'s
/// `the_front_ends_srs_config_json_deserializes`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShockCaptureRules {
    pub trigger: ShockCaptureTrigger,
    /// Save a [`hertzforge_shock_common::ShockSnapshotPayload`] snapshot bundle on stop
    /// (clean stop or abort).
    pub snapshot_on_stop: bool,
}

impl Default for ShockCaptureRules {
    fn default() -> Self {
        ShockCaptureRules {
            trigger: ShockCaptureTrigger::Off,
            snapshot_on_stop: false,
        }
    }
}

/// One user-defined metadata field (same shape as Random's).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetaField {
    /// Stable machine key, e.g. `run_number`.
    pub key: String,
    /// Human label, e.g. `Run Number`.
    pub label: String,
    /// Freeform value.
    pub value: String,
}

impl MetaField {
    pub fn new(key: impl Into<String>, label: impl Into<String>, value: impl Into<String>) -> Self {
        MetaField {
            key: key.into(),
            label: label.into(),
            value: value.into(),
        }
    }

    /// The seeded default metadata set.
    pub fn defaults() -> Vec<MetaField> {
        vec![
            MetaField::new("run_number", "Run Number", ""),
            MetaField::new("test_article", "Test Article", ""),
            MetaField::new("serial_number", "SN", ""),
            MetaField::new("operator", "Operator", ""),
        ]
    }
}

/// A serde mirror of [`hertzforge_shock_common::FeasibilityTolerances`] (that struct has
/// no serde derives; pinned crate).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FeasibilityTol {
    /// Max acceptable `|residual velocity|`, in/s (the compensation gate).
    pub residual_velocity_ips: f64,
    /// Max acceptable `|residual displacement|`, in.
    pub residual_disp_in: f64,
}

impl Default for FeasibilityTol {
    fn default() -> Self {
        let d = FeasibilityTolerances::default();
        FeasibilityTol {
            residual_velocity_ips: d.residual_velocity_ips,
            residual_disp_in: d.residual_disp_in,
        }
    }
}

impl From<FeasibilityTol> for FeasibilityTolerances {
    fn from(t: FeasibilityTol) -> Self {
        FeasibilityTolerances {
            residual_velocity_ips: t.residual_velocity_ips,
            residual_disp_in: t.residual_disp_in,
        }
    }
}

// =============================================================================
// The tolerance envelope on the log-octave SRS grid
// =============================================================================

/// One band's `+/- dB` tolerance half-widths (all values are POSITIVE magnitudes;
/// the low side is applied as `-lo_db`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SrsToleranceBand {
    pub alarm_lo_db: f64,
    pub alarm_hi_db: f64,
    pub abort_lo_db: f64,
    pub abort_hi_db: f64,
}

/// The tolerance envelope: one [`SrsToleranceBand`] per analysis band, on the
/// SAME log-octave grid as the target SRS (the SRS accel envelope is a
/// new per-band structure on the log-octave grid; the `+/- dB` pattern is reused
/// from Random's envelope).
///
/// The check is LEVEL-NORMALIZED: at ladder level `L` dB the measured SRS is
/// compared against `target * 10^(L/20)` per band, so a `-12 dB` pulse does not
/// spuriously breach the full-level envelope (the same normalization Random's
/// envelope check applies).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SrsEnvelope {
    /// One tolerance entry per band-grid point.
    pub bands: Vec<SrsToleranceBand>,
}

/// The outcome of one envelope check.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EnvelopeCheck {
    /// Band indices whose error breached the ALARM band (but not abort).
    pub alarm_bands: Vec<usize>,
    /// Band indices whose error breached the ABORT band.
    pub abort_bands: Vec<usize>,
    /// The worst signed per-band error this pulse, dB (`20*log10(meas/target)` at
    /// the normalized level). `None` when no band was comparable.
    pub worst_err_db: Option<f64>,
    /// The band index of `worst_err_db`.
    pub worst_band: Option<usize>,
}

impl EnvelopeCheck {
    pub fn aborted(&self) -> bool {
        !self.abort_bands.is_empty()
    }
    pub fn alarmed(&self) -> bool {
        !self.alarm_bands.is_empty()
    }
}

impl SrsEnvelope {
    /// A uniform envelope: every band gets `alarm_db = (lo, hi)` and
    /// `abort_db = (lo, hi)`.
    pub fn uniform(nbands: usize, alarm_db: (f64, f64), abort_db: (f64, f64)) -> Self {
        SrsEnvelope {
            bands: vec![
                SrsToleranceBand {
                    alarm_lo_db: alarm_db.0,
                    alarm_hi_db: alarm_db.1,
                    abort_lo_db: abort_db.0,
                    abort_hi_db: abort_db.1,
                };
                nbands
            ],
        }
    }

    /// Validate against a band grid of `nbands` points: length must match, every
    /// half-width must be finite and >= 0, and the abort band must enclose the alarm
    /// band on each side (an abort tighter than its alarm would abort without ever
    /// alarming).
    pub fn validate(&self, nbands: usize) -> Result<(), ConfigError> {
        if self.bands.len() != nbands {
            return Err(ConfigError::BadConfig(format!(
                "envelope has {} bands but the analysis grid has {nbands}",
                self.bands.len()
            )));
        }
        for (i, b) in self.bands.iter().enumerate() {
            let vals = [b.alarm_lo_db, b.alarm_hi_db, b.abort_lo_db, b.abort_hi_db];
            if vals.iter().any(|v| !v.is_finite() || *v < 0.0) {
                return Err(ConfigError::BadConfig(format!(
                    "envelope band {i}: half-widths must be finite and >= 0"
                )));
            }
            if b.abort_lo_db < b.alarm_lo_db || b.abort_hi_db < b.alarm_hi_db {
                return Err(ConfigError::BadConfig(format!(
                    "envelope band {i}: abort band must enclose the alarm band"
                )));
            }
        }
        Ok(())
    }

    /// Check one pulse's measured SRS against the LEVEL-NORMALIZED target
    /// (`target * level_scale` per band). Bands where the target is nonpositive (no
    /// demand) or the measurement is nonpositive are skipped - there is no trusted
    /// ratio there.
    pub fn check(&self, target_full: &[f64], measured: &[f64], level_scale: f64) -> EnvelopeCheck {
        let mut out = EnvelopeCheck::default();
        let n = self.bands.len().min(target_full.len()).min(measured.len());
        for b in 0..n {
            let t = target_full[b] * level_scale;
            let m = measured[b];
            if t <= 0.0 || m <= 0.0 {
                continue;
            }
            let err = 20.0 * (m / t).log10();
            if out
                .worst_err_db
                .map(|w: f64| err.abs() > w.abs())
                .unwrap_or(true)
            {
                out.worst_err_db = Some(err);
                out.worst_band = Some(b);
            }
            let tol = &self.bands[b];
            if err > tol.abort_hi_db || err < -tol.abort_lo_db {
                out.abort_bands.push(b);
            } else if err > tol.alarm_hi_db || err < -tol.alarm_lo_db {
                out.alarm_bands.push(b);
            }
        }
        out
    }
}

/// The envelope specification carried in the config: uniform half-widths plus an
/// optional full per-band override.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvelopeSpec {
    /// Uniform alarm half-widths, dB `(low, high)`.
    pub alarm_db: (f64, f64),
    /// Uniform abort half-widths, dB `(low, high)`.
    pub abort_db: (f64, f64),
    /// Optional per-band override (one entry per band-grid point); when set, the
    /// uniform values are ignored.
    pub per_band: Option<Vec<SrsToleranceBand>>,
}

impl Default for EnvelopeSpec {
    fn default() -> Self {
        EnvelopeSpec {
            alarm_db: DEFAULT_ALARM_DB,
            abort_db: DEFAULT_ABORT_DB,
            per_band: None,
        }
    }
}

impl EnvelopeSpec {
    /// Resolve onto a band grid of `nbands` points.
    pub fn resolve(&self, nbands: usize) -> Result<SrsEnvelope, ConfigError> {
        let env = match &self.per_band {
            Some(bands) => SrsEnvelope {
                bands: bands.clone(),
            },
            None => SrsEnvelope::uniform(nbands, self.alarm_db, self.abort_db),
        };
        env.validate(nbands)?;
        Ok(env)
    }
}

// =============================================================================
// The config
// =============================================================================

/// Errors from validating an [`SrsTestConfig`].
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// A config value was out of its valid range. Carries a note.
    BadConfig(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::BadConfig(s) => write!(f, "srs test config error: {s}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// The full SRS-shock test configuration. See the module docs for the
/// shock-specific pieces. Construct via [`SrsTestConfig::flat`] (the common case)
/// or field-by-field, then [`SrsTestConfig::validate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SrsTestConfig {
    /// Analysis band low edge, Hz (always a band center - the
    /// [`hertzforge_dsp::srs::octave_freqs`] endpoint convention).
    pub f_lo_hz: f64,
    /// Analysis band high edge, Hz (covered by the last center at or just above it).
    pub f_hi_hz: f64,
    /// Fractional-octave band spacing.
    pub fraction: BandFraction,
    /// The target SRS.
    pub target: TargetSrs,
    /// SDOF quality factor for all SRS analysis (`zeta = 1/(2 Q)`), default
    /// [`DEFAULT_Q`].
    pub q: f64,
    /// Which synthesis engine drives the run.
    pub engine: EngineKind,
    /// The offline SRS-matching iteration knobs
    /// ([`crate::iterate::synthesize_to_target`]).
    pub synthesis: SynthesisOpts,
    /// The level ladder.
    pub schedule: ShockLevelSchedule,
    /// Predictive-notch limit channels. `None` = no limiting.
    pub limits: Option<LimitSet>,
    /// The shaker record for the deterministic feasibility gate.
    /// `None` = feasibility NotEvaluated (arming allowed, with a logged note).
    pub shaker: Option<ShakerRecord>,
    /// The moving-mass stack for the force -> acceleration deration.
    pub moving_mass: MovingMass,
    /// The compensation-quality residual tolerances for the feasibility gate.
    pub feasibility_tol: FeasibilityTol,
    /// Drive peak ceilings. Default: none configured.
    pub drive_limit: DriveLimit,
    /// Measured-over-ceiling abort factor for the drive monitors
    /// ([`DEFAULT_DRIVE_ABORT_FACTOR`]).
    pub drive_abort_factor: f64,
    /// Per-pulse kinematic abort ceilings.
    pub kinematic_abort: KinematicAbort,
    /// The tolerance envelope specification.
    pub envelope: EnvelopeSpec,
    /// Reduced-level system ID. `None` = skipped: the drive seed assumes
    /// a unity plant (logged honestly) and predictive drive-accel limiting is
    /// unavailable. Path-B-only.
    pub sysid: Option<SysIdSpec>,
    /// Capture rules.
    ///
    /// Serde-defaulted so a stored setup or a caller that omits the field entirely
    /// gets capture OFF rather than a refused parse. The INNER defaults (on
    /// [`ShockCaptureRules`]) are the ones that unblocked the front end; this one
    /// covers the omit-the-whole-field case the same way.
    #[serde(default)]
    pub capture: ShockCaptureRules,
    /// Also compute the pseudo-velocity SRS per pulse (PVSS).
    pub compute_pvss: bool,
    /// Ring-out window captured after each pulse's drive, seconds.
    pub ring_out_s: f64,
    /// HOLD after a passing system ID for operator review before anything drives.
    /// Default `true`.
    ///
    /// Serde-defaulted to `true` so an older stored setup that predates the field
    /// gets the SAFE behavior rather than silently running straight through.
    #[serde(default = "default_review_after_sysid")]
    pub review_after_sysid: bool,
    /// User metadata.
    pub metadata: Vec<MetaField>,
}

/// The serde default for [`SrsTestConfig::review_after_sysid`].
fn default_review_after_sysid() -> bool {
    true
}

impl SrsTestConfig {
    /// The common starting config: a flat `accel_g` target over
    /// `[f_lo_hz, f_hi_hz]` at 1/3-octave, WAVSYN, the default AUTO ladder, default
    /// envelope, no limits, no shaker, system ID enabled.
    pub fn flat(f_lo_hz: f64, f_hi_hz: f64, accel_g: f64) -> Self {
        SrsTestConfig {
            f_lo_hz,
            f_hi_hz,
            fraction: BandFraction::Third,
            target: TargetSrs::Flat { accel_g },
            q: DEFAULT_Q,
            engine: EngineKind::Wavsyn,
            synthesis: SynthesisOpts::default(),
            schedule: ShockLevelSchedule::auto_default(),
            limits: None,
            shaker: None,
            moving_mass: MovingMass::default(),
            feasibility_tol: FeasibilityTol::default(),
            drive_limit: DriveLimit {
                peak_accel_g: None,
                monitors: Vec::new(),
            },
            drive_abort_factor: DEFAULT_DRIVE_ABORT_FACTOR,
            kinematic_abort: KinematicAbort::default(),
            envelope: EnvelopeSpec::default(),
            sysid: Some(SysIdSpec::default()),
            capture: ShockCaptureRules::default(),
            compute_pvss: true,
            ring_out_s: DEFAULT_RING_OUT_S,
            review_after_sysid: default_review_after_sysid(),
            metadata: MetaField::defaults(),
        }
    }

    /// The analysis band grid: fractional-octave centers over
    /// `[f_lo_hz, f_hi_hz]`.
    pub fn band_freqs(&self) -> Vec<f64> {
        octave_freqs(self.f_lo_hz, self.f_hi_hz, self.fraction.into())
    }

    /// The target SRS resolved onto the band grid (one value per band center, g).
    pub fn target_on_grid(&self) -> Result<Vec<f64>, ConfigError> {
        let bands = self.band_freqs();
        if bands.is_empty() {
            return Err(ConfigError::BadConfig(format!(
                "octave_freqs produced no bands for [{}, {}] Hz",
                self.f_lo_hz, self.f_hi_hz
            )));
        }
        match &self.target {
            TargetSrs::Flat { accel_g } => {
                if !accel_g.is_finite() || *accel_g <= 0.0 {
                    return Err(ConfigError::BadConfig(format!(
                        "flat target accel_g must be finite and > 0 (got {accel_g})"
                    )));
                }
                Ok(vec![*accel_g; bands.len()])
            }
            TargetSrs::Breakpoints { points } => {
                if points.len() < 2 {
                    return Err(ConfigError::BadConfig(
                        "breakpoint target needs at least two points".into(),
                    ));
                }
                let mut prev = 0.0f64;
                for &(f, g) in points {
                    if !f.is_finite() || f <= 0.0 || f <= prev {
                        return Err(ConfigError::BadConfig(
                            "breakpoint frequencies must be finite, > 0, strictly ascending".into(),
                        ));
                    }
                    if !g.is_finite() || g <= 0.0 {
                        return Err(ConfigError::BadConfig(
                            "breakpoint amplitudes must be finite and > 0".into(),
                        ));
                    }
                    prev = f;
                }
                let last_band = *bands.last().unwrap();
                if points[0].0 > self.f_lo_hz || points.last().unwrap().0 < last_band {
                    return Err(ConfigError::BadConfig(format!(
                        "breakpoint table [{:.1}, {:.1}] Hz must cover the analysis grid \
                         [{:.1}, {:.1}] Hz (the last band center sits at or above f_hi - the \
                         octave_freqs cover-the-top rule)",
                        points[0].0,
                        points.last().unwrap().0,
                        self.f_lo_hz,
                        last_band
                    )));
                }
                Ok(bands.iter().map(|&f| log_log_interp(points, f)).collect())
            }
        }
    }

    /// Validate the whole config. Runs every sub-validation and cross-checks the
    /// grid-length couplings (limit curves and per-band envelopes must match the
    /// band grid).
    pub fn validate(&self) -> Result<(), ConfigError> {
        let bands = self.band_freqs();
        if bands.is_empty() {
            return Err(ConfigError::BadConfig(format!(
                "no analysis bands for [{}, {}] Hz at {:?}",
                self.f_lo_hz, self.f_hi_hz, self.fraction
            )));
        }
        let _ = self.target_on_grid()?;
        if !self.q.is_finite() || self.q <= 0.5 {
            return Err(ConfigError::BadConfig(format!(
                "q must be finite and > 0.5 (got {})",
                self.q
            )));
        }
        if self.synthesis.max_iters == 0 {
            return Err(ConfigError::BadConfig(
                "synthesis.max_iters must be >= 1".into(),
            ));
        }
        if !self.synthesis.tol_db.is_finite() || self.synthesis.tol_db <= 0.0 {
            return Err(ConfigError::BadConfig(
                "synthesis.tol_db must be finite and > 0".into(),
            ));
        }
        if !self.synthesis.max_gain_db_per_iter.is_finite()
            || self.synthesis.max_gain_db_per_iter <= 0.0
        {
            return Err(ConfigError::BadConfig(
                "synthesis.max_gain_db_per_iter must be finite and > 0".into(),
            ));
        }
        self.schedule
            .validate()
            .map_err(|e| ConfigError::BadConfig(format!("schedule: {e}")))?;
        if let Some(limits) = &self.limits {
            if limits.channels.is_empty() {
                return Err(ConfigError::BadConfig(
                    "limits configured with no channels".into(),
                ));
            }
            if !limits.headroom_db.is_finite() || limits.headroom_db < 0.0 {
                return Err(ConfigError::BadConfig(
                    "limits.headroom_db must be finite and >= 0".into(),
                ));
            }
            for ch in &limits.channels {
                if ch.name.trim().is_empty() {
                    return Err(ConfigError::BadConfig("limit channel name is empty".into()));
                }
                match &ch.limit {
                    LimitTarget::Flat { accel_g } => {
                        if accel_g.is_nan() || *accel_g <= 0.0 {
                            return Err(ConfigError::BadConfig(format!(
                                "limit channel '{}': flat bound must be > 0",
                                ch.name
                            )));
                        }
                    }
                    LimitTarget::PerBand { bounds_g } => {
                        if bounds_g.len() != bands.len() {
                            return Err(ConfigError::BadConfig(format!(
                                "limit channel '{}': {} bounds vs {} bands",
                                ch.name,
                                bounds_g.len(),
                                bands.len()
                            )));
                        }
                        if bounds_g.iter().any(|&b| b.is_nan() || b <= 0.0) {
                            return Err(ConfigError::BadConfig(format!(
                                "limit channel '{}': bounds must be > 0 (+inf = no bound)",
                                ch.name
                            )));
                        }
                    }
                }
                if let Some(frf) = &ch.frf {
                    if frf.freqs_hz.is_empty() || frf.freqs_hz.len() != frf.mag.len() {
                        return Err(ConfigError::BadConfig(format!(
                            "limit channel '{}': FRF freqs/mag must be non-empty and equal \
                             length",
                            ch.name
                        )));
                    }
                }
            }
        }
        self.drive_limit
            .validate()
            .map_err(|e| ConfigError::BadConfig(format!("drive limit: {e}")))?;
        if !self.drive_abort_factor.is_finite() || self.drive_abort_factor < 1.0 {
            return Err(ConfigError::BadConfig(
                "drive_abort_factor must be finite and >= 1".into(),
            ));
        }
        let _ = self.envelope.resolve(bands.len())?;
        if let Some(s) = &self.sysid {
            if !s.nfft.is_power_of_two() || s.nfft < 64 {
                return Err(ConfigError::BadConfig(format!(
                    "sysid.nfft must be a power of two >= 64 (got {})",
                    s.nfft
                )));
            }
            if !s.level_db.is_finite() || s.level_db >= 0.0 {
                return Err(ConfigError::BadConfig(format!(
                    "sysid.level_db must be finite and < 0 dB (a REDUCED level; \
                     got {})",
                    s.level_db
                )));
            }
            if !s.h_floor_frac.is_finite() || s.h_floor_frac <= 0.0 || s.h_floor_frac > 1.0 {
                return Err(ConfigError::BadConfig(
                    "sysid.h_floor_frac must be in (0, 1]".into(),
                ));
            }
        }
        if !self.ring_out_s.is_finite() || self.ring_out_s < 0.0 {
            return Err(ConfigError::BadConfig(
                "ring_out_s must be finite and >= 0".into(),
            ));
        }
        Ok(())
    }
}

/// Log-log linear interpolation of the breakpoint table at `f` (clamped to the end
/// values outside the table - callers validate coverage first, so the clamp only
/// absorbs float edges).
fn log_log_interp(points: &[(f64, f64)], f: f64) -> f64 {
    let n = points.len();
    if f <= points[0].0 {
        return points[0].1;
    }
    if f >= points[n - 1].0 {
        return points[n - 1].1;
    }
    let mut lo = 0usize;
    let mut hi = n - 1;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if points[mid].0 <= f {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (f0, g0) = points[lo];
    let (f1, g1) = points[hi];
    let t = (f / f0).ln() / (f1 / f0).ln();
    g0 * (g1 / g0).powf(t)
}
