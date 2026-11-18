//! The run's setup: everything a MIMO Sine run is configured with, fixed at
//! arm.
//!
//! A run plays one tone on every drive and judges every control against its
//! own reference. Its setup says what is controlled ([`ControlChannel`]: the
//! reference amplitude against frequency, with its alarm and abort bands, and
//! the reference phase against frequency), what is played ([`DriveChannel`]:
//! each drive's ceiling and abort), what is watched ([`LimitChannel`]: each
//! limit channel's bound against frequency), what the test is ([`TestPlan`]:
//! a sweep, a dwell or a stepped sine) and its level schedule ([`Schedule`]),
//! how the tone is tracked ([`Tracking`]), how the structure is identified
//! ([`IdentificationSettings`]), which law runs ([`LawChoice`]), whether a
//! law's requests are honoured ([`RequestSettings`]) and the guards every law
//! runs under ([`GuardSettings`]).
//!
//! The schedule types are single-shaker Sine's: a sweep is a
//! [`BidirSweepConfig`], a dwell a [`DwellPlan`], a stepped sine on a grid a
//! [`StepSineConfig`] and the level ladder a [`LadderConfig`], each checked by
//! its own scheduler. A reference amplitude and a limit bound are single-shaker
//! Sine's [`SineProfile`] (rows in acceleration, velocity or displacement, with
//! alarm and abort bands per row).
//!
//! [`MimoSineSetup::validate`] names the first rule broken ([`SetupRule`]) and
//! where ([`SetupError::field`], the path inside the setup).

use crate::limits::{
    MAX_CONTROLS, MAX_DRIVES, MAX_DRIVE_VPK, MAX_FS_HZ, MAX_HOLD_S_BOUND, MAX_LIMITS, MAX_TICK_S,
    MIN_FS_HZ, MIN_SLOW_FACTOR_BOUND, MIN_TICK_S,
};
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_sdm::law::MimoShape;
use hertzforge_sine::profile::SineProfile;
use hertzforge_sine::scheduler::{
    BidirSweep, BidirSweepConfig, DwellFor, DwellPlan, DwellRun, LadderConfig, LadderPlan,
    StepSine, StepSineConfig,
};
use serde::{Deserialize, Serialize};
use std::fmt;

/// What kind of test a run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestType {
    /// The tone sweeps the band.
    Sweep,
    /// The tone stays at one frequency.
    Dwell,
    /// The tone steps through a list or a grid of frequencies.
    Stepped,
}

impl TestType {
    /// Every test type.
    pub const ALL: [TestType; 3] = [TestType::Sweep, TestType::Dwell, TestType::Stepped];

    /// The stable wire name: `sweep`, `dwell` or `stepped`.
    pub fn as_str(self) -> &'static str {
        match self {
            TestType::Sweep => "sweep",
            TestType::Dwell => "dwell",
            TestType::Stepped => "stepped",
        }
    }
}

/// The stable wire name of a shape: `square`, `more_controls_than_drives` or
/// `fewer_controls_than_drives`.
pub fn shape_name(shape: MimoShape) -> &'static str {
    match shape {
        MimoShape::Square => "square",
        MimoShape::MoreControlsThanDrives => "more_controls_than_drives",
        MimoShape::FewerControlsThanDrives => "fewer_controls_than_drives",
    }
}

/// A stepped sine: on a grid across a band (single-shaker Sine's step test),
/// or on a list of frequencies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "steps", rename_all = "snake_case")]
pub enum StepPlan {
    /// Steps evenly spaced in octaves or in hertz across a band.
    Grid(StepSineConfig),
    /// Steps at the frequencies listed, in the order given, each held for
    /// `dwell`.
    List {
        /// The step frequencies, Hz, each above 0, at least one.
        frequencies_hz: Vec<f64>,
        /// How long each step is held.
        dwell: DwellFor,
    },
}

/// The test a run plays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TestPlan {
    /// A sweep, logarithmic or linear, up, down or both, for a count of
    /// one-way sweeps or a test time.
    Sweep(BidirSweepConfig),
    /// A dwell at one frequency for a time or a count of cycles.
    Dwell(DwellPlan),
    /// A stepped sine.
    Stepped(StepPlan),
}

impl TestPlan {
    /// The test's type.
    pub fn test_type(&self) -> TestType {
        match self {
            TestPlan::Sweep(_) => TestType::Sweep,
            TestPlan::Dwell(_) => TestType::Dwell,
            TestPlan::Stepped(_) => TestType::Stepped,
        }
    }

    /// The band the tone moves in, Hz: a sweep's or a grid's band, a dwell's
    /// frequency at both ends, a list's lowest and highest frequencies.
    pub fn band_hz(&self) -> (f64, f64) {
        match self {
            TestPlan::Sweep(s) => s.band,
            TestPlan::Dwell(d) => (d.freq_hz, d.freq_hz),
            TestPlan::Stepped(StepPlan::Grid(g)) => g.band,
            TestPlan::Stepped(StepPlan::List { frequencies_hz, .. }) => (
                frequencies_hz.iter().copied().fold(f64::INFINITY, f64::min),
                frequencies_hz
                    .iter()
                    .copied()
                    .fold(f64::NEG_INFINITY, f64::max),
            ),
        }
    }
}

/// How the level ladder moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LevelMode {
    /// The ladder moves on by itself as each level's time or sweeps are done.
    #[default]
    Auto,
    /// The operator moves the level.
    Manual,
}

/// The level schedule: single-shaker Sine's ladder and its mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Schedule {
    pub mode: LevelMode,
    /// The levels, how long each is held and where the run starts.
    pub ladder: LadderConfig,
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule {
            mode: LevelMode::Auto,
            ladder: LadderConfig::default(),
        }
    }
}

/// One point of a reference phase against frequency.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PhasePoint {
    /// Frequency, Hz, above 0.
    pub freq_hz: f64,
    /// The reference's phase at this frequency, degrees, above -180 and at
    /// most 180, relative to the tone.
    pub phase_deg: f64,
}

/// A control channel: what is controlled there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlChannel {
    /// The channel table's index of the control row.
    pub channel: usize,
    /// The reference amplitude against frequency at full level, with the
    /// alarm and abort bands about it.
    pub profile: SineProfile,
    /// The reference phase against frequency, interpolated linearly in phase
    /// against the logarithm of frequency and held beyond the ends. Needed
    /// when the setup's phases are specified; ignored otherwise.
    #[serde(default)]
    pub phase: Vec<PhasePoint>,
}

/// A drive channel: one shaker's drive.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DriveChannel {
    /// The channel table's index of the drive row.
    pub channel: usize,
    /// The drive's ceiling, volts peak, above 0 and at most
    /// [`MAX_DRIVE_VPK`]. A drive vector with a drive over its ceiling is
    /// scaled down as a whole so that drive sits at its ceiling.
    pub ceiling_vpk: f64,
    /// The drive's abort, volts peak, above the ceiling and at most
    /// [`MAX_DRIVE_VPK`]. A drive at or over it ends the run.
    pub abort_vpk: f64,
}

/// A limit channel: an input whose response is held under a bound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitChannel {
    /// The channel table's index of the input.
    pub channel: usize,
    /// The bound against frequency, in the profile's form. The drive is held
    /// down where the channel would pass the bound; the channel passing the
    /// top of its abort band ends the run.
    pub profile: SineProfile,
}

/// The tracking filter's bandwidth rule.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Tracking {
    /// A percentage of the tone's frequency (above 0, at most 100), capped at
    /// `max_hz`.
    Proportional { pct: f64, max_hz: f64 },
    /// A fixed bandwidth, Hz.
    Fixed { hz: f64 },
}

impl Default for Tracking {
    fn default() -> Self {
        Tracking::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        }
    }
}

impl Tracking {
    /// The rule as the tracking filters take it.
    pub fn bandwidth_mode(self) -> BandwidthMode {
        match self {
            Tracking::Proportional { pct, max_hz } => BandwidthMode::Proportional { pct, max_hz },
            Tracking::Fixed { hz } => BandwidthMode::FixedHz(hz),
        }
    }

    /// The bandwidth at `f_hz`, Hz.
    pub fn bandwidth_hz(self, f_hz: f64) -> f64 {
        self.bandwidth_mode().effective_bw(f_hz)
    }
}

/// How the structure is identified before the run: independent random on
/// every drive at low level, the response over the drive at every control
/// and limit channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IdentificationSettings {
    /// The identification's level under the full-level reference, dB, -60 to
    /// 0.
    pub level_db: f64,
    /// The identification's line spacing, Hz: each record is the shortest
    /// power of two of samples whose spacing is at most this (a record of
    /// about `1 / resolution_hz` seconds), from
    /// [`MIN_IDENT_RESOLUTION_HZ`] to [`MAX_IDENT_RESOLUTION_HZ`]. A lightly
    /// damped mode needs several lines across its half-power bandwidth
    /// (`f / Q`).
    pub resolution_hz: f64,
    /// Records averaged, each played twice (once to settle, once to
    /// measure); `None` takes [`IdentificationSettings::default_frames`]. At
    /// most 4096.
    pub frames: Option<u32>,
    /// A line where a control's multiple coherence is under this is not
    /// trusted, 0 to 1.
    pub trust_coherence: f64,
    /// The band-mean multiple coherence every control must reach, or the run
    /// is held, 0 to 1.
    pub gate_coherence: f64,
    /// Hold for the operator after identification.
    pub review: bool,
}

impl Default for IdentificationSettings {
    fn default() -> Self {
        IdentificationSettings {
            level_db: -20.0,
            resolution_hz: DEFAULT_IDENT_RESOLUTION_HZ,
            frames: None,
            trust_coherence: 0.5,
            gate_coherence: 0.5,
            review: true,
        }
    }
}

/// The identification's line spacing when none is given, Hz.
pub const DEFAULT_IDENT_RESOLUTION_HZ: f64 = 0.25;

/// The finest line spacing an identification may ask for, Hz.
pub const MIN_IDENT_RESOLUTION_HZ: f64 = 0.05;

/// The coarsest line spacing an identification may ask for, Hz.
pub const MAX_IDENT_RESOLUTION_HZ: f64 = 4.0;

impl IdentificationSettings {
    /// The records averaged when none is given for `drives` drives: the
    /// larger of 24 and six times the drives (enough records over the drives
    /// for the estimate's inverse to be well posed and its coherence
    /// meaningful).
    pub fn default_frames(drives: usize) -> usize {
        24.max(6 * drives)
    }
}

/// The control law by its catalog id, and values for its declared
/// parameters (defaults fill the rest).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LawChoice {
    /// The catalog id: a built-in law's name, or a law file's SHA-256.
    pub id: String,
    /// Parameter values by key.
    pub params: serde_json::Map<String, serde_json::Value>,
}

impl Default for LawChoice {
    fn default() -> Self {
        LawChoice {
            id: crate::law::DEFAULT_LAW_ID.into(),
            params: serde_json::Map::new(),
        }
    }
}

/// Whether a law's requests to hold or slow the sweep are honoured, and their
/// bounds. Every request is recorded either way.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RequestSettings {
    /// Honour requests. Off: they are recorded and not acted on.
    pub honour: bool,
    /// The longest a granted hold may last without a break, s, above 0 and at
    /// most [`MAX_HOLD_S_BOUND`].
    pub max_hold_s: f64,
    /// The lowest share of the scheduled sweep rate a granted slow may ask
    /// for, at least [`MIN_SLOW_FACTOR_BOUND`] and at most 1.
    pub min_slow_factor: f64,
}

impl Default for RequestSettings {
    fn default() -> Self {
        RequestSettings {
            honour: false,
            max_hold_s: MAX_HOLD_S_BOUND,
            min_slow_factor: MIN_SLOW_FACTOR_BOUND,
        }
    }
}

/// The default rate the notch's depth rises at once the limits allow, dB per
/// second.
pub const DEFAULT_NOTCH_RELEASE_DB_PER_S: f64 = 10.0;

/// The default gap between the notch going in and coming out, dB.
pub const DEFAULT_NOTCH_HYSTERESIS_DB: f64 = 1.0;

/// The backstop notch: how the limit channels hold the drive down.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotchSettings {
    /// Updates each limit channel's measured-over-identified ratio is
    /// smoothed over, at least 1.
    pub smoothing_ticks: u32,
    /// How fast the notch's depth (the share of the reference the controls
    /// are held to while it is in) may rise once the limit readings allow,
    /// dB per second, above 0.
    pub release_ramp_db_per_s: f64,
    /// How far the limit readings must allow the depth to rise before it
    /// begins to, dB, 0 to 6: the gap between going in and coming out, so
    /// the notch does not toggle on readings near its bound.
    pub release_hysteresis_db: f64,
    /// A notched reference a law returns is taken only while a limit channel
    /// is within this of its bound (or the notch is releasing), dB, above 0
    /// and at most 20.
    pub near_bound_db: f64,
}

impl Default for NotchSettings {
    fn default() -> Self {
        NotchSettings {
            smoothing_ticks: 3,
            release_ramp_db_per_s: DEFAULT_NOTCH_RELEASE_DB_PER_S,
            release_hysteresis_db: DEFAULT_NOTCH_HYSTERESIS_DB,
            near_bound_db: 3.0,
        }
    }
}

/// The guards the controller applies to every law.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuardSettings {
    /// The share of the tick one law call may take, above 0 and at most 1.
    pub law_budget: f64,
    /// Unusable calls in a row that take the controlled stop, 1 to 100.
    pub law_misses: u32,
    /// The most each drive's magnitude may change in one update, dB either
    /// way, 1 to 3.
    pub max_step_db: f64,
    /// The most each drive's magnitude may rise in a second, dB, above 0 and
    /// at most 10000.
    pub max_rise_db_per_s: f64,
    /// The most each drive's phase may turn in one update, degrees, above 0
    /// and at most 180; `None` (the default) for no cap. The largest turn a
    /// law asks for is recorded either way.
    pub rotation_cap_deg: Option<f64>,
    /// Seconds a cap may bind without a break before the controlled stop
    /// (`divergence`), above 0 and at most 60.
    pub divergence_s: f64,
    /// Updates in a row a control may be outside its abort band, outside a
    /// settle window, before the run aborts, 1 to 100.
    pub abort_ticks: u32,
    /// With the shakers' ratings, abort when a response passes a rating.
    pub kinematic_monitor: bool,
    pub notch: NotchSettings,
}

impl Default for GuardSettings {
    fn default() -> Self {
        GuardSettings {
            law_budget: crate::limits::DEFAULT_LAW_BUDGET_FRACTION,
            law_misses: crate::limits::DEFAULT_LAW_MISSES,
            max_step_db: crate::limits::DEFAULT_MAX_STEP_DB,
            max_rise_db_per_s: crate::limits::DEFAULT_MAX_RISE_DB_PER_S,
            rotation_cap_deg: None,
            divergence_s: crate::limits::DEFAULT_DIVERGENCE_S,
            abort_ticks: 3,
            kinematic_monitor: true,
            notch: NotchSettings::default(),
        }
    }
}

/// Everything a MIMO Sine run is configured with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MimoSineSetup {
    /// The control channels, 1 to [`MAX_CONTROLS`]. Their order is the order
    /// of every per-control list.
    pub controls: Vec<ControlChannel>,
    /// The drives, 1 to [`MAX_DRIVES`], in the order of every per-drive list.
    pub drives: Vec<DriveChannel>,
    /// The limit channels, 0 to [`MAX_LIMITS`].
    #[serde(default)]
    pub limits: Vec<LimitChannel>,
    /// Whether the references' phases are part of the test. False: each
    /// control's amplitude only is specified for the whole run.
    pub phase_specified: bool,
    pub test: TestPlan,
    #[serde(default)]
    pub schedule: Schedule,
    /// The sample rate asked of the station, Hz.
    pub fs_hz: f64,
    /// The control update interval, s.
    pub tick_s: f64,
    #[serde(default)]
    pub tracking: Tracking,
    #[serde(default)]
    pub identification: IdentificationSettings,
    #[serde(default)]
    pub law: LawChoice,
    #[serde(default)]
    pub requests: RequestSettings,
    #[serde(default)]
    pub guards: GuardSettings,
}

/// The rule a [`MimoSineSetup`] broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupRule {
    /// No control channel, or more than [`MAX_CONTROLS`].
    ControlsOutOfRange,
    /// No drive, or more than [`MAX_DRIVES`].
    DrivesOutOfRange,
    /// More limit channels than [`MAX_LIMITS`].
    LimitsOutOfRange,
    /// A channel named twice among the controls, drives and limits.
    DuplicateChannel,
    /// A drive's ceiling is not above 0 and at most [`MAX_DRIVE_VPK`].
    CeilingInvalid,
    /// A drive's abort is not above its ceiling and at most [`MAX_DRIVE_VPK`].
    AbortBelowCeiling,
    /// A control's or a limit channel's profile is not usable.
    ProfileInvalid,
    /// A profile does not cover the test's band.
    ProfileNotCovering,
    /// The phases are specified and a control has no phase point.
    PhaseMissing,
    /// A phase point's frequency is not above 0 and ascending, or its phase
    /// is not above -180 and at most 180 degrees.
    PhaseInvalid,
    /// The test cannot be built: its scheduler refused it.
    TestInvalid,
    /// The level schedule cannot be built: its ladder refused it, or its
    /// levels are not ascending, or a level is outside -60 to 12 dB.
    ScheduleInvalid,
    /// The sample rate is outside [`MIN_FS_HZ`] to [`MAX_FS_HZ`].
    RateOutOfRange,
    /// The tick is outside [`MIN_TICK_S`] to [`MAX_TICK_S`], or is shorter
    /// than two samples.
    TickOutOfRange,
    /// A band edge at or above 0.45 times the sample rate.
    BandAboveGrid,
    /// The tracking rule is not usable.
    TrackingInvalid,
    /// An identification setting is out of its range.
    IdentificationInvalid,
    /// The law's id is empty.
    LawUnknown,
    /// A request bound is out of its range.
    RequestBoundsInvalid,
    /// A guard setting is out of its range.
    GuardInvalid,
    /// The law does not accept the test's shape.
    LawShapeNotAccepted,
    /// The law does not accept the test's type.
    LawTestTypeNotAccepted,
    /// The law does not accept an amplitude-only reference.
    LawAmplitudeOnlyNotAccepted,
}

impl SetupRule {
    /// The stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            SetupRule::ControlsOutOfRange => "controls_out_of_range",
            SetupRule::DrivesOutOfRange => "drives_out_of_range",
            SetupRule::LimitsOutOfRange => "limits_out_of_range",
            SetupRule::DuplicateChannel => "duplicate_channel",
            SetupRule::CeilingInvalid => "ceiling_invalid",
            SetupRule::AbortBelowCeiling => "abort_below_ceiling",
            SetupRule::ProfileInvalid => "profile_invalid",
            SetupRule::ProfileNotCovering => "profile_not_covering",
            SetupRule::PhaseMissing => "phase_missing",
            SetupRule::PhaseInvalid => "phase_invalid",
            SetupRule::TestInvalid => "test_invalid",
            SetupRule::ScheduleInvalid => "schedule_invalid",
            SetupRule::RateOutOfRange => "rate_out_of_range",
            SetupRule::TickOutOfRange => "tick_out_of_range",
            SetupRule::BandAboveGrid => "band_above_grid",
            SetupRule::TrackingInvalid => "tracking_invalid",
            SetupRule::IdentificationInvalid => "identification_invalid",
            SetupRule::LawUnknown => "law_unknown",
            SetupRule::RequestBoundsInvalid => "request_bounds_invalid",
            SetupRule::GuardInvalid => "guard_invalid",
            SetupRule::LawShapeNotAccepted => "law_shape_not_accepted",
            SetupRule::LawTestTypeNotAccepted => "law_test_type_not_accepted",
            SetupRule::LawAmplitudeOnlyNotAccepted => "law_amplitude_only_not_accepted",
        }
    }
}

/// Why a setup was refused: the rule, where in the setup, and the words.
#[derive(Debug, Clone, PartialEq)]
pub struct SetupError {
    pub rule: SetupRule,
    /// The path inside the setup, such as `drives[1].abort_vpk`.
    pub field: String,
    pub message: String,
}

impl SetupError {
    /// A refusal under `rule` at `field`.
    pub fn new(rule: SetupRule, field: impl Into<String>, message: impl Into<String>) -> Self {
        SetupError {
            rule,
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at {}: {}",
            self.rule.as_str(),
            self.field,
            self.message
        )
    }
}

impl std::error::Error for SetupError {}

fn refuse(rule: SetupRule, field: impl Into<String>, message: impl Into<String>) -> SetupError {
    SetupError::new(rule, field, message)
}

fn in_range(v: f64, lo_excl: f64, hi_incl: f64) -> bool {
    v.is_finite() && v > lo_excl && v <= hi_incl
}

fn check_profile(p: &SineProfile, field: &str, band: (f64, f64)) -> Result<(), SetupError> {
    p.validate()
        .map_err(|e| refuse(SetupRule::ProfileInvalid, field, e.to_string()))?;
    let (first, last) = match (p.rows.first(), p.rows.last()) {
        (Some(a), Some(b)) => (a.freq_hz, b.freq_hz),
        _ => {
            return Err(refuse(
                SetupRule::ProfileInvalid,
                field,
                "the profile has no rows",
            ))
        }
    };
    if first > band.0 || last < band.1 {
        return Err(refuse(
            SetupRule::ProfileNotCovering,
            field,
            format!(
                "the profile runs from {first} to {last} Hz; the test runs from {} to {} Hz",
                band.0, band.1
            ),
        ));
    }
    Ok(())
}

impl MimoSineSetup {
    /// The test's shape: square, more controls than drives, or fewer.
    pub fn shape(&self) -> MimoShape {
        MimoShape::of(self.controls.len(), self.drives.len())
    }

    /// The test's type.
    pub fn test_type(&self) -> TestType {
        self.test.test_type()
    }

    /// Check the setup; the first rule broken is named, with where.
    pub fn validate(&self) -> Result<(), SetupError> {
        self.validate_sizes()?;
        self.validate_acquisition()?;
        let band = self.validate_test()?;
        self.validate_channels(band)?;
        self.validate_schedule()?;
        self.validate_settings()
    }

    fn validate_sizes(&self) -> Result<(), SetupError> {
        let (nc, nd, nl) = (self.controls.len(), self.drives.len(), self.limits.len());
        if nc == 0 || nc > MAX_CONTROLS {
            return Err(refuse(
                SetupRule::ControlsOutOfRange,
                "controls",
                format!("{nc} control channels; 1 to {MAX_CONTROLS} are accepted"),
            ));
        }
        if nd == 0 || nd > MAX_DRIVES {
            return Err(refuse(
                SetupRule::DrivesOutOfRange,
                "drives",
                format!("{nd} drives; 1 to {MAX_DRIVES} are accepted"),
            ));
        }
        if nl > MAX_LIMITS {
            return Err(refuse(
                SetupRule::LimitsOutOfRange,
                "limits",
                format!("{nl} limit channels; at most {MAX_LIMITS} are accepted"),
            ));
        }
        let mut seen = Vec::with_capacity(nc + nd + nl);
        let named = self
            .controls
            .iter()
            .enumerate()
            .map(|(i, c)| (c.channel, format!("controls[{i}].channel")))
            .chain(
                self.drives
                    .iter()
                    .enumerate()
                    .map(|(i, d)| (d.channel, format!("drives[{i}].channel"))),
            )
            .chain(
                self.limits
                    .iter()
                    .enumerate()
                    .map(|(i, l)| (l.channel, format!("limits[{i}].channel"))),
            );
        for (ch, field) in named {
            if seen.contains(&ch) {
                return Err(refuse(
                    SetupRule::DuplicateChannel,
                    field,
                    format!("channel {ch} is named twice"),
                ));
            }
            seen.push(ch);
        }
        Ok(())
    }

    fn validate_acquisition(&self) -> Result<(), SetupError> {
        if !(self.fs_hz.is_finite() && (MIN_FS_HZ..=MAX_FS_HZ).contains(&self.fs_hz)) {
            return Err(refuse(
                SetupRule::RateOutOfRange,
                "fs_hz",
                format!(
                    "a rate of {} Hz; {MIN_FS_HZ} to {MAX_FS_HZ} Hz is accepted",
                    self.fs_hz
                ),
            ));
        }
        if !(self.tick_s.is_finite()
            && (MIN_TICK_S..=MAX_TICK_S).contains(&self.tick_s)
            && self.tick_s * self.fs_hz >= 2.0)
        {
            return Err(refuse(
                SetupRule::TickOutOfRange,
                "tick_s",
                format!(
                    "a tick of {} s; {MIN_TICK_S} to {MAX_TICK_S} s, and at least two samples, \
                     is accepted",
                    self.tick_s
                ),
            ));
        }
        Ok(())
    }

    fn validate_test(&self) -> Result<(f64, f64), SetupError> {
        let bad = |e: &dyn fmt::Display| refuse(SetupRule::TestInvalid, "test", e.to_string());
        match &self.test {
            TestPlan::Sweep(s) => {
                BidirSweep::new(*s).map_err(|e| bad(&e))?;
            }
            TestPlan::Dwell(d) => {
                DwellRun::new(*d).map_err(|e| bad(&e))?;
            }
            TestPlan::Stepped(StepPlan::Grid(g)) => {
                StepSine::new(*g).map_err(|e| bad(&e))?;
            }
            TestPlan::Stepped(StepPlan::List {
                frequencies_hz,
                dwell,
            }) => {
                if frequencies_hz.is_empty() {
                    return Err(refuse(
                        SetupRule::TestInvalid,
                        "test.frequencies_hz",
                        "a stepped sine on a list needs at least one frequency",
                    ));
                }
                if let Some(i) = frequencies_hz
                    .iter()
                    .position(|f| !(f.is_finite() && *f > 0.0))
                {
                    return Err(refuse(
                        SetupRule::TestInvalid,
                        format!("test.frequencies_hz[{i}]"),
                        "a step frequency is not a number above 0",
                    ));
                }
                // The dwell's own check, at the first step's frequency.
                DwellRun::new(DwellPlan {
                    freq_hz: frequencies_hz[0],
                    for_: *dwell,
                })
                .map_err(|e| refuse(SetupRule::TestInvalid, "test.dwell", e.to_string()))?;
            }
        }
        let band = self.test.band_hz();
        if band.1 >= 0.45 * self.fs_hz {
            return Err(refuse(
                SetupRule::BandAboveGrid,
                "test",
                format!(
                    "the test reaches {} Hz; under {} Hz (0.45 times the rate) is accepted",
                    band.1,
                    0.45 * self.fs_hz
                ),
            ));
        }
        Ok(band)
    }

    fn validate_channels(&self, band: (f64, f64)) -> Result<(), SetupError> {
        for (i, d) in self.drives.iter().enumerate() {
            if !in_range(d.ceiling_vpk, 0.0, MAX_DRIVE_VPK) {
                return Err(refuse(
                    SetupRule::CeilingInvalid,
                    format!("drives[{i}].ceiling_vpk"),
                    format!(
                        "a ceiling of {} V; above 0 and at most {MAX_DRIVE_VPK} V is accepted",
                        d.ceiling_vpk
                    ),
                ));
            }
            if !(in_range(d.abort_vpk, 0.0, MAX_DRIVE_VPK) && d.abort_vpk > d.ceiling_vpk) {
                return Err(refuse(
                    SetupRule::AbortBelowCeiling,
                    format!("drives[{i}].abort_vpk"),
                    format!(
                        "an abort of {} V; above the ceiling ({} V) and at most {MAX_DRIVE_VPK} V \
                         is accepted",
                        d.abort_vpk, d.ceiling_vpk
                    ),
                ));
            }
        }
        for (i, c) in self.controls.iter().enumerate() {
            check_profile(&c.profile, &format!("controls[{i}].profile"), band)?;
            if self.phase_specified {
                if c.phase.is_empty() {
                    return Err(refuse(
                        SetupRule::PhaseMissing,
                        format!("controls[{i}].phase"),
                        "the phases are specified and this control has none",
                    ));
                }
                let mut last = 0.0;
                for (k, p) in c.phase.iter().enumerate() {
                    let ok = p.freq_hz.is_finite()
                        && p.freq_hz > last
                        && in_range(p.phase_deg, -180.0, 180.0);
                    if !ok {
                        return Err(refuse(
                            SetupRule::PhaseInvalid,
                            format!("controls[{i}].phase[{k}]"),
                            "a phase point needs a frequency above 0 and above the one before, \
                             and a phase above -180 and at most 180 degrees",
                        ));
                    }
                    last = p.freq_hz;
                }
            }
        }
        for (i, l) in self.limits.iter().enumerate() {
            check_profile(&l.profile, &format!("limits[{i}].profile"), band)?;
        }
        Ok(())
    }

    fn validate_schedule(&self) -> Result<(), SetupError> {
        let l = &self.schedule.ladder;
        LadderPlan::new(l.clone())
            .map_err(|e| refuse(SetupRule::ScheduleInvalid, "schedule.ladder", e.to_string()))?;
        if l.table_db.windows(2).any(|w| w[1] <= w[0]) {
            return Err(refuse(
                SetupRule::ScheduleInvalid,
                "schedule.ladder.table_db",
                "the levels are not ascending",
            ));
        }
        if let Some(i) = l.table_db.iter().position(|v| !(-60.0..=12.0).contains(v)) {
            return Err(refuse(
                SetupRule::ScheduleInvalid,
                format!("schedule.ladder.table_db[{i}]"),
                "a level is outside -60 to 12 dB",
            ));
        }
        Ok(())
    }

    fn validate_settings(&self) -> Result<(), SetupError> {
        match self.tracking {
            Tracking::Proportional { pct, max_hz } => {
                if !(in_range(pct, 0.0, 100.0) && max_hz.is_finite() && max_hz > 0.0) {
                    return Err(refuse(
                        SetupRule::TrackingInvalid,
                        "tracking",
                        "a proportional bandwidth needs a percentage above 0 and at most 100 and \
                         a cap above 0 Hz",
                    ));
                }
            }
            Tracking::Fixed { hz } => {
                if !(hz.is_finite() && hz > 0.0) {
                    return Err(refuse(
                        SetupRule::TrackingInvalid,
                        "tracking",
                        "a fixed bandwidth needs a number of hertz above 0",
                    ));
                }
            }
        }
        let id = &self.identification;
        let id_bad = |field: &str, msg: &str| {
            Err(refuse(
                SetupRule::IdentificationInvalid,
                format!("identification.{field}"),
                msg,
            ))
        };
        if !(id.level_db.is_finite() && (-60.0..=0.0).contains(&id.level_db)) {
            return id_bad("level_db", "the level is outside -60 to 0 dB");
        }
        if !(id.resolution_hz.is_finite()
            && (MIN_IDENT_RESOLUTION_HZ..=MAX_IDENT_RESOLUTION_HZ).contains(&id.resolution_hz))
        {
            return id_bad("resolution_hz", "the line spacing is outside 0.05 to 4 Hz");
        }
        if let Some(f) = id.frames {
            if f == 0 || f > 4096 {
                return id_bad("frames", "the frames are outside 1 to 4096");
            }
        }
        if !(0.0..=1.0).contains(&id.trust_coherence) {
            return id_bad("trust_coherence", "the coherence is outside 0 to 1");
        }
        if !(0.0..=1.0).contains(&id.gate_coherence) {
            return id_bad("gate_coherence", "the coherence is outside 0 to 1");
        }
        if self.law.id.trim().is_empty() {
            return Err(refuse(SetupRule::LawUnknown, "law.id", "no law is named"));
        }
        let r = &self.requests;
        if !in_range(r.max_hold_s, 0.0, MAX_HOLD_S_BOUND) {
            return Err(refuse(
                SetupRule::RequestBoundsInvalid,
                "requests.max_hold_s",
                format!("a hold of at most {MAX_HOLD_S_BOUND} s, above 0, is accepted"),
            ));
        }
        if !(r.min_slow_factor.is_finite()
            && (MIN_SLOW_FACTOR_BOUND..=1.0).contains(&r.min_slow_factor))
        {
            return Err(refuse(
                SetupRule::RequestBoundsInvalid,
                "requests.min_slow_factor",
                format!("a factor from {MIN_SLOW_FACTOR_BOUND} to 1 is accepted"),
            ));
        }
        let g = &self.guards;
        let g_bad = |field: &str, msg: &str| {
            Err(refuse(
                SetupRule::GuardInvalid,
                format!("guards.{field}"),
                msg,
            ))
        };
        if !in_range(g.law_budget, 0.0, 1.0) {
            return g_bad("law_budget", "above 0 and at most 1 is accepted");
        }
        if !(1..=100).contains(&g.law_misses) {
            return g_bad("law_misses", "1 to 100 is accepted");
        }
        if !(g.max_step_db.is_finite() && (1.0..=3.0).contains(&g.max_step_db)) {
            return g_bad("max_step_db", "1 to 3 dB is accepted");
        }
        if !in_range(g.max_rise_db_per_s, 0.0, 10000.0) {
            return g_bad(
                "max_rise_db_per_s",
                "above 0 and at most 10000 dB/s is accepted",
            );
        }
        if let Some(c) = g.rotation_cap_deg {
            if !in_range(c, 0.0, 180.0) {
                return g_bad(
                    "rotation_cap_deg",
                    "above 0 and at most 180 degrees is accepted",
                );
            }
        }
        if !in_range(g.divergence_s, 0.0, 60.0) {
            return g_bad("divergence_s", "above 0 and at most 60 s is accepted");
        }
        if !(1..=100).contains(&g.abort_ticks) {
            return g_bad("abort_ticks", "1 to 100 is accepted");
        }
        if g.notch.smoothing_ticks == 0 {
            return g_bad("notch.smoothing_ticks", "at least 1 is accepted");
        }
        if !(g.notch.release_ramp_db_per_s.is_finite() && g.notch.release_ramp_db_per_s > 0.0) {
            return g_bad("notch.release_ramp_db_per_s", "above 0 dB/s is accepted");
        }
        if !(g.notch.release_hysteresis_db.is_finite()
            && (0.0..=6.0).contains(&g.notch.release_hysteresis_db))
        {
            return g_bad("notch.release_hysteresis_db", "0 to 6 dB is accepted");
        }
        if !in_range(g.notch.near_bound_db, 0.0, 20.0) {
            return g_bad(
                "notch.near_bound_db",
                "above 0 and at most 20 dB is accepted",
            );
        }
        Ok(())
    }
}
