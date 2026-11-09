//! The classical shock configuration (schema/PROTOCOL.md, "Classical Shock
//! Configuration").
//!
//! The wire configuration is parsed (a wrong type or a missing part is `bad_params`
//! naming it), then checked so every problem comes back as a named item located by
//! its `field`, then built into the controller's own [`ClassicalTestConfig`] with
//! its closed-loop [`ClassicalControl`]. The wire always sets `control`: it carries
//! no open-loop classical shock. Nothing here is control math; the run is the
//! controller's.

use super::preview::{self, ReferenceFacts};
use crate::protocol::{num, ErrorCode, MethodError};
use crate::table::{Item, Report};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_dsp::shock::{CompensationConfig, CompensationShape};
use hertzforge_law_host::{ParamKind, ParamSpec};
use hertzforge_shaker_db::{FeasibilityReport, MovingMass, ShakerRecord};
use hertzforge_shock_common::{
    pulse_feasibility, DriveLimit, DriveMonitor, DriveMonitorKind, DrivePath,
    FeasibilityTolerances, LadderAdvance, RateModel, ShockLevelSchedule, ShockRunMode, TrainSpec,
};
use hertzforge_shock_host::{
    CaptureRules, ClassicalControl, ClassicalPulseSpec, ClassicalShape, ClassicalTestConfig,
    GunfireSpec, MetaField, Polarity, ToleranceAbort, ToleranceStandard,
};
use serde_json::{json, Map, Value};

/// The rate a configuration asks for when it gives none, Hz.
pub const DEFAULT_FS_HZ: f64 = 51_200.0;
/// Accepted requested rates, Hz.
pub const FS_RANGE: (f64, f64) = (1000.0, 204_800.0);
/// Accepted level range, dB relative to full level.
pub const LEVEL_DB_RANGE: (f64, f64) = (-60.0, 12.0);
/// The most pulses one level may ask for.
pub const MAX_PULSES_PER_RUNG: u32 = 1000;
/// The most pulses at one level without a pass, when nothing is given.
pub const DEFAULT_MAX_PULSES_PER_LEVEL: u32 = 5;
/// The largest drive in volts any field may name.
pub const MAX_DRIVE_V: f64 = 10.0;
/// The longest a spacing floor or a train spacing may be, seconds.
pub const MAX_SPACING_S: f64 = 3600.0;
/// Accepted identification FFT block sizes.
pub const NFFT_RANGE: (u64, u64) = (256, 65_536);
/// Accepted identification lengths, seconds.
pub const SYSID_DURATION_RANGE: (f64, f64) = (0.5, 120.0);

/// How the AUTO ladder moves on from a level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advance {
    /// After the level's pulses, whatever they measured.
    Count,
    /// Only on a passing pulse.
    OnPass,
}

impl Advance {
    pub fn name(self) -> &'static str {
        match self {
            Advance::Count => "count",
            Advance::OnPass => "on_pass",
        }
    }
}

/// A monitored channel as sent.
#[derive(Debug, Clone, PartialEq)]
pub struct WireMonitor {
    pub channel: u32,
    pub unit: String,
    pub peak_ceiling: f64,
    pub kind: DriveMonitorKind,
}

/// The control law a configuration names, with its parameter values by key.
#[derive(Debug, Clone, PartialEq)]
pub struct LawSetup {
    pub id: String,
    pub params: Map<String, Value>,
}

/// What a configuration's checks need to know about a law named by id: its
/// identity, whether it is a classical shock law, and its declared parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct LawFacts {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Empty for a built-in law.
    pub sha256: String,
    pub builtin: bool,
    /// `Err` with the test type it is for when it is not a classical shock law.
    pub world: Result<(), &'static str>,
    pub params: Vec<ParamSpec>,
}

/// Everything a configuration's checks are made against.
pub struct CheckContext<'a> {
    /// The channel table the run would arm with.
    pub table: &'a ChannelTable,
    /// Looks a law up by id.
    pub law: &'a dyn Fn(&str) -> Option<LawFacts>,
    /// The controller timeout, seconds: one pulse's record must be shorter, so a
    /// safe stop is never behind a pulse still playing.
    pub timeout_s: f64,
}

/// Feasibility inputs as sent.
#[derive(Debug, Clone, PartialEq)]
pub struct WireFeasibility {
    pub shaker: ShakerRecord,
    pub moving_mass: MovingMass,
    pub residual_velocity_ips: f64,
    pub residual_disp_in: f64,
}

/// A parsed classical shock configuration, every default filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct WireShock {
    pub title: String,
    pub shape: ClassicalShape,
    pub amplitude_g: f64,
    pub duration_ms: f64,
    pub polarity: Polarity,
    pub compensation: CompensationConfig,
    pub standard: ToleranceStandard,
    pub tolerance_abort: ToleranceAbort,
    pub overshoot_abort: Option<f64>,
    pub fs_hz: f64,
    pub srs_f_lo_hz: f64,
    pub srs_f_hi_hz: f64,
    pub srs_bands_per_octave: f64,
    pub srs_band_tol_db: f64,
    pub pvss: bool,
    pub mode: ShockRunMode,
    pub levels_db: Vec<f64>,
    pub pulses: Vec<u32>,
    pub start_db: f64,
    pub advance: Advance,
    pub max_pulses_per_level: u32,
    pub cooling_s: f64,
    pub settling_s: f64,
    pub compute_s: f64,
    pub sysid_drive_vrms: f64,
    pub sysid_duration_s: f64,
    pub sysid_nfft: u64,
    pub sysid_band_hz: (f64, f64),
    pub sysid_min_mean_coherence: f64,
    pub review: bool,
    pub law: LawSetup,
    pub band_hz: (f64, f64),
    pub coherence_floor: f64,
    pub tikhonov_floor: f64,
    pub ref_floor_db: f64,
    pub min_band_energy: f64,
    pub law_budget: f64,
    pub law_misses: u32,
    pub max_pulses: u64,
    pub max_peak_rise_db: f64,
    pub max_bin_rise_db: f64,
    pub first_drive_ratio: f64,
    pub divergence_pulses: u32,
    pub drive_vpk_ceiling: f64,
    pub drive_vpk_abort: f64,
    pub peak_accel_g: Option<f64>,
    pub monitors: Vec<WireMonitor>,
    pub feasibility: Option<WireFeasibility>,
    pub train_count: u32,
    pub train_spacing_s: f64,
    pub gunfire: Option<GunfireSpec>,
    pub capture: CaptureRules,
    pub metadata: Vec<MetaField>,
    /// The configuration as accepted, kept with the run.
    pub raw: Value,
}

/// The grid a run's records are on: requested until it arms, then the station's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub fs_hz: f64,
    pub record_len: usize,
    pub actual: bool,
}

impl Grid {
    pub fn to_json(&self) -> Value {
        json!({
            "fs_hz": num(round(self.fs_hz, 4)),
            "record_len": self.record_len,
            "actual": self.actual,
        })
    }
}

/// The composite's worst deviation in the standard's quiet window against the
/// standard's amplitude tolerance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuietWindow {
    pub worst_frac: f64,
    pub limit_frac: f64,
}

impl QuietWindow {
    pub fn inside(&self) -> bool {
        self.worst_frac <= self.limit_frac
    }

    pub fn to_json(&self) -> Value {
        json!({
            "worst_frac": num(round(self.worst_frac, 4)),
            "limit_frac": num(round(self.limit_frac, 4)),
            "inside": self.inside(),
        })
    }
}

/// The derived values of a valid configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub grid: Grid,
    pub reference: ReferenceFacts,
    pub quiet_window: QuietWindow,
    /// The least time between pulse starts, seconds.
    pub spacing_s: f64,
    /// One law call's time budget, seconds.
    pub call_budget_s: f64,
    /// The feasibility verdict and its words.
    pub feasibility: (&'static str, String),
}

impl Derived {
    pub fn to_json(&self) -> Value {
        json!({
            "grid": self.grid.to_json(),
            "reference": self.reference.to_json(),
            "quiet_window": self.quiet_window.to_json(),
            "spacing_s": num(round(self.spacing_s, 4)),
            "call_budget_s": num(round(self.call_budget_s, 4)),
            "feasibility": {"verdict": self.feasibility.0, "message": self.feasibility.1},
        })
    }
}

pub fn round(x: f64, digits: i32) -> f64 {
    let p = 10f64.powi(digits);
    (x * p).round() / p
}

// ---------------------------------------------------------------------------------
// Parsing: types and structure. Content problems are items, not parse errors.
// ---------------------------------------------------------------------------------

pub(crate) struct Obj<'a> {
    pub(crate) m: &'a Map<String, Value>,
    pub(crate) path: String,
}

impl<'a> Obj<'a> {
    pub(crate) fn of(v: &'a Value, path: &str) -> Result<Obj<'a>, MethodError> {
        v.as_object()
            .map(|m| Obj {
                m,
                path: path.to_owned(),
            })
            .ok_or_else(|| MethodError::bad_params(path, format!("\"{path}\" must be an object")))
    }

    /// `None` when `k` is absent or null.
    pub(crate) fn opt_obj(&self, k: &str) -> Result<Option<Obj<'a>>, MethodError> {
        match self.m.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => Obj::of(v, &self.at(k)).map(Some),
        }
    }

    pub(crate) fn at(&self, k: &str) -> String {
        format!("{}.{k}", self.path)
    }

    pub(crate) fn require(&self, k: &str) -> Result<&'a Value, MethodError> {
        match self.m.get(k) {
            None | Some(Value::Null) => Err(MethodError::bad_params(
                &self.at(k),
                format!("the configuration has no \"{}\"", self.at(k)),
            )),
            Some(v) => Ok(v),
        }
    }

    pub(crate) fn obj(&self, k: &str) -> Result<Obj<'a>, MethodError> {
        Obj::of(self.require(k)?, &self.at(k))
    }

    pub(crate) fn opt_f64(&self, k: &str) -> Result<Option<f64>, MethodError> {
        match self.m.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_f64()
                .filter(|x| x.is_finite())
                .map(Some)
                .ok_or_else(|| {
                    MethodError::bad_params(
                        &self.at(k),
                        format!("\"{}\" must be a number", self.at(k)),
                    )
                }),
        }
    }

    pub(crate) fn f64(&self, k: &str) -> Result<f64, MethodError> {
        self.opt_f64(k)?.ok_or_else(|| {
            MethodError::bad_params(&self.at(k), format!("\"{}\" must be a number", self.at(k)))
        })
    }

    pub(crate) fn f64_or(&self, k: &str, default: f64) -> Result<f64, MethodError> {
        Ok(self.opt_f64(k)?.unwrap_or(default))
    }

    /// A whole number from 0. Fractions and negatives are `bad_params`; a size out
    /// of range is an item.
    pub(crate) fn opt_count(&self, k: &str) -> Result<Option<u64>, MethodError> {
        match self.m.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .or_else(|| {
                    v.as_f64()
                        .filter(|x| x.fract() == 0.0 && *x >= 0.0 && *x < 9e15)
                        .map(|x| x as u64)
                })
                .map(Some)
                .ok_or_else(|| {
                    MethodError::bad_params(
                        &self.at(k),
                        format!("\"{}\" must be a whole number from 0", self.at(k)),
                    )
                }),
        }
    }

    pub(crate) fn opt_bool(&self, k: &str) -> Result<Option<bool>, MethodError> {
        match self.m.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(MethodError::bad_params(
                &self.at(k),
                format!("\"{}\" must be true or false", self.at(k)),
            )),
        }
    }

    pub(crate) fn opt_str(&self, k: &str) -> Result<Option<&'a str>, MethodError> {
        match self.m.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(MethodError::bad_params(
                &self.at(k),
                format!("\"{}\" must be a string", self.at(k)),
            )),
        }
    }

    pub(crate) fn array(&self, k: &str) -> Result<Option<&'a Vec<Value>>, MethodError> {
        match self.m.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(a)) => Ok(Some(a)),
            Some(_) => Err(MethodError::bad_params(
                &self.at(k),
                format!("\"{}\" must be a list", self.at(k)),
            )),
        }
    }

    /// `[low, high]` in Hz.
    pub(crate) fn band(&self, k: &str) -> Result<Option<(f64, f64)>, MethodError> {
        let Some(a) = self.array(k)? else {
            return Ok(None);
        };
        let bad = || {
            MethodError::bad_params(
                &self.at(k),
                format!("\"{}\" is [low, high] in Hz", self.at(k)),
            )
        };
        if a.len() != 2 {
            return Err(bad());
        }
        let lo = a[0].as_f64().filter(|x| x.is_finite()).ok_or_else(bad)?;
        let hi = a[1].as_f64().filter(|x| x.is_finite()).ok_or_else(bad)?;
        Ok(Some((lo, hi)))
    }

    /// One of `names`, or `default` when absent.
    pub(crate) fn choice<T: Copy>(
        &self,
        k: &str,
        names: &[(&str, T)],
        default: Option<T>,
    ) -> Result<T, MethodError> {
        let given = self.opt_str(k)?;
        let list = || {
            names
                .iter()
                .map(|(n, _)| format!("\"{n}\""))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match given {
            None => default.ok_or_else(|| {
                MethodError::bad_params(
                    &self.at(k),
                    format!("\"{}\" is one of {}", self.at(k), list()),
                )
            }),
            Some(s) => names
                .iter()
                .find(|(n, _)| *n == s)
                .map(|(_, v)| *v)
                .ok_or_else(|| {
                    MethodError::bad_params(
                        &self.at(k),
                        format!("\"{}\" is one of {}, not \"{s}\"", self.at(k), list()),
                    )
                }),
        }
    }
}

/// The pulse shapes on the wire.
pub const SHAPES: [&str; 6] = [
    "half_sine",
    "haversine",
    "terminal_peak_sawtooth",
    "initial_peak_sawtooth",
    "trapezoid",
    "rectangle",
];

pub fn shape_name(s: ClassicalShape) -> &'static str {
    match s {
        ClassicalShape::HalfSine => "half_sine",
        ClassicalShape::Haversine => "haversine",
        ClassicalShape::TerminalPeakSawtooth => "terminal_peak_sawtooth",
        ClassicalShape::InitialPeakSawtooth => "initial_peak_sawtooth",
        ClassicalShape::Trapezoid { .. } => "trapezoid",
        ClassicalShape::Rectangle => "rectangle",
    }
}

pub fn standard_name(s: ToleranceStandard) -> &'static str {
    match s {
        ToleranceStandard::Mil810_516 => "mil_std_810_516",
        ToleranceStandard::Iec60068_2_27 => "iec_60068_2_27",
    }
}

const STANDARDS: [(&str, ToleranceStandard); 2] = [
    ("mil_std_810_516", ToleranceStandard::Mil810_516),
    ("iec_60068_2_27", ToleranceStandard::Iec60068_2_27),
];

/// The pulse, its standard and its rate as sent: what `shock.preview` and a
/// configuration share.
#[derive(Debug, Clone, PartialEq)]
pub struct WirePulse {
    pub shape: ClassicalShape,
    pub amplitude_g: f64,
    pub duration_ms: f64,
    pub polarity: Polarity,
    pub compensation: CompensationConfig,
}

impl WirePulse {
    /// The controller's pulse spec (always positive: the polarity is the core's).
    pub fn spec(&self) -> ClassicalPulseSpec {
        ClassicalPulseSpec {
            shape: self.shape,
            amplitude_g: self.amplitude_g,
            duration_ms: self.duration_ms,
            compensation: self.compensation,
        }
    }
}

/// The wire's compensation when none is given: rectangular shoulders, each 4
/// times the duration, symmetric. Not the controller's library default (half-sine
/// 1.5 times), which leaves the standard's quiet window.
pub fn default_compensation() -> CompensationConfig {
    CompensationConfig {
        shape: CompensationShape::Rectangular,
        pre_frac: 4.0,
        post_frac: 4.0,
        symmetric: true,
    }
}

/// Parse a `pulse` object at `path`.
pub fn parse_pulse(v: &Value, path: &str) -> Result<WirePulse, MethodError> {
    let p = Obj::of(v, path)?;
    let shape_s = p.opt_str("shape")?.ok_or_else(|| {
        MethodError::bad_params(
            &p.at("shape"),
            format!("\"{}\" is one of {}", p.at("shape"), SHAPES.join(", ")),
        )
    })?;
    let rise = p.opt_f64("rise_frac")?;
    let fall = p.opt_f64("fall_frac")?;
    let shape = match shape_s {
        "half_sine" => ClassicalShape::HalfSine,
        "haversine" => ClassicalShape::Haversine,
        "terminal_peak_sawtooth" => ClassicalShape::TerminalPeakSawtooth,
        "initial_peak_sawtooth" => ClassicalShape::InitialPeakSawtooth,
        "trapezoid" => match (rise, fall) {
            (Some(rise_frac), Some(fall_frac)) => ClassicalShape::Trapezoid {
                rise_frac,
                fall_frac,
            },
            _ => {
                return Err(MethodError::bad_params(
                    &p.at("rise_frac"),
                    "a trapezoid gives its rise and fall, each a fraction of the duration",
                ))
            }
        },
        "rectangle" => ClassicalShape::Rectangle,
        other => {
            return Err(MethodError::bad_params(
                &p.at("shape"),
                format!(
                    "\"{}\" is one of {}, not \"{other}\"",
                    p.at("shape"),
                    SHAPES.join(", ")
                ),
            ))
        }
    };
    if shape_s != "trapezoid" && (rise.is_some() || fall.is_some()) {
        return Err(MethodError::bad_params(
            &p.at("rise_frac"),
            "rise and fall fractions apply to a trapezoid only",
        ));
    }
    let amplitude_g = p.f64("amplitude_g")?;
    let duration_ms = p.f64("duration_ms")?;
    let polarity = p.choice(
        "polarity",
        &[
            ("positive", Polarity::Positive),
            ("negative", Polarity::Negative),
        ],
        Some(Polarity::Positive),
    )?;
    let compensation = match p.opt_obj("compensation")? {
        None => default_compensation(),
        Some(c) => {
            let d = default_compensation();
            CompensationConfig {
                shape: c.choice(
                    "shape",
                    &[
                        ("half_sine", CompensationShape::HalfSine),
                        ("rectangular", CompensationShape::Rectangular),
                        ("rounded_rectangular", CompensationShape::RoundedRectangular),
                    ],
                    Some(d.shape),
                )?,
                pre_frac: c.f64_or("pre_frac", d.pre_frac)?,
                post_frac: c.f64_or("post_frac", d.post_frac)?,
                symmetric: c.opt_bool("symmetric")?.unwrap_or(d.symmetric),
            }
        }
    };
    Ok(WirePulse {
        shape,
        amplitude_g,
        duration_ms,
        polarity,
        compensation,
    })
}

/// Parse a `standard` object at `path` (absent: the defaults).
pub fn parse_standard(
    v: Option<&Value>,
    path: &str,
) -> Result<(ToleranceStandard, ToleranceAbort, Option<f64>), MethodError> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Ok((
            ToleranceStandard::Mil810_516,
            ToleranceAbort::FullLevel,
            None,
        ));
    };
    let s = Obj::of(v, path)?;
    let standard = s.choice("name", &STANDARDS, Some(ToleranceStandard::Mil810_516))?;
    let abort = s.choice(
        "tolerance_abort",
        &[
            ("full_level", ToleranceAbort::FullLevel),
            ("every_level", ToleranceAbort::EveryLevel),
        ],
        Some(ToleranceAbort::FullLevel),
    )?;
    let overshoot = s.opt_f64("overshoot_abort")?;
    Ok((standard, abort, overshoot))
}

fn monitor_kind_name(k: DriveMonitorKind) -> &'static str {
    match k {
        DriveMonitorKind::Predictive => "predictive",
        DriveMonitorKind::BetweenPulse => "between_pulse",
    }
}

/// Parse `params.config`.
pub fn parse(config: &Value) -> Result<WireShock, MethodError> {
    let c = Obj::of(config, "config")?;
    let title = c.opt_str("title")?.unwrap_or("").to_owned();
    let pulse = parse_pulse(c.require("pulse")?, "config.pulse")?;
    let (standard, tolerance_abort, overshoot_abort) =
        parse_standard(c.m.get("standard"), "config.standard")?;

    let mut fs_hz = DEFAULT_FS_HZ;
    let (mut srs_lo, mut srs_hi, mut srs_n, mut srs_tol) = (
        hertzforge_shock_host::service::config::DEFAULT_SRS_F_LO_HZ,
        hertzforge_shock_host::service::config::DEFAULT_SRS_F_HI_HZ,
        hertzforge_shock_host::service::config::DEFAULT_SRS_BANDS_PER_OCTAVE,
        hertzforge_shock_host::service::config::DEFAULT_SRS_BAND_TOL_DB,
    );
    let mut pvss = false;
    if let Some(a) = c.opt_obj("acquisition")? {
        fs_hz = a.f64_or("fs_hz", fs_hz)?;
        if let Some(s) = a.opt_obj("srs")? {
            srs_lo = s.f64_or("f_lo_hz", srs_lo)?;
            srs_hi = s.f64_or("f_hi_hz", srs_hi)?;
            srs_n = s.f64_or("bands_per_octave", srs_n)?;
            srs_tol = s.f64_or("band_tol_db", srs_tol)?;
        }
        pvss = a.opt_bool("pvss")?.unwrap_or(false);
    }

    let sch = c.obj("schedule")?;
    let mode = sch.choice(
        "mode",
        &[
            ("auto", ShockRunMode::Auto),
            ("manual", ShockRunMode::Manual),
        ],
        None,
    )?;
    let levels_v = sch.array("levels_db")?.ok_or_else(|| {
        MethodError::bad_params(
            "config.schedule.levels_db",
            "the schedule has no \"levels_db\" list",
        )
    })?;
    let mut levels_db = Vec::with_capacity(levels_v.len());
    for (i, l) in levels_v.iter().enumerate() {
        levels_db.push(l.as_f64().filter(|x| x.is_finite()).ok_or_else(|| {
            MethodError::bad_params(
                &format!("config.schedule.levels_db[{i}]"),
                "a level is a number of dB",
            )
        })?);
    }
    let pulses_v = sch.array("pulses")?.ok_or_else(|| {
        MethodError::bad_params(
            "config.schedule.pulses",
            "the schedule has no \"pulses\" list: one count for every level, or one per level",
        )
    })?;
    let mut pulses = Vec::with_capacity(pulses_v.len());
    for (i, p) in pulses_v.iter().enumerate() {
        let n = p
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| {
                MethodError::bad_params(
                    &format!("config.schedule.pulses[{i}]"),
                    "a pulse count is a whole number from 0",
                )
            })?;
        pulses.push(n);
    }
    let start_db = sch.f64_or("start_db", levels_db.first().copied().unwrap_or(0.0))?;
    let advance = sch.choice(
        "advance",
        &[("count", Advance::Count), ("on_pass", Advance::OnPass)],
        Some(Advance::Count),
    )?;
    let max_pulses_per_level = sch
        .opt_count("max_pulses_per_level")?
        .map_or(DEFAULT_MAX_PULSES_PER_LEVEL, |n| {
            u32::try_from(n).unwrap_or(u32::MAX)
        });

    let rate = RateModel::conservative_default();
    let (mut cooling_s, mut settling_s, mut compute_s) = (
        rate.cooling_floor_s,
        rate.settling_floor_s,
        rate.compute_floor_s,
    );
    if let Some(s) = c.opt_obj("spacing")? {
        cooling_s = s.f64_or("cooling_s", cooling_s)?;
        settling_s = s.f64_or("settling_s", settling_s)?;
        compute_s = s.f64_or("compute_s", compute_s)?;
    }

    let limits = c.obj("limits")?;
    let drive_vpk_ceiling = limits.f64("drive_vpk_ceiling")?;
    let drive_vpk_abort = limits.f64("drive_vpk_abort")?;
    let peak_accel_g = limits.opt_f64("peak_accel_g")?;
    let mut monitors = Vec::new();
    if let Some(ms) = limits.array("monitors")? {
        for (i, m) in ms.iter().enumerate() {
            let o = Obj::of(m, &format!("config.limits.monitors[{i}]"))?;
            let channel = o
                .opt_count("channel")?
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| {
                    MethodError::bad_params(&o.at("channel"), "a monitor names a channel index")
                })?;
            let unit = o
                .opt_str("unit")?
                .ok_or_else(|| {
                    MethodError::bad_params(&o.at("unit"), "a monitor names its channel's unit")
                })?
                .to_owned();
            monitors.push(WireMonitor {
                channel,
                unit,
                peak_ceiling: o.f64("peak_ceiling")?,
                kind: o.choice(
                    "kind",
                    &[
                        ("predictive", DriveMonitorKind::Predictive),
                        ("between_pulse", DriveMonitorKind::BetweenPulse),
                    ],
                    None,
                )?,
            });
        }
    }

    // The control band's default follows the rate, as the controller's own does.
    let base = ClassicalControl::new(fs_hz, drive_vpk_ceiling, drive_vpk_abort);
    let default_band = (base.band_lo_hz, base.band_hi_hz);
    let mut sysid = (
        base.sysid.drive_rms,
        base.sysid.duration_s,
        base.sysid.nfft as u64,
        (base.sysid.f_lo_hz, base.sysid.f_hi_hz),
        base.sysid.min_mean_coherence,
        base.review_hold,
    );
    if let Some(i) = c.opt_obj("identification")? {
        sysid.0 = i.f64_or("drive_vrms", sysid.0)?;
        sysid.1 = i.f64_or("duration_s", sysid.1)?;
        sysid.2 = i.opt_count("nfft")?.unwrap_or(sysid.2);
        sysid.3 = i.band("band_hz")?.unwrap_or(sysid.3);
        sysid.4 = i.f64_or("min_mean_coherence", sysid.4)?;
        sysid.5 = i.opt_bool("review")?.unwrap_or(sysid.5);
    }

    let mut law = LawSetup {
        id: hertzforge_shock_laws::DEFAULT_CLASSICAL_LAW_ID.to_owned(),
        params: Map::new(),
    };
    let mut band_hz = default_band;
    let mut coherence_floor = base.inverse.coherence_floor;
    let mut tikhonov_floor = base.inverse.tikhonov_beta_floor;
    let mut ref_floor_db = base.ref_floor_db;
    let mut min_band_energy = base.min_band_energy;
    let mut law_budget = base.guards.budget_fraction;
    let mut law_misses = u64::from(base.guards.max_misses);
    let mut max_pulses = base.guards.max_pulses;
    let mut max_peak_rise_db = base.max_peak_rise_db;
    let mut max_bin_rise_db = base.max_bin_rise_db;
    let mut first_drive_ratio = base.first_drive_ratio;
    let mut divergence_pulses = u64::from(base.divergence_pulses);
    if let Some(o) = c.opt_obj("control")? {
        if let Some(l) = o.opt_obj("law")? {
            let id = l
                .opt_str("id")?
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    MethodError::bad_params(&l.at("id"), "a control law is named by its id")
                })?
                .to_owned();
            let params = match l.m.get("params") {
                None | Some(Value::Null) => Map::new(),
                Some(Value::Object(p)) => p.clone(),
                Some(_) => {
                    return Err(MethodError::bad_params(
                        &l.at("params"),
                        "\"params\" is an object of parameter values by key",
                    ))
                }
            };
            law = LawSetup { id, params };
        }
        band_hz = o.band("band_hz")?.unwrap_or(band_hz);
        coherence_floor = o.f64_or("coherence_floor", coherence_floor)?;
        tikhonov_floor = o.f64_or("tikhonov_floor", tikhonov_floor)?;
        ref_floor_db = o.f64_or("ref_floor_db", ref_floor_db)?;
        min_band_energy = o.f64_or("min_band_energy", min_band_energy)?;
        law_budget = o.f64_or("law_budget", law_budget)?;
        law_misses = o.opt_count("law_misses")?.unwrap_or(law_misses);
        max_pulses = o.opt_count("max_pulses")?.unwrap_or(max_pulses);
        max_peak_rise_db = o.f64_or("max_peak_rise_db", max_peak_rise_db)?;
        max_bin_rise_db = o.f64_or("max_bin_rise_db", max_bin_rise_db)?;
        first_drive_ratio = o.f64_or("first_drive_ratio", first_drive_ratio)?;
        divergence_pulses = o
            .opt_count("divergence_pulses")?
            .unwrap_or(divergence_pulses);
    }

    let feasibility = match c.opt_obj("feasibility")? {
        None => None,
        Some(o) => {
            let shaker: ShakerRecord = serde_json::from_value(o.require("shaker")?.clone())
                .map_err(|e| {
                    MethodError::named(
                        ErrorCode::BadRequest,
                        "shaker_record_invalid",
                        format!(
                            "The feasibility shaker record does not parse ({e}), so the pulse \
                             cannot be checked against it."
                        ),
                    )
                })?;
            let mm = o.obj("moving_mass_lb")?;
            let mass = |k: &str| -> Result<f64, MethodError> {
                let x = mm.f64(k)?;
                if x < 0.0 {
                    return Err(MethodError::bad_params(
                        &mm.at(k),
                        "a moving mass is 0 lb or more",
                    ));
                }
                Ok(x)
            };
            let tol = FeasibilityTolerances::default();
            Some(WireFeasibility {
                shaker,
                moving_mass: MovingMass {
                    armature_lb: mass("armature")?,
                    slip_table_lb: mass("slip_table")?,
                    drive_adapter_lb: mass("drive_adapter")?,
                    fixturing_lb: mass("fixturing")?,
                    test_article_lb: mass("test_article")?,
                },
                residual_velocity_ips: o
                    .f64_or("residual_velocity_ips", tol.residual_velocity_ips)?,
                residual_disp_in: o.f64_or("residual_disp_in", tol.residual_disp_in)?,
            })
        }
    };

    let (mut train_count, mut train_spacing_s) = (1u64, 0.0);
    if let Some(t) = c.opt_obj("train")? {
        train_count = t.opt_count("count")?.ok_or_else(|| {
            MethodError::bad_params(&t.at("count"), "a train gives its pulse count")
        })?;
        train_spacing_s = t.f64("spacing_s")?;
    }
    let gunfire = match c.opt_obj("gunfire")? {
        None => None,
        Some(g) => Some(GunfireSpec {
            rounds_per_min: g.f64("rounds_per_min")?,
            rounds: g
                .opt_count("rounds")?
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| {
                    MethodError::bad_params(&g.at("rounds"), "a burst gives its rounds")
                })?,
        }),
    };
    let capture = match c.opt_obj("capture")? {
        None => CaptureRules::default(),
        Some(o) => {
            let d = CaptureRules::default();
            CaptureRules {
                auto_start: o.opt_bool("auto_start")?.unwrap_or(d.auto_start),
                run_name: o.opt_str("run_name")?.map_or(d.run_name, str::to_owned),
            }
        }
    };
    let metadata = match c.array("metadata")? {
        None => MetaField::defaults(),
        Some(list) => {
            let mut out = Vec::with_capacity(list.len());
            for (i, m) in list.iter().enumerate() {
                let o = Obj::of(m, &format!("config.metadata[{i}]"))?;
                let text = |k: &str| -> Result<String, MethodError> {
                    o.opt_str(k)?.map(str::to_owned).ok_or_else(|| {
                        MethodError::bad_params(&o.at(k), format!("\"{}\" is text", o.at(k)))
                    })
                };
                out.push(MetaField::new(text("key")?, text("label")?, text("value")?));
            }
            out
        }
    };

    let clip32 = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
    Ok(WireShock {
        title,
        shape: pulse.shape,
        amplitude_g: pulse.amplitude_g,
        duration_ms: pulse.duration_ms,
        polarity: pulse.polarity,
        compensation: pulse.compensation,
        standard,
        tolerance_abort,
        overshoot_abort,
        fs_hz,
        srs_f_lo_hz: srs_lo,
        srs_f_hi_hz: srs_hi,
        srs_bands_per_octave: srs_n,
        srs_band_tol_db: srs_tol,
        pvss,
        mode,
        levels_db,
        pulses,
        start_db,
        advance,
        max_pulses_per_level,
        cooling_s,
        settling_s,
        compute_s,
        sysid_drive_vrms: sysid.0,
        sysid_duration_s: sysid.1,
        sysid_nfft: sysid.2,
        sysid_band_hz: sysid.3,
        sysid_min_mean_coherence: sysid.4,
        review: sysid.5,
        law,
        band_hz,
        coherence_floor,
        tikhonov_floor,
        ref_floor_db,
        min_band_energy,
        law_budget,
        law_misses: clip32(law_misses),
        max_pulses,
        max_peak_rise_db,
        max_bin_rise_db,
        first_drive_ratio,
        divergence_pulses: clip32(divergence_pulses),
        drive_vpk_ceiling,
        drive_vpk_abort,
        peak_accel_g,
        monitors,
        feasibility,
        train_count: clip32(train_count),
        train_spacing_s,
        gunfire,
        capture,
        metadata,
        raw: config.clone(),
    })
}

// ---------------------------------------------------------------------------------
// Checking: every problem is a named item.
// ---------------------------------------------------------------------------------

fn out_of_range(r: &mut Report, field: &str, value: f64, ok: bool, text: &str) {
    if !ok {
        r.errors.push(Item::field(
            "out_of_range",
            field,
            format!("{} is {}; {text}", field_words(field), num(value)),
        ));
    }
}

/// A configuration field in words, for a message a person reads.
pub fn field_words(field: &str) -> String {
    let last = field.rsplit('.').next().unwrap_or(field);
    let last = last.split('[').next().unwrap_or(last);
    let words = match last {
        "amplitude_g" => "The pulse's amplitude",
        "duration_ms" => "The pulse's duration",
        "rise_frac" => "The trapezoid's rise",
        "fall_frac" => "The trapezoid's fall",
        "pre_frac" => "The pre-pulse's length",
        "post_frac" => "The post-pulse's length",
        "overshoot_abort" => "The overshoot abort",
        "fs_hz" => "The sample rate",
        "f_lo_hz" => "The SRS grid's low edge",
        "f_hi_hz" => "The SRS grid's high edge",
        "bands_per_octave" => "The SRS bands per octave",
        "band_tol_db" => "The SRS band tolerance",
        "levels_db" => "A level",
        "pulses" => "A pulse count",
        "start_db" => "The start level",
        "max_pulses_per_level" => "The pulses allowed at one level",
        "cooling_s" => "The cooling time",
        "settling_s" => "The settling time",
        "compute_s" => "The compute time",
        "drive_vrms" => "The identification drive",
        "duration_s" => "The identification length",
        "nfft" => "The identification block size",
        "band_hz" => "The band",
        "min_mean_coherence" => "The identification's coherence hold",
        "coherence_floor" => "The coherence floor",
        "tikhonov_floor" => "The inverse's floor",
        "ref_floor_db" => "The reference floor",
        "min_band_energy" => "The reference energy the trusted bins must cover",
        "law_budget" => "The law's time budget",
        "law_misses" => "The unusable calls allowed in a row",
        "max_pulses" => "The run's pulse cap",
        "max_peak_rise_db" => "The cap on the drive peak's rise",
        "max_bin_rise_db" => "The cap on a bin's rise",
        "first_drive_ratio" => "The first drive's bound",
        "divergence_pulses" => "The divergence count",
        "drive_vpk_ceiling" => "The drive ceiling",
        "drive_vpk_abort" => "The drive abort",
        "peak_accel_g" => "The peak acceleration ceiling",
        "peak_ceiling" => "A monitor's ceiling",
        "residual_velocity_ips" => "The residual velocity tolerance",
        "residual_disp_in" => "The residual displacement tolerance",
        "count" => "The train's pulse count",
        "spacing_s" => "The train's spacing",
        "rounds_per_min" => "The gunfire rate",
        "rounds" => "The gunfire rounds",
        other => return format!("\"{other}\""),
    };
    words.to_owned()
}

impl WireShock {
    /// The pulse as `shock.preview` takes it.
    pub fn pulse(&self) -> WirePulse {
        WirePulse {
            shape: self.shape,
            amplitude_g: self.amplitude_g,
            duration_ms: self.duration_ms,
            polarity: self.polarity,
            compensation: self.compensation,
        }
    }

    /// The least time between pulse starts: the largest of the floors.
    pub fn spacing_s(&self) -> f64 {
        self.rate_model().effective_min_spacing(DrivePath::HostDac)
    }

    pub fn rate_model(&self) -> RateModel {
        RateModel {
            cooling_floor_s: self.cooling_s,
            settling_floor_s: self.settling_s,
            compute_floor_s: self.compute_s,
            ..RateModel::conservative_default()
        }
    }

    /// The controller's schedule, with how the AUTO ladder advances and the
    /// pulses allowed at one level without a pass.
    pub fn schedule(&self) -> ShockLevelSchedule {
        ShockLevelSchedule {
            mode: self.mode,
            start_db: self.start_db,
            table_db: self.levels_db.clone(),
            pulses_per_level: self.pulses.clone(),
            advance: match self.advance {
                Advance::Count => LadderAdvance::Count,
                Advance::OnPass => LadderAdvance::OnPass,
            },
            max_pulses_per_level: self.max_pulses_per_level,
        }
    }

    /// The closed-loop settings.
    pub fn control(&self) -> ClassicalControl {
        let mut c = ClassicalControl::new(self.fs_hz, self.drive_vpk_ceiling, self.drive_vpk_abort);
        c.sysid.drive_rms = self.sysid_drive_vrms;
        c.sysid.duration_s = self.sysid_duration_s;
        c.sysid.nfft = usize::try_from(self.sysid_nfft).unwrap_or(usize::MAX);
        c.sysid.f_lo_hz = self.sysid_band_hz.0;
        c.sysid.f_hi_hz = self.sysid_band_hz.1;
        c.sysid.min_mean_coherence = self.sysid_min_mean_coherence;
        c.band_lo_hz = self.band_hz.0;
        c.band_hi_hz = self.band_hz.1;
        c.inverse.coherence_floor = self.coherence_floor;
        c.inverse.tikhonov_beta_floor = self.tikhonov_floor;
        c.ref_floor_db = self.ref_floor_db;
        c.min_band_energy = self.min_band_energy;
        c.review_hold = self.review;
        c.guards.budget_fraction = self.law_budget;
        c.guards.max_misses = self.law_misses;
        c.guards.max_pulses = self.max_pulses;
        c.max_peak_rise_db = self.max_peak_rise_db;
        c.max_bin_rise_db = self.max_bin_rise_db;
        c.first_drive_ratio = self.first_drive_ratio;
        c.drive_vpk_ceiling = self.drive_vpk_ceiling;
        c.drive_vpk_abort = self.drive_vpk_abort;
        c.overshoot_abort = self.overshoot_abort;
        c.tolerance_abort = self.tolerance_abort;
        c.divergence_pulses = self.divergence_pulses;
        c.polarity = self.polarity;
        c
    }

    /// The controller's whole test configuration, `control` set.
    pub fn build(&self) -> ClassicalTestConfig {
        let mut cfg = ClassicalTestConfig::new(self.pulse().spec());
        cfg.fs_hz = self.fs_hz;
        cfg.standard = self.standard;
        cfg.schedule = self.schedule();
        cfg.train = TrainSpec::delayed(self.train_count, self.train_spacing_s);
        cfg.gunfire = self.gunfire;
        cfg.rate_model = self.rate_model();
        cfg.drive_limit = DriveLimit {
            peak_accel_g: self.peak_accel_g,
            monitors: self
                .monitors
                .iter()
                .map(|m| DriveMonitor {
                    channel: format!("ch{}", m.channel),
                    eu: m.unit.clone(),
                    peak_ceiling: m.peak_ceiling,
                    kind: m.kind,
                })
                .collect(),
        };
        if let Some(f) = &self.feasibility {
            cfg.shaker = Some(f.shaker.clone());
            cfg.moving_mass = f.moving_mass;
            cfg.residual_velocity_tol_ips = f.residual_velocity_ips;
            cfg.residual_disp_tol_in = f.residual_disp_in;
        }
        cfg.srs_f_lo_hz = self.srs_f_lo_hz;
        cfg.srs_f_hi_hz = self.srs_f_hi_hz;
        cfg.srs_bands_per_octave = self.srs_bands_per_octave;
        cfg.srs_band_tol_db = self.srs_band_tol_db;
        cfg.compute_pvss = self.pvss;
        // Every failed pulse aborts in the open loop; the closed loop's own rule
        // (`control.tolerance_abort`) says when a failure aborts instead.
        cfg.abort_on_tolerance_failure = self.tolerance_abort == ToleranceAbort::EveryLevel;
        cfg.capture = self.capture.clone();
        cfg.metadata = self.metadata.clone();
        cfg.control = Some(self.control());
        cfg
    }

    /// Pulses the schedule fires at level `i` before it moves on (AUTO).
    pub fn pulses_for(&self, i: usize) -> u32 {
        self.schedule().pulses_for(i)
    }

    /// The law as a catalog choice: its id and the values its manifest declares.
    pub fn law_choice(&self, facts: &LawFacts) -> hertzforge_random_laws::LawChoice {
        hertzforge_random_laws::LawChoice::new(
            self.law.id.clone(),
            crate::mimo::adapter::params_of(&facts.params, &self.law.params),
        )
    }

    /// Check every field: every problem a named item, and the derived values when
    /// there are no errors.
    pub fn check(&self, ctx: &CheckContext<'_>) -> (Report, Option<Derived>) {
        let mut r = Report::default();
        let pos = |x: f64| x.is_finite() && x > 0.0;

        // The pulse.
        out_of_range(
            &mut r,
            "pulse.amplitude_g",
            self.amplitude_g,
            pos(self.amplitude_g) && self.amplitude_g <= 10_000.0,
            "it is above 0 and at most 10000 g",
        );
        out_of_range(
            &mut r,
            "pulse.duration_ms",
            self.duration_ms,
            pos(self.duration_ms) && self.duration_ms <= 1000.0,
            "it is above 0 and at most 1000 ms",
        );
        if let ClassicalShape::Trapezoid {
            rise_frac,
            fall_frac,
        } = self.shape
        {
            let ok = |x: f64| x.is_finite() && (0.0..=1.0).contains(&x);
            out_of_range(
                &mut r,
                "pulse.rise_frac",
                rise_frac,
                ok(rise_frac),
                "it is a fraction of the duration from 0 to 1",
            );
            out_of_range(
                &mut r,
                "pulse.fall_frac",
                fall_frac,
                ok(fall_frac) && rise_frac + fall_frac <= 1.0,
                "it is a fraction of the duration, and rise and fall together are at most 1",
            );
        }
        let frac_ok = |x: f64| pos(x) && x <= 20.0;
        out_of_range(
            &mut r,
            "pulse.compensation.pre_frac",
            self.compensation.pre_frac,
            frac_ok(self.compensation.pre_frac),
            "it is above 0 and at most 20 times the duration",
        );
        if !self.compensation.symmetric {
            out_of_range(
                &mut r,
                "pulse.compensation.post_frac",
                self.compensation.post_frac,
                frac_ok(self.compensation.post_frac),
                "it is above 0 and at most 20 times the duration",
            );
        }
        if let Some(o) = self.overshoot_abort {
            out_of_range(
                &mut r,
                "standard.overshoot_abort",
                o,
                pos(o) && o <= 2.0,
                "it is a fraction over the commanded peak, above 0 and at most 2",
            );
        }

        // Acquisition.
        let nyquist = self.fs_hz / 2.0;
        out_of_range(
            &mut r,
            "acquisition.fs_hz",
            self.fs_hz,
            self.fs_hz.is_finite() && (FS_RANGE.0..=FS_RANGE.1).contains(&self.fs_hz),
            &format!(
                "the station runs from {} to {} Hz",
                num(FS_RANGE.0),
                num(FS_RANGE.1)
            ),
        );
        out_of_range(
            &mut r,
            "acquisition.srs.f_lo_hz",
            self.srs_f_lo_hz,
            pos(self.srs_f_lo_hz),
            "it is above 0 Hz",
        );
        if !(self.srs_f_hi_hz.is_finite() && self.srs_f_hi_hz > self.srs_f_lo_hz) {
            out_of_range(
                &mut r,
                "acquisition.srs.f_hi_hz",
                self.srs_f_hi_hz,
                false,
                "it is above the SRS grid's low edge",
            );
        } else if self.srs_f_hi_hz >= nyquist {
            r.errors.push(Item::field(
                "band_above_grid",
                "acquisition.srs.f_hi_hz",
                format!(
                    "The SRS grid reaches {} Hz, at or above half the sample rate ({} Hz).",
                    num(self.srs_f_hi_hz),
                    num(nyquist)
                ),
            ));
        }
        out_of_range(
            &mut r,
            "acquisition.srs.bands_per_octave",
            self.srs_bands_per_octave,
            pos(self.srs_bands_per_octave) && self.srs_bands_per_octave <= 24.0,
            "it is above 0 and at most 24",
        );
        out_of_range(
            &mut r,
            "acquisition.srs.band_tol_db",
            self.srs_band_tol_db,
            pos(self.srs_band_tol_db) && self.srs_band_tol_db <= 20.0,
            "it is above 0 and at most 20 dB",
        );

        // The pulse on its record can be built once these are sound.
        let pulse_sound = r.errors.is_empty();

        // The schedule.
        if self.levels_db.is_empty() {
            r.errors.push(Item::field(
                "levels_empty",
                "schedule.levels_db",
                "The schedule has no levels; give at least the full level.",
            ));
        }
        for (i, l) in self.levels_db.iter().enumerate() {
            out_of_range(
                &mut r,
                &format!("schedule.levels_db[{i}]"),
                *l,
                (LEVEL_DB_RANGE.0..=LEVEL_DB_RANGE.1).contains(l),
                &format!(
                    "a level is from {} to {} dB",
                    num(LEVEL_DB_RANGE.0),
                    num(LEVEL_DB_RANGE.1)
                ),
            );
        }
        for (i, w) in self.levels_db.windows(2).enumerate() {
            if w[1] < w[0] {
                r.errors.push(Item::field(
                    "levels_not_ascending",
                    format!("schedule.levels_db[{}]", i + 1),
                    format!(
                        "The levels go down ({} dB then {} dB); list them from the lowest to \
                         the full level.",
                        num(w[0]),
                        num(w[1])
                    ),
                ));
            }
        }
        if !(self.pulses.len() == 1 || self.pulses.len() == self.levels_db.len()) {
            r.errors.push(Item::field(
                "pulses_length_mismatch",
                "schedule.pulses",
                format!(
                    "The schedule has {} pulse counts for {} levels; give one count for every \
                     level, or one per level.",
                    self.pulses.len(),
                    self.levels_db.len()
                ),
            ));
        }
        for (i, p) in self.pulses.iter().enumerate() {
            let min = if self.mode == ShockRunMode::Auto {
                1
            } else {
                0
            };
            out_of_range(
                &mut r,
                &format!("schedule.pulses[{i}]"),
                f64::from(*p),
                (min..=MAX_PULSES_PER_RUNG).contains(p),
                &format!("a level fires from {min} to {MAX_PULSES_PER_RUNG} pulses"),
            );
        }
        if let (Some(lo), Some(hi)) = (self.levels_db.first(), self.levels_db.last()) {
            out_of_range(
                &mut r,
                "schedule.start_db",
                self.start_db,
                self.start_db >= *lo && self.start_db <= *hi,
                &format!(
                    "it is between the first and last levels ({} to {} dB)",
                    num(*lo),
                    num(*hi)
                ),
            );
        }
        out_of_range(
            &mut r,
            "schedule.max_pulses_per_level",
            f64::from(self.max_pulses_per_level),
            (1..=100).contains(&self.max_pulses_per_level),
            "it is from 1 to 100",
        );

        // Spacing.
        let floor_ok = |x: f64| x.is_finite() && (0.0..=MAX_SPACING_S).contains(&x);
        for (field, v) in [
            ("spacing.cooling_s", self.cooling_s),
            ("spacing.settling_s", self.settling_s),
            ("spacing.compute_s", self.compute_s),
        ] {
            out_of_range(
                &mut r,
                field,
                v,
                floor_ok(v),
                &format!("it is from 0 to {} s", num(MAX_SPACING_S)),
            );
        }
        let spacing_s = self.spacing_s();
        if floor_ok(self.cooling_s)
            && floor_ok(self.settling_s)
            && floor_ok(self.compute_s)
            && spacing_s <= 0.0
        {
            r.errors.push(Item::field(
                "out_of_range",
                "spacing.cooling_s",
                "The time between pulses is 0 s; at least one of the floors is above 0.",
            ));
        }

        // Identification.
        out_of_range(
            &mut r,
            "identification.drive_vrms",
            self.sysid_drive_vrms,
            pos(self.sysid_drive_vrms) && self.sysid_drive_vrms < self.drive_vpk_ceiling,
            "it is above 0 V rms and below the drive ceiling",
        );
        out_of_range(
            &mut r,
            "identification.duration_s",
            self.sysid_duration_s,
            (SYSID_DURATION_RANGE.0..=SYSID_DURATION_RANGE.1).contains(&self.sysid_duration_s),
            &format!(
                "it is from {} to {} s",
                num(SYSID_DURATION_RANGE.0),
                num(SYSID_DURATION_RANGE.1)
            ),
        );
        let nfft_ok = self.sysid_nfft.is_power_of_two()
            && (NFFT_RANGE.0..=NFFT_RANGE.1).contains(&self.sysid_nfft);
        out_of_range(
            &mut r,
            "identification.nfft",
            self.sysid_nfft as f64,
            nfft_ok,
            &format!(
                "it is a power of two from {} to {}",
                NFFT_RANGE.0, NFFT_RANGE.1
            ),
        );
        if nfft_ok && self.fs_hz.is_finite() && self.sysid_duration_s.is_finite() {
            let samples = self.sysid_duration_s * self.fs_hz;
            if samples < 2.0 * self.sysid_nfft as f64 {
                r.errors.push(Item::field(
                    "out_of_range",
                    "identification.duration_s",
                    format!(
                        "The identification is {} samples long, under two blocks of {}; make it \
                         longer or the blocks shorter.",
                        num(samples.floor()),
                        self.sysid_nfft
                    ),
                ));
            }
        }
        self.check_band(
            &mut r,
            "identification.band_hz",
            self.sysid_band_hz,
            nyquist,
        );
        out_of_range(
            &mut r,
            "identification.min_mean_coherence",
            self.sysid_min_mean_coherence,
            self.sysid_min_mean_coherence > 0.0 && self.sysid_min_mean_coherence <= 1.0,
            "it is above 0 and at most 1",
        );

        // The control law and the core's settings.
        self.check_law(&mut r, ctx);
        self.check_band(&mut r, "control.band_hz", self.band_hz, nyquist);
        let unit = |x: f64| x.is_finite() && (0.0..=1.0).contains(&x);
        let open_unit = |x: f64| x.is_finite() && x > 0.0 && x < 1.0;
        out_of_range(
            &mut r,
            "control.coherence_floor",
            self.coherence_floor,
            unit(self.coherence_floor),
            "it is from 0 to 1",
        );
        out_of_range(
            &mut r,
            "control.tikhonov_floor",
            self.tikhonov_floor,
            open_unit(self.tikhonov_floor),
            "it is a fraction of the in-band peak gain, above 0 and below 1",
        );
        out_of_range(
            &mut r,
            "control.ref_floor_db",
            self.ref_floor_db,
            pos(self.ref_floor_db) && self.ref_floor_db <= 200.0,
            "it is above 0 and at most 200 dB",
        );
        out_of_range(
            &mut r,
            "control.min_band_energy",
            self.min_band_energy,
            open_unit(self.min_band_energy),
            "it is a fraction above 0 and below 1",
        );
        out_of_range(
            &mut r,
            "control.law_budget",
            self.law_budget,
            pos(self.law_budget) && self.law_budget <= 1.0,
            "it is a fraction of the time between pulses, above 0 and at most 1",
        );
        out_of_range(
            &mut r,
            "control.law_misses",
            f64::from(self.law_misses),
            (1..=100).contains(&self.law_misses),
            "it is from 1 to 100",
        );
        out_of_range(
            &mut r,
            "control.max_pulses",
            self.max_pulses as f64,
            (1..=100_000).contains(&self.max_pulses),
            "it is from 1 to 100000",
        );
        let rise_ok = |x: f64| pos(x) && x <= 40.0;
        out_of_range(
            &mut r,
            "control.max_peak_rise_db",
            self.max_peak_rise_db,
            rise_ok(self.max_peak_rise_db),
            "it is above 0 and at most 40 dB",
        );
        out_of_range(
            &mut r,
            "control.max_bin_rise_db",
            self.max_bin_rise_db,
            rise_ok(self.max_bin_rise_db),
            "it is above 0 and at most 40 dB",
        );
        out_of_range(
            &mut r,
            "control.first_drive_ratio",
            self.first_drive_ratio,
            self.first_drive_ratio.is_finite() && (1.0..=10.0).contains(&self.first_drive_ratio),
            "it is from 1 to 10 times the reference's peak",
        );
        out_of_range(
            &mut r,
            "control.divergence_pulses",
            f64::from(self.divergence_pulses),
            (1..=100).contains(&self.divergence_pulses),
            "it is from 1 to 100",
        );

        // Limits.
        out_of_range(
            &mut r,
            "limits.drive_vpk_ceiling",
            self.drive_vpk_ceiling,
            pos(self.drive_vpk_ceiling) && self.drive_vpk_ceiling <= MAX_DRIVE_V,
            &format!("it is above 0 and at most {} V", num(MAX_DRIVE_V)),
        );
        out_of_range(
            &mut r,
            "limits.drive_vpk_abort",
            self.drive_vpk_abort,
            self.drive_vpk_abort.is_finite()
                && self.drive_vpk_abort > self.drive_vpk_ceiling
                && self.drive_vpk_abort <= MAX_DRIVE_V,
            &format!(
                "it is above the drive ceiling of {} V and at most {} V",
                num(self.drive_vpk_ceiling),
                num(MAX_DRIVE_V)
            ),
        );
        if let Some(a) = self.peak_accel_g {
            out_of_range(&mut r, "limits.peak_accel_g", a, pos(a), "it is above 0 g");
        }
        let inputs: Vec<u32> = ctx
            .table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .map(|c| c.index)
            .collect();
        for (i, m) in self.monitors.iter().enumerate() {
            if !inputs.contains(&m.channel) {
                r.errors.push(Item::field(
                    "limit_unknown_channel",
                    format!("limits.monitors[{i}].channel"),
                    format!(
                        "Channel {} is not an enabled input of the channel table; monitor an \
                         enabled input, or remove the monitor.",
                        m.channel
                    ),
                ));
            }
            out_of_range(
                &mut r,
                &format!("limits.monitors[{i}].peak_ceiling"),
                m.peak_ceiling,
                pos(m.peak_ceiling),
                &format!("it is above 0 {}", m.unit),
            );
        }

        // The table.
        let enabled = |role: ChannelRole| {
            ctx.table
                .channels
                .iter()
                .any(|c| c.enabled && c.role == role)
        };
        if !enabled(ChannelRole::Control) {
            r.errors.push(Item::channel(
                "no_control_channel",
                None,
                "The channel table has no enabled control row; the pulse is controlled on the \
                 first one.",
            ));
        }
        if !enabled(ChannelRole::Drive) {
            r.errors.push(Item::channel(
                "no_drive_channel",
                None,
                "The channel table has no enabled drive row; the pulse is played on the first \
                 one.",
            ));
        }

        // Feasibility.
        if let Some(f) = &self.feasibility {
            out_of_range(
                &mut r,
                "feasibility.residual_velocity_ips",
                f.residual_velocity_ips,
                pos(f.residual_velocity_ips),
                "it is above 0 in/s",
            );
            out_of_range(
                &mut r,
                "feasibility.residual_disp_in",
                f.residual_disp_in,
                pos(f.residual_disp_in),
                "it is above 0 in",
            );
        }

        // Trains and gunfire.
        out_of_range(
            &mut r,
            "train.count",
            f64::from(self.train_count),
            (1..=MAX_PULSES_PER_RUNG).contains(&self.train_count),
            &format!("a train is from 1 to {MAX_PULSES_PER_RUNG} pulses"),
        );
        if self.train_count > 1 {
            if !(self.train_spacing_s.is_finite()
                && (0.0..=MAX_SPACING_S).contains(&self.train_spacing_s))
            {
                out_of_range(
                    &mut r,
                    "train.spacing_s",
                    self.train_spacing_s,
                    false,
                    &format!("it is from 0 to {} s", num(MAX_SPACING_S)),
                );
            } else if self.train_spacing_s < spacing_s {
                r.errors.push(spacing_below_floor(
                    "train.spacing_s",
                    self.train_spacing_s,
                    spacing_s,
                ));
            }
        }
        if let Some(g) = self.gunfire {
            out_of_range(
                &mut r,
                "gunfire.rounds_per_min",
                g.rounds_per_min,
                pos(g.rounds_per_min) && g.rounds_per_min <= 6000.0,
                "it is above 0 and at most 6000 rounds a minute",
            );
            out_of_range(
                &mut r,
                "gunfire.rounds",
                f64::from(g.rounds),
                (1..=10_000).contains(&g.rounds),
                "a burst is from 1 to 10000 rounds",
            );
            if pos(g.rounds_per_min) && 60.0 / g.rounds_per_min < spacing_s {
                r.errors.push(spacing_below_floor(
                    "gunfire.rounds_per_min",
                    60.0 / g.rounds_per_min,
                    spacing_s,
                ));
            }
        }

        // The pulse itself: its record, its quiet window, its kinematics. Only
        // once the fields it is built from are sound.
        if !pulse_sound {
            return (r, None);
        }
        let figures = preview::reference(&self.pulse(), self.standard, self.fs_hz);
        let record_s = figures.record_len as f64 / self.fs_hz;
        if record_s >= ctx.timeout_s {
            r.errors.push(Item::field(
                "record_too_long",
                "pulse.duration_ms",
                format!(
                    "One pulse is {} s of record, and the controller timeout is {} s: a safe \
                     stop could not wait for the pulse in flight. Shorten the pulse or its \
                     compensation.",
                    num(round(record_s, 3)),
                    num(round(ctx.timeout_s, 1))
                ),
            ));
        }
        if !figures.quiet_window.inside() {
            r.warnings.push(Item::field(
                "quiet_window_exceeded",
                "pulse.compensation",
                format!(
                    "The compensation reaches {}% of the peak in the standard's quiet window, \
                     whose tolerance is {}%: the reference itself would fail the amplitude \
                     check there. Longer or rectangular shoulders sit lower.",
                    num(round(100.0 * figures.quiet_window.worst_frac, 1)),
                    num(round(100.0 * figures.quiet_window.limit_frac, 1))
                ),
            ));
        }
        if !r.errors.is_empty() {
            return (r, None);
        }
        let cfg = self.build();
        if let Err(e) = cfg.validate() {
            r.errors
                .push(Item::field("out_of_range", "pulse", e.to_string()));
            return (r, None);
        }
        let feasibility = feasibility_verdict(self, figures.composite_g());
        let derived = Derived {
            grid: Grid {
                fs_hz: self.fs_hz,
                record_len: figures.record_len,
                actual: false,
            },
            reference: figures.facts,
            quiet_window: figures.quiet_window,
            spacing_s,
            call_budget_s: self.law_budget * spacing_s,
            feasibility,
        };
        (r, Some(derived))
    }

    fn check_band(&self, r: &mut Report, field: &str, (lo, hi): (f64, f64), nyquist: f64) {
        if !(lo.is_finite() && lo > 0.0 && hi.is_finite() && hi > lo) {
            r.errors.push(Item::field(
                "out_of_range",
                field,
                format!(
                    "{} is [{}, {}] Hz; it is [low, high] with the low edge above 0 Hz.",
                    field_words(field),
                    num(lo),
                    num(hi)
                ),
            ));
        } else if hi >= nyquist {
            r.errors.push(Item::field(
                "band_above_grid",
                field,
                format!(
                    "{} reaches {} Hz, at or above half the sample rate ({} Hz).",
                    field_words(field),
                    num(hi),
                    num(nyquist)
                ),
            ));
        }
    }

    fn check_law(&self, r: &mut Report, ctx: &CheckContext<'_>) {
        let Some(facts) = (ctx.law)(&self.law.id) else {
            r.errors.push(Item::field(
                "law_unknown",
                "control.law.id",
                format!(
                    "No control law \"{}\" is in the catalog; choose one laws.list names for \
                     classical shock.",
                    self.law.id
                ),
            ));
            return;
        };
        if let Err(is_for) = facts.world {
            r.errors.push(Item::field(
                "law_wrong_world",
                "control.law.id",
                format!(
                    "\"{}\" is a control law for {is_for}, not for classical shock.",
                    facts.name
                ),
            ));
            return;
        }
        for (key, value) in &self.law.params {
            let Some(spec) = facts.params.iter().find(|p| &p.key == key) else {
                r.errors.push(Item::field(
                    "law_param_unknown",
                    format!("control.law.params.{key}"),
                    format!("\"{}\" has no parameter \"{key}\".", facts.name),
                ));
                continue;
            };
            if let Some(why) = param_problem(spec, value) {
                r.errors.push(Item::field(
                    "law_param_invalid",
                    format!("control.law.params.{key}"),
                    format!("{} {why}", spec.label),
                ));
            }
        }
        // The core's bin cap is never tighter than the built-in law's own clamp
        // plus 1 dB, so the built-in never meets the core's cap in normal running.
        if facts.builtin && facts.id == hertzforge_shock_law_math::classical::ID {
            let own = self
                .law
                .params
                .get("max_db_per_pulse")
                .and_then(Value::as_f64)
                .unwrap_or(hertzforge_shock_law_math::classical::KNOBS[1].default);
            if self.max_bin_rise_db < own + 1.0 {
                r.errors.push(Item::field(
                    "out_of_range",
                    "control.max_bin_rise_db",
                    format!(
                        "The cap on a bin's rise is {} dB, tighter than the law's own largest \
                         rise per pulse plus 1 dB ({} dB); raise the cap or lower the law's.",
                        num(self.max_bin_rise_db),
                        num(own + 1.0)
                    ),
                ));
            }
        }
    }
}

fn spacing_below_floor(field: &str, given_s: f64, floor_s: f64) -> Item {
    Item::field(
        "spacing_below_floor",
        field,
        format!(
            "{} s between pulses is below the floor of {} s (the largest of the cooling, \
             settling and compute times).",
            num(round(given_s, 4)),
            num(round(floor_s, 4))
        ),
    )
}

/// Why `value` is not a value of `spec`, or `None` when it is.
pub fn param_problem(spec: &ParamSpec, value: &Value) -> Option<String> {
    match &spec.kind {
        ParamKind::Number(s) => match value.as_f64() {
            Some(x) if x.is_finite() && x >= s.min && x <= s.max => None,
            Some(x) => Some(format!(
                "is {}; it runs from {} to {}.",
                num(x),
                num(s.min),
                num(s.max)
            )),
            None => Some("is a number.".into()),
        },
        ParamKind::Integer(s) => match value.as_i64() {
            Some(x) if x >= s.min && x <= s.max => None,
            Some(x) => Some(format!("is {x}; it runs from {} to {}.", s.min, s.max)),
            None => Some("is a whole number.".into()),
        },
        ParamKind::Toggle(_) => (!value.is_boolean()).then(|| "is true or false.".to_owned()),
        ParamKind::Choice(s) => match value.as_str() {
            Some(x) if s.options.iter().any(|o| o == x) => None,
            _ => Some(format!("is one of {}.", s.options.join(", "))),
        },
        // A classical shock law has one control channel; a list or curve is
        // checked by the law's own door when the run resolves it.
        ParamKind::PerChannel(_) | ParamKind::Curve(_) => {
            (!value.is_array()).then(|| "is a list.".to_owned())
        }
    }
}

/// The feasibility verdict at full level, with its words.
fn feasibility_verdict(cfg: &WireShock, full_level_g: &[f64]) -> (&'static str, String) {
    let Some(f) = &cfg.feasibility else {
        return (
            "not_evaluated",
            "No shaker was given, so the pulse was not checked against shaker ratings.".into(),
        );
    };
    let report = pulse_feasibility(
        &f.shaker,
        &f.moving_mass,
        full_level_g,
        cfg.fs_hz,
        FeasibilityTolerances {
            residual_velocity_ips: f.residual_velocity_ips,
            residual_disp_in: f.residual_disp_in,
        },
    );
    verdict(&report)
}

/// A feasibility report in words.
pub fn verdict(r: &FeasibilityReport) -> (&'static str, String) {
    use hertzforge_shaker_db::Verdict;
    let items = |want: Verdict| -> String {
        r.items
            .iter()
            .filter(|i| i.verdict == want)
            .map(|i| i.label.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match r.overall {
        Verdict::NotEvaluated => (
            "not_evaluated",
            "The shaker record lacks the ratings to check this pulse against.".into(),
        ),
        Verdict::Pass => ("pass", "The pulse is within the shaker's ratings.".into()),
        Verdict::Advisory => (
            "advisory",
            format!(
                "The pulse is within the shaker's ratings, with advisories: {}.",
                items(Verdict::Advisory)
            ),
        ),
        Verdict::Exceeded => (
            "exceeded",
            format!(
                "The pulse exceeds the shaker's ratings: {}.",
                items(Verdict::Exceeded)
            ),
        ),
    }
}

/// The refusal of a configuration with errors.
pub fn config_invalid(report: &Report) -> MethodError {
    MethodError::named(
        ErrorCode::BadRequest,
        "config_invalid",
        format!(
            "The configuration has {} error{}.",
            report.errors.len(),
            if report.errors.len() == 1 { "" } else { "s" }
        ),
    )
    .with_detail(report.to_json())
}

/// The monitor kinds on the wire.
pub fn monitor_kind(k: DriveMonitorKind) -> &'static str {
    monitor_kind_name(k)
}
