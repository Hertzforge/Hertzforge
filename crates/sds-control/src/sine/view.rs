//! What a Sine run shows: the `sine.state` object, the numbers every live frame
//! carries, and the views drawn from them (schema/PROTOCOL.md, "Sine Run State" and
//! "Sine Display"), built from a [`SineView`] the run publishes and the
//! [`Figures`] it keeps. Keys are written in the contract's order.

use super::config::{direction_name, mode_name, Grid, LawRef, WireSine};
use crate::codec::Arrays;
use crate::protocol::{num, opt_num};
use crate::views::{self, Axis, Role, Row, Scale, Series, Status};
use hertzforge_sine::{RunMode, SweepDirection};
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub use crate::random::view::State;

/// The most points an identification view carries.
pub const IDENT_POINTS: usize = 1024;

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

    pub fn to_json(&self) -> Value {
        json!({
            "kind": self.kind,
            "class": self.class,
            "devices": self.devices.iter().map(|(id, sn)| json!({"id": id, "serial": sn})).collect::<Vec<_>>(),
        })
    }
}

/// The identification's facts, once the gate has run.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentFacts {
    pub level_db: f64,
    pub band_coherence: f64,
    pub gate_coherence: f64,
    pub lag_frames: usize,
    pub trusted_frac: f64,
    pub passed: bool,
}

/// The run's law and its counters.
#[derive(Debug, Clone, PartialEq)]
pub struct LawState {
    pub law: LawRef,
    pub misses: u32,
    pub capped_ticks: u64,
    /// The live parameters' values in force, by key.
    pub live: Vec<(String, f64)>,
    /// The law's values on its last call, by key; empty until it is called.
    pub values: Vec<(String, f64)>,
    /// Labels and units of the declared values, by key, for the Law reading.
    pub labels: Vec<(String, String, String)>,
    /// Whether the law has been built (the live frame's `law` is null until then).
    pub built: bool,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
pub struct End {
    pub outcome: &'static str,
    pub message: String,
    pub at_s: Option<f64>,
    pub level_db: Option<f64>,
    pub at_hz: Option<f64>,
    pub ramp_s: Option<f64>,
    pub trip: Option<Value>,
}

/// One enabled input in the live frame.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveChannel {
    pub index: u32,
    pub description: String,
    pub role: &'static str,
    pub amplitude: Option<f64>,
    pub phase_deg: Option<f64>,
    pub rms: Option<f64>,
    pub unit: String,
    pub overload: bool,
}

/// What holds now.
#[derive(Debug, Clone, PartialEq)]
pub enum Alarm {
    Band { side: &'static str, error_db: f64 },
    Overload { channel: u32 },
}

/// The sweep's rate now, in its own law's unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rate {
    OctPerMin(f64),
    HzPerS(f64),
}

/// Everything the state and the live frame say, as the run last published it.
#[derive(Debug, Clone, PartialEq)]
pub struct SineView {
    pub run_id: String,
    pub title: String,
    pub state: State,
    pub phase: &'static str,
    pub station: StationRef,
    pub grid: Grid,
    pub clock: Option<&'static str>,
    pub test: &'static str,
    pub mode: RunMode,
    pub levels_db: Vec<f64>,
    /// The scheduled level.
    pub level_db: Option<f64>,
    pub rung: Option<usize>,
    pub frequency_hz: Option<f64>,
    pub direction: Option<SweepDirection>,
    /// One-way sweeps done at this level, and of how many (`auto` with a count).
    pub sweeps: Option<(u32, Option<u32>)>,
    pub held: bool,
    pub elapsed_s: Option<f64>,
    pub at_level_s: Option<f64>,
    pub remaining_s: Option<f64>,
    pub alarm: bool,
    pub notch: bool,
    pub identification: Option<IdentFacts>,
    pub law: Option<LawState>,
    pub feasibility: Option<(&'static str, String)>,
    pub end: Option<End>,
    // ---- the live frame's own numbers ----
    pub t_s: Option<f64>,
    pub rate: Rate,
    /// The level the drive is at now.
    pub achieved_db: Option<f64>,
    pub target: Option<f64>,
    pub measured: Option<f64>,
    pub drive_vpk: Option<f64>,
    pub cola_vpk: Option<f64>,
    pub channels: Vec<LiveChannel>,
    pub alarms: Vec<Alarm>,
    /// The control rows' unit, for the views.
    pub control_unit: String,
    /// Whether the run has started streaming (`sine.live` from identification on).
    pub streaming: bool,
    /// The figures the views are drawn from, as of the last publish.
    pub figures: Option<Arc<Figures>>,
    /// The run's record, once it is armed.
    pub record: Option<crate::random::view::RecordRef>,
}

/// One input's curves on the figures' x.
#[derive(Debug, Clone, PartialEq)]
pub struct InputCurves {
    pub index: u32,
    pub tracked: Vec<f64>,
    pub phase_deg: Vec<f64>,
    /// A limit channel's bound on the figures' x.
    pub limit: Option<Vec<f64>>,
}

/// The identification as drawn: its grid inside the band, at most
/// [`IDENT_POINTS`] points.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentFigures {
    pub freqs_hz: Vec<f64>,
    pub gain: Vec<f64>,
    pub coherence: Vec<f64>,
    pub trusted: Vec<bool>,
    /// The gain floor the identification is trusted above.
    pub floor: f64,
    pub trust_coherence: f64,
    pub unit: String,
}

/// Every curve the views are drawn from: against frequency on the sweep grid in a
/// sweep or a step test (`visited` the points the run has reached), against
/// station time in a dwell or a resonance search.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Figures {
    pub against_time: bool,
    pub x: Vec<f64>,
    pub visited: Vec<bool>,
    pub reference: Vec<f64>,
    pub alarm_lo: Vec<f64>,
    pub alarm_hi: Vec<f64>,
    pub abort_lo: Vec<f64>,
    pub abort_hi: Vec<f64>,
    pub measured: Vec<f64>,
    pub drive: Vec<f64>,
    pub cola: Option<Vec<f64>>,
    /// The control channel's THD, a fraction.
    pub thd: Vec<f64>,
    pub inputs: Vec<InputCurves>,
    /// The last batch's samples of every enabled input, in table order, and their
    /// rate.
    pub time: Vec<(u32, Vec<f32>)>,
    pub time_fs_hz: f64,
    pub identification: Option<IdentFigures>,
}

pub fn station_ref_json(s: &StationRef) -> Value {
    s.to_json()
}

fn law_json(l: &LawState) -> Value {
    let mut live = Map::new();
    for (k, v) in &l.live {
        live.insert(k.clone(), num(*v));
    }
    json!({
        "id": l.law.id,
        "name": l.law.name,
        "version": l.law.version,
        "sha256": l.law.sha_json(),
        "misses": l.misses,
        "capped_ticks": l.capped_ticks,
        "live": live,
    })
}

fn trip_json(t: &Option<Value>) -> Value {
    t.clone().unwrap_or(Value::Null)
}

fn end_json(e: &End) -> Value {
    json!({
        "outcome": e.outcome,
        "message": e.message,
        "at_s": opt_num(e.at_s.map(|x| round(x, 3))),
        "level_db": opt_num(e.level_db.map(|x| round(x, 2))),
        "at_hz": opt_num(e.at_hz.map(|x| round(x, 2))),
        "ramp_s": opt_num(e.ramp_s.map(|x| round(x, 3))),
        "trip": trip_json(&e.trip),
    })
}

pub fn round(x: f64, digits: i32) -> f64 {
    super::config::round(x, digits)
}

fn ident_json(i: &IdentFacts) -> Value {
    json!({
        "level_db": num(i.level_db),
        "band_coherence": num(round(i.band_coherence, 4)),
        "gate_coherence": num(i.gate_coherence),
        "lag_frames": i.lag_frames,
        "trusted_frac": num(round(i.trusted_frac, 4)),
        "passed": i.passed,
    })
}

fn sweeps_json(s: Option<(u32, Option<u32>)>) -> Value {
    match s {
        None => Value::Null,
        Some((done, of)) => json!({"done": done, "of": of}),
    }
}

/// The `sine.state` object; `None` is the unconfigured service.
pub fn state_json(v: Option<&SineView>) -> Value {
    let Some(v) = v else {
        return json!({
            "run_id": null, "state": "unconfigured", "phase": "idle", "title": null,
            "source": null, "station": null, "grid": null, "clock": null, "test": null,
            "mode": null, "levels_db": null, "level_db": null, "rung": null,
            "frequency_hz": null, "direction": null, "sweeps": null, "held": false,
            "elapsed_s": null, "at_level_s": null, "remaining_s": null, "alarm": false,
            "notch": false, "identification": null, "law": null, "feasibility": null,
            "end": null,
        });
    };
    let mut m = json!({
        "run_id": v.run_id,
        "state": v.state.name(),
        "phase": v.phase,
        "title": v.title,
        "source": v.station.source(),
        "station": v.station.to_json(),
        "grid": v.grid.to_json(),
        "clock": v.clock,
        "test": v.test,
        "mode": mode_name(v.mode),
        "levels_db": v.levels_db.iter().map(|l| num(*l)).collect::<Vec<_>>(),
        "level_db": opt_num(v.level_db.map(|x| round(x, 2))),
        "rung": v.rung,
        "frequency_hz": opt_num(v.frequency_hz.map(|x| round(x, 2))),
        "direction": v.direction.map(direction_name),
        "sweeps": sweeps_json(v.sweeps),
        "held": v.held,
        "elapsed_s": opt_num(v.elapsed_s.map(|x| round(x, 3))),
        "at_level_s": opt_num(v.at_level_s.map(|x| round(x, 3))),
        "remaining_s": opt_num(v.remaining_s.map(|x| round(x, 3))),
        "alarm": v.alarm,
        "notch": v.notch,
        "identification": v.identification.as_ref().map_or(Value::Null, ident_json),
        "law": v.law.as_ref().map_or(Value::Null, law_json),
        "feasibility": v.feasibility.as_ref().map_or(Value::Null, |(verdict, message)| {
            json!({"verdict": verdict, "message": message})
        }),
        "end": v.end.as_ref().map_or(Value::Null, end_json),
    });
    if let (Some(r), Value::Object(o)) = (&v.record, &mut m) {
        o.insert("record".into(), r.to_json());
    }
    m
}

/// What `sine.state` events are sent on a change of.
pub fn state_key(v: Option<&SineView>) -> String {
    match v {
        None => "unconfigured".into(),
        Some(v) => format!(
            "{}|{}|{}|{}|{:?}|{}|{}|{}|{}|{:?}|{}",
            v.run_id,
            v.state.name(),
            v.phase,
            mode_name(v.mode),
            v.level_db.map(|x| round(x, 2)),
            v.held,
            v.alarm,
            v.notch,
            v.end.is_some(),
            v.rung,
            v.record.as_ref().map_or(Value::Null, |r| r.to_json()),
        ),
    }
}

/// The error of `measured` against `target`, dB.
pub fn error_db(v: &SineView) -> Option<f64> {
    match (v.measured, v.target) {
        (Some(m), Some(t)) if m > 0.0 && t > 0.0 => Some(20.0 * (m / t).log10()),
        _ => None,
    }
}

/// The numbers every live frame carries, `sine.live` and `sine.data` alike.
pub fn live_head(v: &SineView) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("run_id".into(), Value::from(v.run_id.as_str()));
    m.insert("source".into(), Value::from(v.station.source()));
    m.insert("class".into(), Value::from(v.station.class));
    m.insert("state".into(), Value::from(v.state.name()));
    m.insert("phase".into(), Value::from(v.phase));
    m.insert("test".into(), Value::from(v.test));
    m.insert("mode".into(), Value::from(mode_name(v.mode)));
    m.insert("t_s".into(), opt_num(v.t_s.map(|x| round(x, 6))));
    m.insert(
        "elapsed_s".into(),
        opt_num(v.elapsed_s.map(|x| round(x, 3))),
    );
    m.insert(
        "frequency_hz".into(),
        opt_num(v.frequency_hz.map(|x| round(x, 3))),
    );
    m.insert(
        "direction".into(),
        v.direction
            .map_or(Value::Null, |d| Value::from(direction_name(d))),
    );
    match v.rate {
        Rate::OctPerMin(r) => m.insert("sweep_rate_oct_per_min".into(), num(round(r, 4))),
        Rate::HzPerS(r) => m.insert("sweep_rate_hz_per_s".into(), num(round(r, 4))),
    };
    m.insert("sweeps".into(), sweeps_json(v.sweeps));
    m.insert("held".into(), Value::from(v.held));
    m.insert(
        "level_db".into(),
        opt_num(v.achieved_db.map(|x| round(x, 2))),
    );
    m.insert(
        "commanded_db".into(),
        opt_num(v.level_db.map(|x| round(x, 2))),
    );
    m.insert("target".into(), opt_num(v.target.map(|x| sig(x, 6))));
    m.insert("measured".into(), opt_num(v.measured.map(|x| sig(x, 6))));
    m.insert("error_db".into(), opt_num(error_db(v).map(|x| round(x, 3))));
    m.insert("drive_vpk".into(), opt_num(v.drive_vpk.map(|x| sig(x, 6))));
    m.insert("cola_vpk".into(), opt_num(v.cola_vpk.map(|x| sig(x, 6))));
    m.insert("notch".into(), Value::from(v.notch));
    m.insert("alarm".into(), Value::from(v.alarm));
    m.insert(
        "channels".into(),
        Value::Array(
            v.channels
                .iter()
                .map(|c| {
                    json!({
                        "index": c.index,
                        "description": c.description,
                        "role": c.role,
                        "amplitude": opt_num(c.amplitude.map(|x| sig(x, 6))),
                        "phase_deg": opt_num(c.phase_deg.map(|x| round(x, 2))),
                        "rms": opt_num(c.rms.map(|x| sig(x, 6))),
                        "unit": c.unit,
                        "overload": c.overload,
                    })
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
                    Alarm::Band { side, error_db } => {
                        json!({"kind": "band", "side": side, "error_db": num(round(*error_db, 2))})
                    }
                    Alarm::Overload { channel } => json!({"kind": "overload", "channel": channel}),
                })
                .collect(),
        ),
    );
    m.insert(
        "law".into(),
        match v.law.as_ref().filter(|l| l.built) {
            None => Value::Null,
            Some(l) => {
                let mut values = Map::new();
                for (k, x) in &l.values {
                    values.insert(k.clone(), num(sig(*x, 6)));
                }
                json!({"values": values, "misses": l.misses, "capped_ticks": l.capped_ticks})
            }
        },
    );
    m
}

/// `x` to `sig` significant digits (0 and non-finite values as they are).
pub fn sig(x: f64, digits: i32) -> f64 {
    super::profile::sig(x, digits)
}

fn x_axis(f: &Figures) -> Axis {
    if f.against_time {
        Axis::new("Time", Some("s"), Scale::Linear)
    } else {
        super::profile::freq_axis()
    }
}

fn link(f: &Figures) -> &'static str {
    if f.against_time {
        "time"
    } else {
        "frequency"
    }
}

/// The (x, y) pairs of `y` where the run has been and `y` is a number.
fn reached(f: &Figures, y: &[f64]) -> (Vec<f64>, Vec<f64>) {
    f.x.iter()
        .zip(y)
        .enumerate()
        .filter(|(i, (_, y))| y.is_finite() && f.visited.get(*i).copied().unwrap_or(true))
        .map(|(_, (x, y))| (*x, *y))
        .unzip()
}

/// The (x, y) pairs of `y` where it is a number.
fn finite(f: &Figures, y: &[f64]) -> (Vec<f64>, Vec<f64>) {
    f.x.iter()
        .zip(y)
        .filter(|(_, y)| y.is_finite())
        .map(|(x, y)| (*x, *y))
        .unzip()
}

fn series(name: &str, role: Role, xy: (Vec<f64>, Vec<f64>)) -> Series {
    Series::new(name, role, xy.0, xy.1)
}

/// **Control**: the composite where the run has been, the profile and its bands.
pub fn control_view(f: &Figures, unit: &str, arrays: &mut Arrays) -> Value {
    let s = vec![
        series("Control", Role::Measured, reached(f, &f.measured)),
        series("Profile", Role::Reference, finite(f, &f.reference)),
        series("Alarm high", Role::Tolerance, finite(f, &f.alarm_hi)),
        series("Alarm low", Role::Tolerance, finite(f, &f.alarm_lo)),
        series("Abort high", Role::Abort, finite(f, &f.abort_hi)),
        series("Abort low", Role::Abort, finite(f, &f.abort_lo)),
    ];
    views::spectrum(
        arrays,
        "Control",
        &x_axis(f),
        &Axis::new("Amplitude", Some(unit), Scale::Log),
        s,
        Some(link(f)),
    )
}

/// **Drive**: the drive as played, and the reference output with COLA.
pub fn drive_view(f: &Figures, arrays: &mut Arrays) -> Value {
    let mut s = vec![series("Drive", Role::Drive, reached(f, &f.drive))];
    if let Some(c) = &f.cola {
        s.push(series("Reference output", Role::Other, reached(f, c)));
    }
    views::spectrum(
        arrays,
        "Drive",
        &x_axis(f),
        &Axis::new("Drive", Some("V"), Scale::Log),
        s,
        Some(link(f)),
    )
}

/// `Channel <index>: <description>`, the role when there is no description.
pub fn channel_title(c: &LiveChannel) -> String {
    let what = if c.description.trim().is_empty() {
        c.role
    } else {
        c.description.trim()
    };
    format!("Channel {}: {what}", c.index)
}

/// One channel's tracked amplitude, and a limit channel's bound.
pub fn response_view(f: &Figures, c: &LiveChannel, arrays: &mut Arrays) -> Value {
    let mut s = Vec::with_capacity(2);
    if let Some(i) = f.inputs.iter().find(|i| i.index == c.index) {
        s.push(series(
            &format!("Channel {}", c.index),
            Role::Measured,
            reached(f, &i.tracked),
        ));
        if let Some(l) = &i.limit {
            s.push(series("Limit", Role::Limit, finite(f, l)));
        }
    }
    views::spectrum(
        arrays,
        &channel_title(c),
        &x_axis(f),
        &Axis::new("Amplitude", Some(&c.unit), Scale::Log),
        s,
        Some(link(f)),
    )
}

/// One channel's phase against the drive.
pub fn phase_view(f: &Figures, c: &LiveChannel, arrays: &mut Arrays) -> Value {
    let s = f
        .inputs
        .iter()
        .find(|i| i.index == c.index)
        .map(|i| {
            vec![series(
                &format!("Channel {}", c.index),
                Role::Measured,
                reached(f, &i.phase_deg),
            )]
        })
        .unwrap_or_default();
    views::spectrum(
        arrays,
        &channel_title(c),
        &x_axis(f),
        &Axis::new("Phase", Some("deg"), Scale::Linear),
        s,
        Some(link(f)),
    )
}

/// **Distortion**: the control channel's THD, percent.
pub fn thd_view(f: &Figures, arrays: &mut Arrays) -> Value {
    let pct: Vec<f64> = f.thd.iter().map(|x| x * 100.0).collect();
    views::spectrum(
        arrays,
        "Distortion",
        &x_axis(f),
        &Axis::new("THD", Some("%"), Scale::Linear),
        vec![series("THD", Role::Measured, reached(f, &pct))],
        Some(link(f)),
    )
}

/// One channel's last batch of samples (Random's `time` view).
pub fn time_view(f: &Figures, c: &LiveChannel, points: usize, arrays: &mut Arrays) -> Value {
    let mut s = Vec::with_capacity(1);
    if let Some((_, samples)) = f.time.iter().find(|(i, s)| *i == c.index && !s.is_empty()) {
        let (t, y) = crate::random::subscribe::decimate(samples, f.time_fs_hz, points);
        s.push(Series::new(
            &format!("Channel {}", c.index),
            Role::Measured,
            t,
            y,
        ));
    }
    views::spectrum(
        arrays,
        &channel_title(c),
        &Axis::new("Time", Some("s"), Scale::Linear),
        &Axis::new("Amplitude", Some(&c.unit), Scale::Linear),
        s,
        None,
    )
}

/// **Identification Gain** and **Coherence**; empty until the gate has run.
pub fn identification_views(id: Option<&IdentFigures>, arrays: &mut Arrays) -> Vec<Value> {
    let Some(id) = id.filter(|i| !i.freqs_hz.is_empty()) else {
        return Vec::new();
    };
    let f = &id.freqs_hz;
    let ends = vec![f[0], f[f.len() - 1]];
    let gain = views::spectrum(
        arrays,
        "Identification Gain",
        &super::profile::freq_axis(),
        &Axis::new("Gain", Some(&format!("{}/V", id.unit)), Scale::Log),
        vec![
            Series::new("Gain", Role::Measured, f.clone(), id.gain.clone()),
            Series::new(
                "Trust floor",
                Role::Tolerance,
                ends.clone(),
                vec![id.floor, id.floor],
            ),
        ],
        Some("frequency"),
    );
    let (tx, ty): (Vec<f64>, Vec<f64>) = f
        .iter()
        .zip(&id.coherence)
        .zip(&id.trusted)
        .filter(|(_, t)| **t)
        .map(|((x, y), _)| (*x, *y))
        .unzip();
    let coh = views::spectrum(
        arrays,
        "Coherence",
        &super::profile::freq_axis(),
        &Axis::new("Coherence", None, Scale::Linear),
        vec![
            Series::new("Coherence", Role::Measured, f.clone(), id.coherence.clone()),
            Series::new("Trusted", Role::Reference, tx, ty),
            Series::new(
                "Trust coherence",
                Role::Tolerance,
                ends,
                vec![id.trust_coherence, id.trust_coherence],
            ),
        ],
        Some("frequency"),
    );
    vec![gain, coh]
}

/// Whether the control point is inside its alarm band now.
fn in_band(v: &SineView) -> Status {
    if v.measured.is_none() || v.target.is_none() {
        Status::Info
    } else if v.alarms.iter().any(|a| matches!(a, Alarm::Band { .. })) {
        Status::Warn
    } else {
        Status::Pass
    }
}

/// A `reading` view whose numbers are written as the binary frames' headers write
/// them: a whole number as an integer, so every language's writer gives the same
/// bytes.
fn reading(title: &str, rows: Vec<Row>) -> Value {
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let mut e = Map::new();
            e.insert("label".into(), Value::from(r.label));
            e.insert(
                "value".into(),
                match r.value {
                    views::RowValue::Number(x) => num(x),
                    views::RowValue::Text(s) => Value::from(s),
                },
            );
            e.insert("unit".into(), r.unit.map_or(Value::Null, Value::from));
            e.insert("status".into(), Value::from(r.status.as_str()));
            Value::Object(e)
        })
        .collect();
    json!({"view": "reading", "title": title, "rows": rows})
}

/// **Run**: the run's numbers.
pub fn run_reading(v: &SineView) -> Value {
    let mut rows = Vec::with_capacity(10);
    let n =
        |label: &str, x: Option<f64>, unit: Option<&str>, status: Status, rows: &mut Vec<Row>| {
            if let Some(x) = x {
                rows.push(Row::number(label, x, unit, status));
            }
        };
    n(
        "Frequency",
        v.frequency_hz.map(|x| round(x, 2)),
        Some("Hz"),
        Status::Info,
        &mut rows,
    );
    if let Some(d) = v.direction.filter(|_| v.test == "sweep") {
        rows.push(Row::text("Direction", direction_name(d), Status::Info));
    }
    n(
        "Level",
        v.level_db.map(|x| round(x, 2)),
        Some("dB"),
        Status::Info,
        &mut rows,
    );
    let unit = v.control_unit.as_str();
    n(
        "Target",
        v.target.map(|x| sig(x, 4)),
        Some(unit),
        Status::Info,
        &mut rows,
    );
    let band = in_band(v);
    n(
        "Measured",
        v.measured.map(|x| sig(x, 4)),
        Some(unit),
        band,
        &mut rows,
    );
    n(
        "Error",
        error_db(v).map(|x| round(x, 2)),
        Some("dB"),
        band,
        &mut rows,
    );
    n(
        "Drive",
        v.drive_vpk.map(|x| sig(x, 4)),
        Some("V"),
        Status::Info,
        &mut rows,
    );
    if v.test == "sweep" {
        if let Some((done, of)) = v.sweeps {
            let text = match of {
                Some(of) => format!("{done} of {of}"),
                None => done.to_string(),
            };
            rows.push(Row::text("Sweeps", &text, Status::Info));
        }
    }
    n(
        "Elapsed",
        v.elapsed_s.map(|x| round(x, 2)),
        Some("s"),
        Status::Info,
        &mut rows,
    );
    n(
        "Remaining",
        v.remaining_s.map(|x| round(x, 2)),
        Some("s"),
        Status::Info,
        &mut rows,
    );
    reading("Run", rows)
}

/// **Law**: the law's declared values on its last call, and its counters.
pub fn law_reading(v: &SineView) -> Value {
    let mut rows = Vec::new();
    if let Some(l) = &v.law {
        for (key, label, unit) in &l.labels {
            if let Some((_, x)) = l.values.iter().find(|(k, _)| k == key) {
                let unit = (!unit.is_empty()).then_some(unit.as_str());
                rows.push(Row::number(label, sig(*x, 6), unit, Status::Info));
            }
        }
        rows.push(Row::number(
            "Unusable calls in a row",
            f64::from(l.misses),
            None,
            if l.misses == 0 {
                Status::Pass
            } else {
                Status::Warn
            },
        ));
        rows.push(Row::number(
            "Ticks a cap bound",
            l.capped_ticks as f64,
            None,
            Status::Info,
        ));
    }
    reading("Law", rows)
}

/// The `sine.live` frame: the numbers and the Control, Drive and Run views.
pub fn live_json(v: &SineView, arrays: &mut Arrays) -> Value {
    let mut m = live_head(v);
    let empty = Figures::default();
    let f = v.figures.as_deref().unwrap_or(&empty);
    m.insert(
        "views".into(),
        Value::Array(vec![
            control_view(f, &v.control_unit, arrays),
            drive_view(f, arrays),
            run_reading(v),
        ]),
    );
    Value::Object(m)
}

/// The run's view right after configure.
pub fn configured_view(
    run_id: &str,
    w: &WireSine,
    grid: Grid,
    station: StationRef,
    law: Option<LawState>,
) -> SineView {
    SineView {
        run_id: run_id.to_owned(),
        title: w.title.clone(),
        state: State::Configured,
        phase: "idle",
        station,
        grid,
        clock: None,
        test: w.test.kind(),
        mode: w.mode,
        levels_db: w.levels_db.clone(),
        level_db: None,
        rung: None,
        frequency_hz: None,
        direction: None,
        sweeps: None,
        held: false,
        elapsed_s: None,
        at_level_s: None,
        remaining_s: None,
        alarm: false,
        notch: false,
        identification: None,
        law,
        feasibility: None,
        end: None,
        t_s: None,
        rate: Rate::OctPerMin(0.0),
        achieved_db: None,
        target: None,
        measured: None,
        drive_vpk: None,
        cola_vpk: None,
        channels: Vec::new(),
        alarms: Vec::new(),
        control_unit: "g".into(),
        streaming: false,
        figures: None,
        record: None,
    }
}

/// A run's law state before it is built: its identity, the live parameters'
/// values the run starts with, and the labels of its declared values.
pub fn law_state(
    law: LawRef,
    live: Vec<(String, f64)>,
    labels: Vec<(String, String, String)>,
) -> LawState {
    LawState {
        law,
        misses: 0,
        capped_ticks: 0,
        live,
        values: Vec::new(),
        labels,
        built: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reached_series_carries_only_the_points_the_run_has_been_at() {
        let f = Figures {
            x: vec![1.0, 2.0, 3.0],
            visited: vec![false, true, true],
            measured: vec![f64::NAN, 0.5, f64::NAN],
            ..Figures::default()
        };
        assert_eq!(reached(&f, &f.measured), (vec![2.0], vec![0.5]));
    }

    #[test]
    fn the_error_is_the_measured_against_the_target_in_db() {
        let mut v = configured_view(
            "sine-0001",
            &super::super::config::parse(&json!({
                "profile": {"rows": [{"freq_hz": 20, "quantity": "accel_g", "value": 1},
                                     {"freq_hz": 2000, "quantity": "accel_g", "value": 1}]},
                "test": {"kind": "dwell", "freq_hz": 100, "end": {"seconds": 1}},
                "limits": {"drive_vpk_ceiling": 0.5, "drive_vpk_abort": 0.8},
            }))
            .unwrap(),
            Grid::of(16384.0, 10.0, false),
            StationRef {
                kind: "sim",
                class: "sim",
                devices: Vec::new(),
            },
            None,
        );
        assert_eq!(error_db(&v), None);
        v.measured = Some(2.0);
        v.target = Some(1.0);
        assert!((error_db(&v).unwrap() - 6.0206).abs() < 1e-3);
    }
}
