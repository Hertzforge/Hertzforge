//! What a MIMO Shock run puts in its record (`docs/run-folder.md`, "What a MIMO
//! Shock Run Saves"): the manifest, the table and the setup at arm, the two streams
//! of shots (every enabled input and every drive, a shot for each identification
//! shot and one per pulse), and the results (each control's reference, the
//! identification's transfer function matrix and coherence, and each pulse's
//! measures per control, the drives it played and the law's calls), each written as
//! it is known.
//!
//! The writing is the record's ([`crate::record`]); this module only says what is
//! written, in the reader's form (`hertzforge.io.run_folder`). The streams are fed
//! by the shared shot wrapper ([`crate::srs::record::ShotRecording`]). Values are in
//! the channels' own units, as the table names them: the analysis side converts.

use super::config::{WireMs, WireTargetSet};
use super::view::mode_name;
use crate::protocol::num;
use crate::random::record::{self as rec, drive_rows, input_rows, Described};
use crate::record::{ArrayValues, Begin, ResultArray, StreamSpec};
use crate::shock::record::StationFacts;
use crate::table::unit_name;
use hertzforge_channel_table::{Channel, ChannelTable};
use hertzforge_mimo_shock::records::{LawCallEntry, PulseEntry};
use hertzforge_mimo_shock::{
    IdentificationReport, MimoShockConfig, PulseReport, SeedReport, Setup, Target, Timing,
};
use serde_json::{json, Map, Value};
use std::path::PathBuf;

/// The facts of an armed run its record begins with.
pub struct ArmFacts<'a> {
    pub run_id: &'a str,
    pub run_uid: &'a str,
    pub cfg: &'a WireMs,
    /// The engine's own configuration as it runs, at the adopted rate: its band
    /// and guards as resolved.
    pub engine: &'a MimoShockConfig,
    /// The target as the engine built it at the adopted rate.
    pub target: Option<&'a Target>,
    /// The derived values as configure gave them.
    pub derived: &'a Value,
    pub table: &'a ChannelTable,
    pub revision: u64,
    pub station: &'a StationFacts,
    /// The rate the station runs at, the record every pulse is played as, and the
    /// quiet frames before every record of a shot.
    pub fs_hz: f64,
    pub record_len: usize,
    pub quiet_frames: usize,
    pub law: Value,
    pub armed_unix: f64,
    pub utc_offset_s: Option<i64>,
    /// `None` when the streams are recorded; otherwise why not.
    pub streams_off: Option<String>,
}

fn nums(x: &[f64]) -> Vec<Value> {
    x.iter().map(|v| num(*v)).collect()
}

fn row_of(table: &ChannelTable, index: u32) -> Option<&Channel> {
    table.channels.iter().find(|c| c.index == index)
}

/// A channel's unit as the table names it (`g` when the table does not hold it).
pub fn unit_of(table: &ChannelTable, index: u32) -> String {
    row_of(table, index).map_or_else(|| "g".to_owned(), |c| unit_name(&c.units))
}

/// The rms and peak of each control's reference at full level.
fn reference_facts(target: Option<&Target>, nc: usize, n: usize) -> Vec<(f64, f64)> {
    let Some(t) = target else {
        return Vec::new();
    };
    let w = t.waveforms();
    (0..nc)
        .filter_map(|c| w.get(c * n..(c + 1) * n))
        .map(|r| {
            let rms = (r.iter().map(|v| v * v).sum::<f64>() / r.len().max(1) as f64).sqrt();
            let pk = r.iter().fold(0.0f64, |a, v| a.max(v.abs()));
            (rms, pk)
        })
        .collect()
}

fn timing_json(t: &Timing) -> Value {
    match t {
        Timing::Free { window_s } => json!({"mode": "free", "window_s": num(*window_s)}),
        Timing::Hold(h) => json!({"mode": "hold", "window_s": num(h.window_s),
                                  "delays_s": nums(&h.delays_s)}),
    }
}

/// The target as resolved: its kind, the tolerance it is judged by, and for an SRS
/// target the natural frequencies and each control's SRS at full level on them.
fn target_json(f: &ArmFacts<'_>) -> Value {
    let cfg = f.cfg;
    match &cfg.target {
        WireTargetSet::Waveform { standard, .. } => {
            let p = standard.params();
            json!({
                "kind": "waveform",
                "standard": crate::shock::config::standard_name(*standard),
                "tolerance_frac_of_peak": num(p.amp_tol_frac_of_peak),
                "velocity_change_tolerance_frac": num(p.dv_tol_frac),
                "pulses": cfg.raw["target"]["pulses"].clone(),
            })
        }
        WireTargetSet::Srs {
            bands,
            tolerance_db,
            controls,
        } => {
            let (natural, points, q) = match f.target {
                Some(Target::Srs(t)) => (t.natural_hz.clone(), t.points.clone(), t.q),
                _ => (Vec::new(), Vec::new(), bands.q),
            };
            let m = natural.len();
            json!({
                "kind": "srs",
                "bands": {"f_lo_hz": num(bands.f_lo_hz), "f_hi_hz": num(bands.f_hi_hz),
                          "bands_per_octave": num(bands.bands_per_octave), "q": num(q)},
                "natural_hz": nums(&natural),
                "tolerance_db": [num(tolerance_db.0), num(tolerance_db.1)],
                "controls": controls.iter().enumerate().map(|(c, s)| json!({
                    "channel": cfg.controls.get(c),
                    "points": points.get(c * m..(c + 1) * m).map_or_else(Vec::new, nums),
                    "polarity": if s.inverted { "negative" } else { "positive" },
                    "target": cfg.raw["target"]["controls"][c]["target"].clone(),
                })).collect::<Vec<_>>(),
            })
        }
    }
}

/// The setup as accepted and as resolved: the channels, each control's reference,
/// the target and its tolerance, the timing and the verdict, the law and its
/// parameters, the level schedule and the pulse counts, the spacing, the
/// identification, the limits and guards as the engine holds them, and the rate.
fn setup_json(f: &ArmFacts<'_>) -> Value {
    let cfg = f.cfg;
    let n = f.record_len;
    let facts = reference_facts(f.target, cfg.controls.len(), n);
    let g = &f.engine.guards;
    let (lo, hi) = (f.engine.band_lo_hz, f.engine.band_hi_hz);
    json!({
        "config": cfg.raw,
        "resolved": {
            "channels": {
                "controls": cfg.controls.iter().enumerate().map(|(c, ch)| json!({
                    "channel": ch,
                    "unit": unit_of(f.table, *ch),
                    "description": row_of(f.table, *ch).map(|r| r.description.clone()),
                    "reference_peak": facts.get(c).map(|x| num(x.1)),
                    "reference_rms": facts.get(c).map(|x| num(x.0)),
                })).collect::<Vec<_>>(),
                "drives": cfg.drives.iter().map(|d| json!({
                    "channel": d.channel, "unit": unit_of(f.table, d.channel),
                    "ceiling_vpk": num(d.ceiling_vpk), "abort_vpk": num(d.abort_vpk),
                })).collect::<Vec<_>>(),
                "limits": cfg.limits.iter().map(|(ch, a)| json!({
                    "channel": ch, "unit": unit_of(f.table, *ch), "abort_peak": num(*a),
                })).collect::<Vec<_>>(),
            },
            "target": target_json(f),
            "timing": timing_json(&cfg.timing),
            "verdict": cfg.verdict.as_str(),
            "schedule": {
                "mode": mode_name(cfg.mode),
                "levels_db": nums(&cfg.levels_db),
                "max_pulses_per_level": cfg.max_pulses_per_level,
                "full_level_pulses": cfg.full_level_pulses,
                "max_pulses": cfg.max_pulses,
            },
            "spacing": {
                "cooling_s": num(cfg.cooling_s),
                "settling_s": num(cfg.settling_s),
                "compute_s": num(cfg.compute_s),
                "spacing_s": num(cfg.spacing_s()),
            },
            "identification": {
                "drive_vrms": num(cfg.drive_vrms),
                "probe_vrms": num(0.1 * cfg.drive_vrms),
                "frames": f.engine.identification.frames,
                "coherence_floor": num(cfg.coherence_floor),
                "min_mean_coherence": num(hertzforge_mimo_shock::engine::MIN_MEAN_COHERENCE),
                "min_band_energy": num(hertzforge_mimo_shock::engine::MIN_BAND_ENERGY),
                "review": cfg.review,
            },
            "control": {
                "law": {"id": cfg.law_id, "params": f.law.get("params").cloned()
                    .unwrap_or(Value::Null)},
                "law_budget": num(cfg.law_budget),
                "seed_budget_s": num(cfg.seed_budget_s),
                "law_misses": cfg.law_misses,
                "rehearse_law": g.rehearse_law,
            },
            "limits": {
                "abort_db": cfg.abort_db.map(num),
                "scaled_aborts": cfg.scaled_aborts,
                "max_velocity_ips": cfg.max_velocity_ips.map(num),
                "max_displacement_in": cfg.max_displacement_in.map(num),
                "max_growth_db": num(cfg.max_growth_db),
                "max_band_growth_db": num(cfg.max_band_growth_db),
                "divergence_pulses": cfg.divergence_pulses,
                "divergence_drive_ratio": num(cfg.divergence_drive_ratio),
                "first_drive_ratio": num(cfg.first_drive_ratio),
                "max_pulses": cfg.max_pulses,
            },
            "acquisition": {
                "requested_fs_hz": num(cfg.fs_hz), "fs_hz": num(f.fs_hz),
                "record_len": n, "lines": n / 2 + 1,
                "band_hz": [num(lo), num(hi)], "lead_s": num(cfg.lead_s),
                "quiet_frames": f.quiet_frames,
            },
            "plant": cfg.sim_plant(),
        },
        "derived": f.derived,
    })
}

fn stream_channel(c: &Channel) -> Value {
    json!({"index": c.index, "unit": unit_name(&c.units), "description": c.description})
}

/// The record a MIMO Shock run begins with, at arm.
pub fn begin(f: &ArmFacts<'_>, root: PathBuf) -> Begin {
    let cfg = f.cfg;
    let mut manifest = Map::new();
    manifest.insert("test".into(), Value::from("mimo_shock"));
    manifest.insert("run_uid".into(), Value::from(f.run_uid));
    manifest.insert("run_id".into(), Value::from(f.run_id));
    manifest.insert("title".into(), Value::from(cfg.title.as_str()));
    manifest.insert("operator".into(), Value::from(cfg.operator.as_str()));
    manifest.insert("article".into(), Value::from(cfg.article.as_str()));
    manifest.insert("notes".into(), Value::from(cfg.notes.as_str()));
    if let Some((document, clause)) = &cfg.specification {
        manifest.insert(
            "specification".into(),
            json!({"document": document, "clause": clause}),
        );
    }
    manifest.insert("source".into(), Value::from(f.station.source));
    if let Some(sim) = &f.station.sim {
        let mut sim = sim.clone();
        if let (Value::Object(m), Some(plant)) = (&mut sim, cfg.sim_plant()) {
            m.insert("plant".into(), Value::from(plant));
        }
        manifest.insert("sim".into(), sim);
    }
    manifest.insert("station".into(), f.station.station.clone());
    manifest.insert(
        "grid".into(),
        json!({"requested_fs_hz": num(cfg.fs_hz), "fs_hz": num(f.fs_hz),
               "record_len": f.record_len, "lines": f.record_len / 2 + 1,
               "df_hz": num(f.fs_hz / f.record_len as f64)}),
    );
    manifest.insert("build".into(), f.station.build.clone());
    manifest.insert(
        "armed".into(),
        json!({"unix": f.armed_unix, "utc_offset_s": f.utc_offset_s}),
    );
    manifest.insert(
        "start".into(),
        rec::start_json(f.armed_unix, f.utc_offset_s, 0),
    );
    manifest.insert(
        "recording".into(),
        match &f.streams_off {
            None => json!({"streams": "on"}),
            Some(why) => json!({"streams": "off", "why": why}),
        },
    );
    let mut streams = Vec::new();
    if let (None, Some((inputs_link, drives_link))) = (&f.streams_off, &f.station.links) {
        let shots = |kind: &str, channels: Vec<Value>| {
            let mut d = Map::new();
            d.insert("kind".into(), Value::from(kind));
            d.insert("fs_hz".into(), num(f.fs_hz));
            d.insert("channels".into(), Value::Array(channels));
            d.insert("first_station_frame".into(), Value::from(0));
            d.insert("shots".into(), Value::Array(Vec::new()));
            d
        };
        streams.push(StreamSpec {
            name: "inputs".into(),
            description: shots(
                "inputs",
                input_rows(f.table)
                    .into_iter()
                    .map(stream_channel)
                    .collect(),
            ),
            link: inputs_link.clone(),
        });
        let mut d = shots(
            "drives",
            drive_rows(f.table)
                .into_iter()
                .map(stream_channel)
                .collect(),
        );
        d.insert(
            "output_latency_frames".into(),
            f.station.output_latency.clone(),
        );
        streams.push(StreamSpec {
            name: "drives".into(),
            description: d,
            link: drives_link.clone(),
        });
    }
    let name = format!("{}_mimo_shock_{}", rec::utc_stamp(f.armed_unix), f.run_id);
    Begin {
        root,
        name,
        manifest,
        table: rec::table_json(f.table, f.revision),
        setup: setup_json(f),
        law: Some((f.law.clone(), None)),
        log: Vec::new(),
        streams,
    }
}

/// What a shot of both streams was.
pub enum Shot<'a> {
    /// An identification shot: the probe that sets the level, or the shot the
    /// plant is estimated from, with the rms each drive was asked for.
    Identification { step: &'static str, drive_vrms: f64 },
    /// A pulse that was judged.
    Pulse(&'a PulseReport),
    /// A shot no pulse was judged on: a pulse stopped part way, or a drive
    /// committed and withheld before it played (its inputs then hold no frames and
    /// say `played` false).
    Unjudged,
}

/// A shot's members in `stream.json`. Every record of a shot follows the quiet
/// frames before it (`pre_frames`), so its firing is the record's first sample.
pub fn shot_label(shot: Shot<'_>, quiet_frames: usize) -> Map<String, Value> {
    let mut m = Map::new();
    match shot {
        Shot::Identification { step, drive_vrms } => {
            m.insert("what".into(), Value::from("identification"));
            m.insert("pulse".into(), Value::Null);
            m.insert("step".into(), Value::from(step));
            m.insert("drive_vrms".into(), num(drive_vrms));
            m.insert("level_db".into(), Value::Null);
        }
        Shot::Pulse(p) => {
            m.insert("what".into(), Value::from("pulse"));
            m.insert("pulse".into(), Value::from(p.index));
            m.insert("origin".into(), Value::from("ladder"));
            m.insert("level_db".into(), num(p.level_db));
            m.insert("scheduled_db".into(), num(p.scheduled_db));
        }
        Shot::Unjudged => {
            m.insert("what".into(), Value::from("unjudged"));
            m.insert("pulse".into(), Value::Null);
            m.insert("level_db".into(), Value::Null);
        }
    }
    m.insert("pre_frames".into(), Value::from(quiet_frames));
    m
}

// ---- results ----------------------------------------------------------------------

fn f64s(name: &str, shape: Vec<usize>, unit: &str, values: Vec<f64>) -> ResultArray {
    ResultArray {
        name: name.into(),
        shape,
        unit: unit.into(),
        values: ArrayValues::F64(values),
    }
}

fn described(kind: &str, abscissa: Value, channels: Value, words: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("kind".into(), Value::from(kind));
    m.insert("abscissa".into(), abscissa);
    m.insert("channels".into(), channels);
    m.insert("description".into(), Value::from(words));
    m
}

fn time_axis() -> Value {
    json!({"kind": "time", "unit": "s", "array": "time"})
}

/// Time from a record's start, by its first sample and its step.
fn sample_axis(fs_hz: f64) -> Value {
    json!({"kind": "time", "unit": "s", "start": 0.0, "step": 1.0 / fs_hz})
}

fn frequency_axis() -> Value {
    json!({"kind": "frequency", "unit": "Hz", "array": "frequency"})
}

/// The names the results give the channels (the controls, the drives, and the
/// units the controls and the drives are in) and the record they are on (its
/// length and rate).
pub struct Names {
    pub controls: Vec<u32>,
    pub drives: Vec<u32>,
    pub control_unit: String,
    pub drive_unit: String,
    pub record_len: usize,
    pub fs_hz: f64,
}

impl Names {
    pub fn of(cfg: &WireMs, table: &ChannelTable, fs_hz: f64, record_len: usize) -> Names {
        let controls = cfg.controls.clone();
        let drives: Vec<u32> = cfg.drives.iter().map(|d| d.channel).collect();
        Names {
            control_unit: controls
                .first()
                .map_or_else(|| "g".into(), |c| unit_of(table, *c)),
            drive_unit: drives
                .first()
                .map_or_else(|| "V".into(), |d| unit_of(table, *d)),
            controls,
            drives,
            record_len,
            fs_hz,
        }
    }
}

/// Each control's reference at full level as the module holds it at the adopted
/// rate: for a waveform target the waveforms it is judged against (`target`, kind
/// `transient_reference`), with its tolerance as a share of each peak; for an SRS
/// target each control's SRS on the natural frequencies with the tolerance band
/// (`srs_reference`) and the waveform the module synthesized for each
/// (`synthesis`, kept as arrays).
pub fn reference_results(
    target: &Target,
    cfg: &WireMs,
    fs_hz: f64,
    record_len: usize,
    names: &Names,
) -> Vec<Described> {
    let (nc, n) = (names.controls.len(), record_len);
    let w = target.waveforms();
    if nc == 0 || w.len() < nc * n {
        return Vec::new();
    }
    let time: Vec<f64> = (0..n).map(|i| i as f64 / fs_hz).collect();
    let unit = names.control_unit.as_str();
    let responses = json!({"responses": names.controls});
    match (target, &cfg.target) {
        (Target::Srs(t), WireTargetSet::Srs { .. }) => {
            let m = t.natural_hz.len();
            if t.points.len() < nc * m {
                return Vec::new();
            }
            let scale = |db: f64| 10f64.powf(db / 20.0);
            let points = t.points[..nc * m].to_vec();
            let mut d = described(
                "srs_reference",
                frequency_axis(),
                responses.clone(),
                "each control's target SRS at full level on the natural frequencies, with the \
                 band it passes in (band)",
            );
            d.insert("q".into(), num(t.q));
            d.insert("srs_kind".into(), Value::from("maximax"));
            d.insert(
                "tolerance_db".into(),
                json!([num(t.tol_lo_db), num(t.tol_hi_db)]),
            );
            let mut syn = described(
                "mimo_shock_synthesis",
                sample_axis(fs_hz),
                responses,
                "the waveform the module synthesized for each control at full level, from \
                 its target SRS: a design the drives started from, not a measurement",
            );
            syn.insert("fs_hz".into(), num(fs_hz));
            vec![
                (
                    "srs_reference".into(),
                    d,
                    vec![
                        f64s("frequency", vec![m], "Hz", t.natural_hz.clone()),
                        f64s(
                            "band_low",
                            vec![nc, m],
                            unit,
                            points.iter().map(|v| v * scale(t.tol_lo_db)).collect(),
                        ),
                        f64s(
                            "band_high",
                            vec![nc, m],
                            unit,
                            points.iter().map(|v| v * scale(t.tol_hi_db)).collect(),
                        ),
                        f64s("reference", vec![nc, m], unit, points),
                    ],
                ),
                (
                    "synthesis".into(),
                    syn,
                    vec![f64s("waveform", vec![nc, n], unit, w[..nc * n].to_vec())],
                ),
            ]
        }
        (_, WireTargetSet::Waveform { standard, .. }) => {
            let p = standard.params();
            let mut d = described(
                "transient_reference",
                time_axis(),
                responses,
                "each control's reference waveform at full level, time from the record's \
                 start; a control passes inside its standard's band, a share of its peak \
                 (tolerance_frac_of_peak), after its own delay is taken out",
            );
            d.insert(
                "standard".into(),
                Value::from(crate::shock::config::standard_name(*standard)),
            );
            d.insert("tolerance_frac_of_peak".into(), num(p.amp_tol_frac_of_peak));
            d.insert("fs_hz".into(), num(fs_hz));
            vec![(
                "target".into(),
                d,
                vec![
                    f64s("time", vec![n], "s", time),
                    f64s("reference", vec![nc, n], unit, w[..nc * n].to_vec()),
                ],
            )]
        }
        _ => Vec::new(),
    }
}

/// The lines the identification excited, contiguous, from 1: the first and how
/// many.
fn band_span(s: &Setup) -> (usize, usize) {
    let n = s.record_len;
    let lines = n / 2 + 1;
    let df = s.fs_hz / n as f64;
    let on: Vec<usize> = (1..lines)
        .filter(|&f| {
            let hz = f as f64 * df;
            hz >= s.band_lo_hz && hz <= s.band_hi_hz
        })
        .collect();
    match (on.first(), on.last()) {
        (Some(a), Some(b)) => (*a, b - a + 1),
        _ => (0, 0),
    }
}

/// The identification: its transfer function matrix, each drive to each control,
/// in the file's convention (`identification_frf`), each control's multiple
/// coherence with the drives (`identification_coherence`), both on the band's
/// lines, and the shot's facts (`identification`, kept as arrays).
///
/// The module's estimate is `G_yx G_xx^-1` on spectra of the negative-exponent
/// transform with `G_yx = Y conj(X)`: a response over a drive, which is the file's
/// `conj(X) Y / |X|^2` already, so nothing is conjugated here.
pub fn identification_results(
    id: &IdentificationReport,
    setup: Option<&Setup>,
    names: &Names,
) -> Vec<Described> {
    let mut out = Vec::new();
    let (nc, nd) = (names.controls.len(), names.drives.len());
    if let Some(s) = setup {
        let (first, l) = band_span(s);
        let lines = s.record_len / 2 + 1;
        let df = s.fs_hz / s.record_len as f64;
        if l > 0 && s.n_controls == nc && s.n_drives == nd {
            let axis = json!({"kind": "frequency", "unit": "Hz",
                              "start": first as f64 * df, "step": df});
            let mut h = Vec::with_capacity(nc * nd * l);
            for c in 0..nc {
                for d in 0..nd {
                    for f in first..first + l {
                        let v = s.plant.frf.line(f)[c * nd + d];
                        h.push((v.re, v.im));
                    }
                }
            }
            let mut frf = described(
                "frf",
                axis.clone(),
                json!({"responses": names.controls, "references": names.drives}),
                "identification: the transfer function matrix from each drive to each control",
            );
            frf.insert("convention".into(), Value::from("file"));
            let coherence: Vec<f64> = (0..nc)
                .flat_map(|c| {
                    (first..first + l).map(move |f| {
                        s.plant
                            .coherence
                            .get(c * lines + f)
                            .copied()
                            .unwrap_or(f64::NAN)
                    })
                })
                .collect();
            let coh = described(
                "coherence",
                axis,
                json!({"responses": names.controls}),
                "identification: each control's multiple coherence with the drives",
            );
            out.push((
                "identification_frf".into(),
                frf,
                vec![ResultArray {
                    name: "values".into(),
                    shape: vec![nc, nd, l],
                    unit: format!("{}/{}", names.control_unit, names.drive_unit),
                    values: ArrayValues::C128(h),
                }],
            ));
            out.push((
                "identification_coherence".into(),
                coh,
                vec![f64s("values", vec![nc, l], "", coherence)],
            ));
        }
    }
    let mut d = described(
        "mimo_shock_identification",
        json!({"kind": "none"}),
        json!({"controls": names.controls, "drives": names.drives}),
        "identification: the rms and peak each drive played in the shot, each control's \
         band-mean multiple coherence, the share of its target's energy on trusted lines and \
         its peak response",
    );
    d.insert("realizations".into(), Value::from(id.realizations));
    d.insert("trusted_lines".into(), Value::from(id.trusted_lines));
    d.insert("max_condition".into(), num(id.max_condition));
    d.insert("df_hz".into(), num(id.df_hz));
    let pad = |x: &[f64], k: usize| -> Vec<f64> {
        (0..k)
            .map(|i| x.get(i).copied().unwrap_or(f64::NAN))
            .collect()
    };
    out.push((
        "identification".into(),
        d,
        vec![
            f64s("drive_rms", vec![nd], "V", pad(&id.drive_vrms, nd)),
            f64s("drive_peak", vec![nd], "V", pad(&id.drive_vpk, nd)),
            f64s("mean_coherence", vec![nc], "", pad(&id.mean_coherence, nc)),
            f64s("coverage", vec![nc], "", pad(&id.coverage, nc)),
            f64s(
                "response_peak",
                vec![nc],
                &names.control_unit,
                pad(&id.response_pk, nc),
            ),
        ],
    ));
    out
}

/// What the module found before the first pulse: each control's reachable share,
/// its floor, whether its best reachable response passes, and the seed's
/// predicted peak over its target's (`seed`, kept as arrays).
pub fn seed_results(sr: &SeedReport, names: &Names) -> Vec<Described> {
    let nc = names.controls.len();
    let pad = |x: &[f64]| -> Vec<f64> {
        (0..nc)
            .map(|i| x.get(i).copied().unwrap_or(f64::NAN))
            .collect()
    };
    let d = described(
        "mimo_shock_seed",
        json!({"kind": "none"}),
        json!({"responses": names.controls}),
        "before the first pulse: each control's reachable share of its target's energy, its \
         floor (dB), whether its best reachable response passes its judge (1 pass, 0 fail), \
         and the seed's predicted peak over its target's at the first level",
    );
    vec![(
        "seed".into(),
        d,
        vec![
            f64s("reachable", vec![nc], "", pad(&sr.reachable_fraction)),
            f64s("floor", vec![nc], "dB", pad(&sr.floor_db)),
            f64s(
                "reachable_ok",
                vec![nc],
                "",
                (0..nc)
                    .map(|i| match sr.reachable_ok.get(i) {
                        Some(true) => 1.0,
                        Some(false) => 0.0,
                        None => f64::NAN,
                    })
                    .collect(),
            ),
            f64s(
                "predicted_peak_ratio",
                vec![nc],
                "",
                pad(&sr.predicted_peak_ratio),
            ),
        ],
    )]
}

/// The places each repeating result has reached: the next record of each.
#[derive(Debug, Clone, Copy, Default)]
pub struct Records {
    pub srs: usize,
    pub drives: usize,
    pub law_calls: usize,
}

fn flag(b: bool) -> f64 {
    if b {
        1.0
    } else {
        0.0
    }
}

/// One pulse's measured SRS per control, with each control's target at the level
/// fired and its band: the module's judge computes it on the natural frequencies
/// from the measured record (`hertzforge_dsp::srs`), as the pulse's view shows it.
pub struct PulseSrs {
    pub natural_hz: Vec<f64>,
    /// Control after control.
    pub measured: Vec<f64>,
    pub target: Vec<f64>,
    pub band_low: Vec<f64>,
    pub band_high: Vec<f64>,
}

/// One pulse's results, each a record of a repeating result written when the pulse
/// is judged: each control's SRS where the module computes one (`pulse_srs`), the
/// drives it played (`pulse_drives`), and its levels, each control's measures and
/// verdict and each drive's peak and scale (`pulses`). `frame` is the station frame
/// when it was kept.
pub fn pulse_results(
    p: &PulseReport,
    entry: Option<&PulseEntry>,
    srs: Option<&PulseSrs>,
    records: &mut Records,
    frame: Option<u64>,
    names: &Names,
) -> Vec<(Described, Value)> {
    let (nc, nd, n) = (names.controls.len(), names.drives.len(), names.record_len);
    let fs_hz = names.fs_hz;
    let unit = names.control_unit.as_str();
    let mut out = Vec::new();
    if let Some(s) = srs {
        let m = s.natural_hz.len();
        if m > 0 && s.measured.len() == nc * m && s.target.len() == nc * m {
            let mut d = described(
                "srs",
                frequency_axis(),
                json!({"responses": names.controls}),
                "each pulse's SRS at each control, and each control's target at the level fired \
                 with the band it passes in (band)",
            );
            d.insert("srs_kind".into(), Value::from("maximax"));
            d.insert(
                "records".into(),
                json!({"index": "record", "pulse": "the pulse, from 0",
                       "level_db": "the level fired", "frame": "the station frame when it was kept"}),
            );
            out.push((
                (
                    "pulse_srs".to_owned(),
                    d,
                    vec![
                        f64s("frequency", vec![m], "Hz", s.natural_hz.clone()),
                        f64s("values", vec![nc, m], unit, s.measured.clone()),
                        f64s("reference", vec![nc, m], unit, s.target.clone()),
                        f64s("band_low", vec![nc, m], unit, s.band_low.clone()),
                        f64s("band_high", vec![nc, m], unit, s.band_high.clone()),
                    ],
                ),
                json!({"index": records.srs, "pulse": p.index, "level_db": num(p.level_db),
                       "frame": frame}),
            ));
            records.srs += 1;
        }
    }
    if let Some(e) = entry.filter(|e| e.drives.len() == nd * n) {
        let mut d = described(
            "mimo_shock_drives",
            sample_axis(fs_hz),
            json!({"references": names.drives}),
            "the drives each pulse played, after the module's ceilings and limits, time from \
             the record's start",
        );
        d.insert(
            "records".into(),
            json!({"index": "record", "pulse": "the pulse, from 0", "level_db": "the level fired"}),
        );
        out.push((
            (
                "pulse_drives".to_owned(),
                d,
                vec![f64s(
                    "values",
                    vec![nd, n],
                    &names.drive_unit,
                    e.drives.clone(),
                )],
            ),
            json!({"index": records.drives, "pulse": p.index, "level_db": num(e.level_db)}),
        ));
        records.drives += 1;
    }
    let per_control = |name: &str, u: &str, f: &dyn Fn(usize) -> f64| {
        f64s(name, vec![nc], u, (0..nc).map(f).collect())
    };
    let c = |k: usize| p.controls.get(k);
    let nan = f64::NAN;
    let mut d = described(
        "pulse_results",
        json!({"kind": "none"}),
        json!({"responses": names.controls, "references": names.drives}),
        "each pulse as judged: its levels and joint verdict, and per control and per drive \
         the measures its row shows (1 pass, 0 fail, NaN not judged)",
    );
    d.insert(
        "records".into(),
        json!({"index": "the pulse, from 0", "frame": "the station frame when it was kept",
               "at_s": "the pulse's start, seconds from arm"}),
    );
    let one = |name: &str, u: &str, v: f64| f64s(name, vec![1], u, vec![v]);
    out.push((
        (
            "pulses".to_owned(),
            d,
            vec![
                one("level_db", "dB", p.level_db),
                one("scheduled_db", "dB", p.scheduled_db),
                one("passed", "", flag(p.passed)),
                one("over_test", "dB", p.over_test_db),
                one(
                    "global_srs_error",
                    "dB",
                    p.global_srs_error_db.unwrap_or(nan),
                ),
                one("arrival_spread", "s", p.arrival_spread_s),
                per_control("control_passed", "", &|k| {
                    c(k).map_or(nan, |x| flag(x.verdict.passed))
                }),
                per_control("error", "dB", &|k| c(k).map_or(nan, |x| x.verdict.error_db)),
                per_control("control_over_test", "dB", &|k| {
                    c(k).map_or(nan, |x| x.over_test_db)
                }),
                per_control("delay", "s", &|k| c(k).map_or(nan, |x| x.verdict.delay_s)),
                per_control("peak", unit, &|k| c(k).map_or(nan, |x| x.peak)),
                per_control("peak_ratio", "", &|k| {
                    c(k).map_or(nan, |x| x.verdict.peak_ratio)
                }),
                per_control("waveform_error", "dB", &|k| {
                    c(k).map_or(nan, |x| x.waveform_error_db)
                }),
                per_control("scaled_margin", "dB", &|k| {
                    c(k).map_or(nan, |x| x.scaled_margin_db)
                }),
                per_control("absolute_margin", "dB", &|k| {
                    c(k).map_or(nan, |x| x.absolute_margin_db)
                }),
                per_control("shape_ok", "", &|k| c(k).map_or(nan, |x| flag(x.shape_ok))),
                per_control("timing_ok", "", &|k| {
                    c(k).map_or(nan, |x| flag(x.timing_ok))
                }),
                per_control("polarity_ok", "", &|k| {
                    c(k).map_or(nan, |x| flag(x.polarity_ok))
                }),
                per_control("duration_ratio", "", &|k| {
                    c(k).and_then(|x| x.duration_ratio).unwrap_or(nan)
                }),
                per_control("duration_ok", "", &|k| {
                    c(k).map_or(nan, |x| flag(x.duration_ok))
                }),
                per_control("energy", "dB", &|k| c(k).map_or(nan, |x| x.energy_db)),
                per_control("rms_duration_ratio", "", &|k| {
                    c(k).map_or(nan, |x| x.rms_duration_ratio)
                }),
                per_control("peak_velocity", "in/s", &|k| {
                    c(k).map_or(nan, |x| x.velocity_ips)
                }),
                per_control("displacement_pk_pk", "in", &|k| {
                    c(k).map_or(nan, |x| x.displacement_pk_pk_in)
                }),
                per_control("predicted_peak_ratio", "", &|k| {
                    p.predicted_peak_ratio.get(k).copied().unwrap_or(nan)
                }),
                f64s(
                    "drive_peak",
                    vec![nd],
                    "V",
                    (0..nd)
                        .map(|k| p.drive_vpk.get(k).copied().unwrap_or(nan))
                        .collect(),
                ),
                f64s(
                    "limit_scale",
                    vec![nd],
                    "",
                    (0..nd)
                        .map(|k| p.limit_scale.get(k).copied().unwrap_or(nan))
                        .collect(),
                ),
            ],
        ),
        json!({"index": p.index, "level_db": num(p.level_db), "scheduled_db": num(p.scheduled_db),
               "passed": p.passed, "frame": frame, "at_s": num(p.at_s),
               "law": {"used": p.law_used, "miss": p.law_miss},
               "warnings": p.warnings,
               "step_note": p.step_note.as_ref().map_or(Value::Null, |s| json!({
                   "from_db": num(s.from_db), "planned_db": num(s.planned_db),
                   "taken_db": num(s.taken_db), "tolerance_db": num(s.tolerance_db)}))}),
    ));
    out
}

/// One call of the law, as a record of `law_calls`: the drives it returned at the
/// level it was asked for (NaN when it gave none) and the delays it expects at each
/// control (NaN when it gave none); the record's line says which call, after which
/// pulse, the level, whether its answer was used and why not, how long it took and
/// the values it reported.
pub fn law_call_results(
    call: &LawCallEntry,
    after_pulse: Option<u64>,
    records: &mut Records,
    names: &Names,
) -> (Described, Value) {
    let (nc, nd, n) = (names.controls.len(), names.drives.len(), names.record_len);
    let fs_hz = names.fs_hz;
    let out = call.outputs.as_ref();
    let drives = out
        .map(|o| o.drives.clone())
        .filter(|d| d.len() == nd * n)
        .unwrap_or_else(|| vec![f64::NAN; nd * n]);
    let delays = out
        .and_then(|o| o.delays_s.clone())
        .filter(|d| d.len() == nc)
        .unwrap_or_else(|| vec![f64::NAN; nc]);
    let mut d = described(
        "mimo_shock_law_calls",
        sample_axis(fs_hz),
        json!({"responses": names.controls, "references": names.drives}),
        "every call of the control law: the drives it returned, time from the record's start \
         (NaN when it gave none), and the delay it expects at each control (NaN when none)",
    );
    d.insert(
        "records".into(),
        json!({"index": "the call, from 0", "call": "seed or iterate",
               "after_pulse": "the pulse it followed, from 0 (null for the seed)",
               "level_db": "the level it was asked for", "used": "its answer was used",
               "miss": "why not", "call_ms": "how long it took",
               "values": "the values it reported"}),
    );
    let level = match (&call.inputs, call.seed_level_db) {
        (Some(i), _) => Some(i.next_level_db),
        (None, l) => l,
    };
    let line = json!({
        "index": records.law_calls,
        "call": match call.call {
            hertzforge_shock_common::law_guard::LawCall::Seed => "seed",
            hertzforge_shock_common::law_guard::LawCall::Iterate => "iterate",
        },
        "after_pulse": after_pulse,
        "level_db": level.map(num),
        "used": call.miss.is_none() && call.outputs.is_some(),
        "miss": call.miss.as_ref().map_or(Value::Null, |m| serde_json::to_value(m).unwrap_or(Value::Null)),
        "call_ms": num(call.call_us as f64 / 1e3),
        "values": out.map_or_else(Vec::new, |o| o.values.iter()
            .map(|v| json!({"key": v.key, "value": num(v.value)})).collect()),
    });
    records.law_calls += 1;
    (
        (
            "law_calls".to_owned(),
            d,
            vec![
                f64s("drives", vec![nd, n], &names.drive_unit, drives),
                f64s("delays", vec![nc], "s", delays),
            ],
        ),
        line,
    )
}
