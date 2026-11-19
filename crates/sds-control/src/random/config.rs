//! The Random test configuration (schema/PROTOCOL.md, "Random Test Configuration").
//!
//! The wire configuration is parsed (a wrong type or a missing part is `bad_params`
//! naming it), then checked so every problem comes back as a named item, then built
//! into the controller's own `RandomTestConfig` and `LoopConfig`. The checks reuse
//! the controller's validators where it has them (the breakpoint table's report,
//! `MIN_CONTROL_LINES`, the loop's band bins) and add what the controller reports
//! only as a sentence. Nothing here is control math: the run is the controller's.

use crate::protocol::{num, ErrorCode, MethodError};
use crate::table::{Item, Report};
use hertzforge_breakpoints::{
    BreakpointError, BreakpointRow, BreakpointTable, BreakpointWarning, LevelSpec,
};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_random::service::config::{
    ControlChannel, LevelSchedule, LimitKind, LimitProfile, MetaField, RandomTestConfig, RunMode,
};
use hertzforge_random::{CompositeMode, LoopConfig, MIN_CONTROL_LINES};
use serde_json::{json, Map, Value};

/// The anti-aliased bandwidth a profile may reach: `fs / 2.56`.
pub const BANDWIDTH_RATIO: f64 = 2.56;
/// Accepted FFT lengths.
pub const N_PTS_RANGE: (u64, u64) = (16, 65_536);
/// Accepted averages per control loop.
pub const N_AVG_RANGE: (u64, u64) = (1, 64);
/// Accepted level range, dB relative to the reference.
pub const LEVEL_DB_RANGE: (f64, f64) = (-60.0, 12.0);
/// Longest accepted full-level hold, s.
pub const MAX_FULL_LEVEL_S: f64 = 24.0 * 3600.0;

/// One breakpoint row as sent.
#[derive(Debug, Clone, PartialEq)]
pub struct WireRow {
    pub freq_hz: f64,
    pub psd_g2_hz: Option<f64>,
    pub slope_db_oct: Option<f64>,
    pub alarm_db: Option<(f64, f64)>,
    pub abort_db: Option<(f64, f64)>,
}

/// One limit profile as sent.
#[derive(Debug, Clone, PartialEq)]
pub struct WireLimit {
    pub channel: u32,
    pub force: bool,
    pub rows: Vec<(f64, f64)>,
}

/// Feasibility inputs as sent.
#[derive(Debug, Clone, PartialEq)]
pub struct WireFeasibility {
    pub shaker: hertzforge_shaker_db::ShakerRecord,
    pub moving_mass: hertzforge_shaker_db::MovingMass,
    pub crest_factor: f64,
}

/// The control parameters, defaults applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Control {
    pub composite: CompositeMode,
    pub weights: Vec<(u32, f64)>,
    pub control_dof: f64,
    pub damping: f64,
    pub max_lines_out: i64,
    pub max_loops_out: i64,
    pub max_sigma: Option<f64>,
    pub drive_ceiling_vrms: f64,
    pub drive_abort_vrms: Option<f64>,
    pub coherence_floor: f64,
    pub review_after_sysid: bool,
    pub pretest_averages: i64,
    /// The identification level, dB relative to the reference (the control
    /// response the identification steps its drive up to).
    pub pretest_level_db: f64,
    pub pretest_max_vrms: Option<f64>,
    /// The drive the identification starts from, V rms.
    pub pretest_start_vrms: f64,
    pub pretest_shaped: bool,
    pub soft_start_db: f64,
    pub soft_start_relax_db: f64,
    pub level_ramp_s: f64,
    pub shutdown_ramp_s: f64,
}

impl Default for Control {
    fn default() -> Self {
        Control {
            composite: CompositeMode::WeightedAvg,
            weights: Vec::new(),
            control_dof: 120.0,
            damping: 4.0,
            max_lines_out: 10,
            max_loops_out: 5,
            max_sigma: Some(5.0),
            drive_ceiling_vrms: 10.0,
            drive_abort_vrms: None,
            coherence_floor: 0.5,
            review_after_sysid: true,
            pretest_averages: 4,
            pretest_level_db: -20.0,
            pretest_max_vrms: Some(1.0),
            pretest_start_vrms: 0.01,
            pretest_shaped: false,
            soft_start_db: 9.0,
            soft_start_relax_db: 6.0,
            level_ramp_s: 3.0,
            shutdown_ramp_s: 3.0,
        }
    }
}

/// A parsed Random configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct WireConfig {
    pub title: String,
    pub rows: Vec<WireRow>,
    pub alarm_db: (f64, f64),
    pub abort_db: (f64, f64),
    pub fs_hz: f64,
    pub n_pts: u64,
    pub n_avg: u64,
    pub mode: RunMode,
    pub levels: Vec<(f64, Option<f64>)>,
    pub full_level_s: f64,
    pub control: Control,
    pub limits: Vec<WireLimit>,
    pub feasibility: Option<WireFeasibility>,
    /// The operator's words for the run's record: who ran it, on what, and notes.
    pub operator: String,
    pub article: String,
    pub notes: String,
    /// The document and clause the test is run to, when given.
    pub specification: Option<(String, String)>,
    /// Whether the run's record holds its streams (`record.streams`, default true).
    pub record_streams: bool,
    /// The configuration as accepted, kept with the run.
    pub raw: Value,
}

/// The grid a run uses: requested until it arms, then what the station programmed.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub fs_hz: f64,
    pub n_pts: usize,
    pub df_hz: f64,
    pub lines: usize,
    pub loop_s: f64,
    pub actual: bool,
}

impl Grid {
    /// The grid of a loop configuration.
    pub fn of(loop_cfg: &LoopConfig, actual: bool) -> Grid {
        let (lo, hi) = loop_cfg.band_bins();
        Grid {
            fs_hz: loop_cfg.fs_hz,
            n_pts: loop_cfg.nfft,
            df_hz: loop_cfg.df(),
            lines: hi.saturating_sub(lo),
            loop_s: loop_cfg.segment_seconds(),
            actual,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "fs_hz": num(round(self.fs_hz, 4)),
            "n_pts": self.n_pts,
            "df_hz": num(round(self.df_hz, 6)),
            "lines": self.lines,
            "loop_s": num(round(self.loop_s, 4)),
            "actual": self.actual,
        })
    }
}

/// The derived values of a valid configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub grms_g: f64,
    pub velocity_rms_ips: f64,
    pub displacement_rms_mil: f64,
    pub band_hz: (f64, f64),
    pub grid: Grid,
    pub control_channels: Vec<u32>,
}

impl Derived {
    pub fn to_json(&self) -> Value {
        json!({
            "grms_g": num(round(self.grms_g, 3)),
            "velocity_rms_ips": num(round(self.velocity_rms_ips, 3)),
            "displacement_rms_mil": num(round(self.displacement_rms_mil, 2)),
            "band_hz": [num(self.band_hz.0), num(self.band_hz.1)],
            "grid": self.grid.to_json(),
            "control_channels": self.control_channels,
        })
    }
}

/// What the controller is built from.
pub struct Built {
    pub test: RandomTestConfig,
    pub loop_cfg: LoopConfig,
}

pub fn round(x: f64, digits: i32) -> f64 {
    let p = 10f64.powi(digits);
    (x * p).round() / p
}

// ---------------------------------------------------------------------------------
// Parsing: types and structure. Content problems are items, not parse errors.
// ---------------------------------------------------------------------------------

struct Obj<'a> {
    m: &'a Map<String, Value>,
    path: String,
}

impl<'a> Obj<'a> {
    fn of(v: &'a Value, path: &str) -> Result<Obj<'a>, MethodError> {
        v.as_object()
            .map(|m| Obj {
                m,
                path: path.to_owned(),
            })
            .ok_or_else(|| MethodError::bad_params(path, format!("\"{path}\" must be an object")))
    }

    fn at(&self, k: &str) -> String {
        format!("{}.{k}", self.path)
    }

    fn get(&self, k: &str) -> Option<&'a Value> {
        self.m.get(k)
    }

    fn require(&self, k: &str) -> Result<&'a Value, MethodError> {
        self.m.get(k).ok_or_else(|| {
            MethodError::bad_params(
                &self.at(k),
                format!("the configuration has no \"{}\"", self.at(k)),
            )
        })
    }

    fn obj(&self, k: &str) -> Result<Obj<'a>, MethodError> {
        Obj::of(self.require(k)?, &self.at(k))
    }

    fn opt_f64(&self, k: &str) -> Result<Option<f64>, MethodError> {
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

    fn f64(&self, k: &str) -> Result<f64, MethodError> {
        self.opt_f64(k)?.ok_or_else(|| {
            MethodError::bad_params(&self.at(k), format!("\"{}\" must be a number", self.at(k)))
        })
    }

    /// Absent keeps `default`; null is `None`; a number is `Some`.
    fn nullable_f64(&self, k: &str, default: Option<f64>) -> Result<Option<f64>, MethodError> {
        match self.m.get(k) {
            None => Ok(default),
            Some(Value::Null) => Ok(None),
            Some(_) => self.opt_f64(k),
        }
    }

    fn opt_int(&self, k: &str) -> Result<Option<i64>, MethodError> {
        match self.m.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_i64()
                .or_else(|| {
                    v.as_f64()
                        .filter(|x| x.fract() == 0.0 && x.abs() < 9e15)
                        .map(|x| x as i64)
                })
                .map(Some)
                .ok_or_else(|| {
                    MethodError::bad_params(
                        &self.at(k),
                        format!("\"{}\" must be a whole number", self.at(k)),
                    )
                }),
        }
    }

    fn opt_bool(&self, k: &str) -> Result<Option<bool>, MethodError> {
        match self.m.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(MethodError::bad_params(
                &self.at(k),
                format!("\"{}\" must be true or false", self.at(k)),
            )),
        }
    }

    fn opt_str(&self, k: &str) -> Result<Option<&'a str>, MethodError> {
        match self.m.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(MethodError::bad_params(
                &self.at(k),
                format!("\"{}\" must be a string", self.at(k)),
            )),
        }
    }

    fn array(&self, k: &str) -> Result<Option<&'a Vec<Value>>, MethodError> {
        match self.m.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(a)) => Ok(Some(a)),
            Some(_) => Err(MethodError::bad_params(
                &self.at(k),
                format!("\"{}\" must be a list", self.at(k)),
            )),
        }
    }

    fn band(&self, k: &str) -> Result<Option<(f64, f64)>, MethodError> {
        let Some(a) = self.array(k)? else {
            return Ok(None);
        };
        let bad = || {
            MethodError::bad_params(
                &self.at(k),
                format!("\"{}\" is [below, above] in positive dB", self.at(k)),
            )
        };
        if a.len() != 2 {
            return Err(bad());
        }
        let below = a[0]
            .as_f64()
            .filter(|x| x.is_finite() && *x >= 0.0)
            .ok_or_else(bad)?;
        let above = a[1]
            .as_f64()
            .filter(|x| x.is_finite() && *x >= 0.0)
            .ok_or_else(bad)?;
        Ok(Some((below, above)))
    }
}

/// Parse `params.config`.
pub fn parse(config: &Value) -> Result<WireConfig, MethodError> {
    let c = Obj::of(config, "config")?;
    let title = c.opt_str("title")?.unwrap_or("").to_owned();

    let profile = c.obj("profile")?;
    let rows_v = profile.array("breakpoints")?.ok_or_else(|| {
        MethodError::bad_params(
            "config.profile.breakpoints",
            "the profile has no \"breakpoints\" list",
        )
    })?;
    let mut rows = Vec::with_capacity(rows_v.len());
    for (i, r) in rows_v.iter().enumerate() {
        let o = Obj::of(r, &format!("config.profile.breakpoints[{i}]"))?;
        rows.push(WireRow {
            freq_hz: o.f64("freq_hz")?,
            psd_g2_hz: o.opt_f64("psd_g2_hz")?,
            slope_db_oct: o.opt_f64("slope_db_oct")?,
            alarm_db: o.band("alarm_db")?,
            abort_db: o.band("abort_db")?,
        });
    }
    let alarm_db = profile.band("alarm_db")?.unwrap_or((3.0, 3.0));
    let abort_db = profile.band("abort_db")?.unwrap_or((6.0, 6.0));

    let acq = c.obj("acquisition")?;
    let fs_hz = acq.f64("fs_hz")?;
    let positive_int = |o: &Obj<'_>, k: &str| -> Result<u64, MethodError> {
        o.opt_int(k)?
            .and_then(|n| u64::try_from(n).ok())
            .filter(|n| *n >= 1)
            .ok_or_else(|| {
                MethodError::bad_params(
                    &o.at(k),
                    format!("\"{}\" must be a whole number of at least 1", o.at(k)),
                )
            })
    };
    let n_pts = positive_int(&acq, "n_pts")?;
    let n_avg = positive_int(&acq, "n_avg")?;

    let sch = c.obj("schedule")?;
    let mode = match sch.opt_str("mode")? {
        Some("auto") => RunMode::Auto,
        Some("manual") => RunMode::Manual,
        _ => {
            return Err(MethodError::bad_params(
                "config.schedule.mode",
                "\"config.schedule.mode\" is \"auto\" or \"manual\"",
            ))
        }
    };
    let levels_v = sch.array("levels")?.ok_or_else(|| {
        MethodError::bad_params(
            "config.schedule.levels",
            "the schedule has no \"levels\" list",
        )
    })?;
    let mut levels = Vec::with_capacity(levels_v.len());
    for (i, l) in levels_v.iter().enumerate() {
        let o = Obj::of(l, &format!("config.schedule.levels[{i}]"))?;
        levels.push((o.f64("db")?, o.opt_f64("dwell_s")?));
    }
    let full_level_s = sch.f64("full_level_s")?;

    let mut control = Control::default();
    if let Some(v) = c.get("control").filter(|v| !v.is_null()) {
        let o = Obj::of(v, "config.control")?;
        if let Some(s) = o.opt_str("composite")? {
            control.composite = match s {
                "weighted_avg" => CompositeMode::WeightedAvg,
                "min" => CompositeMode::Min,
                "max" => CompositeMode::Max,
                _ => {
                    return Err(MethodError::bad_params(
                        "config.control.composite",
                        "\"config.control.composite\" is weighted_avg, min or max",
                    ))
                }
            };
        }
        if let Some(ws) = o.array("weights")? {
            for (i, w) in ws.iter().enumerate() {
                let wo = Obj::of(w, &format!("config.control.weights[{i}]"))?;
                let ch = wo
                    .opt_int("channel")?
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| {
                        MethodError::bad_params(&wo.at("channel"), "a weight names a channel index")
                    })?;
                control.weights.push((ch, wo.f64("weight")?));
            }
        }
        let set = |slot: &mut f64, k: &str| -> Result<(), MethodError> {
            if let Some(x) = o.opt_f64(k)? {
                *slot = x;
            }
            Ok(())
        };
        set(&mut control.control_dof, "control_dof")?;
        set(&mut control.damping, "damping")?;
        set(&mut control.drive_ceiling_vrms, "drive_ceiling_vrms")?;
        set(&mut control.coherence_floor, "coherence_floor")?;
        set(&mut control.soft_start_db, "soft_start_db")?;
        set(&mut control.soft_start_relax_db, "soft_start_relax_db")?;
        set(&mut control.level_ramp_s, "level_ramp_s")?;
        set(&mut control.shutdown_ramp_s, "shutdown_ramp_s")?;
        if let Some(n) = o.opt_int("max_lines_out")? {
            control.max_lines_out = n;
        }
        if let Some(n) = o.opt_int("max_loops_out")? {
            control.max_loops_out = n;
        }
        control.max_sigma = o.nullable_f64("max_sigma", control.max_sigma)?;
        control.drive_abort_vrms = o.nullable_f64("drive_abort_vrms", control.drive_abort_vrms)?;
        if let Some(b) = o.opt_bool("review_after_sysid")? {
            control.review_after_sysid = b;
        }
        if let Some(p) = o.get("pretest").filter(|v| !v.is_null()) {
            let po = Obj::of(p, "config.control.pretest")?;
            if let Some(n) = po.opt_int("averages")? {
                control.pretest_averages = n;
            }
            if let Some(x) = po.opt_f64("level_db")? {
                control.pretest_level_db = x;
            }
            control.pretest_max_vrms = po.nullable_f64("max_vrms", control.pretest_max_vrms)?;
            if let Some(x) = po.opt_f64("start_vrms")? {
                control.pretest_start_vrms = x;
            }
            if let Some(b) = po.opt_bool("shaped")? {
                control.pretest_shaped = b;
            }
        }
    }

    let mut limits = Vec::new();
    if let Some(ls) = c.array("limits")? {
        for (i, l) in ls.iter().enumerate() {
            let o = Obj::of(l, &format!("config.limits[{i}]"))?;
            let channel = o
                .opt_int("channel")?
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| {
                    MethodError::bad_params(&o.at("channel"), "a limit names a channel index")
                })?;
            let force = match o.opt_str("kind")? {
                Some("response") => false,
                Some("force") => true,
                _ => {
                    return Err(MethodError::bad_params(
                        &o.at("kind"),
                        "a limit's kind is response or force",
                    ))
                }
            };
            let rows_v = o.array("rows")?.ok_or_else(|| {
                MethodError::bad_params(&o.at("rows"), "a limit has a \"rows\" list")
            })?;
            let mut rows = Vec::new();
            for (k, r) in rows_v.iter().enumerate() {
                let ro = Obj::of(r, &format!("{}.rows[{k}]", o.path))?;
                rows.push((ro.f64("freq_hz")?, ro.f64("ceiling")?));
            }
            limits.push(WireLimit {
                channel,
                force,
                rows,
            });
        }
    }

    let feasibility = match c.get("feasibility") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let o = Obj::of(v, "config.feasibility")?;
            let shaker_v = o.require("shaker")?;
            let shaker: hertzforge_shaker_db::ShakerRecord = serde_json::from_value(shaker_v.clone())
                .map_err(|e| {
                    MethodError::named(
                        ErrorCode::BadRequest,
                        "shaker_record_invalid",
                        format!("The feasibility shaker record does not parse ({e}), so the profile cannot be checked against it."),
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
            let moving_mass = hertzforge_shaker_db::MovingMass {
                armature_lb: mass("armature")?,
                slip_table_lb: mass("slip_table")?,
                drive_adapter_lb: mass("drive_adapter")?,
                fixturing_lb: mass("fixturing")?,
                test_article_lb: mass("test_article")?,
            };
            let crest_factor = o
                .opt_f64("crest_factor")?
                .unwrap_or(hertzforge_shaker_db::DEFAULT_CREST_FACTOR);
            if crest_factor <= 0.0 {
                return Err(MethodError::bad_params(
                    "config.feasibility.crest_factor",
                    "the crest factor is above 0",
                ));
            }
            Some(WireFeasibility {
                shaker,
                moving_mass,
                crest_factor,
            })
        }
    };

    let words =
        |k: &str| -> Result<String, MethodError> { Ok(c.opt_str(k)?.unwrap_or("").to_owned()) };
    let (operator, article, notes) = (words("operator")?, words("article")?, words("notes")?);
    let specification = match c.get("specification") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let o = Obj::of(v, "config.specification")?;
            Some((
                o.opt_str("document")?.unwrap_or("").to_owned(),
                o.opt_str("clause")?.unwrap_or("").to_owned(),
            ))
        }
    };
    let record_streams = match c.get("record") {
        None | Some(Value::Null) => true,
        Some(v) => Obj::of(v, "config.record")?
            .opt_bool("streams")?
            .unwrap_or(true),
    };

    Ok(WireConfig {
        title,
        rows,
        alarm_db,
        abort_db,
        fs_hz,
        n_pts,
        n_avg,
        mode,
        levels,
        full_level_s,
        control,
        limits,
        feasibility,
        operator,
        article,
        notes,
        specification,
        record_streams,
        raw: config.clone(),
    })
}

// ---------------------------------------------------------------------------------
// Checking: every problem is a named item.
// ---------------------------------------------------------------------------------

impl WireConfig {
    /// The breakpoint table, each row carrying its own bands or the profile's.
    pub fn breakpoints(&self) -> BreakpointTable {
        let mut bt = BreakpointTable::new(self.title.clone());
        for r in &self.rows {
            bt.rows.push(BreakpointRow {
                freq_hz: r.freq_hz,
                level: match (r.psd_g2_hz, r.slope_db_oct) {
                    (Some(p), _) => LevelSpec::Psd(p),
                    (None, Some(s)) => LevelSpec::SlopeDbPerOct(s),
                    (None, None) => LevelSpec::Psd(0.0),
                },
                alarm_db: Some(r.alarm_db.unwrap_or(self.alarm_db)),
                abort_db: Some(r.abort_db.unwrap_or(self.abort_db)),
            });
        }
        bt
    }

    /// The enabled control rows of `table`, in table order.
    pub fn control_channels(table: &ChannelTable) -> Vec<u32> {
        table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Control)
            .map(|c| c.index)
            .collect()
    }

    /// The loop the configuration asks for.
    pub fn loop_config(&self) -> LoopConfig {
        let mut l = LoopConfig::demo_default();
        l.fs_hz = self.fs_hz;
        l.nfft = self.n_pts as usize;
        l.blocks_per_window = self.n_avg as usize;
        if let (Some(first), Some(last)) = (self.rows.first(), self.rows.last()) {
            l.band_lo_hz = first.freq_hz.max(l.df());
            l.band_hi_hz = last.freq_hz;
        }
        l
    }

    /// Check against `table`. Returns the report and, when it has no errors, the
    /// derived values.
    pub fn check(&self, table: &ChannelTable) -> (Report, Option<Derived>) {
        let mut r = Report::default();
        self.check_profile(&mut r);
        self.check_schedule(&mut r);
        self.check_control(&mut r, table);
        self.check_limits(&mut r, table);
        let n_pts_ok = self.check_acquisition(&mut r);
        if !r.errors.is_empty() || !n_pts_ok {
            return (r, None);
        }
        let band = (self.rows[0].freq_hz, self.rows[self.rows.len() - 1].freq_hz);
        let loop_cfg = self.loop_config();
        let grid = Grid::of(&loop_cfg, false);
        check_grid(&mut r, band, &grid);
        if !r.errors.is_empty() {
            return (r, None);
        }
        let bt = self.breakpoints();
        let derived = Derived {
            grms_g: bt.grms_of_table().unwrap_or(0.0),
            velocity_rms_ips: bt.velocity_rms_ips().unwrap_or(0.0),
            displacement_rms_mil: bt.displacement_rms_mil().unwrap_or(0.0),
            band_hz: band,
            grid,
            control_channels: Self::control_channels(table),
        };
        (r, Some(derived))
    }

    fn check_profile(&self, r: &mut Report) {
        for (i, row) in self.rows.iter().enumerate() {
            if row.psd_g2_hz.is_some() == row.slope_db_oct.is_some() {
                r.errors.push(Item::row(
                    "row_level_ambiguous",
                    i,
                    format!(
                        "row {i}: give a PSD or a slope, not {}",
                        if row.psd_g2_hz.is_some() {
                            "both"
                        } else {
                            "neither"
                        }
                    ),
                ));
            }
        }
        let report = self.breakpoints().validate_report();
        for e in report.errors {
            let message = e.to_string();
            r.errors.push(match e {
                BreakpointError::Empty => {
                    Item::field("profile_empty", "profile.breakpoints", message)
                }
                BreakpointError::FirstRowNotAnchor => Item::row("first_row_not_anchor", 0, message),
                BreakpointError::NonPositiveFreq { row, .. } => {
                    Item::row("frequency_not_positive", row, message)
                }
                BreakpointError::NonMonotonicFreq { row, .. } => {
                    Item::row("frequency_not_ascending", row, message)
                }
                BreakpointError::NegativePsd { row, .. } => Item::row("psd_negative", row, message),
                BreakpointError::Parse(_) => {
                    Item::field("profile_empty", "profile.breakpoints", message)
                }
            });
        }
        for w in report.warnings {
            if let BreakpointWarning::AlarmWiderThanAbort(row) = w {
                r.warnings
                    .push(Item::row("alarm_wider_than_abort", row, w.to_string()));
            }
        }
    }

    fn check_schedule(&self, r: &mut Report) {
        if self.levels.is_empty() {
            r.errors.push(Item::field(
                "levels_empty",
                "schedule.levels",
                "the level schedule is empty",
            ));
        }
        for (i, w) in self.levels.windows(2).enumerate() {
            if w[1].0 < w[0].0 {
                r.errors.push(Item::field(
                    "levels_not_ascending",
                    format!("schedule.levels.{}", i + 1),
                    format!(
                        "level {} ({} dB) is below the level before it",
                        i + 1,
                        w[1].0
                    ),
                ));
            }
        }
        let last = self.levels.len().saturating_sub(1);
        for (i, (db, dwell)) in self.levels.iter().enumerate() {
            range(
                r,
                &format!("schedule.levels.{i}.db"),
                Some(*db),
                |v| (LEVEL_DB_RANGE.0..=LEVEL_DB_RANGE.1).contains(&v),
                &format!(
                    "it runs from {} to +{} dB",
                    LEVEL_DB_RANGE.0, LEVEL_DB_RANGE.1
                ),
            );
            if i < last && dwell.is_none() {
                r.errors.push(Item::field(
                    "dwell_missing",
                    format!("schedule.levels.{i}"),
                    format!(
                        "level {i} ({db} dB) has no dwell_s; every level but the last needs one"
                    ),
                ));
            }
            range(
                r,
                &format!("schedule.levels.{i}.dwell_s"),
                *dwell,
                |v| v >= 0.0,
                "it must not be negative",
            );
        }
        range(
            r,
            "schedule.full_level_s",
            Some(self.full_level_s),
            |v| (0.0..=MAX_FULL_LEVEL_S).contains(&v),
            &format!("it runs from 0 to {MAX_FULL_LEVEL_S} s"),
        );
    }

    fn check_control(&self, r: &mut Report, table: &ChannelTable) {
        let c = &self.control;
        range(
            r,
            "control.damping",
            Some(c.damping),
            |v| (0.0..=10.0).contains(&v),
            "it runs from 0 to 10",
        );
        range(
            r,
            "control.coherence_floor",
            Some(c.coherence_floor),
            |v| (0.0..=1.0).contains(&v),
            "it runs from 0 to 1",
        );
        range(
            r,
            "control.drive_ceiling_vrms",
            Some(c.drive_ceiling_vrms),
            |v| v > 0.0,
            "it must be above 0",
        );
        range(
            r,
            "control.drive_abort_vrms",
            c.drive_abort_vrms,
            |v| v > 0.0,
            "it must be above 0, or null for none",
        );
        range(
            r,
            "control.max_loops_out",
            Some(c.max_loops_out as f64),
            |v| v >= 1.0,
            "it must be a whole number of at least 1",
        );
        range(
            r,
            "control.max_lines_out",
            Some(c.max_lines_out as f64),
            |v| v >= 0.0,
            "it must be a whole number of at least 0",
        );
        range(
            r,
            "control.control_dof",
            Some(c.control_dof),
            |v| v >= 2.0,
            "it must be at least 2",
        );
        range(
            r,
            "control.max_sigma",
            c.max_sigma,
            |v| v >= 1.0,
            "it must be at least 1, or null for no clip",
        );
        range(
            r,
            "control.pretest.averages",
            Some(c.pretest_averages as f64),
            |v| (1.0..=64.0).contains(&v),
            "it runs from 1 to 64",
        );
        range(
            r,
            "control.pretest.max_vrms",
            c.pretest_max_vrms,
            |v| v > 0.0,
            "it must be above 0, or null for no ceiling",
        );
        range(
            r,
            "control.pretest.level_db",
            Some(c.pretest_level_db),
            |v| (-60.0..=0.0).contains(&v),
            "it runs from -60 to 0 dB",
        );
        range(
            r,
            "control.pretest.start_vrms",
            Some(c.pretest_start_vrms),
            |v| v > 0.0,
            "it must be above 0",
        );
        if let Some(mx) = c.pretest_max_vrms.filter(|m| *m > 0.0) {
            range(
                r,
                "control.pretest.start_vrms",
                Some(c.pretest_start_vrms),
                |v| v <= 0.0 || v <= mx,
                &format!(
                    "it must not exceed the largest identification drive, {} V rms",
                    num(mx)
                ),
            );
        }
        for (k, v) in [
            ("soft_start_db", c.soft_start_db),
            ("soft_start_relax_db", c.soft_start_relax_db),
            ("level_ramp_s", c.level_ramp_s),
            ("shutdown_ramp_s", c.shutdown_ramp_s),
        ] {
            range(
                r,
                &format!("control.{k}"),
                Some(v),
                |x| x >= 0.0,
                "it must not be negative",
            );
        }
        let controls = Self::control_channels(table);
        if controls.is_empty() {
            r.errors.push(Item::field(
                "no_control_channel",
                "control",
                "the channel table has no enabled control channel",
            ));
        }
        for (ch, _) in &c.weights {
            if !controls.contains(ch) {
                r.errors.push(Item::channel(
                    "weight_unknown_channel",
                    Some(*ch),
                    format!("a weight names channel {ch}, which is not an enabled control channel"),
                ));
            }
        }
        let sum: f64 = controls.iter().map(|i| self.weight_of(*i)).sum();
        if !controls.is_empty() && sum.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            r.errors.push(Item::field(
                "weights_sum_not_positive",
                "control.weights",
                "the control weights sum to zero or less",
            ));
        }
    }

    fn check_limits(&self, r: &mut Report, table: &ChannelTable) {
        for (i, l) in self.limits.iter().enumerate() {
            let known = table
                .channels
                .iter()
                .any(|c| c.enabled && c.role != ChannelRole::Drive && c.index == l.channel);
            if !known {
                r.errors.push(Item::channel(
                    "limit_unknown_channel",
                    Some(l.channel),
                    format!(
                        "limit {i} names channel {}, which is not an enabled input",
                        l.channel
                    ),
                ));
            }
            for (k, w) in l.rows.windows(2).enumerate() {
                if w[1].0 <= w[0].0 {
                    r.errors.push(Item::field(
                        "limit_rows_not_ascending",
                        format!("limits.{i}.rows.{}", k + 1),
                        format!("limit {i}: row {} is not above the row before", k + 1),
                    ));
                    break;
                }
            }
            for (k, (f, ceiling)) in l.rows.iter().enumerate() {
                range(
                    r,
                    &format!("limits.{i}.rows.{k}.freq_hz"),
                    Some(*f),
                    |v| v > 0.0,
                    "it must be above 0 Hz",
                );
                range(
                    r,
                    &format!("limits.{i}.rows.{k}.ceiling"),
                    Some(*ceiling),
                    |v| v > 0.0,
                    "a ceiling is above 0",
                );
            }
        }
    }

    /// Check the acquisition; false when the grid cannot be built at all.
    fn check_acquisition(&self, r: &mut Report) -> bool {
        let before = r.errors.len();
        range(
            r,
            "acquisition.fs_hz",
            Some(self.fs_hz),
            |v| v > 0.0 && v <= 1.0e6,
            "it runs above 0 to 1 MHz",
        );
        if !self.n_pts.is_power_of_two() {
            r.errors.push(Item::field(
                "n_pts_not_power_of_two",
                "acquisition.n_pts",
                format!("the FFT length {} is not a power of two", self.n_pts),
            ));
        }
        range(
            r,
            "acquisition.n_pts",
            Some(self.n_pts as f64),
            |v| (N_PTS_RANGE.0 as f64..=N_PTS_RANGE.1 as f64).contains(&v),
            &format!("it runs from {} to {}", N_PTS_RANGE.0, N_PTS_RANGE.1),
        );
        range(
            r,
            "acquisition.n_avg",
            Some(self.n_avg as f64),
            |v| (N_AVG_RANGE.0 as f64..=N_AVG_RANGE.1 as f64).contains(&v),
            &format!("it runs from {} to {}", N_AVG_RANGE.0, N_AVG_RANGE.1),
        );
        r.errors.len() == before
    }

    /// A control row's weight (1 unless given).
    pub fn weight_of(&self, channel: u32) -> f64 {
        self.control
            .weights
            .iter()
            .find(|(c, _)| *c == channel)
            .map_or(1.0, |(_, w)| *w)
    }

    /// Build the controller's configuration for `table`. Call after [`Self::check`]
    /// found no errors.
    pub fn build(&self, table: &ChannelTable) -> Result<Built, MethodError> {
        let controls = Self::control_channels(table);
        let first = *controls.first().ok_or_else(|| {
            MethodError::named(ErrorCode::BadRequest, "config_invalid", "the channel table has no enabled control channel")
                .with_detail(json!({"errors": [Item::field("no_control_channel", "control", "the channel table has no enabled control channel").to_json()], "warnings": []}))
        })?;
        let loop_cfg = self.loop_config();
        let mut t = RandomTestConfig::siso(self.breakpoints(), first);
        let c = &self.control;
        t.control.channels = controls
            .iter()
            .map(|&i| ControlChannel {
                channel_index: i,
                weight: self.weight_of(i),
            })
            .collect();
        t.control.mode = c.composite;
        t.alarm_db = self.alarm_db;
        t.abort_db = self.abort_db;
        t.df_hz = loop_cfg.df();
        t.lines = Grid::of(&loop_cfg, false).lines;
        let table_db: Vec<f64> = self.levels.iter().map(|l| l.0).collect();
        let last = self.levels.len().saturating_sub(1);
        t.schedule = LevelSchedule {
            mode: self.mode,
            start_db: table_db.first().copied().unwrap_or(0.0),
            dwell_s: self
                .levels
                .iter()
                .enumerate()
                .map(|(i, l)| if i < last { l.1.unwrap_or(0.0) } else { 0.0 })
                .collect(),
            table_db,
        };
        t.full_level_duration_s = self.full_level_s;
        t.drive_ceiling_vrms = c.drive_ceiling_vrms;
        t.drive_abort_vrms = c.drive_abort_vrms;
        t.hinv.damping = c.damping;
        t.divergence_loops = c.max_loops_out.max(1) as u32;
        t.max_lines_out = c.max_lines_out.max(0) as usize;
        t.coherence_floor = c.coherence_floor;
        t.drive_clip_sigma = c.max_sigma;
        t.level_ramp_s = c.level_ramp_s;
        t.shutdown_ramp_s = c.shutdown_ramp_s;
        // The identification excites relative to the reference (2026-11-01, bench
        // finding 1): it steps its drive from `start_vrms` until the control
        // response is `level_db` below the reference, never above `max_vrms`.
        t.pretest_reference_db = Some(c.pretest_level_db);
        t.pretest_start_vrms = Some(c.pretest_start_vrms);
        t.pretest_max_vrms = c.pretest_max_vrms;
        t.pretest_min_vrms = None;
        t.shaped_pretest = c.pretest_shaped;
        t.soft_start_db = c.soft_start_db;
        t.soft_start_relax_db = c.soft_start_relax_db;
        t.control_dof = c.control_dof;
        // The operator's words travel with the controller's configuration too, so its
        // own report carries them.
        for field in t.metadata.iter_mut() {
            match field.key.as_str() {
                "operator" => field.value = self.operator.clone(),
                "test_article" => field.value = self.article.clone(),
                _ => {}
            }
        }
        if !self.notes.is_empty() {
            t.metadata
                .push(MetaField::new("notes", "Notes", self.notes.clone()));
        }
        if let Some((document, clause)) = &self.specification {
            t.metadata.push(MetaField::new(
                "specification",
                "Specification",
                document.clone(),
            ));
            t.metadata
                .push(MetaField::new("clause", "Clause", clause.clone()));
        }
        if let Some(f) = &self.feasibility {
            t.shaker = Some(f.shaker.clone());
            t.moving_mass = f.moving_mass;
            t.crest_factor = f.crest_factor;
        }
        t.limit_profiles = self
            .limits
            .iter()
            .map(|l| LimitProfile {
                channel_index: l.channel,
                kind: if l.force {
                    LimitKind::Force
                } else {
                    LimitKind::Response
                },
                resp_ceiling: ceiling_on_grid(&l.rows, loop_cfg.nbins(), loop_cfg.df()),
            })
            .collect();
        // The controller's own validator has the last word; a refusal here is a
        // configuration the checks above let through, named as such.
        t.validate().map_err(|e| {
            let item = Item::field("out_of_range", "control", e.to_string());
            MethodError::named(
                ErrorCode::BadRequest,
                "config_invalid",
                format!("The configuration has an error: {e}"),
            )
            .with_detail(json!({"errors": [item.to_json()], "warnings": []}))
        })?;
        Ok(Built { test: t, loop_cfg })
    }
}

/// The grid checks, at the requested rate (configure) or the rate the station
/// programmed (arm).
pub fn check_grid(r: &mut Report, band: (f64, f64), grid: &Grid) {
    let top = grid.fs_hz / BANDWIDTH_RATIO;
    // The controller's own limit too: the top usable line of the analysis grid.
    let top_line = (grid.n_pts / 2).saturating_sub(1) as f64 * grid.df_hz;
    let reach = top.min(top_line);
    if band.1 > reach + 1e-9 {
        r.errors.push(Item::field(
            "band_above_grid",
            "acquisition.fs_hz",
            format!(
                "the profile reaches {} Hz but the grid at {} Hz resolves only to {:.1} Hz; raise \
                 fs_hz or lower the top breakpoint",
                band.1,
                round(grid.fs_hz, 2),
                reach
            ),
        ));
    }
    if grid.lines < MIN_CONTROL_LINES {
        r.errors.push(Item::field(
            "too_few_lines",
            "acquisition.n_pts",
            format!(
                "{} lines fall in {} to {} Hz at {} Hz spacing; at least {MIN_CONTROL_LINES} are \
                 needed",
                grid.lines,
                band.0,
                band.1,
                round(grid.df_hz, 4)
            ),
        ));
    }
}

fn range(r: &mut Report, field: &str, value: Option<f64>, ok: impl Fn(f64) -> bool, text: &str) {
    if let Some(v) = value {
        if !ok(v) {
            r.errors.push(Item::field(
                "out_of_range",
                field,
                format!("{} is {}; {text}", field_words(field), num(v)),
            ));
        }
    }
}

/// A configuration field in words, for a message a person reads.
pub fn field_words(field: &str) -> String {
    let last = field.rsplit('.').next().unwrap_or(field);
    let words = match last {
        "fs_hz" => "The sample rate",
        "n_pts" => "The FFT length",
        "n_avg" => "The averages per loop",
        "full_level_s" => "The time at full level",
        "dwell_s" => "The dwell",
        "db" => "The level",
        "damping" => "Damping",
        "coherence_floor" => "The coherence floor",
        "drive_ceiling_vrms" => "The drive ceiling",
        "drive_abort_vrms" => "The drive abort",
        "max_loops_out" => "The diverging loops before an abort",
        "max_lines_out" => "The lines out before an abort",
        "control_dof" => "The control degrees of freedom",
        "level_db" => "The identification level",
        "max_sigma" => "The drive clip",
        "averages" => "The identification averages",
        "max_vrms" => "The identification drive ceiling",
        "start_vrms" => "The identification starting drive",
        "soft_start_db" => "The soft start depth",
        "soft_start_relax_db" => "The soft start band widening",
        "level_ramp_s" => "The level ramp",
        "shutdown_ramp_s" => "The shutdown ramp",
        "freq_hz" => "The frequency",
        "ceiling" => "The ceiling",
        other => return other.replace('_', " "),
    };
    let level = field
        .strip_prefix("schedule.levels.")
        .and_then(|rest| rest.split('.').next());
    match level {
        Some(n) => format!("{words} of level {n}"),
        None => words.to_owned(),
    }
}

/// A limit's rows on the run's line grid, log-log between rows; unconstrained
/// outside the authored band; a single row is a flat ceiling. The same rule the
/// controller's daemon applies when it receives limit rows.
pub fn ceiling_on_grid(rows: &[(f64, f64)], nbins: usize, df_hz: f64) -> Vec<f64> {
    let mut pts: Vec<(f64, f64)> = rows
        .iter()
        .copied()
        .filter(|(f, c)| f.is_finite() && *f > 0.0 && c.is_finite() && *c > 0.0)
        .collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = vec![f64::INFINITY; nbins];
    match pts.len() {
        0 => return out,
        1 => {
            out.fill(pts[0].1);
            return out;
        }
        _ => {}
    }
    let (f_lo, f_hi) = (pts[0].0, pts[pts.len() - 1].0);
    let mut seg = 0usize;
    for (k, o) in out.iter_mut().enumerate() {
        let f = k as f64 * df_hz;
        if f < f_lo || f > f_hi {
            continue;
        }
        while seg + 2 < pts.len() && f > pts[seg + 1].0 {
            seg += 1;
        }
        let ((fa, ca), (fb, cb)) = (pts[seg], pts[seg + 1]);
        *o = if f <= fa {
            ca
        } else if f >= fb {
            cb
        } else {
            let t = (f / fa).ln() / (fb / fa).ln();
            (ca.ln() + t * (cb / ca).ln()).exp()
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::Table;

    fn golden_config() -> Value {
        let g: Value = serde_json::from_str(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../schema/golden/request_random_configure.json"
            ))
            .unwrap(),
        )
        .unwrap();
        g["params"]["config"].clone()
    }

    fn table_with_monitor() -> ChannelTable {
        let mut t = Table::new().table;
        let mut m = hertzforge_channel_table::Channel::new(3);
        m.description = "Fixture".into();
        t.channels.insert(1, m);
        t.channels[1].index = 2;
        t.channels[2].index = 3;
        t
    }

    fn reasons(items: &[Item]) -> Vec<&'static str> {
        items.iter().map(|i| i.reason).collect()
    }

    #[test]
    fn the_golden_configuration_derives_the_golden_values() {
        let cfg = parse(&golden_config()).unwrap();
        let (report, derived) = cfg.check(&table_with_monitor());
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let d = derived.unwrap();
        let golden: Value = serde_json::from_str(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../schema/golden/response_random_configure.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let want = &golden["result"]["derived"];
        let got = d.to_json();
        assert_eq!(got["grid"], want["grid"], "the grid is the controller's");
        assert_eq!(got["band_hz"], want["band_hz"]);
        assert_eq!(got["control_channels"], want["control_channels"]);
        let g = got["grms_g"].as_f64().unwrap();
        assert!(
            (g - want["grms_g"].as_f64().unwrap()).abs() < 0.01,
            "grms {g}"
        );
        // The golden's velocity and displacement are illustrative; check the
        // controller's analytic values against an independent integration of the
        // same profile on a fine log grid instead.
        let (v, dsp) = integrate_kinematics(&cfg.breakpoints());
        let gv = got["velocity_rms_ips"].as_f64().unwrap();
        let gd = got["displacement_rms_mil"].as_f64().unwrap();
        assert!((gv - v).abs() / v < 0.005, "velocity {gv} against {v}");
        assert!(
            (gd - dsp).abs() / dsp < 0.005,
            "displacement {gd} against {dsp}"
        );
    }

    /// Velocity (in/s) and displacement (mil) rms of a g²/Hz profile by brute-force
    /// integration on a log grid.
    fn integrate_kinematics(bt: &BreakpointTable) -> (f64, f64) {
        let a = bt.resolve().unwrap();
        let (f0, f1) = (a[0].freq_hz, a[a.len() - 1].freq_hz);
        let psd = |f: f64| -> f64 {
            for w in a.windows(2) {
                if f >= w[0].freq_hz && f <= w[1].freq_hz {
                    let b = (w[1].level_g2_hz / w[0].level_g2_hz).ln()
                        / (w[1].freq_hz / w[0].freq_hz).ln();
                    return w[0].level_g2_hz * (f / w[0].freq_hz).powf(b);
                }
            }
            0.0
        };
        let n = 100_000;
        let (mut v, mut d) = (0.0, 0.0);
        let g_in = 386.088_582_7;
        for i in 0..n {
            let fa = f0 * (f1 / f0).powf(i as f64 / n as f64);
            let fb = f0 * (f1 / f0).powf((i + 1) as f64 / n as f64);
            let f = (fa * fb).sqrt();
            let p = psd(f) * (fb - fa);
            let w = std::f64::consts::TAU * f;
            v += p * g_in * g_in / (w * w);
            d += p * g_in * g_in / (w * w * w * w);
        }
        (v.sqrt(), d.sqrt() * 1000.0)
    }

    #[test]
    fn the_profile_errors_are_named_and_located() {
        let mut v = golden_config();
        v["profile"]["breakpoints"] = json!([
            {"freq_hz": 20, "slope_db_oct": 3},
            {"freq_hz": 10, "psd_g2_hz": -0.1, "slope_db_oct": 1},
            {"freq_hz": 0, "psd_g2_hz": 0.01, "alarm_db": [8, 8]},
        ]);
        let cfg = parse(&v).unwrap();
        let (r, d) = cfg.check(&table_with_monitor());
        assert!(d.is_none());
        let got = reasons(&r.errors);
        for want in [
            "row_level_ambiguous",
            "first_row_not_anchor",
            "frequency_not_ascending",
            "frequency_not_positive",
            "psd_negative",
        ] {
            assert!(got.contains(&want), "{want} missing from {got:?}");
        }
        assert_eq!(reasons(&r.warnings), ["alarm_wider_than_abort"]);
        let ambiguous = r
            .errors
            .iter()
            .find(|i| i.reason == "row_level_ambiguous")
            .unwrap();
        assert_eq!(ambiguous.at, crate::table::At::Row(1));
    }

    #[test]
    fn the_schedule_and_acquisition_errors_are_named() {
        let mut v = golden_config();
        v["schedule"]["levels"] = json!([{"db": -6}, {"db": -12, "dwell_s": 5}, {"db": 0}]);
        v["acquisition"]["n_pts"] = json!(1000);
        let (r, _) = parse(&v).unwrap().check(&table_with_monitor());
        let got = reasons(&r.errors);
        assert!(
            got.contains(&"levels_not_ascending") && got.contains(&"dwell_missing"),
            "{got:?}"
        );
        assert!(got.contains(&"n_pts_not_power_of_two"), "{got:?}");

        let mut v = golden_config();
        v["schedule"]["levels"] = json!([]);
        let (r, _) = parse(&v).unwrap().check(&table_with_monitor());
        assert!(reasons(&r.errors).contains(&"levels_empty"));
    }

    #[test]
    fn a_band_the_grid_cannot_reach_and_a_coarse_grid_are_refused() {
        let mut v = golden_config();
        v["acquisition"]["fs_hz"] = json!(4096);
        let (r, d) = parse(&v).unwrap().check(&table_with_monitor());
        assert!(d.is_none());
        assert_eq!(reasons(&r.errors), ["band_above_grid"]);

        let mut v = golden_config();
        v["acquisition"]["n_pts"] = json!(16);
        v["profile"]["breakpoints"] =
            json!([{"freq_hz": 1000, "psd_g2_hz": 0.01}, {"freq_hz": 2000, "psd_g2_hz": 0.01}]);
        let (r, _) = parse(&v).unwrap().check(&table_with_monitor());
        assert_eq!(reasons(&r.errors), ["too_few_lines"]);
    }

    #[test]
    fn control_and_limit_errors_are_named() {
        let mut v = golden_config();
        v["control"]["damping"] = json!(11);
        v["control"]["weights"] = json!([{"channel": 2, "weight": 1}]);
        v["limits"][0]["channel"] = json!(9);
        v["limits"][0]["rows"] =
            json!([{"freq_hz": 100, "ceiling": 1}, {"freq_hz": 50, "ceiling": 1}]);
        let (r, _) = parse(&v).unwrap().check(&table_with_monitor());
        let got = reasons(&r.errors);
        for want in [
            "out_of_range",
            "weight_unknown_channel",
            "limit_unknown_channel",
            "limit_rows_not_ascending",
        ] {
            assert!(got.contains(&want), "{want} missing from {got:?}");
        }
        let damping = r
            .errors
            .iter()
            .find(|i| i.reason == "out_of_range")
            .unwrap();
        assert_eq!(
            damping.at,
            crate::table::At::Field("control.damping".into())
        );
        assert_eq!(damping.message, "Damping is 11; it runs from 0 to 10");

        let mut t = table_with_monitor();
        t.channels[0].role = ChannelRole::Monitor;
        let (r, _) = parse(&golden_config()).unwrap().check(&t);
        assert!(reasons(&r.errors).contains(&"no_control_channel"));

        let mut v = golden_config();
        v["control"]["weights"] = json!([{"channel": 1, "weight": 0}]);
        let (r, _) = parse(&v).unwrap().check(&table_with_monitor());
        assert!(reasons(&r.errors).contains(&"weights_sum_not_positive"));
    }

    #[test]
    fn a_wrong_type_is_bad_params_naming_the_parameter() {
        let mut v = golden_config();
        v["acquisition"]["n_pts"] = json!("2048");
        let e = parse(&v).unwrap_err();
        assert_eq!(e.reason, Some("bad_params"));
        assert_eq!(e.detail.unwrap()["param"], "config.acquisition.n_pts");
        let mut v = golden_config();
        v.as_object_mut().unwrap().remove("schedule");
        assert_eq!(
            parse(&v).unwrap_err().detail.unwrap()["param"],
            "config.schedule"
        );
        let mut v = golden_config();
        v["feasibility"] = json!({"shaker": 7, "moving_mass_lb": {}});
        assert_eq!(parse(&v).unwrap_err().reason, Some("shaker_record_invalid"));
    }

    #[test]
    fn the_build_is_the_controllers_config_with_the_protocol_defaults() {
        let cfg = parse(&golden_config()).unwrap();
        let built = cfg.build(&table_with_monitor()).unwrap();
        let t = &built.test;
        assert_eq!(t.schedule.table_db, vec![-12.0, -6.0, 0.0]);
        assert_eq!(t.schedule.dwell_s, vec![30.0, 30.0, 0.0]);
        assert_eq!(t.schedule.mode, RunMode::Auto);
        assert_eq!(t.full_level_duration_s, 300.0);
        assert_eq!(t.control.indices(), vec![1]);
        assert_eq!(t.drive_ceiling_vrms, 2.5);
        assert_eq!(t.drive_clip_sigma, Some(5.0));
        assert_eq!(t.level_ramp_s, 3.0);
        assert_eq!(t.shutdown_ramp_s, 3.0);
        assert_eq!(t.pretest_max_vrms, Some(0.5));
        assert_eq!(t.limit_profiles.len(), 1);
        assert_eq!(built.loop_cfg.fs_hz, 8192.0);
        assert_eq!(built.loop_cfg.nfft, 2048);
        assert_eq!(
            built.loop_cfg.band_hi_hz, 2000.0,
            "the band is the profile's, not clamped"
        );
    }

    /// The identification is relative to the reference: the build hands the
    /// controller `pretest.level_db` and `start_vrms` (2026-11-01, bench finding
    /// 1), and a start above the ceiling is a named item.
    #[test]
    fn the_identification_level_is_relative_to_the_reference() {
        let cfg = parse(&golden_config()).unwrap();
        assert_eq!(cfg.control.pretest_level_db, -20.0);
        assert_eq!(cfg.control.pretest_start_vrms, 0.01);
        let t = cfg.build(&table_with_monitor()).unwrap().test;
        assert_eq!(t.pretest_reference_db, Some(-20.0));
        assert_eq!(t.pretest_start_vrms, Some(0.01));
        assert_eq!(t.pretest_max_vrms, Some(0.5));
        assert_eq!(
            t.pretest_min_vrms, None,
            "the referenced level has no floor"
        );

        let mut v = golden_config();
        v["control"]["pretest"]["level_db"] = json!(-30);
        v["control"]["pretest"]["start_vrms"] = json!(0.002);
        let t = parse(&v)
            .unwrap()
            .build(&table_with_monitor())
            .unwrap()
            .test;
        assert_eq!(t.pretest_reference_db, Some(-30.0));
        assert_eq!(t.pretest_start_vrms, Some(0.002));

        let mut v = golden_config();
        v["control"]["pretest"]["start_vrms"] = json!(0.8);
        let (r, _) = parse(&v).unwrap().check(&table_with_monitor());
        let item = r
            .errors
            .iter()
            .find(|i| i.at == crate::table::At::Field("control.pretest.start_vrms".into()))
            .expect("a start above the ceiling is named");
        assert_eq!(item.reason, "out_of_range");
        assert!(
            item.message.contains("largest identification drive"),
            "{}",
            item.message
        );

        let mut v = golden_config();
        v["control"]["pretest"]["level_db"] = json!(3);
        let (r, _) = parse(&v).unwrap().check(&table_with_monitor());
        assert!(r
            .errors
            .iter()
            .any(|i| i.at == crate::table::At::Field("control.pretest.level_db".into())));
    }

    #[test]
    fn limit_rows_land_on_the_run_grid() {
        let c = ceiling_on_grid(&[(100.0, 1.0), (400.0, 4.0)], 101, 10.0);
        assert!(
            c[5].is_infinite(),
            "outside the authored band is unconstrained"
        );
        assert!((c[10] - 1.0).abs() < 1e-12);
        assert!((c[20] - 2.0).abs() < 1e-9, "log-log midway");
        assert!((c[40] - 4.0).abs() < 1e-12);
    }
}
