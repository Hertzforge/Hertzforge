//! What a Random run shows: the `random.state` object and the `random.live` frame
//! (schema/PROTOCOL.md, "Run State" and "Live Display"), built from a [`RunView`]
//! the run publishes after every step. Keys are written in the contract's order.

use super::config::{Grid, WireConfig};
use crate::codec::Arrays;
use crate::protocol::{num, opt_num};
use crate::views::{self, Axis, Role, Row, Scale, Series, Status};
use hertzforge_breakpoints::Envelope;
use hertzforge_random::LoopConfig;
use hertzforge_random::RunMode;
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// The run's state, as the protocol names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Configured,
    Blocked,
    Armed,
    Running,
    Review,
    Stopping,
    Finished,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Configured => "configured",
            State::Blocked => "blocked",
            State::Armed => "armed",
            State::Running => "running",
            State::Review => "review",
            State::Stopping => "stopping",
            State::Finished => "finished",
        }
    }

    /// Armed, running, holding for review or stopping.
    pub fn is_live(self) -> bool {
        matches!(
            self,
            State::Armed | State::Running | State::Review | State::Stopping
        )
    }

    /// Driving or about to: the states `random.reset` and `random.clear` refuse.
    pub fn is_busy(self) -> bool {
        matches!(self, State::Running | State::Review | State::Stopping)
    }
}

pub fn mode_name(m: RunMode) -> &'static str {
    match m {
        RunMode::Auto => "auto",
        RunMode::Manual => "manual",
    }
}

/// The run's station: kind, class and the devices it is bound to.
#[derive(Debug, Clone, PartialEq)]
pub struct StationRef {
    pub kind: &'static str,
    pub class: &'static str,
    pub devices: Vec<(String, String)>,
}

impl StationRef {
    pub fn source(&self) -> &'static str {
        if self.class == "ni_real" {
            "HARDWARE"
        } else {
            "SIM"
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "kind": self.kind,
            "class": self.class,
            "devices": self.devices.iter().map(|(id, sn)| json!({"id": id, "serial": sn})).collect::<Vec<_>>(),
        })
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
pub struct End {
    pub outcome: &'static str,
    pub message: String,
    pub at_s: Option<f64>,
    pub level_db: Option<f64>,
    pub trip: Option<Value>,
}

/// One enabled input in the live display.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveChannel {
    pub index: u32,
    pub description: String,
    pub role: &'static str,
    pub rms: Option<f64>,
    pub unit: String,
    pub overload: bool,
}

/// An alarm holding now.
#[derive(Debug, Clone, PartialEq)]
pub enum Alarm {
    Envelope { lines: usize, worst_db: f64 },
    Overload { channel: u32 },
}

/// The run's lines and, at 0 dB, its reference and bands on them: fixed once the
/// run arms on the grid the station programmed.
#[derive(Debug, Clone, PartialEq)]
pub struct Lines {
    pub freqs: Vec<f64>,
    pub reference: Vec<f64>,
    pub alarm_hi: Vec<f64>,
    pub alarm_lo: Vec<f64>,
    pub abort_hi: Vec<f64>,
    pub abort_lo: Vec<f64>,
}

/// The lines of `loop_cfg`'s control band and, at 0 dB, the reference and each
/// row's own bands on them: the controller's own envelope construction. On the grid a
/// run arms on, these are the live display's lines; on the requested grid, the
/// reference view's.
pub fn lines_on(loop_cfg: &LoopConfig, cfg: &WireConfig) -> Lines {
    let (lo, hi) = loop_cfg.band_bins();
    let df = loop_cfg.df();
    let freqs: Vec<f64> = (lo..hi).map(|k| k as f64 * df).collect();
    let f_max = df * (loop_cfg.nbins() as f64 - 1.0);
    let zeros = || vec![0.0; freqs.len()];
    match Envelope::from_table_on_grid(&cfg.breakpoints(), df, f_max, cfg.alarm_db, cfg.abort_db) {
        Ok(env) => {
            let take = |v: &[f64]| -> Vec<f64> {
                (lo..hi)
                    .map(|k| v.get(k).copied().filter(|x| x.is_finite()).unwrap_or(0.0))
                    .collect()
            };
            Lines {
                reference: take(&env.reference.psd),
                alarm_hi: take(&env.alarm_hi),
                alarm_lo: take(&env.alarm_lo),
                abort_hi: take(&env.abort_hi),
                abort_lo: take(&env.abort_lo),
                freqs,
            }
        }
        Err(_) => Lines {
            reference: zeros(),
            alarm_hi: zeros(),
            alarm_lo: zeros(),
            abort_hi: zeros(),
            abort_lo: zeros(),
            freqs,
        },
    }
}

/// Every enabled input's data after a control loop, in the order of
/// [`RunView::channels`]: what the `response` and `time` quantities show
/// (schema/PROTOCOL.md, "Live Data Subscriptions").
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Inputs {
    /// Each input's PSD on the band's lines, in its own unit squared per Hz; `None`
    /// when it was not measured.
    pub psd: Vec<Option<Vec<f64>>>,
    /// The last control loop's acquired samples of each input, in its own unit.
    pub time: Vec<Vec<f32>>,
    /// The rate of `time`.
    pub fs_hz: f64,
}

/// One limited channel's ceiling on the band's lines where its limit applies
/// (the lines its rows cover).
#[derive(Debug, Clone, PartialEq)]
pub struct LimitLines {
    pub channel: u32,
    pub freqs: Vec<f64>,
    pub ceiling: Vec<f64>,
}

/// The reference view (schema/PROTOCOL.md, "Random Test Configuration"): the profile
/// and its bands at 0 dB on `lines`, every series stepped.
pub fn reference_view(lines: &Lines, arrays: &mut Arrays) -> Value {
    let f = || lines.freqs.clone();
    let series = vec![
        Series::new("Reference", Role::Reference, f(), lines.reference.clone()).stepped(),
        Series::new("Alarm high", Role::Tolerance, f(), lines.alarm_hi.clone()).stepped(),
        Series::new("Alarm low", Role::Tolerance, f(), lines.alarm_lo.clone()).stepped(),
        Series::new("Abort high", Role::Abort, f(), lines.abort_hi.clone()).stepped(),
        Series::new("Abort low", Role::Abort, f(), lines.abort_lo.clone()).stepped(),
    ];
    views::spectrum(
        arrays,
        "Reference",
        &Axis::new("Frequency", Some("Hz"), Scale::Log),
        &Axis::new("PSD", Some("g²/Hz"), Scale::Log),
        series,
        Some("frequency"),
    )
}

/// Everything the run shows, as of its last step.
#[derive(Debug, Clone)]
pub struct RunView {
    pub run_id: String,
    pub title: String,
    pub state: State,
    pub phase: &'static str,
    pub mode: RunMode,
    pub levels_db: Vec<f64>,
    pub full_level_s: f64,
    pub station: StationRef,
    pub grid: Grid,
    pub clock: Option<&'static str>,
    /// The commanded level (the identification level during system identification).
    pub level_db: Option<f64>,
    /// The level the loop runs at now (the level of its last control loop).
    pub live_level_db: Option<f64>,
    /// The level the drive is at now, block by block (the drive level ramp),
    /// while the loop drives; the live display's `level_db`.
    pub drive_now_db: Option<f64>,
    /// The level `drive_psd` and `drive_rms_v` were synthesized at.
    pub drive_level_db: Option<f64>,
    pub elapsed_s: Option<f64>,
    pub at_level_s: Option<f64>,
    pub remaining_at_level_s: Option<f64>,
    pub alarm: bool,
    pub feasibility: Option<(&'static str, String)>,
    pub end: Option<End>,
    pub loops: u64,
    pub control_rms_g: Option<f64>,
    pub reference_rms_g: Option<f64>,
    pub drive_rms_v: Option<f64>,
    pub lines_alarm: usize,
    pub lines_abort: usize,
    pub channels: Vec<LiveChannel>,
    pub alarms: Vec<Alarm>,
    pub lines: Option<Arc<Lines>>,
    pub control_psd: Option<Arc<Vec<f64>>>,
    pub drive_psd: Option<Arc<Vec<f64>>>,
    /// Every enabled input's PSD and samples from the last control loop.
    pub inputs: Option<Arc<Inputs>>,
    /// The system identification's ordinary coherence on the band's lines.
    pub coherence: Option<Arc<Vec<f64>>>,
    /// Each limited channel's ceiling, fixed when the run arms.
    pub limits: Arc<Vec<LimitLines>>,
    /// The time the schedule holds the present level (its dwell, or the full
    /// level time), while driving in `auto`.
    pub level_s: Option<f64>,
    /// `random.live` frames are due: from the start of system identification.
    pub streaming: bool,
}

fn r(x: f64, d: i32) -> f64 {
    super::config::round(x, d)
}

fn opt_r(x: Option<f64>, d: i32) -> Value {
    opt_num(x.map(|v| r(v, d)))
}

/// The `random.state` object; `None` is the unconfigured service.
pub fn state_json(v: Option<&RunView>) -> Value {
    let Some(v) = v else {
        return json!({
            "run_id": null, "state": "unconfigured", "phase": "idle", "title": null,
            "source": null, "station": null, "grid": null, "clock": null, "mode": null,
            "levels_db": null, "level_db": null, "full_level_s": null, "elapsed_s": null,
            "at_level_s": null, "remaining_at_level_s": null, "alarm": false,
            "feasibility": null, "end": null,
        });
    };
    let mut m = Map::new();
    m.insert("run_id".into(), Value::from(v.run_id.as_str()));
    m.insert("state".into(), Value::from(v.state.name()));
    m.insert("phase".into(), Value::from(v.phase));
    m.insert("title".into(), Value::from(v.title.as_str()));
    m.insert("source".into(), Value::from(v.station.source()));
    m.insert("station".into(), v.station.to_json());
    m.insert("grid".into(), v.grid.to_json());
    m.insert("clock".into(), v.clock.map_or(Value::Null, Value::from));
    m.insert("mode".into(), Value::from(mode_name(v.mode)));
    m.insert(
        "levels_db".into(),
        Value::Array(v.levels_db.iter().map(|x| num(*x)).collect()),
    );
    m.insert("level_db".into(), opt_r(v.level_db, 2));
    m.insert("full_level_s".into(), num(v.full_level_s));
    m.insert("elapsed_s".into(), opt_r(v.elapsed_s, 2));
    m.insert("at_level_s".into(), opt_r(v.at_level_s, 2));
    m.insert(
        "remaining_at_level_s".into(),
        opt_r(v.remaining_at_level_s, 2),
    );
    m.insert("alarm".into(), Value::from(v.alarm));
    m.insert(
        "feasibility".into(),
        v.feasibility.as_ref().map_or(
            Value::Null,
            |(verdict, message)| json!({"verdict": verdict, "message": message}),
        ),
    );
    m.insert(
        "end".into(),
        v.end.as_ref().map_or(Value::Null, |e| {
            json!({
                "outcome": e.outcome,
                "message": e.message,
                "at_s": opt_r(e.at_s, 2),
                "level_db": opt_r(e.level_db, 2),
                "trip": e.trip.clone().unwrap_or(Value::Null),
            })
        }),
    );
    Value::Object(m)
}

/// What decides whether a `random.state` event is due: a change of state, phase,
/// mode, commanded level, alarm, end (or the facts the run bar shows beside them).
pub fn state_key(v: Option<&RunView>) -> String {
    let s = state_json(v);
    serde_json::to_string(&json!([
        s["run_id"],
        s["state"],
        s["phase"],
        s["mode"],
        s["level_db"],
        s["alarm"],
        s["end"],
        s["feasibility"],
        s["clock"],
        s["grid"],
    ]))
    .unwrap_or_default()
}

fn db_text(x: f64) -> String {
    format!("{}{:.1} dB", if x >= 0.0 { "+" } else { "" }, x)
}

fn no_lines() -> Lines {
    Lines {
        freqs: Vec::new(),
        reference: Vec::new(),
        alarm_hi: Vec::new(),
        alarm_lo: Vec::new(),
        abort_hi: Vec::new(),
        abort_lo: Vec::new(),
    }
}

/// The frequency axis every live spectrum shares.
pub fn frequency_axis() -> Axis {
    Axis::new("Frequency", Some("Hz"), Scale::Log)
}

/// The **Control** view: control against the reference and its bands, at the last
/// control loop's level, stepped on the run's lines.
pub fn control_view(v: &RunView, arrays: &mut Arrays) -> Value {
    let empty = no_lines();
    let lines = v.lines.as_deref().unwrap_or(&empty);
    let level = v.live_level_db.unwrap_or(0.0);
    let k = 10f64.powf(level / 10.0);
    let at = |band: &[f64]| -> Vec<f64> { band.iter().map(|x| x * k).collect() };
    let f = || lines.freqs.clone();
    let mut control = Vec::with_capacity(6);
    if let Some(c) = &v.control_psd {
        control.push(Series::new("Control", Role::Measured, f(), c.to_vec()).stepped());
    }
    control.push(Series::new("Reference", Role::Reference, f(), at(&lines.reference)).stepped());
    control.push(Series::new("Alarm high", Role::Tolerance, f(), at(&lines.alarm_hi)).stepped());
    control.push(Series::new("Alarm low", Role::Tolerance, f(), at(&lines.alarm_lo)).stepped());
    control.push(Series::new("Abort high", Role::Abort, f(), at(&lines.abort_hi)).stepped());
    control.push(Series::new("Abort low", Role::Abort, f(), at(&lines.abort_lo)).stepped());
    views::spectrum(
        arrays,
        "Control",
        &frequency_axis(),
        &Axis::new("PSD", Some("g²/Hz"), Scale::Log),
        control,
        Some("frequency"),
    )
}

/// The **Drive** view, stepped on the run's lines.
pub fn drive_view(v: &RunView, arrays: &mut Arrays) -> Value {
    let freqs = v
        .lines
        .as_deref()
        .map_or_else(Vec::new, |l| l.freqs.clone());
    let drive_series = v
        .drive_psd
        .as_ref()
        .map(|d| vec![Series::new("Drive", Role::Drive, freqs, d.to_vec()).stepped()])
        .unwrap_or_default();
    views::spectrum(
        arrays,
        "Drive",
        &frequency_axis(),
        &Axis::new("PSD", Some("V²/Hz"), Scale::Log),
        drive_series,
        Some("frequency"),
    )
}

/// Control against the reference in dB, from the same control loop.
pub fn control_error_db(v: &RunView) -> Option<f64> {
    match (v.control_rms_g, v.reference_rms_g) {
        (Some(c), Some(rf)) if c > 0.0 && rf > 0.0 => Some(20.0 * (c / rf).log10()),
        _ => None,
    }
}

/// The **Run** reading view: the run's numbers as rows.
pub fn run_reading(v: &RunView) -> Value {
    let in_band = v.lines.as_deref().map_or(0, |l| l.freqs.len());
    let rows = vec![
        match (v.control_rms_g, control_error_db(v)) {
            (Some(c), Some(e)) => Row::number(
                "Control rms",
                r(c, 3),
                Some("g"),
                if e.abs() <= 1.0 {
                    Status::Pass
                } else {
                    Status::Warn
                },
            ),
            (Some(c), None) => Row::number("Control rms", r(c, 3), Some("g"), Status::Info),
            _ => Row::text("Control rms", "pending", Status::Info),
        },
        match v.reference_rms_g {
            Some(x) => Row::number("Reference rms", r(x, 3), Some("g"), Status::Info),
            None => Row::text("Reference rms", "pending", Status::Info),
        },
        match v.drive_rms_v {
            Some(x) => Row::number("Drive rms", r(x, 3), Some("V"), Status::Info),
            None => Row::text("Drive rms", "pending", Status::Info),
        },
        Row::text(
            "Level",
            &v.drive_now_db
                .or(v.live_level_db)
                .map_or_else(|| "not driving".to_owned(), db_text),
            Status::Info,
        ),
        Row::text(
            "Lines in alarm",
            &format!("{} of {in_band}", v.lines_alarm),
            if v.lines_alarm == 0 {
                Status::Pass
            } else {
                Status::Warn
            },
        ),
        Row::number(
            "Elapsed",
            r(v.elapsed_s.unwrap_or(0.0), 1),
            Some("s"),
            Status::Info,
        ),
    ];
    views::reading("Run", rows)
}

/// Every field of a live frame but its views (`random.live`) or plots
/// (`random.data`), in the contract's order.
pub fn live_head(v: &RunView) -> Map<String, Value> {
    let in_band = v.lines.as_deref().map_or(0, |l| l.freqs.len());
    let state = match v.state {
        State::Running | State::Review | State::Stopping | State::Finished => v.state.name(),
        // A live frame is only sent from the start of system identification; an
        // armed run has not started.
        _ => "running",
    };
    let driving = matches!(v.state, State::Running | State::Stopping);
    let mut m = Map::new();
    m.insert("run_id".into(), Value::from(v.run_id.as_str()));
    m.insert("source".into(), Value::from(v.station.source()));
    m.insert("class".into(), Value::from(v.station.class));
    m.insert("state".into(), Value::from(state));
    m.insert("phase".into(), Value::from(v.phase));
    m.insert("mode".into(), Value::from(mode_name(v.mode)));
    m.insert("elapsed_s".into(), num(r(v.elapsed_s.unwrap_or(0.0), 2)));
    m.insert(
        "level_db".into(),
        if driving {
            opt_r(v.drive_now_db.or(v.live_level_db), 2)
        } else {
            Value::Null
        },
    );
    m.insert(
        "commanded_db".into(),
        if driving {
            opt_r(v.level_db, 2)
        } else {
            Value::Null
        },
    );
    m.insert("at_level_s".into(), opt_r(v.at_level_s, 2));
    m.insert(
        "remaining_at_level_s".into(),
        opt_r(v.remaining_at_level_s, 2),
    );
    m.insert(
        "level_s".into(),
        if driving {
            opt_r(v.level_s, 2)
        } else {
            Value::Null
        },
    );
    m.insert("loop".into(), Value::from(v.loops));
    m.insert("control_rms_g".into(), opt_r(v.control_rms_g, 3));
    m.insert("reference_rms_g".into(), opt_r(v.reference_rms_g, 3));
    m.insert("control_error_db".into(), opt_r(control_error_db(v), 2));
    m.insert("drive_rms_v".into(), opt_r(v.drive_rms_v, 4));
    m.insert(
        "lines".into(),
        json!({"in_band": in_band, "alarm": v.lines_alarm, "abort": v.lines_abort}),
    );
    m.insert(
        "channels".into(),
        Value::Array(
            v.channels
                .iter()
                .map(|c| {
                    json!({"index": c.index, "description": c.description, "role": c.role,
                           "rms": opt_r(c.rms, 3), "unit": c.unit, "overload": c.overload})
                })
                .collect(),
        ),
    );
    m.insert(
        "alarms".into(),
        Value::Array(
            v.alarms
                .iter()
                .map(|a| match a {
                    Alarm::Envelope { lines, worst_db } => {
                        json!({"kind": "envelope", "lines": lines, "worst_db": num(r(*worst_db, 2))})
                    }
                    Alarm::Overload { channel } => json!({"kind": "overload", "channel": channel}),
                })
                .collect(),
        ),
    );
    m
}

/// The `random.live` data; series attach to `arrays`.
pub fn live_json(v: &RunView, arrays: &mut Arrays) -> Value {
    let mut m = live_head(v);
    let control = control_view(v, arrays);
    let drive = drive_view(v, arrays);
    m.insert(
        "views".into(),
        Value::Array(vec![control, drive, run_reading(v)]),
    );
    Value::Object(m)
}
