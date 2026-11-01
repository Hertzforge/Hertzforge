//! The SHOCK wire surface (v5): [`ClassicalShockProfile`], [`SrsProfile`], and the
//! [`ShockCommand`] / [`SrsCommand`] groups.
//!
//! # Two profiles, not one
//!
//! Classical shock and SRS synthesis are different tests with different operator
//! surfaces: one states a PULSE (shape, amplitude, duration) and is judged in the time
//! domain against a standard's tolerance bands; the other states a TARGET SPECTRUM and
//! is synthesized to match it. Folding them into one variant would have produced a
//! payload where half the fields are meaningless for whichever test is running - and
//! "meaningless" is one refactor away from "silently defaulted". Two variants, each
//! `deny_unknown_fields`.
//!
//! Both mirror their service configuration field-for-field
//! (`eigen_shock_host::service::config::ClassicalTestConfig` and
//! `eigen_srs::service::config::SrsTestConfig`), typed rather than opaque, for the
//! reasons stated in [`crate::sine`]'s module docs.
//!
//! # What is DELIBERATELY absent: the capture block
//!
//! Neither profile carries capture rules, and that is a safety decision rather than an
//! omission. `CaptureRules::auto_start` would start a disk writer at ARM on the core
//! host; over this seam the operator cannot see the disk, cannot see the writer die,
//! and has no verb to stop it (remote stream-to-disk is a committed
//! requirement that is not built). The daemon therefore FORCES `auto_start = false`
//! when it translates either profile, and the wire has no field an older or
//! hand-written client could use to flip it back. The three capture VERBS exist on
//! both command groups and answer a named refusal - see [`ShockCommand::StartCapture`].
//!
//! # `max_drive_vpk` is required on both
//!
//! The operator's drive ceiling, volts PEAK, defaulted to
//! [`DEFAULT_SHOCK_MAX_DRIVE_VPK`]. It is the Explorer's posture applied to a transient
//! producer: the ceiling is a number a human chose, it is enforced whatever the station
//! is, and a profile that cleared it is REFUSED rather than run uncapped.
//!
//! # `Command::Start` means ARM here
//!
//! A shock run has no continuous "running" state to start - it arms, and then pulses
//! are FIRED one verb at a time. So the general [`crate::Command::Start`] maps to ARM
//! on the Shock and Srs dispatch arms, and [`ShockCommand::FirePulse`] /
//! [`ShockCommand::RunAuto`] are what make the machine move. The Explorer set the
//! precedent that `Start` means "whatever this module does when the operator says go";
//! stating it here is what stops a UI from waiting for a `Running` that never arrives.

use serde::{Deserialize, Serialize};

use crate::caps::ActionKind;
use crate::command::{profile_schema, FeasibilityInputs};

/// The default operator drive ceiling for a shock profile, volts PEAK.
///
/// Stated as a constant rather than inlined because two profiles and the daemon's
/// translation all have to mean the same number, and a ceiling that differs between
/// the two shock tests would be the worst kind of surprise.
pub const DEFAULT_SHOCK_MAX_DRIVE_VPK: f64 = 5.0;

/// The serde default for both profiles' `max_drive_vpk`.
fn default_max_drive_vpk() -> f64 {
    DEFAULT_SHOCK_MAX_DRIVE_VPK
}

/// The serde default for [`SrsProfile::review_after_sysid`]: ON.
///
/// Defaulted to the SAFE value, exactly like the sine review hold: a stored profile
/// written before the field existed gets the hold rather than driving straight through
/// a passing system ID.
fn default_review_after_sysid() -> bool {
    true
}

// =====================================================================================
// Classical shock
// =====================================================================================

/// A CLASSICAL SHOCK test, as the operator authored it (v5).
///
/// The wire mirror of `eigen_shock_host::service::config::ClassicalTestConfig`,
/// minus the capture block and plus `max_drive_vpk` - see the module docs for both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassicalShockProfile {
    /// The profile-schema version this was written at. See [`crate::PROFILE_SCHEMA`].
    #[serde(default = "profile_schema")]
    pub schema_version: u32,
    /// The pulse: shape, amplitude, duration, and the host-side compensation.
    pub spec: ShockPulseSpec,
    /// REQUESTED synthesis / plant sample rate, Hz.
    ///
    /// A request, never a measurement: the station coerces it up its own ladder and
    /// the run reports what it actually got through
    /// [`crate::StatusReport::actual_fs_hz`]. Every duration in this profile is stated
    /// in TIME (milliseconds, seconds), so a coerced rate re-derives the sample counts
    /// without changing what the operator asked for.
    pub fs_hz: f64,
    /// Which standard's tolerance parameters the per-pulse checks apply.
    pub standard: ShockToleranceStandard,
    /// The level schedule.
    pub schedule: ShockLevelSchedule,
    /// The pulse-train specification, fired by [`ShockCommand::FireTrain`].
    pub train: ShockTrainSpec,
    /// Gunfire settings, fired by [`ShockCommand::FireGunfireBurst`]. `None` = no
    /// gunfire configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gunfire: Option<ShockGunfireSpec>,
    /// The rate model whose floors set the minimum legal inter-pulse spacing.
    ///
    /// AN OPERATOR / SITE INPUT, deliberately on the wire rather than a hidden core
    /// default. The floors are cooling, settling and compute times for THIS shaker in
    /// THIS room; a default buried in the daemon would silently govern how fast a
    /// train may fire, and a train refused by a number nobody can see is a refusal
    /// nobody can act on.
    pub rate_model: ShockRateModel,
    /// Predictive peak-accel ceiling and the between-pulse monitor channels.
    pub drive_limit: ShockDriveLimit,
    /// The shaker record and moving-mass stack for the feasibility gate. `None` =
    /// feasibility NOT evaluated; arming is allowed with a logged note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feasibility: Option<FeasibilityInputs>,
    /// Compensation-quality residual-velocity tolerance, in/s.
    pub residual_velocity_tol_ips: f64,
    /// Compensation-quality residual-displacement tolerance, in.
    pub residual_disp_tol_in: f64,
    /// SRS analysis grid low edge, Hz.
    pub srs_f_lo_hz: f64,
    /// SRS analysis grid high edge, Hz.
    pub srs_f_hi_hz: f64,
    /// SRS band density: the `N` of the geometric ratio `2^(1/N)` (3 = third-octave).
    pub srs_bands_per_octave: f64,
    /// Per-band SRS fidelity tolerance, dB. INFORMATIONAL - the time-domain checks are
    /// the classical acceptance.
    pub srs_band_tol_db: f64,
    /// Also compute the pseudo-velocity SRS per pulse.
    pub compute_pvss: bool,
    /// Abort the run on a failed tolerance report. `false` = alarm-only: a breach
    /// raises the indicator and an event, and firing continues.
    pub abort_on_tolerance_failure: bool,
    /// The operator's drive ceiling, volts PEAK. See the module docs.
    #[serde(default = "default_max_drive_vpk")]
    pub max_drive_vpk: f64,
    /// User metadata carried into the report.
    #[serde(default)]
    pub metadata: Vec<ShockMetaField>,
}

/// The classical pulse: shape, amplitude, duration, and host-side compensation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShockPulseSpec {
    /// The pulse shape.
    pub shape: ShockPulseShape,
    /// Peak acceleration, g.
    pub amplitude_g: f64,
    /// Main-pulse duration, MILLISECONDS.
    pub duration_ms: f64,
    /// Host-side compensation configuration. Ignored on the delegated path, which
    /// downloads the uncompensated main pulse and lets the device compensate.
    pub compensation: ShockCompensation,
}

/// The classical pulse shape.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShockPulseShape {
    /// Half-sine.
    HalfSine,
    /// Haversine (versed sine): zero endpoint slope.
    Haversine,
    /// Terminal-peak sawtooth: linear ramp up to the peak at the last sample.
    TerminalPeakSawtooth,
    /// Initial-peak sawtooth: peak at the first sample, linear ramp down.
    InitialPeakSawtooth,
    /// Trapezoid: linear rise, plateau, linear fall.
    Trapezoid {
        /// Fraction of the duration spent rising.
        rise_frac: f64,
        /// Fraction of the duration spent falling.
        fall_frac: f64,
    },
    /// Nominally flat-top rectangle with bounded-slew rise/fall edges.
    Rectangle,
}

/// Host-side pulse compensation: the shoulders that null the pulse's residual
/// velocity and displacement.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShockCompensation {
    /// Shoulder shape; both sides use the same one.
    pub shape: ShockCompensationShape,
    /// Pre-shoulder duration as a fraction of the main-pulse duration.
    pub pre_frac: f64,
    /// Post-shoulder duration as a fraction of the main-pulse duration. Ignored when
    /// `symmetric`.
    pub post_frac: f64,
    /// Mirror the pre-shoulder geometry onto the post side.
    pub symmetric: bool,
}

/// The compensation shoulder shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShockCompensationShape {
    /// Half-sine shoulder: smooth, zero at both ends.
    HalfSine,
    /// Rectangular shoulder: the lowest peak amplitude for a duration, with step edges.
    Rectangular,
    /// Rectangle with raised-cosine tapers: near-rectangular efficiency, bounded slew.
    RoundedRectangular,
}

/// Which standard's tolerance parameters the per-pulse checks apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShockToleranceStandard {
    /// MIL-STD-810 Method 516.
    Mil810_516,
    /// IEC 60068-2-27.
    Iec60068_2_27,
}

// =====================================================================================
// Shared shock configuration (both profiles)
// =====================================================================================

/// The shock level schedule. Levels are dB relative to full (`0` = the full reference
/// pulse).
///
/// Shared by both shock profiles because it IS one thing: the shock analogue of
/// Random's level ladder with the dwell replaced by a pulse count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShockLevelSchedule {
    /// Who advances the ladder.
    pub mode: ShockRunMode,
    /// The engage/start level, dB.
    pub start_db: f64,
    /// The AUTO level table, dB. Non-empty and non-decreasing.
    pub table_db: Vec<f64>,
    /// Pulses fired per table level in AUTO: one entry per level, or a SINGLE value
    /// applied to every level.
    pub pulses_per_level: Vec<u32>,
}

/// Who advances the shock ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShockRunMode {
    /// AUTO: fire the configured pulses at each table level, advancing up the table.
    Auto,
    /// MANUAL: the operator steps and jumps levels; a fired pulse advances nothing.
    Manual,
}

/// A pulse-train specification: how many pulses and how they are spaced.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShockTrainSpec {
    /// Number of pulses to fire, `>= 1`.
    pub count: u32,
    /// Inter-pulse spacing.
    pub spacing: ShockSpacing,
}

/// How pulses in a train are spaced.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShockSpacing {
    /// A fixed delay between pulse STARTS, seconds. Below the rate model's floor this
    /// is REFUSED, with both numbers - never quietly stretched.
    Delay(f64),
    /// As fast as the hardware allows: the realized spacing IS the rate model's floor.
    MaxRate,
}

/// Gunfire burst settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShockGunfireSpec {
    /// Firing rate, rounds per minute.
    pub rounds_per_min: f64,
    /// Rounds per burst.
    pub rounds: u32,
}

/// The inter-pulse rate model: the floors whose MAXIMUM is the minimum legal spacing.
///
/// See [`ClassicalShockProfile::rate_model`] for why this crosses the wire instead of
/// living as a core default.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShockRateModel {
    /// Armature/coil cooling floor, seconds.
    pub cooling_floor_s: f64,
    /// Fixture ring-down settling floor, seconds.
    pub settling_floor_s: f64,
    /// SRS/capture compute floor, seconds.
    pub compute_floor_s: f64,
    /// On-device per-pulse compensation-recompute floor, seconds. Applies on the
    /// DELEGATED path only.
    pub device_recompute_floor_s: f64,
    /// Whether the read cadence can be sustained. `false` makes the model refuse EVERY
    /// spacing rather than answer a plausible-looking number.
    pub read_cadence_ok: bool,
}

/// Drive limiting: an optional predictive peak-accel ceiling plus monitored channels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShockDriveLimit {
    /// Predictive peak-acceleration ceiling, g. `None` = no accel ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_accel_g: Option<f64>,
    /// The monitored channels.
    #[serde(default)]
    pub monitors: Vec<ShockDriveMonitor>,
}

/// One monitored channel with a peak ceiling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShockDriveMonitor {
    /// The channel-table channel NAME this monitor reads.
    pub channel: String,
    /// The channel's engineering unit as a classification string (`"V"`, `"A"`, `"g"`).
    pub eu: String,
    /// The peak ceiling in the channel's EU. Finite and `> 0`.
    pub peak_ceiling: f64,
    /// When the ceiling is enforced.
    pub kind: ShockMonitorKind,
}

/// When a drive monitor's ceiling is enforced. The two timings are NOT
/// interchangeable and the distinction is on the wire because it changes what the
/// monitor can promise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShockMonitorKind {
    /// PRE-FIRE, from a predicted peak. Needs a measured drive-to-channel FRF.
    Predictive,
    /// BETWEEN pulses, from the MEASURED peak of the last one. The first breach has
    /// already fired by the time it is caught: this is enforcement, not prediction.
    BetweenPulse,
}

/// One user metadata field carried into a shock report.
///
/// Three fields, unlike [`crate::sine::SineMetaField`]'s two: the shock services carry
/// a human `label` beside the machine `key`. Mirrored as they are rather than
/// normalized, because the report prints the label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShockMetaField {
    /// Stable machine key.
    pub key: String,
    /// Human label.
    pub label: String,
    /// Freeform value.
    pub value: String,
}

// =====================================================================================
// SRS
// =====================================================================================

/// An SRS (shock response spectrum) synthesis test, as the operator authored it (v5).
///
/// The wire mirror of `eigen_srs::service::config::SrsTestConfig`, minus the capture
/// block and plus `max_drive_vpk` - see the module docs for both.
///
/// **No `fs_hz` field, deliberately.** Classical states a requested rate because its
/// pulse is authored in the time domain; an SRS run is authored as a SPECTRUM and the
/// synthesis happens at whatever rate the station adopted. Carrying a request the
/// operator never made would put a number on the panel that means nothing, so the SRS
/// panel shows the ACTUAL rate only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SrsProfile {
    /// The profile-schema version this was written at. See [`crate::PROFILE_SCHEMA`].
    #[serde(default = "profile_schema")]
    pub schema_version: u32,
    /// Analysis band low edge, Hz.
    pub f_lo_hz: f64,
    /// Analysis band high edge, Hz.
    pub f_hi_hz: f64,
    /// Fractional-octave band spacing.
    pub fraction: SrsBandFraction,
    /// The target SRS.
    pub target: SrsTarget,
    /// SDOF quality factor for all SRS analysis (`zeta = 1/(2Q)`).
    pub q: f64,
    /// Which synthesis engine drives the run.
    pub engine: SrsEngineKind,
    /// The offline SRS-matching iteration knobs.
    pub synthesis: SrsSynthesisOpts,
    /// The level ladder.
    pub schedule: ShockLevelSchedule,
    /// Predictive-notch limit channels. `None` = no limiting.
    ///
    /// Carried for the same reason as [`crate::RandomProfile::limits`]: a limit
    /// authored on screen and silently dropped on the remote path is an operator who
    /// believes a fixture is protected when it is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<SrsLimitSet>,
    /// The shaker record and moving-mass stack for the feasibility gate. `None` =
    /// feasibility NOT evaluated; arming is allowed with a logged note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feasibility: Option<FeasibilityInputs>,
    /// Compensation-quality residual tolerances for the feasibility gate.
    pub feasibility_tol: SrsFeasibilityTol,
    /// Drive peak ceilings.
    pub drive_limit: ShockDriveLimit,
    /// Measured-over-ceiling abort factor for the drive monitors.
    pub drive_abort_factor: f64,
    /// Per-pulse kinematic abort ceilings.
    pub kinematic_abort: SrsKinematicAbort,
    /// The tolerance-envelope specification.
    pub envelope: SrsEnvelopeSpec,
    /// Reduced-level system ID. `None` = SKIPPED: the drive seed assumes a unity plant
    /// (logged honestly) and predictive drive-accel limiting is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sysid: Option<SrsSysIdSpec>,
    /// Also compute the pseudo-velocity SRS per pulse.
    pub compute_pvss: bool,
    /// Ring-out window captured after each pulse's drive, seconds.
    pub ring_out_s: f64,
    /// HOLD after a PASSING system ID for operator review, before anything drives.
    /// **Default ON.**
    ///
    /// Entering the hold PARKS the hardware - the tasks stop and
    /// nothing sits reserved-and-running through a minutes-long review - and
    /// [`crate::Command::ProceedAfterReview`] re-arms and releases it.
    #[serde(default = "default_review_after_sysid")]
    pub review_after_sysid: bool,
    /// The operator's drive ceiling, volts PEAK. See the module docs.
    #[serde(default = "default_max_drive_vpk")]
    pub max_drive_vpk: f64,
    /// User metadata carried into the report.
    #[serde(default)]
    pub metadata: Vec<ShockMetaField>,
}

/// Fractional-octave band spacing for the SRS analysis grid.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrsBandFraction {
    /// Whole octave (`N = 1`).
    Whole,
    /// One-third octave (`N = 3`), the classic default.
    Third,
    /// One-sixth octave (`N = 6`).
    Sixth,
    /// One-twelfth octave (`N = 12`).
    Twelfth,
    /// Arbitrary `N`, ratio `2^(1/N)`.
    Arbitrary(f64),
}

/// The target SRS specification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrsTarget {
    /// A flat target: every band demands `accel_g`.
    Flat {
        /// The per-band SRS demand, g.
        accel_g: f64,
    },
    /// A breakpoint table `(freq_hz, accel_g)`, interpolated LOG-LOG onto the band
    /// grid - the standard SRS-spec convention (straight lines on a log-log plot).
    ///
    /// Carried as the AUTHORED points, not pre-interpolated: the band grid is a
    /// function of `f_lo_hz`/`f_hi_hz`/`fraction`, so the only end that can place them
    /// correctly is the one that owns the grid.
    Breakpoints {
        /// Ascending `(freq_hz, accel_g)` points covering the analysis band.
        points: Vec<(f64, f64)>,
    },
}

/// Which synthesis engine drives an SRS run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrsEngineKind {
    /// WAVSYN odd-half-cycle wavelets: self-nulling, no compensation needed.
    Wavsyn,
    /// SEDS decaying sinusoids with Smallwood-Nord compensation.
    Seds,
}

/// The offline SRS-matching iteration knobs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SrsSynthesisOpts {
    /// Maximum outer-loop iterations before returning the best-so-far.
    pub max_iters: usize,
    /// Convergence tolerance on the worst per-band `|error|`, dB.
    pub tol_db: f64,
    /// Per-iteration per-component amplitude clamp, dB.
    pub max_gain_db_per_iter: f64,
    /// SDOF quality factor `Q` for the iteration's SRS analysis.
    pub q: f64,
}

/// The predictive-notch limit set: the channels plus the prediction headroom.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SrsLimitSet {
    /// The limited channels.
    pub channels: Vec<SrsLimitChannel>,
    /// Prediction headroom, dB (`>= 0`): predictions must stay this far BELOW the
    /// bound, budgeting the zero-phase FRF prediction's slack.
    pub headroom_db: f64,
}

/// One limit channel: a non-control location whose PREDICTED response SRS must stay
/// under a bound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SrsLimitChannel {
    /// The channel-table channel NAME.
    pub name: String,
    /// The bound.
    pub limit: SrsLimitTarget,
    /// A sampled drive-to-channel FRF magnitude, when one has been measured. `None` =
    /// no prediction is possible for this channel and the core says so rather than
    /// assuming unity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frf: Option<SrsFrfSpec>,
}

/// How a limit channel states its bound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrsLimitTarget {
    /// Every band bounded at one level.
    Flat {
        /// The bound, g.
        accel_g: f64,
    },
    /// One bound per analysis band; the length must match the band grid.
    PerBand {
        /// Per-band bounds, g.
        bounds_g: Vec<f64>,
    },
}

/// A sampled FRF magnitude `|H(f)|` (drive to channel) on an ascending grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SrsFrfSpec {
    /// Sample frequencies, Hz, ascending.
    pub freqs_hz: Vec<f64>,
    /// `|H|` per sample.
    pub mag: Vec<f64>,
}

/// Compensation-quality residual tolerances for the SRS feasibility gate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SrsFeasibilityTol {
    /// Residual-velocity tolerance, in/s.
    pub residual_velocity_ips: f64,
    /// Residual-displacement tolerance, in.
    pub residual_disp_in: f64,
}

/// Per-pulse kinematic abort ceilings, checked against every ingested pulse's measured
/// kinematics. `None` on an axis = no ceiling there.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SrsKinematicAbort {
    /// Peak acceleration ceiling, g.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accel_g: Option<f64>,
    /// Peak velocity ceiling, in/s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity_ips: Option<f64>,
    /// Peak-to-peak displacement ceiling, in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disp_pp_in: Option<f64>,
}

/// The tolerance envelope: uniform half-widths plus an optional full per-band override.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SrsEnvelopeSpec {
    /// Uniform alarm half-widths, dB `(low, high)`, POSITIVE magnitudes.
    pub alarm_db: (f64, f64),
    /// Uniform abort half-widths, dB `(low, high)`, POSITIVE magnitudes.
    pub abort_db: (f64, f64),
    /// Per-band override, one entry per band-grid point. When set, the uniform values
    /// above are IGNORED.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_band: Option<Vec<SrsToleranceBand>>,
}

/// One band's `+/- dB` tolerance half-widths. All POSITIVE magnitudes; the low side is
/// applied as `-lo_db`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SrsToleranceBand {
    /// Alarm low-side half-width, dB.
    pub alarm_lo_db: f64,
    /// Alarm high-side half-width, dB.
    pub alarm_hi_db: f64,
    /// Abort low-side half-width, dB.
    pub abort_lo_db: f64,
    /// Abort high-side half-width, dB.
    pub abort_hi_db: f64,
}

/// Reduced-level system-ID options.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SrsSysIdSpec {
    /// Excitation level, dB relative to full.
    pub level_db: f64,
    /// The excitation shape.
    pub excitation: SrsSysIdExcitation,
    /// FFT size for the FRF estimate; a power of two.
    pub nfft: usize,
    /// `|H|` trust floor as a fraction of the peak, for the feedforward seed.
    pub h_floor_frac: f64,
}

/// The system-ID excitation shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrsSysIdExcitation {
    /// SHAPED: the target-converged synthesis waveform scaled down - concentrates
    /// identification energy exactly where the test will drive.
    Shaped,
    /// STANDARD: a short low-level broadband pulse.
    StandardPulse,
}

// =====================================================================================
// The command groups
// =====================================================================================

/// The CLASSICAL SHOCK verbs (v5).
///
/// A transient producer needs verbs the continuous modules do not: there is no
/// "running" to be in, so firing is an explicit act each time. [`crate::Command::Start`]
/// ARMS (see the module docs) and everything below is what happens afterwards.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShockCommand {
    /// Fire ONE pulse at the current level. MUTATING.
    FirePulse,
    /// Run the AUTO ladder: fire the scheduled pulses at each level, advancing up the
    /// table. MUTATING.
    RunAuto {
        /// Stop after this many pulses regardless of the schedule. `None` = run the
        /// schedule to completion.
        ///
        /// A bound rather than an open-ended run, for the same reason the Explorer's
        /// run loop is bounded: an open-ended firing loop with a remote stop is a loop
        /// whose end depends on the network.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_pulses: Option<u32>,
    },
    /// Jump to a level-table index. MUTATING.
    JumpToLevel {
        /// The zero-based index into the schedule's `table_db`.
        index: usize,
    },
    /// ARM past a FEASIBILITY refusal, with an explicit acknowledgement. MUTATING.
    ///
    /// `ack` is the operator's signed statement that they read the reason and are
    /// proceeding anyway; an EMPTY ack is refused, because a required field nobody has
    /// to fill in is not a gate.
    ArmWithOverride {
        /// The acknowledgement text. Must be non-empty.
        ack: String,
    },
    /// Clear the run state back to unarmed. MUTATING.
    Reset,
    /// List the fired pulses - ROWS ONLY, not the time histories. Read-only.
    ///
    /// Answers [`crate::Response::Shock`] with a [`crate::Doc`] at
    /// [`crate::SchemaKind::ShockPulse`]. The split from `PulseDetail` is a byte-budget
    /// decision: a pulse list is a table an operator scrolls, and shipping every
    /// record's samples to render it would put megabytes on the command path.
    PulseList,
    /// Fetch ONE fired pulse in full, by sequence number. Read-only.
    PulseDetail {
        /// The pulse sequence number, as reported by
        /// [`crate::StatusReport::last_pulse_seq`] or by `PulseList`.
        seq: u64,
    },
    /// Fetch the SRS analysis grid this run is using. Read-only.
    ///
    /// A QUERY rather than configure-time-only output, because the two peers who most
    /// need it never saw the configure: an observer that joined mid-run, and a
    /// controller that reconnected after a vanish. Without it their panels have
    /// spectra and no frequency axis to draw them on.
    SrsFreqs,
    /// Fire the configured pulse TRAIN, or an explicit one. MUTATING.
    FireTrain {
        /// The train to fire. `None` = the profile's configured train.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spec: Option<ShockTrainSpec>,
    },
    /// Fire the configured gunfire burst. MUTATING.
    FireGunfireBurst,
    /// Start a durable capture on the core host. MUTATING.
    ///
    /// **PRESENT AND REFUSED**. The verb exists so the refusal can be NAMED: an
    /// absent verb decodes as a malformed payload, which reads to an operator as "your
    /// client is broken" rather than "this is not served yet". Remote stream-to-disk is
    /// a committed requirement that is not built - the disk-budget model for an
    /// unbounded run, and the fetch story for captures that land on the box, both have
    /// to be designed first.
    StartCapture,
    /// Stop a durable capture on the core host. MUTATING. **PRESENT AND REFUSED**
    /// - see [`ShockCommand::StartCapture`].
    StopCapture,
    /// Save a snapshot of the current run state to the core host. MUTATING.
    /// **PRESENT AND REFUSED** - see [`ShockCommand::StartCapture`].
    SaveSnapshot,
}

impl ShockCommand {
    /// Does this verb change core state? The Observer gate consults
    /// [`crate::Command::is_mutating`], which consults this.
    ///
    /// The three capture verbs count as MUTATING even though the daemon refuses them
    /// today: the classification answers what the verb IS, not whether it is currently
    /// served, and the day remote capture lands they write files on the core host. A verb
    /// classified read-only because its implementation happens to be a refusal is a
    /// verb that becomes Observer-reachable the moment someone implements it.
    pub fn is_mutating(&self) -> bool {
        match self {
            ShockCommand::FirePulse
            | ShockCommand::RunAuto { .. }
            | ShockCommand::JumpToLevel { .. }
            | ShockCommand::ArmWithOverride { .. }
            | ShockCommand::Reset
            | ShockCommand::FireTrain { .. }
            | ShockCommand::FireGunfireBurst
            | ShockCommand::StartCapture
            | ShockCommand::StopCapture
            | ShockCommand::SaveSnapshot => true,
            ShockCommand::PulseList | ShockCommand::PulseDetail { .. } | ShockCommand::SrsFreqs => {
                false
            }
        }
    }

    /// The [`ActionKind`] this verb is gated by, or `None` for the read-only queries
    /// and the refused capture verbs (which have no matrix row and must not claim one).
    ///
    /// MAPPED, never matrixed - the same trade as [`crate::ExplorerCommand::action_kind`].
    pub fn action_kind(&self) -> Option<ActionKind> {
        match self {
            // Everything that makes the machine move is gated like Start: on a
            // transient producer, firing IS the run.
            ShockCommand::FirePulse
            | ShockCommand::RunAuto { .. }
            | ShockCommand::FireTrain { .. }
            | ShockCommand::FireGunfireBurst
            | ShockCommand::ArmWithOverride { .. } => Some(ActionKind::Start),
            ShockCommand::JumpToLevel { .. } => Some(ActionKind::SetLevel),
            // Reset is the run ENDING and the state clearing, which is Stop's gate.
            ShockCommand::Reset => Some(ActionKind::Stop),
            ShockCommand::PulseList
            | ShockCommand::PulseDetail { .. }
            | ShockCommand::SrsFreqs
            | ShockCommand::StartCapture
            | ShockCommand::StopCapture
            | ShockCommand::SaveSnapshot => None,
        }
    }
}

/// The SRS verbs (v5).
///
/// The classical set minus the two train verbs (an SRS run fires synthesized
/// waveforms, not pulse trains) plus [`SrsCommand::RunSysId`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrsCommand {
    /// Fire ONE synthesized pulse at the current level. MUTATING.
    FirePulse,
    /// Run the AUTO ladder. MUTATING.
    RunAuto {
        /// Stop after this many pulses regardless of the schedule. `None` = run the
        /// schedule to completion.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_pulses: Option<u32>,
    },
    /// Jump to a level-table index. MUTATING.
    JumpToLevel {
        /// The zero-based index into the schedule's `table_db`.
        index: usize,
    },
    /// ARM past a FEASIBILITY refusal, with an explicit acknowledgement. MUTATING.
    /// See [`ShockCommand::ArmWithOverride`].
    ArmWithOverride {
        /// The acknowledgement text. Must be non-empty.
        ack: String,
    },
    /// Clear the run state back to unarmed. MUTATING.
    Reset,
    /// Run the reduced-level system ID. MUTATING.
    ///
    /// It DRIVES - one low-level fire - which is why it is gated like Start and why a
    /// passing result parks at the review hold rather than proceeding
    /// ([`SrsProfile::review_after_sysid`]).
    RunSysId,
    /// List the fired pulses - rows only. Read-only. See [`ShockCommand::PulseList`].
    PulseList,
    /// Fetch ONE fired pulse in full, by sequence number. Read-only.
    PulseDetail {
        /// The pulse sequence number.
        seq: u64,
    },
    /// Fetch the SRS analysis grid this run is using. Read-only. See
    /// [`ShockCommand::SrsFreqs`] for why it is a query.
    SrsFreqs,
    /// Fetch the PREDICTIVE-NOTCH readout. Read-only. **ADDITIVE within v5**
    /// (2026-08-09) - see [`crate::SHOCK_NOTCH_SCHEMA`].
    ///
    /// Answers a [`crate::Doc`] at [`crate::SchemaKind::ShockNotch`] carrying the
    /// arm-time notch plan, the limit report, and the predicted limit-channel SRS
    /// curves - the same three the local core's `shock_srs_notch_status` returns.
    ///
    /// # Why a verb rather than a field on something already crossing
    ///
    /// It was checked, and neither of the two candidates fits. `PulseDetail{seq}` is
    /// PER-PULSE and the notch plan is RUN state, computed once at arm. `PulseList` is
    /// run-scoped and would have avoided a new verb, but a front end polls this readout
    /// beside the status (5 Hz on the local path) and `PulseList` carries up to 512
    /// rows - a notch poll that dragged the whole pulse table across the link every
    /// 200 ms would be a worse answer than the empty plan it replaces.
    ///
    /// # Why the version does NOT move
    ///
    /// This is a QUERY, and its answer is a new [`crate::SchemaKind`] variant. The v5
    /// one-directional additivity invariant is that a new `Response` / `Event` /
    /// `SchemaKind` variant may only be emitted in reply to a command that only a NEW
    /// peer can send - which is exactly this one, so the invariant is satisfied by
    /// construction rather than by care.
    ///
    /// The direction that is genuinely new is the COMMAND: a v5 peer built after this
    /// can send a verb a v5 daemon built before it cannot decode. Deploy order is
    /// DAEMON FIRST, so that ordering is the abnormal one, and when it does happen the
    /// old daemon answers a typed decode refusal and the client falls back to the empty
    /// plan it rendered before this verb existed. That is a degradation an operator can
    /// read, on a READ-ONLY verb that points nothing at a shaker. A bump would have cost
    /// a coordinated redeploy of both ends for a display readout.
    NotchStatus,
    /// Start a durable capture on the core host. MUTATING. **PRESENT AND REFUSED**
    /// - see [`ShockCommand::StartCapture`].
    StartCapture,
    /// Stop a durable capture on the core host. MUTATING. **PRESENT AND REFUSED**
    /// - see [`ShockCommand::StartCapture`].
    StopCapture,
    /// Save a snapshot of the current run state. MUTATING. **PRESENT AND REFUSED**
    /// - see [`ShockCommand::StartCapture`].
    SaveSnapshot,
}

impl SrsCommand {
    /// Does this verb change core state? See [`ShockCommand::is_mutating`], including
    /// why the refused capture verbs are classified as mutating.
    pub fn is_mutating(&self) -> bool {
        match self {
            SrsCommand::FirePulse
            | SrsCommand::RunAuto { .. }
            | SrsCommand::JumpToLevel { .. }
            | SrsCommand::ArmWithOverride { .. }
            | SrsCommand::Reset
            | SrsCommand::RunSysId
            | SrsCommand::StartCapture
            | SrsCommand::StopCapture
            | SrsCommand::SaveSnapshot => true,
            SrsCommand::PulseList
            | SrsCommand::PulseDetail { .. }
            | SrsCommand::SrsFreqs
            | SrsCommand::NotchStatus => false,
        }
    }

    /// The [`ActionKind`] this verb is gated by, or `None`. See
    /// [`ShockCommand::action_kind`].
    pub fn action_kind(&self) -> Option<ActionKind> {
        match self {
            SrsCommand::FirePulse
            | SrsCommand::RunAuto { .. }
            | SrsCommand::ArmWithOverride { .. }
            | SrsCommand::RunSysId => Some(ActionKind::Start),
            SrsCommand::JumpToLevel { .. } => Some(ActionKind::SetLevel),
            SrsCommand::Reset => Some(ActionKind::Stop),
            SrsCommand::PulseList
            | SrsCommand::PulseDetail { .. }
            | SrsCommand::SrsFreqs
            | SrsCommand::NotchStatus
            | SrsCommand::StartCapture
            | SrsCommand::StopCapture
            | SrsCommand::SaveSnapshot => None,
        }
    }
}
