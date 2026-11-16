//! What a Playback run puts in its run folder (`docs/run-folder.md`, "What a Playback
//! Run Saves"): the manifest, the channel table and the setup at start (the
//! configuration as accepted, the drive file's sidecar, the check report, the map, the
//! level, the ceiling, the rolls, the stop and every abort in force, and that the run is
//! open loop with no control law), the log, the two streams (every enabled input as
//! read, every drive as the device accepted it), the two results at the end
//! (`playback_check`, `playback_outcome`) and the end.
//!
//! The writing is the shared record's ([`crate::record`]), on its own thread; this
//! module only says what is written.

use super::config;
use super::run::{state_json, PlaybackRun, PlaybackView};
use crate::protocol::num;
use crate::random::record::{
    build_value, drive_rows, input_rows, sim_value, start_json, station_member,
    stream_channel_value, table_json,
};
use crate::record::{tap::TapLink, ArrayValues, Begin, ResultArray, StreamSpec};
use crate::station::{source_of, StationPlan};
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::{Caps, DiscoveredDevice};
use hertzforge_module_abi::StationClass;
use hertzforge_playback::plan::{STOP_RAMP_DEPTH_DB, STOP_RAMP_S};
use hertzforge_playback::CheckedDrive;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

/// The words a Playback record states of its control.
pub const OPEN_LOOP: &str = "open loop, no control law: the drive file is played as it is, \
                             scaled by the level, and nothing is corrected from the response";

/// The facts a run's record begins with, before the station is opened.
pub struct StartFacts<'a> {
    pub run: &'a PlaybackRun,
    pub run_uid: &'a str,
    pub table: &'a ChannelTable,
    pub revision: u64,
    pub plan: &'a StationPlan,
    pub class: StationClass,
    pub provenance: &'a hertzforge_station::ProvenanceBasis,
    pub devices: &'a [DiscoveredDevice],
    pub caps: &'a Caps,
    pub started_unix: f64,
    pub utc_offset_s: Option<i64>,
    pub streams_off: Option<String>,
}

fn manifest(f: &StartFacts<'_>) -> Map<String, Value> {
    let w = &f.run.wire;
    let fs = f.run.drive.file.rate_hz;
    let mut m = Map::new();
    m.insert("test".into(), Value::from("playback"));
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
            "rule": "the drive file's own rate: a station that programs another is refused",
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

/// Every abort in force.
pub fn aborts_json(drive: &CheckedDrive, table: &ChannelTable) -> Value {
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
                   "displacement_mil": c.abort.displacement_mil.map(num),
                   "unit": crate::table::unit_name(&c.units)})
        })
        .collect();
    json!({
        "drive": {"ceiling_v": num(drive.setup.ceiling_v),
                  "judged": "every sample, after the level and the stop ramp"},
        "channels": channels,
        "response": "each row's abort limits from the channel table, on every sample: a \
                     breach takes the drive to zero at once",
        "input_overload": "an overload on a row with abort limits takes the drive to zero at once",
        "underrun": "the read-ahead running empty takes the drive to zero at once",
        "reader": "a failure of the drive file's reader takes the drive to zero at once",
    })
}

/// The check report without its previews (the previews are in `playback_check`).
pub fn check_summary(drive: &CheckedDrive) -> Value {
    drive.report_json()
}

fn setup(f: &StartFacts<'_>) -> Value {
    let w = &f.run.wire;
    let d = &f.run.drive;
    json!({
        "config": w.raw,
        "resolved": {
            "control": OPEN_LOOP,
            "drive": {
                "folder": d.file.folder.display().to_string(),
                "sha256": d.file.sha256,
                "rate_hz": num(d.file.rate_hz),
                "samples": d.file.samples,
                "channels": d.file.channels.iter().enumerate().map(|(k, c)| json!({
                    "channel": k, "name": c.name, "unit": c.unit, "source": c.source,
                    "drive_row": d.setup.map.get(k)})).collect::<Vec<_>>(),
                "sidecar": d.file.sidecar,
            },
            "map": d.setup.map,
            "level_db": num(d.setup.level_db),
            "gain": num(d.setup.gain()),
            "ceiling_v": num(d.setup.ceiling_v),
            "pre_roll_s": num(d.setup.pre_roll_s),
            "post_roll_s": num(d.setup.post_roll_s),
            "stop": {"depth_db": num(STOP_RAMP_DEPTH_DB), "ramp_s": num(STOP_RAMP_S),
                     "then": "zero"},
            "inputs": input_rows(f.table).iter().map(|c| c.index).collect::<Vec<_>>(),
            "drives": drive_rows(f.table).iter().map(|c| c.index).collect::<Vec<_>>(),
            "aborts": aborts_json(d, f.table),
            "capture": {"run_name": w.run_name},
        },
        "check": check_summary(d),
        "derived": config::derived(w, &d.file),
    })
}

/// The record a Playback run begins with, before the station is opened.
pub fn begin(f: &StartFacts<'_>, root: PathBuf, links: (TapLink, TapLink)) -> Begin {
    let fs = f.run.drive.file.rate_hz;
    let mut streams = Vec::new();
    if f.streams_off.is_none() {
        let mut inputs = Map::new();
        inputs.insert("kind".into(), Value::from("inputs"));
        inputs.insert("fs_hz".into(), num(fs));
        inputs.insert(
            "channels".into(),
            Value::Array(
                input_rows(f.table)
                    .into_iter()
                    .map(stream_channel_value)
                    .collect(),
            ),
        );
        inputs.insert("first_station_frame".into(), Value::from(0));
        streams.push(StreamSpec {
            name: "inputs".into(),
            description: inputs,
            link: links.0,
        });
        let mut d = Map::new();
        d.insert("kind".into(), Value::from("drives"));
        d.insert("fs_hz".into(), num(fs));
        d.insert(
            "channels".into(),
            Value::Array(
                drive_rows(f.table)
                    .into_iter()
                    .map(stream_channel_value)
                    .collect(),
            ),
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

/// `playback_check`: the D3 report, each file channel's numbers and its envelopes.
pub fn check_result(drive: &CheckedDrive) -> (String, Map<String, Value>, Vec<ResultArray>) {
    let ch = &drive.check.channels;
    let n = ch.len();
    let buckets = ch.first().map_or(0, |c| c.drive.min.len());
    let bucket = ch.first().map_or(1, |c| c.drive.bucket);
    let mut d = Map::new();
    d.insert("kind".into(), Value::from("drive_check"));
    d.insert(
        "abscissa".into(),
        json!({"name": "bucket_start_s", "unit": "s", "step": num(bucket as f64 / drive.file.rate_hz),
               "samples_per_bucket": bucket}),
    );
    d.insert(
        "channels".into(),
        Value::Array(
            ch.iter()
                .map(|c| {
                    json!({"file_channel": c.channel, "name": c.name,
                                "drive_row": drive.setup.map.get(c.channel)})
                })
                .collect(),
        ),
    );
    d.insert("report".into(), drive.report_json());
    let col = |f: &dyn Fn(&hertzforge_playback::ChannelCheck) -> f64| -> Vec<f64> {
        ch.iter().map(f).collect()
    };
    let env = |f: &dyn Fn(&hertzforge_playback::ChannelCheck) -> &Vec<f64>| -> Vec<f64> {
        ch.iter()
            .flat_map(|c| {
                let mut v = f(c).clone();
                v.resize(buckets, f64::NAN);
                v
            })
            .collect()
    };
    let arrays = vec![
        f64s("peak", vec![n], "V", col(&|c| c.peak_v)),
        f64s("start", vec![n], "V", col(&|c| c.start_v)),
        f64s("end", vec![n], "V", col(&|c| c.end_v)),
        f64s("velocity_end", vec![n], "V s", col(&|c| c.velocity_end)),
        f64s("velocity_peak", vec![n], "V s", col(&|c| c.velocity_peak)),
        f64s(
            "displacement_end",
            vec![n],
            "V s^2",
            col(&|c| c.displacement_end),
        ),
        f64s(
            "displacement_peak",
            vec![n],
            "V s^2",
            col(&|c| c.displacement_peak),
        ),
        f64s("drive_min", vec![n, buckets], "V", env(&|c| &c.drive.min)),
        f64s("drive_max", vec![n, buckets], "V", env(&|c| &c.drive.max)),
        f64s(
            "velocity_min",
            vec![n, buckets],
            "V s",
            env(&|c| &c.velocity.min),
        ),
        f64s(
            "velocity_max",
            vec![n, buckets],
            "V s",
            env(&|c| &c.velocity.max),
        ),
        f64s(
            "displacement_min",
            vec![n, buckets],
            "V s^2",
            env(&|c| &c.displacement.min),
        ),
        f64s(
            "displacement_max",
            vec![n, buckets],
            "V s^2",
            env(&|c| &c.displacement.max),
        ),
    ];
    ("playback_check".into(), d, arrays)
}

/// `playback_outcome`: the samples played, and how and why the run ended.
pub fn outcome_result(v: &PlaybackView) -> (String, Map<String, Value>, Vec<ResultArray>) {
    let end = v.end.as_ref();
    let outcome = end.map_or("unknown", |e| e.outcome);
    let ended = match outcome {
        "completed" => "completed",
        "stopped" => "stopped",
        _ => "aborted",
    };
    let mut d = Map::new();
    d.insert("kind".into(), Value::from("playback_outcome"));
    d.insert("abscissa".into(), Value::Null);
    d.insert(
        "channels".into(),
        Value::Array(v.map.iter().map(|r| Value::from(*r)).collect()),
    );
    d.insert("ended".into(), Value::from(ended));
    d.insert("outcome".into(), Value::from(outcome));
    d.insert(
        "reason".into(),
        Value::from(end.map_or(String::new(), |e| e.message.clone())),
    );
    d.insert("samples".into(), Value::from(v.samples));
    d.insert(
        "emergency_stop".into(),
        Value::from(end.is_some_and(|e| e.emergency_stop)),
    );
    d.insert(
        "trip".into(),
        end.and_then(|e| e.trip.clone()).unwrap_or(Value::Null),
    );
    let played = end.map_or(v.played, |e| e.played);
    let arrays = vec![
        f64s("played", vec![1], "sample", vec![played as f64]),
        f64s(
            "played_s",
            vec![1],
            "s",
            vec![played as f64 / v.fs_hz.max(1.0)],
        ),
        f64s("generated", vec![1], "sample", vec![v.generated as f64]),
    ];
    ("playback_outcome".into(), d, arrays)
}

/// `end.json`: the end as the run's state reports it, the frames, the state.
pub fn end_map(
    run_uid: &str,
    v: &PlaybackView,
    at_unix: f64,
    station_frames: Option<u64>,
    drive: &CheckedDrive,
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
    end.insert(
        "played".into(),
        json!({"samples": v.end.as_ref().map_or(v.played, |e| e.played),
               "of": drive.file.samples}),
    );
    end.insert("state".into(), state_json(Some(v)));
    end
}
