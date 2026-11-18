//! What a MIMO Sine run puts in its record (`docs/run-folder.md`, "What a MIMO
//! Sine Run Saves"): the manifest, the table and the setup at arm, the two
//! streams (every input as read, every drive as played), and the results: each
//! control's reference amplitude, phase and bands on the run's band, each limit
//! channel's bound, the identification (every control's and limit channel's
//! response to every drive and its coherence), every control's, limit
//! channel's and drive's tracked amplitude and phase tick by tick with the
//! reference, the error, the notch and the readings, and every law call.
//!
//! The writing is the record's ([`crate::record`]); this module only says what
//! is written, in the reader's form (`hertzforge.io.run_folder`). Values are in
//! the channels' own units, as the table names them.

use super::config::{schedule_json, test_json, tracking_json, WireMsine};
use crate::protocol::num;
use crate::random::record as rrec;
use crate::record::{tap::TapLink, ArrayValues, Begin, ResultArray, StreamSpec};
use crate::sine::record::{profile_json, resolve_grid};
use crate::station::{source_of, StationPlan};
use crate::table::unit_name;
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::{Caps, DiscoveredDevice};
use hertzforge_mimo_sine::core::{reference_at, ShakerRating};
use hertzforge_mimo_sine::records::TickRecord;
use hertzforge_mimo_sine::service::IdentificationReport;
use hertzforge_mimo_sine::LawSetup;
use hertzforge_module_abi::StationClass;
use hertzforge_sdm::C64;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

/// Ticks of one record of `channel_levels` and of `control_trace`; the last
/// record of a run is filled out with NaN.
pub const TICK_BLOCK: usize = 16;

/// The holds a law is told, by their codes in `control_trace`.
pub const HOLDS: [&str; 5] = ["free", "notch", "held", "probe", "engage"];

/// The facts of an armed run its record begins with.
pub struct ArmFacts<'a> {
    pub run_id: &'a str,
    pub run_uid: &'a str,
    pub cfg: &'a WireMsine,
    pub table: &'a ChannelTable,
    pub revision: u64,
    pub plan: &'a StationPlan,
    pub class: StationClass,
    pub provenance: &'a hertzforge_station::ProvenanceBasis,
    pub devices: &'a [DiscoveredDevice],
    pub caps: &'a Caps,
    pub fs_hz: f64,
    pub tick_frames: usize,
    pub derived: Value,
    pub feasibility: &'a (&'static str, String),
    pub law: Value,
    pub ratings: &'a [ShakerRating],
    pub structure: Option<&'a str>,
    pub armed_unix: f64,
    pub utc_offset_s: Option<i64>,
    pub streams_off: Option<String>,
}

fn unit_of(table: &ChannelTable, ch: usize) -> String {
    table
        .channels
        .iter()
        .find(|c| c.index as usize == ch)
        .map_or_else(|| "g".to_owned(), |c| unit_name(&c.units).to_string())
}

fn setup_json(f: &ArmFacts<'_>) -> Value {
    let w = f.cfg;
    let s = &w.setup;
    let mut derived = f.derived.clone();
    if let Value::Object(m) = &mut derived {
        m.insert(
            "grid".into(),
            json!({"fs_hz": num(f.fs_hz), "tick_frames": f.tick_frames,
                   "tick_s": num(f.tick_frames as f64 / f.fs_hz), "actual": true}),
        );
    }
    let g = &s.guards;
    json!({
        "config": w.raw,
        "resolved": {
            "test": test_json(&s.test),
            "schedule": schedule_json(&s.schedule),
            "phase_specified": s.phase_specified,
            "controls": s.controls.iter().map(|c| json!({
                "channel": c.channel, "unit": unit_of(f.table, c.channel),
                "profile": profile_json(&c.profile),
                "phase": c.phase.iter().map(|p| json!({"freq_hz": num(p.freq_hz), "phase_deg": num(p.phase_deg)})).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "drives": s.drives.iter().map(|d| json!({
                "channel": d.channel, "unit": unit_of(f.table, d.channel),
                "ceiling_vpk": num(d.ceiling_vpk), "abort_vpk": num(d.abort_vpk),
            })).collect::<Vec<_>>(),
            "limits": s.limits.iter().map(|l| json!({
                "channel": l.channel, "unit": unit_of(f.table, l.channel),
                "profile": profile_json(&l.profile),
            })).collect::<Vec<_>>(),
            "aborts": {
                "abort_ticks": g.abort_ticks,
                "kinematic_monitor": g.kinematic_monitor,
                "divergence_s": num(g.divergence_s),
                "notch": {"smoothing_ticks": g.notch.smoothing_ticks,
                          "release_ramp_db_per_s": num(g.notch.release_ramp_db_per_s),
                          "near_bound_db": num(g.notch.near_bound_db)},
                "shaker_ratings": f.ratings.iter().map(|r| json!({
                    "control": s.controls.get(r.control).map(|c| c.channel),
                    "max_displacement_mm_pk": r.max_displacement_mm_pk.map(num),
                    "max_velocity_m_s_pk": r.max_velocity_m_s_pk.map(num),
                    "max_acceleration_g_pk": r.max_acceleration_g_pk.map(num),
                })).collect::<Vec<_>>(),
            },
            "identification": {
                "level_db": num(s.identification.level_db),
                "resolution_hz": num(s.identification.resolution_hz),
                "frames": s.identification.frames,
                "trust_coherence": num(s.identification.trust_coherence),
                "gate_coherence": num(s.identification.gate_coherence),
                "review": s.identification.review,
            },
            "control": {
                "tracking": tracking_json(s.tracking),
                "law": {"id": s.law.id, "params": s.law.params},
                "law_budget": num(g.law_budget),
                "law_misses": g.law_misses,
                "max_step_db": num(g.max_step_db),
                "max_rise_db_per_s": num(g.max_rise_db_per_s),
                "rotation_cap_deg": g.rotation_cap_deg.map(num),
            },
            "requests": {"honour": s.requests.honour, "max_hold_s": num(s.requests.max_hold_s),
                         "min_slow_factor": num(s.requests.min_slow_factor)},
            "acquisition": {"requested_fs_hz": num(w.fs_hz()), "fs_hz": num(f.fs_hz),
                            "tick_ms": num(w.tick_ms), "tick_frames": f.tick_frames},
            "simulator": {"structure": f.structure},
        },
        "derived": derived,
        "feasibility": {
            "inputs": w.raw.get("feasibility").cloned().unwrap_or(Value::Null),
            "verdict": f.feasibility.0,
            "message": f.feasibility.1,
        },
    })
}

/// The record a MIMO Sine run begins with, at arm.
pub fn begin(f: &ArmFacts<'_>, root: PathBuf, links: (TapLink, TapLink)) -> Begin {
    let w = f.cfg;
    let mut manifest = Map::new();
    manifest.insert("test".into(), Value::from("mimo_sine"));
    manifest.insert("run_uid".into(), Value::from(f.run_uid));
    manifest.insert("run_id".into(), Value::from(f.run_id));
    manifest.insert("title".into(), Value::from(w.title.as_str()));
    manifest.insert("operator".into(), Value::from(w.operator.as_str()));
    manifest.insert("article".into(), Value::from(w.article.as_str()));
    manifest.insert("notes".into(), Value::from(w.notes.as_str()));
    if let Some((document, clause)) = &w.specification {
        manifest.insert(
            "specification".into(),
            json!({"document": document, "clause": clause}),
        );
    }
    manifest.insert("metadata".into(), w.metadata.clone());
    manifest.insert("source".into(), Value::from(source_of(f.class)));
    if let Some(sim) = rrec::sim_value(f.plan, f.class) {
        manifest.insert("sim".into(), sim);
    }
    manifest.insert(
        "station".into(),
        rrec::station_member(
            matches!(f.plan, StationPlan::Sim { .. }),
            f.class,
            f.provenance,
            f.devices,
            f.caps,
        ),
    );
    manifest.insert(
        "grid".into(),
        json!({"requested_fs_hz": num(w.fs_hz()), "fs_hz": num(f.fs_hz),
               "tick_frames": f.tick_frames, "tick_s": num(f.tick_frames as f64 / f.fs_hz)}),
    );
    manifest.insert("build".into(), rrec::build_value(f.provenance.core_build));
    manifest.insert(
        "armed".into(),
        json!({"unix": f.armed_unix, "utc_offset_s": f.utc_offset_s}),
    );
    manifest.insert(
        "start".into(),
        rrec::start_json(f.armed_unix, f.utc_offset_s, 0),
    );
    manifest.insert(
        "recording".into(),
        match &f.streams_off {
            None => json!({"streams": "on"}),
            Some(why) => json!({"streams": "off", "why": why}),
        },
    );
    let mut streams = Vec::new();
    if f.streams_off.is_none() {
        let mut inputs = Map::new();
        inputs.insert("kind".into(), Value::from("inputs"));
        inputs.insert("fs_hz".into(), num(f.fs_hz));
        inputs.insert(
            "channels".into(),
            Value::Array(
                rrec::input_rows(f.table)
                    .into_iter()
                    .map(rrec::stream_channel_value)
                    .collect(),
            ),
        );
        inputs.insert("first_station_frame".into(), Value::from(0));
        streams.push(StreamSpec {
            name: "inputs".into(),
            description: inputs,
            link: links.0,
        });
        let drives = rrec::drive_rows(f.table);
        if !drives.is_empty() {
            let mut d = Map::new();
            d.insert("kind".into(), Value::from("drives"));
            d.insert("fs_hz".into(), num(f.fs_hz));
            d.insert(
                "channels".into(),
                Value::Array(drives.into_iter().map(rrec::stream_channel_value).collect()),
            );
            d.insert("first_station_frame".into(), Value::from(0));
            d.insert(
                "output_latency_frames".into(),
                match &f.caps.output_buffer {
                    Some(b) => json!({"frames": b.min_lookahead_frames, "how": "declared"}),
                    None => json!({"frames": null, "how": "unknown"}),
                },
            );
            streams.push(StreamSpec {
                name: "drives".into(),
                description: d,
                link: links.1,
            });
        }
    }
    let name = format!("{}_mimo_sine_{}", rrec::utc_stamp(f.armed_unix), f.run_id);
    Begin {
        root,
        name,
        manifest,
        table: rrec::table_json(f.table, f.revision),
        setup: setup_json(f),
        law: Some((f.law.clone(), None)),
        log: Vec::new(),
        streams,
    }
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

fn described(kind: &str, abscissa: Value, channels: Value) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("kind".into(), Value::from(kind));
    m.insert("abscissa".into(), abscissa);
    m.insert("channels".into(), channels);
    m
}

fn freq_axis() -> Value {
    json!({"kind": "frequency", "unit": "Hz", "array": "frequency"})
}

/// The common unit of `units`, or empty when they differ.
fn common(units: &[String]) -> String {
    match units.first() {
        Some(u) if units.iter().all(|x| x == u) => u.clone(),
        _ => String::new(),
    }
}

/// Every control's reference at full level and its bands, on the run's band:
/// every control's breakpoints and points between them, merged.
pub fn reference_result(w: &WireMsine, table: &ChannelTable) -> rrec::Described {
    let s = &w.setup;
    let band = w.band();
    let mut freqs: Vec<f64> = s
        .controls
        .iter()
        .flat_map(|c| resolve_grid(&c.profile, band))
        .collect();
    freqs.sort_by(|a, b| a.total_cmp(b));
    freqs.dedup_by(|a, b| (*a - *b).abs() <= 1e-9 * b.abs().max(1.0));
    let (n, nc) = (freqs.len(), s.controls.len());
    let mut r = vec![C64::new(0.0, 0.0); nc];
    let mut amp = Vec::with_capacity(nc * n);
    let mut phase = Vec::with_capacity(nc * n);
    let per = |g: &dyn Fn(&hertzforge_sine::SineProfile, f64) -> Option<f64>| -> Vec<f64> {
        s.controls
            .iter()
            .flat_map(|c| freqs.iter().map(|f| g(&c.profile, *f).unwrap_or(f64::NAN)))
            .collect()
    };
    let mut at: Vec<Vec<C64>> = Vec::with_capacity(n);
    for f in &freqs {
        reference_at(s, *f, &mut r);
        at.push(r.clone());
    }
    for c in 0..nc {
        for row in &at {
            amp.push(row[c].norm());
            phase.push(if s.phase_specified {
                row[c].arg().to_degrees()
            } else {
                f64::NAN
            });
        }
    }
    let units: Vec<String> = s
        .controls
        .iter()
        .map(|c| unit_of(table, c.channel))
        .collect();
    let unit = common(&units);
    let mut d = described(
        "mimo_sine_reference",
        freq_axis(),
        json!({"controls": s.controls.iter().map(|c| c.channel).collect::<Vec<_>>()}),
    );
    d.insert(
        "description".into(),
        Value::from(
            "every control's reference amplitude and phase at full level, peak, and its alarm \
             and abort bands, on the run's band: every control's breakpoints and the points \
             between them; the phase is NaN where the run's phases are not specified",
        ),
    );
    d.insert("phase_specified".into(), Value::from(s.phase_specified));
    (
        "reference".into(),
        d,
        vec![
            f64s("frequency", vec![n], "Hz", freqs.clone()),
            f64s("reference", vec![nc, n], &unit, amp),
            f64s("phase", vec![nc, n], "deg", phase),
            f64s(
                "alarm_low",
                vec![nc, n],
                &unit,
                per(&|p, f| p.alarm_bounds(f).map(|b| b.0)),
            ),
            f64s(
                "alarm_high",
                vec![nc, n],
                &unit,
                per(&|p, f| p.alarm_bounds(f).map(|b| b.1)),
            ),
            f64s(
                "abort_low",
                vec![nc, n],
                &unit,
                per(&|p, f| p.abort_bounds(f).map(|b| b.0)),
            ),
            f64s(
                "abort_high",
                vec![nc, n],
                &unit,
                per(&|p, f| p.abort_bounds(f).map(|b| b.1)),
            ),
        ],
    )
}

/// Each limit channel's bound and its abort bound at full level, on the run's
/// band.
pub fn limit_results(w: &WireMsine, table: &ChannelTable) -> Vec<rrec::Described> {
    w.setup
        .limits
        .iter()
        .map(|l| {
            let freqs = resolve_grid(&l.profile, w.band());
            let n = freqs.len();
            let unit = unit_of(table, l.channel);
            let mut d = described("limit", freq_axis(), json!({"responses": [l.channel]}));
            d.insert(
                "description".into(),
                Value::from(
                    "a limit channel's lines at full level, peak: the bound the notch holds it \
                     under and its abort bound",
                ),
            );
            (
                format!("limit_ch{}", l.channel),
                d,
                vec![
                    f64s("frequency", vec![n], "Hz", freqs.clone()),
                    f64s(
                        "limit",
                        vec![n],
                        &unit,
                        freqs
                            .iter()
                            .map(|f| l.profile.target_accel_g_pk(*f).unwrap_or(f64::NAN))
                            .collect(),
                    ),
                    f64s(
                        "abort_high",
                        vec![n],
                        &unit,
                        freqs
                            .iter()
                            .map(|f| l.profile.abort_bounds(*f).map_or(f64::NAN, |b| b.1))
                            .collect(),
                    ),
                ],
            )
        })
        .collect()
}

/// The identification: every control's and limit channel's response to every
/// drive on the identification's grid inside the band (kind `frf`), each one's
/// multiple coherence (kind `coherence`) and the lines it trusts.
pub fn identification_results(
    rep: &IdentificationReport,
    w: &WireMsine,
    table: &ChannelTable,
) -> Vec<rrec::Described> {
    let s = &w.setup;
    let id = &rep.identification;
    let (nc, nl, m) = (s.controls.len(), s.limits.len(), s.drives.len());
    let lines = id.frequency_hz.len();
    if lines == 0 {
        return Vec::new();
    }
    let rows: Vec<usize> = s
        .controls
        .iter()
        .map(|c| c.channel)
        .chain(s.limits.iter().map(|l| l.channel))
        .collect();
    let refs: Vec<usize> = s.drives.iter().map(|d| d.channel).collect();
    let mut h = Vec::with_capacity((nc + nl) * m * lines);
    for r in 0..nc + nl {
        for d in 0..m {
            for k in 0..lines {
                let v = if r < nc {
                    id.response.get(k, r, d)
                } else {
                    id.limit_response.get(k, r - nc, d)
                };
                h.push((v.re, v.im));
            }
        }
    }
    let mut coh = Vec::with_capacity((nc + nl) * lines);
    coh.extend_from_slice(&id.coherence[..nc * lines]);
    coh.extend_from_slice(&id.limit_coherence[..nl * lines]);
    let in_units: Vec<String> = rows.iter().map(|c| unit_of(table, *c)).collect();
    let d_units: Vec<String> = refs.iter().map(|c| unit_of(table, *c)).collect();
    let unit = format!("{}/{}", common(&in_units), common(&d_units));
    let channels = json!({"responses": rows, "references": refs});
    let words = format!(
        "identification: every control's and limit channel's response to every drive, the \
         response taken {} frames later (the lag the identification measured); a delay lags",
        rep.lag_frames
    );
    let mut frf = described("frf", freq_axis(), channels.clone());
    frf.insert("convention".into(), Value::from("file"));
    frf.insert("description".into(), Value::from(words.clone()));
    frf.insert("lag_frames".into(), Value::from(rep.lag_frames));
    let mut c = described("coherence", freq_axis(), json!({"responses": rows}));
    c.insert(
        "description".into(),
        Value::from(format!("{words}: each response's multiple coherence")),
    );
    let mut t = described("mimo_sine_trust", freq_axis(), json!({}));
    t.insert(
        "description".into(),
        Value::from("1 where the identification's line is trusted, 0 where not"),
    );
    let freqs = id.frequency_hz.clone();
    vec![
        (
            "identification_frf".into(),
            frf,
            vec![
                f64s("frequency", vec![lines], "Hz", freqs.clone()),
                ResultArray {
                    name: "values".into(),
                    shape: vec![nc + nl, m, lines],
                    unit,
                    values: ArrayValues::C128(rrec::frf_to_file(&h)),
                },
            ],
        ),
        (
            "identification_coherence".into(),
            c,
            vec![
                f64s("frequency", vec![lines], "Hz", freqs.clone()),
                f64s("values", vec![nc + nl, lines], "", coh),
            ],
        ),
        (
            "identification_trust".into(),
            t,
            vec![
                f64s("frequency", vec![lines], "Hz", freqs),
                f64s(
                    "trusted",
                    vec![lines],
                    "",
                    id.trusted.iter().map(|b| f64::from(u8::from(*b))).collect(),
                ),
            ],
        ),
    ]
}

/// The identification's facts for the manifest, with what the law was told
/// of the loop's delays.
pub fn identification_json(rep: &IdentificationReport, law: Option<&LawSetup>) -> Value {
    json!({
        "level_db": rep.level_db.map(num),
        "lag_frames": rep.lag_frames,
        "record_len": rep.record_len,
        "realizations": rep.realizations,
        "line_spacing_hz": num(rep.line_spacing_hz),
        "duration_s": num(rep.duration_s),
        "drive_vrms": rep.drive_rms_v.iter().map(|v| num(*v)).collect::<Vec<_>>(),
        "coherence_mean": rep.band_coherence.iter().map(|v| num(*v)).collect::<Vec<_>>(),
        "gate_passed": rep.gate_passed,
        "modes_found": rep.modes_found,
        "sweep_rate_warning": rep.sweep_rate_warning.iter().map(|r| json!({
            "freq_hz": num(r.f_hz), "q": num(r.q), "seconds_in_bandwidth": num(r.seconds_in_bandwidth),
            "time_constants": num(r.time_constants), "flagged": r.flagged})).collect::<Vec<_>>(),
        "tracker_delay_factor": law.map(|l| num(l.tracker_delay_factor)),
        "transport_delay_s": law.map(|l| num(l.transport_delay_s)),
    })
}

/// What the run keeps of each tick between records.
#[derive(Debug, Clone)]
pub struct Kept {
    pub ticks: Vec<TickRecord>,
    /// The one-way pass each tick belongs to, from 0.
    pub pass: Vec<f64>,
}

impl Kept {
    pub fn new() -> Self {
        Kept {
            ticks: Vec::with_capacity(TICK_BLOCK),
            pass: Vec::with_capacity(TICK_BLOCK),
        }
    }
}

impl Default for Kept {
    fn default() -> Self {
        Self::new()
    }
}

fn padded(mut v: Vec<f64>, n: usize) -> Vec<f64> {
    v.resize(n, f64::NAN);
    v
}

fn wrap_deg(x: f64) -> f64 {
    let d = (x + 180.0).rem_euclid(360.0) - 180.0;
    if d <= -180.0 {
        d + 360.0
    } else {
        d
    }
}

/// One record of `channel_levels`: [`TICK_BLOCK`] ticks, the unused ones NaN.
/// Every control's, limit channel's and drive's tracked amplitude and phase
/// (`values`, `phase`, channels in that order), with the tone's frequency,
/// the scheduled level, the sweep's direction and pass, each control's
/// reference and error, the notch and the readings.
pub fn levels_result(
    k: &Kept,
    channels: &[(usize, String)],
    controls: usize,
    limits: usize,
    tick_s: f64,
) -> rrec::Described {
    let n = TICK_BLOCK;
    let t = &k.ticks;
    let col = |g: &dyn Fn(&TickRecord) -> f64| padded(t.iter().map(g).collect(), n);
    let per = |count: usize, g: &dyn Fn(&TickRecord, usize) -> f64| -> Vec<f64> {
        (0..count)
            .flat_map(|c| padded(t.iter().map(|r| g(r, c)).collect(), n))
            .collect()
    };
    let rows = channels.len();
    let value = |r: &TickRecord, c: usize| -> C64 {
        if c < controls {
            r.control[c]
        } else if c < controls + limits {
            r.limit[c - controls]
        } else {
            r.drive[c - controls - limits]
        }
    };
    let units: Vec<String> = channels.iter().map(|(_, u)| u.clone()).collect();
    let cu = common(&units[..controls]);
    let mut d = described(
        "sine_levels",
        json!({"kind": "time", "unit": "s", "array": "time"}),
        json!({"responses": channels.iter().map(|(i, _)| i).collect::<Vec<_>>()}),
    );
    d.insert(
        "description".into(),
        Value::from(
            "every control's, limit channel's and drive's tracked amplitude, peak, and phase \
             against the tone, tick by tick, with the tone's frequency, the scheduled level, the \
             sweep's direction and pass, each control's reference and error, the notch and the \
             readings; time from engage; a tick is NaN where a record was not filled",
        ),
    );
    d.insert(
        "records".into(),
        json!({"index": "record", "points": "the ticks the record holds before its NaN fill"}),
    );
    d.insert(
        "units".into(),
        json!(channels
            .iter()
            .map(|(i, u)| json!({"channel": i, "unit": u}))
            .collect::<Vec<_>>()),
    );
    d.insert(
        "roles".into(),
        json!({"controls": controls, "limits": limits, "drives": rows - controls - limits}),
    );
    let _ = tick_s;
    let nc = controls;
    (
        "channel_levels".into(),
        d,
        vec![
            f64s("time", vec![n], "s", col(&|r| r.time_s)),
            f64s("frequency", vec![n], "Hz", col(&|r| r.frequency_hz)),
            f64s("level_db", vec![n], "dB", col(&|r| r.level_db)),
            f64s(
                "direction",
                vec![n],
                "",
                col(&|r| r.sweep_hz_per_s.signum() * f64::from(u8::from(r.sweep_hz_per_s != 0.0))),
            ),
            f64s("pass", vec![n], "", padded(k.pass.clone(), n)),
            f64s("settling", vec![n], "", col(&|r| f64::from(u8::from(r.settling)))),
            f64s(
                "values",
                vec![rows, n],
                &common(&units),
                per(rows, &|r, c| value(r, c).norm()),
            ),
            f64s(
                "phase",
                vec![rows, n],
                "deg",
                per(rows, &|r, c| wrap_deg(value(r, c).arg().to_degrees())),
            ),
            f64s(
                "reference",
                vec![nc, n],
                &cu,
                per(nc, &|r, c| r.judged_reference[c].norm()),
            ),
            f64s(
                "reference_phase",
                vec![nc, n],
                "deg",
                per(nc, &|r, c| r.reference[c].arg().to_degrees()),
            ),
            f64s(
                "error_db",
                vec![nc, n],
                "dB",
                per(nc, &|r, c| r.error_db[c]),
            ),
            f64s(
                "limit_bound",
                vec![limits, n],
                &common(&units[nc..nc + limits]),
                per(limits, &|r, c| r.limit_bound[c]),
            ),
            f64s("notch_scale", vec![n], "", col(&|r| r.notch_scale)),
            f64s("notch_depth", vec![n], "", col(&|r| r.notch_depth)),
            f64s("notched", vec![n], "", col(&|r| f64::from(u8::from(r.notched)))),
            f64s("opposition_ratio", vec![n], "", col(&|r| r.opposition_ratio)),
            f64s("largest_share", vec![n], "", col(&|r| r.largest_share)),
        ],
    )
}

/// One record of `control_trace`: [`TICK_BLOCK`] ticks, the unused ones NaN:
/// every law call's inputs and answer, what the guards did with it, and the
/// law's values.
pub fn trace_result(k: &Kept, drives: &[usize], diagnostics: &[String]) -> rrec::Described {
    let n = TICK_BLOCK;
    let t = &k.ticks;
    let m = drives.len();
    let col = |g: &dyn Fn(&TickRecord) -> f64| padded(t.iter().map(g).collect(), n);
    let per = |count: usize, g: &dyn Fn(&TickRecord, usize) -> f64| -> Vec<f64> {
        (0..count)
            .flat_map(|c| padded(t.iter().map(|r| g(r, c)).collect(), n))
            .collect()
    };
    let cplx = |g: &dyn Fn(&TickRecord, usize) -> C64| -> (Vec<f64>, Vec<f64>) {
        (
            per(m, &|r, c| g(r, c).re),
            per(m, &|r, c| g(r, c).im),
        )
    };
    let (fi_re, fi_im) = cplx(&|r, c| r.drive_in_force[c]);
    let (as_re, as_im) = cplx(&|r, c| r.drive_asked[c]);
    let (pl_re, pl_im) = cplx(&|r, c| r.drive[c]);
    let mut d = described(
        "mimo_sine_control",
        json!({"kind": "time", "unit": "s", "array": "time"}),
        json!({"drives": drives}),
    );
    d.insert(
        "description".into(),
        Value::from(
            "every tick of the control law: whether it was called, the drive in force it was \
             told, the drive it asked for, the drive played after every guard (each drive's \
             real and imaginary parts, V peak), the miss, the call's time, the request and its \
             outcome, the caps and scales that bound, and the law's declared values; time from \
             engage; a tick is NaN where a record was not filled",
        ),
    );
    d.insert("diagnostics".into(), json!(diagnostics));
    d.insert(
        "miss".into(),
        json!([
            "error",
            "panicked",
            "over_budget",
            "over_memory",
            "wrong_drive_count",
            "non_finite",
            "too_many_values",
            "undeclared_value",
            "bad_request",
            "wrong_reference_count"
        ]),
    );
    d.insert("hold".into(), json!(HOLDS));
    d.insert("request".into(), json!(["hold", "slow"]));
    d.insert(
        "request_outcome".into(),
        json!(["none", "granted", "bounded", "not_honoured", "not_sweeping", "hold_too_long"]),
    );
    let miss_code = |r: &TickRecord| match r.miss {
        None => -1.0,
        Some(s) => [
            "error",
            "panicked",
            "over_budget",
            "over_memory",
            "wrong_drive_count",
            "non_finite",
            "too_many_values",
            "undeclared_value",
            "bad_request",
            "wrong_reference_count",
        ]
        .iter()
        .position(|x| *x == s)
        .map_or(99.0, |p| p as f64),
    };
    let hold_code = |r: &TickRecord| {
        HOLDS
            .iter()
            .position(|x| *x == r.hold.as_str())
            .map_or(-1.0, |p| p as f64)
    };
    let request_code = |r: &TickRecord| match r.request {
        None => -1.0,
        Some(hertzforge_mimo_sine::Request::Hold) => 0.0,
        Some(hertzforge_mimo_sine::Request::Slow(_)) => 1.0,
    };
    let factor = |r: &TickRecord| match r.request {
        Some(hertzforge_mimo_sine::Request::Slow(f)) => f,
        _ => f64::NAN,
    };
    let outcome_code = |r: &TickRecord| {
        [
            "none",
            "granted",
            "bounded",
            "not_honoured",
            "not_sweeping",
            "hold_too_long",
        ]
        .iter()
        .position(|x| *x == r.request_outcome.as_str())
        .map_or(-1.0, |p| p as f64)
    };
    let k_values = diagnostics.len();
    let mut arrays = vec![
        f64s("time", vec![n], "s", col(&|r| r.time_s)),
        f64s("tick", vec![n], "", col(&|r| r.index as f64)),
        f64s("frequency", vec![n], "Hz", col(&|r| r.frequency_hz)),
        f64s("dt", vec![n], "s", col(&|r| r.dt_s)),
        f64s("called", vec![n], "", col(&|r| f64::from(u8::from(r.law_called)))),
        f64s("engage", vec![n], "", col(&|r| f64::from(u8::from(r.engage)))),
        f64s("hold", vec![n], "", col(&hold_code)),
        f64s("drive_in_force_re", vec![m, n], "V", fi_re),
        f64s("drive_in_force_im", vec![m, n], "V", fi_im),
        f64s("drive_asked_re", vec![m, n], "V", as_re),
        f64s("drive_asked_im", vec![m, n], "V", as_im),
        f64s("drive_re", vec![m, n], "V", pl_re),
        f64s("drive_im", vec![m, n], "V", pl_im),
        f64s("miss", vec![n], "", col(&miss_code)),
        f64s("call_s", vec![n], "s", col(&|r| r.law_s)),
        f64s("faulted", vec![n], "", col(&|r| f64::from(u8::from(r.law_faulted)))),
        f64s("request", vec![n], "", col(&request_code)),
        f64s("request_factor", vec![n], "", col(&factor)),
        f64s("request_outcome", vec![n], "", col(&outcome_code)),
        f64s("ceiling_scale", vec![n], "", col(&|r| r.ceiling_scale)),
        f64s("first_drive_scale", vec![n], "", col(&|r| r.first_drive_scale)),
        f64s("cap_bound", vec![n], "", col(&|r| f64::from(u8::from(r.cap_bound)))),
        f64s(
            "rotation_bound",
            vec![n],
            "",
            col(&|r| f64::from(u8::from(r.rotation_bound))),
        ),
        f64s(
            "largest_rotation",
            vec![n],
            "deg",
            col(&|r| r.largest_rotation_deg),
        ),
    ];
    if k_values > 0 {
        arrays.push(f64s(
            "values",
            vec![k_values, n],
            "",
            per(k_values, &|r, c| r.values[c]),
        ));
    }
    ("control_trace".into(), d, arrays)
}
