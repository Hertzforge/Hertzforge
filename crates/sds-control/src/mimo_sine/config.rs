//! The MIMO Sine configuration on the wire (schema/PROTOCOL.md, "MIMO Sine
//! Configuration"): parsing (types and structure; a wrong shape is `bad_params`
//! naming the field), the checks (content; every problem a named item located
//! by `field`, or by `row` and `channel` for a profile row), the derived values,
//! and the engine's own `MimoSineSetup` built from it with every field supplied.

use super::laws::LawFacts;
use crate::protocol::{num, MethodError};
use crate::shock::config::{param_problem, Obj};
use crate::sine::config::{
    band_hz, channel_index, parse_dwell_for, parse_rate, parse_rung_for, round, Grid, Tracking,
    DIRECTIONS, MODES, SPACINGS, STOP_RAMP_DB_PER_S,
};
use crate::sine::profile::{self, check_profile, covers, parse_profile, Locate};
use crate::table::{Item, Report};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_dsp::sweep::sweep_seconds;
use hertzforge_mimo_sine::core::ShakerRating;
use hertzforge_mimo_sine::{
    shape_name, ControlChannel, DriveChannel, GuardSettings, IdentificationSettings, LawChoice,
    LevelMode, LimitChannel, MimoSineSetup, NotchSettings, PhasePoint, RequestSettings, Schedule,
    SetupRule, StepPlan, TestPlan, TestType, Tracking as MsTracking, MAX_CONTROLS, MAX_DRIVES,
    MAX_LIMITS,
};
use hertzforge_shaker_db::{MovingMass, ShakerRecord};
use hertzforge_sine::service::config::TrackingLagVerdict;
use hertzforge_sine::service::engine::SineProcessStep;
use hertzforge_sine::{
    BidirSweepConfig, DwellFor, DwellPlan, LadderConfig, RunMode, RungFor, SineProfile,
    StepSineConfig, StepSpacing, SweepDirection, SweepRate, SweepTarget,
};
use serde_json::{json, Map, Value};

/// The rate a configuration asks for when it gives none, Hz: the simulator's
/// sine structures are calibrated at it.
pub const DEFAULT_FS_HZ: f64 = 16_384.0;
/// The control tick when none is given, ms.
pub const DEFAULT_TICK_MS: f64 = 10.0;
/// The longest control tick, ms.
pub const MAX_TICK_MS: f64 = 1000.0;
/// The shortest control tick, ms, for a built-in law and for a law file.
pub const MIN_TICK_MS_BUILTIN: f64 = 2.0;
pub const MIN_TICK_MS_LAW_FILE: f64 = 8.0;
/// Accepted requested rates, Hz.
pub const FS_RANGE: (f64, f64) = (1000.0, 204_800.0);
/// Accepted schedule levels, dB.
pub const LEVEL_DB_RANGE: (f64, f64) = (-60.0, 12.0);
/// The largest drive ceiling and abort, V pk.
pub const MAX_DRIVE_V: f64 = 10.0;
/// A level held this long has no clock of its own: the test's end governs.
pub const UNTIL_THE_END_S: f64 = 1e9;
/// The per-level time a dwell or a stepped sine holds each level below full
/// when the schedule gives none, s.
pub const DEFAULT_PER_LEVEL_S: f64 = 10.0;

/// One shaker for the feasibility check and the kinematic monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct WireShaker {
    /// The drive's table index.
    pub drive: u32,
    pub shaker: ShakerRecord,
    pub moving_mass: MovingMass,
}

/// A parsed MIMO Sine configuration, every default filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct WireMsine {
    pub title: String,
    /// The engine's setup, built from the configuration (the rate the one asked
    /// of the station; the band as given).
    pub setup: MimoSineSetup,
    /// The rate as sent; null asks for [`DEFAULT_FS_HZ`].
    pub fs_given: Option<f64>,
    pub tick_ms: f64,
    /// The station's sine structure by name (`station.select`), set by the
    /// service after parsing; null takes the linear one of the run's size.
    pub structure: Option<String>,
    pub feasibility: Option<Vec<WireShaker>>,
    pub operator: String,
    pub article: String,
    pub notes: String,
    pub specification: Option<(String, String)>,
    pub record_streams: bool,
    pub metadata: Value,
    /// The configuration as sent, for `mimo_sine.config` and `mimo_sine.reset`.
    pub raw: Value,
}

/// Everything a configuration's checks are made against.
pub struct CheckContext<'a> {
    pub table: &'a ChannelTable,
    pub law: &'a dyn Fn(&str) -> Option<LawFacts>,
    pub timeout_s: f64,
    /// True when the simulator is the selected station.
    pub sim: bool,
}

/// What configure computes from a configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub band_hz: (f64, f64),
    pub shape: &'static str,
    pub controls: usize,
    pub drives: usize,
    pub limits: usize,
    pub grid: Grid,
    pub profiles: Vec<Value>,
    pub test_s: Option<f64>,
    pub identification_s: f64,
    pub call_budget_s: f64,
    pub safe_stop_s: f64,
    pub tracking: Tracking,
    pub law: Value,
    pub feasibility: (&'static str, String),
    /// The simulator's structure the run would play, on the simulator.
    pub structure: Option<String>,
}

impl Derived {
    pub fn to_json(&self) -> Value {
        json!({
            "band_hz": [num(self.band_hz.0), num(self.band_hz.1)],
            "shape": self.shape,
            "controls": self.controls,
            "drives": self.drives,
            "limits": self.limits,
            "grid": self.grid.to_json(),
            "profiles": self.profiles,
            "test_s": self.test_s.map_or(Value::Null, |s| num(round(s, 1))),
            "identification_s": num(round(self.identification_s, 1)),
            "call_budget_s": num(round(self.call_budget_s, 7)),
            "stop_ramp_db_per_s": num(STOP_RAMP_DB_PER_S),
            "safe_stop_s": num(round(self.safe_stop_s, 3)),
            "tracking": self.tracking.to_json(),
            "law": self.law,
            "feasibility": {"verdict": self.feasibility.0, "message": self.feasibility.1},
        })
    }
}

fn bad(path: &str, message: impl Into<String>) -> MethodError {
    MethodError::bad_params(path, message)
}

fn mode_of(m: RunMode) -> LevelMode {
    match m {
        RunMode::Auto => LevelMode::Auto,
        RunMode::Manual => LevelMode::Manual,
    }
}

pub fn mode_name(m: LevelMode) -> &'static str {
    match m {
        LevelMode::Auto => "auto",
        LevelMode::Manual => "manual",
    }
}

pub fn direction_name(d: SweepDirection) -> &'static str {
    match d {
        SweepDirection::Up => "up",
        SweepDirection::Down => "down",
    }
}

fn parse_test(v: &Value) -> Result<TestPlan, MethodError> {
    let t = Obj::of(v, "config.test")?;
    match t.opt_str("kind")? {
        Some("sweep") => {
            let band = band_hz(&t, "band_hz")?;
            let rate = parse_rate(&t, "rate")?;
            let e = t.obj("end")?;
            let sweeps = e.opt_count("sweeps")?;
            let duration_s = e.opt_f64("duration_s")?;
            let direction = t.choice("direction", &DIRECTIONS, Some(SweepDirection::Up))?;
            let target = match (sweeps, duration_s) {
                (Some(n), Some(s)) => SweepTarget::WhicheverFirst {
                    sweeps: u32::try_from(n).unwrap_or(u32::MAX),
                    duration_s: s,
                },
                (Some(n), None) => SweepTarget::Sweeps(u32::try_from(n).unwrap_or(u32::MAX)),
                (None, Some(s)) => SweepTarget::DurationS(s),
                (None, None) => {
                    return Err(bad(
                        &e.path,
                        "the sweep's \"end\" gives \"sweeps\", \"duration_s\" or both",
                    ))
                }
            };
            Ok(TestPlan::Sweep(BidirSweepConfig {
                band,
                rate,
                start_hz: t.opt_f64("start_hz")?.unwrap_or(match direction {
                    SweepDirection::Up => band.0,
                    SweepDirection::Down => band.1,
                }),
                initial_direction: direction,
                target,
            }))
        }
        Some("dwell") => Ok(TestPlan::Dwell(DwellPlan {
            freq_hz: t.f64("freq_hz")?,
            for_: parse_dwell_for(t.require("end")?, &t.at("end"))?,
        })),
        Some("stepped") => {
            let dwell = match t.m.get("dwell") {
                None | Some(Value::Null) => None,
                Some(d) => Some(parse_dwell_for(d, &t.at("dwell"))?),
            };
            match (t.m.get("band_hz"), t.array("frequencies_hz")?) {
                (Some(_), None) => Ok(TestPlan::Stepped(StepPlan::Grid(StepSineConfig {
                    band: band_hz(&t, "band_hz")?,
                    steps_per_octave: t.f64("steps_per_octave")?,
                    spacing: t.choice("spacing", &SPACINGS, Some(StepSpacing::Log))?,
                    dwell,
                }))),
                (None, Some(list)) => {
                    let frequencies_hz = list
                        .iter()
                        .enumerate()
                        .map(|(i, v)| {
                            v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| {
                                bad(
                                    &format!("config.test.frequencies_hz[{i}]"),
                                    "a step's frequency is a number of hertz",
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let dwell = dwell.ok_or_else(|| {
                        bad(
                            "config.test.dwell",
                            "a stepped sine on a list gives each step's \"dwell\"",
                        )
                    })?;
                    Ok(TestPlan::Stepped(StepPlan::List {
                        frequencies_hz,
                        dwell,
                    }))
                }
                _ => Err(bad(
                    "config.test",
                    "a stepped sine gives \"band_hz\" (a grid) or \"frequencies_hz\" (a list), \
                     one of the two",
                )),
            }
        }
        _ => Err(bad(
            "config.test.kind",
            "\"config.test.kind\" is \"sweep\", \"dwell\" or \"stepped\"",
        )),
    }
}

fn parse_phase(v: &Value, path: &str) -> Result<Vec<PhasePoint>, MethodError> {
    let list = v
        .as_array()
        .ok_or_else(|| bad(path, format!("\"{path}\" is a list of phase rows")))?;
    list.iter()
        .enumerate()
        .map(|(i, r)| {
            let o = Obj::of(r, &format!("{path}[{i}]"))?;
            Ok(PhasePoint {
                freq_hz: o.f64("freq_hz")?,
                phase_deg: o.f64("phase_deg")?,
            })
        })
        .collect()
}

fn parse_shaker(o: &Obj<'_>) -> Result<WireShaker, MethodError> {
    let drive = channel_index(o, "drive")?;
    let f = crate::sine::config::parse_feasibility(o)?;
    Ok(WireShaker {
        drive,
        shaker: f.shaker,
        moving_mass: f.moving_mass,
    })
}

/// Parse `params.config`.
pub fn parse(config: &Value) -> Result<WireMsine, MethodError> {
    let c = Obj::of(config, "config")?;
    for k in ["capture", "cola"] {
        if !matches!(c.m.get(k), None | Some(Value::Null)) {
            return Err(bad(
                &format!("config.{k}"),
                format!(
                    "MIMO Sine takes no \"{k}\": every output of the module is a drive, and every \
                     run is recorded in its run folder."
                ),
            ));
        }
    }
    let title = c.opt_str("title")?.unwrap_or("").to_owned();
    let ch = c.obj("channels")?;
    let mut controls = Vec::new();
    for (i, v) in ch.array("controls")?.into_iter().flatten().enumerate() {
        let o = Obj::of(v, &format!("config.channels.controls[{i}]"))?;
        controls.push(ControlChannel {
            channel: channel_index(&o, "channel")? as usize,
            profile: parse_profile(o.require("profile")?, &o.at("profile"))?,
            phase: match o.m.get("phase") {
                None | Some(Value::Null) => Vec::new(),
                Some(p) => parse_phase(p, &o.at("phase"))?,
            },
        });
    }
    let mut drives = Vec::new();
    for (i, v) in ch.array("drives")?.into_iter().flatten().enumerate() {
        let o = Obj::of(v, &format!("config.channels.drives[{i}]"))?;
        drives.push(DriveChannel {
            channel: channel_index(&o, "channel")? as usize,
            ceiling_vpk: o.f64("ceiling_vpk")?,
            abort_vpk: o.f64("abort_vpk")?,
        });
    }
    let phase_specified = c.opt_bool("phase_specified")?.unwrap_or(true);
    let test = parse_test(c.require("test")?)?;
    let is_sweep = matches!(test, TestPlan::Sweep(_));

    let acq = c.opt_obj("acquisition")?;
    let fs_given = match &acq {
        Some(a) => a.opt_f64("fs_hz")?,
        None => None,
    };
    let tick_ms = match &acq {
        Some(a) => a.f64_or("tick_ms", DEFAULT_TICK_MS)?,
        None => DEFAULT_TICK_MS,
    };

    // The schedule: one level, full, the test's own end, unless given.
    let default_rung = if is_sweep {
        RungFor::Sweeps(1)
    } else {
        RungFor::Seconds(DEFAULT_PER_LEVEL_S)
    };
    let mut schedule = Schedule {
        mode: LevelMode::Auto,
        ladder: LadderConfig {
            sysid_level_db: -20.0,
            start_db: 0.0,
            table_db: vec![0.0],
            per_rung: default_rung,
            full_for: RungFor::Seconds(UNTIL_THE_END_S),
        },
    };
    if let Some(s) = c.opt_obj("schedule")? {
        schedule.mode = mode_of(s.choice("mode", &MODES, Some(RunMode::Auto))?);
        if let Some(list) = s.array("levels_db")? {
            schedule.ladder.table_db = list
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
        schedule.ladder.start_db = s
            .opt_f64("start_db")?
            .unwrap_or_else(|| schedule.ladder.table_db.first().copied().unwrap_or(0.0));
        if let Some(r) = parse_rung_for(&s, "per_level")? {
            schedule.ladder.per_rung = r;
        }
        if let Some(r) = parse_rung_for(&s, "full_level")? {
            schedule.ladder.full_for = r;
        }
    }

    let mut identification = IdentificationSettings::default();
    if let Some(i) = c.opt_obj("identification")? {
        identification.level_db = i.f64_or("level_db", identification.level_db)?;
        identification.resolution_hz = i.f64_or("resolution_hz", identification.resolution_hz)?;
        identification.frames = i
            .opt_count("frames")?
            .map(|n| u32::try_from(n).unwrap_or(u32::MAX));
        identification.trust_coherence =
            i.f64_or("trust_coherence", identification.trust_coherence)?;
        identification.gate_coherence =
            i.f64_or("gate_coherence", identification.gate_coherence)?;
        identification.review = i.opt_bool("review")?.unwrap_or(identification.review);
    }
    schedule.ladder.sysid_level_db = identification.level_db;

    let mut tracking = MsTracking::default();
    let mut law = LawChoice::default();
    let mut guards = GuardSettings::default();
    if let Some(o) = c.opt_obj("control")? {
        if let Some(t) = o.opt_obj("tracking")? {
            tracking = match t.opt_str("kind")? {
                Some("fixed") => MsTracking::Fixed { hz: t.f64("hz")? },
                Some("proportional") => MsTracking::Proportional {
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
            law = LawChoice {
                id: id.to_owned(),
                params: match l.opt_obj("params")? {
                    None => Map::new(),
                    Some(p) => p.m.clone(),
                },
            };
        }
        guards.law_budget = o.f64_or("law_budget", guards.law_budget)?;
        guards.law_misses = o
            .opt_count("law_misses")?
            .map_or(guards.law_misses, |n| u32::try_from(n).unwrap_or(u32::MAX));
        guards.max_step_db = o.f64_or("max_step_db", guards.max_step_db)?;
        guards.max_rise_db_per_s = o.f64_or("max_rise_db_per_s", guards.max_rise_db_per_s)?;
        guards.rotation_cap_deg = o.opt_f64("rotation_cap_deg")?;
        guards.divergence_s = o.f64_or("divergence_s", guards.divergence_s)?;
        guards.abort_ticks = o
            .opt_count("abort_ticks")?
            .map_or(guards.abort_ticks, |n| u32::try_from(n).unwrap_or(u32::MAX));
    }

    let mut requests = RequestSettings::default();
    if let Some(r) = c.opt_obj("requests")? {
        requests.honour = r.opt_bool("honour")?.unwrap_or(requests.honour);
        requests.max_hold_s = r.f64_or("max_hold_s", requests.max_hold_s)?;
        requests.min_slow_factor = r.f64_or("min_slow_factor", requests.min_slow_factor)?;
    }

    let mut limits = Vec::new();
    if let Some(lim) = c.opt_obj("limits")? {
        guards.kinematic_monitor = lim.opt_bool("kinematic_monitor")?.unwrap_or(true);
        for (i, v) in lim.array("channels")?.into_iter().flatten().enumerate() {
            let o = Obj::of(v, &format!("config.limits.channels[{i}]"))?;
            limits.push(LimitChannel {
                channel: channel_index(&o, "channel")? as usize,
                profile: parse_profile(o.require("profile")?, &o.at("profile"))?,
            });
        }
        let nd = NotchSettings::default();
        if let Some(n) = lim.opt_obj("notch")? {
            guards.notch = NotchSettings {
                smoothing_ticks: n
                    .opt_count("smoothing_ticks")?
                    .map_or(nd.smoothing_ticks, |x| u32::try_from(x).unwrap_or(u32::MAX)),
                release_ramp_db_per_s: n
                    .f64_or("release_ramp_db_per_s", nd.release_ramp_db_per_s)?,
                release_hysteresis_db: n
                    .f64_or("release_hysteresis_db", nd.release_hysteresis_db)?,
                near_bound_db: n.f64_or("near_bound_db", nd.near_bound_db)?,
            };
        }
    }

    let feasibility = match c.opt_obj("feasibility")? {
        None => None,
        Some(f) => {
            let mut out = Vec::new();
            for (i, v) in f.array("shakers")?.into_iter().flatten().enumerate() {
                out.push(parse_shaker(&Obj::of(
                    v,
                    &format!("config.feasibility.shakers[{i}]"),
                )?)?);
            }
            Some(out)
        }
    };
    // The simulator's structure is the station's (`station.select`), set by the
    // service after parsing; a configuration does not name it.
    let structure = None;
    let words =
        |k: &str| -> Result<String, MethodError> { Ok(c.opt_str(k)?.unwrap_or("").to_owned()) };
    let (operator, article, notes) = (words("operator")?, words("article")?, words("notes")?);
    let specification = match c.m.get("specification") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let o = Obj::of(v, "config.specification")?;
            Some((
                o.opt_str("document")?.unwrap_or("").to_owned(),
                o.opt_str("clause")?.unwrap_or("").to_owned(),
            ))
        }
    };
    let record_streams = match c.m.get("record") {
        None | Some(Value::Null) => true,
        Some(v) => Obj::of(v, "config.record")?
            .opt_bool("streams")?
            .unwrap_or(true),
    };
    let metadata = match c.array("metadata")? {
        None => Value::Array(Vec::new()),
        Some(list) => {
            for (i, m) in list.iter().enumerate() {
                let o = Obj::of(m, &format!("config.metadata[{i}]"))?;
                for k in ["key", "value"] {
                    o.opt_str(k)?
                        .ok_or_else(|| bad(&o.at(k), format!("\"{}\" is text", o.at(k))))?;
                }
            }
            Value::Array(list.clone())
        }
    };
    let setup = MimoSineSetup {
        controls,
        drives,
        limits,
        phase_specified,
        test,
        schedule,
        fs_hz: fs_given.unwrap_or(DEFAULT_FS_HZ),
        tick_s: tick_ms / 1000.0,
        tracking,
        identification,
        law,
        requests,
        guards,
    };
    Ok(WireMsine {
        title,
        setup,
        fs_given,
        tick_ms,
        structure,
        feasibility,
        operator,
        article,
        notes,
        specification,
        record_streams,
        metadata,
        raw: config.clone(),
    })
}

// ---------------------------------------------------------------------------------
// Checking: every problem is a named item.
// ---------------------------------------------------------------------------------

fn out_of_range(r: &mut Report, field: &str, ok: bool, text: String) {
    if !ok {
        r.errors.push(Item::field("out_of_range", field, text));
    }
}

fn within(x: f64, lo_excl: f64, hi_incl: f64) -> bool {
    x.is_finite() && x > lo_excl && x <= hi_incl
}

fn inside(x: f64, lo: f64, hi: f64) -> bool {
    x.is_finite() && x >= lo && x <= hi
}

/// The test's kind as the wire names it.
pub fn test_kind(t: &TestPlan) -> &'static str {
    t.test_type().as_str()
}

/// The field a setup rule of the engine's points at, on the wire.
fn wire_field(engine_field: &str) -> String {
    let f = engine_field;
    if let Some(rest) = f.strip_prefix("controls") {
        return format!("channels.controls{rest}");
    }
    if let Some(rest) = f.strip_prefix("drives") {
        return format!("channels.drives{rest}");
    }
    if let Some(rest) = f.strip_prefix("limits") {
        return format!("limits.channels{rest}");
    }
    if let Some(rest) = f.strip_prefix("guards.notch") {
        return format!("limits.notch{rest}");
    }
    if f == "guards.kinematic_monitor" {
        return "limits.kinematic_monitor".into();
    }
    if let Some(rest) = f.strip_prefix("guards.") {
        return format!("control.{rest}");
    }
    if f == "fs_hz" {
        return "acquisition.fs_hz".into();
    }
    if f == "tick_s" {
        return "acquisition.tick_ms".into();
    }
    if f == "tracking" || f.starts_with("tracking.") {
        return format!("control.{f}");
    }
    if f.starts_with("law") {
        return format!("control.{f}");
    }
    f.to_owned()
}

/// The protocol's reason for one of the engine's setup rules.
fn wire_reason(rule: SetupRule, field: &str) -> &'static str {
    match rule {
        SetupRule::ControlsOutOfRange => "controls_out_of_range",
        SetupRule::DrivesOutOfRange => "drives_out_of_range",
        SetupRule::LimitsOutOfRange => "limits_out_of_range",
        SetupRule::DuplicateChannel => "duplicate_channel",
        SetupRule::AbortBelowCeiling => "abort_below_ceiling",
        SetupRule::ProfileNotCovering => "profile_not_covering",
        SetupRule::PhaseMissing => "phase_missing",
        SetupRule::PhaseInvalid => "phase_invalid",
        SetupRule::BandAboveGrid => "band_above_grid",
        SetupRule::TickOutOfRange => "tick_out_of_range",
        SetupRule::LawUnknown => "law_unknown",
        SetupRule::LawShapeNotAccepted => "law_shape_not_accepted",
        SetupRule::LawTestTypeNotAccepted => "law_test_type_not_accepted",
        SetupRule::LawAmplitudeOnlyNotAccepted => "law_amplitude_only_not_accepted",
        SetupRule::ScheduleInvalid if field.contains("start") => "start_not_a_level",
        SetupRule::ScheduleInvalid => "levels_not_ascending",
        SetupRule::TestInvalid if field.contains("frequencies") => "steps_empty",
        SetupRule::ProfileInvalid => "profile_too_short",
        SetupRule::CeilingInvalid
        | SetupRule::TestInvalid
        | SetupRule::RateOutOfRange
        | SetupRule::TrackingInvalid
        | SetupRule::IdentificationInvalid
        | SetupRule::RequestBoundsInvalid
        | SetupRule::GuardInvalid => "out_of_range",
    }
}

impl WireMsine {
    /// The rate the run asks the station for, Hz.
    pub fn fs_hz(&self) -> f64 {
        self.fs_given.unwrap_or(DEFAULT_FS_HZ)
    }

    pub fn tick_s(&self) -> f64 {
        self.tick_ms / 1000.0
    }

    /// Whether the test is a sweep.
    pub fn is_sweep(&self) -> bool {
        matches!(self.setup.test, TestPlan::Sweep(_))
    }

    pub fn kind(&self) -> &'static str {
        test_kind(&self.setup.test)
    }

    pub fn band(&self) -> (f64, f64) {
        self.setup.test.band_hz()
    }

    /// The test's length in auto mode from its own end, s: a sweep's sweeps or
    /// time, a dwell's time, every step's dwell.
    pub fn test_s(&self) -> Option<f64> {
        match &self.setup.test {
            TestPlan::Sweep(c) => {
                let full = sweep_seconds(c.band.0, c.band.1, c.rate).abs();
                let first = match c.initial_direction {
                    SweepDirection::Up => sweep_seconds(c.start_hz, c.band.1, c.rate).abs(),
                    SweepDirection::Down => sweep_seconds(c.band.0, c.start_hz, c.rate).abs(),
                };
                let by = |n: u32| {
                    if n == 0 {
                        0.0
                    } else {
                        first + full * f64::from(n - 1)
                    }
                };
                Some(match c.target {
                    SweepTarget::Sweeps(n) => by(n),
                    SweepTarget::DurationS(s) => s,
                    SweepTarget::WhicheverFirst { sweeps, duration_s } => {
                        by(sweeps).min(duration_s)
                    }
                })
            }
            TestPlan::Dwell(d) => Some(match d.for_ {
                DwellFor::Seconds(s) => s,
                DwellFor::Cycles(c) => c as f64 / d.freq_hz,
            }),
            TestPlan::Stepped(StepPlan::List {
                frequencies_hz,
                dwell,
            }) => Some(
                frequencies_hz
                    .iter()
                    .map(|f| match *dwell {
                        DwellFor::Seconds(s) => s,
                        DwellFor::Cycles(c) => c as f64 / f,
                    })
                    .sum(),
            ),
            TestPlan::Stepped(StepPlan::Grid(g)) => {
                let steps = hertzforge_sine::StepSine::new(*g).ok()?;
                let d = g.dwell?;
                Some(
                    steps
                        .ladder()
                        .iter()
                        .map(|f| match d {
                            DwellFor::Seconds(s) => s,
                            DwellFor::Cycles(c) => c as f64 / f,
                        })
                        .sum(),
                )
            }
        }
    }

    /// The step frequencies of a stepped sine, in the order played.
    pub fn steps(&self) -> Option<Vec<f64>> {
        match &self.setup.test {
            TestPlan::Stepped(StepPlan::List { frequencies_hz, .. }) => {
                Some(frequencies_hz.clone())
            }
            TestPlan::Stepped(StepPlan::Grid(g)) => hertzforge_sine::StepSine::new(*g)
                .ok()
                .map(|s| s.ladder().to_vec()),
            _ => None,
        }
    }

    /// The identification's length at `fs_hz`, seconds: its records, each played
    /// twice, and the probe.
    pub fn identification_s(&self, fs_hz: f64) -> f64 {
        let n = hertzforge_mimo_sine::ident::record_len_for(
            fs_hz,
            self.setup.identification.resolution_hz,
        );
        let m = self.setup.drives.len().max(1);
        let frames = self
            .setup
            .identification
            .frames
            .map(|f| f as usize)
            .unwrap_or_else(|| IdentificationSettings::default_frames(m))
            .max(m);
        (2 * (1 + frames) * n) as f64 / fs_hz
    }

    /// The shaker ratings the engine's kinematic monitor judges, one per shaker
    /// given, on the control in the same position as its drive.
    pub fn ratings(&self) -> Vec<ShakerRating> {
        if !self.setup.guards.kinematic_monitor {
            return Vec::new();
        }
        let Some(shakers) = &self.feasibility else {
            return Vec::new();
        };
        shakers
            .iter()
            .filter_map(|s| {
                let pos = self
                    .setup
                    .drives
                    .iter()
                    .position(|d| d.channel == s.drive as usize)?;
                (pos < self.setup.controls.len()).then(|| ShakerRating {
                    control: pos,
                    max_displacement_mm_pk: s
                        .shaker
                        .max_displacement_in_pp
                        .map(|r| r.value * 25.4 / 2.0),
                    max_velocity_m_s_pk: s.shaker.max_velocity_ips.map(|r| r.value * 0.0254),
                    max_acceleration_g_pk: s.shaker.max_accel_bare_g.map(|r| r.value),
                })
            })
            .collect()
    }

    /// The configuration's feasibility verdict: each shaker against the
    /// reference of the control in its drive's position, the worst of them.
    pub fn feasibility_verdict(&self) -> (&'static str, String) {
        let Some(shakers) = self.feasibility.as_ref().filter(|s| !s.is_empty()) else {
            return (
                "not_evaluated",
                "No shaker was given, so the references were not checked against shaker \
                 ratings and the kinematic monitor is off."
                    .into(),
            );
        };
        let rank = |v: &str| match v {
            "exceeded" => 3,
            "advisory" => 2,
            "pass" => 1,
            _ => 0,
        };
        let mut worst: (&'static str, String) = ("pass", String::new());
        let mut first = true;
        for s in shakers {
            let pos = self
                .setup
                .drives
                .iter()
                .position(|d| d.channel == s.drive as usize)
                .unwrap_or(0);
            let Some(c) = self.setup.controls.get(pos).or(self.setup.controls.first()) else {
                continue;
            };
            let v = profile::feasibility_verdict(&c.profile, Some((&s.shaker, &s.moving_mass)));
            if first || rank(v.0) > rank(worst.0) {
                worst = v;
                first = false;
            }
        }
        worst
    }

    /// Check the configuration against `ctx`. The derived values come back
    /// when there are no errors.
    pub fn check(&self, ctx: &CheckContext<'_>) -> (Report, Option<Derived>) {
        let mut r = Report::default();
        let s = &self.setup;
        let fs = self.fs_hz();
        let band = self.band();

        // Sizes.
        let (nc, nd, nl) = (s.controls.len(), s.drives.len(), s.limits.len());
        if nc == 0 || nc > MAX_CONTROLS {
            r.errors.push(Item::field(
                "controls_out_of_range",
                "channels.controls",
                format!("The run names {nc} controls; it takes 1 to {MAX_CONTROLS}."),
            ));
        }
        if nd == 0 || nd > MAX_DRIVES {
            r.errors.push(Item::field(
                "drives_out_of_range",
                "channels.drives",
                format!("The run names {nd} drives; it takes 1 to {MAX_DRIVES}."),
            ));
        }
        if nl > MAX_LIMITS {
            r.errors.push(Item::field(
                "limits_out_of_range",
                "limits.channels",
                format!("The run names {nl} limit channels; it takes at most {MAX_LIMITS}."),
            ));
        }
        self.check_channels(&mut r, ctx.table);

        // Profiles, phases and their cover of the band.
        for (i, c) in s.controls.iter().enumerate() {
            let at = format!("channels.controls[{i}].profile");
            let pr = check_profile(&c.profile, Locate::Field(&at));
            let ok = pr.errors.is_empty();
            r.extend(pr);
            if ok && self.band_ok() && !covers(&c.profile, band) {
                r.errors.push(not_covering(&at, &c.profile, band));
            }
            if s.phase_specified {
                if c.phase.is_empty() {
                    r.errors.push(Item::field(
                        "phase_missing",
                        format!("channels.controls[{i}].phase"),
                        format!(
                            "Control {} has no reference phase; give its phase rows, or turn \
                             phase_specified off for an amplitude-only run.",
                            c.channel
                        ),
                    ));
                }
                let mut last = 0.0;
                for (k, p) in c.phase.iter().enumerate() {
                    let ok = p.freq_hz.is_finite()
                        && p.freq_hz > last
                        && within(p.phase_deg, -180.0, 180.0);
                    if !ok {
                        r.errors.push(Item::field(
                            "phase_invalid",
                            format!("channels.controls[{i}].phase[{k}]"),
                            "A phase row's frequency is above 0 Hz and above the row before \
                             it, and its phase is above -180 and at most 180 degrees.",
                        ));
                    }
                    last = p.freq_hz;
                }
            }
        }
        for (i, l) in s.limits.iter().enumerate() {
            let at = format!("limits.channels[{i}].profile");
            let pr = check_profile(&l.profile, Locate::Field(&at));
            let ok = pr.errors.is_empty();
            r.extend(pr);
            if ok && self.band_ok() && !covers(&l.profile, band) {
                r.errors.push(not_covering(&at, &l.profile, band));
            }
        }
        for (i, d) in s.drives.iter().enumerate() {
            out_of_range(
                &mut r,
                &format!("channels.drives[{i}].ceiling_vpk"),
                within(d.ceiling_vpk, 0.0, MAX_DRIVE_V),
                format!(
                    "Drive {}'s ceiling is {} V; it is above 0 and at most {} V.",
                    d.channel,
                    num(d.ceiling_vpk),
                    num(MAX_DRIVE_V)
                ),
            );
            if !(d.abort_vpk.is_finite()
                && d.abort_vpk > d.ceiling_vpk
                && d.abort_vpk <= MAX_DRIVE_V)
            {
                r.errors.push(Item::field(
                    "abort_below_ceiling",
                    format!("channels.drives[{i}].abort_vpk"),
                    format!(
                        "Drive {}'s abort of {} V is not above its ceiling of {} V and at most \
                         {} V.",
                        d.channel,
                        num(d.abort_vpk),
                        num(d.ceiling_vpk),
                        num(MAX_DRIVE_V)
                    ),
                ));
            }
        }

        // Rate, tick and test.
        if let Some(f) = self.fs_given {
            out_of_range(
                &mut r,
                "acquisition.fs_hz",
                inside(f, FS_RANGE.0, FS_RANGE.1),
                format!(
                    "The rate is {} Hz; it runs from {} to {} Hz.",
                    num(f),
                    num(FS_RANGE.0),
                    num(FS_RANGE.1)
                ),
            );
        }
        let band_ok = self.check_test(&mut r, fs);

        // The law.
        let facts = (ctx.law)(&s.law.id);
        self.check_law(&mut r, facts.as_ref());
        let min_tick = if facts.as_ref().is_some_and(|f| !f.builtin) {
            MIN_TICK_MS_LAW_FILE
        } else {
            MIN_TICK_MS_BUILTIN
        };
        if !inside(self.tick_ms, min_tick, MAX_TICK_MS)
            || crate::sine::config::tick_frames(fs, self.tick_ms) < 2
        {
            r.errors.push(Item::field(
                "tick_out_of_range",
                "acquisition.tick_ms",
                format!(
                    "The control tick is {} ms; it runs from {} to {} ms for {}, and is at \
                     least two samples.",
                    num(self.tick_ms),
                    num(min_tick),
                    num(MAX_TICK_MS),
                    if min_tick > MIN_TICK_MS_BUILTIN {
                        "a law file"
                    } else {
                        "a built-in law"
                    }
                ),
            ));
        }
        self.check_schedule(&mut r);
        self.check_settings(&mut r);

        // The simulator's structure.
        let structure = if ctx.sim {
            self.check_structure(&mut r, ctx.table)
        } else {
            None
        };

        // The controlled stop inside the service's own stop budget.
        let ceiling = s
            .drives
            .iter()
            .map(|d| d.ceiling_vpk)
            .fold(0.0f64, f64::max);
        let safe_stop_s = SineProcessStep::safe_stop_seconds(ceiling.max(0.0));
        if ceiling.is_finite() && safe_stop_s > ctx.timeout_s {
            r.errors.push(Item::field(
                "safe_stop_budget",
                "channels.drives",
                format!(
                    "From the largest drive ceiling the controlled stop takes {} s, longer than \
                     the {} s the service waits for a stop; lower the ceilings.",
                    num(round(safe_stop_s, 3)),
                    num(ctx.timeout_s)
                ),
            ));
        }

        // The sweep against the tracking filter.
        let tracking = match (&s.test, band_ok && self.tracking_ok()) {
            (TestPlan::Sweep(c), true) => {
                Tracking::of(&hertzforge_sine::service::config::assess_tracking_lag(
                    c.band,
                    c.rate,
                    s.tracking.bandwidth_mode(),
                    hertzforge_hal::default_block_frames(fs),
                    fs,
                ))
            }
            _ => Tracking::of(&TrackingLagVerdict::Ok { ratio: 0.0 }),
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

        // The engine's own rules, the backstop: whatever the checks above did
        // not name.
        if r.errors.is_empty() {
            if let Err(e) = s.validate() {
                let field = wire_field(&e.field);
                r.errors.push(Item::field(
                    wire_reason(e.rule, &e.field),
                    field,
                    e.message.clone(),
                ));
            }
        }
        if !r.errors.is_empty() {
            return (r, None);
        }
        let Some(facts) = facts else {
            return (r, None);
        };
        let grid = Grid::of(fs, self.tick_ms, false);
        let derived = Derived {
            band_hz: band,
            shape: shape_name(s.shape()),
            controls: nc,
            drives: nd,
            limits: nl,
            grid,
            profiles: s
                .controls
                .iter()
                .map(|c| {
                    let mut v = profile::facts_json(&c.profile, band);
                    if let Value::Object(m) = &mut v {
                        m.insert("channel".into(), Value::from(c.channel));
                    }
                    v
                })
                .collect(),
            test_s: if s.schedule.mode == LevelMode::Auto {
                self.test_s()
            } else {
                None
            },
            identification_s: self.identification_s(fs),
            call_budget_s: s.guards.law_budget * grid.tick_s(),
            safe_stop_s,
            tracking,
            law: facts.derived_json(),
            feasibility: self.feasibility_verdict(),
            structure,
        };
        (r, Some(derived))
    }

    fn band_ok(&self) -> bool {
        let b = self.band();
        b.0.is_finite() && b.1.is_finite() && b.0 > 0.0 && b.1 >= b.0
    }

    fn tracking_ok(&self) -> bool {
        match self.setup.tracking {
            MsTracking::Fixed { hz } => hz.is_finite() && hz > 0.0,
            MsTracking::Proportional { pct, max_hz } => {
                within(pct, 0.0, 100.0) && max_hz.is_finite() && max_hz > 0.0
            }
        }
    }

    fn check_channels(&self, r: &mut Report, table: &ChannelTable) {
        let s = &self.setup;
        let row = |ch: usize| table.channels.iter().find(|c| c.index as usize == ch);
        let mut seen: Vec<usize> = Vec::new();
        let mut dup = |r: &mut Report, ch: usize, field: String| {
            if seen.contains(&ch) {
                r.errors.push(Item::field(
                    "duplicate_channel",
                    field,
                    format!("Channel {ch} is named twice among the controls, drives and limits."),
                ));
            }
            seen.push(ch);
        };
        for (i, c) in s.controls.iter().enumerate() {
            let field = format!("channels.controls[{i}].channel");
            dup(r, c.channel, field.clone());
            match row(c.channel) {
                Some(t) if t.enabled && t.role == ChannelRole::Control => {}
                _ => r.errors.push(Item::field(
                    "control_unknown_channel",
                    field,
                    format!(
                        "Channel {} is not an enabled control row of the channel table.",
                        c.channel
                    ),
                )),
            }
        }
        let table_drives: Vec<usize> = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Drive)
            .map(|c| c.index as usize)
            .collect();
        for (i, d) in s.drives.iter().enumerate() {
            let field = format!("channels.drives[{i}].channel");
            dup(r, d.channel, field.clone());
            if !table_drives.contains(&d.channel) {
                r.errors.push(Item::field(
                    "drive_unknown_channel",
                    field,
                    format!(
                        "Channel {} is not an enabled drive row of the channel table.",
                        d.channel
                    ),
                ));
            }
        }
        let named: Vec<usize> = s.drives.iter().map(|d| d.channel).collect();
        if named.iter().all(|c| table_drives.contains(c)) && named != table_drives {
            r.errors.push(Item::field(
                "drive_unknown_channel",
                "channels.drives",
                format!(
                    "The run's drives are channels {named:?}; the table's enabled drive rows are \
                     {table_drives:?}. Name every enabled drive row, in the table's order, or \
                     disable the rows the run does not drive."
                ),
            ));
        }
        for (i, l) in s.limits.iter().enumerate() {
            let field = format!("limits.channels[{i}].channel");
            dup(r, l.channel, field.clone());
            match row(l.channel) {
                Some(t) if t.enabled && t.role == ChannelRole::Control => {
                    r.errors.push(Item::field(
                        "limit_on_control_channel",
                        field,
                        format!(
                            "Channel {} is a control row; a limit channel is an input that is \
                             not a control.",
                            l.channel
                        ),
                    ))
                }
                Some(t) if t.enabled && t.role != ChannelRole::Drive => {}
                _ => r.errors.push(Item::field(
                    "limit_unknown_channel",
                    field,
                    format!(
                        "Channel {} is not an enabled input row of the channel table.",
                        l.channel
                    ),
                )),
            }
        }
    }

    /// The test's own fields; true when its band is usable.
    fn check_test(&self, r: &mut Report, fs: f64) -> bool {
        let n_before = r.errors.len();
        let band = self.band();
        let field = match &self.setup.test {
            TestPlan::Dwell(_) => "test.freq_hz",
            TestPlan::Stepped(StepPlan::List { .. }) => "test.frequencies_hz",
            _ => "test.band_hz",
        };
        match &self.setup.test {
            TestPlan::Dwell(d) => {
                out_of_range(
                    r,
                    "test.freq_hz",
                    d.freq_hz.is_finite() && d.freq_hz > 0.0,
                    format!(
                        "The dwell's frequency is {} Hz; it is above 0.",
                        num(d.freq_hz)
                    ),
                );
                dwell_for(r, "test.end", d.for_);
            }
            TestPlan::Stepped(StepPlan::List {
                frequencies_hz,
                dwell,
            }) => {
                if frequencies_hz.is_empty() {
                    r.errors.push(Item::field(
                        "steps_empty",
                        "test.frequencies_hz",
                        "A stepped sine on a list needs at least one frequency.",
                    ));
                }
                for (i, f) in frequencies_hz.iter().enumerate() {
                    if !(f.is_finite() && *f > 0.0) {
                        r.errors.push(Item::field(
                            "frequency_not_positive",
                            format!("test.frequencies_hz[{i}]"),
                            format!("Step {i}'s frequency is {}; it is above 0 Hz.", num(*f)),
                        ));
                    }
                }
                dwell_for(r, "test.dwell", *dwell);
            }
            TestPlan::Stepped(StepPlan::Grid(g)) => {
                out_of_range(
                    r,
                    "test.band_hz",
                    band.0 > 0.0 && band.1 > band.0,
                    format!(
                        "The band runs from above 0 Hz to a higher edge (here {} to {} Hz).",
                        num(band.0),
                        num(band.1)
                    ),
                );
                out_of_range(
                    r,
                    "test.steps_per_octave",
                    g.steps_per_octave.is_finite() && g.steps_per_octave > 0.0,
                    "The steps per octave are above 0.".into(),
                );
                if let Some(d) = g.dwell {
                    dwell_for(r, "test.dwell", d);
                }
            }
            TestPlan::Sweep(c) => {
                out_of_range(
                    r,
                    "test.band_hz",
                    band.0 > 0.0 && band.1 > band.0,
                    format!(
                        "The band runs from above 0 Hz to a higher edge (here {} to {} Hz).",
                        num(band.0),
                        num(band.1)
                    ),
                );
                let (f, x) = match c.rate {
                    SweepRate::LogOctPerMin(x) => ("test.rate.oct_per_min", x),
                    SweepRate::LinearHzPerSec(x) => ("test.rate.hz_per_s", x),
                };
                out_of_range(
                    r,
                    f,
                    x.is_finite() && x > 0.0,
                    "The sweep's rate is above 0.".into(),
                );
                out_of_range(
                    r,
                    "test.start_hz",
                    c.start_hz >= c.band.0 && c.start_hz <= c.band.1,
                    format!(
                        "The sweep starts at {} Hz; it starts inside the band.",
                        num(c.start_hz)
                    ),
                );
                let (sweeps, secs) = match c.target {
                    SweepTarget::Sweeps(n) => (Some(n), None),
                    SweepTarget::DurationS(s) => (None, Some(s)),
                    SweepTarget::WhicheverFirst { sweeps, duration_s } => {
                        (Some(sweeps), Some(duration_s))
                    }
                };
                if let Some(n) = sweeps {
                    out_of_range(
                        r,
                        "test.end.sweeps",
                        n >= 1,
                        "The sweeps are at least 1.".into(),
                    );
                }
                if let Some(s) = secs {
                    out_of_range(
                        r,
                        "test.end.duration_s",
                        s.is_finite() && s > 0.0,
                        "The test time is above 0 s.".into(),
                    );
                }
            }
        }
        let band_ok = r.errors.len() == n_before;
        let edge = 0.45 * fs;
        if band_ok && band.1 >= edge {
            r.errors.push(Item::field(
                "band_above_grid",
                field,
                format!(
                    "The test reaches {} Hz, at or above 0.45 times the rate of {} Hz ({} Hz); \
                     raise the rate or lower the band.",
                    num(band.1),
                    num(fs),
                    num(round(edge, 1))
                ),
            ));
            return false;
        }
        band_ok
    }

    fn check_law(&self, r: &mut Report, facts: Option<&LawFacts>) {
        let s = &self.setup;
        let Some(f) = facts else {
            r.errors.push(Item::field(
                "law_unknown",
                "control.law.id",
                format!(
                    "No control law \"{}\" is in the catalog; laws.list with world sine_mimo \
                     names the ones there are.",
                    s.law.id
                ),
            ));
            return;
        };
        if let Err(test) = f.world {
            r.errors.push(Item::field(
                "law_wrong_world",
                "control.law.id",
                format!(
                    "{} is a law for {test}, not for MIMO Sine; choose a MIMO Sine law.",
                    f.name
                ),
            ));
            return;
        }
        if !f.shapes.contains(&s.shape()) {
            r.errors.push(Item::field(
                "law_shape_not_accepted",
                "control.law.id",
                format!(
                    "{} does not accept a run of {} controls and {} drives ({}).",
                    f.name,
                    s.controls.len(),
                    s.drives.len(),
                    shape_name(s.shape()).replace('_', " ")
                ),
            ));
        }
        let tt = s.test_type();
        if !f.test_types.contains(&tt) {
            r.errors.push(Item::field(
                "law_test_type_not_accepted",
                "control.law.id",
                format!(
                    "{} does not accept a {}; it accepts {}.",
                    f.name,
                    tt.as_str(),
                    and_list(&f.test_types.iter().map(|t| t.as_str()).collect::<Vec<_>>())
                ),
            ));
        }
        if !s.phase_specified && !f.amplitude_only {
            r.errors.push(Item::field(
                "law_amplitude_only_not_accepted",
                "phase_specified",
                format!(
                    "{} needs every control's reference phase; give the phases, or choose a \
                     law that accepts an amplitude-only reference.",
                    f.name
                ),
            ));
        }
        for (k, v) in &s.law.params {
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

    fn check_schedule(&self, r: &mut Report) {
        let l = &self.setup.schedule.ladder;
        if l.table_db.is_empty() {
            r.errors.push(Item::field(
                "levels_empty",
                "schedule.levels_db",
                "The schedule needs at least one level.",
            ));
            return;
        }
        for (i, x) in l.table_db.iter().enumerate() {
            out_of_range(
                r,
                &format!("schedule.levels_db[{i}]"),
                inside(*x, LEVEL_DB_RANGE.0, LEVEL_DB_RANGE.1),
                format!("A level is {} dB; it runs from -60 to 12 dB.", num(*x)),
            );
        }
        if l.table_db.windows(2).any(|w| w[1] <= w[0]) {
            r.errors.push(Item::field(
                "levels_not_ascending",
                "schedule.levels_db",
                "The levels run in ascending order, the last full level.",
            ));
        }
        if !l.table_db.iter().any(|x| (x - l.start_db).abs() < 1e-9) {
            r.errors.push(Item::field(
                "start_not_a_level",
                "schedule.start_db",
                format!(
                    "The run starts at {} dB, which is not a level of the schedule.",
                    num(l.start_db)
                ),
            ));
        }
        if !self.is_sweep() {
            for (k, rung) in [("per_level", l.per_rung), ("full_level", l.full_for)] {
                if let RungFor::Sweeps(_) = rung {
                    r.errors.push(Item::field(
                        "out_of_range",
                        format!("schedule.{k}"),
                        "A level is held for sweeps only in a sweep; give seconds.",
                    ));
                }
            }
        }
        for (k, rung) in [("per_level", l.per_rung), ("full_level", l.full_for)] {
            match rung {
                RungFor::Seconds(s) if !(s.is_finite() && s > 0.0) => r.errors.push(Item::field(
                    "out_of_range",
                    format!("schedule.{k}.seconds"),
                    "A level's time is above 0 s.",
                )),
                RungFor::Sweeps(0) => r.errors.push(Item::field(
                    "out_of_range",
                    format!("schedule.{k}.sweeps"),
                    "A level's sweeps are at least 1.",
                )),
                _ => {}
            }
        }
    }

    fn check_settings(&self, r: &mut Report) {
        let s = &self.setup;
        let i = &s.identification;
        out_of_range(
            r,
            "identification.level_db",
            inside(i.level_db, -60.0, 0.0),
            "The identification's level runs from -60 to 0 dB.".into(),
        );
        out_of_range(
            r,
            "identification.resolution_hz",
            inside(i.resolution_hz, 0.05, 4.0),
            "The identification's line spacing runs from 0.05 to 4 Hz.".into(),
        );
        if let Some(f) = i.frames {
            out_of_range(
                r,
                "identification.frames",
                (1..=4096).contains(&f),
                "The records averaged run from 1 to 4096.".into(),
            );
        }
        for (k, x) in [
            ("trust_coherence", i.trust_coherence),
            ("gate_coherence", i.gate_coherence),
        ] {
            out_of_range(
                r,
                &format!("identification.{k}"),
                inside(x, 0.0, 1.0),
                "A coherence runs from 0 to 1.".into(),
            );
        }
        let g = &s.guards;
        out_of_range(
            r,
            "control.law_budget",
            within(g.law_budget, 0.0, 1.0),
            "The law's share of the tick is above 0 and at most 1.".into(),
        );
        out_of_range(
            r,
            "control.law_misses",
            (1..=100).contains(&g.law_misses),
            "The unusable calls in a row run from 1 to 100.".into(),
        );
        out_of_range(
            r,
            "control.max_step_db",
            inside(g.max_step_db, 1.0, 3.0),
            "The change cap runs from 1 to 3 dB.".into(),
        );
        out_of_range(
            r,
            "control.max_rise_db_per_s",
            within(g.max_rise_db_per_s, 0.0, 10_000.0),
            "The rise cap is above 0 and at most 10000 dB a second.".into(),
        );
        if let Some(x) = g.rotation_cap_deg {
            out_of_range(
                r,
                "control.rotation_cap_deg",
                within(x, 0.0, 180.0),
                "The rotation cap is above 0 and at most 180 degrees.".into(),
            );
        }
        out_of_range(
            r,
            "control.divergence_s",
            within(g.divergence_s, 0.0, 60.0),
            "The divergence time is above 0 and at most 60 s.".into(),
        );
        out_of_range(
            r,
            "control.abort_ticks",
            (1..=100).contains(&g.abort_ticks),
            "The ticks outside the abort band run from 1 to 100.".into(),
        );
        if !self.tracking_ok() {
            r.errors.push(Item::field(
                "out_of_range",
                "control.tracking",
                "The tracking filter's bandwidth is above 0 (a percentage at most 100).",
            ));
        }
        let q = &s.requests;
        out_of_range(
            r,
            "requests.max_hold_s",
            within(q.max_hold_s, 0.0, 2.0),
            "A granted hold lasts above 0 and at most 2 s.".into(),
        );
        out_of_range(
            r,
            "requests.min_slow_factor",
            inside(q.min_slow_factor, 0.1, 1.0),
            "The lowest slow factor runs from 0.1 to 1.".into(),
        );
        let n = &g.notch;
        out_of_range(
            r,
            "limits.notch.smoothing_ticks",
            n.smoothing_ticks >= 1,
            "The notch's smoothing is at least one tick.".into(),
        );
        out_of_range(
            r,
            "limits.notch.release_ramp_db_per_s",
            n.release_ramp_db_per_s.is_finite() && n.release_ramp_db_per_s > 0.0,
            "The notch's release ramp is above 0 dB a second.".into(),
        );
        out_of_range(
            r,
            "limits.notch.release_hysteresis_db",
            inside(n.release_hysteresis_db, 0.0, 6.0),
            "The notch's release hysteresis runs from 0 to 6 dB.".into(),
        );
        out_of_range(
            r,
            "limits.notch.near_bound_db",
            within(n.near_bound_db, 0.0, 20.0),
            "The notch's near-bound margin is above 0 and at most 20 dB.".into(),
        );
    }

    /// The simulator's structure for this run on `table`: named, or the linear
    /// one of its size. Refused by name when unknown or of another size.
    fn check_structure(&self, r: &mut Report, table: &ChannelTable) -> Option<String> {
        let inputs = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .count();
        let drives = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Drive)
            .count();
        let s = match &self.structure {
            Some(name) => match hertzforge_station::SimSineStructure::from_name(name) {
                Some(s) => s,
                None => {
                    r.errors.push(Item::field(
                        "structure_unknown",
                        "station.simulator.sine_structure",
                        format!(
                            "The simulator has no structure \"{name}\"; it plays {}.",
                            and_list(&structure_names())
                        ),
                    ));
                    return None;
                }
            },
            None => match default_structure(inputs, drives) {
                Some(s) => s,
                None => {
                    r.errors.push(Item::field(
                        "structure_wrong_size",
                        "station.simulator.sine_structure",
                        format!(
                            "The simulator has no sine structure of {inputs} inputs and {drives} \
                             drives; size the table to one of {}.",
                            and_list(&structure_names())
                        ),
                    ));
                    return None;
                }
            },
        };
        let (n_out, n_d) = s.dims();
        if (inputs, drives) != (n_out, n_d) {
            r.errors.push(Item::field(
                "structure_wrong_size",
                "station.simulator.sine_structure",
                format!(
                    "The simulator's structure \"{}\" has {n_out} inputs and {n_d} drives; the \
                     table has {inputs} enabled inputs and {drives} drives.",
                    s.name()
                ),
            ));
            return None;
        }
        Some(s.name().to_owned())
    }
}

/// Every sine structure's name.
pub fn structure_names() -> Vec<&'static str> {
    hertzforge_station::SimSineStructure::ALL
        .iter()
        .map(|s| s.name())
        .collect()
}

/// The linear structure of a table's size, where there is one.
pub fn default_structure(
    inputs: usize,
    drives: usize,
) -> Option<hertzforge_station::SimSineStructure> {
    use hertzforge_station::SimSineStructure as S;
    [S::Linear4x4, S::Tall6x3, S::Wide3x6]
        .into_iter()
        .find(|s| s.dims() == (inputs, drives))
}

fn and_list(xs: &[&str]) -> String {
    match xs.len() {
        0 => String::new(),
        1 => xs[0].to_owned(),
        n => format!("{} and {}", xs[..n - 1].join(", "), xs[n - 1]),
    }
}

fn dwell_for(r: &mut Report, field: &str, d: DwellFor) {
    match d {
        DwellFor::Seconds(s) => out_of_range(
            r,
            &format!("{field}.seconds"),
            s.is_finite() && s > 0.0,
            "A dwell lasts above 0 s.".into(),
        ),
        DwellFor::Cycles(c) => out_of_range(
            r,
            &format!("{field}.cycles"),
            c >= 1,
            "A dwell lasts at least one cycle.".into(),
        ),
    }
}

fn not_covering(at: &str, p: &SineProfile, band: (f64, f64)) -> Item {
    let (a, b) = (
        p.rows.first().map_or(0.0, |r| r.freq_hz),
        p.rows.last().map_or(0.0, |r| r.freq_hz),
    );
    Item::field(
        "profile_not_covering",
        at,
        format!(
            "The profile runs from {} to {} Hz; the test runs from {} to {} Hz.",
            num(a),
            num(b),
            num(band.0),
            num(band.1)
        ),
    )
}

/// The test as resolved, for the record.
pub fn test_json(t: &TestPlan) -> Value {
    let pair = |p: (f64, f64)| json!([num(p.0), num(p.1)]);
    let rate = |r: &SweepRate| match *r {
        SweepRate::LogOctPerMin(x) => json!({"law": "log", "oct_per_min": num(x)}),
        SweepRate::LinearHzPerSec(x) => json!({"law": "linear", "hz_per_s": num(x)}),
    };
    let dwell = |d: &DwellFor| match *d {
        DwellFor::Seconds(s) => json!({"seconds": num(s)}),
        DwellFor::Cycles(c) => json!({"cycles": c}),
    };
    match t {
        TestPlan::Sweep(c) => {
            let (sweeps, secs) = match c.target {
                SweepTarget::Sweeps(n) => (Some(n), None),
                SweepTarget::DurationS(s) => (None, Some(s)),
                SweepTarget::WhicheverFirst { sweeps, duration_s } => {
                    (Some(sweeps), Some(duration_s))
                }
            };
            json!({"kind": "sweep", "band_hz": pair(c.band), "rate": rate(&c.rate),
                   "start_hz": num(c.start_hz), "direction": direction_name(c.initial_direction),
                   "sweeps": sweeps, "duration_s": secs.map(num)})
        }
        TestPlan::Dwell(d) => {
            json!({"kind": "dwell", "freq_hz": num(d.freq_hz), "dwell": dwell(&d.for_)})
        }
        TestPlan::Stepped(StepPlan::Grid(g)) => json!({
            "kind": "stepped", "band_hz": pair(g.band),
            "steps_per_octave": num(g.steps_per_octave),
            "spacing": match g.spacing { StepSpacing::Log => "log", StepSpacing::Linear => "linear" },
            "dwell": g.dwell.as_ref().map(dwell)}),
        TestPlan::Stepped(StepPlan::List {
            frequencies_hz,
            dwell: d,
        }) => json!({
            "kind": "stepped",
            "frequencies_hz": frequencies_hz.iter().map(|f| num(*f)).collect::<Vec<_>>(),
            "dwell": dwell(d)}),
    }
}

/// The schedule as resolved, for the record.
pub fn schedule_json(s: &Schedule) -> Value {
    let rung = |r: RungFor| match r {
        RungFor::Seconds(s) if s >= UNTIL_THE_END_S => Value::Null,
        RungFor::Seconds(s) => json!({"seconds": num(s)}),
        RungFor::Sweeps(n) => json!({"sweeps": n}),
    };
    json!({"mode": mode_name(s.mode),
           "levels_db": s.ladder.table_db.iter().map(|l| num(*l)).collect::<Vec<_>>(),
           "start_db": num(s.ladder.start_db),
           "per_level": rung(s.ladder.per_rung),
           "full_level": rung(s.ladder.full_for)})
}

/// The tracking rule, for the record.
pub fn tracking_json(t: MsTracking) -> Value {
    match t {
        MsTracking::Fixed { hz } => json!({"kind": "fixed", "hz": num(hz)}),
        MsTracking::Proportional { pct, max_hz } => {
            json!({"kind": "proportional", "pct": num(pct), "max_hz": num(max_hz)})
        }
    }
}

/// The test's type, by its wire name.
pub fn test_type_name(t: TestType) -> &'static str {
    t.as_str()
}
