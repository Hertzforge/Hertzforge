//! What a Modal Test run puts in its run folder (`docs/run-folder.md`, "What a Modal
//! Test Run Saves"): the manifest, the channel table, the setup at start (the
//! configuration as accepted and the configure report: the rows of every measurement,
//! the windows, the trigger, the drives and their protections), the log, the streams
//! (every enabled input as read; in shaker mode every drive as the device accepted it),
//! a line in `modal_frames` for every hit or frame taken and every change of its
//! status, the estimates after every finished measurement (`modal_frf`,
//! `modal_coherence`, `modal_reference_spectra`), `modal_outcome` at the end, and the
//! end.
//!
//! The writing is the shared record's ([`crate::record`]), on its own thread; this
//! module only says what is written.

use super::run::{state_json, ModalRun, ModalView};
use crate::protocol::num;
use crate::random::record::{
    build_value, drive_rows, input_rows, sim_value, start_json, station_member,
    stream_channel_value, table_json,
};
use crate::record::{tap::TapLink, ArrayValues, Begin, ResultArray, StreamSpec};
use crate::station::{source_of, StationPlan};
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::{Caps, DiscoveredDevice};
use hertzforge_modal_test::average::Estimates;
use hertzforge_modal_test::hits::Decay;
use hertzforge_modal_test::plan::Measurement;
use hertzforge_modal_test::session::Session;
use hertzforge_modal_test::setup::Mode;
use hertzforge_module_abi::StationClass;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

/// A result: its name, its description and its arrays.
pub type Described = (String, Map<String, Value>, Vec<ResultArray>);
/// A record of a repeating result: its name, description, arrays and line.
pub type Line = (String, Map<String, Value>, Vec<ResultArray>, Value);

/// The facts a run's record begins with, before the station is opened.
pub struct StartFacts<'a> {
    pub run: &'a ModalRun,
    pub run_uid: &'a str,
    pub table: &'a ChannelTable,
    /// The table the engine runs: in impact mode without its drive rows.
    pub engine_table: &'a ChannelTable,
    pub revision: u64,
    pub plan: &'a StationPlan,
    pub class: StationClass,
    pub provenance: &'a hertzforge_station::ProvenanceBasis,
    pub devices: &'a [DiscoveredDevice],
    pub caps: &'a Caps,
    pub started_unix: f64,
    pub utc_offset_s: Option<i64>,
    pub streams_off: Option<String>,
    /// The session's windows, as `setup.json` states them.
    pub windows: Value,
}

fn manifest(f: &StartFacts<'_>) -> Map<String, Value> {
    let w = &f.run.wire;
    let fs = w.setup.fs_hz;
    let mut m = Map::new();
    m.insert("test".into(), Value::from("modal_test"));
    m.insert("run_uid".into(), Value::from(f.run_uid));
    m.insert("run_id".into(), Value::from(f.run.id.as_str()));
    m.insert("title".into(), Value::from(w.title.as_str()));
    m.insert("operator".into(), Value::from(w.operator.as_str()));
    m.insert("article".into(), Value::from(w.article.as_str()));
    m.insert("notes".into(), Value::from(w.notes.as_str()));
    if let Some((document, clause)) = &w.specification {
        m.insert(
            "specification".into(),
            json!({"document": document, "clause": clause}),
        );
    }
    m.insert(
        "metadata".into(),
        Value::Array(
            w.metadata
                .iter()
                .map(|(key, label, value)| json!({"key": key, "label": label, "value": value}))
                .collect(),
        ),
    );
    m.insert("source".into(), Value::from(source_of(f.class)));
    if let Some(sim) = sim_value(f.plan, f.class) {
        m.insert("sim".into(), sim);
    }
    m.insert(
        "station".into(),
        station_member(
            matches!(f.plan, StationPlan::Sim { .. }),
            f.class,
            f.provenance,
            f.devices,
            f.caps,
        ),
    );
    m.insert(
        "grid".into(),
        json!({
            "requested_fs_hz": num(fs),
            "fs_hz": num(fs),
            "block_frames": hertzforge_hal::default_block_frames(fs),
            "analysis_block": w.setup.block,
            "df_hz": num(w.setup.df_hz()),
            "rule": "the setup's own rate: a station that programs another is refused",
        }),
    );
    m.insert("build".into(), build_value(f.provenance.core_build));
    m.insert(
        "armed".into(),
        json!({"unix": f.started_unix, "utc_offset_s": f.utc_offset_s}),
    );
    m.insert(
        "start".into(),
        start_json(f.started_unix, f.utc_offset_s, 0),
    );
    m.insert(
        "recording".into(),
        match &f.streams_off {
            None => json!({"streams": "on"}),
            Some(why) => json!({"streams": "off", "why": why}),
        },
    );
    m
}

/// How the simulator makes the structure's signals, stated in the record of every
/// simulated run.
pub fn sim_structure(mode: Mode) -> Value {
    use hertzforge_modal_test::structure as st;
    json!({
        "modes": st::MODES.iter().map(|(f, z)| json!({"f_hz": num(*f), "damping": num(*z)})).collect::<Vec<_>>(),
        "scale_g_per_lbf": num(st::SCALE_G_PER_LBF),
        "shape": "sin(m pi x), x = ((node - 1) mod 10 + 0.5) / 10; a direction ending in - flips the sign",
        "force": match mode {
            Mode::Impact => json!({"how": "a half-sine hammer pulse on the reference row",
                                   "pulse_s": num(st::PULSE_S), "peak_lbf": num(st::PULSE_PEAK_LBF)}),
            Mode::Shaker => json!({"how": "each shaker's force is its drive as written, times the shaker's \
                                           constant, one engine block later; each response is the sum over \
                                           the shakers of the structure's impulse response convolved with \
                                           that force",
                                   "lbf_per_v": num(st::SHAKER_LBF_PER_V)}),
        },
        "impulse_response_s": num(st::FIR_S),
    })
}

/// The time from an abort to the drive at zero the service works to: the station's own
/// characterized bound (the arm gate refuses a driving hardware station without one), or,
/// with none, what bounds it in the loop.
pub fn abort_latency(caps: &Caps) -> Value {
    let ticks = hertzforge_modal_test::host::TICKS_PER_BATCH;
    match caps.abort_latency_bound {
        Some(b) => json!({"bound_s": num(b.as_secs_f64()),
                          "from": "the station's characterized abort latency bound, which the arm gate requires of a driving hardware station"}),
        None => json!({"bound_s": null,
                       "from": format!("no characterized bound on this station: the drive's step writes zero from the next engine block after the abort, and the loop issues the emergency stop once its current batch of {ticks} blocks is done")}),
    }
}

fn setup(f: &StartFacts<'_>) -> Value {
    let w = &f.run.wire;
    let s = &w.setup;
    let mut resolved = json!({
        "control": match s.mode {
            Mode::Impact => "no drive: impact mode writes no drive sample",
            Mode::Shaker => "open loop, no control law: each shaker plays its source at its level, \
                             and nothing is corrected from the response",
        },
        "inputs": input_rows(f.engine_table).iter().map(|c| c.index).collect::<Vec<_>>(),
        "drives": drive_rows(f.engine_table).iter().map(|c| c.index).collect::<Vec<_>>(),
        "windows": f.windows,
        "capture": {"run_name": w.run_name},
    });
    if s.mode == Mode::Shaker {
        resolved["stop"] = json!({
            "depth_db": num(hertzforge_modal_test::setup::STOP_RAMP_DEPTH_DB),
            "ramp_s": num(hertzforge_modal_test::setup::STOP_RAMP_S),
            "then": "zero",
        });
        resolved["aborts"] = json!({
            "drive": {"ceiling_v": num(s.shaker.ceiling_v), "judged": "every sample, after the ramp"},
            "underrun": "the source running empty takes the drive to zero at once",
            "input_overload": "an overload on a row with abort limits takes the drive to zero at once",
            "latency": abort_latency(f.caps),
        });
    }
    if !f.class.is_hardware() {
        resolved["structure"] = sim_structure(s.mode);
    }
    json!({
        "config": w.raw,
        "resolved": resolved,
        "check": f.run.report,
    })
}

fn stream(kind: &str, fs: f64, channels: Vec<Value>, link: TapLink) -> StreamSpec {
    let mut d = Map::new();
    d.insert("kind".into(), Value::from(kind));
    d.insert("fs_hz".into(), num(fs));
    d.insert("channels".into(), Value::Array(channels));
    d.insert("first_station_frame".into(), Value::from(0));
    StreamSpec {
        name: kind.into(),
        description: d,
        link,
    }
}

/// The record a Modal Test run begins with, before the station is opened. Impact mode
/// has no drive rows, so it records the inputs only.
pub fn begin(f: &StartFacts<'_>, root: PathBuf, links: (TapLink, TapLink)) -> Begin {
    let fs = f.run.wire.setup.fs_hz;
    let mut streams = Vec::new();
    if f.streams_off.is_none() {
        let inputs = input_rows(f.engine_table)
            .into_iter()
            .map(stream_channel_value)
            .collect();
        streams.push(stream("inputs", fs, inputs, links.0));
        if f.run.wire.setup.mode == Mode::Shaker {
            let drives = drive_rows(f.engine_table)
                .into_iter()
                .map(stream_channel_value)
                .collect();
            let mut s = stream("drives", fs, drives, links.1);
            s.description.insert(
                "output_latency_frames".into(),
                match &f.caps.output_buffer {
                    Some(b) => json!({"frames": b.min_lookahead_frames, "how": "declared"}),
                    None => json!({"frames": null, "how": "unknown"}),
                },
            );
            streams.push(s);
        }
    }
    Begin {
        root,
        name: f.run.wire.run_name.clone(),
        manifest: manifest(f),
        table: table_json(f.table, f.revision),
        setup: setup(f),
        law: None,
        log: Vec::new(),
        streams,
    }
}

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

fn rows(s: &Session, m: &Measurement) -> Value {
    let (refs, resps) = s.plan.rows_json(m);
    json!({"references": refs, "responses": resps})
}

/// A line of `modal_frames`: a hit or frame as it was taken, or a change of its status.
/// Its one array, `span`, is the first station input frame and the length.
pub fn frame_line(s: &Session, point: usize, at: usize, m: &Measurement, event: &str) -> Line {
    let t = &s.points[point].taken[at];
    let unit = if s.setup.mode == Mode::Impact {
        "hits"
    } else {
        "frames"
    };
    let mut d = described("modal_frames", Value::Null, rows(s, m));
    d.insert("unit".into(), Value::from(unit));
    d.insert(
        "records".into(),
        json!({"event": "taken (the hit or frame as it came) or status (its status changed)",
               "point": "the point", "n": "its number at the point", "seq": "its number in the test",
               "status": "pending, accepted, rejected or undone",
               "reason": "why it was rejected: overload, double_hit or operator",
               "report": "impact mode: the hit's checks",
               "decay": "a hit or a burst frame: the response row whose last 5 percent of the block, after the windows, has the largest RMS as a fraction of its loudest 5 percent window; that ratio; and whether it is over decay_warn_pct (a warning, never a rejection)"}),
    );
    let mut line = t.to_json();
    if let Some(o) = line.as_object_mut() {
        o.insert("event".into(), Value::from(event));
        o.insert("point".into(), Value::from(point));
        if let Some(r) = &t.report {
            o.insert("report".into(), r.to_json());
        }
    }
    let arrays = vec![f64s(
        "span",
        vec![2],
        "frame",
        vec![t.start_frame as f64, t.frames as f64],
    )];
    ("modal_frames".into(), d, arrays, line)
}

fn frf_unit(s: &Session) -> String {
    let resp = s.plan.resps.first().map_or("", |r| r.unit.as_str());
    let refu = s.plan.refs.first().map_or("", |r| r.unit.as_str());
    if resp.is_empty() || refu.is_empty() {
        String::new()
    } else {
        format!("{resp}/{refu}")
    }
}

/// The results of a finished measurement: `modal_frf`, `modal_coherence` and
/// `modal_reference_spectra`, each a new line of its repeating result.
pub fn measurement_results(
    s: &Session,
    point: usize,
    measurement: u32,
    replaces: Option<u32>,
    e: &Estimates,
) -> Vec<Line> {
    let m = &s.plan.measurements[point];
    let freqs = s.freqs();
    let (first, df) = (freqs.first().copied().unwrap_or(0.0), s.setup.df_hz());
    let axis = json!({"kind": "frequency", "unit": "Hz", "start": num(first), "step": num(df)});
    let nl = freqs.len();
    let (nr, nf) = (e.frf.len(), e.frf.first().map_or(0, Vec::len));
    let line = json!({"measurement": measurement, "point": point, "replaces": replaces,
                      "averages": e.averages,
                      "estimator": if s.plan.refs.len() > 1 { "matrix H1" } else { s.setup.estimator.name() },
                      "rows": rows(s, m),
                      "decay": s.points[point].decay().as_ref().map(Decay::to_json)});
    let records = json!({"measurement": "the measurement's number in the test",
                         "point": "the point", "replaces": "the measurement of the point it replaces, if any",
                         "averages": "the hits or frames averaged",
                         "estimator": "H1, H2 or Hv with one reference; matrix H1 with several",
                         "rows": "the references and responses at the point",
                         "decay": "the largest decay check over the hits or frames averaged, or null when none was checked"});
    let mut out = Vec::new();

    let mut d = described("frf", axis.clone(), rows(s, m));
    d.insert("records".into(), records.clone());
    d.insert("layout".into(), json!(["response", "reference", "line"]));
    // The windows the blocks were taken with: with the exponential window, tau and
    // 1/tau, so its added damping can be taken out.
    d.insert("windows".into(), s.windows.to_json());
    let frf: Vec<(f64, f64)> = e
        .frf
        .iter()
        .flat_map(|r| r.iter().flat_map(|h| h.iter().map(|c| (c.re, c.im))))
        .collect();
    out.push((
        "modal_frf".to_owned(),
        d,
        vec![ResultArray {
            name: "frf".into(),
            shape: vec![nr, nf, nl],
            unit: frf_unit(s),
            values: ArrayValues::C128(frf),
        }],
        line.clone(),
    ));

    let mut d = described("coherence", axis.clone(), rows(s, m));
    d.insert("records".into(), records.clone());
    d.insert("layout".into(), json!(["response", "line"]));
    let (name, c) = match (&e.coherence, &e.multiple_coherence) {
        (Some(c), _) => ("coherence", c),
        (None, Some(c)) => ("multiple_coherence", c),
        (None, None) => ("coherence", &Vec::new()),
    };
    out.push((
        "modal_coherence".to_owned(),
        d,
        vec![f64s(
            name,
            vec![c.len(), nl],
            "",
            c.iter().flatten().copied().collect(),
        )],
        line.clone(),
    ));

    let mut d = described("reference_spectra", axis, rows(s, m));
    d.insert("records".into(), records);
    d.insert("layout".into(), json!(["reference", "line"]));
    let ru = s
        .plan
        .refs
        .first()
        .map_or(String::new(), |r| format!("{}^2/Hz", r.unit));
    let mut arrays = vec![f64s(
        "autospectra",
        vec![e.ref_auto.len(), nl],
        &ru,
        e.ref_auto.iter().flatten().copied().collect(),
    )];
    if let Some(k) = &e.condition {
        arrays.push(f64s("condition_number", vec![k.len()], "", k.clone()));
    }
    out.push(("modal_reference_spectra".to_owned(), d, arrays, line));
    out
}

/// `modal_outcome`: how and why the test ended, and its totals.
pub fn outcome_result(v: &ModalView, s: &Session) -> Described {
    let end = v.end.as_ref();
    let outcome = end.map_or("unknown", |e| e.outcome);
    let mut d = described("modal_outcome", Value::Null, Value::Array(Vec::new()));
    d.insert("mode".into(), Value::from(v.mode));
    d.insert("outcome".into(), Value::from(outcome));
    d.insert(
        "reason".into(),
        Value::from(end.map_or(String::new(), |e| e.message.clone())),
    );
    d.insert(
        "emergency_stop".into(),
        Value::from(end.is_some_and(|e| e.emergency_stop)),
    );
    d.insert("totals".into(), s.totals_json());
    d.insert("counts".into(), s.counts_json());
    let t = s.totals_json();
    let get = |k: &str| t[k].as_u64().unwrap_or(0) as f64;
    let arrays = vec![
        f64s("accepted", vec![1], "", vec![get("accepted")]),
        f64s("taken", vec![1], "", vec![get("taken")]),
        f64s("points_measured", vec![1], "", vec![get("points_measured")]),
        f64s("points_planned", vec![1], "", vec![get("points_planned")]),
    ];
    ("modal_outcome".into(), d, arrays)
}

/// `end.json`: the end as the run's state reports it, the frames, the totals, the state.
pub fn end_map(
    run_uid: &str,
    v: &ModalView,
    at_unix: f64,
    station_frames: Option<u64>,
    s: &Session,
) -> Map<String, Value> {
    let mut end = Map::new();
    end.insert("run_uid".into(), Value::from(run_uid));
    end.insert("run_id".into(), Value::from(v.run_id.as_str()));
    end.insert(
        "end".into(),
        v.end.as_ref().map_or(Value::Null, super::run::End::to_json),
    );
    end.insert("at_unix".into(), Value::from(at_unix));
    end.insert(
        "station_frames".into(),
        station_frames.map_or(Value::Null, Value::from),
    );
    end.insert("totals".into(), s.totals_json());
    end.insert("state".into(), state_json(Some(v)));
    end
}
