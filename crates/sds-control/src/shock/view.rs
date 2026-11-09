//! What a classical shock run shows: the `shock.state` object, the pulse rows, the
//! `shock.live` frame and the display views (schema/PROTOCOL.md, "Classical Shock
//! Run State", "Classical Shock Pulses" and "Classical Shock Display"), built from
//! a [`ShockView`] the run publishes after every step. Keys are written in the
//! contract's order.
//!
//! The view builders take figures already computed (the reference at a level, its
//! band, the measured record, the SRS and its band) and only lay them out, so the
//! same figures make the same bytes in every language the goldens are read in.

use super::config::Grid;
use crate::codec::Arrays;
use crate::protocol::{num, opt_num};
use crate::views::{self, Axis, Role, Row as ReadingRow, Scale, Series, Status};
use hertzforge_shock_common::ShockRunMode;
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub use crate::random::view::{State, StationRef};

fn r(x: f64, d: i32) -> f64 {
    super::config::round(x, d)
}

fn rn(x: f64, d: i32) -> Value {
    num(r(x, d))
}

fn opt_rn(x: Option<f64>, d: i32) -> Value {
    opt_num(x.map(|v| r(v, d)))
}

pub fn mode_name(m: ShockRunMode) -> &'static str {
    match m {
        ShockRunMode::Auto => "auto",
        ShockRunMode::Manual => "manual",
    }
}

// ---------------------------------------------------------------------------------
// Trips, ends, rows.
// ---------------------------------------------------------------------------------

/// What tripped, with its kind's facts (schema/PROTOCOL.md, the trip table).
#[derive(Debug, Clone, PartialEq)]
pub enum TripFacts {
    Tolerance {
        envelope_pass: bool,
        velocity_change_pass: bool,
        error_pct: f64,
        worst_deviation_frac: f64,
    },
    Overshoot {
        peak_ratio: f64,
        limit_ratio: f64,
    },
    Kinematic {
        /// `velocity` or `displacement`.
        axis: &'static str,
        value: f64,
        limit: f64,
        predicted: bool,
    },
    DriveVpk {
        vpk: f64,
        abort_vpk: f64,
        predicted: bool,
    },
    Accel {
        predicted_g: f64,
        limit_g: f64,
    },
    Monitor {
        channel: u32,
        value: f64,
        limit: f64,
    },
    Divergence {
        pulses: u32,
        error_pct: f64,
    },
    NotConverged {
        /// `per_level` or `run`.
        limit: &'static str,
        pulses: u64,
    },
    LawFault {
        /// A `LawFault::reason()` name, or `first_drive`.
        reason: &'static str,
        consecutive: u32,
    },
}

/// A trip and where it came.
#[derive(Debug, Clone, PartialEq)]
pub struct Trip {
    pub facts: TripFacts,
    /// The pulse it came at (the next pulse's index for one found before it fired).
    pub pulse: u64,
    pub at_level_db: Option<f64>,
}

impl Trip {
    pub fn kind(&self) -> &'static str {
        match self.facts {
            TripFacts::Tolerance { .. } => "tolerance",
            TripFacts::Overshoot { .. } => "overshoot",
            TripFacts::Kinematic { .. } => "kinematic",
            TripFacts::DriveVpk { .. } => "drive_vpk",
            TripFacts::Accel { .. } => "accel",
            TripFacts::Monitor { .. } => "monitor",
            TripFacts::Divergence { .. } => "divergence",
            TripFacts::NotConverged { .. } => "not_converged",
            TripFacts::LawFault { .. } => "law_fault",
        }
    }

    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("kind".into(), Value::from(self.kind()));
        m.insert("pulse".into(), Value::from(self.pulse));
        m.insert("at_level_db".into(), opt_rn(self.at_level_db, 2));
        let mut put = |k: &str, v: Value| {
            m.insert(k.into(), v);
        };
        match &self.facts {
            TripFacts::Tolerance {
                envelope_pass,
                velocity_change_pass,
                error_pct,
                worst_deviation_frac,
            } => {
                put("envelope_pass", Value::from(*envelope_pass));
                put("velocity_change_pass", Value::from(*velocity_change_pass));
                put("error_pct", rn(*error_pct, 2));
                put("worst_deviation_frac", rn(*worst_deviation_frac, 4));
            }
            TripFacts::Overshoot {
                peak_ratio,
                limit_ratio,
            } => {
                put("peak_ratio", rn(*peak_ratio, 4));
                put("limit_ratio", rn(*limit_ratio, 4));
            }
            TripFacts::Kinematic {
                axis,
                value,
                limit,
                predicted,
            } => {
                put("axis", Value::from(*axis));
                put("value", num(sig(*value, 4)));
                put("limit", num(sig(*limit, 4)));
                put("predicted", Value::from(*predicted));
            }
            TripFacts::DriveVpk {
                vpk,
                abort_vpk,
                predicted,
            } => {
                put("vpk", rn(*vpk, 4));
                put("abort_vpk", rn(*abort_vpk, 4));
                put("predicted", Value::from(*predicted));
            }
            TripFacts::Accel {
                predicted_g,
                limit_g,
            } => {
                put("predicted_g", rn(*predicted_g, 4));
                put("limit_g", rn(*limit_g, 4));
            }
            TripFacts::Monitor {
                channel,
                value,
                limit,
            } => {
                put("channel", Value::from(*channel));
                put("value", num(sig(*value, 4)));
                put("limit", num(sig(*limit, 4)));
            }
            TripFacts::Divergence { pulses, error_pct } => {
                put("pulses", Value::from(*pulses));
                put("error_pct", rn(*error_pct, 2));
            }
            TripFacts::NotConverged { limit, pulses } => {
                put("limit", Value::from(*limit));
                put("pulses", Value::from(*pulses));
            }
            TripFacts::LawFault {
                reason,
                consecutive,
            } => {
                put("reason", Value::from(*reason));
                put("consecutive", Value::from(*consecutive));
            }
        }
        Value::Object(m)
    }
}

/// `x` to `digits` significant digits.
pub fn sig(x: f64, digits: i32) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let e = x.abs().log10().floor() as i32;
    let p = digits - 1 - e;
    if p >= 0 {
        let m = 10f64.powi(p);
        (x * m).round() / m
    } else {
        let m = 10f64.powi(-p);
        (x / m).round() * m
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
pub struct End {
    pub outcome: &'static str,
    pub message: String,
    pub at_s: Option<f64>,
    pub level_db: Option<f64>,
    pub pulse: Option<u64>,
    pub trip: Option<Trip>,
}

impl End {
    pub fn to_json(&self) -> Value {
        json!({
            "outcome": self.outcome,
            "message": self.message,
            "at_s": opt_rn(self.at_s, 2),
            "level_db": opt_rn(self.level_db, 2),
            "pulse": self.pulse,
            "trip": self.trip.as_ref().map_or(Value::Null, Trip::to_json),
        })
    }
}

/// The law call that produced the drive a pulse played.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LawCall {
    pub used: bool,
    /// Why it was not used: a law-fault reason name.
    pub miss: Option<&'static str>,
    pub capped_bins: u64,
    pub peak_capped: bool,
    pub call_ms: f64,
}

/// One fired pulse.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub index: u64,
    /// `ladder`, `train` or `gunfire`.
    pub origin: &'static str,
    pub scheduled_db: f64,
    pub commanded_db: f64,
    pub limit_scale: f64,
    pub drive_vpk: f64,
    pub peak_g: Option<f64>,
    pub peak_ratio: Option<f64>,
    pub error_pct: Option<f64>,
    pub velocity_change_ips: Option<f64>,
    pub velocity_change_ratio: Option<f64>,
    /// Envelope, velocity change, substitution.
    pub verdicts: Option<(bool, bool, bool)>,
    pub passed: Option<bool>,
    /// SRS bands, and those within the informational band.
    pub srs: Option<(usize, usize)>,
    /// Seconds from the start of the run.
    pub at_s: f64,
    pub law: Option<LawCall>,
}

impl Row {
    pub fn to_json(&self) -> Value {
        json!({
            "index": self.index,
            "origin": self.origin,
            "scheduled_db": rn(self.scheduled_db, 2),
            "commanded_db": rn(self.commanded_db, 2),
            "limit_scale": rn(self.limit_scale, 4),
            "drive_vpk": rn(self.drive_vpk, 4),
            "peak_g": opt_rn(self.peak_g, 4),
            "peak_ratio": opt_rn(self.peak_ratio, 4),
            "error_pct": opt_rn(self.error_pct, 2),
            "velocity_change_ips": opt_rn(self.velocity_change_ips, 4),
            "velocity_change_ratio": opt_rn(self.velocity_change_ratio, 4),
            "verdicts": self.verdicts.map_or(Value::Null, |(e, v, s)| {
                json!({"envelope": e, "velocity": v, "substitution": s})
            }),
            "passed": self.passed,
            "srs": self.srs.map_or(Value::Null, |(b, p)| json!({"bands": b, "passed_bands": p})),
            "at_s": rn(self.at_s, 3),
            "law": self.law.map_or(Value::Null, |l| json!({
                "used": l.used,
                "miss": l.miss,
                "capped_bins": l.capped_bins,
                "peak_capped": l.peak_capped,
                "call_ms": rn(l.call_ms, 3),
            })),
        })
    }
}

// ---------------------------------------------------------------------------------
// The run's state.
// ---------------------------------------------------------------------------------

/// Pulse counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub fired: u64,
    pub passed: u64,
    pub at_level: u32,
    pub of_level: u32,
}

impl Counts {
    fn to_json(self) -> Value {
        json!({
            "fired": self.fired, "passed": self.passed,
            "at_level": self.at_level, "of_level": self.of_level,
        })
    }
}

/// What identification found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IdentFacts {
    pub drive_vrms: f64,
    pub drive_vpk: f64,
    pub coherence_mean: f64,
    pub min_mean_coherence: f64,
    pub passed: bool,
}

/// The predicted full-level drive and response.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prediction {
    pub drive_vpk: f64,
    pub drive_vpk_ceiling: f64,
    pub drive_vpk_abort: f64,
    pub response_peak_g: f64,
    pub velocity_pk_ips: f64,
    pub displacement_pk_pk_in: f64,
}

/// The law a run names itself by.
#[derive(Debug, Clone, PartialEq)]
pub struct LawRef {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Empty for a built-in law.
    pub sha256: String,
}

/// Everything the run shows, as of its last step.
#[derive(Debug, Clone)]
pub struct ShockView {
    pub run_id: String,
    pub title: String,
    pub state: State,
    pub phase: &'static str,
    pub station: StationRef,
    pub grid: Grid,
    pub clock: Option<&'static str>,
    pub mode: ShockRunMode,
    pub levels_db: Vec<f64>,
    pub level_db: Option<f64>,
    pub rung: Option<usize>,
    pub pulses: Counts,
    pub next_pulse_s: Option<f64>,
    pub elapsed_s: Option<f64>,
    pub alarm: bool,
    pub identification: Option<IdentFacts>,
    pub prediction: Option<Prediction>,
    pub last_pulse: Option<Row>,
    pub law: Option<LawRef>,
    pub misses: u32,
    pub feasibility: Option<(&'static str, String)>,
    pub end: Option<End>,
    /// The fire in flight: frames delivered of expected.
    pub fire: Option<(u64, u64)>,
    /// `shock.live` frames are due: from arm.
    pub streaming: bool,
}

/// The `shock.state` object; `None` is the unconfigured service.
pub fn state_json(v: Option<&ShockView>) -> Value {
    let Some(v) = v else {
        return json!({
            "run_id": null, "state": "unconfigured", "phase": "idle", "title": null,
            "source": null, "station": null, "grid": null, "clock": null, "mode": null,
            "levels_db": null, "level_db": null, "rung": null,
            "pulses": Counts::default().to_json(), "next_pulse_s": null, "elapsed_s": null,
            "alarm": false, "identification": null, "prediction": null, "last_pulse": null,
            "law": null, "feasibility": null, "end": null,
        });
    };
    let station = json!({
        "kind": v.station.kind,
        "class": v.station.class,
        "devices": v.station.devices.iter().map(|(id, sn)| json!({"id": id, "serial": sn})).collect::<Vec<_>>(),
    });
    json!({
        "run_id": v.run_id,
        "state": v.state.name(),
        "phase": v.phase,
        "title": v.title,
        "source": v.station.source(),
        "station": station,
        "grid": v.grid.to_json(),
        "clock": v.clock,
        "mode": mode_name(v.mode),
        "levels_db": v.levels_db.iter().map(|x| num(*x)).collect::<Vec<_>>(),
        "level_db": opt_rn(v.level_db, 2),
        "rung": v.rung,
        "pulses": v.pulses.to_json(),
        "next_pulse_s": opt_rn(v.next_pulse_s, 2),
        "elapsed_s": opt_rn(v.elapsed_s, 2),
        "alarm": v.alarm,
        "identification": v.identification.map_or(Value::Null, |i| json!({
            "drive_vrms": rn(i.drive_vrms, 4),
            "drive_vpk": rn(i.drive_vpk, 4),
            "coherence_mean": rn(i.coherence_mean, 4),
            "min_mean_coherence": rn(i.min_mean_coherence, 4),
            "passed": i.passed,
        })),
        "prediction": v.prediction.map_or(Value::Null, |p| json!({
            "drive_vpk": rn(p.drive_vpk, 4),
            "drive_vpk_ceiling": rn(p.drive_vpk_ceiling, 4),
            "drive_vpk_abort": rn(p.drive_vpk_abort, 4),
            "response_peak_g": rn(p.response_peak_g, 4),
            "velocity_pk_ips": rn(p.velocity_pk_ips, 4),
            "displacement_pk_pk_in": rn(p.displacement_pk_pk_in, 6),
        })),
        "last_pulse": v.last_pulse.as_ref().map_or(Value::Null, Row::to_json),
        "law": v.law.as_ref().map_or(Value::Null, |l| json!({
            "id": l.id,
            "name": l.name,
            "version": l.version,
            "sha256": if l.sha256.is_empty() { Value::Null } else { Value::from(l.sha256.as_str()) },
            "misses": v.misses,
        })),
        "feasibility": v.feasibility.as_ref().map_or(Value::Null, |(verdict, message)| {
            json!({"verdict": verdict, "message": message})
        }),
        "end": v.end.as_ref().map_or(Value::Null, End::to_json),
    })
}

/// What decides whether a `shock.state` event is due: everything but the clocks.
pub fn state_key(v: Option<&ShockView>) -> String {
    let mut s = state_json(v);
    if let Value::Object(m) = &mut s {
        m.remove("next_pulse_s");
        m.remove("elapsed_s");
    }
    s.to_string()
}

/// The `shock.live` data.
pub fn live_json(v: &ShockView) -> Value {
    json!({
        "run_id": v.run_id,
        "source": v.station.source(),
        "state": v.state.name(),
        "phase": v.phase,
        "mode": mode_name(v.mode),
        "level_db": opt_rn(v.level_db, 2),
        "rung": v.rung,
        "pulses": v.pulses.to_json(),
        "next_pulse_s": opt_rn(v.next_pulse_s, 2),
        "fire": v.fire.map_or(Value::Null, |(done, of)| json!({"done": done, "of": of})),
    })
}

// ---------------------------------------------------------------------------------
// The display views.
// ---------------------------------------------------------------------------------

/// A tolerance band over part of a record: its first sample and its edges.
#[derive(Debug, Clone, PartialEq)]
pub struct Band {
    pub start: usize,
    pub hi: Vec<f64>,
    pub lo: Vec<f64>,
}

/// One pulse's acceleration figures on a stretch of record. Sample `i` is at
/// `(start + i) / fs_hz` seconds from the main pulse's start.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeFigures {
    pub start: i64,
    pub fs_hz: f64,
    /// The reference at the level fired, g.
    pub reference: Vec<f64>,
    /// Its tolerance band, `start` counted on the same stretch.
    pub band: Option<Band>,
    /// The measured pulse, g, on the same stretch.
    pub measured: Option<Vec<f64>>,
}

/// The times of `n` samples from `start` at `fs_hz`, seconds from the main
/// pulse's start.
pub fn time_axis(start: i64, n: usize, fs_hz: f64) -> Vec<f64> {
    (0..n).map(|i| (start + i as i64) as f64 / fs_hz).collect()
}

/// A time series, decimated to at most `points` as an envelope (each point the
/// largest and smallest value in its interval) when it is longer.
pub fn time_series(
    name: &str,
    role: Role,
    t: Vec<f64>,
    y: &[f64],
    points: Option<usize>,
) -> Series {
    match points {
        Some(p) if p >= 2 && y.len() > p => {
            let n = y.len();
            let mut tx = Vec::with_capacity(p);
            let mut hi = Vec::with_capacity(p);
            let mut lo = Vec::with_capacity(p);
            for k in 0..p {
                let a = k * n / p;
                let b = ((k + 1) * n / p).max(a + 1);
                let chunk = &y[a..b];
                tx.push(t[a]);
                hi.push(chunk.iter().copied().fold(f64::NEG_INFINITY, f64::max));
                lo.push(chunk.iter().copied().fold(f64::INFINITY, f64::min));
            }
            let mut s = Series::new(name, role, tx, hi);
            s.y_min = Some(lo);
            s
        }
        _ => Series::new(name, role, t, y.to_vec()),
    }
}

fn time_axis_spec() -> Axis {
    Axis::new("Time", Some("s"), Scale::Linear)
}

/// The Pulse view: the reference at the level fired, its tolerance band and the
/// measured pulse, g, against time.
pub fn pulse_view(f: &TimeFigures, points: Option<usize>, arrays: &mut Arrays) -> Value {
    let n = f.reference.len();
    let mut series = vec![time_series(
        "Reference",
        Role::Reference,
        time_axis(f.start, n, f.fs_hz),
        &f.reference,
        points,
    )];
    if let Some(b) = &f.band {
        let t = time_axis(f.start + b.start as i64, b.hi.len(), f.fs_hz);
        series.push(time_series(
            "Tolerance high",
            Role::Tolerance,
            t.clone(),
            &b.hi,
            points,
        ));
        series.push(time_series(
            "Tolerance low",
            Role::Tolerance,
            t,
            &b.lo,
            points,
        ));
    }
    if let Some(m) = &f.measured {
        series.push(time_series(
            "Measured",
            Role::Measured,
            time_axis(f.start, m.len(), f.fs_hz),
            m,
            points,
        ));
    }
    views::spectrum(
        arrays,
        "Pulse",
        &time_axis_spec(),
        &Axis::new("Acceleration", Some("g"), Scale::Linear),
        series,
        Some("time"),
    )
}

/// The Drive view: the drive as played, volts, against time.
pub fn drive_view(
    start: i64,
    fs_hz: f64,
    drive: &[f64],
    points: Option<usize>,
    arrays: &mut Arrays,
) -> Value {
    views::spectrum(
        arrays,
        "Drive",
        &time_axis_spec(),
        &Axis::new("Drive", Some("V"), Scale::Linear),
        vec![time_series(
            "Drive",
            Role::Drive,
            time_axis(start, drive.len(), fs_hz),
            drive,
            points,
        )],
        Some("time"),
    )
}

/// Which kinematic a view shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kinematic {
    Velocity,
    Displacement,
}

/// The Velocity or Displacement view: the reference's and the measured pulse's.
pub fn kinematic_view(
    which: Kinematic,
    start: i64,
    fs_hz: f64,
    reference: &[f64],
    measured: Option<&[f64]>,
    points: Option<usize>,
    arrays: &mut Arrays,
) -> Value {
    let (title, unit) = match which {
        Kinematic::Velocity => ("Velocity", "in/s"),
        Kinematic::Displacement => ("Displacement", "in"),
    };
    let mut series = vec![time_series(
        "Reference",
        Role::Reference,
        time_axis(start, reference.len(), fs_hz),
        reference,
        points,
    )];
    if let Some(m) = measured {
        series.push(time_series(
            "Measured",
            Role::Measured,
            time_axis(start, m.len(), fs_hz),
            m,
            points,
        ));
    }
    views::spectrum(
        arrays,
        title,
        &time_axis_spec(),
        &Axis::new(title, Some(unit), Scale::Linear),
        series,
        Some("time"),
    )
}

/// One pulse's SRS figures: on the run's band grid, the measured SRS, the
/// reference's at the level fired and the informational band around it.
#[derive(Debug, Clone, PartialEq)]
pub struct SrsFigures {
    pub freqs_hz: Vec<f64>,
    pub reference: Vec<f64>,
    pub band_hi: Vec<f64>,
    pub band_lo: Vec<f64>,
    pub measured: Option<Vec<f64>>,
}

/// The SRS view.
pub fn srs_view(f: &SrsFigures, arrays: &mut Arrays) -> Value {
    let x = || f.freqs_hz.clone();
    let mut series = vec![
        Series::new("Reference", Role::Reference, x(), f.reference.clone()),
        Series::new("Band high", Role::Tolerance, x(), f.band_hi.clone()),
        Series::new("Band low", Role::Tolerance, x(), f.band_lo.clone()),
    ];
    if let Some(m) = &f.measured {
        series.push(Series::new("Measured", Role::Measured, x(), m.clone()));
    }
    views::spectrum(
        arrays,
        "SRS",
        &Axis::new("Natural frequency", Some("Hz"), Scale::Log),
        &Axis::new("Peak acceleration", Some("g"), Scale::Log),
        series,
        Some("frequency"),
    )
}

/// The Spectrum view: the magnitudes of the reference's and the response's
/// transforms on the pulse grid.
pub fn spectrum_view(
    freqs_hz: &[f64],
    reference: &[f64],
    measured: Option<&[f64]>,
    arrays: &mut Arrays,
) -> Value {
    let mut series = vec![Series::new(
        "Reference",
        Role::Reference,
        freqs_hz.to_vec(),
        reference.to_vec(),
    )];
    if let Some(m) = measured {
        series.push(Series::new(
            "Measured",
            Role::Measured,
            freqs_hz.to_vec(),
            m.to_vec(),
        ));
    }
    views::spectrum(
        arrays,
        "Spectrum",
        &Axis::new("Frequency", Some("Hz"), Scale::Log),
        &Axis::new("Magnitude", Some("g/Hz"), Scale::Log),
        series,
        Some("frequency"),
    )
}

/// The identified plant on the correction band's bins.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentFigures {
    pub freqs_hz: Vec<f64>,
    /// `abs(H)`, control unit per volt.
    pub gain: Vec<f64>,
    /// The phase of `H`, degrees.
    pub phase_deg: Vec<f64>,
    pub coherence: Vec<f64>,
    pub trusted: Vec<bool>,
    /// The coherence floor the trusted bins are held to.
    pub floor: f64,
    /// The control channel's unit.
    pub unit: String,
}

/// The identification views: gain, phase, and the coherence with the trusted
/// bins and the floor.
pub fn identification_views(f: &IdentFigures, arrays: &mut Arrays) -> Vec<Value> {
    let freq = Axis::new("Frequency", Some("Hz"), Scale::Log);
    let x = || f.freqs_hz.clone();
    let gain_unit = format!("{}/V", f.unit);
    let gain = views::spectrum(
        arrays,
        "Identification Gain",
        &freq,
        &Axis::new("Gain", Some(&gain_unit), Scale::Log),
        vec![Series::new("Gain", Role::Measured, x(), f.gain.clone())],
        Some("frequency"),
    );
    let phase = views::spectrum(
        arrays,
        "Identification Phase",
        &freq,
        &Axis::new("Phase", Some("deg"), Scale::Linear),
        vec![Series::new(
            "Phase",
            Role::Measured,
            x(),
            f.phase_deg.clone(),
        )],
        Some("frequency"),
    );
    let (tx, ty): (Vec<f64>, Vec<f64>) = f
        .freqs_hz
        .iter()
        .zip(&f.coherence)
        .zip(&f.trusted)
        .filter(|(_, t)| **t)
        .map(|((x, c), _)| (*x, *c))
        .unzip();
    let ends = match (f.freqs_hz.first(), f.freqs_hz.last()) {
        (Some(a), Some(b)) => vec![*a, *b],
        _ => Vec::new(),
    };
    let floor = vec![f.floor; ends.len()];
    let coherence = views::spectrum(
        arrays,
        "Coherence",
        &freq,
        &Axis::new("Coherence", None, Scale::Linear),
        vec![
            Series::new("Coherence", Role::Measured, x(), f.coherence.clone()),
            Series::new("Trusted bins", Role::Other, tx, ty),
            Series::new("Floor", Role::Tolerance, ends, floor),
        ],
        Some("frequency"),
    );
    vec![gain, phase, coherence]
}

/// One value a law reported on its last call.
#[derive(Debug, Clone, PartialEq)]
pub struct LawReading {
    pub label: String,
    pub value: f64,
    pub unit: String,
}

/// The Law reading.
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

/// The run's numbers, as a reading.
pub fn readings_view(v: &ShockView) -> Value {
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
        if let Some(peak) = p.peak_g {
            rows.push(ReadingRow::number(
                "Last peak",
                r(peak, 4),
                Some("g"),
                match p.passed {
                    Some(true) => Status::Pass,
                    Some(false) => Status::Fail,
                    None => Status::Info,
                },
            ));
        }
        if let Some(e) = p.error_pct {
            rows.push(ReadingRow::number(
                "Last error",
                r(e, 2),
                Some("%"),
                Status::Info,
            ));
        }
        rows.push(ReadingRow::number(
            "Last drive peak",
            r(p.drive_vpk, 4),
            Some("V"),
            Status::Info,
        ));
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

/// The Pulses table.
pub fn pulses_view(rows: &[Row]) -> Value {
    let yes = |b: Option<bool>| match b {
        Some(true) => "pass",
        Some(false) => "fail",
        None => "",
    };
    let fmt = |x: Option<f64>, d: i32| x.map_or(String::new(), |v| num(r(v, d)).to_string());
    let body: Vec<Value> = rows
        .iter()
        .map(|p| {
            let (e, vc, s) = match p.verdicts {
                Some((e, v, s)) => (Some(e), Some(v), Some(s)),
                None => (None, None, None),
            };
            json!([
                (p.index + 1).to_string(),
                p.origin,
                num(r(p.commanded_db, 2)).to_string(),
                fmt(p.peak_g, 4),
                fmt(p.error_pct, 2),
                fmt(p.velocity_change_ips, 4),
                yes(e),
                yes(vc),
                yes(s),
                yes(p.passed),
            ])
        })
        .collect();
    json!({
        "view": "table",
        "title": "Pulses",
        "columns": ["Pulse", "Origin", "Level (dB)", "Peak (g)", "Error (%)",
                    "Velocity change (in/s)", "Envelope", "Velocity", "Substitution", "Passed"],
        "rows": body,
    })
}

/// One fired pulse's figures, for its views.
#[derive(Debug, Clone, PartialEq)]
pub struct PulseFigures {
    pub time: TimeFigures,
    /// The drive as played, volts, on the same stretch.
    pub drive: Vec<f64>,
    /// The measured and reference velocity and displacement, on the same stretch.
    pub velocity: Option<(Vec<f64>, Vec<f64>)>,
    pub displacement: Option<(Vec<f64>, Vec<f64>)>,
    pub srs: Option<SrsFigures>,
    /// `abs(R)` and `abs(Y)` on the pulse grid's bins in the correction band.
    pub spectrum: Option<(Vec<f64>, Vec<f64>, Vec<f64>)>,
}

/// A pulse's default views: Pulse, Drive and SRS (`shock.pulse`, `shock.pulse`
/// the event).
pub fn default_pulse_views(
    f: &PulseFigures,
    points: Option<usize>,
    arrays: &mut Arrays,
) -> Vec<Value> {
    let mut out = vec![
        pulse_view(&f.time, points, arrays),
        drive_view(f.time.start, f.time.fs_hz, &f.drive, points, arrays),
    ];
    if let Some(s) = &f.srs {
        out.push(srs_view(s, arrays));
    }
    out
}

/// The figures behind one pulse view, kept per pulse by the run's ring.
pub type SharedFigures = Arc<PulseFigures>;

/// A freshly configured run's view: nothing opened, nothing fired.
pub fn configured_view(
    run_id: &str,
    cfg: &super::config::WireShock,
    derived: &super::config::Derived,
    station: StationRef,
    law: LawRef,
) -> ShockView {
    ShockView {
        run_id: run_id.to_owned(),
        title: cfg.title.clone(),
        state: State::Configured,
        phase: "idle",
        station,
        grid: derived.grid,
        clock: None,
        mode: cfg.mode,
        levels_db: cfg.levels_db.clone(),
        level_db: None,
        rung: None,
        pulses: Counts::default(),
        next_pulse_s: None,
        elapsed_s: None,
        alarm: false,
        identification: None,
        prediction: None,
        last_pulse: None,
        law: Some(law),
        misses: 0,
        feasibility: None,
        end: None,
        fire: None,
        streaming: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn significant_digits_round_like_a_person_reads_them() {
        assert_eq!(sig(0.123456, 3), 0.123);
        assert_eq!(sig(1234.5, 3), 1230.0);
        assert_eq!(sig(-0.0098765, 2), -0.0099);
        assert_eq!(sig(0.0, 3), 0.0);
    }

    #[test]
    fn a_long_series_is_decimated_to_an_envelope_and_a_short_one_is_not() {
        let y: Vec<f64> = (0..100).map(|i| (i as f64).sin()).collect();
        let t = time_axis(-10, 100, 1000.0);
        let s = time_series("x", Role::Measured, t.clone(), &y, Some(10));
        assert_eq!(s.x.len(), 10);
        let lo = s.y_min.clone().unwrap();
        for k in 0..10 {
            let chunk = &y[k * 10..(k + 1) * 10];
            assert_eq!(s.y[k], chunk.iter().copied().fold(f64::MIN, f64::max));
            assert_eq!(lo[k], chunk.iter().copied().fold(f64::MAX, f64::min));
        }
        let s = time_series("x", Role::Measured, t, &y, Some(4096));
        assert_eq!(s.y, y);
        assert!(s.y_min.is_none());
    }

    #[test]
    fn the_time_axis_counts_from_the_main_pulse() {
        assert_eq!(time_axis(-2, 4, 4.0), vec![-0.5, -0.25, 0.0, 0.25]);
    }

    #[test]
    fn a_trip_names_its_kind_first_and_its_facts_after() {
        let t = Trip {
            facts: TripFacts::DriveVpk {
                vpk: 0.4,
                abort_vpk: 0.38,
                predicted: true,
            },
            pulse: 0,
            at_level_db: Some(0.0),
        };
        let j = t.to_json();
        let keys: Vec<&str> = j.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "kind",
                "pulse",
                "at_level_db",
                "vpk",
                "abort_vpk",
                "predicted"
            ]
        );
        assert_eq!(j["kind"], "drive_vpk");
    }
}
