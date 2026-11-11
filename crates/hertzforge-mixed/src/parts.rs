//! The parts list: what a Mixed Mode test plays, how each part moves, and the
//! checks a configuration must pass before anything is built from it.
//!
//! A test is a list of parts played together on one drive and controlled
//! together. Exactly one part is the **floor** (broadband random on a breakpoint
//! profile). **Tones** (sines of stated peak amplitude) and **narrowbands**
//! (random added over the floor across a stated width) each dwell, sweep, or
//! follow another part's frequency at a ratio. Floor plus tones is
//! sine-on-random; floor plus narrowbands is random-on-random; all three is
//! sine-and-random-on-random.
//!
//! Every refusal is a [`ConfigError`] variant with a stable wire name
//! ([`ConfigError::name`]). The motion checks (`part_out_of_band`,
//! `parts_too_close`) are judged over the whole motion of every part: a sweep
//! over the frequencies it will pass through before its target is met, and a
//! follower through its ratio over the motion of the part it follows.

use hertzforge_breakpoints::BreakpointTable;
use hertzforge_dsp::sweep::{sweep_seconds, SweepRate};
use hertzforge_random::service::config::{
    CaptureRules, ControlSet, LevelSchedule, LimitProfile, MetaField, DEFAULT_COHERENCE_FLOOR,
    DEFAULT_OVERLOAD_FRACTION, DEFAULT_SYSID_LEVEL_DB,
};
use hertzforge_sine::profile::{ProfileRow, SineProfile};
use hertzforge_sine::scheduler::{BidirSweepConfig, SweepDirection, SweepTarget};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fmt;

/// The most tones a test may hold.
pub const MAX_TONES: usize = 12;
/// The most narrowbands a test may hold.
pub const MAX_BANDS: usize = 12;
/// Default half-width, Hz, of the lines around a dwelling tone that the floor
/// estimate leaves out.
pub const DEFAULT_EXCLUSION_HZ: f64 = 2.0;
/// Default tracking-filter width, Hz, of every tone's tracker.
pub const DEFAULT_TRACKING_HZ: f64 = 0.5;
/// Tones must stay at least this many tracking widths apart for the whole run.
pub const MIN_TONE_SPACING_WIDTHS: f64 = 2.0;
/// Part kinds the format reserves for later; a part of one of these kinds is
/// refused `unknown_part_kind` until it is built.
pub const RESERVED_PART_KINDS: [&str; 1] = ["pulse_train"];

/// Default floor alarm band `(minus_db, plus_db)`: Random's.
pub const DEFAULT_FLOOR_ALARM_DB: (f64, f64) = (3.0, 3.0);
/// Default floor abort band `(minus_db, plus_db)`: Random's.
pub const DEFAULT_FLOOR_ABORT_DB: (f64, f64) = (6.0, 6.0);
/// Default tone alarm band: Sine's.
pub const DEFAULT_TONE_ALARM_DB: (f64, f64) = hertzforge_sine::profile::DEFAULT_ALARM_DB;
/// Default tone abort band: Sine's.
pub const DEFAULT_TONE_ABORT_DB: (f64, f64) = hertzforge_sine::profile::DEFAULT_ABORT_DB;
/// Default narrowband alarm band: the floor's (the core widens it to the band's
/// own estimation scatter when it judges).
pub const DEFAULT_BAND_ALARM_DB: (f64, f64) = DEFAULT_FLOOR_ALARM_DB;
/// Default narrowband abort band: the floor's.
pub const DEFAULT_BAND_ABORT_DB: (f64, f64) = DEFAULT_FLOOR_ABORT_DB;

// =============================================================================
// Parts
// =============================================================================

/// One part of the test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Part {
    /// Stable id, unique in the list; `follow` names a part by it.
    pub id: String,
    /// The name an operator gave it; may be empty.
    #[serde(default)]
    pub name: String,
    /// Whether the part plays. A part switched off is still validated, since it
    /// can be switched on during the run.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// When the part plays on the test clock.
    #[serde(default)]
    pub window: PartWindow,
    /// What the part is.
    pub body: PartBody,
}

fn yes() -> bool {
    true
}

impl Part {
    /// The part's kind as its wire name.
    pub fn kind(&self) -> &str {
        self.body.kind()
    }

    /// The part's schedule, for a tone or a narrowband.
    pub fn schedule(&self) -> Option<&PartSchedule> {
        match &self.body {
            PartBody::Tone(t) => Some(&t.schedule),
            PartBody::Narrowband(b) => Some(&b.schedule),
            PartBody::Floor(_) | PartBody::Unknown { .. } => None,
        }
    }
}

/// When a part plays, on the test clock (seconds of test time, holds
/// excluded, from the moment tones and bands first switch on).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PartWindow {
    /// The part switches on at this test time, seconds. Default 0.
    #[serde(default)]
    pub on_s: f64,
    /// The part switches off at this test time, seconds; `None` plays to the end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off_s: Option<f64>,
}

impl PartWindow {
    /// Whether the window is open at test time `t_s`.
    pub fn contains(&self, t_s: f64) -> bool {
        t_s >= self.on_s && self.off_s.is_none_or(|off| t_s < off)
    }

    /// Whether this window and `other` share any time.
    pub fn overlaps(&self, other: &PartWindow) -> bool {
        let end_a = self.off_s.unwrap_or(f64::INFINITY);
        let end_b = other.off_s.unwrap_or(f64::INFINITY);
        self.on_s < end_b && other.on_s < end_a
    }
}

/// What a part is, tagged by `kind` on the wire (`floor`, `tone`,
/// `narrowband`).
///
/// A kind the program does not know (including a reserved one) is read as
/// [`PartBody::Unknown`], keeping its name, so a stored configuration with a
/// later kind still reads and [`MixedConfig::validate`] refuses it
/// `unknown_part_kind` by name.
#[derive(Debug, Clone, PartialEq)]
pub enum PartBody {
    /// The broadband random floor.
    Floor(FloorPart),
    /// A sine tone.
    Tone(TonePart),
    /// A narrow band of random over the floor.
    Narrowband(NarrowbandPart),
    /// A kind this program does not build; refused at validation.
    Unknown {
        /// The kind as written.
        kind: String,
        /// The part's other fields as written, kept so the record round-trips.
        fields: Map<String, Value>,
    },
}

impl PartBody {
    /// The kind's wire name.
    pub fn kind(&self) -> &str {
        match self {
            PartBody::Floor(_) => "floor",
            PartBody::Tone(_) => "tone",
            PartBody::Narrowband(_) => "narrowband",
            PartBody::Unknown { kind, .. } => kind,
        }
    }
}

impl Serialize for PartBody {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let body = match self {
            PartBody::Floor(p) => serde_json::to_value(p),
            PartBody::Tone(p) => serde_json::to_value(p),
            PartBody::Narrowband(p) => serde_json::to_value(p),
            PartBody::Unknown { fields, .. } => Ok(Value::Object(fields.clone())),
        }
        .map_err(S::Error::custom)?;
        let mut out = Map::new();
        out.insert("kind".into(), Value::String(self.kind().to_owned()));
        if let Value::Object(m) = body {
            for (k, v) in m {
                if k != "kind" {
                    out.insert(k, v);
                }
            }
        }
        out.serialize(s)
    }
}

impl<'de> Deserialize<'de> for PartBody {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut m = Map::<String, Value>::deserialize(d)?;
        let kind = match m.remove("kind") {
            Some(Value::String(k)) => k,
            Some(_) => return Err(D::Error::custom("a part's \"kind\" is a string")),
            None => return Err(D::Error::missing_field("kind")),
        };
        let v = Value::Object(m);
        match kind.as_str() {
            "floor" => serde_json::from_value(v)
                .map(PartBody::Floor)
                .map_err(D::Error::custom),
            "tone" => serde_json::from_value(v)
                .map(PartBody::Tone)
                .map_err(D::Error::custom),
            "narrowband" => serde_json::from_value(v)
                .map(PartBody::Narrowband)
                .map_err(D::Error::custom),
            _ => {
                let Value::Object(fields) = v else {
                    unreachable!("built from a map")
                };
                Ok(PartBody::Unknown { kind, fields })
            }
        }
    }
}

/// The floor: broadband random on a breakpoint profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloorPart {
    /// The floor's reference, g^2/Hz breakpoints. Its first and last
    /// frequencies are the floor's band: every tone and band stays inside it.
    pub breakpoints: BreakpointTable,
    /// Alarm band `(minus_db, plus_db)`, positive magnitudes.
    #[serde(default = "floor_alarm")]
    pub alarm_db: (f64, f64),
    /// Abort band `(minus_db, plus_db)`, positive magnitudes.
    #[serde(default = "floor_abort")]
    pub abort_db: (f64, f64),
}

fn floor_alarm() -> (f64, f64) {
    DEFAULT_FLOOR_ALARM_DB
}
fn floor_abort() -> (f64, f64) {
    DEFAULT_FLOOR_ABORT_DB
}
fn tone_alarm() -> (f64, f64) {
    DEFAULT_TONE_ALARM_DB
}
fn tone_abort() -> (f64, f64) {
    DEFAULT_TONE_ABORT_DB
}
fn band_alarm() -> (f64, f64) {
    DEFAULT_BAND_ALARM_DB
}
fn band_abort() -> (f64, f64) {
    DEFAULT_BAND_ABORT_DB
}

impl FloorPart {
    /// The floor's band `(lo, hi)`, Hz: its first and last breakpoint
    /// frequencies. `None` for an empty table.
    pub fn band(&self) -> Option<(f64, f64)> {
        Some((
            self.breakpoints.rows.first()?.freq_hz,
            self.breakpoints.rows.last()?.freq_hz,
        ))
    }
}

/// A sine tone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TonePart {
    /// How its frequency moves.
    pub schedule: PartSchedule,
    /// Its peak amplitude at full level.
    pub amplitude: ToneAmplitude,
    /// Alarm band on its tracked amplitude, dB.
    #[serde(default = "tone_alarm")]
    pub alarm_db: (f64, f64),
    /// Abort band on its tracked amplitude, dB.
    #[serde(default = "tone_abort")]
    pub abort_db: (f64, f64),
}

/// A tone's full-level peak amplitude, tagged by `kind` (`constant`,
/// `profile`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToneAmplitude {
    /// The same amplitude at every frequency, g peak.
    Constant {
        /// g peak.
        g_pk: f64,
    },
    /// An amplitude against frequency, as Sine's profile rows state it
    /// (acceleration, velocity or displacement rows, log-log between rows).
    Profile {
        /// The rows, ascending in frequency; they cover the tone's whole motion.
        rows: Vec<ProfileRow>,
    },
}

impl ToneAmplitude {
    /// The amplitude, g peak, at `f_hz`; `None` where a profile does not reach.
    pub fn g_pk_at(&self, f_hz: f64) -> Option<f64> {
        match self {
            ToneAmplitude::Constant { g_pk } => Some(*g_pk),
            ToneAmplitude::Profile { rows } => profile_of(rows).target_accel_g_pk(f_hz),
        }
    }
}

fn profile_of(rows: &[ProfileRow]) -> SineProfile {
    SineProfile {
        rows: rows.to_vec(),
        ..SineProfile::default()
    }
}

/// A narrow band of random added over the floor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NarrowbandPart {
    /// How its center moves.
    pub schedule: PartSchedule,
    /// Density added over the floor across the band at full level, g^2/Hz. The
    /// band's reference is the floor's target plus this, over the same lines.
    pub level_g2_hz: f64,
    /// Its width.
    pub width: BandWidth,
    /// Alarm band on its measured level, dB (widened to the band's own scatter
    /// when judged).
    #[serde(default = "band_alarm")]
    pub alarm_db: (f64, f64),
    /// Abort band on its measured level, dB.
    #[serde(default = "band_abort")]
    pub abort_db: (f64, f64),
}

/// A narrowband's width, tagged by `kind` (`hz`, `share`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BandWidth {
    /// A fixed width, Hz.
    Hz {
        /// Hz.
        hz: f64,
    },
    /// A share of the center frequency (0.1 is a band a tenth of its center
    /// wide), so a swept band widens as it rises.
    Share {
        /// In (0, 1].
        share: f64,
    },
}

impl BandWidth {
    /// The band's edges `(lo, hi)`, Hz, at center `center_hz`.
    pub fn edges_at(&self, center_hz: f64) -> (f64, f64) {
        let w = match *self {
            BandWidth::Hz { hz } => hz,
            BandWidth::Share { share } => share * center_hz,
        };
        (center_hz - w / 2.0, center_hz + w / 2.0)
    }
}

// =============================================================================
// Schedules
// =============================================================================

/// How a tone's frequency or a band's center moves, tagged by `kind`
/// (`fixed`, `sweep`, `follow`, `segments`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PartSchedule {
    /// A dwell at `hz`, for `for_s` seconds of test time or `for_cycles`
    /// cycles when one is given (the part then ends), else for as long as the
    /// part plays. At most one of the two.
    Fixed {
        /// Hz.
        hz: f64,
        /// Seconds of test time; `None` dwells until the part's window closes
        /// or the test ends.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        for_s: Option<f64>,
        /// Cycles of the tone (Sine's dwell by cycles).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        for_cycles: Option<u64>,
    },
    /// A sweep between `lo_hz` and `hi_hz`, bouncing at the limits, from
    /// `start_hz` in `direction` at `rate` under `law`, until `target`.
    Sweep {
        /// Lower limit, Hz.
        lo_hz: f64,
        /// Upper limit, Hz.
        hi_hz: f64,
        /// Where the sweep starts, Hz, inside the limits.
        start_hz: f64,
        /// The direction it starts in.
        #[serde(default)]
        direction: Direction,
        /// The rate's law.
        law: SweepLaw,
        /// The rate in the law's unit (Hz per second, or octaves per minute).
        rate: f64,
        /// When the sweep ends (the part then ends).
        target: PartSweepTarget,
    },
    /// Another part's frequency (a tone's frequency or a band's center) times
    /// `ratio`.
    Follow {
        /// The id of the part followed.
        of: String,
        /// The ratio; greater than zero.
        ratio: f64,
    },
    /// Sweep, dwell and continue in one part. Reserved: it reads, and
    /// validation refuses it `segments_not_built` until it is built.
    Segments {
        /// The segments as written.
        #[serde(default)]
        segments: Vec<Value>,
    },
}

/// A sweep's starting direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Rising.
    #[default]
    Up,
    /// Falling.
    Down,
}

impl From<Direction> for SweepDirection {
    fn from(d: Direction) -> Self {
        match d {
            Direction::Up => SweepDirection::Up,
            Direction::Down => SweepDirection::Down,
        }
    }
}

impl From<SweepDirection> for Direction {
    fn from(d: SweepDirection) -> Self {
        match d {
            SweepDirection::Up => Direction::Up,
            SweepDirection::Down => Direction::Down,
        }
    }
}

/// A sweep rate's law.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepLaw {
    /// Linear, Hz per second.
    LinearHzPerS,
    /// Logarithmic, octaves per minute.
    LogOctPerMin,
}

impl SweepLaw {
    /// The rate as the sweep oscillator states it (magnitude; the scheduler
    /// owns direction).
    pub fn rate(self, rate: f64) -> SweepRate {
        match self {
            SweepLaw::LinearHzPerS => SweepRate::LinearHzPerSec(rate.abs()),
            SweepLaw::LogOctPerMin => SweepRate::LogOctPerMin(rate.abs()),
        }
    }
}

/// When a sweep ends, tagged by `kind` (`sweeps`, `duration`,
/// `whichever_first`). A one-direction pass is one sweep (up and back is two).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PartSweepTarget {
    /// After `n` one-direction passes.
    Sweeps {
        /// At least 1.
        n: u32,
    },
    /// After `s` seconds of test time.
    Duration {
        /// Seconds, greater than zero.
        s: f64,
    },
    /// At whichever of the two comes first.
    WhicheverFirst {
        /// Passes.
        sweeps: u32,
        /// Seconds.
        duration_s: f64,
    },
}

impl From<PartSweepTarget> for SweepTarget {
    fn from(t: PartSweepTarget) -> Self {
        match t {
            PartSweepTarget::Sweeps { n } => SweepTarget::Sweeps(n),
            PartSweepTarget::Duration { s } => SweepTarget::DurationS(s),
            PartSweepTarget::WhicheverFirst { sweeps, duration_s } => {
                SweepTarget::WhicheverFirst { sweeps, duration_s }
            }
        }
    }
}

impl PartSchedule {
    /// The wire name of the schedule's kind.
    pub fn kind(&self) -> &'static str {
        match self {
            PartSchedule::Fixed { .. } => "fixed",
            PartSchedule::Sweep { .. } => "sweep",
            PartSchedule::Follow { .. } => "follow",
            PartSchedule::Segments { .. } => "segments",
        }
    }

    /// The configuration of Sine's bouncing sweep for a `sweep` schedule.
    pub fn sweep_config(&self) -> Option<BidirSweepConfig> {
        match *self {
            PartSchedule::Sweep {
                lo_hz,
                hi_hz,
                start_hz,
                direction,
                law,
                rate,
                target,
            } => Some(BidirSweepConfig {
                band: (lo_hz, hi_hz),
                rate: law.rate(rate),
                start_hz,
                initial_direction: direction.into(),
                target: target.into(),
            }),
            _ => None,
        }
    }
}

/// The frequencies a sweep passes through before its target is met, `(lo,
/// hi)`, Hz: exact, walking the passes as the sweep will (after two passes the
/// whole range has been covered).
pub fn sweep_span(cfg: &BidirSweepConfig) -> (f64, f64) {
    let (lo, hi) = cfg.band;
    let (n_target, mut t_left) = match cfg.target {
        SweepTarget::Sweeps(n) => (n, f64::INFINITY),
        SweepTarget::DurationS(s) => (u32::MAX, s),
        SweepTarget::WhicheverFirst { sweeps, duration_s } => (sweeps, duration_s),
    };
    let mut f = cfg.start_hz.clamp(lo, hi);
    let mut dir = cfg.initial_direction;
    let (mut f_min, mut f_max) = (f, f);
    let mut passes = 0u32;
    // Two edge arrivals cover the whole range; four turns bound the walk (a
    // start on the edge pointing out flips without counting).
    for _ in 0..4 {
        if passes >= n_target.min(2) || t_left <= 0.0 {
            break;
        }
        let edge = match dir {
            SweepDirection::Up => hi,
            SweepDirection::Down => lo,
        };
        let t_edge = sweep_seconds(f, edge, cfg.rate);
        if t_edge <= 0.0 {
            dir = flip(dir);
            continue;
        }
        if t_edge > t_left {
            let signed = match dir {
                SweepDirection::Up => t_left,
                SweepDirection::Down => -t_left,
            };
            f = cfg.rate.freq_at(f, signed).clamp(lo, hi);
            f_min = f_min.min(f);
            f_max = f_max.max(f);
            break;
        }
        f = edge;
        f_min = f_min.min(f);
        f_max = f_max.max(f);
        t_left -= t_edge;
        passes += 1;
        dir = flip(dir);
    }
    (f_min, f_max)
}

fn flip(d: SweepDirection) -> SweepDirection {
    match d {
        SweepDirection::Up => SweepDirection::Down,
        SweepDirection::Down => SweepDirection::Up,
    }
}

// =============================================================================
// The configuration
// =============================================================================

/// The identification before the test: Random's broadband pretest, with
/// Random's field names and defaults. One identification serves every part;
/// each tone's feed-forward is read from it at the tone's frequency.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MixedIdentification {
    /// The pretest's level, dB below the floor's reference (Random's
    /// `sysid_level_db`).
    pub sysid_level_db: f64,
    /// Band coherence the gate needs.
    pub coherence_floor: f64,
    /// Pretest drive ceiling, V rms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pretest_max_vrms: Option<f64>,
    /// Pretest drive floor, V rms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pretest_min_vrms: Option<f64>,
    /// Shape the pretest like the floor's reference instead of flat in band.
    pub shaped_pretest: bool,
    /// The identification level relative to the floor's reference, dB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pretest_reference_db: Option<f64>,
    /// The drive the referenced identification starts from, V rms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pretest_start_vrms: Option<f64>,
    /// Hold for the operator's review after a passed gate (Sine's
    /// `review_hold`, default on).
    pub review_hold: bool,
}

impl Default for MixedIdentification {
    fn default() -> Self {
        MixedIdentification {
            sysid_level_db: DEFAULT_SYSID_LEVEL_DB,
            coherence_floor: DEFAULT_COHERENCE_FLOOR,
            pretest_max_vrms: None,
            pretest_min_vrms: None,
            shaped_pretest: false,
            pretest_reference_db: None,
            pretest_start_vrms: None,
            review_hold: true,
        }
    }
}

/// Default drive ceiling, drive units peak, on the summed drive.
pub const DEFAULT_DRIVE_VPK_CEILING: f64 = 10.0;

/// The run's limits. The drive is judged by its peak, per frame, on the sum of
/// every part (Sine's names); the rest are Random's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MixedLimits {
    /// The summed drive's peak may not pass this, drive units peak (volts peak
    /// on hardware).
    pub drive_vpk_ceiling: f64,
    /// A summed drive peak at or above this ends the run; `None` uses the
    /// ceiling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drive_vpk_abort: Option<f64>,
    /// The random part's rms ceiling (floor and bands), volts: its synthesis
    /// is clamped to it (Random's `drive_ceiling_vrms`). `None`: no clamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drive_ceiling_vrms: Option<f64>,
    /// The random part's rms at which the run ends (Random's
    /// `drive_abort_vrms`). `None`: none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drive_abort_vrms: Option<f64>,
    /// Judge raw input samples against each channel's velocity and
    /// displacement limits (Random's kinematic monitor). Default on.
    pub kinematic_monitor: bool,
    /// Sigma clip on the random part of the drive (floor and bands); `None` is
    /// no clip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drive_clip_sigma: Option<f64>,
    /// Per-channel response and force limit profiles on the line grid.
    pub limit_profiles: Vec<LimitProfile>,
    /// Floor abort-envelope breaches tolerated per loop.
    pub max_lines_out: usize,
    /// Consecutive divergent floor loops before the divergence abort.
    pub divergence_loops: u32,
    /// Overload proxy fraction of each input's range.
    pub overload_fraction: f64,
}

impl Default for MixedLimits {
    fn default() -> Self {
        MixedLimits {
            drive_vpk_ceiling: DEFAULT_DRIVE_VPK_CEILING,
            drive_vpk_abort: None,
            drive_ceiling_vrms: None,
            drive_abort_vrms: None,
            kinematic_monitor: true,
            drive_clip_sigma: None,
            limit_profiles: Vec::new(),
            max_lines_out: 10,
            divergence_loops: 5,
            overload_fraction: DEFAULT_OVERLOAD_FRACTION,
        }
    }
}

/// A law by catalog id, with the parameter values the operator set, as the
/// control service's setups carry them (a JSON object by key; values left out
/// take the law's declared defaults).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawPick {
    /// The catalog id: a built-in id or an imported law file's SHA-256.
    pub id: String,
    /// Parameter values by key.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
}

impl LawPick {
    /// The law `id` with its defaults.
    pub fn new(id: impl Into<String>) -> Self {
        LawPick {
            id: id.into(),
            params: Map::new(),
        }
    }
}

/// The built-in floor law's id (Random's `hinv`).
pub const DEFAULT_FLOOR_LAW_ID: &str = "hinv";
/// The built-in tone law's id (Sine's `servo_blend`).
pub const DEFAULT_TONE_LAW_ID: &str = hertzforge_sine::law::SERVO_BLEND_ID;

/// The run's two law choices: a single-output Random law for the floor, and a
/// Sine law for the tones (one choice, one instance per tone). Narrowbands are
/// the core's own servo, not a law.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixedLawChoice {
    /// A law of the `single` world.
    pub floor: LawPick,
    /// A law of the `sine` world.
    pub tones: LawPick,
}

impl Default for MixedLawChoice {
    fn default() -> Self {
        MixedLawChoice {
            floor: LawPick::new(DEFAULT_FLOOR_LAW_ID),
            tones: LawPick::new(DEFAULT_TONE_LAW_ID),
        }
    }
}

/// The core's guards on the two laws: Random's on the floor law, Sine's on
/// each tone's.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MixedLawGuards {
    /// Random's guards on the floor law.
    pub floor: hertzforge_random::service::config::LawGuards,
    /// Sine's guards on every tone's law.
    pub tones: hertzforge_sine::law::LawGuards,
}

/// A Mixed Mode test's configuration: the parts and everything the run needs
/// around them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixedConfig {
    /// Requested line spacing, Hz (Random's `df_hz`; the realized spacing is
    /// the adopted rate over the transform length).
    pub df_hz: f64,
    /// Requested number of control lines (Random's `lines`).
    pub lines: usize,
    /// Statistical degrees of freedom the floor estimate targets (Random's
    /// `control_dof`).
    #[serde(default = "default_dof")]
    pub control_dof: f64,
    /// The control channels and how they compose (Random's).
    pub control: ControlSet,
    /// The parts.
    pub parts: Vec<Part>,
    /// Half-width, Hz, of the lines around a dwelling tone left out of the
    /// floor estimate.
    #[serde(default = "default_exclusion")]
    pub exclusion_hz: f64,
    /// Every tone tracker's width, Hz.
    #[serde(default = "default_tracking")]
    pub tracking_hz: f64,
    /// The level schedule; one level moves every part by the same dB.
    pub schedule: LevelSchedule,
    /// Test time at full level before the scheduled stop, seconds (Random's
    /// `full_level_duration_s`). `None` runs until every part's schedule ends
    /// or the operator stops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_level_duration_s: Option<f64>,
    /// Ramp time per level change, seconds (Random's `level_ramp_s`).
    #[serde(default)]
    pub level_ramp_s: f64,
    /// Controlled-stop ramp, seconds (Random's `shutdown_ramp_s`).
    #[serde(default)]
    pub shutdown_ramp_s: f64,
    /// How long a tone or band takes to ramp in or out (at start-up, switched
    /// live, at its window's edges and its schedule's end), seconds.
    #[serde(default = "default_part_ramp")]
    pub part_ramp_s: f64,
    /// The longest the floor plays alone before tones and bands ramp in,
    /// seconds: they come in once the floor has equalized (its estimate at
    /// `control_dof` and its rms within 1 dB of the target), or at this time
    /// regardless, logged.
    #[serde(default = "default_floor_settle")]
    pub floor_settle_s: f64,
    /// The identification.
    #[serde(default)]
    pub identification: MixedIdentification,
    /// The limits.
    #[serde(default)]
    pub limits: MixedLimits,
    /// Capture rules (Random's).
    #[serde(default)]
    pub capture: CaptureRules,
    /// The laws.
    #[serde(default)]
    pub law: MixedLawChoice,
    /// The core's guards on the laws; `None` takes the defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub law_guards: Option<MixedLawGuards>,
    /// The tones' control tick, seconds; `None` takes Sine's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_tick_s: Option<f64>,
    /// Operator metadata (Random's).
    #[serde(default)]
    pub metadata: Vec<MetaField>,
}

/// Default part ramp, seconds.
pub const DEFAULT_PART_RAMP_S: f64 = 3.0;
/// Default longest floor-alone time, seconds.
pub const DEFAULT_FLOOR_SETTLE_S: f64 = 30.0;

fn default_part_ramp() -> f64 {
    DEFAULT_PART_RAMP_S
}
fn default_floor_settle() -> f64 {
    DEFAULT_FLOOR_SETTLE_S
}
fn default_dof() -> f64 {
    120.0
}
fn default_exclusion() -> f64 {
    DEFAULT_EXCLUSION_HZ
}
fn default_tracking() -> f64 {
    DEFAULT_TRACKING_HZ
}

/// Why a configuration is refused. Each variant has a stable wire name
/// ([`ConfigError::name`]) and plain words ([`fmt::Display`]).
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// `no_floor`: the parts list has no floor.
    NoFloor,
    /// `more_than_one_floor`: more than one floor.
    MoreThanOneFloor,
    /// `unknown_part_kind`: a kind this program does not build.
    UnknownPartKind {
        /// The part's id.
        part: String,
        /// The kind as written.
        kind: String,
    },
    /// `too_many_tones`.
    TooManyTones {
        /// How many were given.
        count: usize,
    },
    /// `too_many_bands`.
    TooManyBands {
        /// How many were given.
        count: usize,
    },
    /// `duplicate_part_id`: two parts share an id (or an id is empty).
    DuplicatePartId {
        /// The id.
        part: String,
    },
    /// `part_out_of_band`: a tone or band center, or a band edge, leaves the
    /// floor's band (or the rate's usable band) somewhere in its motion.
    PartOutOfBand {
        /// The part's id.
        part: String,
        /// The lowest and highest frequency it reaches, Hz.
        reaches: (f64, f64),
        /// The band it must stay in, Hz.
        band: (f64, f64),
    },
    /// `parts_too_close`: two tones come closer than the spacing allows.
    PartsTooClose {
        /// One tone's id.
        a: String,
        /// The other's.
        b: String,
        /// The closest they come, Hz.
        closest_hz: f64,
        /// The spacing required, Hz.
        required_hz: f64,
    },
    /// `follow_unknown_part`: a `follow` names no part.
    FollowUnknownPart {
        /// The follower's id.
        part: String,
        /// The id it names.
        of: String,
    },
    /// `follow_self`: a part follows itself.
    FollowSelf {
        /// The part's id.
        part: String,
    },
    /// `follow_loop`: parts follow each other round in a loop.
    FollowLoop {
        /// The ids in the loop, in order.
        parts: Vec<String>,
    },
    /// `follow_no_frequency`: a `follow` names the floor, which has no
    /// frequency.
    FollowNoFrequency {
        /// The follower's id.
        part: String,
        /// The id it names.
        of: String,
    },
    /// `segments_not_built`: a `segments` schedule.
    SegmentsNotBuilt {
        /// The part's id.
        part: String,
    },
    /// `bad_part`: a value of one part that cannot be used.
    BadPart {
        /// The part's id.
        part: String,
        /// The field.
        field: &'static str,
        /// Why.
        why: String,
    },
    /// `bad_config`: a value of the configuration around the parts.
    BadConfig {
        /// The field.
        field: &'static str,
        /// Why.
        why: String,
    },
}

impl ConfigError {
    /// The stable wire name.
    pub fn name(&self) -> &'static str {
        match self {
            ConfigError::NoFloor => "no_floor",
            ConfigError::MoreThanOneFloor => "more_than_one_floor",
            ConfigError::UnknownPartKind { .. } => "unknown_part_kind",
            ConfigError::TooManyTones { .. } => "too_many_tones",
            ConfigError::TooManyBands { .. } => "too_many_bands",
            ConfigError::DuplicatePartId { .. } => "duplicate_part_id",
            ConfigError::PartOutOfBand { .. } => "part_out_of_band",
            ConfigError::PartsTooClose { .. } => "parts_too_close",
            ConfigError::FollowUnknownPart { .. } => "follow_unknown_part",
            ConfigError::FollowSelf { .. } => "follow_self",
            ConfigError::FollowLoop { .. } => "follow_loop",
            ConfigError::FollowNoFrequency { .. } => "follow_no_frequency",
            ConfigError::SegmentsNotBuilt { .. } => "segments_not_built",
            ConfigError::BadPart { .. } => "bad_part",
            ConfigError::BadConfig { .. } => "bad_config",
        }
    }

    /// The part the refusal is about, when it is about one.
    pub fn part(&self) -> Option<&str> {
        match self {
            ConfigError::UnknownPartKind { part, .. }
            | ConfigError::DuplicatePartId { part }
            | ConfigError::PartOutOfBand { part, .. }
            | ConfigError::FollowUnknownPart { part, .. }
            | ConfigError::FollowSelf { part }
            | ConfigError::FollowNoFrequency { part, .. }
            | ConfigError::SegmentsNotBuilt { part }
            | ConfigError::BadPart { part, .. } => Some(part),
            ConfigError::PartsTooClose { a, .. } => Some(a),
            ConfigError::FollowLoop { parts } => parts.first().map(String::as_str),
            _ => None,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::NoFloor => f.write_str("the test needs a floor"),
            ConfigError::MoreThanOneFloor => f.write_str("the test has more than one floor"),
            ConfigError::UnknownPartKind { part, kind } => {
                write!(f, "part {part}: there is no part of kind \"{kind}\"")
            }
            ConfigError::TooManyTones { count } => {
                write!(f, "{count} tones; a test holds at most {MAX_TONES}")
            }
            ConfigError::TooManyBands { count } => {
                write!(f, "{count} narrowbands; a test holds at most {MAX_BANDS}")
            }
            ConfigError::DuplicatePartId { part } => {
                write!(f, "part id \"{part}\" is empty or used twice")
            }
            ConfigError::PartOutOfBand {
                part,
                reaches,
                band,
            } => write!(
                f,
                "part {part} reaches {:.3} to {:.3} Hz, outside {:.3} to {:.3} Hz",
                reaches.0, reaches.1, band.0, band.1
            ),
            ConfigError::PartsTooClose {
                a,
                b,
                closest_hz,
                required_hz,
            } => write!(
                f,
                "tones {a} and {b} come within {closest_hz:.3} Hz; they must stay {required_hz:.3} Hz apart"
            ),
            ConfigError::FollowUnknownPart { part, of } => {
                write!(f, "part {part} follows \"{of}\", which is not a part")
            }
            ConfigError::FollowSelf { part } => write!(f, "part {part} follows itself"),
            ConfigError::FollowLoop { parts } => {
                write!(f, "parts follow each other in a loop: {}", parts.join(", "))
            }
            ConfigError::FollowNoFrequency { part, of } => {
                write!(f, "part {part} follows {of}, which has no frequency")
            }
            ConfigError::SegmentsNotBuilt { part } => {
                write!(f, "part {part}: a schedule in segments is not available")
            }
            ConfigError::BadPart { part, field, why } => write!(f, "part {part}, {field}: {why}"),
            ConfigError::BadConfig { field, why } => write!(f, "{field}: {why}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// The span of frequencies a part with a frequency covers over its whole
/// motion, `(lo, hi)`, Hz, and where it comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    /// The lowest frequency (tone) or center (band) it reaches, Hz.
    pub lo_hz: f64,
    /// The highest, Hz.
    pub hi_hz: f64,
    /// The index of the part at the root of its `follow` chain (itself when it
    /// does not follow).
    pub root: usize,
    /// Its frequency over the root's: the product of the ratios along the chain
    /// (1 for a root).
    pub factor: f64,
}

fn finite_pos(x: f64) -> bool {
    x.is_finite() && x > 0.0
}

fn bad_part(p: &Part, field: &'static str, why: impl Into<String>) -> ConfigError {
    ConfigError::BadPart {
        part: p.id.clone(),
        field,
        why: why.into(),
    }
}

fn check_band_db(p: &Part, field: &'static str, b: (f64, f64)) -> Result<(), ConfigError> {
    if b.0.is_finite() && b.1.is_finite() && b.0 > 0.0 && b.1 > 0.0 {
        Ok(())
    } else {
        Err(bad_part(p, field, "both sides are positive dB magnitudes"))
    }
}

impl MixedConfig {
    /// The floor part, if there is exactly one.
    pub fn floor(&self) -> Option<&FloorPart> {
        let mut floors = self.parts.iter().filter_map(|p| match &p.body {
            PartBody::Floor(f) => Some(f),
            _ => None,
        });
        let f = floors.next()?;
        floors.next().is_none().then_some(f)
    }

    /// The tones, in parts-list order.
    pub fn tones(&self) -> impl Iterator<Item = (&Part, &TonePart)> {
        self.parts.iter().filter_map(|p| match &p.body {
            PartBody::Tone(t) => Some((p, t)),
            _ => None,
        })
    }

    /// The narrowbands, in parts-list order.
    pub fn bands(&self) -> impl Iterator<Item = (&Part, &NarrowbandPart)> {
        self.parts.iter().filter_map(|p| match &p.body {
            PartBody::Narrowband(b) => Some((p, b)),
            _ => None,
        })
    }

    /// The index of the part with `id`.
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.parts.iter().position(|p| p.id == id)
    }

    /// The order in which part frequencies are resolved: every part after the
    /// part it follows. Refuses a follow of no part, of itself, of the floor,
    /// or round a loop.
    pub fn follow_order(&self) -> Result<Vec<usize>, ConfigError> {
        let n = self.parts.len();
        let ids: HashMap<&str, usize> = self
            .parts
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.as_str(), i))
            .collect();
        // parent[i]: the part i follows.
        let mut parent = vec![None; n];
        for (i, p) in self.parts.iter().enumerate() {
            if let Some(PartSchedule::Follow { of, .. }) = p.schedule() {
                let j = *ids
                    .get(of.as_str())
                    .ok_or_else(|| ConfigError::FollowUnknownPart {
                        part: p.id.clone(),
                        of: of.clone(),
                    })?;
                if j == i {
                    return Err(ConfigError::FollowSelf { part: p.id.clone() });
                }
                if self.parts[j].schedule().is_none() {
                    return Err(ConfigError::FollowNoFrequency {
                        part: p.id.clone(),
                        of: of.clone(),
                    });
                }
                parent[i] = Some(j);
            }
        }
        // 0 unvisited, 1 on the path, 2 placed.
        let mut mark = vec![0u8; n];
        let mut order = Vec::with_capacity(n);
        for start in 0..n {
            let mut path = Vec::new();
            let mut i = start;
            loop {
                match mark[i] {
                    2 => break,
                    1 => {
                        let at = path.iter().position(|&k| k == i).unwrap_or(0);
                        return Err(ConfigError::FollowLoop {
                            parts: path[at..]
                                .iter()
                                .map(|&k: &usize| self.parts[k].id.clone())
                                .collect(),
                        });
                    }
                    _ => {
                        mark[i] = 1;
                        path.push(i);
                        match parent[i] {
                            Some(j) => i = j,
                            None => break,
                        }
                    }
                }
            }
            for &k in path.iter().rev() {
                mark[k] = 2;
                order.push(k);
            }
        }
        Ok(order)
    }

    /// Every part's motion (`None` for the floor and unknown kinds), indexed
    /// as the parts list. Needs a valid follow order and valid schedules.
    pub fn motions(&self) -> Result<Vec<Option<Motion>>, ConfigError> {
        let order = self.follow_order()?;
        let mut out: Vec<Option<Motion>> = vec![None; self.parts.len()];
        for i in order {
            let p = &self.parts[i];
            let Some(s) = p.schedule() else { continue };
            out[i] = Some(match s {
                PartSchedule::Fixed { hz, .. } => Motion {
                    lo_hz: *hz,
                    hi_hz: *hz,
                    root: i,
                    factor: 1.0,
                },
                PartSchedule::Sweep { .. } => {
                    let cfg = s.sweep_config().expect("a sweep");
                    let (lo, hi) = sweep_span(&cfg);
                    Motion {
                        lo_hz: lo,
                        hi_hz: hi,
                        root: i,
                        factor: 1.0,
                    }
                }
                PartSchedule::Follow { of, ratio } => {
                    let j = self.index_of(of).expect("checked by the follow order");
                    let m = out[j].expect("followed parts are placed first");
                    Motion {
                        lo_hz: m.lo_hz * ratio,
                        hi_hz: m.hi_hz * ratio,
                        root: m.root,
                        factor: m.factor * ratio,
                    }
                }
                PartSchedule::Segments { .. } => {
                    return Err(ConfigError::SegmentsNotBuilt { part: p.id.clone() })
                }
            });
        }
        Ok(out)
    }

    /// Check the configuration. `fs_hz` is the rate the run will adopt, when
    /// known (`None` before a station is chosen): with it, every frequency must
    /// also stay under the rate's half.
    ///
    /// Refusals, in the order they are judged: the configuration around the
    /// parts (`bad_config`); unknown kinds; the floor count (`no_floor`,
    /// `more_than_one_floor`); ids; the tone and band counts; each part's own
    /// values (`bad_part`, `segments_not_built`); follows (`follow_*`); the
    /// motion of every part against the floor's band (`part_out_of_band`);
    /// tone spacing over the whole motion (`parts_too_close`).
    pub fn validate(&self, fs_hz: Option<f64>) -> Result<(), ConfigError> {
        self.validate_around()?;
        for p in &self.parts {
            if let PartBody::Unknown { kind, .. } = &p.body {
                return Err(ConfigError::UnknownPartKind {
                    part: p.id.clone(),
                    kind: kind.clone(),
                });
            }
        }
        let floors = self
            .parts
            .iter()
            .filter(|p| matches!(p.body, PartBody::Floor(_)))
            .count();
        match floors {
            0 => return Err(ConfigError::NoFloor),
            1 => {}
            _ => return Err(ConfigError::MoreThanOneFloor),
        }
        let mut seen = std::collections::HashSet::new();
        for p in &self.parts {
            if p.id.trim().is_empty() || !seen.insert(p.id.as_str()) {
                return Err(ConfigError::DuplicatePartId { part: p.id.clone() });
            }
        }
        let tones = self.tones().count();
        if tones > MAX_TONES {
            return Err(ConfigError::TooManyTones { count: tones });
        }
        let bands = self.bands().count();
        if bands > MAX_BANDS {
            return Err(ConfigError::TooManyBands { count: bands });
        }
        for p in &self.parts {
            self.validate_part(p)?;
        }
        let motions = self.motions()?;
        let floor = self.floor().expect("one floor");
        let (mut lo, mut hi) = floor.band().expect("a validated table has rows");
        if let Some(fs) = fs_hz {
            if hi >= fs / 2.0 {
                return Err(ConfigError::PartOutOfBand {
                    part: self
                        .parts
                        .iter()
                        .find(|p| matches!(p.body, PartBody::Floor(_)))
                        .map(|p| p.id.clone())
                        .unwrap_or_default(),
                    reaches: (lo, hi),
                    band: (0.0, fs / 2.0),
                });
            }
            hi = hi.min(fs / 2.0);
            lo = lo.max(0.0);
        }
        self.validate_motion(&motions, (lo, hi))?;
        self.validate_spacing(&motions)
    }

    fn validate_around(&self) -> Result<(), ConfigError> {
        let bad = |field, why: &str| ConfigError::BadConfig {
            field,
            why: why.into(),
        };
        if !finite_pos(self.df_hz) {
            return Err(bad("df_hz", "a line spacing greater than zero"));
        }
        if self.lines == 0 {
            return Err(bad("lines", "at least one line"));
        }
        if !finite_pos(self.control_dof) {
            return Err(bad("control_dof", "greater than zero"));
        }
        if self.control.channels.is_empty() {
            return Err(bad("control", "at least one control channel"));
        }
        if self
            .control
            .channels
            .iter()
            .any(|c| !(c.weight.is_finite() && c.weight >= 0.0))
        {
            return Err(bad("control", "weights are zero or more"));
        }
        if !(self.exclusion_hz.is_finite() && self.exclusion_hz >= 0.0) {
            return Err(bad("exclusion_hz", "zero or more"));
        }
        if !finite_pos(self.tracking_hz) {
            return Err(bad("tracking_hz", "greater than zero"));
        }
        let s = &self.schedule;
        if s.table_db.is_empty() || s.table_db.iter().any(|d| !d.is_finite()) {
            return Err(bad("schedule", "a level table of finite dB"));
        }
        if s.table_db.windows(2).any(|w| w[1] < w[0]) {
            return Err(bad("schedule", "levels that do not fall"));
        }
        if !s.start_db.is_finite() || s.dwell_s.iter().any(|d| !(d.is_finite() && *d >= 0.0)) {
            return Err(bad("schedule", "a finite start and dwells of zero or more"));
        }
        if let Some(d) = self.full_level_duration_s {
            if !finite_pos(d) {
                return Err(bad("full_level_duration_s", "greater than zero"));
            }
        }
        for (field, v) in [
            ("level_ramp_s", self.level_ramp_s),
            ("shutdown_ramp_s", self.shutdown_ramp_s),
            ("part_ramp_s", self.part_ramp_s),
            ("floor_settle_s", self.floor_settle_s),
        ] {
            if !(v.is_finite() && v >= 0.0) {
                return Err(bad(field, "zero or more"));
            }
        }
        let l = &self.limits;
        if !finite_pos(l.drive_vpk_ceiling) {
            return Err(bad("drive_vpk_ceiling", "greater than zero"));
        }
        if let Some(a) = l.drive_vpk_abort {
            if !finite_pos(a) || a < l.drive_vpk_ceiling {
                return Err(bad("drive_vpk_abort", "at or above the ceiling"));
            }
        }
        if let Some(v) = l.drive_ceiling_vrms {
            if !finite_pos(v) {
                return Err(bad("drive_ceiling_vrms", "greater than zero"));
            }
        }
        if let Some(a) = l.drive_abort_vrms {
            if !finite_pos(a) || l.drive_ceiling_vrms.is_some_and(|c| a < c) {
                return Err(bad("drive_abort_vrms", "at or above the rms ceiling"));
            }
        }
        if let Some(c) = l.drive_clip_sigma {
            if !finite_pos(c) {
                return Err(bad("drive_clip_sigma", "greater than zero"));
            }
        }
        let id = &self.identification;
        if !(id.coherence_floor.is_finite() && (0.0..=1.0).contains(&id.coherence_floor)) {
            return Err(bad("coherence_floor", "0 to 1"));
        }
        if !id.sysid_level_db.is_finite() {
            return Err(bad("sysid_level_db", "a finite dB"));
        }
        if let Some(t) = self.control_tick_s {
            if !finite_pos(t) {
                return Err(bad("control_tick_s", "greater than zero"));
            }
        }
        if let Some(g) = &self.law_guards {
            g.tones.validate().map_err(|why| ConfigError::BadConfig {
                field: "law_guards",
                why,
            })?;
            let f = &g.floor;
            if !(finite_pos(f.max_change_db) && f.budget_fraction > 0.0 && f.budget_fraction <= 1.0)
                || f.max_misses == 0
            {
                return Err(bad(
                    "law_guards",
                    "the floor's change cap and budget greater than zero, and at least one miss",
                ));
            }
        }
        if self.law.floor.id.is_empty() || self.law.tones.id.is_empty() {
            return Err(bad("law", "each law is named by its catalog id"));
        }
        Ok(())
    }

    fn validate_part(&self, p: &Part) -> Result<(), ConfigError> {
        let w = p.window;
        if !(w.on_s.is_finite() && w.on_s >= 0.0) {
            return Err(bad_part(p, "window", "on_s is zero or more"));
        }
        if let Some(off) = w.off_s {
            if !(off.is_finite() && off > w.on_s) {
                return Err(bad_part(p, "window", "off_s is after on_s"));
            }
        }
        match &p.body {
            PartBody::Floor(f) => {
                if !p.enabled || w.on_s != 0.0 || w.off_s.is_some() {
                    return Err(bad_part(p, "window", "the floor plays for the whole run"));
                }
                f.breakpoints
                    .validate()
                    .map_err(|e| bad_part(p, "breakpoints", e.to_string()))?;
                if f.breakpoints.rows.len() < 2 {
                    return Err(bad_part(p, "breakpoints", "at least two rows"));
                }
                check_band_db(p, "alarm_db", f.alarm_db)?;
                check_band_db(p, "abort_db", f.abort_db)?;
            }
            PartBody::Tone(t) => {
                self.validate_schedule(p, &t.schedule)?;
                match &t.amplitude {
                    ToneAmplitude::Constant { g_pk } => {
                        if !finite_pos(*g_pk) {
                            return Err(bad_part(p, "amplitude", "g_pk greater than zero"));
                        }
                    }
                    ToneAmplitude::Profile { rows } => {
                        profile_of(rows)
                            .validate()
                            .map_err(|e| bad_part(p, "amplitude", format!("{e:?}")))?;
                    }
                }
                check_band_db(p, "alarm_db", t.alarm_db)?;
                check_band_db(p, "abort_db", t.abort_db)?;
            }
            PartBody::Narrowband(b) => {
                self.validate_schedule(p, &b.schedule)?;
                if !finite_pos(b.level_g2_hz) {
                    return Err(bad_part(p, "level_g2_hz", "greater than zero"));
                }
                match b.width {
                    BandWidth::Hz { hz } if !finite_pos(hz) => {
                        return Err(bad_part(p, "width", "hz greater than zero"))
                    }
                    BandWidth::Share { share } if !(finite_pos(share) && share <= 1.0) => {
                        return Err(bad_part(p, "width", "share greater than 0, at most 1"))
                    }
                    _ => {}
                }
                check_band_db(p, "alarm_db", b.alarm_db)?;
                check_band_db(p, "abort_db", b.abort_db)?;
            }
            PartBody::Unknown { kind, .. } => {
                return Err(ConfigError::UnknownPartKind {
                    part: p.id.clone(),
                    kind: kind.clone(),
                })
            }
        }
        Ok(())
    }

    fn validate_schedule(&self, p: &Part, s: &PartSchedule) -> Result<(), ConfigError> {
        match s {
            PartSchedule::Fixed {
                hz,
                for_s,
                for_cycles,
            } => {
                if !finite_pos(*hz) {
                    return Err(bad_part(p, "schedule", "hz greater than zero"));
                }
                if let Some(t) = for_s {
                    if !finite_pos(*t) {
                        return Err(bad_part(p, "schedule", "for_s greater than zero"));
                    }
                }
                if *for_cycles == Some(0) {
                    return Err(bad_part(p, "schedule", "for_cycles at least 1"));
                }
                if for_s.is_some() && for_cycles.is_some() {
                    return Err(bad_part(
                        p,
                        "schedule",
                        "a dwell is for seconds or for cycles, not both",
                    ));
                }
            }
            PartSchedule::Sweep { .. } => {
                let cfg = s.sweep_config().expect("a sweep");
                hertzforge_sine::scheduler::BidirSweep::new(cfg)
                    .map_err(|e| bad_part(p, "schedule", e.to_string()))?;
            }
            PartSchedule::Follow { ratio, .. } => {
                if !finite_pos(*ratio) {
                    return Err(bad_part(p, "schedule", "ratio greater than zero"));
                }
            }
            PartSchedule::Segments { .. } => {
                return Err(ConfigError::SegmentsNotBuilt { part: p.id.clone() })
            }
        }
        Ok(())
    }

    fn validate_motion(
        &self,
        motions: &[Option<Motion>],
        band: (f64, f64),
    ) -> Result<(), ConfigError> {
        for (p, m) in self.parts.iter().zip(motions) {
            let Some(m) = m else { continue };
            let reaches = match &p.body {
                PartBody::Narrowband(b) => {
                    (b.width.edges_at(m.lo_hz).0, b.width.edges_at(m.hi_hz).1)
                }
                _ => (m.lo_hz, m.hi_hz),
            };
            if reaches.0 < band.0 || reaches.1 > band.1 {
                return Err(ConfigError::PartOutOfBand {
                    part: p.id.clone(),
                    reaches,
                    band,
                });
            }
            if let PartBody::Tone(t) = &p.body {
                if let ToneAmplitude::Profile { rows } = &t.amplitude {
                    let prof = profile_of(rows);
                    for f in [m.lo_hz, m.hi_hz] {
                        if prof.target_accel_g_pk(f).is_none() {
                            return Err(bad_part(
                                p,
                                "amplitude",
                                format!("the profile does not reach {f:.3} Hz"),
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Tones must stay [`MIN_TONE_SPACING_WIDTHS`] tracking widths apart for
    /// the whole run. Two tones on the same `follow` root keep a fixed ratio,
    /// so they are closest where the root is lowest. Two tones on different
    /// roots move independently and may meet anywhere their spans come
    /// together (crossing tones are not built), so their spans must stay that
    /// far apart. Tones whose windows never overlap are not compared.
    fn validate_spacing(&self, motions: &[Option<Motion>]) -> Result<(), ConfigError> {
        let required = MIN_TONE_SPACING_WIDTHS * self.tracking_hz;
        let tones: Vec<(usize, &Part, Motion)> = self
            .parts
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(p.body, PartBody::Tone(_)))
            .filter_map(|(i, p)| motions[i].map(|m| (i, p, m)))
            .collect();
        for (x, (_, pa, ma)) in tones.iter().enumerate() {
            for (_, pb, mb) in &tones[x + 1..] {
                if !pa.window.overlaps(&pb.window) {
                    continue;
                }
                let closest = if ma.root == mb.root {
                    let root = motions[ma.root].expect("a root has a motion");
                    (ma.factor - mb.factor).abs() * root.lo_hz
                } else {
                    (ma.lo_hz - mb.hi_hz).max(mb.lo_hz - ma.hi_hz).max(0.0)
                };
                if closest < required {
                    return Err(ConfigError::PartsTooClose {
                        a: pa.id.clone(),
                        b: pb.id.clone(),
                        closest_hz: closest,
                        required_hz: required,
                    });
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests_support;

#[cfg(test)]
mod tests;
