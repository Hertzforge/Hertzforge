//! The configuration of a MIMO Mixed Mode test, its parts and its validation.
//!
//! The parts model is single-output Mixed Mode's (design D2): one floor, up to
//! [`MAX_TONES`] tones and [`MAX_BANDS`] narrowbands, each with a window and a
//! schedule (`fixed`, `sweep`, `follow`; `segments` reserved). What changes is
//! what a part states: the floor's spectrum is the reference matrix (D6), a
//! tone states a peak amplitude per control and optionally a phase per control
//! (D9), a narrowband states `raise` or `added` (D10). The schedules, windows,
//! widths and amplitudes are single-output Mixed Mode's own types.
//!
//! [`MimoMixedConfig::validate`] refuses a configuration by name, first
//! refusal first, as the single-output validation does; it reuses that
//! validation on a projection of this configuration onto one control
//! ([`MimoMixedConfig::projection`]), so every single-output refusal keeps its
//! name and its rule.

use crate::band::{classify, BandStatement};
use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};
use hertzforge_mimo_random::config::{
    Caps, DriveSpec, Estimation, FrfMode, Gates, Identification, LimitSpec, Protections,
};
use hertzforge_mixed::{
    BandWidth, ConfigError, FloorPart, LawPick, MixedConfig, MixedIdentification, MixedLawChoice,
    MixedLimits, NarrowbandPart, Part, PartBody, PartSchedule, PartWindow, ToneAmplitude, TonePart,
};
use hertzforge_random::service::config::{CaptureRules, ControlSet, LevelSchedule, MetaField};
use hertzforge_reference::Reference;
use hertzforge_shaker_db::{MovingMass, ShakerRecord};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;

/// The most control channels: MIMO Sine's, whose laws hold the tones (the
/// service checks the two agree).
pub const MAX_CONTROLS: usize = 24;
/// The most drives: MIMO Sine's, as [`MAX_CONTROLS`].
pub const MAX_DRIVES: usize = 12;
/// The most limit channels (design D17).
pub const MAX_LIMITS: usize = 8;
/// The most tones (design D17; single-output Mixed Mode's).
pub const MAX_TONES: usize = hertzforge_mixed::MAX_TONES;
/// The most narrowbands (design D17; single-output Mixed Mode's).
pub const MAX_BANDS: usize = hertzforge_mixed::MAX_BANDS;

/// The test type id (design D1).
pub const TEST_TYPE_ID: &str = "mimo_mixed";

/// The default floor law: MIMO Random's default.
pub const DEFAULT_FLOOR_LAW_ID: &str = "pinv_closed";
/// The default tone law: MIMO Sine's default.
pub const DEFAULT_TONE_LAW_ID: &str = "matrix";
/// The default band law: a resolution rule (fold where eligible, else
/// `scalar_servo`).
pub const DEFAULT_BAND_LAW_ID: &str = "auto";
/// The default ceiling rule (design D8, S2).
pub const DEFAULT_CEILING_RULE_ID: &str = "common_scale";
/// The default fill rule (design D7).
pub const DEFAULT_FILL_RULE_ID: &str = "no_error";

/// Band law ids this crate builds (design D10). `auto` is a resolution rule.
pub const BAND_LAW_IDS: [&str; 3] = ["auto", "scalar_servo", "per_control"];
/// Ceiling rule ids this crate builds (design D8).
pub const CEILING_RULE_IDS: [&str; 2] = ["common_scale", "in_turn"];
/// Fill rule ids this crate builds (design D7).
pub const FILL_RULE_IDS: [&str; 2] = ["no_error", "model_fill"];

/// The default exclusion half-width, Hz (single-output Mixed Mode's).
pub const DEFAULT_EXCLUSION_HZ: f64 = hertzforge_mixed::DEFAULT_EXCLUSION_HZ;
/// The default tracker noise bandwidth, Hz (single-output Mixed Mode's).
pub const DEFAULT_TRACKING_HZ: f64 = hertzforge_mixed::DEFAULT_TRACKING_HZ;
/// Default floor alarm band, dB below and above.
pub const DEFAULT_FLOOR_ALARM_DB: (f64, f64) = (3.0, 3.0);
/// Default floor abort band, dB below and above.
pub const DEFAULT_FLOOR_ABORT_DB: (f64, f64) = (6.0, 6.0);
/// Default tone alarm band, dB (Sine's).
pub const DEFAULT_TONE_ALARM_DB: (f64, f64) = hertzforge_mixed::parts::DEFAULT_TONE_ALARM_DB;
/// Default tone abort band, dB (Sine's).
pub const DEFAULT_TONE_ABORT_DB: (f64, f64) = hertzforge_mixed::parts::DEFAULT_TONE_ABORT_DB;
/// Default band alarm band, dB.
pub const DEFAULT_BAND_ALARM_DB: (f64, f64) = DEFAULT_FLOOR_ALARM_DB;
/// Default band abort band, dB.
pub const DEFAULT_BAND_ABORT_DB: (f64, f64) = DEFAULT_FLOOR_ABORT_DB;

// =============================================================================
// Per-control values
// =============================================================================

/// A value stated once for every control, or once per control. On the wire
/// one value (`3.0`, `[3, 3]`) or a list of them, one per control.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PerControl<T> {
    /// One value for every control.
    All(T),
    /// One value per control, in the order of the controls.
    Each(Vec<T>),
}

impl<T: Clone> PerControl<T> {
    /// The value for control `i` (`None` past a list's end).
    pub fn get(&self, i: usize) -> Option<&T> {
        match self {
            PerControl::All(v) => Some(v),
            PerControl::Each(v) => v.get(i),
        }
    }

    /// The values for `n` controls, or the length given when it is a list of
    /// the wrong length.
    pub fn expand(&self, n: usize) -> Result<Vec<T>, usize> {
        match self {
            PerControl::All(v) => Ok(vec![v.clone(); n]),
            PerControl::Each(v) if v.len() == n => Ok(v.clone()),
            PerControl::Each(v) => Err(v.len()),
        }
    }

    /// Every value stated.
    pub fn values(&self) -> &[T] {
        match self {
            PerControl::All(v) => std::slice::from_ref(v),
            PerControl::Each(v) => v,
        }
    }
}

// =============================================================================
// Parts
// =============================================================================

/// One part of the test (single-output Mixed Mode's `Part`, with the MIMO
/// bodies).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MimoPart {
    /// Stable id, unique in the list; `follow` names a part by it.
    pub id: String,
    /// The name an operator gave it; may be empty.
    #[serde(default)]
    pub name: String,
    /// Whether the part plays. A part switched off is still validated.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// When the part plays on the test clock.
    #[serde(default)]
    pub window: PartWindow,
    /// What the part is, tagged by `kind`.
    pub body: MimoPartBody,
}

fn yes() -> bool {
    true
}

impl MimoPart {
    /// The part's kind as its wire name.
    pub fn kind(&self) -> &str {
        self.body.kind()
    }

    /// The part's schedule, for a tone or a narrowband.
    pub fn schedule(&self) -> Option<&PartSchedule> {
        match &self.body {
            MimoPartBody::Tone(t) => Some(&t.schedule),
            MimoPartBody::Narrowband(b) => Some(&b.schedule),
            MimoPartBody::Floor(_) | MimoPartBody::Unknown { .. } => None,
        }
    }
}

/// What a part is, tagged by `kind` (`floor`, `tone`, `narrowband`). A kind
/// this program does not build reads as [`MimoPartBody::Unknown`] and is
/// refused `unknown_part_kind` at validation.
#[derive(Debug, Clone, PartialEq)]
pub enum MimoPartBody {
    /// The broadband random floor; its spectrum is the reference matrix.
    Floor(MimoFloorPart),
    /// A sine tone on every drive.
    Tone(MimoTonePart),
    /// A narrow band of random over the floor.
    Narrowband(MimoNarrowbandPart),
    /// A kind this program does not build.
    Unknown {
        /// The kind as written.
        kind: String,
        /// The part's other fields as written.
        fields: Map<String, Value>,
    },
}

impl MimoPartBody {
    /// The kind's wire name.
    pub fn kind(&self) -> &str {
        match self {
            MimoPartBody::Floor(_) => "floor",
            MimoPartBody::Tone(_) => "tone",
            MimoPartBody::Narrowband(_) => "narrowband",
            MimoPartBody::Unknown { kind, .. } => kind,
        }
    }
}

impl Serialize for MimoPartBody {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let body = match self {
            MimoPartBody::Floor(p) => serde_json::to_value(p),
            MimoPartBody::Tone(p) => serde_json::to_value(p),
            MimoPartBody::Narrowband(p) => serde_json::to_value(p),
            MimoPartBody::Unknown { fields, .. } => Ok(Value::Object(fields.clone())),
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

impl<'de> Deserialize<'de> for MimoPartBody {
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
                .map(MimoPartBody::Floor)
                .map_err(D::Error::custom),
            "tone" => serde_json::from_value(v)
                .map(MimoPartBody::Tone)
                .map_err(D::Error::custom),
            "narrowband" => serde_json::from_value(v)
                .map(MimoPartBody::Narrowband)
                .map_err(D::Error::custom),
            _ => {
                let Value::Object(fields) = v else {
                    unreachable!("built from a map")
                };
                Ok(MimoPartBody::Unknown { kind, fields })
            }
        }
    }
}

/// The floor: its spectrum is the reference matrix ([`MimoMixedConfig::reference`]);
/// the part states its tolerances about the reference's autospectra, per
/// control (MIMO Random's tolerances, design D6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MimoFloorPart {
    /// Alarm band `(minus_db, plus_db)`, positive magnitudes; one pair or one
    /// per control.
    #[serde(default = "floor_alarm")]
    pub alarm_db: PerControl<(f64, f64)>,
    /// Abort band, likewise.
    #[serde(default = "floor_abort")]
    pub abort_db: PerControl<(f64, f64)>,
}

impl Default for MimoFloorPart {
    fn default() -> Self {
        MimoFloorPart {
            alarm_db: floor_alarm(),
            abort_db: floor_abort(),
        }
    }
}

fn floor_alarm() -> PerControl<(f64, f64)> {
    PerControl::All(DEFAULT_FLOOR_ALARM_DB)
}
fn floor_abort() -> PerControl<(f64, f64)> {
    PerControl::All(DEFAULT_FLOOR_ABORT_DB)
}
fn tone_alarm() -> PerControl<(f64, f64)> {
    PerControl::All(DEFAULT_TONE_ALARM_DB)
}
fn tone_abort() -> PerControl<(f64, f64)> {
    PerControl::All(DEFAULT_TONE_ABORT_DB)
}
fn band_alarm() -> PerControl<(f64, f64)> {
    PerControl::All(DEFAULT_BAND_ALARM_DB)
}
fn band_abort() -> PerControl<(f64, f64)> {
    PerControl::All(DEFAULT_BAND_ABORT_DB)
}

/// A sine tone: one oscillator played on every drive with a complex
/// amplitude per drive, controlled at every control (design D9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MimoTonePart {
    /// How its frequency moves.
    pub schedule: PartSchedule,
    /// Its full-level peak amplitude per control (constant or a profile
    /// against frequency): one for every control or one per control.
    pub amplitude: PerControl<ToneAmplitude>,
    /// Whether the phases per control are stated. Without them the reference
    /// is amplitude only and the tone law chooses the phases.
    #[serde(default)]
    pub phases_stated: bool,
    /// The phase per control, degrees, against the tone's oscillator, when
    /// `phases_stated`: one per control.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phase_deg: Vec<f64>,
    /// Alarm band on its tracked amplitude per control, dB.
    #[serde(default = "tone_alarm")]
    pub alarm_db: PerControl<(f64, f64)>,
    /// Abort band, dB.
    #[serde(default = "tone_abort")]
    pub abort_db: PerControl<(f64, f64)>,
}

impl MimoTonePart {
    /// The tone's reference at the controls at `f_hz`, full level, peak: a
    /// complex amplitude per control (phase zero when the phases are not
    /// stated). `None` where a profile does not reach.
    pub fn reference_at(&self, n: usize, f_hz: f64) -> Option<Vec<hertzforge_sdm::C64>> {
        (0..n)
            .map(|i| {
                let a = self.amplitude.get(i)?.g_pk_at(f_hz)?;
                let ph = if self.phases_stated {
                    self.phase_deg.get(i).copied().unwrap_or(0.0).to_radians()
                } else {
                    0.0
                };
                Some(hertzforge_sdm::C64::from_polar(a, ph))
            })
            .collect()
    }
}

/// A narrow band of random over the floor (design D10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MimoNarrowbandPart {
    /// How its center moves.
    pub schedule: PartSchedule,
    /// Its width.
    pub width: BandWidth,
    /// What it adds to the floor, tagged by `kind` (`raise`, `added`;
    /// `matrix` reserved).
    pub statement: BandStatement,
    /// Alarm band on its measured level per control, dB.
    #[serde(default = "band_alarm")]
    pub alarm_db: PerControl<(f64, f64)>,
    /// Abort band, dB.
    #[serde(default = "band_abort")]
    pub abort_db: PerControl<(f64, f64)>,
}

// =============================================================================
// Drives and laws
// =============================================================================

/// A drive output: MIMO Random's [`DriveSpec`] in a form that reads and
/// writes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MimoDrive {
    /// The drive row's channel index in the channel table.
    pub channel_index: u32,
    /// The voltage ceiling of the random part, V rms over the grid.
    pub ceiling_vrms: f64,
    /// The output's peak ceiling, volts: the summed drive (random plus every
    /// tone) is held under it (design D8).
    pub peak_v: f64,
    /// A drive whose realized rms reaches this aborts the run; `None` takes
    /// the ceiling plus 1 dB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort_vrms: Option<f64>,
    /// The drive's abort, volts peak, above `peak_v`: a summed drive frame at
    /// or over it ends the run (`drive_vpk`), so the clamp at `peak_v` is a
    /// backstop under it (MIMO Sine's `abort_vpk`). `None` takes `peak_v`
    /// plus 1 dB, so a setup written before the field loads unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort_vpk: Option<f64>,
    /// The highest rms the identification may use on this output.
    pub pretest_max_vrms: f64,
    /// The shaker on this output, for the prediction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shaker: Option<ShakerRecord>,
    /// This shaker's own moving mass.
    #[serde(default)]
    pub moving_mass: MovingMass,
}

impl MimoDrive {
    /// A drive as MIMO Random's [`DriveSpec::new`] makes one.
    pub fn new(channel_index: u32, ceiling_vrms: f64) -> Self {
        MimoDrive::from_spec(&DriveSpec::new(channel_index, ceiling_vrms))
    }

    /// From MIMO Random's drive.
    pub fn from_spec(d: &DriveSpec) -> Self {
        MimoDrive {
            channel_index: d.channel_index,
            ceiling_vrms: d.ceiling_vrms,
            peak_v: d.peak_v,
            abort_vrms: d.abort_vrms,
            abort_vpk: None,
            pretest_max_vrms: d.pretest_max_vrms,
            shaker: d.shaker.clone(),
            moving_mass: d.moving_mass,
        }
    }

    /// The peak abort in force, volts: `abort_vpk`, or `peak_v` plus 1 dB.
    pub fn abort_level_vpk(&self) -> f64 {
        self.abort_vpk
            .unwrap_or(self.peak_v * 10f64.powf(DEFAULT_ABORT_OVER_PEAK_DB / 20.0))
    }

    /// As MIMO Random's drive.
    pub fn to_spec(&self) -> DriveSpec {
        DriveSpec {
            channel_index: self.channel_index,
            ceiling_vrms: self.ceiling_vrms,
            peak_v: self.peak_v,
            abort_vrms: self.abort_vrms,
            pretest_max_vrms: self.pretest_max_vrms,
            shaker: self.shaker.clone(),
            moving_mass: self.moving_mass,
        }
    }
}

/// The five choices of the run (design D4): the floor's `random-mimo` law,
/// the tones' `sine-mimo` law (one instance per tone), the band law, the
/// ceiling rule and the fill rule, each `{id, params}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MimoMixedLawChoice {
    /// The floor law.
    #[serde(default = "floor_law")]
    pub floor: LawPick,
    /// The tone law, one instance per tone.
    #[serde(default = "tone_law")]
    pub tones: LawPick,
    /// The band law ([`BAND_LAW_IDS`]).
    #[serde(default = "band_law")]
    pub bands: LawPick,
    /// The ceiling rule ([`CEILING_RULE_IDS`]).
    #[serde(default = "ceiling_rule")]
    pub ceiling_rule: LawPick,
    /// The fill rule ([`FILL_RULE_IDS`]).
    #[serde(default = "fill_rule")]
    pub fill_rule: LawPick,
}

fn floor_law() -> LawPick {
    LawPick::new(DEFAULT_FLOOR_LAW_ID)
}
fn tone_law() -> LawPick {
    LawPick::new(DEFAULT_TONE_LAW_ID)
}
fn band_law() -> LawPick {
    LawPick::new(DEFAULT_BAND_LAW_ID)
}
fn ceiling_rule() -> LawPick {
    LawPick::new(DEFAULT_CEILING_RULE_ID)
}
fn fill_rule() -> LawPick {
    LawPick::new(DEFAULT_FILL_RULE_ID)
}

impl Default for MimoMixedLawChoice {
    fn default() -> Self {
        MimoMixedLawChoice {
            floor: floor_law(),
            tones: tone_law(),
            bands: band_law(),
            ceiling_rule: ceiling_rule(),
            fill_rule: fill_rule(),
        }
    }
}

// =============================================================================
// The configuration
// =============================================================================

/// A MIMO Mixed Mode test's configuration (design section 4).
///
/// The grid is the reference's (`reference.df_hz`, `reference.lines()`), and
/// its controlled band `reference.band` is the floor's band: every tone and
/// band stays inside it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MimoMixedConfig {
    /// Estimation and loop settings (MIMO Random's).
    #[serde(default)]
    pub estimation: Estimation,
    /// The control channels' channel-table indices, in the reference's order.
    pub controls: Vec<u32>,
    /// The drives.
    pub drives: Vec<MimoDrive>,
    /// The limit channels (MIMO Random's).
    #[serde(default)]
    pub limits: Vec<LimitSpec>,
    /// The floor's reference at full level: an `n` by `n` spectral matrix per
    /// line (MIMO Random's reference, any entry method).
    pub reference: Reference,
    /// The parts.
    pub parts: Vec<MimoPart>,
    /// Half-width, Hz, of the lines around a tone left out of the floor
    /// estimate (widened per frame by half the tone's travel).
    #[serde(default = "default_exclusion")]
    pub exclusion_hz: f64,
    /// Every tone tracker's noise bandwidth, Hz.
    #[serde(default = "default_tracking")]
    pub tracking_hz: f64,
    /// The level schedule; one level moves every part by the same dB.
    pub schedule: LevelSchedule,
    /// Test time at full level before the scheduled stop, seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_level_duration_s: Option<f64>,
    /// Ramp time per level change, seconds; at least two control ticks
    /// (`ramp_too_short`). Absent, [`DEFAULT_LEVEL_RAMP_S`].
    #[serde(default = "default_level_ramp")]
    pub level_ramp_s: f64,
    /// Controlled-stop ramp, seconds.
    #[serde(default)]
    pub shutdown_ramp_s: f64,
    /// How long a tone or band takes to ramp in or out, seconds; at least
    /// two control ticks (`ramp_too_short`).
    #[serde(default = "default_part_ramp")]
    pub part_ramp_s: f64,
    /// The longest the floor plays alone before tones and bands ramp in.
    #[serde(default = "default_floor_settle")]
    pub floor_settle_s: f64,
    /// Soft-start depth, dB below the schedule's start level.
    #[serde(default = "default_soft_start")]
    pub soft_start_db: f64,
    /// dB added to the floor's tolerances below the start level.
    #[serde(default = "default_soft_start_relax")]
    pub soft_start_relax_db: f64,
    /// The identification (MIMO Random's); `None` takes its defaults for the
    /// drive count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identification: Option<Identification>,
    /// The identification gates (MIMO Random's).
    #[serde(default)]
    pub gates: Gates,
    /// The floor's change caps (MIMO Random's).
    #[serde(default)]
    pub caps: Caps,
    /// Protections (MIMO Random's).
    #[serde(default)]
    pub protections: Protections,
    /// Whether the model is updated during the run. Only `Hold` is built
    /// (`frf_update_not_built`).
    #[serde(default = "default_frf_mode")]
    pub frf_mode: FrfMode,
    /// The random part's crest factor for the peak prediction.
    #[serde(default = "default_crest_factor")]
    pub crest_factor: f64,
    /// The sigma clip on the random part, per drive; `None` takes the crest
    /// factor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive_clip_sigma: Option<f64>,
    /// The test article's mass, pounds, split across the shakers.
    #[serde(default)]
    pub article_mass_lb: f64,
    /// Capture rules (Random's).
    #[serde(default)]
    pub capture: CaptureRules,
    /// The laws and rules.
    #[serde(default)]
    pub law: MimoMixedLawChoice,
    /// The tones' control tick, seconds; `None` takes 10 ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_tick_s: Option<f64>,
    /// Operator metadata (Random's).
    #[serde(default)]
    pub metadata: Vec<MetaField>,
    /// The random synthesis seed.
    #[serde(default)]
    pub seed: u64,
}

/// The default control tick, seconds (design section 3).
pub const DEFAULT_CONTROL_TICK_S: f64 = 0.01;

/// A drive's peak abort over its peak ceiling when `abort_vpk` is not
/// stated, dB (as MIMO Random's rms abort over its rms ceiling).
pub const DEFAULT_ABORT_OVER_PEAK_DB: f64 = 1.0;

/// The level ramp when a setup does not state one, seconds. A level ramp
/// under two control ticks is refused, so the old default of zero would
/// refuse a setup that never named the field.
pub const DEFAULT_LEVEL_RAMP_S: f64 = 0.5;

fn default_level_ramp() -> f64 {
    DEFAULT_LEVEL_RAMP_S
}

fn default_exclusion() -> f64 {
    DEFAULT_EXCLUSION_HZ
}
fn default_tracking() -> f64 {
    DEFAULT_TRACKING_HZ
}
fn default_part_ramp() -> f64 {
    hertzforge_mixed::DEFAULT_PART_RAMP_S
}
fn default_floor_settle() -> f64 {
    hertzforge_mixed::DEFAULT_FLOOR_SETTLE_S
}
fn default_soft_start() -> f64 {
    hertzforge_mixed::parts::DEFAULT_SOFT_START_DB
}
fn default_soft_start_relax() -> f64 {
    hertzforge_mixed::parts::DEFAULT_SOFT_START_RELAX_DB
}
fn default_frf_mode() -> FrfMode {
    FrfMode::Hold
}
fn default_crest_factor() -> f64 {
    hertzforge_shaker_db::DEFAULT_CREST_FACTOR
}

// =============================================================================
// Refusals
// =============================================================================

/// Why a configuration is refused. Each variant has a stable wire name
/// ([`MimoMixedConfigError::name`]); the single-output refusals keep theirs.
#[derive(Debug, Clone, PartialEq)]
pub enum MimoMixedConfigError {
    /// A single-output refusal, by its own name (`no_floor`,
    /// `unknown_part_kind`, `part_out_of_band`, `parts_too_close`, the follow
    /// errors, too many tones or bands, `bad_part`, `bad_config`, ...).
    Parts(ConfigError),
    /// `wrong_control_count`: a per-control list of the wrong length.
    WrongControlCount {
        /// The part's id (empty for the configuration around the parts).
        part: String,
        /// The field.
        field: &'static str,
        /// Controls in the test.
        expected: usize,
        /// Entries given.
        got: usize,
    },
    /// `band_not_additive`: a band whose increment over the floor is not
    /// positive semi-definite (unequal ratios on a coherent floor, theory
    /// 5.1).
    BandNotAdditive {
        /// The band's id.
        part: String,
        /// The first line where the increment has a negative eigenvalue.
        line: usize,
        /// Its smallest eigenvalue over its largest modulus.
        min_eigenvalue_rel: f64,
    },
    /// `band_matrix_not_built`: a band stated as a full matrix.
    BandMatrixNotBuilt {
        /// The band's id.
        part: String,
    },
    /// `law_refuses_tone`: the tone law's manifest does not accept this tone
    /// (raised by the engine when it resolves the law).
    LawRefusesTone {
        /// The tone's id.
        part: String,
        /// The law's id.
        law: String,
        /// The test type the law was asked for.
        test_type: String,
    },
    /// `budget_cannot_fit`: the forecast budget does not fit on a drive
    /// (raised by [`crate::budget::forecast`]).
    BudgetCannotFit {
        /// Test time of the first misfit, seconds.
        time_s: f64,
        /// The drive's index in [`MimoMixedConfig::drives`].
        drive: usize,
        /// The peak the drive would need, volts.
        need_v: f64,
        /// Its ceiling at the level, volts.
        ceiling_v: f64,
    },
    /// `tone_model_not_finite`: the model gives no finite drive for a tone
    /// where its schedule plays it (raised by [`crate::budget::forecast`]),
    /// so neither its feed-forward nor its budget can be stated.
    ToneModelNotFinite {
        /// The tone's id.
        part: String,
        /// The frequency, Hz.
        f_hz: f64,
    },
    /// `frf_update_not_built`: a model update during the run.
    FrfUpdateNotBuilt,
    /// `unknown_band_law`.
    UnknownBandLaw {
        /// The id as written.
        id: String,
    },
    /// `unknown_ceiling_rule`.
    UnknownCeilingRule {
        /// The id as written.
        id: String,
    },
    /// `unknown_fill_rule`.
    UnknownFillRule {
        /// The id as written.
        id: String,
    },
    /// `abort_below_ceiling`: a drive's peak abort not above its peak
    /// ceiling (MIMO Sine's name).
    AbortBelowCeiling {
        /// The drive's index in [`MimoMixedConfig::drives`].
        drive: usize,
        /// The abort, volts peak.
        abort_vpk: f64,
        /// The ceiling, volts peak.
        peak_v: f64,
    },
    /// `ramp_too_short`: a level or part ramp under two control ticks, so a
    /// ramp could start and end between two of the ceiling rule's looks.
    RampTooShort {
        /// The field (`level_ramp_s` or `part_ramp_s`).
        field: &'static str,
        /// The ramp, seconds.
        seconds: f64,
        /// The shortest accepted, seconds.
        min_s: f64,
    },
}

impl MimoMixedConfigError {
    /// The stable wire name.
    pub fn name(&self) -> &'static str {
        match self {
            MimoMixedConfigError::Parts(e) => e.name(),
            MimoMixedConfigError::WrongControlCount { .. } => "wrong_control_count",
            MimoMixedConfigError::BandNotAdditive { .. } => "band_not_additive",
            MimoMixedConfigError::BandMatrixNotBuilt { .. } => "band_matrix_not_built",
            MimoMixedConfigError::LawRefusesTone { .. } => "law_refuses_tone",
            MimoMixedConfigError::BudgetCannotFit { .. } => "budget_cannot_fit",
            MimoMixedConfigError::ToneModelNotFinite { .. } => "tone_model_not_finite",
            MimoMixedConfigError::FrfUpdateNotBuilt => "frf_update_not_built",
            MimoMixedConfigError::UnknownBandLaw { .. } => "unknown_band_law",
            MimoMixedConfigError::UnknownCeilingRule { .. } => "unknown_ceiling_rule",
            MimoMixedConfigError::UnknownFillRule { .. } => "unknown_fill_rule",
            MimoMixedConfigError::AbortBelowCeiling { .. } => "abort_below_ceiling",
            MimoMixedConfigError::RampTooShort { .. } => "ramp_too_short",
        }
    }

    /// The part the refusal is about, when it is about one.
    pub fn part(&self) -> Option<&str> {
        match self {
            MimoMixedConfigError::Parts(e) => e.part(),
            MimoMixedConfigError::WrongControlCount { part, .. } if !part.is_empty() => Some(part),
            MimoMixedConfigError::BandNotAdditive { part, .. }
            | MimoMixedConfigError::BandMatrixNotBuilt { part }
            | MimoMixedConfigError::LawRefusesTone { part, .. }
            | MimoMixedConfigError::ToneModelNotFinite { part, .. } => Some(part),
            _ => None,
        }
    }
}

impl fmt::Display for MimoMixedConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MimoMixedConfigError::Parts(e) => write!(f, "{e}"),
            MimoMixedConfigError::WrongControlCount {
                part,
                field,
                expected,
                got,
            } => write!(
                f,
                "{part} {field}: {got} entries for {expected} controls (one value, or one per control)"
            ),
            MimoMixedConfigError::BandNotAdditive {
                part,
                line,
                min_eigenvalue_rel,
            } => write!(
                f,
                "band {part} is not added random: on line {line} its increment over the floor has a \
                 negative eigenvalue ({min_eigenvalue_rel:.3} of its largest); state equal raises \
                 on every control, or an added density with a coherence"
            ),
            MimoMixedConfigError::BandMatrixNotBuilt { part } => {
                write!(f, "band {part}: a band stated as a full matrix is not built yet")
            }
            MimoMixedConfigError::LawRefusesTone {
                part,
                law,
                test_type,
            } => write!(f, "tone {part}: law {law} does not accept this tone for {test_type}"),
            MimoMixedConfigError::BudgetCannotFit {
                time_s,
                drive,
                need_v,
                ceiling_v,
            } => write!(
                f,
                "drive {drive} cannot fit at {time_s:.1} s: it would need {need_v:.2} V peak against \
                 {ceiling_v:.2} V"
            ),
            MimoMixedConfigError::ToneModelNotFinite { part, f_hz } => write!(
                f,
                "tone {part}: the model gives no finite drive at {f_hz:.1} Hz, so its feed-forward \
                 and its budget cannot be stated; identify again"
            ),
            MimoMixedConfigError::FrfUpdateNotBuilt => {
                f.write_str("updating the model during the run is not built yet")
            }
            MimoMixedConfigError::UnknownBandLaw { id } => write!(f, "unknown band law {id:?}"),
            MimoMixedConfigError::UnknownCeilingRule { id } => {
                write!(f, "unknown ceiling rule {id:?}")
            }
            MimoMixedConfigError::UnknownFillRule { id } => write!(f, "unknown fill rule {id:?}"),
            MimoMixedConfigError::AbortBelowCeiling {
                drive,
                abort_vpk,
                peak_v,
            } => write!(
                f,
                "drive {drive}: a peak abort of {abort_vpk} V; above its peak ceiling ({peak_v} V) \
                 is accepted"
            ),
            MimoMixedConfigError::RampTooShort {
                field,
                seconds,
                min_s,
            } => write!(
                f,
                "{field} of {seconds} s; at least two control ticks ({min_s} s) is accepted"
            ),
        }
    }
}

impl std::error::Error for MimoMixedConfigError {}

impl From<ConfigError> for MimoMixedConfigError {
    fn from(e: ConfigError) -> Self {
        MimoMixedConfigError::Parts(e)
    }
}

fn bad_config(field: &'static str, why: impl Into<String>) -> MimoMixedConfigError {
    MimoMixedConfigError::Parts(ConfigError::BadConfig {
        field,
        why: why.into(),
    })
}

fn bad_part(part: &str, field: &'static str, why: impl Into<String>) -> MimoMixedConfigError {
    MimoMixedConfigError::Parts(ConfigError::BadPart {
        part: part.to_owned(),
        field,
        why: why.into(),
    })
}

fn finite_pos(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

// =============================================================================
// Validation
// =============================================================================

impl MimoMixedConfig {
    /// Control channels.
    pub fn n(&self) -> usize {
        self.controls.len()
    }

    /// Drives.
    pub fn m(&self) -> usize {
        self.drives.len()
    }

    /// Limit channels.
    pub fn p(&self) -> usize {
        self.limits.len()
    }

    /// The grid's line spacing, Hz (the reference's).
    pub fn df_hz(&self) -> f64 {
        self.reference.df_hz
    }

    /// Lines on the grid (the reference's).
    pub fn lines(&self) -> usize {
        self.reference.lines()
    }

    /// The control tick, seconds.
    pub fn control_tick_s(&self) -> f64 {
        self.control_tick_s.unwrap_or(DEFAULT_CONTROL_TICK_S)
    }

    /// The identification settings in force.
    pub fn identification(&self) -> Identification {
        self.identification
            .unwrap_or_else(|| Identification::for_drives(self.m()))
    }

    /// The floor's band in Hz: the reference's controlled band
    /// `[lo, hi)` in lines, from line `max(lo, 1)` to line `hi - 1`.
    pub fn floor_band_hz(&self) -> (f64, f64) {
        let (lo, hi) = self.reference.band;
        let df = self.df_hz();
        (lo.max(1) as f64 * df, hi.saturating_sub(1) as f64 * df)
    }

    /// The tones, in part order, with their indices in `parts`.
    pub fn tones(&self) -> impl Iterator<Item = (usize, &MimoPart, &MimoTonePart)> {
        self.parts
            .iter()
            .enumerate()
            .filter_map(|(i, p)| match &p.body {
                MimoPartBody::Tone(t) => Some((i, p, t)),
                _ => None,
            })
    }

    /// The narrowbands, in part order, with their indices in `parts`.
    pub fn bands(&self) -> impl Iterator<Item = (usize, &MimoPart, &MimoNarrowbandPart)> {
        self.parts
            .iter()
            .enumerate()
            .filter_map(|(i, p)| match &p.body {
                MimoPartBody::Narrowband(b) => Some((i, p, b)),
                _ => None,
            })
    }

    /// The floor part, if there is exactly one.
    pub fn floor(&self) -> Option<&MimoFloorPart> {
        let mut floors = self.parts.iter().filter_map(|p| match &p.body {
            MimoPartBody::Floor(f) => Some(f),
            _ => None,
        });
        let f = floors.next()?;
        floors.next().is_none().then_some(f)
    }

    /// The floor's alarm and abort lines as MIMO Random states them: PSDs
    /// per line per control about the reference's autospectra, the floor
    /// part's dB bands applied (`p 10^(+db/10)` above, `p 10^(-db/10)`
    /// below); `NaN` outside the band and where the reference is zero.
    pub fn floor_tolerances(&self) -> hertzforge_sdm::law::Tolerances {
        self.floor_tolerances_about(&self.reference.sdm)
    }

    /// The floor's alarm and abort lines about `sdm` (the floor law's
    /// reference: the floor plus every band folded into it), the floor
    /// part's dB bands applied as in [`Self::floor_tolerances`].
    pub fn floor_tolerances_about(
        &self,
        sdm: &hertzforge_sdm::HermitianStack,
    ) -> hertzforge_sdm::law::Tolerances {
        let (lines, n) = (self.lines(), self.n());
        let mut t = hertzforge_sdm::law::Tolerances::none(lines, n);
        let Some(f) = self.floor() else { return t };
        let (lo, hi) = self.reference.band;
        for l in lo..hi.min(lines) {
            for i in 0..n {
                let p = sdm.get(l, i, i).re;
                if p.is_nan() || p <= 0.0 {
                    continue;
                }
                let al = f.alarm_db.get(i).copied().unwrap_or(DEFAULT_FLOOR_ALARM_DB);
                let ab = f.abort_db.get(i).copied().unwrap_or(DEFAULT_FLOOR_ABORT_DB);
                let k = l * n + i;
                t.alarm_lower[k] = p * 10f64.powf(-al.0 / 10.0);
                t.alarm_upper[k] = p * 10f64.powf(al.1 / 10.0);
                t.abort_lower[k] = p * 10f64.powf(-ab.0 / 10.0);
                t.abort_upper[k] = p * 10f64.powf(ab.1 / 10.0);
            }
        }
        t
    }

    /// The configuration projected onto control `control` as a single-output
    /// Mixed Mode configuration: the same parts, windows and schedules on the
    /// same grid; the floor a flat breakpoint table over the reference's band;
    /// each tone at that control's amplitude; each band at a placeholder
    /// level of 1 g^2/Hz (the statement is checked here, not there). The
    /// single-output validation and part clock run on it unchanged.
    pub fn projection(&self, control: usize) -> MixedConfig {
        let (lo_hz, hi_hz) = self.floor_band_hz();
        let mut table = BreakpointTable::new("floor");
        table.push(BreakpointRow::psd(lo_hz, 1.0));
        table.push(BreakpointRow::psd(hi_hz, 1.0));
        let pair = |v: &PerControl<(f64, f64)>| v.get(control).copied().unwrap_or((1.0, 1.0));
        let parts = self
            .parts
            .iter()
            .map(|p| Part {
                id: p.id.clone(),
                name: p.name.clone(),
                enabled: p.enabled,
                window: p.window,
                body: match &p.body {
                    MimoPartBody::Floor(f) => PartBody::Floor(FloorPart {
                        breakpoints: table.clone(),
                        alarm_db: pair(&f.alarm_db),
                        abort_db: pair(&f.abort_db),
                    }),
                    MimoPartBody::Tone(t) => PartBody::Tone(TonePart {
                        schedule: t.schedule.clone(),
                        amplitude: t
                            .amplitude
                            .get(control)
                            .cloned()
                            .unwrap_or(ToneAmplitude::Constant { g_pk: 1.0 }),
                        alarm_db: pair(&t.alarm_db),
                        abort_db: pair(&t.abort_db),
                    }),
                    MimoPartBody::Narrowband(b) => PartBody::Narrowband(NarrowbandPart {
                        schedule: b.schedule.clone(),
                        level_g2_hz: 1.0,
                        width: b.width,
                        alarm_db: pair(&b.alarm_db),
                        abort_db: pair(&b.abort_db),
                    }),
                    MimoPartBody::Unknown { kind, fields } => PartBody::Unknown {
                        kind: kind.clone(),
                        fields: fields.clone(),
                    },
                },
            })
            .collect();
        MixedConfig {
            df_hz: self.df_hz(),
            lines: self.lines(),
            nfft: None,
            blocks_per_window: self.estimation.frames_per_loop.max(1),
            control_dof: 120.0,
            control: ControlSet::single(self.controls.get(control).copied().unwrap_or(0)),
            parts,
            exclusion_hz: self.exclusion_hz,
            tracking_hz: self.tracking_hz,
            schedule: self.schedule.clone(),
            full_level_duration_s: self.full_level_duration_s,
            level_ramp_s: self.level_ramp_s,
            shutdown_ramp_s: self.shutdown_ramp_s,
            part_ramp_s: self.part_ramp_s,
            floor_settle_s: self.floor_settle_s,
            soft_start_db: self.soft_start_db,
            soft_start_relax_db: self.soft_start_relax_db,
            shaker: None,
            moving_mass: MovingMass::default(),
            crest_factor: self.crest_factor,
            identification: MixedIdentification::default(),
            limits: MixedLimits {
                drive_clip_sigma: self.drive_clip_sigma,
                ..MixedLimits::default()
            },
            capture: self.capture.clone(),
            law: MixedLawChoice {
                floor: self.law.floor.clone(),
                tones: self.law.tones.clone(),
            },
            law_guards: None,
            control_tick_s: self.control_tick_s,
            metadata: self.metadata.clone(),
        }
    }

    /// Validate; the first refusal found. `fs_hz`, when known, bounds the
    /// band by Nyquist as the single-output validation does.
    pub fn validate(&self, fs_hz: Option<f64>) -> Result<(), MimoMixedConfigError> {
        self.validate_around()?;
        self.validate_statements()?;
        for c in 0..self.n() {
            self.projection(c).validate(fs_hz)?;
        }
        self.validate_bands()
    }

    fn validate_around(&self) -> Result<(), MimoMixedConfigError> {
        let (n, m, p) = (self.n(), self.m(), self.p());
        if n == 0 || n > MAX_CONTROLS {
            return Err(bad_config(
                "controls",
                format!("1 to {MAX_CONTROLS} control channels"),
            ));
        }
        if m == 0 || m > MAX_DRIVES {
            return Err(bad_config("drives", format!("1 to {MAX_DRIVES} drives")));
        }
        if p > MAX_LIMITS {
            return Err(bad_config(
                "limits",
                format!("at most {MAX_LIMITS} limit channels"),
            ));
        }
        self.reference
            .validate()
            .map_err(|e| bad_config("reference", e.to_string()))?;
        if self.reference.n() != n {
            return Err(bad_config(
                "reference",
                format!(
                    "a {n} by {n} reference for {n} controls, not {} by {}",
                    self.reference.n(),
                    self.reference.n()
                ),
            ));
        }
        if self.reference.response_transformation.is_some()
            || self.reference.drive_transformation.is_some()
        {
            return Err(bad_config(
                "reference",
                "a reference written on the channels (transformations are not built for this test)",
            ));
        }
        let (lo, hi) = self.reference.band;
        if hi < lo.max(1) + 2 {
            return Err(bad_config("reference", "a band of at least two lines"));
        }
        for d in &self.drives {
            if !(finite_pos(d.ceiling_vrms)
                && finite_pos(d.peak_v)
                && finite_pos(d.pretest_max_vrms))
            {
                return Err(bad_config(
                    "drives",
                    "each drive's ceiling, peak and identification limit greater than zero",
                ));
            }
            if d.abort_vrms
                .is_some_and(|a| !(finite_pos(a) && a >= d.ceiling_vrms))
            {
                return Err(bad_config("drives", "an abort at or above the ceiling"));
            }
        }
        for (j, d) in self.drives.iter().enumerate() {
            let a = d.abort_level_vpk();
            if !(a.is_finite() && a > d.peak_v) {
                return Err(MimoMixedConfigError::AbortBelowCeiling {
                    drive: j,
                    abort_vpk: a,
                    peak_v: d.peak_v,
                });
            }
        }
        let min_s = 2.0 * self.control_tick_s();
        for (field, seconds) in [
            ("level_ramp_s", self.level_ramp_s),
            ("part_ramp_s", self.part_ramp_s),
        ] {
            if seconds.is_finite() && seconds >= 0.0 && seconds < min_s {
                return Err(MimoMixedConfigError::RampTooShort {
                    field,
                    seconds,
                    min_s,
                });
            }
        }
        for l in &self.limits {
            if l.ceiling_psd.is_empty()
                || (l.ceiling_psd.len() != 1 && l.ceiling_psd.len() != self.lines())
                || l.ceiling_psd.iter().any(|v| !(v.is_finite() && *v > 0.0))
            {
                return Err(bad_config(
                    "limits",
                    "a ceiling greater than zero, one value or one per line",
                ));
            }
            if !(l.abort_margin_db.is_finite() && l.abort_margin_db >= 0.0) {
                return Err(bad_config("limits", "an abort margin of zero dB or more"));
            }
        }
        let e = &self.estimation;
        if e.frames_per_loop == 0 {
            return Err(bad_config("estimation", "at least one frame per loop"));
        }
        if !(e.memory_loops.is_finite() && e.memory_loops >= 1.0) {
            return Err(bad_config("estimation", "a memory of at least one loop"));
        }
        if !(e.overlap.is_finite() && (0.0..0.95).contains(&e.overlap)) {
            return Err(bad_config("estimation", "an overlap from 0 to under 0.95"));
        }
        if !(self.article_mass_lb.is_finite() && self.article_mass_lb >= 0.0) {
            return Err(bad_config("article_mass_lb", "zero or more"));
        }
        if matches!(self.frf_mode, FrfMode::Update { .. }) {
            return Err(MimoMixedConfigError::FrfUpdateNotBuilt);
        }
        if self.law.floor.id.is_empty() || self.law.tones.id.is_empty() {
            return Err(bad_config("law", "each law is named by its catalog id"));
        }
        if !BAND_LAW_IDS.contains(&self.law.bands.id.as_str()) {
            return Err(MimoMixedConfigError::UnknownBandLaw {
                id: self.law.bands.id.clone(),
            });
        }
        if !CEILING_RULE_IDS.contains(&self.law.ceiling_rule.id.as_str()) {
            return Err(MimoMixedConfigError::UnknownCeilingRule {
                id: self.law.ceiling_rule.id.clone(),
            });
        }
        if !FILL_RULE_IDS.contains(&self.law.fill_rule.id.as_str()) {
            return Err(MimoMixedConfigError::UnknownFillRule {
                id: self.law.fill_rule.id.clone(),
            });
        }
        crate::band::band_law_params(&self.law.bands)
            .map_err(|why| bad_config("law.bands", why))?;
        crate::fill::fill_rule(&self.law.fill_rule)
            .map_err(|why| bad_config("law.fill_rule", why))?;
        Ok(())
    }

    /// Per-control lists of the right length and statements that can be used.
    fn validate_statements(&self) -> Result<(), MimoMixedConfigError> {
        let n = self.n();
        let count = |p: &MimoPart, field: &'static str, r: Result<(), usize>| {
            r.map_err(|got| MimoMixedConfigError::WrongControlCount {
                part: p.id.clone(),
                field,
                expected: n,
                got,
            })
        };
        let len_of = |len: usize| if len == n { Ok(()) } else { Err(len) };
        for p in &self.parts {
            match &p.body {
                MimoPartBody::Floor(f) => {
                    count(p, "alarm_db", f.alarm_db.expand(n).map(|_| ()))?;
                    count(p, "abort_db", f.abort_db.expand(n).map(|_| ()))?;
                }
                MimoPartBody::Tone(t) => {
                    count(p, "amplitude", t.amplitude.expand(n).map(|_| ()))?;
                    if t.phases_stated {
                        count(p, "phase_deg", len_of(t.phase_deg.len()))?;
                        if t.phase_deg.iter().any(|v| !v.is_finite()) {
                            return Err(bad_part(&p.id, "phase_deg", "finite degrees"));
                        }
                    } else if !t.phase_deg.is_empty() {
                        return Err(bad_part(
                            &p.id,
                            "phase_deg",
                            "phases are given only when phases_stated is set",
                        ));
                    }
                    count(p, "alarm_db", t.alarm_db.expand(n).map(|_| ()))?;
                    count(p, "abort_db", t.abort_db.expand(n).map(|_| ()))?;
                }
                MimoPartBody::Narrowband(b) => {
                    match &b.statement {
                        BandStatement::Raise { db } => {
                            count(p, "statement.db", db.expand(n).map(|_| ()))?;
                            let v = db.values();
                            if v.iter().any(|d| !(d.is_finite() && *d >= 0.0))
                                || !v.iter().any(|d| *d > 0.0)
                            {
                                return Err(bad_part(
                                    &p.id,
                                    "statement",
                                    "raises of 0 dB or more, at least one greater than 0 dB",
                                ));
                            }
                        }
                        BandStatement::Added {
                            level_g2_hz,
                            coherence,
                        } => {
                            count(
                                p,
                                "statement.level_g2_hz",
                                level_g2_hz.expand(n).map(|_| ()),
                            )?;
                            let v = level_g2_hz.values();
                            if v.iter().any(|a| !(a.is_finite() && *a >= 0.0))
                                || !v.iter().any(|a| *a > 0.0)
                            {
                                return Err(bad_part(
                                    &p.id,
                                    "statement",
                                    "added levels of zero or more, at least one greater than zero",
                                ));
                            }
                            if !(coherence.is_finite() && (0.0..=1.0).contains(coherence)) {
                                return Err(bad_part(&p.id, "statement", "a coherence of 0 to 1"));
                            }
                        }
                        BandStatement::Matrix { .. } => {
                            return Err(MimoMixedConfigError::BandMatrixNotBuilt {
                                part: p.id.clone(),
                            })
                        }
                    }
                    count(p, "alarm_db", b.alarm_db.expand(n).map(|_| ()))?;
                    count(p, "abort_db", b.abort_db.expand(n).map(|_| ()))?;
                }
                MimoPartBody::Unknown { .. } => {}
            }
        }
        Ok(())
    }

    /// Every band classified over every line it reaches in its motion.
    fn validate_bands(&self) -> Result<(), MimoMixedConfigError> {
        let proj = self.projection(0);
        let motions = proj.motions()?;
        let df = self.df_hz();
        for (i, p, b) in self.bands() {
            let Some(mo) = motions[i] else { continue };
            let lo = b.width.edges_at(mo.lo_hz).0;
            let hi = b.width.edges_at(mo.hi_hz).1;
            let a = ((lo / df).floor().max(0.0) as usize).min(self.lines());
            let z = (((hi / df).ceil() as usize) + 1).min(self.lines());
            let c = classify(&b.statement, &self.reference.sdm, (a, z));
            if !c.additive {
                return Err(MimoMixedConfigError::BandNotAdditive {
                    part: p.id.clone(),
                    line: c.worst_line,
                    min_eigenvalue_rel: c.min_eigenvalue_rel,
                });
            }
        }
        Ok(())
    }
}
