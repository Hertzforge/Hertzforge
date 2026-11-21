//! What a MIMO Mixed Mode run shows (schema/PROTOCOL.md, "MIMO Mixed Mode Run
//! State" and "MIMO Mixed Mode Frames"): the `mimo_mixed.state` object, the live
//! frame, and the views drawn from the spectra and histories the run thread keeps.
//! The rows are encoded from the controller's `MimoMixedStatus`: one per part
//! (the floor, each tone, each band) with its per-control numbers, one per drive.

use super::config::{round, sig, LawFacts};
use crate::codec::Arrays;
use crate::protocol::{num, opt_num};
use crate::views::{self, Axis, Role, Scale, Series};
use hertzforge_mimo_mixed::service::{MimoMixedPhase, MimoMixedStatus};
use hertzforge_random::RunMode;
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub use crate::random::view::{RecordRef, State};
pub use crate::sine::view::StationRef;

/// Points each history keeps (the oldest dropped first).
pub const HISTORY_POINTS: usize = 600;

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
pub struct End {
    /// Mixed Mode's outcomes: `completed`, `operator_stop`, `operator_abort`,
    /// `safe_stop`, `aborted` (a trip), `held` (the coherence gate), `law_fault`
    /// or `fault`.
    pub outcome: &'static str,
    /// The controller's own name for the end (`stopped`, `drive_vpk`,
    /// `tone_abort`, ...), when it named one.
    pub name: Option<String>,
    pub message: String,
    pub at_s: Option<f64>,
    pub level_db: Option<f64>,
    pub part: Option<String>,
    /// The control's channel, when the end names one.
    pub control: Option<u32>,
    /// The drive's channel, when the end names one.
    pub drive: Option<u32>,
    /// The ramp down's length, s, for an end that ramped.
    pub ramp_s: Option<f64>,
}

/// The identification as the state shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentView {
    pub level_db: f64,
    pub steps: usize,
    pub frames: usize,
    pub lag_frames: usize,
    /// Each control's mean coherence over the band.
    pub band_coherence: Vec<f64>,
    pub gate: f64,
    pub passed: bool,
    pub drive_vrms: Vec<f64>,
}

/// One part's facts from the configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct PartFacts {
    pub id: String,
    pub kind: &'static str,
    pub name: String,
    /// The floor's (alarm, abort) bands per control, dB (below, above).
    pub bands: Vec<((f64, f64), (f64, f64))>,
}

/// The grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub fs_hz: f64,
    pub n_pts: usize,
    pub df_hz: f64,
    pub lines: usize,
    pub band: (usize, usize),
    /// Whether the station has programmed the rate.
    pub actual: bool,
}

impl Grid {
    pub fn to_json(&self) -> Value {
        json!({
            "fs_hz": num(self.fs_hz), "n_pts": self.n_pts, "df_hz": num(sig(self.df_hz, 9)),
            "lines": self.lines,
            "band_hz": [num(sig(self.band.0 as f64 * self.df_hz, 9)),
                        num(sig((self.band.1.max(1) - 1) as f64 * self.df_hz, 9))],
            "actual": self.actual,
        })
    }

    /// The band's line frequencies.
    pub fn band_hz(&self) -> Vec<f64> {
        (self.band.0..self.band.1)
            .map(|l| l as f64 * self.df_hz)
            .collect()
    }
}

/// The spectra the frames are drawn from, on the band's lines, at the level in
/// force, as of the last chunk.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Spectra {
    /// Per control: the composite autospectrum (tones in), averaged.
    pub composite: Vec<Vec<f64>>,
    /// Per control: the residual (floor) autospectrum.
    pub residual: Vec<Vec<f64>>,
    /// Chosen cross terms of the residual: each control with the next, `|G|`.
    pub cross: Vec<(usize, usize, Vec<f64>)>,
    /// Per control: the reference and its alarm and abort lines at the level.
    pub reference: Vec<Vec<f64>>,
    pub alarm: Vec<(Vec<f64>, Vec<f64>)>,
    pub abort: Vec<(Vec<f64>, Vec<f64>)>,
    /// Per drive: the floor's drive autospectrum.
    pub drive: Vec<Vec<f64>>,
}

/// One tone's histories per control.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToneHistory {
    pub id: String,
    pub t: Vec<f64>,
    pub freq_hz: Vec<f64>,
    /// Per control.
    pub amplitude: Vec<Vec<f64>>,
    pub phase_deg: Vec<Vec<f64>>,
    pub error_db: Vec<Vec<f64>>,
}

/// One band's level history per control (its added level against the stated, dB).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BandHistory {
    pub id: String,
    pub t: Vec<f64>,
    pub error_db: Vec<Vec<f64>>,
}

/// Everything the state and the frames say, as the run last published it.
#[derive(Debug, Clone, PartialEq)]
pub struct MimoMixedView {
    pub run_id: String,
    pub title: String,
    pub state: State,
    pub phase: &'static str,
    pub station: StationRef,
    pub grid: Grid,
    pub mode: RunMode,
    pub levels_db: Vec<f64>,
    pub level_db: Option<f64>,
    pub t_s: Option<f64>,
    pub held: bool,
    pub parts: Vec<PartFacts>,
    /// Each part's switch as configured or as `mimo_mixed.part` last set it.
    pub enabled: std::collections::BTreeMap<String, bool>,
    /// The control and drive channels, in the configuration's order.
    pub controls: Vec<u32>,
    pub drives: Vec<u32>,
    /// Each drive's peak ceiling, volts.
    pub peak_v: Vec<f64>,
    /// Each control's reference rms at full level.
    pub reference_rms: Vec<f64>,
    pub unit: String,
    pub status: Option<MimoMixedStatus>,
    /// Each drive's peak held over the last second.
    pub held_peak_v: Vec<f64>,
    pub identification: Option<IdentView>,
    pub law: LawFacts,
    pub end: Option<End>,
    pub record: Option<RecordRef>,
    /// Whether the run has started streaming (`mimo_mixed.live` from identification on).
    pub streaming: bool,
    pub spectra: Option<Arc<Spectra>>,
    pub tones: Arc<Vec<ToneHistory>>,
    pub bands: Arc<Vec<BandHistory>>,
}

/// The wire phase of the controller's phase while it runs (Mixed Mode's names:
/// the floor alone is `floor_only`, the parts coming in and the schedule are
/// `engaged`).
pub fn phase_name(p: MimoMixedPhase) -> &'static str {
    match p {
        MimoMixedPhase::Idle => "armed",
        MimoMixedPhase::FloorAlone => "floor_only",
        MimoMixedPhase::PartsIn | MimoMixedPhase::Schedule => "engaged",
        MimoMixedPhase::FullLevel => "full_level",
        MimoMixedPhase::Stopping => "ramping_down",
    }
}

fn finite(x: f64, digits: i32) -> Value {
    if x.is_finite() {
        num(round(x, digits))
    } else {
        Value::Null
    }
}

/// The common scale the ceiling rule applies to the tones (1 with none).
pub fn tone_scale(s: &MimoMixedStatus) -> f64 {
    s.tones
        .iter()
        .filter(|t| t.part_level > 0.0)
        .map(|t| t.scale)
        .fold(1.0f64, f64::min)
}

fn part_rows(v: &MimoMixedView) -> Vec<Value> {
    let s = v.status.as_ref();
    let lam = 10f64.powf(v.level_db.unwrap_or(0.0) / 20.0);
    v.parts
        .iter()
        .map(|f| {
            let controls: Vec<Value>;
            let (on, freq, limiting, scale) = match f.kind {
                "floor" => {
                    let err = s.map(|s| s.floor_error_db.clone()).unwrap_or_default();
                    controls = v
                        .controls
                        .iter()
                        .enumerate()
                        .map(|(i, ch)| {
                            let target = v.reference_rms.get(i).copied().unwrap_or(0.0) * lam;
                            let e = err.get(i).copied().filter(|e| e.is_finite());
                            let alarm = e.is_some_and(|e| {
                                let ((lo, hi), _) = f.bands.get(i).copied().unwrap_or_default();
                                e < -lo || e > hi
                            });
                            json!({"channel": ch, "target": num(sig(target, 4)),
                                   "measured": opt_num(e.map(|e| sig(target * 10f64.powf(e / 20.0), 4))),
                                   "error_db": opt_num(e.map(|e| round(e, 2))), "alarm": alarm})
                        })
                        .collect();
                    (s.is_some_and(|s| s.state == hertzforge_mimo_mixed::service::MimoMixedState::Running), None, false, None)
                }
                "tone" => {
                    let t = s.and_then(|s| s.tones.iter().find(|t| t.id == f.id));
                    controls = v
                        .controls
                        .iter()
                        .enumerate()
                        .map(|(i, ch)| {
                            json!({"channel": ch,
                                   "target": opt_num(t.and_then(|t| t.target.get(i)).map(|x| sig(*x, 4))),
                                   "measured": opt_num(t.and_then(|t| t.measured.get(i)).map(|x| sig(*x, 4))),
                                   "error_db": t.and_then(|t| t.error_db.get(i)).map_or(Value::Null, |e| finite(*e, 2)),
                                   "alarm": t.and_then(|t| t.alarm.get(i)).copied().unwrap_or(false)})
                        })
                        .collect();
                    (
                        t.is_some_and(|t| t.part_level > 0.0),
                        t.map(|t| t.frequency_hz),
                        t.is_some_and(|t| t.limiting),
                        t.map(|t| t.scale),
                    )
                }
                _ => {
                    let b = s.and_then(|s| s.bands.iter().find(|b| b.id == f.id));
                    controls = v
                        .controls
                        .iter()
                        .enumerate()
                        .map(|(i, ch)| {
                            json!({"channel": ch, "target": null, "measured": null,
                                   "error_db": b.and_then(|b| b.error_db.get(i)).map_or(Value::Null, |e| finite(*e, 2)),
                                   "alarm": b.and_then(|b| b.alarm.get(i)).copied().unwrap_or(false)})
                        })
                        .collect();
                    (
                        b.is_some_and(|b| b.part_level > 0.0),
                        b.map(|b| b.center_hz),
                        false,
                        b.map(|b| b.part_level),
                    )
                }
            };
            let folded = s
                .and_then(|s| s.bands.iter().find(|b| b.id == f.id))
                .map(|b| b.folded);
            let enabled = v.enabled.get(&f.id).copied().unwrap_or(true);
            json!({
                "id": f.id,
                "kind": f.kind,
                "name": f.name,
                "enabled": enabled,
                "on": on,
                "freq_hz": opt_num(freq.map(|x| round(x, 3))),
                "held": v.held,
                "limiting": limiting,
                "scale": opt_num(scale.map(|x| round(x, 4))),
                "folded": folded.map_or(Value::Null, Value::from),
                "amplitude": match f.kind { "floor" => "rms", "tone" => "peak", _ => "db" },
                "unit": match f.kind { "narrowband" => "dB".to_owned(), _ => v.unit.clone() },
                "controls": controls,
            })
        })
        .collect()
}

fn drive_rows(v: &MimoMixedView) -> Vec<Value> {
    let s = v.status.as_ref();
    let scale = s.map_or(1.0, tone_scale);
    v.drives
        .iter()
        .enumerate()
        .map(|(j, ch)| {
            let d = s.and_then(|s| s.drives.get(j));
            let held = v.held_peak_v.get(j).copied().unwrap_or(0.0);
            let ceiling = v.peak_v.get(j).copied().unwrap_or(f64::NAN);
            let headroom = if held > 0.0 && ceiling.is_finite() {
                Some(20.0 * (ceiling / held).log10())
            } else {
                None
            };
            json!({
                "channel": ch,
                "random_rms_v": opt_num(d.map(|d| sig(d.random_rms_v, 4))),
                "peak_v": opt_num(d.map(|d| sig(d.peak_v, 4))),
                "random_peak_v": num(sig(held, 4)),
                "tone_sum_v": opt_num(d.map(|d| sig(d.tone_sum_v, 4))),
                "ceiling_v": num(ceiling),
                "headroom_v": if ceiling.is_finite() { num(sig(ceiling - held, 4)) } else { Value::Null },
                "headroom_db": opt_num(headroom.map(|h| round(h, 2))),
                "scale": num(round(scale, 4)),
            })
        })
        .collect()
}

pub fn end_json(e: &End) -> Value {
    json!({
        "outcome": e.outcome,
        "name": e.name,
        "message": e.message,
        "at_s": opt_num(e.at_s.map(|x| round(x, 3))),
        "level_db": opt_num(e.level_db.map(|x| round(x, 2))),
        "part": e.part,
        "control": e.control,
        "drive": e.drive,
        "ramp_s": opt_num(e.ramp_s.map(|x| round(x, 3))),
    })
}

fn ident_json(i: &IdentView) -> Value {
    json!({
        "level_db": num(round(i.level_db, 2)),
        "steps": i.steps,
        "frames": i.frames,
        "lag_frames": i.lag_frames,
        "band_coherence": i.band_coherence.iter().map(|c| num(round(*c, 4))).collect::<Vec<_>>(),
        "gate_coherence": num(i.gate),
        "passed": i.passed,
        "drive_vrms": i.drive_vrms.iter().map(|c| num(sig(*c, 4))).collect::<Vec<_>>(),
    })
}

/// The floor outside its alarm band at any control, by `st`.
pub fn alarm_floor(v: &MimoMixedView, st: &MimoMixedStatus) -> bool {
    let Some(f) = v.parts.iter().find(|p| p.kind == "floor") else {
        return false;
    };
    st.floor_error_db.iter().enumerate().any(|(i, e)| {
        let ((lo, hi), _) = f.bands.get(i).copied().unwrap_or_default();
        e.is_finite() && (*e < -lo || *e > hi)
    })
}

/// Any part outside its alarm band.
pub fn alarm(v: &MimoMixedView) -> bool {
    part_rows(v).iter().any(|r| {
        r["controls"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| c["alarm"] == true))
    })
}

/// Any tone held down by the shared ceiling, or a frame clamped.
pub fn limiting(v: &MimoMixedView) -> bool {
    v.status
        .as_ref()
        .is_some_and(|s| s.tones.iter().any(|t| t.limiting))
}

/// The `mimo_mixed.state` object; `None` is the unconfigured service.
pub fn state_json(v: Option<&MimoMixedView>) -> Value {
    let Some(v) = v else {
        return json!({
            "run_id": null, "state": "unconfigured", "phase": "idle", "title": null,
            "source": null, "station": null, "grid": null, "clock": null, "mode": null,
            "levels_db": null,
            "level_db": null, "t_s": null, "held": false, "alarm": false, "limiting": false,
            "parts": [], "drives": [], "scale": null, "floor_error_db": [],
            "floor_equalized": false, "floor": null,
            "identification": null, "law": null, "end": null,
        });
    };
    let s = v.status.as_ref();
    let mut m = json!({
        "run_id": v.run_id,
        "state": v.state.name(),
        "phase": v.phase,
        "title": v.title,
        "source": v.station.source(),
        "station": v.station.to_json(),
        "grid": v.grid.to_json(),
        "clock": if v.grid.actual { Value::from("shared") } else { Value::Null },
        "mode": crate::random::view::mode_name(v.mode),
        "levels_db": v.levels_db.iter().map(|l| num(*l)).collect::<Vec<_>>(),
        "level_db": opt_num(v.level_db.map(|x| round(x, 2))),
        "t_s": opt_num(v.t_s.map(|x| round(x, 3))),
        "held": v.held,
        "alarm": alarm(v),
        "limiting": limiting(v),
        "parts": part_rows(v),
        "drives": drive_rows(v),
        "scale": opt_num(s.map(|s| round(tone_scale(s), 4))),
        "floor_error_db": s.map_or(Vec::new(), |s| s.floor_error_db.iter().map(|e| finite(*e, 2)).collect::<Vec<_>>()),
        "floor_equalized": s.is_some_and(|s| s.floor_equalized),
        "floor": s.map_or(Value::Null, |s| json!({
            "equalized": s.floor_equalized,
            "loops": s.loops,
            "misses": s.floor_misses,
            "underruns": s.underruns,
            "clamped": s.clamped,
        })),
        "identification": v.identification.as_ref().map_or(Value::Null, ident_json),
        "law": {"floor": v.law.floor.to_json(), "tones": v.law.tones.to_json()},
        "end": v.end.as_ref().map_or(Value::Null, end_json),
    });
    if let (Some(r), Value::Object(o)) = (&v.record, &mut m) {
        o.insert("record".into(), r.to_json());
    }
    m
}

/// What `mimo_mixed.state` events are sent on a change of.
pub fn state_key(v: Option<&MimoMixedView>) -> String {
    match v {
        None => "unconfigured".into(),
        Some(v) => {
            let rows: String = part_rows(v)
                .iter()
                .map(|r| {
                    format!(
                        "{}{}{}",
                        (r["on"] == true) as u8,
                        (r["limiting"] == true) as u8,
                        r["controls"]
                            .as_array()
                            .is_some_and(|c| c.iter().any(|c| c["alarm"] == true))
                            as u8
                    )
                })
                .collect();
            format!(
                "{}|{}|{}|{:?}|{}|{}|{}|{}",
                v.run_id,
                v.state.name(),
                v.phase,
                v.level_db.map(|x| round(x, 2)),
                v.held,
                v.end.is_some(),
                rows,
                v.record.as_ref().map_or(Value::Null, |r| r.to_json()),
            )
        }
    }
}

fn label(v: &MimoMixedView, i: usize) -> String {
    format!("control {}", v.controls.get(i).copied().unwrap_or(0))
}

/// Control spectra with the tones in, against the reference and its lines.
pub fn control_view(v: &MimoMixedView, sp: &Spectra, pick: &[usize], arrays: &mut Arrays) -> Value {
    let x = v.grid.band_hz();
    let unit = crate::random::record::psd_unit(&v.unit);
    let mut series = Vec::new();
    for &i in pick {
        let name = label(v, i);
        if let Some(y) = sp.composite.get(i) {
            series.push(Series::new(&name, Role::Measured, x.clone(), y.clone()).stepped());
        }
        if let Some(y) = sp.reference.get(i) {
            series.push(Series::new(
                &format!("{name} reference"),
                Role::Reference,
                x.clone(),
                y.clone(),
            ));
        }
        if let Some((lo, hi)) = sp.alarm.get(i) {
            series.push(Series::new(
                &format!("{name} alarm high"),
                Role::Tolerance,
                x.clone(),
                hi.clone(),
            ));
            series.push(Series::new(
                &format!("{name} alarm low"),
                Role::Tolerance,
                x.clone(),
                lo.clone(),
            ));
        }
        if let Some((lo, hi)) = sp.abort.get(i) {
            series.push(Series::new(
                &format!("{name} abort high"),
                Role::Abort,
                x.clone(),
                hi.clone(),
            ));
            series.push(Series::new(
                &format!("{name} abort low"),
                Role::Abort,
                x.clone(),
                lo.clone(),
            ));
        }
    }
    views::spectrum(
        arrays,
        "Control spectra",
        &Axis::new("Frequency", Some("Hz"), Scale::Log),
        &Axis::new("PSD", Some(&unit), Scale::Log),
        series,
        Some("frequency"),
    )
}

/// The residual's autospectra (the floor, tones taken out) and its chosen cross terms.
pub fn residual_view(
    v: &MimoMixedView,
    sp: &Spectra,
    pick: &[usize],
    arrays: &mut Arrays,
) -> Value {
    let x = v.grid.band_hz();
    let unit = crate::random::record::psd_unit(&v.unit);
    let mut series = Vec::new();
    for &i in pick {
        if let Some(y) = sp.residual.get(i) {
            series.push(Series::new(&label(v, i), Role::Measured, x.clone(), y.clone()).stepped());
        }
        if let Some(y) = sp.reference.get(i) {
            series.push(Series::new(
                &format!("{} reference", label(v, i)),
                Role::Reference,
                x.clone(),
                y.clone(),
            ));
        }
    }
    for (r, c, y) in &sp.cross {
        if pick.contains(r) || pick.contains(c) {
            let name = format!(
                "|cross| {} with {}",
                v.controls.get(*r).copied().unwrap_or(0),
                v.controls.get(*c).copied().unwrap_or(0)
            );
            series.push(Series::new(&name, Role::Other, x.clone(), y.clone()).stepped());
        }
    }
    views::spectrum(
        arrays,
        "Residual spectra",
        &Axis::new("Frequency", Some("Hz"), Scale::Log),
        &Axis::new("PSD", Some(&unit), Scale::Log),
        series,
        Some("frequency"),
    )
}

/// The residual's chosen cross terms alone: each control with the next, `|G|`.
pub fn cross_view(v: &MimoMixedView, sp: &Spectra, pick: &[usize], arrays: &mut Arrays) -> Value {
    let x = v.grid.band_hz();
    let unit = crate::random::record::psd_unit(&v.unit);
    let series = sp
        .cross
        .iter()
        .filter(|(r, c, _)| pick.contains(r) || pick.contains(c))
        .map(|(r, c, y)| {
            let name = format!(
                "|cross| {} with {}",
                v.controls.get(*r).copied().unwrap_or(0),
                v.controls.get(*c).copied().unwrap_or(0)
            );
            Series::new(&name, Role::Measured, x.clone(), y.clone()).stepped()
        })
        .collect();
    views::spectrum(
        arrays,
        "Cross spectra",
        &Axis::new("Frequency", Some("Hz"), Scale::Log),
        &Axis::new("|CSD|", Some(&unit), Scale::Log),
        series,
        Some("frequency"),
    )
}

/// The floor's drive spectra.
pub fn drive_view(v: &MimoMixedView, sp: &Spectra, arrays: &mut Arrays) -> Value {
    let x = v.grid.band_hz();
    let series = sp
        .drive
        .iter()
        .enumerate()
        .map(|(j, y)| {
            Series::new(
                &format!("drive {}", v.drives.get(j).copied().unwrap_or(0)),
                Role::Drive,
                x.clone(),
                y.clone(),
            )
            .stepped()
        })
        .collect();
    views::spectrum(
        arrays,
        "Drive spectra",
        &Axis::new("Frequency", Some("Hz"), Scale::Log),
        &Axis::new("PSD", Some("V²/Hz"), Scale::Log),
        series,
        Some("frequency"),
    )
}

/// What a tone history plot shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToneQuantity {
    Amplitude,
    Phase,
}

/// Each picked tone's amplitude or phase history per control.
pub fn tone_view(
    v: &MimoMixedView,
    q: ToneQuantity,
    parts: Option<&[String]>,
    pick: &[usize],
    arrays: &mut Arrays,
) -> Value {
    let mut series = Vec::new();
    for h in v.tones.iter() {
        if parts.is_some_and(|p| !p.contains(&h.id)) {
            continue;
        }
        for &i in pick {
            let y = match q {
                ToneQuantity::Amplitude => h.amplitude.get(i),
                ToneQuantity::Phase => h.phase_deg.get(i),
            };
            if let Some(y) = y {
                series.push(Series::new(
                    &format!("{} {}", h.id, label(v, i)),
                    Role::Measured,
                    h.t.clone(),
                    y.clone(),
                ));
            }
        }
    }
    let (title, y) = match q {
        ToneQuantity::Amplitude => (
            "Tone amplitude",
            Axis::new("Amplitude", Some(&format!("{} pk", v.unit)), Scale::Linear),
        ),
        ToneQuantity::Phase => ("Tone phase", Axis::new("Phase", Some("deg"), Scale::Linear)),
    };
    views::spectrum(
        arrays,
        title,
        &Axis::new("Time", Some("s"), Scale::Linear),
        &y,
        series,
        Some("time"),
    )
}

/// Each picked band's level history per control (dB against its statement).
pub fn band_view(
    v: &MimoMixedView,
    parts: Option<&[String]>,
    pick: &[usize],
    arrays: &mut Arrays,
) -> Value {
    let mut series = Vec::new();
    for h in v.bands.iter() {
        if parts.is_some_and(|p| !p.contains(&h.id)) {
            continue;
        }
        for &i in pick {
            if let Some(y) = h.error_db.get(i) {
                series.push(Series::new(
                    &format!("{} {}", h.id, label(v, i)),
                    Role::Measured,
                    h.t.clone(),
                    y.clone(),
                ));
            }
        }
    }
    views::spectrum(
        arrays,
        "Band level",
        &Axis::new("Time", Some("s"), Scale::Linear),
        &Axis::new("Level against the statement", Some("dB"), Scale::Linear),
        series,
        Some("time"),
    )
}

/// The numbers every frame starts with (Mixed Mode's live head): the state's
/// members a running display reads.
pub fn live_head(v: &MimoMixedView) -> Map<String, Value> {
    let st = state_json(Some(v));
    let mut m = Map::new();
    for k in [
        "run_id",
        "source",
        "state",
        "phase",
        "mode",
        "t_s",
        "level_db",
        "held",
        "alarm",
        "limiting",
        "floor_error_db",
        "floor_equalized",
        "scale",
        "parts",
        "drives",
    ] {
        m.insert(k.into(), st[k].clone());
    }
    m.insert("class".into(), st["station"]["class"].clone());
    m.insert(
        "loop".into(),
        v.status
            .as_ref()
            .map_or(Value::from(0), |s| Value::from(s.loops)),
    );
    m
}

/// The `mimo_mixed.live` frame: the live head and the control and drive views.
pub fn live_json(v: &MimoMixedView, arrays: &mut Arrays) -> Value {
    let mut m = live_head(v);
    let all: Vec<usize> = (0..v.controls.len()).collect();
    let views = match &v.spectra {
        Some(sp) => vec![control_view(v, sp, &all, arrays), drive_view(v, sp, arrays)],
        None => Vec::new(),
    };
    m.insert("views".into(), Value::Array(views));
    Value::Object(m)
}

/// The view a run is configured with, before anything plays.
#[allow(clippy::too_many_arguments)]
pub fn configured_view(
    run_id: &str,
    title: &str,
    core: &hertzforge_mimo_mixed::MimoMixedConfig,
    derived: &super::config::Derived,
    station: StationRef,
    unit: String,
) -> MimoMixedView {
    let bands = super::config::floor_bands(core);
    MimoMixedView {
        run_id: run_id.to_owned(),
        title: title.to_owned(),
        state: State::Configured,
        phase: "idle",
        station,
        grid: Grid {
            fs_hz: derived.fs_hz,
            n_pts: derived.n_pts,
            df_hz: derived.df_hz,
            lines: derived.lines,
            band: derived.band,
            actual: false,
        },
        mode: core.schedule.mode,
        levels_db: core.schedule.table_db.clone(),
        level_db: None,
        t_s: None,
        held: false,
        enabled: core
            .parts
            .iter()
            .map(|p| (p.id.clone(), p.enabled))
            .collect(),
        parts: core
            .parts
            .iter()
            .map(|p| PartFacts {
                id: p.id.clone(),
                kind: super::config::part_kind(&p.body),
                name: if p.name.is_empty() {
                    p.id.clone()
                } else {
                    p.name.clone()
                },
                bands: bands.clone(),
            })
            .collect(),
        controls: core.controls.clone(),
        drives: core.drives.iter().map(|d| d.channel_index).collect(),
        peak_v: core.drives.iter().map(|d| d.peak_v).collect(),
        reference_rms: derived.reference_rms.clone(),
        unit,
        status: None,
        held_peak_v: vec![0.0; core.drives.len()],
        identification: None,
        law: derived.law.clone(),
        end: None,
        record: None,
        streaming: false,
        spectra: None,
        tones: Arc::new(Vec::new()),
        bands: Arc::new(Vec::new()),
    }
}

/// Each control's reference line, and its alarm and abort lines, `(lower, upper)`.
pub type ReferenceLines = (
    Vec<Vec<f64>>,
    Vec<(Vec<f64>, Vec<f64>)>,
    Vec<(Vec<f64>, Vec<f64>)>,
);

/// Fill a level-scaled reference and its lines on the band from the configuration.
pub fn reference_lines(
    core: &hertzforge_mimo_mixed::MimoMixedConfig,
    band: (usize, usize),
    level_db: f64,
) -> ReferenceLines {
    let g = 10f64.powf(level_db / 10.0);
    let n = core.n();
    let t = &core.reference.tolerances;
    let at = |v: &[f64], i: usize| -> Vec<f64> {
        (band.0..band.1)
            .map(|l| v.get(l * n + i).copied().unwrap_or(f64::NAN) * g)
            .collect()
    };
    let reference = (0..n)
        .map(|i| {
            (band.0..band.1)
                .map(|l| core.reference.sdm.get(l, i, i).re * g)
                .collect()
        })
        .collect();
    let alarm = (0..n)
        .map(|i| (at(&t.alarm_lower, i), at(&t.alarm_upper, i)))
        .collect();
    let abort = (0..n)
        .map(|i| (at(&t.abort_lower, i), at(&t.abort_upper, i)))
        .collect();
    (reference, alarm, abort)
}
