//! What a MIMO Shock run shows (schema/PROTOCOL.md, "MIMO Shock Run State",
//! "Pulses" and "Display"): the run view the run thread publishes, its state and
//! live frames, a pulse's row, and the display views.

use crate::codec::Arrays;
use crate::protocol::num;
use crate::random::view::State;
use crate::shock::view::{sig, time_series, StationRef};
use crate::views::{self, Axis, Role, Row as ReadingRow, Scale, Series, Status};
use hertzforge_shock_common::ShockRunMode;
use serde_json::{json, Map, Value};

/// The run's law as the state names it.
#[derive(Debug, Clone, PartialEq)]
pub struct LawRef {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Empty for a built-in law.
    pub sha256: String,
    /// The parameter values in force, by key.
    pub params: Value,
}

/// The pulse counts.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Counts {
    pub fired: u64,
    pub passed: u64,
    pub at_level: u32,
    pub of_level: u32,
}

impl Counts {
    pub fn to_json(&self) -> Value {
        json!({"fired": self.fired, "passed": self.passed, "at_level": self.at_level,
               "of_level": self.of_level})
    }
}

/// How the run ended.
#[derive(Debug, Clone, PartialEq)]
pub struct End {
    pub outcome: &'static str,
    pub reason: String,
    pub message: String,
    pub at_s: Option<f64>,
    pub level_db: Option<f64>,
    pub pulse: Option<u64>,
}

impl End {
    pub fn to_json(&self) -> Value {
        json!({
            "outcome": self.outcome,
            "reason": self.reason,
            "message": self.message,
            "at_s": self.at_s.map_or(Value::Null, |x| num(r(x, 2))),
            "level_db": self.level_db.map_or(Value::Null, |x| num(r(x, 2))),
            "pulse": self.pulse,
        })
    }
}

/// What a run publishes after every step.
#[derive(Debug, Clone, PartialEq)]
pub struct MsView {
    pub run_id: String,
    pub title: String,
    pub state: State,
    pub phase: &'static str,
    pub station: StationRef,
    /// The simulator's plant (null on NI).
    pub plant: Option<String>,
    pub fs_hz: f64,
    pub record_len: usize,
    pub actual: bool,
    pub clock: Option<&'static str>,
    pub target: &'static str,
    pub timing: &'static str,
    pub verdict: &'static str,
    /// Level-scaled abort lines on.
    pub scaled_aborts: bool,
    pub mode: ShockRunMode,
    pub levels_db: Vec<f64>,
    pub level_db: Option<f64>,
    pub rung: Option<usize>,
    pub pulses: Counts,
    pub next_pulse_s: Option<f64>,
    pub elapsed_s: Option<f64>,
    pub identification: Option<Value>,
    pub review: Option<Value>,
    pub last_pulse: Option<Value>,
    pub law: LawRef,
    pub misses: u32,
    pub end: Option<End>,
    /// Armed at least once: the live frame streams.
    pub streaming: bool,
}

/// The standing notice of a run whose level-scaled abort lines are off.
pub const SCALED_OFF_NOTICE: &str = "Level-scaled aborts are off: a pulse over its level's abort \
    line is flagged and judged failing, and only the full-level (absolute) lines end the run.";

/// Round to `digits` decimals.
pub fn r(x: f64, digits: i32) -> f64 {
    views::round_to(x, digits)
}

fn opt(x: Option<f64>, digits: i32) -> Value {
    x.map_or(Value::Null, |x| num(r(x, digits)))
}

pub fn mode_name(m: ShockRunMode) -> &'static str {
    match m {
        ShockRunMode::Auto => "auto",
        ShockRunMode::Manual => "manual",
    }
}

/// The `mimo_shock.state` object (`None`: unconfigured).
pub fn state_json(v: Option<&MsView>) -> Value {
    let Some(v) = v else {
        return json!({
            "run_id": null, "state": "unconfigured", "phase": "idle", "title": null,
            "source": null, "station": null, "grid": null, "clock": null, "target": null,
            "timing": null, "verdict": null, "scaled_aborts": null, "notice": null,
            "mode": null, "levels_db": null,
            "level_db": null, "rung": null, "pulses": Counts::default().to_json(),
            "next_pulse_s": null, "elapsed_s": null, "identification": null, "review": null,
            "last_pulse": null, "law": null, "end": null,
        });
    };
    json!({
        "run_id": v.run_id,
        "state": v.state.name(),
        "phase": v.phase,
        "title": v.title,
        "source": v.station.source(),
        "station": {
            "kind": v.station.kind,
            "class": v.station.class,
            "devices": v.station.devices.iter()
                .map(|(id, sn)| json!({"id": id, "serial": sn})).collect::<Vec<_>>(),
            "plant": v.plant,
        },
        "grid": {"fs_hz": num(r(v.fs_hz, 3)), "record_len": v.record_len,
                 "lines": v.record_len / 2 + 1, "actual": v.actual},
        "clock": v.clock,
        "target": v.target,
        "timing": v.timing,
        "verdict": v.verdict,
        "scaled_aborts": v.scaled_aborts,
        "notice": if v.scaled_aborts { Value::Null } else { Value::from(SCALED_OFF_NOTICE) },
        "mode": mode_name(v.mode),
        "levels_db": v.levels_db.iter().map(|x| num(*x)).collect::<Vec<_>>(),
        "level_db": opt(v.level_db, 2),
        "rung": v.rung,
        "pulses": v.pulses.to_json(),
        "next_pulse_s": opt(v.next_pulse_s, 2),
        "elapsed_s": opt(v.elapsed_s, 2),
        "identification": v.identification.clone().unwrap_or(Value::Null),
        "review": v.review.clone().unwrap_or(Value::Null),
        "last_pulse": v.last_pulse.clone().unwrap_or(Value::Null),
        "law": {
            "id": v.law.id,
            "name": v.law.name,
            "version": v.law.version,
            "sha256": if v.law.sha256.is_empty() { Value::Null } else { Value::from(v.law.sha256.as_str()) },
            "params": v.law.params,
            "misses": v.misses,
        },
        "end": v.end.as_ref().map_or(Value::Null, End::to_json),
    })
}

/// What decides whether a `mimo_shock.state` event is due: everything but the
/// clocks.
pub fn state_key(v: Option<&MsView>) -> String {
    let mut s = state_json(v);
    if let Value::Object(m) = &mut s {
        m.remove("next_pulse_s");
        m.remove("elapsed_s");
    }
    s.to_string()
}

/// The `mimo_shock.live` data.
pub fn live_json(v: &MsView) -> Value {
    json!({
        "run_id": v.run_id,
        "source": v.station.source(),
        "state": v.state.name(),
        "phase": v.phase,
        "mode": mode_name(v.mode),
        "level_db": opt(v.level_db, 2),
        "rung": v.rung,
        "pulses": v.pulses.to_json(),
        "next_pulse_s": opt(v.next_pulse_s, 2),
    })
}

// ---------------------------------------------------------------------------------
// One pulse's figures and its views.
// ---------------------------------------------------------------------------------

/// One control's SRS figures at the level fired.
#[derive(Debug, Clone, PartialEq)]
pub struct SrsFigures {
    pub freqs_hz: Vec<f64>,
    pub target: Vec<f64>,
    pub hi: Vec<f64>,
    pub lo: Vec<f64>,
    pub measured: Vec<f64>,
}

/// One control's figures: the record from its start, at the level fired.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlFigures {
    pub channel: u32,
    /// The control's unit (`g`).
    pub unit: String,
    /// The target at the level fired, moved by the control's delay.
    pub target: Vec<f64>,
    /// The tolerance band (a waveform target).
    pub band: Option<(Vec<f64>, Vec<f64>)>,
    pub measured: Vec<f64>,
    pub srs: Option<SrsFigures>,
}

/// One pulse's figures.
#[derive(Debug, Clone, PartialEq)]
pub struct PulseFigures {
    pub fs_hz: f64,
    pub controls: Vec<ControlFigures>,
    /// Each drive's channel and record as played.
    pub drives: Vec<(u32, Vec<f64>)>,
}

fn time(n: usize, fs_hz: f64) -> Vec<f64> {
    (0..n).map(|i| i as f64 / fs_hz).collect()
}

fn time_axis() -> Axis {
    Axis::new("Time", Some("s"), Scale::Linear)
}

fn with_channel(mut v: Value, channel: u32) -> Value {
    if let Value::Object(m) = &mut v {
        m.insert("channel".into(), Value::from(channel));
    }
    v
}

/// **Pulse** *n*: the target at the level fired (moved by the delay), its
/// tolerance band, and the measured response.
pub fn pulse_view(
    c: &ControlFigures,
    fs_hz: f64,
    points: Option<usize>,
    arrays: &mut Arrays,
) -> Value {
    let t = time(c.target.len(), fs_hz);
    let mut series = vec![time_series(
        "Target",
        Role::Reference,
        t.clone(),
        &c.target,
        points,
    )];
    if let Some((hi, lo)) = &c.band {
        series.push(time_series(
            "Tolerance high",
            Role::Tolerance,
            t.clone(),
            hi,
            points,
        ));
        series.push(time_series(
            "Tolerance low",
            Role::Tolerance,
            t.clone(),
            lo,
            points,
        ));
    }
    series.push(time_series(
        "Measured",
        Role::Measured,
        time(c.measured.len(), fs_hz),
        &c.measured,
        points,
    ));
    let v = views::spectrum(
        arrays,
        &format!("Pulse {}", c.channel),
        &time_axis(),
        &Axis::new("Acceleration", Some(&c.unit), Scale::Linear),
        series,
        Some("time"),
    );
    with_channel(v, c.channel)
}

/// **SRS** *n*: the measured SRS against the target and its tolerance.
pub fn srs_view(channel: u32, unit: &str, s: &SrsFigures, arrays: &mut Arrays) -> Value {
    let x = || s.freqs_hz.clone();
    let v = views::spectrum(
        arrays,
        &format!("SRS {channel}"),
        &Axis::new("Natural frequency", Some("Hz"), Scale::Log),
        &Axis::new("Peak acceleration", Some(unit), Scale::Log),
        vec![
            Series::new("Target", Role::Reference, x(), s.target.clone()),
            Series::new("Tolerance high", Role::Tolerance, x(), s.hi.clone()),
            Series::new("Tolerance low", Role::Tolerance, x(), s.lo.clone()),
            Series::new("Measured", Role::Measured, x(), s.measured.clone()),
        ],
        Some("frequency"),
    );
    with_channel(v, channel)
}

/// **Drives**: every drive as played.
pub fn drives_view(
    drives: &[(u32, Vec<f64>)],
    fs_hz: f64,
    points: Option<usize>,
    arrays: &mut Arrays,
) -> Value {
    views::spectrum(
        arrays,
        "Drives",
        &time_axis(),
        &Axis::new("Drive", Some("V"), Scale::Linear),
        drives
            .iter()
            .map(|(ch, d)| {
                time_series(
                    &format!("Drive {ch}"),
                    Role::Drive,
                    time(d.len(), fs_hz),
                    d,
                    points,
                )
            })
            .collect(),
        Some("time"),
    )
}

/// A pulse's views: per control **Pulse** *n* (and **SRS** *n*), then **Drives**.
pub fn pulse_views(f: &PulseFigures, points: Option<usize>, arrays: &mut Arrays) -> Vec<Value> {
    let mut out = Vec::new();
    for c in &f.controls {
        out.push(pulse_view(c, f.fs_hz, points, arrays));
        if let Some(s) = &c.srs {
            out.push(srs_view(c.channel, &c.unit, s, arrays));
        }
    }
    out.push(drives_view(&f.drives, f.fs_hz, points, arrays));
    out
}

/// The identified plant for the display.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentFigures {
    pub freqs_hz: Vec<f64>,
    pub controls: Vec<u32>,
    pub drives: Vec<u32>,
    pub units: Vec<String>,
    /// `abs(H)`: per control, per drive, per line.
    pub gain: Vec<Vec<Vec<f64>>>,
    /// Per control, per line.
    pub coherence: Vec<Vec<f64>>,
    pub floor: f64,
}

/// **Identification Gain** *n* for the control at `c`, and **Coherence**.
pub fn identification_views(f: &IdentFigures, c: usize, arrays: &mut Arrays) -> Vec<Value> {
    let freq = Axis::new("Frequency", Some("Hz"), Scale::Log);
    let mut out = Vec::new();
    if let Some(rows) = f.gain.get(c) {
        let unit = format!("{}/V", f.units.get(c).map_or("g", String::as_str));
        let v = views::spectrum(
            arrays,
            &format!("Identification Gain {}", f.controls[c]),
            &freq,
            &Axis::new("Gain", Some(&unit), Scale::Log),
            rows.iter()
                .zip(&f.drives)
                .map(|(g, d)| {
                    Series::new(
                        &format!("Drive {d}"),
                        Role::Measured,
                        f.freqs_hz.clone(),
                        g.clone(),
                    )
                })
                .collect(),
            Some("frequency"),
        );
        out.push(with_channel(v, f.controls[c]));
    }
    let mut series: Vec<Series> = f
        .coherence
        .iter()
        .zip(&f.controls)
        .map(|(g, ch)| {
            Series::new(
                &format!("Control {ch}"),
                Role::Measured,
                f.freqs_hz.clone(),
                g.clone(),
            )
        })
        .collect();
    if let (Some(a), Some(b)) = (f.freqs_hz.first(), f.freqs_hz.last()) {
        series.push(Series::new(
            "Floor",
            Role::Tolerance,
            vec![*a, *b],
            vec![f.floor, f.floor],
        ));
    }
    out.push(views::spectrum(
        arrays,
        "Coherence",
        &freq,
        &Axis::new("Coherence", None, Scale::Linear),
        series,
        Some("frequency"),
    ));
    out
}

fn words(x: f64, digits: i32) -> String {
    num(r(x, digits)).to_string()
}

/// A `table` view.
pub fn table(title: &str, columns: &[&str], rows: Vec<Vec<String>>) -> Value {
    let mut m = Map::new();
    m.insert("view".into(), Value::from("table"));
    m.insert("title".into(), Value::from(title));
    m.insert(
        "columns".into(),
        Value::from(columns.iter().map(|c| c.to_string()).collect::<Vec<_>>()),
    );
    m.insert(
        "rows".into(),
        Value::Array(rows.into_iter().map(Value::from).collect()),
    );
    Value::Object(m)
}

/// **Verdicts**: each control's judgement of a pulse row.
pub fn verdicts_view(row: Option<&Value>) -> Value {
    let rows = row
        .and_then(|r| r["controls"].as_array())
        .map(|cs| {
            cs.iter()
                .map(|c| {
                    let f = |k: &str| c[k].as_f64().unwrap_or(f64::NAN);
                    vec![
                        c["channel"].to_string(),
                        if c["passed"].as_bool() == Some(true) {
                            "pass"
                        } else {
                            "fail"
                        }
                        .to_owned(),
                        c["reasons"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default(),
                        words(f("error_db"), 2),
                        words(f("over_test_db"), 2),
                        words(1e3 * f("delay_s"), 2),
                        words(f("peak_ratio"), 3),
                    ]
                })
                .collect()
        })
        .unwrap_or_default();
    table(
        "Verdicts",
        &[
            "Control",
            "Passed",
            "Failed on",
            "Error (dB)",
            "Over-test (dB)",
            "Delay (ms)",
            "Peak ratio",
        ],
        rows,
    )
}

/// **Ladder**: the levels, the pulses fired and passed at each, the level in force.
pub fn ladder_view(levels: &[f64], rows: &[Value], now: Option<f64>) -> Value {
    let out = levels
        .iter()
        .map(|l| {
            let at: Vec<&Value> = rows
                .iter()
                .filter(|p| p["level_db"].as_f64().is_some_and(|x| (x - l).abs() < 1e-9))
                .collect();
            let passed = at.iter().filter(|p| p["passed"] == true).count();
            vec![
                num(*l).to_string(),
                at.len().to_string(),
                passed.to_string(),
                if now.is_some_and(|n| (n - l).abs() < 1e-9) {
                    "in force".into()
                } else {
                    String::new()
                },
            ]
        })
        .collect();
    table("Ladder", &["Level (dB)", "Fired", "Passed", "Now"], out)
}

/// **Review**: each control's reachable share, floor and the seed's prediction.
pub fn review_view(review: Option<&Value>) -> Value {
    let rows = review
        .and_then(|r| r["controls"].as_array())
        .map(|cs| {
            cs.iter()
                .map(|c| {
                    vec![
                        c["channel"].to_string(),
                        words(100.0 * c["reachable"].as_f64().unwrap_or(f64::NAN), 2),
                        words(c["floor_db"].as_f64().unwrap_or(f64::NAN), 2),
                        if c["reachable_ok"] == true {
                            "yes"
                        } else {
                            "no"
                        }
                        .into(),
                        c["predicted_peak_ratio"]
                            .as_f64()
                            .map_or(String::new(), |x| words(x, 3)),
                    ]
                })
                .collect()
        })
        .unwrap_or_default();
    table(
        "Review",
        &[
            "Control",
            "Reachable (%)",
            "Floor (dB)",
            "Reachable",
            "Predicted peak ratio",
        ],
        rows,
    )
}

/// **Pulses**: one row per pulse.
pub fn pulses_view(rows: &[Value]) -> Value {
    let out = rows
        .iter()
        .map(|p| {
            let worst = p["controls"]
                .as_array()
                .and_then(|cs| {
                    cs.iter().max_by(|a, b| {
                        let k = |c: &Value| c["over_test_db"].as_f64().unwrap_or(f64::MIN);
                        k(a).total_cmp(&k(b))
                    })
                })
                .map_or(String::new(), |c| c["channel"].to_string());
            let drive = p["drives"]
                .as_array()
                .map(|d| {
                    d.iter()
                        .filter_map(|x| x["vpk"].as_f64())
                        .fold(0.0f64, f64::max)
                })
                .unwrap_or(0.0);
            vec![
                (p["index"].as_u64().unwrap_or(0) + 1).to_string(),
                p["level_db"].to_string(),
                if p["passed"] == true { "pass" } else { "fail" }.into(),
                worst,
                p["over_test_db"].to_string(),
                p["global_srs_error_db"]
                    .as_f64()
                    .map_or(String::new(), |g| words(g, 2)),
                words(drive, 4),
            ]
        })
        .collect();
    table(
        "Pulses",
        &[
            "Pulse",
            "Level (dB)",
            "Passed",
            "Worst control",
            "Over-test (dB)",
            "Global SRS error (dB)",
            "Drive peak (V)",
        ],
        out,
    )
}

/// One value a law reported, with its label and unit.
#[derive(Debug, Clone, PartialEq)]
pub struct LawReading {
    pub label: String,
    pub value: f64,
    pub unit: String,
}

/// **Law**: its named values on its last call.
pub fn law_view(values: &[LawReading]) -> Value {
    views::reading(
        "Law",
        values
            .iter()
            .map(|v| {
                ReadingRow::number(
                    &v.label,
                    v.value,
                    (!v.unit.is_empty()).then_some(v.unit.as_str()),
                    Status::Info,
                )
            })
            .collect(),
    )
}

/// **Run**: the run's numbers.
pub fn readings_view(v: &MsView) -> Value {
    let mut rows = Vec::new();
    if let Some(l) = v.level_db {
        rows.push(ReadingRow::number(
            "Level",
            r(l, 2),
            Some("dB"),
            Status::Info,
        ));
    }
    rows.push(ReadingRow::number(
        "Pulses fired",
        v.pulses.fired as f64,
        None,
        Status::Info,
    ));
    rows.push(ReadingRow::number(
        "Pulses passed",
        v.pulses.passed as f64,
        None,
        Status::Info,
    ));
    if let Some(p) = &v.last_pulse {
        if let Some(o) = p["over_test_db"].as_f64() {
            rows.push(ReadingRow::number(
                "Last over-test",
                r(o, 2),
                Some("dB"),
                if p["passed"] == true {
                    Status::Pass
                } else {
                    Status::Fail
                },
            ));
        }
    }
    if let Some(s) = v.next_pulse_s {
        rows.push(ReadingRow::number(
            "Next pulse",
            r(s, 2),
            Some("s"),
            Status::Info,
        ));
    }
    views::reading("Run", rows)
}

/// A number rounded to `digits` significant figures, for the rows.
pub fn sg(x: f64, digits: i32) -> Value {
    num(sig(x, digits))
}
