//! The CLOSED-LOOP SINE wire surface (v5): [`SineWireProfile`] and [`SineCommand`].
//!
//! # What this is
//!
//! A field-for-field WIRE MIRROR of `hertzforge_sine::service::config::SineTestConfig` -
//! the operator surface of a closed-loop swept-sine test - plus the five sine verbs
//! the general command set cannot express. It is a new [`crate::TestProfile`] variant,
//! not a grown [`crate::SineProfile`]: that type is the DEAD delegated open-loop payload
//! and is `deny_unknown_fields`, so growing it would be a mid-workflow decode failure
//! on an older peer rather than an additive change (see `PROTOCOL_CHANGELOG.md` v4
//! item 1 for the same argument at the Explorer).
//!
//! # Why a MIRROR and not an opaque [`crate::Doc`]
//!
//! The recorder configuration crosses as an opaque document because this crate cannot
//! import `hertzforge-recorder` and mirroring a live document with its own migration story
//! would be a bad trade. A sine test configuration is the opposite case: it is the
//! payload that POINTS A SHAKER, every field is a scalar or a small enum, and the
//! whole value of the seam is that a misparse is impossible rather than merely
//! unlikely. Typed, `deny_unknown_fields`, exact-schema-gated - the
//! [`crate::FeasibilityInputs`] precedent, applied to a whole configuration.
//!
//! # The FIDELITY FENCE (a past lesson, stated as a build rule)
//!
//! A past bug class: a UI-authored safety feature (a notch table, a control
//! weight, a coherence floor) was edited on screen and silently DEFAULTED on the
//! remote path: the operator authored a ceiling and got no ceiling, with no
//! indication. The rule that keeps sine out of that hole is not a test, it is a
//! shape:
//!
//! 1. **Every operator-settable field of `SineTestConfig` appears here.** A field that
//!    exists on one side and not the other is the defect.
//! 2. **The daemon's wire -> config conversion destructures BOTH structs exhaustively**
//!    (no `..` on either side), so a field added to either is a COMPILE ERROR in the
//!    converter rather than a silent default at run time.
//! 3. **`tests/v5.rs` round-trips the whole profile** including a fully-populated
//!    optional set, so a serializer that drops a field reds.
//!
//! The converter itself lands with the `SineHost`; this module owns halves 1 and 3.
//!
//! # The two DELIBERATE DUPLICATE definitions, named
//!
//! [`SineSweepRate`] is the THIRD definition of the sweep-rate law in the tree
//! (`hertzforge_dsp::sweep::SweepRate` is the real one; `hertzforge_sine::scheduler`'s
//! `SweepRateDef` serde remote-derive is the second) and [`SineBandwidthMode`] is the
//! third of the tracking bandwidth (`hertzforge_dsp::tracking::BandwidthMode` and the
//! `BandwidthModeDef` remote-derive in `hertzforge_sine::service::config`). Both are
//! ACCEPTED drift points, not oversights: this crate's dependency floor is serde and
//! nothing else, so it cannot import `hertzforge-dsp`, and a remote-derive would need the
//! import just the same. The fence is the round-trip corpus in `tests/roundtrip.rs`
//! plus the exhaustive converter above - a variant added to the dsp enum and not here
//! stops the converter compiling.
//!
//! [`crate::SweepDirection`] is NOT duplicated: the Explorer's mirror is the same two
//! variants with the same meaning, and a fourth `Up`/`Down` enum would be a drift
//! point with nothing to gain.

use serde::{Deserialize, Serialize};

use crate::caps::ActionKind;
use crate::command::{profile_schema, FeasibilityInputs, SweepDirection};

// =====================================================================================
// The profile
// =====================================================================================

/// A CLOSED-LOOP SINE test, as the operator authored it (v5).
///
/// The wire mirror of `hertzforge_sine::service::config::SineTestConfig` - see the module
/// docs for the fidelity fence that keeps the two in step, and for why this is a
/// typed mirror rather than an opaque document.
///
/// `schema_version` is the ONLY field with no counterpart on the config side: it is
/// this crate's exact-match profile gate ([`crate::PROFILE_SCHEMA`]), carried on the
/// outermost payload only. The nested types below do not repeat it - one document,
/// one version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineWireProfile {
    /// The profile-schema version this was written at. See [`crate::PROFILE_SCHEMA`].
    #[serde(default = "profile_schema")]
    pub schema_version: u32,
    /// The CONTROL profile: the frequency/level breakpoint table the servo controls to.
    pub profile: SineControlProfile,
    /// Which scheduler drives the frequency, and its configuration.
    pub mode: SineModeSpec,
    /// The control-channel set and how the channels composite into one scalar.
    pub control: SineControlSet,
    /// Per-channel LIMIT profiles (the notch tables). Empty = no limiting.
    ///
    /// Carried as the AUTHORED rows exactly like [`crate::RandomProfile::limits`], and
    /// for the same reason: the end that owns the run's grid is the only end that may
    /// interpolate onto it.
    #[serde(default)]
    pub limit_profiles: Vec<SineLimitProfile>,
    /// The Hinv-feedforward vs feedback blend weight, `[0, 1]`.
    pub blend_w: f64,
    /// The compression preset (the servo's integral gain and rate limit).
    pub compression: SineCompression,
    /// The tracking-filter bandwidth law.
    pub tracking_bw: SineBandwidthMode,
    /// The feedforward trust mask (sysID coherence and `|H|` floors).
    pub feedforward: SineFeedforwardConfig,
    /// The notch governor's knobs.
    pub notch: SineNotchConfig,
    /// The level ladder.
    pub ladder: SineLadderConfig,
    /// The sysID drive level, dB relative to the full profile.
    pub sysid_level_db: f64,
    /// The band-coherence floor the sysID gate holds against, `[0, 1]`.
    pub coherence_floor: f64,
    /// Drive ceiling, volts PEAK - the clamp.
    pub drive_vpk_ceiling: f64,
    /// Drive abort trip, volts PEAK. Must be `>= drive_vpk_ceiling`.
    pub drive_vpk_abort: f64,
    /// Consecutive faulted control ticks before the debounced servo-fault abort.
    /// `0` disables the fault abort.
    pub fault_debounce_ticks: u32,
    /// Run the analytic kinematic monitor against the shaker's velocity and
    /// displacement limits.
    pub kinematic_monitor: bool,
    /// Capture rules. **Carried, and the remote capture VERBS are refused**:
    /// the rules describe what a run would capture on the CORE host, and Mac-initiated
    /// start/stop is not served over this seam yet. A configuration that crossed
    /// without the verbs to drive it is honest; a verb that silently did nothing is not.
    pub capture: SineCaptureConfig,
    /// The COLA reference-output configuration (the second drive channel).
    pub cola: SineColaConfig,
    /// The shaker record and moving-mass stack for feasibility and the kinematic
    /// monitor. `None` = no shaker selected: feasibility is SKIPPED with an advisory
    /// note and the kinematic monitor is disabled, which is exactly what every remote
    /// run did before [`FeasibilityInputs`] existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feasibility: Option<FeasibilityInputs>,
    /// The CONTROL-update cadence, frames. Distinct from the engine's acquisition
    /// block, which the station answers with at configure.
    pub control_tick_frames: usize,
    /// The acquisition rate this run ASKS the station for, Hz. `None` = the
    /// station's own rate.
    ///
    /// Mirrors `SineTestConfig::fs_hz`. It is a REQUEST: the device answers with
    /// `ConfiguredActual::actual_rate_hz` and the core adopts the answer, so this
    /// field can change what is asked for and never what the run believes it got.
    /// The acquisition BLOCK is not here at all - it stays station-derived, which
    /// is why the panel shows it and does not offer it.
    ///
    /// **`PROFILE_SCHEMA` does NOT move for this** (it stays 1). The field is
    /// `#[serde(default)]`, so a v5 profile written before it existed decodes to
    /// `None`, which is the behaviour it had; and `deny_unknown_fields` still bites
    /// in the other direction, which is the half that matters - a newer sender's
    /// typo is refused rather than silently dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs_hz: Option<f64>,
    /// HOLD at `AwaitingReview` after the coherence gate PASSES, until an operator
    /// releases it with [`crate::Command::ProceedAfterReview`]. **Default ON.**
    ///
    /// Not the same thing as the service's `CoherenceHold`, and the two must never
    /// share a release verb: `CoherenceHold` is the gate FAILING, and releasing it
    /// with a click would be a one-button bypass of a safety refusal. Only a
    /// `Reset` clears that one, and this seam deliberately offers no verb for it.
    #[serde(default = "default_review_hold")]
    pub review_hold: bool,
    /// User metadata carried into the report.
    #[serde(default)]
    pub metadata: Vec<SineMetaField>,
}

/// The serde default for [`SineWireProfile::review_hold`]: ON.
///
/// Defaulted to the SAFE value so a stored profile written before the field existed
/// gets the hold rather than silently running straight through a passing gate.
fn default_review_hold() -> bool {
    true
}

/// The sine CONTROL profile: ordered breakpoint rows plus the global alarm/abort bands.
///
/// Mirrors `hertzforge_sine::profile::SineProfile`. The name differs deliberately -
/// [`crate::SineProfile`] in this crate is the dead delegated open-loop payload and the two
/// must never be confusable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineControlProfile {
    /// Operator label for the table.
    #[serde(default)]
    pub label: String,
    /// The breakpoint rows, ascending in frequency.
    pub rows: Vec<SineProfileRow>,
    /// Global alarm band `(minus_db, plus_db)`, POSITIVE magnitudes.
    pub global_alarm_db: (f64, f64),
    /// Global abort band `(minus_db, plus_db)`, POSITIVE magnitudes.
    pub global_abort_db: (f64, f64),
}

/// One control-profile row: a frequency breakpoint carrying ONE quantity and value,
/// with optional per-row band overrides.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineProfileRow {
    /// Breakpoint frequency, Hz. Strictly positive and strictly increasing row to row.
    pub freq_hz: f64,
    /// Which physical quantity `value` states.
    pub quantity: SineQuantity,
    /// The level in `quantity`'s units. Strictly positive.
    pub value: f64,
    /// Per-row alarm band `(minus_db, plus_db)` override; `None` = the global band.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alarm_db: Option<(f64, f64)>,
    /// Per-row abort band `(minus_db, plus_db)` override; `None` = the global band.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort_db: Option<(f64, f64)>,
}

/// The physical quantity a control-profile row states.
///
/// Sine profiles mix quantities row to row (the classic low-frequency displacement
/// ramp into an acceleration plateau); the core converts each to acceleration by SHM
/// at the excitation frequency. Carried as the AUTHORED quantity rather than
/// pre-converted, because the conversion depends on the frequency the run actually
/// reaches and a pre-converted table would bake in the sender's assumed grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SineQuantity {
    /// Acceleration, `g` PEAK.
    AccelGPk,
    /// Velocity, `in/s` PEAK.
    VelocityIpsPk,
    /// Displacement, `in` PEAK-TO-PEAK.
    DisplacementInPp,
}

// =====================================================================================
// Mode (which scheduler drives the frequency)
// =====================================================================================

/// Which scheduler drives the frequency, and its configuration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SineModeSpec {
    /// Bidirectional sweep between the band edges.
    Sweep(SineSweepSpec),
    /// Dwell at one frequency.
    Dwell(SineDwellPlan),
    /// Step-sine: a frequency ladder with a dwell at each step.
    StepSine(SineStepSpec),
    /// Resonance search, track, dwell.
    Rstd(SineRstdSpec),
}

/// A bidirectional-sweep plan.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineSweepSpec {
    /// Sweep band `(lo, hi)`, Hz; `0 < lo < hi`.
    pub band_hz: (f64, f64),
    /// The sweep-rate law. The MAGNITUDE is used; direction comes from
    /// `initial_direction` and the band-edge bounces.
    pub rate: SineSweepRate,
    /// Starting frequency, inside the band.
    pub start_hz: f64,
    /// Which way the first pass travels.
    pub initial_direction: SweepDirection,
    /// When the sweep run is complete.
    pub target: SineSweepTarget,
}

/// The sweep-rate law. **The THIRD definition of this shape in the tree** - see the
/// module docs for why it is wire-local and what fences it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SineSweepRate {
    /// Logarithmic, octaves per minute (the standard sine-test law).
    LogOctPerMin(f64),
    /// Linear, Hz per second.
    LinearHzPerSec(f64),
}

/// When a bidirectional sweep run is complete.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SineSweepTarget {
    /// After N completed ONE-DIRECTION passes (up + down = 2).
    Sweeps(u32),
    /// After this much accumulated TEST time, seconds (holds excluded).
    DurationS(f64),
    /// At whichever of the two is met FIRST.
    WhicheverFirst {
        /// One-direction passes.
        sweeps: u32,
        /// Accumulated test time, seconds.
        duration_s: f64,
    },
}

/// A dwell instruction: hold one frequency for a duration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineDwellPlan {
    /// The dwell frequency, Hz.
    pub freq_hz: f64,
    /// How long. Named `dwell_for` because `for` is reserved.
    pub dwell_for: SineDwellFor,
}

/// A dwell duration: wall TEST time, or accumulated drive cycles. Standards need both.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SineDwellFor {
    /// Accumulated test seconds (holds excluded).
    Seconds(f64),
    /// Accumulated drive cycles.
    Cycles(u64),
}

/// A step-sine plan: a frequency ladder with a dwell at each step.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineStepSpec {
    /// Test band `(lo, hi)`, Hz.
    pub band_hz: (f64, f64),
    /// Steps per octave.
    pub steps_per_octave: f64,
    /// How the ladder is spaced.
    pub spacing: SineStepSpacing,
    /// Per-step dwell. `None` = the core's per-step default, which depends on the
    /// step frequency and is therefore not expressible as one number here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dwell: Option<SineDwellFor>,
}

/// Frequency spacing of a step-sine ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SineStepSpacing {
    /// Geometric steps (the classic step-sine grid).
    Log,
    /// Evenly spaced steps, sized by the same steps-per-octave knob.
    Linear,
}

/// A resonance search-track-dwell plan.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineRstdSpec {
    /// Search band `(lo, hi)`, Hz. Also the dwell tracker's clamp band.
    pub band_hz: (f64, f64),
    /// Search drive level, dB relative to the full profile.
    pub level_db: f64,
    /// Search sweep rate (magnitude; the search always sweeps UP from the low edge).
    pub rate: SineSweepRate,
    /// Tracking-filter bandwidth at the settle point, Hz.
    pub tracking_bw_hz: f64,
    /// Settle multiplier `k`: the settle hold is `k / tracking_bw_hz` seconds.
    pub settle_k: f64,
    /// What the dwell does after the setpoint is captured.
    pub dwell_mode: SineRstdDwellMode,
    /// How long the dwell runs.
    pub dwell_for: SineDwellFor,
}

/// Dwell behavior after the RSTD search and capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SineRstdDwellMode {
    /// Hold the captured peak frequency; no tracking.
    Fixed,
    /// Phase-locked tracking of the resonance on the captured setpoint.
    PhaseTracked,
}

// =====================================================================================
// Control set, limits, servo
// =====================================================================================

/// The control set: which channels the servo controls to, and how they composite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineControlSet {
    /// The control channels, in order. Must be non-empty.
    pub channels: Vec<SineControlChannel>,
    /// How the per-channel tracked amplitudes combine into one scalar.
    pub mode: SineCompositeMode,
}

/// One control channel: a channel-table index and its composite weight.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineControlChannel {
    /// The channel-table index this controls on.
    pub index: u32,
    /// Composite weight, strictly positive. Normalized by the composite.
    pub weight: f64,
}

/// How multiple control channels combine into one control amplitude.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SineCompositeMode {
    /// Weighted average - the standard multi-point average control.
    WeightedAvg,
    /// MIN of the weighted amplitudes: control until the QUIETEST point reaches target.
    Min,
    /// MAX of the weighted amplitudes: the protective mode - back off as soon as ANY
    /// point hits target.
    Max,
}

/// One channel's LIMIT profile - the notch table that channel is held under.
///
/// Carried as a whole [`SineControlProfile`] rather than a bare row list because a
/// sine limit IS a profile (rows in mixed quantities with their own bands), and
/// because sharing is by VALUE: N channels sharing one authored table cross as N
/// copies with the same `shared_name`, so a receiver never has to resolve a name
/// against a registry it may not have.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineLimitProfile {
    /// The channel-table index this limit applies to.
    pub channel_index: u32,
    /// The limit table, evaluated at the current frequency as a ceiling.
    pub profile: SineControlProfile,
    /// The shared-table name, for display and de-duplication. INFORMATIONAL: the
    /// carried `profile` is the source of truth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_name: Option<String>,
}

/// The compression preset: the servo's `(integral gain, max rate)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SineCompression {
    /// `(ki, max_rate) = (2.0, 20 dB/s)`.
    Slow,
    /// `(ki, max_rate) = (6.0, 60 dB/s)`.
    Fast,
    /// An explicit pair, for expert tuning.
    Custom {
        /// Integral gain.
        ki: f64,
        /// Rate limit, dB per second.
        max_rate_db_per_s: f64,
    },
}

/// The tracking-filter bandwidth law. **The THIRD definition of this shape in the
/// tree** - see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SineBandwidthMode {
    /// A fixed corner, Hz (still subject to the core's image-rejection clamp).
    FixedHz(f64),
    /// A proportional corner: `pct` percent of the drive frequency, capped at `max_hz`.
    Proportional {
        /// Percent of the drive frequency.
        pct: f64,
        /// Cap, Hz.
        max_hz: f64,
    },
}

/// The feedforward trust mask: which sysID bins the Hinv seed is allowed to believe.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineFeedforwardConfig {
    /// Per-bin coherence floor, `[0, 1]`.
    pub coherence_floor: f64,
    /// Per-bin `|H|` floor as a fraction of the grid peak, `[0, 1)`.
    pub h_floor_frac: f64,
}

/// The notch governor's knobs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineNotchConfig {
    /// Drive floor as a fraction of full drive scale, below which the measured
    /// sensitivity is too noise-dominated to trust and the sysID `|H|` is used instead.
    pub drive_floor: f64,
    /// One-pole smoothing time constant for the per-channel sensitivity, in control
    /// ticks.
    pub smoothing_ticks: f64,
    /// The release ramp rate the servo honors when a ceiling stops binding, dB/s.
    pub release_ramp_db_per_s: f64,
}

// =====================================================================================
// Ladder, capture, COLA, metadata
// =====================================================================================

/// The level ladder.
///
/// **AUTO vs MANUAL is NOT a field here** and that is deliberate. The ladder mode is
/// live-switchable mid-run through [`crate::Command::SetLadderMode`] (the
/// level-neutral takeover contract), the core's `LadderPlan` carries it as RUN state
/// rather than configuration, and a configure-time copy would be a second authority
/// that disagrees with the live one the moment an operator takes over.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineLadderConfig {
    /// The sysID pass level, dB relative to full. Informational for the ladder: the
    /// run does sysID BEFORE the ladder starts.
    pub sysid_level_db: f64,
    /// Starting level, dB (conventionally `table_db[0]`).
    pub start_db: f64,
    /// The rung table, dB relative to full. The LAST entry is the full rung.
    pub table_db: Vec<f64>,
    /// Auto-advance rule for every rung except the last.
    pub per_rung: SineRungFor,
    /// Completion rule for the last (full) rung.
    pub full_for: SineRungFor,
}

/// How long a ladder rung runs before auto-advancing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SineRungFor {
    /// N completed sweep passes.
    Sweeps(u32),
    /// Accumulated rung test seconds.
    Seconds(f64),
}

/// When the run captures streaming telemetry ON THE CORE HOST.
///
/// See [`SineWireProfile::capture`]: the rules cross, the remote start/stop verbs do
/// not.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SineCaptureRule {
    /// Never capture.
    Off,
    /// Capture the whole test, from engage to stop.
    EntireTest,
    /// The operator starts and stops capture explicitly. **Over this seam that means
    /// the CORE-side operator** until remote capture lands.
    Manual,
    /// Capture the first `n_s` seconds after each level entry settles.
    AutoFirstSeconds {
        /// Window length, seconds.
        n_s: f64,
    },
}

/// Capture configuration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineCaptureConfig {
    /// Which rule applies.
    pub rule: SineCaptureRule,
    /// Settle debounce before an AUTO window opens after a level entry, seconds.
    pub level_settle_s: f64,
    /// Periodic snapshot-save interval, seconds. `None` = no periodic save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub periodic_save_s: Option<f64>,
}

/// The COLA (constant-output-level-amplitude) reference-output configuration: a
/// SECOND drive channel, frequency-locked to the sweep.
///
/// Enabling it makes the run two-channel on the output side, which the core re-checks
/// against the station's real output width at configure - a profile the hardware
/// cannot carry is refused there, not silently run single-channel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineColaConfig {
    /// Whether COLA is enabled.
    pub enabled: bool,
    /// Constant COLA amplitude, volts PEAK.
    pub volts_pk: f64,
    /// The COLA waveform.
    pub waveform: SineColaWaveform,
}

/// The COLA waveform. All are frequency-locked to the sweep through the SAME NCO
/// phase, at constant amplitude.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SineColaWaveform {
    /// A sine at the swept frequency.
    Sine,
    /// A square wave at the swept frequency.
    Square,
    /// A constant DC reference (frequency-independent).
    DcRef,
}

/// One user metadata field carried into the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SineMetaField {
    /// Stable key (machine name).
    pub key: String,
    /// The value (freeform text).
    pub value: String,
}

// =====================================================================================
// The command group
// =====================================================================================

/// The sine verbs the general command set cannot express (v5).
///
/// Everything else a sine run needs already exists as operator intent and stays there,
/// exactly as it does for the Explorer: [`crate::Command::Start`] runs it,
/// [`crate::Command::Stop`] / [`crate::Command::Abort`] end it,
/// [`crate::Command::GoToFrequency`] / [`crate::Command::AdjustFrequency`] move the
/// tone, [`crate::Command::SetLevel`] with [`crate::LevelTarget::Db`] jumps a rung,
/// [`crate::Command::AdjustLevel`] trims it, [`crate::Command::SetLadderMode`] hands
/// the schedule between the timers and the operator, and
/// [`crate::Command::ProceedAfterReview`] releases the post-gate review hold.
///
/// **`ProceedAfterReview` releases `AwaitingReview` and NOTHING ELSE.** The service
/// also has a `CoherenceHold`, which is the gate FAILING; mapping the same verb onto
/// it would ship a one-click bypass of a safety refusal. There is deliberately no
/// wire verb that clears a `CoherenceHold` - it is cleared by a Reset, from the core.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SineCommand {
    /// PAUSE test progress: the sweep, dwell and rung clocks stop and the tone holds
    /// where it is. MUTATING.
    ///
    /// Not a stop. The output CONTINUES at the held frequency and level - which is the
    /// whole point, because an operator holds a sweep at the moment they see something
    /// and moving the tone off that spot destroys what they stopped to look at.
    Hold,
    /// Resume test progress from a [`SineCommand::Hold`]. MUTATING.
    Resume,
    /// Flip the sweep direction mid-pass. MUTATING.
    ///
    /// The abandoned partial pass does NOT count toward the sweep target; only
    /// band-edge arrivals do, including the one the reversed pass eventually reaches.
    Reverse,
    /// Set the Hinv-feedforward vs feedback blend weight, LIVE. MUTATING.
    SetBlendW {
        /// The blend weight, `[0, 1]`. `0` = pure feedback, `1` = pure feedforward.
        w: f64,
    },
    /// Release a run parked at the FEASIBILITY-BLOCKED gate, with an explicit
    /// acknowledgement. MUTATING.
    ///
    /// The park is what the shaker-limits check does when the authored profile exceeds
    /// the selected shaker: the run stops before it drives. `ack` is the operator's
    /// signed statement that they read the reason and are proceeding anyway; an EMPTY
    /// ack is refused, because a required field nobody has to fill in is not a gate.
    OverrideFeasibility {
        /// The operator's acknowledgement text. Must be non-empty.
        ack: String,
    },
}

impl SineCommand {
    /// Does this verb change core state? The Observer gate consults
    /// [`crate::Command::is_mutating`], which consults this.
    ///
    /// ALL of them do. Hold / Resume / Reverse move (or stop moving) a tone across a
    /// resonance, `SetBlendW` changes how drive is computed on the next tick, and
    /// `OverrideFeasibility` is a run STARTING to drive past a refusal. A watcher on
    /// the lab LAN may send none of them.
    pub fn is_mutating(&self) -> bool {
        match self {
            SineCommand::Hold
            | SineCommand::Resume
            | SineCommand::Reverse
            | SineCommand::SetBlendW { .. }
            | SineCommand::OverrideFeasibility { .. } => true,
        }
    }

    /// The [`ActionKind`] this verb is gated by in the capability
    /// [`crate::ActionMatrix`]. MAPPED, never matrixed - the same trade as
    /// [`crate::ExplorerCommand::action_kind`] and
    /// [`crate::Command::SetLadderMode`]: every `ActionMatrix` literal in the tree
    /// would otherwise grow a field for a verb with identical support semantics.
    pub fn action_kind(&self) -> ActionKind {
        match self {
            // Frequency AUTHORITY: a backend that can be commanded to a frequency is
            // exactly one whose traversal between two of them can be paused, resumed
            // and reversed.
            SineCommand::Hold | SineCommand::Resume | SineCommand::Reverse => {
                ActionKind::GoToFrequency
            }
            // The blend weight decides how much of the next drive comes from the
            // feedforward table - level authority, live, same as a dB trim.
            SineCommand::SetBlendW { .. } => ActionKind::AdjustLevel,
            // Releasing a park is the run starting to drive, so it is gated exactly
            // like Start - the same rule as `Command::ProceedAfterReview`.
            SineCommand::OverrideFeasibility { .. } => ActionKind::Start,
        }
    }
}
