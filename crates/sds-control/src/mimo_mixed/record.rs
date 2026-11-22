//! What a MIMO Mixed Mode run puts in its record (`docs/run-folder.md`, "What a
//! MIMO Mixed Mode Run Saves"): the manifest, the table and the setup at arm, the
//! two streams, and the results: the reference with its alarm and abort lines,
//! the identification, the controls' composite and residual spectra and the
//! drives' spectra as the run went, each tone's amplitude, phase and error per
//! control and its drive per drive, each band's level, each drive's headroom and
//! the tones' common scale, and every law call.
//!
//! The writing is the record's ([`crate::record`]); this module only says what is
//! written, in the reader's form (`hertzforge.io.run_folder`). Values are in the
//! channels' own units, as the table names them.

use super::config::{per_control, Derived, WireMimoMixed};
use crate::protocol::num;
use crate::random::record::{self as rrec, Described};
use crate::record::{tap::TapLink, ArrayValues, Begin, ResultArray, StreamSpec};
use crate::station::{source_of, StationPlan};
use crate::table::unit_name;
use hertzforge_channel_table::{Channel, ChannelTable};
use hertzforge_hal::{Caps, DiscoveredDevice};
use hertzforge_mimo_mixed::service::records::{LawRecord, LAW_TICKS_FORMAT, LAW_TICK_FIELDS};
use hertzforge_mimo_mixed::service::{MimoMixedIdent, MimoMixedStatus, TonePhasors};
use hertzforge_mimo_mixed::{MimoMixedConfig, MimoPartBody};
use hertzforge_mixed::PartSchedule;
use hertzforge_module_abi::StationClass;
use hertzforge_sdm::{HermitianStack, MatrixStack};
use serde_json::{json, Map, Value};
use std::path::PathBuf;

/// Points of one record of the level histories, the last filled out with NaN.
pub const LEVEL_BLOCK: usize = 32;
/// Calls of one record of `control_trace`, the last filled out with NaN.
pub const TRACE_BLOCK: usize = 64;

/// The facts of an armed run its record begins with.
pub struct ArmFacts<'a> {
    pub run_id: &'a str,
    pub run_uid: &'a str,
    pub cfg: &'a WireMimoMixed,
    pub controller: &'a MimoMixedConfig,
    pub derived: &'a Derived,
    pub table: &'a ChannelTable,
    pub revision: u64,
    pub plan: &'a StationPlan,
    pub class: StationClass,
    pub provenance: &'a hertzforge_station::ProvenanceBasis,
    pub devices: &'a [DiscoveredDevice],
    pub caps: &'a Caps,
    pub fs_hz: f64,
    pub armed_unix: f64,
    pub utc_offset_s: Option<i64>,
    pub streams_off: Option<String>,
}

fn pair(p: (f64, f64)) -> Value {
    json!([num(p.0), num(p.1)])
}

fn per_control_json<T: Clone>(
    p: &hertzforge_mimo_mixed::config::PerControl<T>,
    n: usize,
    f: impl Fn(&T) -> Value,
) -> Value {
    Value::Array((0..n).map(|i| f(&per_control(p, i))).collect())
}

/// A part's schedule as the setup record states it.
fn schedule_json(s: &PartSchedule) -> Value {
    serde_json::to_value(s).unwrap_or(Value::Null)
}

/// Each part with its statements per control, its schedule and its window.
fn parts_json(c: &MimoMixedConfig) -> Value {
    let n = c.n();
    Value::Array(
        c.parts
            .iter()
            .map(|p| {
                let mut m = json!({
                    "id": p.id,
                    "name": if p.name.is_empty() { &p.id } else { &p.name },
                    "kind": super::config::part_kind(&p.body),
                    "enabled": p.enabled,
                    "window": serde_json::to_value(p.window).unwrap_or(Value::Null),
                });
                let o = m.as_object_mut().expect("an object");
                match &p.body {
                    MimoPartBody::Floor(f) => {
                        o.insert(
                            "alarm_db".into(),
                            per_control_json(&f.alarm_db, n, |x| pair(*x)),
                        );
                        o.insert(
                            "abort_db".into(),
                            per_control_json(&f.abort_db, n, |x| pair(*x)),
                        );
                    }
                    MimoPartBody::Tone(t) => {
                        o.insert("schedule".into(), schedule_json(&t.schedule));
                        o.insert(
                            "amplitude".into(),
                            per_control_json(&t.amplitude, n, |a| {
                                serde_json::to_value(a).unwrap_or(Value::Null)
                            }),
                        );
                        o.insert("phases_stated".into(), Value::from(t.phases_stated));
                        o.insert("phase_deg".into(), Value::from(t.phase_deg.clone()));
                        o.insert(
                            "alarm_db".into(),
                            per_control_json(&t.alarm_db, n, |x| pair(*x)),
                        );
                        o.insert(
                            "abort_db".into(),
                            per_control_json(&t.abort_db, n, |x| pair(*x)),
                        );
                    }
                    MimoPartBody::Narrowband(b) => {
                        o.insert("schedule".into(), schedule_json(&b.schedule));
                        o.insert(
                            "width".into(),
                            serde_json::to_value(b.width).unwrap_or(Value::Null),
                        );
                        o.insert(
                            "statement".into(),
                            serde_json::to_value(&b.statement).unwrap_or(Value::Null),
                        );
                        o.insert(
                            "alarm_db".into(),
                            per_control_json(&b.alarm_db, n, |x| pair(*x)),
                        );
                        o.insert(
                            "abort_db".into(),
                            per_control_json(&b.abort_db, n, |x| pair(*x)),
                        );
                    }
                    _ => {}
                }
                m
            })
            .collect(),
    )
}

/// The controller's configuration as the record states it: every member as the
/// controller took it, the reference by its summary (its values are the
/// `reference` result).
fn controller_json(c: &MimoMixedConfig) -> Value {
    let mut v = serde_json::to_value(c).unwrap_or(Value::Null);
    if let Value::Object(m) = &mut v {
        m.insert(
            "reference".into(),
            json!({"n": c.n(), "lines": c.reference.lines(), "df_hz": num(c.reference.df_hz),
                   "band": [c.reference.band.0, c.reference.band.1],
                   "labels": c.reference.labels, "unit": c.reference.unit,
                   "provenance": serde_json::to_value(&c.reference.provenance).unwrap_or(Value::Null),
                   "result": "reference"}),
        );
    }
    v
}

/// The law choices with their parameters, the rules, and both roles' laws.
pub fn law_json(c: &MimoMixedConfig, d: &Derived) -> Value {
    let pick = |p: &hertzforge_mixed::LawPick| json!({"id": p.id, "params": Value::Object(p.params.clone())});
    json!({
        "id": format!("{}+{}", c.law.floor.id, c.law.tones.id),
        "name": "MIMO Mixed Mode",
        "builtin": d.law.floor.builtin && d.law.tones.builtin,
        "floor": {"law": d.law.floor.to_json(), "choice": pick(&c.law.floor)},
        "tones": {"law": d.law.tones.to_json(), "choice": pick(&c.law.tones)},
        "bands": pick(&c.law.bands),
        "ceiling_rule": pick(&c.law.ceiling_rule),
        "fill_rule": pick(&c.law.fill_rule),
        "calls": {
            "format": LAW_TICKS_FORMAT,
            "record_bytes": LawRecord::BYTES,
            "fields": LAW_TICK_FIELDS.iter().map(|(n, t)| json!({"name": n, "type": t})).collect::<Vec<_>>(),
            "result": "control_trace",
        },
    })
}

fn setup_json(f: &ArmFacts<'_>) -> Value {
    let c = f.controller;
    let n = c.n();
    let bands = super::config::floor_bands(c);
    json!({
        "config": f.cfg.raw,
        "resolved": {
            "parts": parts_json(c),
            "reference": {"source": f.derived.source, "n": n, "labels": c.reference.labels,
                          "unit": c.reference.unit,
                          "band_hz": [num(c.reference.band.0 as f64 * c.reference.df_hz),
                                      num((c.reference.band.1 - 1) as f64 * c.reference.df_hz)],
                          "rms": f.derived.reference_rms.iter().map(|x| num(*x)).collect::<Vec<_>>(),
                          "result": "reference"},
            "controls": c.controls,
            "drives": c.drives.iter().map(|d| serde_json::to_value(d).unwrap_or(Value::Null)).collect::<Vec<_>>(),
            "limits": c.limits.iter().map(|l| serde_json::to_value(l).unwrap_or(Value::Null)).collect::<Vec<_>>(),
            "tolerances": (0..n).map(|i| json!({"channel": c.controls[i],
                "alarm_db": pair(bands[i].0), "abort_db": pair(bands[i].1)})).collect::<Vec<_>>(),
            "law": law_json(c, f.derived),
            "ceiling_rule": c.law.ceiling_rule.id,
            "fill_rule": c.law.fill_rule.id,
            "identification": serde_json::to_value(c.identification).unwrap_or(Value::Null),
            "gates": serde_json::to_value(c.gates).unwrap_or(Value::Null),
            "schedule": serde_json::to_value(&c.schedule).unwrap_or(Value::Null),
            "full_level_s": c.full_level_duration_s,
            "estimation": serde_json::to_value(c.estimation).unwrap_or(Value::Null),
            "acquisition": {"requested_fs_hz": num(f.cfg.fs_hz), "fs_hz": num(f.fs_hz),
                            "n_pts": f.cfg.n_pts, "control_tick_s": num(c.control_tick_s())},
            "controller": controller_json(c),
        },
        "derived": f.derived.to_json(),
    })
}

/// The record a MIMO Mixed Mode run begins with, at arm.
pub fn begin(f: &ArmFacts<'_>, root: PathBuf, links: (TapLink, TapLink)) -> Begin {
    let cfg = f.cfg;
    let mut manifest = Map::new();
    manifest.insert(
        "test".into(),
        Value::from(hertzforge_mimo_mixed::TEST_TYPE_ID),
    );
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
        json!({"requested_fs_hz": num(cfg.fs_hz), "fs_hz": num(f.fs_hz), "n_pts": cfg.n_pts,
               "df_hz": num(f.derived.df_hz), "lines": f.derived.lines}),
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
    let name = format!(
        "{}_{}_{}",
        rrec::utc_stamp(f.armed_unix),
        hertzforge_mimo_mixed::TEST_TYPE_ID,
        f.run_id
    );
    Begin {
        root,
        name,
        manifest,
        table: rrec::table_json(f.table, f.revision),
        setup: setup_json(f),
        law: Some((law_json(f.controller, f.derived), None)),
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

fn c16s(name: &str, shape: Vec<usize>, unit: &str, values: Vec<(f64, f64)>) -> ResultArray {
    ResultArray {
        name: name.into(),
        shape,
        unit: unit.into(),
        values: ArrayValues::C128(values),
    }
}

fn freq_described(
    kind: &str,
    band: (usize, usize),
    df: f64,
    channels: Value,
) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("kind".into(), Value::from(kind));
    m.insert(
        "abscissa".into(),
        json!({"kind": "frequency", "unit": "Hz", "start": band.0 as f64 * df, "step": df}),
    );
    m.insert("channels".into(), channels);
    m.insert("band".into(), json!([band.0, band.1]));
    m
}

fn time_described(kind: &str, channels: Value) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("kind".into(), Value::from(kind));
    m.insert(
        "abscissa".into(),
        json!({"kind": "time", "unit": "s", "array": "time"}),
    );
    m.insert("channels".into(), channels);
    m
}

/// A Hermitian stack on the band as `[n, n, lines]` in the file's convention
/// (`conj(G)` of the controller's `G = E[X X^H]`).
fn herm_file(g: &HermitianStack, band: (usize, usize)) -> Vec<(f64, f64)> {
    let n = g.n();
    let mut out = Vec::with_capacity(n * n * (band.1 - band.0));
    for r in 0..n {
        for c in 0..n {
            for line in band.0..band.1 {
                let v = g.get(line, r, c);
                out.push((v.re, -v.im));
            }
        }
    }
    out
}

fn mat_band(m: &MatrixStack, band: (usize, usize)) -> Vec<(f64, f64)> {
    let (_, rows, cols) = m.dims();
    let mut out = Vec::with_capacity(rows * cols * (band.1 - band.0));
    for r in 0..rows {
        for c in 0..cols {
            for line in band.0..band.1 {
                let v = m.get(line, r, c);
                out.push((v.re, v.im));
            }
        }
    }
    out
}

fn per_line(v: &[f64], n: usize, band: (usize, usize)) -> Vec<f64> {
    let mut out = Vec::with_capacity(n * (band.1 - band.0));
    for k in 0..n {
        for line in band.0..band.1 {
            out.push(v.get(line * n + k).copied().unwrap_or(f64::NAN));
        }
    }
    out
}

/// What the results name.
#[derive(Debug, Clone)]
pub struct Names {
    pub controls: Vec<u32>,
    pub labels: Vec<String>,
    pub limits: Vec<u32>,
    pub drives: Vec<u32>,
    pub tones: Vec<String>,
    pub bands: Vec<String>,
    pub control_unit: String,
    pub drive_unit: String,
    /// The floor's band, lines `[lo, hi)`, and the line spacing.
    pub band: (usize, usize),
    pub df_hz: f64,
}

impl Names {
    pub fn of(c: &MimoMixedConfig, table: &ChannelTable) -> Names {
        let unit = |ch: u32| {
            table
                .channels
                .iter()
                .find(|r| r.index == ch)
                .map_or_else(|| "g".to_owned(), |r| unit_name(&r.units).to_string())
        };
        Names {
            controls: c.controls.clone(),
            labels: c.reference.labels.clone(),
            limits: c.limits.iter().map(|l| l.channel_index).collect(),
            drives: c.drives.iter().map(|d| d.channel_index).collect(),
            tones: c.tones().map(|(_, p, _)| p.id.clone()).collect(),
            bands: c.bands().map(|(_, p, _)| p.id.clone()).collect(),
            control_unit: c
                .controls
                .first()
                .map_or_else(|| "g".into(), |ch| unit(*ch)),
            drive_unit: c
                .drives
                .first()
                .map_or_else(|| "V".into(), |d| unit(d.channel_index)),
            band: c.reference.band,
            df_hz: c.reference.df_hz,
        }
    }
}

/// The reference spectral density matrix at full level, with each control's
/// alarm and abort lines (MIMO Random's `sdm_reference`).
pub fn reference_result(c: &MimoMixedConfig, names: &Names, source: &str) -> Described {
    let r = &c.reference;
    let n = r.n();
    let band = r.band;
    let lb = band.1 - band.0;
    let unit = rrec::psd_unit(&names.control_unit);
    let mut d = freq_described(
        "sdm_reference",
        band,
        r.df_hz,
        json!({"responses": names.controls, "references": names.controls, "labels": names.labels}),
    );
    d.insert("convention".into(), Value::from("file"));
    d.insert("source".into(), Value::from(source));
    d.insert(
        "description".into(),
        Value::from(
            "the floor's reference spectral density matrix at full level, and each control's \
             alarm and abort lines as spectral densities (from the floor part's bands)",
        ),
    );
    let t = &r.tolerances;
    let lines = |v: &[f64]| per_line(v, n, band);
    (
        "reference".into(),
        d,
        vec![
            c16s("values", vec![n, n, lb], &unit, herm_file(&r.sdm, band)),
            f64s("alarm_upper", vec![n, lb], &unit, lines(&t.alarm_upper)),
            f64s("alarm_lower", vec![n, lb], &unit, lines(&t.alarm_lower)),
            f64s("abort_upper", vec![n, lb], &unit, lines(&t.abort_upper)),
            f64s("abort_lower", vec![n, lb], &unit, lines(&t.abort_lower)),
        ],
    )
}

/// The identification: the transfer function from each drive to each control
/// and limit, each control's multiple coherence, the controls' and the drives'
/// spectral matrices, and its facts.
pub fn identification_results(
    id: &MimoMixedIdent,
    band: (usize, usize),
    df: f64,
    names: &Names,
) -> Vec<Described> {
    let lb = band.1 - band.0;
    let (_, n_resp, n_d) = id.h.dims();
    let n = id.g_cc.n();
    let mut responses = names.controls.clone();
    responses.extend(&names.limits);
    let (cu, du) = (&names.control_unit, &names.drive_unit);
    let lag = id.lag_frames;
    let mut frf = freq_described(
        "frf",
        band,
        df,
        json!({"responses": responses, "references": names.drives}),
    );
    frf.insert("convention".into(), Value::from("file"));
    frf.insert("lag_frames".into(), Value::from(lag));
    frf.insert(
        "description".into(),
        Value::from(format!(
            "the identification measured in this run: the transfer function matrix from each \
             drive to each control and limit channel, the response taken {lag} frames later"
        )),
    );
    let mut coh = freq_described(
        "coherence",
        band,
        df,
        json!({"responses": names.controls, "labels": names.labels}),
    );
    coh.insert(
        "description".into(),
        Value::from("each control's multiple coherence with the drives"),
    );
    let mut g_cc = freq_described(
        "cpsd",
        band,
        df,
        json!({"responses": names.controls, "references": names.controls}),
    );
    g_cc.insert("convention".into(), Value::from("file"));
    g_cc.insert(
        "description".into(),
        Value::from("the controls' spectral matrix while the drives played"),
    );
    let mut g_dd = freq_described(
        "cpsd",
        band,
        df,
        json!({"responses": names.drives, "references": names.drives}),
    );
    g_dd.insert("convention".into(), Value::from("file"));
    g_dd.insert(
        "description".into(),
        Value::from("the drives' spectral matrix"),
    );
    let mut facts = freq_described(
        "mimo_identification",
        band,
        df,
        json!({"controls": names.controls, "drives": names.drives}),
    );
    facts.insert(
        "description".into(),
        Value::from("the identification's facts: its level against the reference, the steps of its level search, its frames and lag, each control's band coherence and each drive's rms"),
    );
    facts.insert("level_db".into(), num(id.level_db));
    facts.insert("frames".into(), Value::from(id.frames));
    facts.insert("steps".into(), Value::from(id.steps));
    facts.insert("lag_frames".into(), Value::from(lag));
    facts.insert("frames_ahead".into(), Value::from(id.frames_ahead));
    facts.insert("loaded".into(), Value::from(false));
    let psd = rrec::psd_unit;
    vec![
        (
            "identification_frf".into(),
            frf,
            vec![c16s(
                "values",
                vec![n_resp, n_d, lb],
                &format!("{cu}/{du}"),
                mat_band(&id.h, band),
            )],
        ),
        (
            "identification_coherence".into(),
            coh,
            vec![f64s(
                "values",
                vec![n, lb],
                "",
                per_line(&id.coherence, n, band),
            )],
        ),
        (
            "identification_response".into(),
            g_cc,
            vec![c16s(
                "values",
                vec![n, n, lb],
                &psd(cu),
                herm_file(&id.g_cc, band),
            )],
        ),
        (
            "identification_drive".into(),
            g_dd,
            vec![c16s(
                "values",
                vec![n_d, n_d, lb],
                &psd(du),
                herm_file(&id.g_dd, band),
            )],
        ),
        (
            "identification".into(),
            facts,
            vec![
                f64s("band_coherence", vec![n], "", id.band_coherence.clone()),
                f64s("drive_vrms", vec![n_d], du, id.drive_vrms.clone()),
            ],
        ),
    ]
}

/// One record of the spectra: per control the composite (tones in) and the
/// residual autospectra and the reference at the level; per drive the floor's
/// drive autospectrum at the level. The record's line names the time and level.
pub fn spectra_result(
    sp: &super::view::Spectra,
    band: (usize, usize),
    df: f64,
    names: &Names,
) -> Vec<Described> {
    let lb = band.1 - band.0;
    let n = sp.composite.len();
    let m = sp.drive.len();
    let flat = |v: &[Vec<f64>], k: usize| -> Vec<f64> {
        (0..k)
            .flat_map(|i| {
                let row = v.get(i).cloned().unwrap_or_default();
                (0..lb).map(move |l| row.get(l).copied().unwrap_or(f64::NAN))
            })
            .collect()
    };
    let cu = rrec::psd_unit(&names.control_unit);
    let du = rrec::psd_unit(&names.drive_unit);
    let mut c = freq_described(
        "mimo_mixed_spectra",
        band,
        df,
        json!({"responses": names.controls, "labels": names.labels}),
    );
    c.insert(
        "description".into(),
        Value::from(
            "per control, at the level in force: the composite autospectrum with the tones in \
             (the floor loop's frames averaged over the record), the residual autospectrum the \
             floor law controls (the tones taken out) and the reference",
        ),
    );
    let mut d = freq_described(
        "mimo_mixed_drive_spectra",
        band,
        df,
        json!({"drives": names.drives}),
    );
    d.insert(
        "description".into(),
        Value::from("per drive, at the level in force: the floor law's drive autospectrum"),
    );
    vec![
        (
            "control_spectra".into(),
            c,
            vec![
                f64s("composite", vec![n, lb], &cu, flat(&sp.composite, n)),
                f64s("residual", vec![n, lb], &cu, flat(&sp.residual, n)),
                f64s("reference", vec![n, lb], &cu, flat(&sp.reference, n)),
            ],
        ),
        (
            "drive_spectra".into(),
            d,
            vec![f64s("values", vec![m, lb], &du, flat(&sp.drive, m))],
        ),
    ]
}

/// One point of the level histories.
#[derive(Debug, Clone)]
pub struct LevelPoint {
    pub t_s: f64,
    pub level_db: f64,
    pub status: MimoMixedStatus,
    pub phasors: Vec<TonePhasors>,
    pub held_peak_v: Vec<f64>,
    pub peak_ceiling_v: Vec<f64>,
}

fn padded(mut v: Vec<f64>, k: usize) -> Vec<f64> {
    v.resize(k, f64::NAN);
    v
}

/// One record of `tone_levels`, `band_levels` and `drive_levels`: [`LEVEL_BLOCK`]
/// points, the unused ones NaN.
pub fn levels_results(points: &[LevelPoint], names: &Names) -> Vec<Described> {
    let k = LEVEL_BLOCK;
    let n = names.controls.len();
    let m = names.drives.len();
    let time = || {
        f64s(
            "time",
            vec![k],
            "s",
            padded(points.iter().map(|p| p.t_s).collect(), k),
        )
    };
    // [items, rows, k] from a value per point, item and row.
    let cube = |items: usize, rows: usize, g: &dyn Fn(&LevelPoint, usize, usize) -> f64| {
        let mut out = Vec::with_capacity(items * rows * k);
        for it in 0..items {
            for r in 0..rows {
                out.extend(padded(points.iter().map(|p| g(p, it, r)).collect(), k));
            }
        }
        out
    };
    let deg = |z: hertzforge_sdm::C64| z.arg().to_degrees();
    let nt = names.tones.len();
    let nb = names.bands.len();
    let cu = &names.control_unit;
    let du = &names.drive_unit;
    let mut out = Vec::new();
    if nt > 0 {
        let tone = |p: &LevelPoint, t: usize| p.status.tones.get(t).cloned();
        let ph = |p: &LevelPoint, t: usize| p.phasors.get(t).cloned();
        let mut d = time_described(
            "mimo_mixed_tones",
            json!({"responses": names.controls, "drives": names.drives}),
        );
        d.insert("tones".into(), json!(names.tones));
        d.insert(
            "description".into(),
            Value::from(
                "each tone through the run: its frequency, part level, whether the shared ceiling \
                 held it and the scale it applied; per control its tracked amplitude (peak), \
                 phase, target at the level and error against it (dB, NaN before a reading) and \
                 alarm; per drive the amplitude (peak) and phase it played",
            ),
        );
        out.push((
            "tone_levels".to_owned(),
            d,
            vec![
                time(),
                f64s(
                    "frequency",
                    vec![nt, 1, k],
                    "Hz",
                    cube(nt, 1, &|p, t, _| {
                        tone(p, t).map_or(f64::NAN, |x| x.frequency_hz)
                    }),
                ),
                f64s(
                    "part_level",
                    vec![nt, 1, k],
                    "",
                    cube(nt, 1, &|p, t, _| {
                        tone(p, t).map_or(f64::NAN, |x| x.part_level)
                    }),
                ),
                f64s(
                    "limiting",
                    vec![nt, 1, k],
                    "",
                    cube(nt, 1, &|p, t, _| {
                        tone(p, t).map_or(f64::NAN, |x| f64::from(u8::from(x.limiting)))
                    }),
                ),
                f64s(
                    "scale",
                    vec![nt, 1, k],
                    "",
                    cube(nt, 1, &|p, t, _| tone(p, t).map_or(f64::NAN, |x| x.scale)),
                ),
                f64s(
                    "amplitude",
                    vec![nt, n, k],
                    cu,
                    cube(nt, n, &|p, t, i| {
                        ph(p, t)
                            .and_then(|x| x.measured.get(i).map(|z| z.norm()))
                            .unwrap_or(f64::NAN)
                    }),
                ),
                f64s(
                    "phase",
                    vec![nt, n, k],
                    "deg",
                    cube(nt, n, &|p, t, i| {
                        ph(p, t)
                            .and_then(|x| x.measured.get(i).map(|z| deg(*z)))
                            .unwrap_or(f64::NAN)
                    }),
                ),
                f64s(
                    "target",
                    vec![nt, n, k],
                    cu,
                    cube(nt, n, &|p, t, i| {
                        tone(p, t)
                            .and_then(|x| x.target.get(i).copied())
                            .unwrap_or(f64::NAN)
                    }),
                ),
                f64s(
                    "error_db",
                    vec![nt, n, k],
                    "dB",
                    cube(nt, n, &|p, t, i| {
                        tone(p, t)
                            .and_then(|x| x.error_db.get(i).copied())
                            .unwrap_or(f64::NAN)
                    }),
                ),
                f64s(
                    "alarm",
                    vec![nt, n, k],
                    "",
                    cube(nt, n, &|p, t, i| {
                        tone(p, t)
                            .and_then(|x| x.alarm.get(i).map(|a| f64::from(u8::from(*a))))
                            .unwrap_or(f64::NAN)
                    }),
                ),
                f64s(
                    "drive_amplitude",
                    vec![nt, m, k],
                    du,
                    cube(nt, m, &|p, t, j| {
                        ph(p, t)
                            .and_then(|x| x.drive.get(j).map(|z| z.norm()))
                            .unwrap_or(f64::NAN)
                    }),
                ),
                f64s(
                    "drive_phase",
                    vec![nt, m, k],
                    "deg",
                    cube(nt, m, &|p, t, j| {
                        ph(p, t)
                            .and_then(|x| x.drive.get(j).map(|z| deg(*z)))
                            .unwrap_or(f64::NAN)
                    }),
                ),
            ],
        ));
    }
    if nb > 0 {
        let band = |p: &LevelPoint, b: usize| p.status.bands.get(b).cloned();
        let mut d = time_described("mimo_mixed_bands", json!({"responses": names.controls}));
        d.insert("bands".into(), json!(names.bands));
        d.insert(
            "description".into(),
            Value::from(
                "each band through the run: its center, its ramp, whether it is folded into the \
                 floor's reference; per control its added level against the stated (dB, NaN \
                 before a reading or for a folded band) and alarm",
            ),
        );
        out.push((
            "band_levels".to_owned(),
            d,
            vec![
                time(),
                f64s(
                    "center",
                    vec![nb, 1, k],
                    "Hz",
                    cube(nb, 1, &|p, b, _| {
                        band(p, b).map_or(f64::NAN, |x| x.center_hz)
                    }),
                ),
                f64s(
                    "part_level",
                    vec![nb, 1, k],
                    "",
                    cube(nb, 1, &|p, b, _| {
                        band(p, b).map_or(f64::NAN, |x| x.part_level)
                    }),
                ),
                f64s(
                    "folded",
                    vec![nb, 1, k],
                    "",
                    cube(nb, 1, &|p, b, _| {
                        band(p, b).map_or(f64::NAN, |x| f64::from(u8::from(x.folded)))
                    }),
                ),
                f64s(
                    "error_db",
                    vec![nb, n, k],
                    "dB",
                    cube(nb, n, &|p, b, i| {
                        band(p, b)
                            .and_then(|x| x.error_db.get(i).copied())
                            .unwrap_or(f64::NAN)
                    }),
                ),
                f64s(
                    "alarm",
                    vec![nb, n, k],
                    "",
                    cube(nb, n, &|p, b, i| {
                        band(p, b)
                            .and_then(|x| x.alarm.get(i).map(|a| f64::from(u8::from(*a))))
                            .unwrap_or(f64::NAN)
                    }),
                ),
            ],
        ));
    }
    let mut d = time_described(
        "mimo_mixed_drives",
        json!({"responses": names.controls, "drives": names.drives}),
    );
    d.insert(
        "description".into(),
        Value::from(
            "the run's level and each control's floor error (dB, the floor's rms against the \
             reference on clean lines); per drive the random part's rms, the largest magnitude \
             played, the peak held over the last second, the tones' summed amplitude, the \
             headroom to the drive's peak ceiling (dB) and the tones' common scale",
        ),
    );
    let drv = |p: &LevelPoint, j: usize| p.status.drives.get(j).copied();
    out.push((
        "drive_levels".to_owned(),
        d,
        vec![
            time(),
            f64s(
                "level_db",
                vec![k],
                "dB",
                padded(points.iter().map(|p| p.level_db).collect(), k),
            ),
            f64s(
                "floor_error_db",
                vec![n, k],
                "dB",
                cube(1, n, &|p, _, i| {
                    p.status.floor_error_db.get(i).copied().unwrap_or(f64::NAN)
                }),
            ),
            f64s(
                "random_rms",
                vec![m, k],
                du,
                cube(1, m, &|p, _, j| {
                    drv(p, j).map_or(f64::NAN, |d| d.random_rms_v)
                }),
            ),
            f64s(
                "peak",
                vec![m, k],
                du,
                cube(1, m, &|p, _, j| drv(p, j).map_or(f64::NAN, |d| d.peak_v)),
            ),
            f64s(
                "held_peak",
                vec![m, k],
                du,
                cube(1, m, &|p, _, j| {
                    p.held_peak_v.get(j).copied().unwrap_or(f64::NAN)
                }),
            ),
            f64s(
                "tone_sum",
                vec![m, k],
                du,
                cube(1, m, &|p, _, j| {
                    drv(p, j).map_or(f64::NAN, |d| d.tone_sum_v)
                }),
            ),
            f64s(
                "headroom_db",
                vec![m, k],
                "dB",
                cube(1, m, &|p, _, j| {
                    let h = p.held_peak_v.get(j).copied().unwrap_or(0.0);
                    let c = p.peak_ceiling_v.get(j).copied().unwrap_or(f64::NAN);
                    if h > 0.0 {
                        20.0 * (c / h).log10()
                    } else {
                        f64::NAN
                    }
                }),
            ),
            f64s(
                "scale",
                vec![k],
                "",
                padded(
                    points
                        .iter()
                        .map(|p| super::view::tone_scale(&p.status))
                        .collect(),
                    k,
                ),
            ),
        ],
    ));
    out
}

/// One record of `control_trace`: [`TRACE_BLOCK`] law calls, the unused ones NaN.
pub fn trace_result(calls: &[LawRecord], names: &Names) -> Described {
    let k = TRACE_BLOCK;
    let col = |g: &dyn Fn(&LawRecord) -> f64| padded(calls.iter().map(g).collect(), k);
    let mut d = time_described("mimo_mixed_control", json!({"drives": names.drives}));
    d.insert(
        "description".into(),
        Value::from(
            "every call of the two laws, in the order taken (the controller's law-call record, \
             format hertzforge.mimo_mixed.law_ticks.v1): station time, the law (0 the floor's, \
             once a loop; 1 a tone's, once a tick), the tone's index among the tones, the drive \
             in force before the call and the drive the controller took from it (the floor's \
             in-band rms summed over the drives, full level; a tone's drive vector norm, peak, \
             at the level), the call's time, a miss (0 a usable call) and its flags (1 a cap \
             bound, 2 the engage or the seed); NaN where a record was not filled",
        ),
    );
    d.insert("roles".into(), json!(["floor", "tone"]));
    d.insert("tones".into(), json!(names.tones));
    d.insert("format".into(), Value::from(LAW_TICKS_FORMAT));
    d.insert(
        "fields".into(),
        json!(LAW_TICK_FIELDS.iter().map(|(n, _)| n).collect::<Vec<_>>()),
    );
    let du = names.drive_unit.as_str();
    (
        "control_trace".into(),
        d,
        vec![
            f64s("time", vec![k], "s", col(&|r| r.t_s)),
            f64s("role", vec![k], "", col(&|r| f64::from(r.role as u8))),
            f64s("tone", vec![k], "", col(&|r| f64::from(r.tone))),
            f64s("drive_in", vec![k], du, col(&|r| r.drive_in)),
            f64s("drive_out", vec![k], du, col(&|r| r.drive_out)),
            f64s("call_us", vec![k], "us", col(&|r| f64::from(r.took_us))),
            f64s("miss", vec![k], "", col(&|r| f64::from(r.miss))),
            f64s("flags", vec![k], "", col(&|r| f64::from(r.flags))),
        ],
    )
}

/// The drive rows the run plays, in the table's order.
pub fn played_drives(table: &ChannelTable) -> Vec<&Channel> {
    rrec::drive_rows(table)
}
