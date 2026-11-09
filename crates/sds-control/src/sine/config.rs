//! The Sine test configuration on the wire (schema/PROTOCOL.md, "Sine
//! Configuration"): parsing (types and structure; a wrong shape is `bad_params`
//! naming the field), the checks (content; every problem a named item located by
//! `field` or `row`), the derived values, and the controller's own
//! `SineTestConfig` built from it with every field supplied.

use super::profile::{self, check_profile, covers, not_covering, parse_profile, Locate};
use crate::protocol::{num, ErrorCode, MethodError};
use crate::shock::config::{param_problem, LawSetup, Obj};
use crate::table::{Item, Report};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_dsp::sweep::sweep_seconds;
use hertzforge_law_host::ParamSpec;
use hertzforge_shaker_db::{MovingMass, ShakerRecord};
use hertzforge_shock_host::MetaField as WireMeta;
use hertzforge_sine::law::{LawGuards, MAX_STEP_DB_RANGE, MIN_TICK_S_BUILTIN, MIN_TICK_S_LAW_FILE};
use hertzforge_sine::service::config::TrackingLagVerdict;
use hertzforge_sine::service::engine::SineProcessStep;
use hertzforge_sine::{
    BandwidthMode, BidirSweepConfig, CaptureConfig, CaptureRule, ColaConfig, ColaWaveform,
    CompositeMode, CompressionPreset, ControlChannel, ControlSet, DwellFor, DwellPlan,
    FeedforwardConfig, LadderConfig, LimitProfile, MetaField, NotchGovernorConfig, RstdConfig,
    RstdDwellMode, RunMode, RungFor, SineMode, SineProfile, SineTestConfig, StepSine,
    StepSineConfig, StepSpacing, SweepDirection, SweepRate, SweepTarget,
};
use serde_json::{json, Map, Value};

/// The rate a configuration asks for when it gives none, Hz: the simulator's and
/// the law check's.
pub const DEFAULT_FS_HZ: f64 = 16_384.0;
/// Accepted requested rates, Hz.
pub const FS_RANGE: (f64, f64) = (1000.0, 204_800.0);
/// The control tick when none is given, ms.
pub const DEFAULT_TICK_MS: f64 = 10.0;
/// The longest control tick, ms.
pub const MAX_TICK_MS: f64 = 1000.0;
/// Accepted schedule levels, dB.
pub const LEVEL_DB_RANGE: (f64, f64) = (-60.0, 12.0);
/// Accepted identification levels, dB under full level.
pub const IDENT_LEVEL_DB_RANGE: (f64, f64) = (-60.0, 0.0);
/// The largest drive ceiling, abort and COLA amplitude, V pk.
pub const MAX_DRIVE_V: f64 = 10.0;
/// The most control ticks the servo-fault debounce may wait.
pub const MAX_FAULT_DEBOUNCE_TICKS: u64 = 100_000;
/// A band edge must be under this fraction of the rate.
pub const BAND_FRACTION_OF_RATE: f64 = 0.45;
/// The controlled stop's ramp, dB per second of station time (the controller's
/// own, not configurable).
pub const STOP_RAMP_DB_PER_S: f64 = hertzforge_sine::service::RAMP_DB_PER_S;
/// The identification's grid is fine enough for this resonance Q at the band's
/// low edge: bins no wider than the low edge over four times it (the
/// controller's own sizing of its identification, `SineService::run_sysid`).
pub const IDENT_Q_TARGET: f64 = 50.0;

/// The test's kind and its scheduler.
#[derive(Debug, Clone, PartialEq)]
pub enum WireTest {
    Sweep {
        band: (f64, f64),
        rate: SweepRate,
        start_hz: Option<f64>,
        direction: SweepDirection,
        sweeps: Option<u64>,
        duration_s: Option<f64>,
    },
    Dwell {
        freq_hz: f64,
        end: DwellFor,
    },
    Step {
        band: (f64, f64),
        steps_per_octave: f64,
        spacing: StepSpacing,
        dwell: Option<DwellFor>,
    },
    Rstd {
        band: (f64, f64),
        level_db: f64,
        rate: SweepRate,
        tracking_bw_hz: f64,
        settle_k: f64,
        dwell_mode: RstdDwellMode,
        dwell: DwellFor,
    },
}

impl WireTest {
    pub fn kind(&self) -> &'static str {
        match self {
            WireTest::Sweep { .. } => "sweep",
            WireTest::Dwell { .. } => "dwell",
            WireTest::Step { .. } => "step",
            WireTest::Rstd { .. } => "rstd",
        }
    }

    /// The band the run covers; a dwell's is its frequency.
    pub fn band(&self) -> (f64, f64) {
        match self {
            WireTest::Sweep { band, .. }
            | WireTest::Step { band, .. }
            | WireTest::Rstd { band, .. } => *band,
            WireTest::Dwell { freq_hz, .. } => (*freq_hz, *freq_hz),
        }
    }

    pub fn is_sweep(&self) -> bool {
        matches!(self, WireTest::Sweep { .. })
    }

    /// The field the band is given at.
    fn band_field(&self) -> &'static str {
        match self {
            WireTest::Dwell { .. } => "test.freq_hz",
            _ => "test.band_hz",
        }
    }
}

/// A limit channel as sent.
#[derive(Debug, Clone, PartialEq)]
pub struct WireLimitChannel {
    pub channel: u32,
    pub profile: SineProfile,
    pub shared_name: Option<String>,
}

/// Feasibility inputs as sent.
#[derive(Debug, Clone, PartialEq)]
pub struct WireFeasibility {
    pub shaker: ShakerRecord,
    pub moving_mass: MovingMass,
}

/// The capture settings as sent.
#[derive(Debug, Clone, PartialEq)]
pub struct WireCapture {
    pub rule: &'static str,
    pub first_s: Option<f64>,
    pub level_settle_s: f64,
    pub periodic_save_s: Option<f64>,
    pub run_name: String,
}

/// A parsed Sine configuration, every default filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct WireSine {
    pub title: String,
    pub profile: SineProfile,
    pub test: WireTest,
    /// The rate as sent; null asks for [`DEFAULT_FS_HZ`].
    pub fs_hz: Option<f64>,
    pub tick_ms: f64,
    pub mode: RunMode,
    pub levels_db: Vec<f64>,
    pub start_db: Option<f64>,
    pub per_level: RungFor,
    pub full_level: RungFor,
    pub ident_level_db: f64,
    pub gate_coherence: f64,
    pub trust_coherence: f64,
    pub trust_h_floor_frac: f64,
    pub review: bool,
    pub composite: CompositeMode,
    pub weights: Vec<(u32, f64)>,
    pub tracking: BandwidthMode,
    pub law: LawSetup,
    pub law_budget: f64,
    pub law_misses: u64,
    pub max_step_db: f64,
    pub max_rise_db_per_s: f64,
    pub divergence_s: f64,
    pub fault_debounce_ticks: u64,
    pub drive_vpk_ceiling: f64,
    pub drive_vpk_abort: f64,
    pub kinematic_monitor: bool,
    pub limit_channels: Vec<WireLimitChannel>,
    pub notch: NotchGovernorConfig,
    pub cola: ColaConfig,
    pub feasibility: Option<WireFeasibility>,
    pub capture: WireCapture,
    pub metadata: Vec<WireMeta>,
    /// The configuration as sent, for `sine.config` and `sine.reset`.
    pub raw: Value,
}

/// What a configuration's checks need to know about a law named by id.
#[derive(Debug, Clone, PartialEq)]
pub struct LawFacts {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Empty for a built-in law.
    pub sha256: String,
    pub builtin: bool,
    /// `Err` with the test type it is for when it is not a Sine law.
    pub world: Result<(), &'static str>,
    pub params: Vec<ParamSpec>,
    /// The keys of the parameters the operator may change during a run.
    pub live: Vec<String>,
}

/// Everything a configuration's checks are made against.
pub struct CheckContext<'a> {
    /// The channel table the run would arm with.
    pub table: &'a ChannelTable,
    /// Looks a law up by id.
    pub law: &'a dyn Fn(&str) -> Option<LawFacts>,
    /// The longest the service waits for a stop before it aborts, seconds: the
    /// controlled stop must reach zero inside it.
    pub timeout_s: f64,
    /// The station's output count, for COLA.
    pub station_outputs: u32,
}

/// The rate and tick a run would run at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub fs_hz: f64,
    pub tick_frames: usize,
    /// True once the run has armed and both are the station's.
    pub actual: bool,
}

impl Grid {
    /// `tick_ms` in whole frames at `fs_hz` (at least one).
    pub fn of(fs_hz: f64, tick_ms: f64, actual: bool) -> Grid {
        Grid {
            fs_hz,
            tick_frames: tick_frames(fs_hz, tick_ms),
            actual,
        }
    }

    pub fn tick_s(&self) -> f64 {
        self.tick_frames as f64 / self.fs_hz
    }

    pub fn to_json(&self) -> Value {
        json!({
            "fs_hz": num(self.fs_hz),
            "tick_frames": self.tick_frames,
            "tick_s": num(round(self.tick_s(), 7)),
            "actual": self.actual,
        })
    }
}

/// A tick of `tick_ms` in whole frames at `fs_hz`, nearest, at least one.
pub fn tick_frames(fs_hz: f64, tick_ms: f64) -> usize {
    ((tick_ms / 1000.0 * fs_hz).round() as usize).max(1)
}

/// The sweep against the tracking filter.
#[derive(Debug, Clone, PartialEq)]
pub struct Tracking {
    pub verdict: &'static str,
    pub ratio: f64,
    pub at_hz: Option<f64>,
    pub message: Option<String>,
}

impl Tracking {
    pub fn of(v: &TrackingLagVerdict) -> Tracking {
        match v {
            TrackingLagVerdict::Ok { ratio } => Tracking {
                verdict: "ok",
                ratio: *ratio,
                at_hz: None,
                message: None,
            },
            TrackingLagVerdict::Advisory {
                ratio,
                at_hz,
                message,
            } => Tracking {
                verdict: "advisory",
                ratio: *ratio,
                at_hz: Some(*at_hz),
                message: Some(message.clone()),
            },
            TrackingLagVerdict::Refuse {
                ratio,
                at_hz,
                message,
            } => Tracking {
                verdict: "refused",
                ratio: *ratio,
                at_hz: Some(*at_hz),
                message: Some(message.clone()),
            },
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "verdict": self.verdict,
            "ratio": num(profile::sig(self.ratio, 3)),
            "at_hz": self.at_hz.map_or(Value::Null, |f| num(round(f, 1))),
            "message": self.message,
        })
    }
}

/// The run's law, as `derived` and the state name it.
#[derive(Debug, Clone, PartialEq)]
pub struct LawRef {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Empty for a built-in law.
    pub sha256: String,
    pub live: Vec<String>,
}

impl LawRef {
    pub fn of(f: &LawFacts) -> LawRef {
        LawRef {
            id: f.id.clone(),
            name: f.name.clone(),
            version: f.version.clone(),
            sha256: f.sha256.clone(),
            live: f.live.clone(),
        }
    }

    pub fn sha_json(&self) -> Value {
        if self.sha256.is_empty() {
            Value::Null
        } else {
            Value::from(self.sha256.as_str())
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "version": self.version,
            "sha256": self.sha_json(),
            "live": self.live,
        })
    }
}

/// What configure computes from a configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub band_hz: (f64, f64),
    pub grid: Grid,
    /// The profile's crossovers and peaks over the band.
    pub profile: Value,
    pub test_s: Option<f64>,
    pub identification_s: f64,
    pub call_budget_s: f64,
    pub safe_stop_s: f64,
    pub tracking: Tracking,
    pub law: LawRef,
    pub feasibility: (&'static str, String),
}

impl Derived {
    pub fn to_json(&self) -> Value {
        json!({
            "band_hz": [num(self.band_hz.0), num(self.band_hz.1)],
            "grid": self.grid.to_json(),
            "profile": self.profile,
            "test_s": self.test_s.map_or(Value::Null, |s| num(round(s, 1))),
            "identification_s": num(round(self.identification_s, 1)),
            "call_budget_s": num(round(self.call_budget_s, 7)),
            "stop_ramp_db_per_s": num(STOP_RAMP_DB_PER_S),
            "safe_stop_s": num(round(self.safe_stop_s, 3)),
            "tracking": self.tracking.to_json(),
            "law": self.law.to_json(),
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

fn bad(path: &str, message: impl Into<String>) -> MethodError {
    MethodError::bad_params(path, message)
}

fn band_hz(o: &Obj<'_>, k: &str) -> Result<(f64, f64), MethodError> {
    o.band(k)?
        .ok_or_else(|| bad(&o.at(k), format!("\"{}\" is [low, high] in Hz", o.at(k))))
}

fn parse_rate(o: &Obj<'_>, k: &str) -> Result<SweepRate, MethodError> {
    let r = o.obj(k)?;
    match r.opt_str("law")? {
        Some("log") => Ok(SweepRate::LogOctPerMin(r.f64("oct_per_min")?)),
        Some("linear") => Ok(SweepRate::LinearHzPerSec(r.f64("hz_per_s")?)),
        _ => Err(bad(
            &r.at("law"),
            format!(
                "\"{}\" is \"log\" (with \"oct_per_min\") or \"linear\" (with \"hz_per_s\")",
                r.at("law")
            ),
        )),
    }
}

/// `{"seconds"}` or `{"cycles"}`, exactly one.
fn parse_dwell_for(v: &Value, path: &str) -> Result<DwellFor, MethodError> {
    let o = Obj::of(v, path)?;
    match (o.opt_f64("seconds")?, o.opt_count("cycles")?) {
        (Some(s), None) => Ok(DwellFor::Seconds(s)),
        (None, Some(c)) => Ok(DwellFor::Cycles(c)),
        _ => Err(bad(
            path,
            format!("\"{path}\" is {{\"seconds\"}} or {{\"cycles\"}}, one of the two"),
        )),
    }
}

/// `{"sweeps"}` or `{"seconds"}`, exactly one.
fn parse_rung_for(o: &Obj<'_>, k: &str) -> Result<Option<RungFor>, MethodError> {
    let Some(r) = o.opt_obj(k)? else {
        return Ok(None);
    };
    match (r.opt_count("sweeps")?, r.opt_f64("seconds")?) {
        (Some(n), None) => Ok(Some(RungFor::Sweeps(clip32(n)))),
        (None, Some(s)) => Ok(Some(RungFor::Seconds(s))),
        _ => Err(bad(
            &r.path,
            format!(
                "\"{}\" is {{\"sweeps\"}} or {{\"seconds\"}}, one of the two",
                r.path
            ),
        )),
    }
}

fn clip32(n: u64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

const DIRECTIONS: [(&str, SweepDirection); 2] =
    [("up", SweepDirection::Up), ("down", SweepDirection::Down)];
const SPACINGS: [(&str, StepSpacing); 2] =
    [("log", StepSpacing::Log), ("linear", StepSpacing::Linear)];
const DWELL_MODES: [(&str, RstdDwellMode); 2] = [
    ("fixed", RstdDwellMode::Fixed),
    ("phase_tracked", RstdDwellMode::PhaseTracked),
];
const MODES: [(&str, RunMode); 2] = [("auto", RunMode::Auto), ("manual", RunMode::Manual)];
const COMPOSITES: [(&str, CompositeMode); 3] = [
    ("weighted_avg", CompositeMode::WeightedAvg),
    ("min", CompositeMode::Min),
    ("max", CompositeMode::Max),
];
const WAVEFORMS: [(&str, ColaWaveform); 3] = [
    ("sine", ColaWaveform::Sine),
    ("square", ColaWaveform::Square),
    ("dc", ColaWaveform::DcRef),
];
const CAPTURE_RULES: [(&str, &str); 4] = [
    ("off", "off"),
    ("entire_test", "entire_test"),
    ("manual", "manual"),
    ("first_seconds", "first_seconds"),
];

pub fn direction_name(d: SweepDirection) -> &'static str {
    match d {
        SweepDirection::Up => "up",
        SweepDirection::Down => "down",
    }
}

pub fn mode_name(m: RunMode) -> &'static str {
    match m {
        RunMode::Auto => "auto",
        RunMode::Manual => "manual",
    }
}

pub fn parse_test(v: &Value) -> Result<WireTest, MethodError> {
    let t = Obj::of(v, "config.test")?;
    let kind = t.opt_str("kind")?;
    match kind {
        Some("sweep") => {
            let band = band_hz(&t, "band_hz")?;
            let rate = parse_rate(&t, "rate")?;
            let e = t.obj("end")?;
            let sweeps = e.opt_count("sweeps")?;
            let duration_s = e.opt_f64("duration_s")?;
            if sweeps.is_none() && duration_s.is_none() {
                return Err(bad(
                    &e.path,
                    "the sweep's \"end\" gives \"sweeps\", \"duration_s\" or both",
                ));
            }
            Ok(WireTest::Sweep {
                band,
                rate,
                start_hz: t.opt_f64("start_hz")?,
                direction: t.choice("direction", &DIRECTIONS, Some(SweepDirection::Up))?,
                sweeps,
                duration_s,
            })
        }
        Some("dwell") => Ok(WireTest::Dwell {
            freq_hz: t.f64("freq_hz")?,
            end: parse_dwell_for(t.require("end")?, &t.at("end"))?,
        }),
        Some("step") => Ok(WireTest::Step {
            band: band_hz(&t, "band_hz")?,
            steps_per_octave: t.f64("steps_per_octave")?,
            spacing: t.choice("spacing", &SPACINGS, Some(StepSpacing::Log))?,
            dwell: match t.m.get("dwell") {
                None | Some(Value::Null) => None,
                Some(d) => Some(parse_dwell_for(d, &t.at("dwell"))?),
            },
        }),
        Some("rstd") => Ok(WireTest::Rstd {
            band: band_hz(&t, "band_hz")?,
            level_db: t.f64("level_db")?,
            rate: parse_rate(&t, "rate")?,
            tracking_bw_hz: t.f64("tracking_bw_hz")?,
            settle_k: t.f64_or("settle_k", 5.0)?,
            dwell_mode: t.choice("dwell_mode", &DWELL_MODES, None)?,
            dwell: parse_dwell_for(t.require("dwell")?, &t.at("dwell"))?,
        }),
        _ => Err(bad(
            "config.test.kind",
            "\"config.test.kind\" is \"sweep\", \"dwell\", \"step\" or \"rstd\"",
        )),
    }
}

/// Feasibility inputs: Random's shaker record and moving mass.
pub(crate) fn parse_feasibility(o: &Obj<'_>) -> Result<WireFeasibility, MethodError> {
    let shaker_v = o.require("shaker")?;
    let shaker: ShakerRecord = serde_json::from_value(shaker_v.clone()).map_err(|e| {
        MethodError::named(
            ErrorCode::BadRequest,
            "shaker_record_invalid",
            format!(
                "The feasibility shaker record does not parse ({e}), so the profile cannot be \
                 checked against it."
            ),
        )
    })?;
    let mm = o.obj("moving_mass_lb")?;
    let mass = |k: &str| -> Result<f64, MethodError> {
        let x = mm.f64(k)?;
        if x < 0.0 {
            return Err(bad(&mm.at(k), "a moving mass is 0 lb or more"));
        }
        Ok(x)
    };
    Ok(WireFeasibility {
        shaker,
        moving_mass: MovingMass {
            armature_lb: mass("armature")?,
            slip_table_lb: mass("slip_table")?,
            drive_adapter_lb: mass("drive_adapter")?,
            fixturing_lb: mass("fixturing")?,
            test_article_lb: mass("test_article")?,
        },
    })
}

/// A channel index at `k`.
fn channel_index(o: &Obj<'_>, k: &str) -> Result<u32, MethodError> {
    o.opt_count(k)?
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| {
            bad(
                &o.at(k),
                format!("\"{}\" is a channel table index", o.at(k)),
            )
        })
}

/// Parse `params.config`.
pub fn parse(config: &Value) -> Result<WireSine, MethodError> {
    let c = Obj::of(config, "config")?;
    let title = c.opt_str("title")?.unwrap_or("").to_owned();
    let profile = parse_profile(c.require("profile")?, "config.profile")?;
    let test = parse_test(c.require("test")?)?;

    let acq = c.opt_obj("acquisition")?;
    let fs_hz = match &acq {
        Some(a) => a.opt_f64("fs_hz")?,
        None => None,
    };
    let tick_ms = match &acq {
        Some(a) => a.f64_or("tick_ms", DEFAULT_TICK_MS)?,
        None => DEFAULT_TICK_MS,
    };

    let ladder = LadderConfig::default();
    let (mut mode, mut levels_db, mut start_db) = (RunMode::Auto, ladder.table_db.clone(), None);
    let (mut per_level, mut full_level) = (ladder.per_rung, ladder.full_for);
    if let Some(s) = c.opt_obj("schedule")? {
        mode = s.choice("mode", &MODES, Some(RunMode::Auto))?;
        if let Some(list) = s.array("levels_db")? {
            levels_db = list
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| {
                        bad(
                            &format!("config.schedule.levels_db[{i}]"),
                            "a level is a number of dB",
                        )
                    })
                })
                .collect::<Result<_, _>>()?;
        }
        start_db = s.opt_f64("start_db")?;
        if let Some(r) = parse_rung_for(&s, "per_level")? {
            per_level = r;
        }
        if let Some(r) = parse_rung_for(&s, "full_level")? {
            full_level = r;
        }
    }

    let ff = FeedforwardConfig::default();
    let (mut ident_level_db, mut gate_coherence) = (ladder.sysid_level_db, 0.5);
    let (mut trust_coherence, mut trust_h_floor_frac, mut review) =
        (ff.coherence_floor, ff.h_floor_frac, true);
    if let Some(i) = c.opt_obj("identification")? {
        ident_level_db = i.f64_or("level_db", ident_level_db)?;
        gate_coherence = i.f64_or("gate_coherence", gate_coherence)?;
        trust_coherence = i.f64_or("trust_coherence", trust_coherence)?;
        trust_h_floor_frac = i.f64_or("trust_h_floor_frac", trust_h_floor_frac)?;
        review = i.opt_bool("review")?.unwrap_or(review);
    }

    let g = LawGuards::default();
    let mut composite = CompositeMode::WeightedAvg;
    let mut weights = Vec::new();
    let mut tracking = BandwidthMode::Proportional {
        pct: 10.0,
        max_hz: 100.0,
    };
    let mut law = LawSetup {
        id: hertzforge_sine::law::SERVO_BLEND_ID.to_owned(),
        params: Map::new(),
    };
    let (mut law_budget, mut law_misses) = (g.budget_fraction, u64::from(g.max_misses));
    let (mut max_step_db, mut max_rise_db_per_s, mut divergence_s) =
        (g.max_step_db, g.max_rise_db_per_s, g.divergence_s);
    let mut fault_debounce_ticks = 50;
    if let Some(o) = c.opt_obj("control")? {
        composite = o.choice("composite", &COMPOSITES, Some(CompositeMode::WeightedAvg))?;
        if let Some(ws) = o.array("weights")? {
            for (i, w) in ws.iter().enumerate() {
                let wo = Obj::of(w, &format!("config.control.weights[{i}]"))?;
                weights.push((channel_index(&wo, "channel")?, wo.f64("weight")?));
            }
        }
        if let Some(t) = o.opt_obj("tracking")? {
            tracking = match t.opt_str("kind")? {
                Some("fixed") => BandwidthMode::FixedHz(t.f64("hz")?),
                Some("proportional") => BandwidthMode::Proportional {
                    pct: t.f64("pct")?,
                    max_hz: t.f64("max_hz")?,
                },
                _ => {
                    return Err(bad(
                        &t.at("kind"),
                        format!("\"{}\" is \"fixed\" or \"proportional\"", t.at("kind")),
                    ))
                }
            };
        }
        if let Some(l) = o.opt_obj("law")? {
            let id = l
                .opt_str("id")?
                .ok_or_else(|| bad(&l.at("id"), "the law is named by its catalog \"id\""))?;
            let params = match l.opt_obj("params")? {
                None => Map::new(),
                Some(p) => p.m.clone(),
            };
            law = LawSetup {
                id: id.to_owned(),
                params,
            };
        }
        law_budget = o.f64_or("law_budget", law_budget)?;
        law_misses = o.opt_count("law_misses")?.unwrap_or(law_misses);
        max_step_db = o.f64_or("max_step_db", max_step_db)?;
        max_rise_db_per_s = o.f64_or("max_rise_db_per_s", max_rise_db_per_s)?;
        divergence_s = o.f64_or("divergence_s", divergence_s)?;
        fault_debounce_ticks = o
            .opt_count("fault_debounce_ticks")?
            .unwrap_or(fault_debounce_ticks);
    }

    let lim = c.obj("limits")?;
    let drive_vpk_ceiling = lim.f64("drive_vpk_ceiling")?;
    let drive_vpk_abort = lim.f64("drive_vpk_abort")?;
    let kinematic_monitor = lim.opt_bool("kinematic_monitor")?.unwrap_or(true);
    let mut limit_channels = Vec::new();
    if let Some(list) = lim.array("channels")? {
        for (i, v) in list.iter().enumerate() {
            let path = format!("config.limits.channels[{i}]");
            let o = Obj::of(v, &path)?;
            limit_channels.push(WireLimitChannel {
                channel: channel_index(&o, "channel")?,
                profile: parse_profile(o.require("profile")?, &o.at("profile"))?,
                shared_name: o.opt_str("shared_name")?.map(str::to_owned),
            });
        }
    }
    let nd = NotchGovernorConfig::default();
    let notch = match lim.opt_obj("notch")? {
        None => nd,
        Some(n) => NotchGovernorConfig {
            drive_floor: n.f64_or("drive_floor", nd.drive_floor)?,
            smoothing_ticks: n.f64_or("smoothing_ticks", nd.smoothing_ticks)?,
            release_ramp_db_per_s: n.f64_or("release_ramp_db_per_s", nd.release_ramp_db_per_s)?,
        },
    };

    let cola = match c.opt_obj("cola")? {
        None => ColaConfig {
            enabled: false,
            volts_pk: 0.1,
            waveform: ColaWaveform::Sine,
        },
        Some(o) => ColaConfig {
            enabled: o.opt_bool("enabled")?.unwrap_or(false),
            volts_pk: o.f64_or("volts_pk", 0.1)?,
            waveform: o.choice("waveform", &WAVEFORMS, Some(ColaWaveform::Sine))?,
        },
    };

    let feasibility = match c.opt_obj("feasibility")? {
        None => None,
        Some(o) => Some(parse_feasibility(&o)?),
    };

    let cd = CaptureConfig::default();
    let capture = match c.opt_obj("capture")? {
        None => WireCapture {
            rule: "off",
            first_s: None,
            level_settle_s: cd.level_settle_s,
            periodic_save_s: None,
            run_name: "sine".into(),
        },
        Some(o) => WireCapture {
            rule: o.choice("rule", &CAPTURE_RULES, Some("off"))?,
            first_s: o.opt_f64("first_s")?,
            level_settle_s: o.f64_or("level_settle_s", cd.level_settle_s)?,
            periodic_save_s: o.opt_f64("periodic_save_s")?,
            run_name: o.opt_str("run_name")?.unwrap_or("sine").to_owned(),
        },
    };

    let metadata = match c.array("metadata")? {
        None => WireMeta::defaults(),
        Some(list) => {
            let mut out = Vec::with_capacity(list.len());
            for (i, m) in list.iter().enumerate() {
                let o = Obj::of(m, &format!("config.metadata[{i}]"))?;
                let text = |k: &str| -> Result<String, MethodError> {
                    o.opt_str(k)?
                        .map(str::to_owned)
                        .ok_or_else(|| bad(&o.at(k), format!("\"{}\" is text", o.at(k))))
                };
                let key = text("key")?;
                let label = o.opt_str("label")?.unwrap_or(&key).to_owned();
                out.push(WireMeta::new(key, label, text("value")?));
            }
            out
        }
    };

    Ok(WireSine {
        title,
        profile,
        test,
        fs_hz,
        tick_ms,
        mode,
        levels_db,
        start_db,
        per_level,
        full_level,
        ident_level_db,
        gate_coherence,
        trust_coherence,
        trust_h_floor_frac,
        review,
        composite,
        weights,
        tracking,
        law,
        law_budget,
        law_misses,
        max_step_db,
        max_rise_db_per_s,
        divergence_s,
        fault_debounce_ticks,
        drive_vpk_ceiling,
        drive_vpk_abort,
        kinematic_monitor,
        limit_channels,
        notch,
        cola,
        feasibility,
        capture,
        metadata,
        raw: config.clone(),
    })
}

// ---------------------------------------------------------------------------------
// Checking: every problem is a named item.
// ---------------------------------------------------------------------------------

/// A configuration field in words, for a message a person reads.
pub fn field_words(field: &str) -> String {
    let last = field.rsplit('.').next().unwrap_or(field);
    let last = last.split('[').next().unwrap_or(last);
    let words = match last {
        "band_hz" => "The band",
        "oct_per_min" | "hz_per_s" => "The sweep rate",
        "start_hz" => "The start frequency",
        "sweeps" => "The sweep count",
        "duration_s" => "The test's length",
        "freq_hz" => "The frequency",
        "seconds" => "The dwell",
        "cycles" => "The dwell's cycles",
        "steps_per_octave" => "The steps per octave",
        "level_db" => "The level",
        "tracking_bw_hz" => "The search's tracking bandwidth",
        "settle_k" => "The settle factor",
        "fs_hz" => "The sample rate",
        "tick_ms" => "The control tick",
        "levels_db" => "A level",
        "per_level" => "The time at each level",
        "full_level" => "The time at full level",
        "gate_coherence" => "The identification's coherence gate",
        "trust_coherence" => "The coherence the identification is trusted above",
        "trust_h_floor_frac" => "The gain floor the identification is trusted above",
        "weight" => "A control weight",
        "hz" => "The tracking bandwidth",
        "pct" => "The tracking bandwidth's percentage",
        "max_hz" => "The tracking bandwidth's cap",
        "law_budget" => "The law's time budget",
        "law_misses" => "The unusable calls allowed in a row",
        "max_step_db" => "The cap on one tick's change",
        "max_rise_db_per_s" => "The cap on the drive's rise",
        "divergence_s" => "The divergence time",
        "fault_debounce_ticks" => "The servo-fault debounce",
        "drive_vpk_ceiling" => "The drive ceiling",
        "drive_vpk_abort" => "The drive abort",
        "drive_floor" => "The notch's drive floor",
        "smoothing_ticks" => "The notch's smoothing",
        "release_ramp_db_per_s" => "The notch's release rate",
        "volts_pk" => "The reference output's amplitude",
        "first_s" => "The seconds captured at each level",
        "level_settle_s" => "The settle time before a capture",
        "periodic_save_s" => "The periodic save interval",
        "run_name" => "The run folder's name",
        other => return format!("\"{other}\""),
    };
    words.to_owned()
}

fn out_of_range(r: &mut Report, field: &str, value: f64, ok: bool, text: &str) {
    if !ok {
        r.errors.push(Item::field(
            "out_of_range",
            field,
            format!("{} is {}; {text}", field_words(field), num(value)),
        ));
    }
}

fn positive(r: &mut Report, field: &str, x: f64) {
    out_of_range(r, field, x, x.is_finite() && x > 0.0, "it is above 0.");
}

fn dwell_for(r: &mut Report, field: &str, d: DwellFor) {
    match d {
        DwellFor::Seconds(s) => positive(r, &format!("{field}.seconds"), s),
        DwellFor::Cycles(c) => out_of_range(
            r,
            &format!("{field}.cycles"),
            c as f64,
            c >= 1,
            "it is at least 1.",
        ),
    }
}

fn rate_field(rate: &SweepRate) -> (&'static str, f64) {
    match *rate {
        SweepRate::LogOctPerMin(x) => ("test.rate.oct_per_min", x),
        SweepRate::LinearHzPerSec(x) => ("test.rate.hz_per_s", x),
    }
}

impl WireSine {
    /// The rate the run asks the station for.
    pub fn fs_hz(&self) -> f64 {
        self.fs_hz.unwrap_or(DEFAULT_FS_HZ)
    }

    /// The control tick, seconds.
    pub fn tick_s(&self) -> f64 {
        self.tick_ms / 1000.0
    }

    /// The core's guards, as configured.
    pub fn guards(&self) -> LawGuards {
        LawGuards {
            max_step_db: self.max_step_db,
            max_rise_db_per_s: self.max_rise_db_per_s,
            budget_fraction: self.law_budget,
            max_misses: clip32(self.law_misses),
            divergence_s: self.divergence_s,
        }
    }

    /// The table's enabled control rows, in table order.
    pub fn control_channels(table: &ChannelTable) -> Vec<u32> {
        table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Control)
            .map(|c| c.index)
            .collect()
    }

    /// The table's enabled drive rows, in table order.
    pub fn drive_channels(table: &ChannelTable) -> Vec<u32> {
        table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Drive)
            .map(|c| c.index)
            .collect()
    }

    pub fn weight_of(&self, channel: u32) -> f64 {
        self.weights
            .iter()
            .rev()
            .find(|(c, _)| *c == channel)
            .map_or(1.0, |(_, w)| *w)
    }

    /// The scheduled level the run engages at.
    pub fn start_level(&self) -> f64 {
        self.start_db
            .or_else(|| self.levels_db.first().copied())
            .unwrap_or(0.0)
    }

    /// One one-way sweep's length, seconds.
    pub fn sweep_s(&self) -> Option<f64> {
        match &self.test {
            WireTest::Sweep { band, rate, .. } => Some(sweep_seconds(band.0, band.1, *rate).abs()),
            _ => None,
        }
    }

    /// How long a rung lasts in test time, seconds.
    fn rung_s(&self, r: RungFor) -> Option<f64> {
        match r {
            RungFor::Seconds(s) => Some(s),
            RungFor::Sweeps(n) => self.sweep_s().map(|s| s * f64::from(n)),
        }
    }

    /// When the ladder has held full level, seconds from engage (`auto` only).
    pub fn ladder_s(&self) -> Option<f64> {
        if self.mode != RunMode::Auto {
            return None;
        }
        let start = self.start_level();
        let idx = self.levels_db.iter().position(|l| *l == start)?;
        let rungs = self.levels_db.len().checked_sub(idx + 1)?;
        Some(self.rung_s(self.per_level)? * rungs as f64 + self.rung_s(self.full_level)?)
    }

    /// The run's own end in test time, seconds; `None` when it is not known
    /// (a step test with dwells computed per step, a resonance search).
    pub fn end_s(&self) -> Option<f64> {
        match &self.test {
            WireTest::Sweep {
                band,
                rate,
                start_hz,
                direction,
                sweeps,
                duration_s,
            } => {
                let by_sweeps = sweeps.map(|n| {
                    let full = sweep_seconds(band.0, band.1, *rate).abs();
                    let start = start_hz.unwrap_or(match direction {
                        SweepDirection::Up => band.0,
                        SweepDirection::Down => band.1,
                    });
                    let first = match direction {
                        SweepDirection::Up => sweep_seconds(start, band.1, *rate).abs(),
                        SweepDirection::Down => sweep_seconds(band.0, start, *rate).abs(),
                    };
                    if n == 0 {
                        0.0
                    } else {
                        first + full * (n - 1) as f64
                    }
                });
                match (by_sweeps, duration_s) {
                    (Some(a), Some(b)) => Some(a.min(*b)),
                    (Some(a), None) => Some(a),
                    (None, b) => *b,
                }
            }
            WireTest::Dwell { freq_hz, end } => Some(match *end {
                DwellFor::Seconds(s) => s,
                DwellFor::Cycles(c) => c as f64 / freq_hz,
            }),
            WireTest::Step {
                band,
                steps_per_octave,
                spacing,
                dwell: Some(d),
            } => {
                let steps = StepSine::new(StepSineConfig {
                    band: *band,
                    steps_per_octave: *steps_per_octave,
                    spacing: *spacing,
                    dwell: Some(*d),
                })
                .ok()?;
                Some(
                    steps
                        .ladder()
                        .iter()
                        .map(|f| match *d {
                            DwellFor::Seconds(s) => s,
                            DwellFor::Cycles(c) => c as f64 / f,
                        })
                        .sum(),
                )
            }
            _ => None,
        }
    }

    /// The identification burst's length at `fs_hz`, seconds: the controller's own
    /// sizing (bins no wider than the band's low edge over 200, at least 16384
    /// points; eight segments).
    pub fn identification_s(&self, fs_hz: f64) -> f64 {
        let f_lo = self.test.band().0.max(1.0);
        let target_df = (f_lo / IDENT_Q_TARGET / 4.0).max(0.05);
        let mut nfft = 16_384usize;
        while (fs_hz / nfft as f64) > target_df && nfft < 1 << 20 {
            nfft *= 2;
        }
        (nfft * 8) as f64 / fs_hz
    }

    /// The law as a catalog choice: its id and the values its manifest declares.
    pub fn law_choice(&self, facts: &LawFacts) -> hertzforge_random_laws::LawChoice {
        hertzforge_random_laws::LawChoice::new(
            self.law.id.clone(),
            crate::mimo::adapter::params_of(&facts.params, &self.law.params),
        )
    }

    /// The built-in law's number parameter `key`: given, else its declared default.
    fn builtin_number(&self, facts: &LawFacts, key: &str, default: f64) -> f64 {
        if let Some(v) = self.law.params.get(key).and_then(Value::as_f64) {
            return v;
        }
        facts
            .params
            .iter()
            .find(|p| p.key == key)
            .and_then(|p| match &p.kind {
                hertzforge_law_host::ParamKind::Number(s) => Some(s.default),
                _ => None,
            })
            .unwrap_or(default)
    }

    /// The controller's configuration, every field supplied (no `..`: a field added
    /// to `SineTestConfig` stops this compiling until it is given a meaning here).
    /// For the built-in law its blend weight and compression are written into the
    /// configuration; a law file's parameters reach the law through its choice.
    pub fn build(&self, table: &ChannelTable, facts: &LawFacts) -> SineTestConfig {
        let fs = self.fs_hz();
        let builtin = facts.builtin && facts.id == hertzforge_sine::law::SERVO_BLEND_ID;
        let (blend_w, compression) = if builtin {
            let w = self.builtin_number(facts, "blend_w", 0.5);
            // Given, else the default the law's manifest declares: the law is
            // built at it, and the settle window reads it from here.
            let declared = facts.params.iter().find_map(|p| match &p.kind {
                hertzforge_law_host::ParamKind::Choice(s) if p.key == "compression" => {
                    Some(s.default.as_str())
                }
                _ => None,
            });
            let named = self.law.params.get("compression").and_then(Value::as_str);
            let c = match named.or(declared) {
                Some("fast") => CompressionPreset::Fast,
                Some("custom") => CompressionPreset::Custom {
                    ki: self.builtin_number(facts, "ki", 2.0),
                    max_rate_db_per_s: self.builtin_number(facts, "max_rate_db_per_s", 20.0),
                },
                _ => CompressionPreset::Slow,
            };
            (w, c)
        } else {
            (0.5, CompressionPreset::Slow)
        };
        let mode = match &self.test {
            WireTest::Sweep {
                band,
                rate,
                start_hz,
                direction,
                sweeps,
                duration_s,
            } => SineMode::Sweep(BidirSweepConfig {
                band: *band,
                rate: *rate,
                start_hz: start_hz.unwrap_or(match direction {
                    SweepDirection::Up => band.0,
                    SweepDirection::Down => band.1,
                }),
                initial_direction: *direction,
                target: match (sweeps, duration_s) {
                    (Some(n), Some(s)) => SweepTarget::WhicheverFirst {
                        sweeps: clip32(*n),
                        duration_s: *s,
                    },
                    (Some(n), None) => SweepTarget::Sweeps(clip32(*n)),
                    (None, s) => SweepTarget::DurationS(s.unwrap_or(0.0)),
                },
            }),
            WireTest::Dwell { freq_hz, end } => SineMode::Dwell(DwellPlan {
                freq_hz: *freq_hz,
                for_: *end,
            }),
            WireTest::Step {
                band,
                steps_per_octave,
                spacing,
                dwell,
            } => SineMode::StepSine(StepSineConfig {
                band: *band,
                steps_per_octave: *steps_per_octave,
                spacing: *spacing,
                dwell: *dwell,
            }),
            WireTest::Rstd {
                band,
                level_db,
                rate,
                tracking_bw_hz,
                settle_k,
                dwell_mode,
                dwell,
            } => SineMode::Rstd(RstdConfig {
                band: *band,
                level_db: *level_db,
                rate: *rate,
                tracking_bw_hz: *tracking_bw_hz,
                settle_k: *settle_k,
                dwell_mode: *dwell_mode,
                dwell_for: *dwell,
            }),
        };
        let shaker = self.feasibility.as_ref().map(|f| f.shaker.clone());
        let moving_mass = self
            .feasibility
            .as_ref()
            .map_or_else(MovingMass::default, |f| f.moving_mass);
        SineTestConfig {
            profile: self.profile.clone(),
            mode,
            control: ControlSet {
                channels: Self::control_channels(table)
                    .into_iter()
                    .map(|index| ControlChannel {
                        index,
                        weight: self.weight_of(index),
                    })
                    .collect(),
                mode: self.composite,
            },
            limit_profiles: self
                .limit_channels
                .iter()
                .map(|l| LimitProfile {
                    channel_index: l.channel,
                    profile: l.profile.clone(),
                    shared_name: l.shared_name.clone(),
                })
                .collect(),
            blend_w,
            compression,
            tracking_bw: self.tracking,
            feedforward: FeedforwardConfig {
                coherence_floor: self.trust_coherence,
                h_floor_frac: self.trust_h_floor_frac,
            },
            notch: self.notch,
            ladder: LadderConfig {
                sysid_level_db: self.ident_level_db,
                start_db: self.start_level(),
                table_db: self.levels_db.clone(),
                per_rung: self.per_level,
                full_for: self.full_level,
            },
            sysid_level_db: self.ident_level_db,
            coherence_floor: self.gate_coherence,
            drive_vpk_ceiling: self.drive_vpk_ceiling,
            drive_vpk_abort: self.drive_vpk_abort,
            fault_debounce_ticks: u32::try_from(self.fault_debounce_ticks).unwrap_or(u32::MAX),
            kinematic_monitor: self.kinematic_monitor,
            capture: CaptureConfig {
                rule: match self.capture.rule {
                    "entire_test" => CaptureRule::EntireTest,
                    "manual" => CaptureRule::Manual,
                    "first_seconds" => CaptureRule::AutoFirstSeconds {
                        n_s: self.capture.first_s.unwrap_or(0.0),
                    },
                    _ => CaptureRule::Off,
                },
                level_settle_s: self.capture.level_settle_s,
                periodic_save_s: self.capture.periodic_save_s,
            },
            cola: self.cola,
            shaker,
            moving_mass,
            control_tick_frames: tick_frames(fs, self.tick_ms),
            fs_hz: Some(fs),
            // The controller always holds at review; the run thread holds there or
            // proceeds at once (`identification.review`).
            review_hold: true,
            metadata: self
                .metadata
                .iter()
                .map(|m| MetaField {
                    key: m.key.clone(),
                    value: m.value.clone(),
                })
                .collect(),
            law_guards: Some(self.guards()),
            control_tick_s: Some(self.tick_s()),
        }
    }

    /// Check the configuration against `ctx`. The derived values come back when
    /// there are no errors.
    pub fn check(&self, ctx: &CheckContext<'_>) -> (Report, Option<Derived>) {
        let mut r = Report::default();
        let fs = self.fs_hz();
        let band = self.test.band();

        // The profile, and that it covers the band.
        let pr = check_profile(&self.profile, Locate::Control);
        let profile_ok = pr.errors.is_empty();
        r.extend(pr);

        // The rate and the tick.
        if let Some(f) = self.fs_hz {
            out_of_range(
                &mut r,
                "acquisition.fs_hz",
                f,
                (FS_RANGE.0..=FS_RANGE.1).contains(&f),
                &format!(
                    "it runs from {} to {} Hz.",
                    num(FS_RANGE.0),
                    num(FS_RANGE.1)
                ),
            );
        }

        // The test.
        let band_ok = self.check_test(&mut r, fs);
        if band_ok && profile_ok && !covers(&self.profile, band) {
            r.errors
                .push(not_covering(self.test.band_field(), &self.profile, band));
        }

        // The law, and the tick it may run at.
        let facts = (ctx.law)(&self.law.id);
        match &facts {
            None => r.errors.push(Item::field(
                "law_unknown",
                "control.law.id",
                format!(
                    "No control law \"{}\" is in the catalog; laws.list names the ones there are.",
                    self.law.id
                ),
            )),
            Some(f) => {
                if let Err(test) = f.world {
                    r.errors.push(Item::field(
                        "law_wrong_world",
                        "control.law.id",
                        format!(
                            "{} is a law for {test}, not for Sine; choose a Sine law.",
                            f.name
                        ),
                    ));
                }
                for (k, v) in &self.law.params {
                    let field = format!("control.law.params.{k}");
                    match f.params.iter().find(|p| &p.key == k) {
                        None => r.errors.push(Item::field(
                            "law_param_unknown",
                            field,
                            format!("{} declares no parameter \"{k}\".", f.name),
                        )),
                        Some(spec) => {
                            if let Some(why) = param_problem(spec, v) {
                                r.errors.push(Item::field(
                                    "law_param_invalid",
                                    field,
                                    format!("{}'s {} {why}", f.name, spec.label),
                                ));
                            }
                        }
                    }
                }
            }
        }
        let min_tick_ms = if facts.as_ref().is_some_and(|f| !f.builtin) {
            MIN_TICK_S_LAW_FILE * 1000.0
        } else {
            MIN_TICK_S_BUILTIN * 1000.0
        };
        if !(self.tick_ms.is_finite() && (min_tick_ms..=MAX_TICK_MS).contains(&self.tick_ms)) {
            r.errors.push(Item::field(
                "tick_out_of_range",
                "acquisition.tick_ms",
                format!(
                    "The control tick is {} ms; it runs from {} to {} ms for {}.",
                    num(self.tick_ms),
                    num(min_tick_ms),
                    num(MAX_TICK_MS),
                    if min_tick_ms > MIN_TICK_S_BUILTIN * 1000.0 {
                        "a law file"
                    } else {
                        "a built-in law"
                    }
                ),
            ));
        }

        self.check_schedule(&mut r);
        self.check_identification(&mut r);
        self.check_control(&mut r, ctx);
        self.check_limits(&mut r, ctx, band, band_ok);
        self.check_cola(&mut r, ctx);
        self.check_capture(&mut r);

        // The controlled stop inside the service's own stop budget.
        let safe_stop_s = SineProcessStep::safe_stop_seconds(self.drive_vpk_ceiling.max(0.0));
        if self.drive_vpk_ceiling.is_finite() && safe_stop_s > ctx.timeout_s {
            r.errors.push(Item::field(
                "safe_stop_budget",
                "limits.drive_vpk_ceiling",
                format!(
                    "From the drive ceiling the controlled stop takes {} s, longer than the {} \
                     s the service waits for a stop; lower the ceiling.",
                    num(round(safe_stop_s, 3)),
                    num(ctx.timeout_s)
                ),
            ));
        }

        // The sweep against the tracking filter, on the lag the run can expect
        // before the station measures it: one block at the requested rate.
        let tracking = if band_ok && self.tracking_ok() {
            let lag = hertzforge_hal::default_block_frames(fs);
            match &self.test {
                WireTest::Sweep { band, rate, .. } => {
                    Tracking::of(&hertzforge_sine::service::config::assess_tracking_lag(
                        *band,
                        *rate,
                        self.tracking,
                        lag,
                        fs,
                    ))
                }
                _ => Tracking::of(&TrackingLagVerdict::Ok { ratio: 0.0 }),
            }
        } else {
            Tracking::of(&TrackingLagVerdict::Ok { ratio: 0.0 })
        };
        match tracking.verdict {
            "refused" => r.errors.push(Item::field(
                "tracking_lag",
                "control.tracking",
                tracking.message.clone().unwrap_or_default(),
            )),
            "advisory" => r.warnings.push(Item::field(
                "tracking_lag",
                "control.tracking",
                tracking.message.clone().unwrap_or_default(),
            )),
            _ => {}
        }

        if !r.errors.is_empty() {
            return (r, None);
        }
        let Some(facts) = facts else {
            return (r, None);
        };
        let grid = Grid::of(fs, self.tick_ms, false);
        let feasibility = profile::feasibility_verdict(
            &self.profile,
            self.feasibility
                .as_ref()
                .map(|f| (&f.shaker, &f.moving_mass)),
        );
        let derived = Derived {
            band_hz: band,
            grid,
            profile: profile::facts_json(&self.profile, band),
            test_s: if self.mode == RunMode::Auto {
                self.end_s()
            } else {
                None
            },
            identification_s: self.identification_s(fs),
            call_budget_s: self.law_budget * grid.tick_s(),
            safe_stop_s,
            tracking,
            law: LawRef::of(&facts),
            feasibility,
        };
        (r, Some(derived))
    }

    fn tracking_ok(&self) -> bool {
        match self.tracking {
            BandwidthMode::FixedHz(hz) => hz.is_finite() && hz > 0.0,
            BandwidthMode::Proportional { pct, max_hz } => {
                pct.is_finite() && pct > 0.0 && max_hz.is_finite() && max_hz > 0.0
            }
        }
    }

    /// The test's own fields; true when its band is usable.
    fn check_test(&self, r: &mut Report, fs: f64) -> bool {
        let n_before = r.errors.len();
        let band = self.test.band();
        let field = self.test.band_field();
        let edge = BAND_FRACTION_OF_RATE * fs;
        match &self.test {
            WireTest::Dwell { freq_hz, end } => {
                positive(r, "test.freq_hz", *freq_hz);
                dwell_for(r, "test.end", *end);
            }
            _ => out_of_range(
                r,
                field,
                band.0,
                band.0 > 0.0 && band.1 > band.0,
                &format!(
                    "the band runs from above 0 Hz to a higher edge (here {} to {} Hz).",
                    num(band.0),
                    num(band.1)
                ),
            ),
        }
        let band_ok = r.errors.len() == n_before;
        if band_ok && band.1 >= edge {
            r.errors.push(Item::field(
                "band_above_grid",
                field,
                format!(
                    "The band reaches {} Hz, at or above 0.45 times the rate of {} Hz ({} Hz); \
                     raise the rate or lower the band.",
                    num(band.1),
                    num(fs),
                    num(round(edge, 1))
                ),
            ));
        }
        match &self.test {
            WireTest::Sweep {
                band,
                rate,
                start_hz,
                sweeps,
                duration_s,
                ..
            } => {
                let (f, x) = rate_field(rate);
                positive(r, f, x);
                if let Some(s) = start_hz {
                    out_of_range(
                        r,
                        "test.start_hz",
                        *s,
                        *s >= band.0 && *s <= band.1,
                        "it is inside the band.",
                    );
                }
                if let Some(n) = sweeps {
                    out_of_range(
                        r,
                        "test.end.sweeps",
                        *n as f64,
                        *n >= 1,
                        "it is at least 1.",
                    );
                }
                if let Some(d) = duration_s {
                    positive(r, "test.end.duration_s", *d);
                }
            }
            WireTest::Dwell { .. } => {}
            WireTest::Step {
                steps_per_octave,
                dwell,
                ..
            } => {
                positive(r, "test.steps_per_octave", *steps_per_octave);
                if let Some(d) = dwell {
                    dwell_for(r, "test.dwell", *d);
                }
            }
            WireTest::Rstd {
                level_db,
                rate,
                tracking_bw_hz,
                settle_k,
                dwell,
                ..
            } => {
                out_of_range(
                    r,
                    "test.level_db",
                    *level_db,
                    (IDENT_LEVEL_DB_RANGE.0..=IDENT_LEVEL_DB_RANGE.1).contains(level_db),
                    "it runs from -60 to 0 dB.",
                );
                let (f, x) = rate_field(rate);
                positive(r, f, x);
                positive(r, "test.tracking_bw_hz", *tracking_bw_hz);
                positive(r, "test.settle_k", *settle_k);
                dwell_for(r, "test.dwell", *dwell);
            }
        }
        band_ok
    }

    fn check_schedule(&self, r: &mut Report) {
        if self.levels_db.is_empty() {
            r.errors.push(Item::field(
                "levels_empty",
                "schedule.levels_db",
                "The schedule has no levels; give at least one (the last is full level).",
            ));
            return;
        }
        for (i, l) in self.levels_db.iter().enumerate() {
            out_of_range(
                r,
                &format!("schedule.levels_db[{i}]"),
                *l,
                (LEVEL_DB_RANGE.0..=LEVEL_DB_RANGE.1).contains(l),
                &format!(
                    "a level runs from {} to {} dB.",
                    num(LEVEL_DB_RANGE.0),
                    num(LEVEL_DB_RANGE.1)
                ),
            );
        }
        if self.levels_db.windows(2).any(|w| w[1] <= w[0]) {
            r.errors.push(Item::field(
                "levels_not_ascending",
                "schedule.levels_db",
                "The levels run in ascending order, the last full level.",
            ));
        }
        if let Some(s) = self.start_db {
            if !self.levels_db.contains(&s) {
                r.errors.push(Item::field(
                    "start_not_a_level",
                    "schedule.start_db",
                    format!(
                        "The start level of {} dB is not a level of the schedule.",
                        num(s)
                    ),
                ));
            }
        }
        // The time at each level before the last is used only when the run starts below the
        // last level; with one level, or a start at the last, it is never read.
        let start = self.start_level();
        let before_last = self
            .levels_db
            .iter()
            .position(|l| *l == start)
            .is_some_and(|i| i + 1 < self.levels_db.len());
        let rungs = [
            ("per_level", self.per_level),
            ("full_level", self.full_level),
        ];
        for (k, rung) in rungs
            .into_iter()
            .filter(|(k, _)| *k != "per_level" || before_last)
        {
            let field = format!("schedule.{k}");
            match rung {
                RungFor::Sweeps(_) if !self.test.is_sweep() => r.errors.push(Item::field(
                    "out_of_range",
                    format!("{field}.sweeps"),
                    format!(
                        "{} is counted in sweeps only in a sweep test; give seconds.",
                        field_words(&field)
                    ),
                )),
                RungFor::Sweeps(n) => out_of_range(
                    r,
                    &format!("{field}.sweeps"),
                    f64::from(n),
                    n >= 1,
                    "it is at least 1.",
                ),
                RungFor::Seconds(s) => positive(r, &format!("{field}.seconds"), s),
            }
        }
        if let (Some(ladder), Some(end)) = (self.ladder_s(), self.end_s()) {
            if end + 1e-9 < ladder {
                r.errors.push(Item::field(
                    "end_before_full_level",
                    "test.end",
                    format!(
                        "The test ends after {} s, before the schedule has held full level ({} \
                         s); lengthen the test or shorten the schedule.",
                        num(round(end, 1)),
                        num(round(ladder, 1))
                    ),
                ));
            }
        }
    }

    fn check_identification(&self, r: &mut Report) {
        out_of_range(
            r,
            "identification.level_db",
            self.ident_level_db,
            (IDENT_LEVEL_DB_RANGE.0..=IDENT_LEVEL_DB_RANGE.1).contains(&self.ident_level_db),
            "it runs from -60 to 0 dB.",
        );
        for (k, x) in [
            ("identification.gate_coherence", self.gate_coherence),
            ("identification.trust_coherence", self.trust_coherence),
            ("identification.trust_h_floor_frac", self.trust_h_floor_frac),
        ] {
            out_of_range(r, k, x, (0.0..=1.0).contains(&x), "it runs from 0 to 1.");
        }
    }

    fn check_control(&self, r: &mut Report, ctx: &CheckContext<'_>) {
        let controls = Self::control_channels(ctx.table);
        if controls.is_empty() {
            r.errors.push(Item::field(
                "no_control_channel",
                "control",
                "The channel table has no enabled control row; the run controls to the control \
                 rows.",
            ));
        }
        for (i, (ch, w)) in self.weights.iter().enumerate() {
            if !controls.contains(ch) {
                r.errors.push(Item::field(
                    "weight_unknown_channel",
                    format!("control.weights[{i}].channel"),
                    format!("Channel {ch} is not an enabled control row of the channel table."),
                ));
            }
            out_of_range(
                r,
                &format!("control.weights[{i}].weight"),
                *w,
                w.is_finite() && *w > 0.0,
                "a weight is above 0.",
            );
        }
        match self.tracking {
            BandwidthMode::FixedHz(hz) => positive(r, "control.tracking.hz", hz),
            BandwidthMode::Proportional { pct, max_hz } => {
                out_of_range(
                    r,
                    "control.tracking.pct",
                    pct,
                    pct.is_finite() && pct > 0.0 && pct <= 100.0,
                    "it is above 0 and at most 100.",
                );
                positive(r, "control.tracking.max_hz", max_hz);
            }
        }
        out_of_range(
            r,
            "control.law_budget",
            self.law_budget,
            self.law_budget > 0.0 && self.law_budget <= 1.0,
            "it is above 0 and at most 1.",
        );
        out_of_range(
            r,
            "control.law_misses",
            self.law_misses as f64,
            (1..=100).contains(&self.law_misses),
            "it runs from 1 to 100.",
        );
        let (lo, hi) = MAX_STEP_DB_RANGE;
        out_of_range(
            r,
            "control.max_step_db",
            self.max_step_db,
            (lo..=hi).contains(&self.max_step_db),
            &format!("it runs from {} to {} dB.", num(lo), num(hi)),
        );
        out_of_range(
            r,
            "control.max_rise_db_per_s",
            self.max_rise_db_per_s,
            self.max_rise_db_per_s > 0.0 && self.max_rise_db_per_s <= 10_000.0,
            "it is above 0 and at most 10000 dB/s.",
        );
        out_of_range(
            r,
            "control.divergence_s",
            self.divergence_s,
            self.divergence_s > 0.0 && self.divergence_s <= 60.0,
            "it is above 0 and at most 60 s.",
        );
        out_of_range(
            r,
            "control.fault_debounce_ticks",
            self.fault_debounce_ticks as f64,
            self.fault_debounce_ticks <= MAX_FAULT_DEBOUNCE_TICKS,
            &format!("it is at most {MAX_FAULT_DEBOUNCE_TICKS} (0 turns it off)."),
        );
    }

    fn check_limits(
        &self,
        r: &mut Report,
        ctx: &CheckContext<'_>,
        band: (f64, f64),
        band_ok: bool,
    ) {
        let c = self.drive_vpk_ceiling;
        let a = self.drive_vpk_abort;
        let ceiling_ok = c > 0.0 && c <= MAX_DRIVE_V;
        out_of_range(
            r,
            "limits.drive_vpk_ceiling",
            c,
            ceiling_ok,
            &format!("it is above 0 and at most {} V.", num(MAX_DRIVE_V)),
        );
        if ceiling_ok && a < c {
            r.errors.push(Item::field(
                "abort_below_ceiling",
                "limits.drive_vpk_abort",
                format!(
                    "The drive abort of {} V is under the drive ceiling of {} V; the abort is at \
                     least the ceiling.",
                    num(a),
                    num(c)
                ),
            ));
        } else {
            out_of_range(
                r,
                "limits.drive_vpk_abort",
                a,
                a > 0.0 && a <= MAX_DRIVE_V,
                &format!("it is above 0 and at most {} V.", num(MAX_DRIVE_V)),
            );
        }
        if Self::drive_channels(ctx.table).is_empty() {
            r.errors.push(Item::field(
                "no_drive_channel",
                "limits",
                "The channel table has no enabled drive row; the run drives the first one.",
            ));
        }
        let controls = Self::control_channels(ctx.table);
        for (i, l) in self.limit_channels.iter().enumerate() {
            let at = format!("limits.channels[{i}]");
            let row = ctx
                .table
                .channels
                .iter()
                .find(|c| c.index == l.channel && c.enabled);
            match row {
                Some(_) if controls.contains(&l.channel) => r.errors.push(Item::field(
                    "limit_on_control_channel",
                    format!("{at}.channel"),
                    format!(
                        "Channel {} is a control row; a limit channel is another enabled input.",
                        l.channel
                    ),
                )),
                Some(c) if c.role != ChannelRole::Drive => {}
                _ => r.errors.push(Item::field(
                    "limit_unknown_channel",
                    format!("{at}.channel"),
                    format!(
                        "Channel {} is not an enabled input of the channel table; name the row \
                         the bound is for.",
                        l.channel
                    ),
                )),
            }
            let field = format!("{at}.profile");
            let pr = check_profile(&l.profile, Locate::Field(&field));
            let ok = pr.errors.is_empty();
            r.extend(pr);
            if ok && band_ok && !covers(&l.profile, band) {
                r.errors.push(not_covering(&field, &l.profile, band));
            }
        }
        let n = self.notch;
        out_of_range(
            r,
            "limits.notch.drive_floor",
            n.drive_floor,
            n.drive_floor > 0.0 && n.drive_floor < 1.0,
            "it is above 0 and under 1.",
        );
        out_of_range(
            r,
            "limits.notch.smoothing_ticks",
            n.smoothing_ticks,
            n.smoothing_ticks >= 1.0,
            "it is at least 1.",
        );
        positive(
            r,
            "limits.notch.release_ramp_db_per_s",
            n.release_ramp_db_per_s,
        );
    }

    fn check_cola(&self, r: &mut Report, ctx: &CheckContext<'_>) {
        if !self.cola.enabled {
            return;
        }
        out_of_range(
            r,
            "cola.volts_pk",
            self.cola.volts_pk,
            self.cola.volts_pk > 0.0 && self.cola.volts_pk <= MAX_DRIVE_V,
            &format!("it is above 0 and at most {} V.", num(MAX_DRIVE_V)),
        );
        let drives = Self::drive_channels(ctx.table).len();
        if drives < 2 || ctx.station_outputs < 2 {
            r.errors.push(Item::field(
                "cola_needs_two_outputs",
                "cola.enabled",
                format!(
                    "The reference output needs a second enabled drive row and a station with \
                     two outputs; the table has {drives} drive row{} and the station {} output{}.",
                    if drives == 1 { "" } else { "s" },
                    ctx.station_outputs,
                    if ctx.station_outputs == 1 { "" } else { "s" }
                ),
            ));
        }
    }

    fn check_capture(&self, r: &mut Report) {
        let c = &self.capture;
        // The controller records the whole test from engage; it has no verbs for a
        // recording the operator starts and stops, and no first-seconds rule.
        if matches!(c.rule, "manual" | "first_seconds") {
            r.errors.push(Item::field(
                "out_of_range",
                "capture.rule",
                format!(
                    "Recording \"{}\" is not offered: the service records the entire test \
                     (\"entire_test\") or nothing (\"off\").",
                    c.rule
                ),
            ));
        }
        out_of_range(
            r,
            "capture.level_settle_s",
            c.level_settle_s,
            c.level_settle_s >= 0.0,
            "it is 0 or more.",
        );
        if let Some(p) = c.periodic_save_s {
            positive(r, "capture.periodic_save_s", p);
        }
        let plain = !c.run_name.is_empty()
            && !c.run_name.contains(['/', '\\', ':'])
            && c.run_name != "."
            && c.run_name != "..";
        if !plain {
            r.errors.push(Item::field(
                "out_of_range",
                "capture.run_name",
                format!(
                    "The run folder's name is \"{}\"; it is a plain folder name, with no path in \
                     it.",
                    c.run_name
                ),
            ));
        }
    }
}

/// The refusal of a configuration with errors.
pub fn config_invalid(report: &Report) -> MethodError {
    crate::shock::config::config_invalid(report)
}
