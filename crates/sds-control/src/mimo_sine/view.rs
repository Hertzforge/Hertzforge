//! What a MIMO Sine run shows (schema/PROTOCOL.md, "MIMO Sine Run State" and
//! "MIMO Sine Display"): the state object, the live frame and its views, the
//! sweep frame and the report, built from the [`MsView`] the run thread
//! publishes.

use super::config::mode_name;
use crate::sine::config::round;
use crate::codec::Arrays;
use crate::protocol::num;
use crate::random::view::{RecordRef, State};
use crate::sine::config::Grid;
use crate::sine::view::StationRef;
use crate::views::{self, Axis, Role, Row, Scale, Series, Status};
use hertzforge_mimo_sine::LevelMode;
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// Points of the run's sweep grid across the band.
pub const GRID_POINTS: usize = 256;
/// The span a dwell's figures hold, s.
pub const TIME_SPAN_S: f64 = 60.0;
/// The most points a dwell's figures hold.
pub const TIME_POINTS: usize = 1200;

/// One channel the run reads or drives, as shown.
#[derive(Debug, Clone, PartialEq)]
pub struct Chan {
    pub channel: u32,
    pub description: String,
    pub unit: String,
}

/// How the run ended.
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

impl End {
    pub fn to_json(&self) -> Value {
        json!({
            "outcome": self.outcome,
            "message": self.message,
            "at_s": self.at_s.map_or(Value::Null, |s| num(round(s, 3))),
            "level_db": self.level_db.map_or(Value::Null, |l| num(round(l, 2))),
            "at_hz": self.at_hz.map_or(Value::Null, |f| num(round(f, 3))),
            "ramp_s": self.ramp_s.map_or(Value::Null, |s| num(round(s, 3))),
            "trip": self.trip.clone().unwrap_or(Value::Null),
        })
    }
}

/// The law's counts as the state shows them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LawCounts {
    pub misses: u32,
    pub capped_ticks: u64,
    pub over_ceiling: u64,
    pub rotation_max_deg: f64,
    pub asked: u64,
    pub granted: u64,
}

/// One tick as the live frame shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Live {
    pub t_s: f64,
    /// Per control: amplitude, phase deg, reference, reference phase deg,
    /// error dB, phase error deg, notch depth dB.
    pub controls: Vec<[f64; 7]>,
    /// Per limit channel: amplitude, phase deg, bound, margin dB.
    pub limits: Vec<[f64; 4]>,
    /// Per drive: amplitude V, phase deg.
    pub drives: Vec<[f64; 2]>,
    pub opposition_ratio: f64,
    pub largest_share: f64,
    pub rotation_deg: f64,
    /// The law's request: `hold` or `slow` with its factor, and whether granted.
    pub request: Option<(&'static str, Option<f64>, bool)>,
    /// The law's values by declared index.
    pub values: Vec<f64>,
    pub law_faulted: bool,
    pub sweep_hz_per_s: f64,
    pub sweep_oct_per_min: f64,
    pub commanded_db: f64,
}

/// The figures the views draw: per point of the sweep grid (or of time, in a
/// dwell), the latest tick there.
#[derive(Debug, Clone, PartialEq)]
pub struct Figures {
    pub against_time: bool,
    /// The x of each point: the sweep grid, or the time of each kept point.
    pub x: Vec<f64>,
    /// Per control, per point: amplitude, phase, reference, reference phase,
    /// error, notch depth.
    pub control: Vec<Vec<[f64; 6]>>,
    /// Per limit channel, per point: amplitude, bound.
    pub limit: Vec<Vec<[f64; 2]>>,
    /// Per drive, per point: amplitude, phase.
    pub drive: Vec<Vec<[f64; 2]>>,
    /// Per point: opposition ratio, largest share.
    pub readings: Vec<[f64; 2]>,
}

impl Figures {
    /// Empty figures on the run's sweep grid (`band` in Hz, the steps of a
    /// stepped sine when given) or against time.
    pub fn new(
        band: (f64, f64),
        steps: Option<&[f64]>,
        against_time: bool,
        n: usize,
        m: usize,
        p: usize,
    ) -> Figures {
        let x: Vec<f64> = if against_time {
            Vec::new()
        } else if let Some(s) = steps {
            let mut v = s.to_vec();
            v.sort_by(|a, b| a.total_cmp(b));
            v.dedup();
            v
        } else {
            sweep_grid(band)
        };
        let k = x.len();
        Figures {
            against_time,
            control: vec![vec![[f64::NAN; 6]; k]; n],
            limit: vec![vec![[f64::NAN; 2]; k]; p],
            drive: vec![vec![[f64::NAN; 2]; k]; m],
            readings: vec![[f64::NAN; 2]; k],
            x,
        }
    }

    /// Put `live` at frequency `f_hz` (or at its time in a dwell).
    pub fn put(&mut self, f_hz: f64, live: &Live, settling: bool) {
        let k = if self.against_time {
            let step = TIME_SPAN_S / TIME_POINTS as f64;
            if let Some(last) = self.x.last() {
                if live.t_s - last < step {
                    return;
                }
            }
            self.x.push(live.t_s);
            for c in &mut self.control {
                c.push([f64::NAN; 6]);
            }
            for l in &mut self.limit {
                l.push([f64::NAN; 2]);
            }
            for d in &mut self.drive {
                d.push([f64::NAN; 2]);
            }
            self.readings.push([f64::NAN; 2]);
            // The last span only.
            let keep_from = self.x.partition_point(|t| *t < live.t_s - TIME_SPAN_S);
            if keep_from > 0 {
                self.x.drain(..keep_from);
                self.control.iter_mut().for_each(|c| {
                    c.drain(..keep_from);
                });
                self.limit.iter_mut().for_each(|c| {
                    c.drain(..keep_from);
                });
                self.drive.iter_mut().for_each(|c| {
                    c.drain(..keep_from);
                });
                self.readings.drain(..keep_from);
            }
            self.x.len() - 1
        } else {
            if self.x.is_empty() || settling {
                return;
            }
            nearest(&self.x, f_hz)
        };
        for (c, v) in self.control.iter_mut().zip(&live.controls) {
            c[k] = [v[0], v[1], v[2], v[3], v[4], v[6]];
        }
        for (l, v) in self.limit.iter_mut().zip(&live.limits) {
            l[k] = [v[0], v[2]];
        }
        for (d, v) in self.drive.iter_mut().zip(&live.drives) {
            d[k] = *v;
        }
        self.readings[k] = [live.opposition_ratio, live.largest_share];
    }
}

/// The run's sweep grid: [`GRID_POINTS`] log-spaced points across `band`
/// with its exact ends.
pub fn sweep_grid(band: (f64, f64)) -> Vec<f64> {
    let (lo, hi) = band;
    if hi <= lo {
        return vec![lo];
    }
    (0..GRID_POINTS)
        .map(|k| {
            if k == GRID_POINTS - 1 {
                hi
            } else {
                lo * (hi / lo).powf(k as f64 / (GRID_POINTS - 1) as f64)
            }
        })
        .collect()
}

fn nearest(x: &[f64], f: f64) -> usize {
    let p = x.partition_point(|v| *v < f);
    if p == 0 {
        return 0;
    }
    if p >= x.len() {
        return x.len() - 1;
    }
    if (f.ln() - x[p - 1].ln()).abs() <= (x[p].ln() - f.ln()).abs() {
        p - 1
    } else {
        p
    }
}

/// One finished pass's figures, for the sweep frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Pass {
    pub pass: u64,
    pub level_db: f64,
    pub direction: Option<&'static str>,
    pub figures: Figures,
}

/// A run's view, as published.
#[derive(Debug, Clone, PartialEq)]
pub struct MsView {
    pub run_id: String,
    pub title: String,
    pub state: State,
    pub phase: &'static str,
    pub station: StationRef,
    pub structure: Option<String>,
    pub grid: Grid,
    pub clock: Option<&'static str>,
    pub test: &'static str,
    pub shape: &'static str,
    pub phase_specified: bool,
    pub mode: LevelMode,
    pub levels_db: Vec<f64>,
    pub level_db: Option<f64>,
    pub rung: Option<usize>,
    pub frequency_hz: Option<f64>,
    pub direction: Option<&'static str>,
    pub sweeps: Option<(u64, Option<u64>)>,
    pub steps: Option<(u64, u64)>,
    pub held: bool,
    pub request_held: bool,
    pub elapsed_s: Option<f64>,
    pub at_level_s: Option<f64>,
    pub remaining_s: Option<f64>,
    pub alarm: bool,
    pub notch: bool,
    pub notch_scale: f64,
    pub identification: Option<Value>,
    pub review: Option<Value>,
    /// The law's identity and its parameters in force.
    pub law: Value,
    pub law_values: Vec<(String, String, String)>,
    pub law_built: bool,
    pub counts: LawCounts,
    pub feasibility: (&'static str, String),
    pub end: Option<End>,
    pub record: Option<RecordRef>,
    pub streaming: bool,
    pub controls: Vec<Chan>,
    pub limits: Vec<Chan>,
    pub drives: Vec<Chan>,
    pub ceilings: Vec<f64>,
    pub live: Option<Arc<Live>>,
    pub figures: Option<Arc<Figures>>,
    pub last_pass: Option<Arc<Pass>>,
    pub identification_figures: Option<Arc<IdentFigures>>,
    pub report: Option<Value>,
    /// The last samples read of every enabled input, by table index, and the
    /// rate: the `time` plot's.
    pub time: Option<Arc<TimeFigures>>,
}

/// The last samples of every enabled input.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeFigures {
    pub fs_hz: f64,
    pub inputs: Vec<(u32, Vec<f32>)>,
}

/// The identification as drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentFigures {
    pub freqs_hz: Vec<f64>,
    /// Per row (control then limit channel), per drive, per line: |H|, phase deg.
    pub gain: Vec<Vec<Vec<f64>>>,
    pub phase: Vec<Vec<Vec<f64>>>,
    /// Per row, per line: multiple coherence.
    pub coherence: Vec<Vec<f64>>,
    pub trust_coherence: f64,
}

fn opt(x: Option<f64>, digits: i32) -> Value {
    x.map_or(Value::Null, |v| num(round(v, digits)))
}

/// The state object (`mimo_sine.state`); `None` when nothing is configured.
pub fn state_json(v: Option<&MsView>) -> Value {
    let Some(v) = v else {
        return json!({
            "run_id": null, "state": "unconfigured", "phase": "idle", "title": "",
            "source": null, "station": null, "grid": null, "clock": null, "test": null,
            "shape": null, "phase_specified": null, "mode": null, "levels_db": [],
            "level_db": null, "rung": null, "frequency_hz": null, "direction": null,
            "sweeps": null, "steps": null, "held": false, "request_held": false,
            "elapsed_s": null, "at_level_s": null, "remaining_s": null, "alarm": false,
            "notch": false, "notch_scale": 1, "identification": null, "review": null,
            "law": null, "feasibility": null, "end": null, "record": null,
        });
    };
    let mut station = v.station.to_json();
    if let Value::Object(m) = &mut station {
        m.insert(
            "structure".into(),
            v.structure.clone().map_or(Value::Null, Value::from),
        );
    }
    let mut grid = v.grid.to_json();
    if let Value::Object(m) = &mut grid {
        m.insert("actual".into(), Value::from(v.grid.actual));
    }
    let mut law = v.law.clone();
    if let Value::Object(m) = &mut law {
        m.insert("misses".into(), Value::from(v.counts.misses));
        m.insert("capped_ticks".into(), Value::from(v.counts.capped_ticks));
        m.insert("over_ceiling".into(), Value::from(v.counts.over_ceiling));
        m.insert(
            "rotation_max_deg".into(),
            num(round(v.counts.rotation_max_deg, 2)),
        );
        m.insert(
            "requests".into(),
            json!({"asked": v.counts.asked, "granted": v.counts.granted}),
        );
    }
    json!({
        "run_id": v.run_id,
        "state": v.state.name(),
        "phase": v.phase,
        "title": v.title,
        "source": v.station.source(),
        "station": station,
        "grid": grid,
        "clock": v.clock,
        "test": v.test,
        "shape": v.shape,
        "phase_specified": v.phase_specified,
        "mode": mode_name(v.mode),
        "levels_db": v.levels_db.iter().map(|l| num(*l)).collect::<Vec<_>>(),
        "level_db": opt(v.level_db, 2),
        "rung": v.rung,
        "frequency_hz": opt(v.frequency_hz, 3),
        "direction": v.direction,
        "sweeps": v.sweeps.map(|(d, of)| json!({"done": d, "of": of})),
        "steps": v.steps.map(|(d, of)| json!({"done": d, "of": of})),
        "held": v.held,
        "request_held": v.request_held,
        "elapsed_s": opt(v.elapsed_s, 2),
        "at_level_s": opt(v.at_level_s, 2),
        "remaining_s": opt(v.remaining_s, 1),
        "alarm": v.alarm,
        "notch": v.notch,
        "notch_scale": num(round(v.notch_scale, 4)),
        "identification": v.identification.clone().unwrap_or(Value::Null),
        "review": v.review.clone().unwrap_or(Value::Null),
        "law": law,
        "feasibility": {"verdict": v.feasibility.0, "message": v.feasibility.1},
        "end": v.end.as_ref().map_or(Value::Null, End::to_json),
        "record": v.record.as_ref().map_or(Value::Null, RecordRef::to_json),
    })
}

/// What a `mimo_sine.state` event is sent for: a change of any of these.
pub fn state_key(v: Option<&MsView>) -> String {
    match v {
        None => "none".into(),
        Some(v) => format!(
            "{}|{}|{}|{:?}|{}|{}|{}|{}|{}|{}|{}|{}",
            v.run_id,
            v.state.name(),
            v.phase,
            v.mode,
            v.level_db.map_or(f64::NAN, |l| round(l, 2)),
            v.held,
            v.request_held,
            v.alarm,
            v.notch,
            v.end.is_some(),
            v.identification.is_some(),
            v.record.as_ref().and_then(|r| r.problem.clone()).unwrap_or_default(),
        ),
    }
}

fn deg(x: f64) -> Value {
    num(round(x, 2))
}

fn x_axis(f: &Figures) -> (Axis, &'static str) {
    if f.against_time {
        (Axis::new("Time", Some("s"), Scale::Linear), "time")
    } else {
        (Axis::new("Frequency", Some("Hz"), Scale::Log), "frequency")
    }
}

/// Points of `series` with values (the run has reached them).
fn reached(x: &[f64], y: impl Iterator<Item = f64>) -> (Vec<f64>, Vec<f64>) {
    x.iter()
        .zip(y)
        .filter(|(_, y)| y.is_finite())
        .map(|(x, y)| (*x, y))
        .unzip()
}

fn series_of(name: &str, role: Role, x: &[f64], y: impl Iterator<Item = f64>) -> Series {
    let (x, y) = reached(x, y);
    Series::new(name, role, x, y)
}

/// The live frame's three views: Controls, Drives and Run.
pub fn live_views(v: &MsView, arrays: &mut Arrays) -> Vec<Value> {
    let Some(f) = v.figures.as_deref() else {
        return Vec::new();
    };
    let (x, link) = x_axis(f);
    let unit = v.controls.first().map_or("g", |c| c.unit.as_str());
    let mut controls = Vec::new();
    for (c, pts) in v.controls.iter().zip(&f.control) {
        controls.push(series_of(
            &format!("Control {}", c.channel),
            Role::Measured,
            &f.x,
            pts.iter().map(|p| p[0]),
        ));
        controls.push(series_of(
            &format!("Reference {}", c.channel),
            Role::Reference,
            &f.x,
            pts.iter().map(|p| p[2]),
        ));
    }
    let drives: Vec<Series> = v
        .drives
        .iter()
        .zip(&f.drive)
        .map(|(d, pts)| {
            series_of(
                &format!("Drive {}", d.channel),
                Role::Drive,
                &f.x,
                pts.iter().map(|p| p[0]),
            )
        })
        .collect();
    vec![
        views::spectrum(
            arrays,
            "Controls",
            &x,
            &Axis::new("Amplitude", Some(unit), Scale::Log),
            controls,
            Some(link),
        ),
        views::spectrum(
            arrays,
            "Drives",
            &x,
            &Axis::new("Drive", Some("V"), Scale::Log),
            drives,
            Some(link),
        ),
        run_reading(v),
    ]
}

/// The run's reading.
pub fn run_reading(v: &MsView) -> Value {
    let mut rows = vec![Row::number(
        "Frequency",
        round(v.frequency_hz.unwrap_or(f64::NAN), 2),
        Some("Hz"),
        Status::Info,
    )];
    if let Some(d) = v.direction {
        rows.push(Row::text("Direction", d, Status::Info));
    }
    rows.push(Row::number(
        "Level",
        round(v.level_db.unwrap_or(f64::NAN), 2),
        Some("dB"),
        Status::Info,
    ));
    rows.push(Row::number(
        "Notch",
        round(v.notch_scale, 3),
        None,
        if v.notch { Status::Warn } else { Status::Info },
    ));
    let live = v.live.as_deref();
    let drive_max = live
        .map(|l| l.drives.iter().map(|d| d[0]).fold(0.0f64, f64::max))
        .unwrap_or(f64::NAN);
    rows.push(Row::number(
        "Drive",
        round(drive_max, 4),
        Some("V"),
        Status::Info,
    ));
    rows.push(Row::number(
        "Opposition",
        round(live.map_or(f64::NAN, |l| l.opposition_ratio), 3),
        None,
        Status::Info,
    ));
    rows.push(Row::number(
        "Largest share",
        round(live.map_or(f64::NAN, |l| l.largest_share), 3),
        None,
        Status::Info,
    ));
    if let Some((d, of)) = v.sweeps {
        rows.push(Row::text(
            "Sweeps",
            &match of {
                Some(n) => format!("{d} of {n}"),
                None => d.to_string(),
            },
            Status::Info,
        ));
    }
    if let Some((d, of)) = v.steps {
        rows.push(Row::text("Steps", &format!("{d} of {of}"), Status::Info));
    }
    rows.push(Row::number(
        "Elapsed",
        round(v.elapsed_s.unwrap_or(f64::NAN), 1),
        Some("s"),
        Status::Info,
    ));
    rows.push(Row::number(
        "Remaining",
        round(v.remaining_s.unwrap_or(f64::NAN), 1),
        Some("s"),
        Status::Info,
    ));
    views::reading("Run", rows)
}

/// The live frame's members but `views`.
pub fn live_head(v: &MsView) -> Map<String, Value> {
    let live = v.live.as_deref();
    let mut m = Map::new();
    m.insert("run_id".into(), Value::from(v.run_id.as_str()));
    m.insert("source".into(), Value::from(v.station.source()));
    m.insert("class".into(), Value::from(v.station.class));
    m.insert("state".into(), Value::from(v.state.name()));
    m.insert("phase".into(), Value::from(v.phase));
    m.insert("test".into(), Value::from(v.test));
    m.insert("mode".into(), Value::from(mode_name(v.mode)));
    m.insert("t_s".into(), opt(live.map(|l| l.t_s), 3));
    m.insert("elapsed_s".into(), opt(v.elapsed_s, 2));
    m.insert("frequency_hz".into(), opt(v.frequency_hz, 3));
    m.insert(
        "direction".into(),
        v.direction.map_or(Value::Null, Value::from),
    );
    let linear = live.is_some_and(|l| l.sweep_oct_per_min == 0.0 && l.sweep_hz_per_s != 0.0);
    if v.test == "sweep" && linear {
        m.insert(
            "sweep_rate_hz_per_s".into(),
            opt(live.map(|l| l.sweep_hz_per_s.abs()), 4),
        );
    } else {
        m.insert(
            "sweep_rate_oct_per_min".into(),
            if v.test == "sweep" {
                opt(live.map(|l| l.sweep_oct_per_min.abs()), 4)
            } else {
                Value::Null
            },
        );
    }
    m.insert(
        "sweeps".into(),
        v.sweeps
            .map_or(Value::Null, |(d, of)| json!({"done": d, "of": of})),
    );
    m.insert(
        "steps".into(),
        v.steps
            .map_or(Value::Null, |(d, of)| json!({"done": d, "of": of})),
    );
    m.insert("held".into(), Value::from(v.held));
    m.insert("request_held".into(), Value::from(v.request_held));
    m.insert("level_db".into(), opt(v.level_db, 2));
    m.insert("commanded_db".into(), opt(live.map(|l| l.commanded_db), 2));
    m.insert("notch".into(), Value::from(v.notch));
    m.insert("notch_scale".into(), num(round(v.notch_scale, 4)));
    m.insert("alarm".into(), Value::from(v.alarm));
    let empty = Live::default();
    let l = live.unwrap_or(&empty);
    m.insert(
        "controls".into(),
        Value::Array(
            v.controls
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let x = l.controls.get(i).copied().unwrap_or([f64::NAN; 7]);
                    json!({"channel": c.channel, "description": c.description, "unit": c.unit,
                           "amplitude": num(sig(x[0])), "phase_deg": deg(x[1]),
                           "reference": num(sig(x[2])),
                           "reference_phase_deg": if v.phase_specified { deg(x[3]) } else { Value::Null },
                           "error_db": num(round(x[4], 3)),
                           "phase_error_deg": if v.phase_specified { deg(x[5]) } else { Value::Null },
                           "notch_depth_db": num(round(x[6], 3)), "overload": false})
                })
                .collect(),
        ),
    );
    m.insert(
        "limits".into(),
        Value::Array(
            v.limits
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let x = l.limits.get(i).copied().unwrap_or([f64::NAN; 4]);
                    json!({"channel": c.channel, "description": c.description, "unit": c.unit,
                           "amplitude": num(sig(x[0])), "phase_deg": deg(x[1]),
                           "bound": num(sig(x[2])), "margin_db": num(round(x[3], 3))})
                })
                .collect(),
        ),
    );
    m.insert(
        "drives".into(),
        Value::Array(
            v.drives
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let x = l.drives.get(i).copied().unwrap_or([f64::NAN; 2]);
                    json!({"channel": c.channel, "vpk": num(sig(x[0])), "phase_deg": deg(x[1]),
                           "ceiling_vpk": num(v.ceilings.get(i).copied().unwrap_or(f64::NAN))})
                })
                .collect(),
        ),
    );
    m.insert(
        "readings".into(),
        json!({"opposition_ratio": num(round(l.opposition_ratio, 4)),
               "largest_share": num(round(l.largest_share, 4)),
               "rotation_deg": num(round(l.rotation_deg, 3))}),
    );
    m.insert(
        "request".into(),
        match l.request {
            None => Value::Null,
            Some((kind, factor, granted)) => {
                let mut r = json!({"kind": kind, "granted": granted});
                if let Some(f) = factor {
                    r["factor"] = num(round(f, 4));
                }
                r
            }
        },
    );
    m.insert(
        "law".into(),
        if v.law_built {
            let mut values = Map::new();
            for (k, (key, _, _)) in v.law_values.iter().enumerate() {
                values.insert(
                    key.clone(),
                    num(sig(l.values.get(k).copied().unwrap_or(f64::NAN))),
                );
            }
            json!({"values": values, "misses": v.counts.misses,
                   "capped_ticks": v.counts.capped_ticks, "faulted": l.law_faulted})
        } else {
            Value::Null
        },
    );
    m
}

/// Five significant figures.
pub fn sig(x: f64) -> f64 {
    crate::sine::profile::sig(x, 5)
}

/// The live frame (`mimo_sine.live`).
pub fn live_json(v: &MsView, arrays: &mut Arrays) -> Value {
    let mut m = live_head(v);
    m.insert("views".into(), Value::Array(live_views(v, arrays)));
    Value::Object(m)
}

/// The views of one pass's (or the run's) figures against frequency, as the
/// sweep frame carries them.
pub fn pass_views(v: &MsView, f: &Figures, arrays: &mut Arrays) -> Vec<Value> {
    pass_views_where(v, f, arrays, &|_| true)
}

/// [`pass_views`], only the views whose title `want` takes.
pub fn pass_views_where(
    v: &MsView,
    f: &Figures,
    arrays: &mut Arrays,
    want: &dyn Fn(&str) -> bool,
) -> Vec<Value> {
    let (x, link) = x_axis(f);
    let mut out = Vec::new();
    for (c, pts) in v.controls.iter().zip(&f.control) {
        let title = &format!("Control {}", c.channel);
        if want(&title) {
            out.push(views::spectrum(
                arrays,
                &title,
                &x,
                &Axis::new("Amplitude", Some(&c.unit), Scale::Log),
                vec![
                    series_of("Measured", Role::Measured, &f.x, pts.iter().map(|p| p[0])),
                    series_of("Reference", Role::Reference, &f.x, pts.iter().map(|p| p[2])),
                ],
                Some(link),
            ));
        }
        if v.phase_specified {
            let title = &format!("Phase {}", c.channel);
            if want(&title) {
                out.push(views::spectrum(
                    arrays,
                    &title,
                    &x,
                    &Axis::new("Phase", Some("deg"), Scale::Linear),
                    vec![
                        series_of("Measured", Role::Measured, &f.x, pts.iter().map(|p| p[1])),
                        series_of("Reference", Role::Reference, &f.x, pts.iter().map(|p| p[3])),
                    ],
                    Some(link),
                ));
            }
        }
        let title = &format!("Error {}", c.channel);
        if want(&title) {
            out.push(views::spectrum(
                arrays,
                &title,
                &x,
                &Axis::new("Error", Some("dB"), Scale::Linear),
                vec![
                    series_of("Error", Role::Measured, &f.x, pts.iter().map(|p| p[4])),
                    series_of("Notch depth", Role::Limit, &f.x, pts.iter().map(|p| p[5])),
                ],
                Some(link),
            ));
        }
    }
    for (c, pts) in v.limits.iter().zip(&f.limit) {
        let title = &format!("Limit {}", c.channel);
        if want(&title) {
            out.push(views::spectrum(
                arrays,
                &title,
                &x,
                &Axis::new("Amplitude", Some(&c.unit), Scale::Log),
                vec![
                    series_of("Measured", Role::Measured, &f.x, pts.iter().map(|p| p[0])),
                    series_of("Bound", Role::Limit, &f.x, pts.iter().map(|p| p[1])),
                ],
                Some(link),
            ));
        }
    }
    let title = "Drives";
    if want(&title) {
        out.push(views::spectrum(
            arrays,
            &title,
            &x,
            &Axis::new("Drive", Some("V"), Scale::Log),
            v.drives
                .iter()
                .zip(&f.drive)
                .map(|(d, pts)| {
                    series_of(
                        &format!("Drive {}", d.channel),
                        Role::Drive,
                        &f.x,
                        pts.iter().map(|p| p[0]),
                    )
                })
                .collect(),
            Some(link),
        ));
    }
    let title = "Drive phase";
    if want(&title) {
        out.push(views::spectrum(
            arrays,
            &title,
            &x,
            &Axis::new("Phase", Some("deg"), Scale::Linear),
            v.drives
                .iter()
                .zip(&f.drive)
                .map(|(d, pts)| {
                    series_of(
                        &format!("Drive {}", d.channel),
                        Role::Drive,
                        &f.x,
                        pts.iter().map(|p| p[1]),
                    )
                })
                .collect(),
            Some(link),
        ));
    }
    let title = "Readings";
    if want(&title) {
        out.push(views::spectrum(
            arrays,
            &title,
            &x,
            &Axis::new("Reading", None, Scale::Linear),
            vec![
                series_of(
                    "Opposition",
                    Role::Other,
                    &f.x,
                    f.readings.iter().map(|r| r[0]),
                ),
                series_of(
                    "Largest share",
                    Role::Other,
                    &f.x,
                    f.readings.iter().map(|r| r[1]),
                ),
            ],
            Some(link),
        ));
    }
    out
}

/// The sweep frame (`mimo_sine.sweep`) of a finished pass.
pub fn sweep_json(v: &MsView, p: &Pass, arrays: &mut Arrays) -> Value {
    json!({
        "run_id": v.run_id,
        "pass": p.pass,
        "level_db": num(round(p.level_db, 2)),
        "direction": p.direction,
        "views": pass_views(v, &p.figures, arrays),
    })
}
