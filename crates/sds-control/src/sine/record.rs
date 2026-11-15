//! What a Sine run puts in its record (`docs/run-folder.md`, "What a Sine Run
//! Saves"): the manifest, the table and the setup at arm, the two streams, and the
//! results (the profile and its bands on the run's band, the limit lines, the
//! identification's transfer function and coherence, each input's tracked level and
//! phase as the run went, and the control law's trace, call by call).
//!
//! The writing is the record's ([`crate::record`]); this module only says what is
//! written, in the reader's form (`hertzforge.io.run_folder`). Values are in the
//! channels' own units, as the table names them: the analysis side converts.

use super::config::{direction_name, mode_name, WireSine, WireTest};
use crate::protocol::num;
use crate::random::record as rrec;
use crate::record::{tap::TapLink, ArrayValues, Begin, ResultArray, StreamSpec};
use crate::station::{source_of, StationPlan};
use crate::table::unit_name;
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal::{Caps, DiscoveredDevice};
use hertzforge_module_abi::StationClass;
use hertzforge_sine::service::law_records::LawRecord;
use hertzforge_sine::{
    DwellFor, Quantity, RungFor, SineProfile, SineTestConfig, StepSpacing, SweepRate,
};
use serde_json::{json, Map, Value};
use std::path::PathBuf;

/// Points of one record of `channel_levels`: the record repeats every this many
/// points, the last one filled out with NaN.
pub const LEVEL_BLOCK: usize = 64;
/// Calls of one record of `control_trace`, the last one filled out with NaN.
pub const TRACE_BLOCK: usize = 64;
/// Points each profile segment is resolved at between its breakpoints.
pub const SEGMENT_POINTS: usize = 8;

/// The drive rows the run plays, in the order the station takes them: the first
/// enabled drive row, and the second when the reference output is on.
pub fn played_drives(table: &ChannelTable, outputs: usize) -> Vec<&Channel> {
    rrec::drive_rows(table).into_iter().take(outputs).collect()
}

/// The facts of an armed run its record begins with.
pub struct ArmFacts<'a> {
    pub run_id: &'a str,
    pub run_uid: &'a str,
    pub cfg: &'a WireSine,
    /// The controller's configuration as it armed.
    pub controller: &'a SineTestConfig,
    pub table: &'a ChannelTable,
    pub revision: u64,
    pub plan: &'a StationPlan,
    pub class: StationClass,
    pub provenance: &'a hertzforge_station::ProvenanceBasis,
    pub devices: &'a [DiscoveredDevice],
    pub caps: &'a Caps,
    /// The rate the station programmed and the control tick in frames.
    pub fs_hz: f64,
    pub tick_frames: usize,
    pub derived: Value,
    pub feasibility: Option<&'a (&'static str, String)>,
    pub law: Value,
    pub armed_unix: f64,
    pub utc_offset_s: Option<i64>,
    /// `None` when the streams are recorded; otherwise why not.
    pub streams_off: Option<String>,
    pub outputs: usize,
}

fn quantity_name(q: Quantity) -> &'static str {
    match q {
        Quantity::AccelGPk => "accel_g_pk",
        Quantity::VelocityIpsPk => "velocity_ips_pk",
        Quantity::DisplacementInPp => "displacement_in_pp",
    }
}

fn pair(p: (f64, f64)) -> Value {
    json!([num(p.0), num(p.1)])
}

/// The profile as resolved: every breakpoint with the quantity and value it was
/// given, the acceleration the controller makes of it there, and the alarm and
/// abort bands that hold from it to the next.
pub fn profile_json(p: &SineProfile) -> Value {
    let points: Vec<Value> = p
        .rows
        .iter()
        .map(|r| {
            json!({"freq_hz": num(r.freq_hz), "quantity": quantity_name(r.quantity),
                   "value": num(r.value),
                   "accel_g_pk": p.target_accel_g_pk(r.freq_hz).map(num),
                   "alarm_db": pair(r.alarm_db.unwrap_or(p.global_alarm_db)),
                   "abort_db": pair(r.abort_db.unwrap_or(p.global_abort_db))})
        })
        .collect();
    json!({"breakpoints": points, "alarm_db": pair(p.global_alarm_db),
           "abort_db": pair(p.global_abort_db), "unit": "g", "label": p.label})
}

fn rate_json(rate: &SweepRate) -> Value {
    match *rate {
        SweepRate::LogOctPerMin(x) => json!({"law": "log", "oct_per_min": num(x)}),
        SweepRate::LinearHzPerSec(x) => json!({"law": "linear", "hz_per_s": num(x)}),
    }
}

fn dwell_json(d: &DwellFor) -> Value {
    match *d {
        DwellFor::Seconds(s) => json!({"seconds": num(s)}),
        DwellFor::Cycles(c) => json!({"cycles": c}),
    }
}

fn rung_json(r: RungFor) -> Value {
    match r {
        RungFor::Seconds(s) => json!({"seconds": num(s)}),
        RungFor::Sweeps(n) => json!({"sweeps": n}),
    }
}

/// The test as resolved: its kind, band, sweep law, rate, direction, count, dwell
/// and step settings.
fn test_json(t: &WireTest) -> Value {
    match t {
        WireTest::Sweep {
            band,
            rate,
            start_hz,
            direction,
            sweeps,
            duration_s,
        } => json!({"kind": "sweep", "band_hz": pair(*band), "rate": rate_json(rate),
                    "start_hz": start_hz.map(num), "direction": direction_name(*direction),
                    "sweeps": sweeps, "duration_s": duration_s.map(num)}),
        WireTest::Dwell { freq_hz, end } => {
            json!({"kind": "dwell", "freq_hz": num(*freq_hz), "dwell": dwell_json(end)})
        }
        WireTest::Step {
            band,
            steps_per_octave,
            spacing,
            dwell,
        } => json!({"kind": "step", "band_hz": pair(*band),
                    "steps_per_octave": num(*steps_per_octave),
                    "spacing": match spacing { StepSpacing::Log => "log", StepSpacing::Linear => "linear" },
                    "dwell": dwell.as_ref().map(dwell_json)}),
        WireTest::Rstd {
            band,
            level_db,
            rate,
            tracking_bw_hz,
            settle_k,
            dwell_mode,
            dwell,
        } => json!({"kind": "rstd", "band_hz": pair(*band), "level_db": num(*level_db),
                    "rate": rate_json(rate), "tracking_bw_hz": num(*tracking_bw_hz),
                    "settle_k": num(*settle_k),
                    "dwell_mode": format!("{dwell_mode:?}").to_lowercase(),
                    "dwell": dwell_json(dwell)}),
    }
}

fn limits_json(cfg: &WireSine, table: &ChannelTable) -> (Value, Value) {
    let limits: Vec<Value> = cfg
        .limit_channels
        .iter()
        .map(|l| {
            json!({"channel": l.channel, "unit": unit_of(table, l.channel),
                   "shared_name": l.shared_name, "profile": profile_json(&l.profile)})
        })
        .collect();
    let channels: Vec<Value> = table
        .channels
        .iter()
        .filter(|c| {
            c.enabled
                && (c.abort.accel_g.is_some()
                    || c.abort.velocity_ips.is_some()
                    || c.abort.displacement_mil.is_some())
        })
        .map(|c| {
            json!({"channel": c.index, "accel_g": c.abort.accel_g.map(num),
                   "velocity_ips": c.abort.velocity_ips.map(num),
                   "displacement_mil": c.abort.displacement_mil.map(num)})
        })
        .collect();
    let aborts = json!({
        "envelope": {"alarm_db": pair(cfg.profile.global_alarm_db),
                     "abort_db": pair(cfg.profile.global_abort_db)},
        "drive": {"ceiling_vpk": num(cfg.drive_vpk_ceiling), "abort_vpk": num(cfg.drive_vpk_abort)},
        "divergence_s": num(cfg.divergence_s),
        "fault_debounce_ticks": cfg.fault_debounce_ticks,
        "kinematic_monitor": cfg.kinematic_monitor,
        "channels": channels,
        "notch": {"drive_floor": num(cfg.notch.drive_floor),
                  "smoothing_ticks": num(cfg.notch.smoothing_ticks),
                  "release_ramp_db_per_s": num(cfg.notch.release_ramp_db_per_s)},
    });
    (Value::Array(limits), aborts)
}

fn unit_of(table: &ChannelTable, ch: u32) -> String {
    table
        .channels
        .iter()
        .find(|c| c.index == ch)
        .map_or_else(|| "g".to_owned(), |c| unit_name(&c.units).to_string())
}

fn setup_json(f: &ArmFacts<'_>) -> Value {
    let cfg = f.cfg;
    let (limits, aborts) = limits_json(cfg, f.table);
    let mut derived = f.derived.clone();
    if let Value::Object(m) = &mut derived {
        m.insert(
            "grid".into(),
            json!({"fs_hz": num(f.fs_hz), "tick_frames": f.tick_frames,
                   "tick_s": num(f.tick_frames as f64 / f.fs_hz), "actual": true}),
        );
    }
    json!({
        "config": cfg.raw,
        "resolved": {
            "profile": profile_json(&cfg.profile),
            "test": test_json(&cfg.test),
            "schedule": {"mode": mode_name(cfg.mode),
                         "levels_db": cfg.levels_db.iter().map(|l| num(*l)).collect::<Vec<_>>(),
                         "start_db": num(cfg.start_level()),
                         "per_level": rung_json(cfg.per_level),
                         "full_level": rung_json(cfg.full_level)},
            "tolerances": {"alarm_db": pair(cfg.profile.global_alarm_db),
                           "abort_db": pair(cfg.profile.global_abort_db)},
            "limits": limits,
            "aborts": aborts,
            "identification": {"level_db": num(cfg.ident_level_db),
                               "gate_coherence": num(cfg.gate_coherence),
                               "trust_coherence": num(cfg.trust_coherence),
                               "trust_h_floor_frac": num(cfg.trust_h_floor_frac),
                               "review": cfg.review},
            "control": {
                "composite": format!("{:?}", cfg.composite).to_lowercase(),
                "weights": cfg.weights.iter().map(|(ch, w)| json!({"channel": ch, "weight": num(*w)})).collect::<Vec<_>>(),
                "tracking": match cfg.tracking {
                    hertzforge_sine::BandwidthMode::FixedHz(hz) => json!({"hz": num(hz)}),
                    hertzforge_sine::BandwidthMode::Proportional { pct, max_hz } => {
                        json!({"pct": num(pct), "max_hz": num(max_hz)})
                    }
                },
                "law_budget": num(cfg.law_budget),
                "law_misses": cfg.law_misses,
                "max_step_db": num(cfg.max_step_db),
                "max_rise_db_per_s": num(cfg.max_rise_db_per_s),
                "cola": serde_json::to_value(cfg.cola).unwrap_or(Value::Null),
            },
            "control_channels": WireSine::control_channels(f.table),
            "acquisition": {"requested_fs_hz": num(cfg.fs_hz()), "fs_hz": num(f.fs_hz),
                            "tick_ms": num(cfg.tick_ms), "tick_frames": f.tick_frames},
            "controller": serde_json::to_value(f.controller).unwrap_or(Value::Null),
        },
        "derived": derived,
        "feasibility": {
            "inputs": cfg.raw.get("feasibility").cloned().unwrap_or(Value::Null),
            "verdict": f.feasibility.map(|(v, _)| *v),
            "message": f.feasibility.map(|(_, m)| m.clone()),
        },
    })
}

/// The record a Sine run begins with, at arm.
pub fn begin(f: &ArmFacts<'_>, root: PathBuf, links: (TapLink, TapLink), log: Vec<Value>) -> Begin {
    let cfg = f.cfg;
    let mut manifest = Map::new();
    manifest.insert("test".into(), Value::from("sine"));
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
        json!({"requested_fs_hz": num(cfg.fs_hz()), "fs_hz": num(f.fs_hz),
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
        let drives = played_drives(f.table, f.outputs);
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
    let name = format!("{}_sine_{}", rrec::utc_stamp(f.armed_unix), f.run_id);
    Begin {
        root,
        name,
        manifest,
        table: rrec::table_json(f.table, f.revision),
        setup: setup_json(f),
        law: Some((f.law.clone(), None)),
        log,
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

/// The frequencies a profile is resolved at across `band`: the band's edges, the
/// breakpoints inside it, and [`SEGMENT_POINTS`] log-spaced points inside each
/// segment between them. A band of one frequency (a dwell) is that frequency.
pub fn resolve_grid(profile: &SineProfile, band: (f64, f64)) -> Vec<f64> {
    let (lo, hi) = (band.0.min(band.1), band.0.max(band.1));
    if hi <= lo * (1.0 + 1e-12) {
        return vec![lo];
    }
    let mut knots = vec![lo];
    knots.extend(
        profile
            .rows
            .iter()
            .map(|r| r.freq_hz)
            .filter(|f| *f > lo && *f < hi),
    );
    knots.push(hi);
    let mut out = Vec::new();
    for w in knots.windows(2) {
        let (a, b) = (w[0], w[1]);
        for k in 0..SEGMENT_POINTS {
            out.push(a * (b / a).powf(k as f64 / SEGMENT_POINTS as f64));
        }
    }
    out.push(hi);
    out
}

/// The profile and its alarm and abort bands at 0 dB on the run's band, in the
/// control unit, peak.
pub fn reference_result(cfg: &WireSine, table: &ChannelTable, unit: &str) -> rrec::Described {
    let freqs = resolve_grid(&cfg.profile, cfg.test.band());
    let p = &cfg.profile;
    let on = |g: &dyn Fn(f64) -> Option<f64>| -> Vec<f64> {
        freqs.iter().map(|f| g(*f).unwrap_or(f64::NAN)).collect()
    };
    let n = freqs.len();
    let mut d = described(
        "envelope",
        json!({"kind": "frequency", "unit": "Hz", "array": "frequency"}),
        json!({"controls": WireSine::control_channels(table)}),
    );
    d.insert(
        "description".into(),
        Value::from(
            "the profile and its alarm and abort bands at 0 dB, peak, on the run's band: \
             the breakpoints and points between them",
        ),
    );
    (
        "reference".into(),
        d,
        vec![
            f64s("frequency", vec![n], "Hz", freqs.clone()),
            f64s("reference", vec![n], unit, on(&|f| p.target_accel_g_pk(f))),
            f64s(
                "alarm_low",
                vec![n],
                unit,
                on(&|f| p.alarm_bounds(f).map(|b| b.0)),
            ),
            f64s(
                "alarm_high",
                vec![n],
                unit,
                on(&|f| p.alarm_bounds(f).map(|b| b.1)),
            ),
            f64s(
                "abort_low",
                vec![n],
                unit,
                on(&|f| p.abort_bounds(f).map(|b| b.0)),
            ),
            f64s(
                "abort_high",
                vec![n],
                unit,
                on(&|f| p.abort_bounds(f).map(|b| b.1)),
            ),
        ],
    )
}

/// One limit channel's lines on the run's band: the level its profile states (the
/// level the notch holds it to) and its abort bound.
pub fn limit_results(cfg: &WireSine, table: &ChannelTable) -> Vec<rrec::Described> {
    cfg.limit_channels
        .iter()
        .map(|l| {
            let freqs = resolve_grid(&l.profile, cfg.test.band());
            let n = freqs.len();
            let unit = unit_of(table, l.channel);
            let mut d = described(
                "limit",
                json!({"kind": "frequency", "unit": "Hz", "array": "frequency"}),
                json!({"responses": [l.channel]}),
            );
            d.insert(
                "description".into(),
                Value::from(
                    "a limit channel's lines at 0 dB, peak: the level the notch holds it to \
                     and its abort bound",
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

/// The identification's transfer function (drive to control response) and its
/// coherence on the band the gate judges (a dwell's an octave either side).
pub fn identification_results(
    freqs: &[f64],
    h: &[(f64, f64)],
    coherence: &[f64],
    band: (f64, f64),
    lag_frames: usize,
    table: &ChannelTable,
) -> Vec<rrec::Described> {
    let (lo, hi) = if (band.1 - band.0).abs() < 1e-6 {
        (band.0 * 0.5, band.0 * 2.0)
    } else {
        band
    };
    let n_all = freqs.len().min(h.len()).min(coherence.len());
    let idx: Vec<usize> = (0..n_all)
        .filter(|&i| freqs[i] >= lo && freqs[i] <= hi)
        .collect();
    let (Some(&first), Some(&last)) = (idx.first(), idx.last()) else {
        return Vec::new();
    };
    let controls: Vec<&Channel> = table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role == ChannelRole::Control)
        .collect();
    let Some(drive) = rrec::drive_rows(table).into_iter().next() else {
        return Vec::new();
    };
    let Some(control) = controls.first() else {
        return Vec::new();
    };
    let n = last + 1 - first;
    let df = if n_all > 1 {
        freqs[1] - freqs[0]
    } else {
        freqs[0]
    };
    let single = controls.len() == 1;
    let indexes: Vec<u32> = controls.iter().map(|c| c.index).collect();
    let channels = if single {
        json!({"responses": indexes, "references": [drive.index]})
    } else {
        json!({"controls": indexes, "references": [drive.index]})
    };
    let axis = json!({"kind": "frequency", "unit": "Hz", "start": freqs[first], "step": df});
    let words = format!(
        "identification: drive to control response, the response taken {lag_frames} frames \
         later (the lag the identification measured)"
    );
    let mut frf = described(
        if single { "frf" } else { "composite_frf" },
        axis.clone(),
        channels.clone(),
    );
    frf.insert("convention".into(), Value::from("file"));
    frf.insert("description".into(), Value::from(words.clone()));
    frf.insert("lag_frames".into(), Value::from(lag_frames));
    let mut coh = described(
        if single {
            "coherence"
        } else {
            "composite_coherence"
        },
        axis,
        channels,
    );
    coh.insert(
        "description".into(),
        Value::from(format!("{words}: ordinary coherence")),
    );
    let unit = format!("{}/{}", unit_name(&control.units), unit_name(&drive.units));
    let shape = vec![1, 1, n];
    vec![
        (
            "identification_frf".into(),
            frf,
            vec![ResultArray {
                name: "values".into(),
                shape: shape.clone(),
                unit,
                values: ArrayValues::C128(rrec::frf_to_file(&h[first..=last])),
            }],
        ),
        (
            "identification_coherence".into(),
            coh,
            vec![f64s("values", shape, "", coherence[first..=last].to_vec())],
        ),
    ]
}

/// One point of the inputs' levels as the run went.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelPoint {
    pub t_s: f64,
    pub freq_hz: f64,
    pub level_db: f64,
    /// +1 rising, -1 falling, 0 held.
    pub direction: f64,
    /// The one-way sweep the point belongs to, from 0.
    pub pass: f64,
    pub measured: f64,
    pub target: f64,
    pub drive: f64,
    /// Per input, in table order: tracked amplitude, peak, and phase, degrees.
    pub tracked: Vec<f64>,
    pub phase_deg: Vec<f64>,
}

/// Points that fill records of fixed length: a record is handed out when it is
/// full, and the last, part full, at the end.
pub struct Blocks<T> {
    size: usize,
    held: Vec<T>,
    pub kept: u64,
}

impl<T> Blocks<T> {
    pub fn new(size: usize) -> Self {
        Blocks {
            size: size.max(1),
            held: Vec::new(),
            kept: 0,
        }
    }

    pub fn push(&mut self, t: T) -> Option<Vec<T>> {
        self.held.push(t);
        (self.held.len() >= self.size).then(|| std::mem::take(&mut self.held))
    }

    pub fn rest(&mut self) -> Option<Vec<T>> {
        (!self.held.is_empty()).then(|| std::mem::take(&mut self.held))
    }

    pub fn size(&self) -> usize {
        self.size
    }
}

fn padded(mut v: Vec<f64>, n: usize) -> Vec<f64> {
    v.resize(n, f64::NAN);
    v
}

/// One record of `channel_levels`: [`LEVEL_BLOCK`] points, the unused ones NaN.
pub fn levels_result(
    points: &[LevelPoint],
    inputs: &[(u32, String)],
    control_unit: &str,
    drive_unit: &str,
) -> rrec::Described {
    let n = LEVEL_BLOCK;
    let col = |g: &dyn Fn(&LevelPoint) -> f64| padded(points.iter().map(g).collect(), n);
    let per = |g: &dyn Fn(&LevelPoint, usize) -> f64| -> Vec<f64> {
        (0..inputs.len())
            .flat_map(|k| padded(points.iter().map(|p| g(p, k)).collect(), n))
            .collect()
    };
    let common = match inputs.first() {
        Some((_, u)) if inputs.iter().all(|(_, x)| x == u) => u.clone(),
        _ => String::new(),
    };
    let mut d = described(
        "sine_levels",
        json!({"kind": "time", "unit": "s", "array": "time"}),
        json!({"responses": inputs.iter().map(|(i, _)| i).collect::<Vec<_>>()}),
    );
    d.insert(
        "description".into(),
        Value::from(
            "each input's tracked level, peak, and phase against the drive, with the tone's \
             frequency, the scheduled level, the sweep's direction and pass, and the control \
             composite's target, measured value and drive, as the run went; a point is NaN \
             where a record was not filled",
        ),
    );
    d.insert(
        "records".into(),
        json!({"index": "record", "points": "the points the record holds before its NaN fill"}),
    );
    d.insert(
        "units".into(),
        json!(inputs
            .iter()
            .map(|(i, u)| json!({"channel": i, "unit": u}))
            .collect::<Vec<_>>()),
    );
    let k = inputs.len();
    (
        "channel_levels".into(),
        d,
        vec![
            f64s("time", vec![n], "s", col(&|p| p.t_s)),
            f64s("frequency", vec![n], "Hz", col(&|p| p.freq_hz)),
            f64s("level_db", vec![n], "dB", col(&|p| p.level_db)),
            f64s("direction", vec![n], "", col(&|p| p.direction)),
            f64s("pass", vec![n], "", col(&|p| p.pass)),
            f64s("target", vec![n], control_unit, col(&|p| p.target)),
            f64s("measured", vec![n], control_unit, col(&|p| p.measured)),
            f64s("drive", vec![n], drive_unit, col(&|p| p.drive)),
            f64s(
                "values",
                vec![k, n],
                &common,
                per(&|p, c| p.tracked.get(c).copied().unwrap_or(f64::NAN)),
            ),
            f64s(
                "phase",
                vec![k, n],
                "deg",
                per(&|p, c| p.phase_deg.get(c).copied().unwrap_or(f64::NAN)),
            ),
        ],
    )
}

/// One record of `control_trace`: [`TRACE_BLOCK`] control-law calls, the unused
/// ones NaN, `values` the law's declared values by index.
pub fn trace_result(
    calls: &[LawRecord],
    tick_s: f64,
    diagnostics: &[String],
    control_unit: &str,
    drive_unit: &str,
) -> rrec::Described {
    let n = TRACE_BLOCK;
    let col = |g: &dyn Fn(&LawRecord) -> f64| padded(calls.iter().map(g).collect(), n);
    let k = diagnostics.len();
    let values: Vec<f64> = (0..k)
        .flat_map(|i| padded(calls.iter().map(|r| r.values[i]).collect(), n))
        .collect();
    let mut d = described(
        "sine_control",
        json!({"kind": "time", "unit": "s", "array": "time"}),
        json!({}),
    );
    d.insert(
        "description".into(),
        Value::from(
            "every call of the control law: the tone's frequency, the control composite's \
             target and measured value, the drive in force before the call, the law's output \
             and the drive the core took from it (before the notch and the drive ceiling); \
             time from engage; a call is NaN where a record was not filled",
        ),
    );
    d.insert("diagnostics".into(), json!(diagnostics));
    d.insert("hold".into(), json!(["free", "notch", "probe"]));
    d.insert(
        "miss".into(),
        json!(hertzforge_sine::service::law_records::MISS_NAMES),
    );
    let mut arrays = vec![
        f64s("time", vec![n], "s", col(&|r| r.tick as f64 * tick_s)),
        f64s("tick", vec![n], "", col(&|r| r.tick as f64)),
        f64s("frequency", vec![n], "Hz", col(&|r| r.freq_hz)),
        f64s("target", vec![n], control_unit, col(&|r| r.target)),
        f64s("measured", vec![n], control_unit, col(&|r| r.measured)),
        f64s(
            "drive_in_force",
            vec![n],
            drive_unit,
            col(&|r| r.drive_in_force),
        ),
        f64s("output", vec![n], drive_unit, col(&|r| r.output)),
        f64s("applied", vec![n], drive_unit, col(&|r| r.applied)),
        f64s("prediction_db", vec![n], "dB", col(&|r| r.prediction_db)),
        f64s("hold", vec![n], "", col(&|r| f64::from(r.hold))),
        f64s("miss", vec![n], "", col(&|r| f64::from(r.miss))),
        f64s(
            "capped",
            vec![n],
            "",
            col(&|r| f64::from(u8::from(r.capped))),
        ),
        f64s("call_us", vec![n], "us", col(&|r| f64::from(r.call_us))),
    ];
    if k > 0 {
        arrays.push(f64s("values", vec![k, n], "", values));
    }
    ("control_trace".into(), d, arrays)
}
