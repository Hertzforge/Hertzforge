//! What a Recorder run puts in its run folder (`docs/run-folder.md`, "What a Recorder
//! Run Saves"): the manifest, the channel table and the setup at start, the log, one
//! stream of kind `recording` per recording under `streams/`, and the end with every
//! recorded row's statistics.
//!
//! The run-level files are written by the shared record ([`crate::record`]) on its own
//! thread. The recordings are written by the core's record thread (its
//! `RecordingWriter`, which states each one in its `stream.json`), never by the thread
//! that reads the station. This module only says what is written, in the reader's form
//! (`hertzforge.io.run_folder`). Values are in the rows' own units.

use super::config::{round, Derived, Mode, WireRecorder, AVERAGING, SLOPES, WINDOWS};
use super::view::LiveRow;
use crate::protocol::num;
use crate::random::record::{start_json, station_member, table_json};
use crate::record::Begin;
use crate::station::{self, source_of, StationPlan};
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::{Caps, DiscoveredDevice};
use hertzforge_module_abi::StationClass;
use hertzforge_recorder::recording::{RecorderSidecar, STREAM_FILE};
use hertzforge_recorder::LevelTrigger;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// The recordings' folder inside a run folder: each recording is a stream there.
pub const STREAMS: &str = "streams";

/// The facts a Recorder run's record begins with, once the station is up.
pub struct StartFacts<'a> {
    pub run_id: &'a str,
    pub run_uid: &'a str,
    pub wire: &'a WireRecorder,
    pub table: &'a ChannelTable,
    pub revision: u64,
    pub plan: &'a StationPlan,
    pub class: StationClass,
    pub provenance: &'a hertzforge_station::ProvenanceBasis,
    pub devices: &'a [DiscoveredDevice],
    pub caps: &'a Caps,
    /// The derived values at the rate the station programmed.
    pub derived: &'a Derived,
    /// The recorded rows, in column order.
    pub rows: &'a [LiveRow],
    /// The signal the simulator's inputs carry, in words; `None` on hardware.
    pub sim_source: Option<Value>,
    pub started_unix: f64,
    pub utc_offset_s: Option<i64>,
    /// The overload indicator's definition.
    pub overload: Value,
    /// The log entries made before the record opened.
    pub log: Vec<Value>,
}

fn name_of<T: PartialEq + Copy>(table: &[(&'static str, T)], v: T) -> &'static str {
    table.iter().find(|(_, k)| *k == v).map_or("", |(n, _)| n)
}

/// The manifest's operator and article: the configuration's own, else the metadata
/// field of that name.
fn words(w: &WireRecorder, given: &str, key: &str) -> String {
    if !given.is_empty() {
        return given.to_owned();
    }
    w.metadata
        .iter()
        .find(|m| m.key == key)
        .map_or_else(String::new, |m| m.value.clone())
}

fn manifest(f: &StartFacts<'_>) -> Map<String, Value> {
    let w = f.wire;
    let d = f.derived;
    let mut m = Map::new();
    m.insert("test".into(), Value::from("recorder"));
    m.insert("run_uid".into(), Value::from(f.run_uid));
    m.insert("run_id".into(), Value::from(f.run_id));
    m.insert("title".into(), Value::from(w.title.as_str()));
    m.insert(
        "operator".into(),
        Value::from(words(w, &w.operator, "operator")),
    );
    m.insert(
        "article".into(),
        Value::from(words(w, &w.article, "test_article")),
    );
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
                .map(|x| json!({"key": x.key, "label": x.label, "value": x.value}))
                .collect(),
        ),
    );
    m.insert("source".into(), Value::from(source_of(f.class)));
    if let StationPlan::Sim {
        fs_hz,
        plant,
        outputs,
    } = f.plan
    {
        if source_of(f.class) == "SIM" {
            let mut sim =
                station::sim_identity(&station::sim_spec(*fs_hz, plant.as_ref(), *outputs));
            if let (Value::Object(s), Some(src)) = (&mut sim, f.sim_source.clone()) {
                s.insert("source".into(), src);
            }
            m.insert("sim".into(), sim);
        }
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
        json!({"requested_fs_hz": num(w.fs_hz), "fs_hz": num(d.fs_hz),
               "block_frames": d.block_frames, "batch_frames": d.batch_frames,
               "group_delay_frames": num(d.group_delay_frames),
               "analysis_fs_hz": num(round(d.analysis_fs_hz, 6)),
               "df_hz": num(round(d.df_hz, 6)), "lines": d.lines}),
    );
    let core = f.provenance.core_build;
    m.insert(
        "build".into(),
        json!({"version": env!("CARGO_PKG_VERSION"),
               "commit": core.trim_end_matches("-dirty"),
               "dirty": core.ends_with("-dirty")}),
    );
    m.insert(
        "start".into(),
        start_json(f.started_unix, f.utc_offset_s, 0),
    );
    // A Recorder run always records: a start under the free-space floor is refused.
    m.insert("recording".into(), json!({"streams": "on"}));
    m
}

fn setup(f: &StartFacts<'_>) -> Value {
    let w = f.wire;
    let d = f.derived;
    let t = &w.trigger;
    let range_of = |index: u32| {
        f.table
            .channels
            .iter()
            .find(|c| c.index == index)
            .map_or(0.0, |c| c.range_max)
    };
    let hysteresis = match (t.mode, t.row) {
        (Mode::Level, Some(row)) => Some(
            t.hysteresis
                .unwrap_or_else(|| LevelTrigger::default_hysteresis_eu(range_of(row))),
        ),
        _ => None,
    };
    json!({
        "config": w.raw,
        "resolved": {
            "acquisition": {"requested_fs_hz": num(w.fs_hz), "fs_hz": num(d.fs_hz),
                            "block_frames": d.block_frames, "batch_frames": d.batch_frames,
                            "group_delay_frames": num(d.group_delay_frames)},
            "outputs": "none",
            "rows": f.rows.iter().map(|r| json!({
                "index": r.index, "description": r.description, "role": r.role,
                "unit": r.unit, "range_max": num(range_of(r.index))})).collect::<Vec<_>>(),
            "trigger": {
                "mode": t.mode.name(),
                "row": t.row,
                "level": t.level.map(num),
                "slope": (t.mode == Mode::Level).then(|| name_of(&SLOPES, t.slope)),
                "hysteresis": hysteresis.map(num),
                "pre_s": num(w.pre_s()),
                "pre_frames": d.pre_frames,
                "record_s": t.record_s.map(num),
                "record_frames": d.record_frames,
                "count": t.count,
                "holdoff_s": num(t.holdoff_s),
                "holdoff_frames": d.holdoff_frames,
            },
            "analysis": {
                "block": w.analysis.block,
                "window": name_of(&WINDOWS, w.analysis.window),
                "overlap_pct": num(w.overlap_pct()),
                "averaging": name_of(&AVERAGING, w.analysis.averaging),
                "averages": w.analysis.averages,
                "decimation": w.analysis.decimation,
                "frf": w.analysis.frf.map(|(x, y)| json!({"reference": x, "response": y})),
            },
            "capture": {"run_name": w.run_name, "keep_free_gb": num(w.keep_free_gb),
                        "floor_bytes": d.floor_bytes},
        },
        "derived": d.to_json(),
        "overload": f.overload,
    })
}

/// The record a Recorder run begins with, at start: no stream yet; each recording is
/// declared as it begins ([`streams_member`]).
pub fn begin(f: &StartFacts<'_>, root: PathBuf) -> Begin {
    Begin {
        root,
        name: f.wire.run_name.clone(),
        manifest: manifest(f),
        table: table_json(f.table, f.revision),
        setup: setup(f),
        law: None,
        log: f.log.clone(),
        streams: Vec::new(),
    }
}

/// The manifest's `streams`: every recording begun so far, each a stream of kind
/// `recording` over the recorded rows.
pub fn streams_member(names: &[String], rows: &[LiveRow]) -> Value {
    let channels: Vec<u32> = rows.iter().map(|r| r.index).collect();
    Value::Array(
        names
            .iter()
            .map(|n| json!({"name": n, "kind": "recording", "channels": channels}))
            .collect(),
    )
}

/// `end.json`'s `streams`: each recording's frames, gaps and fault, as its record
/// states them.
pub fn end_streams(streams: &Path) -> Value {
    let mut out = Map::new();
    for (name, rec, _frames) in read_all(streams) {
        let dropped: u64 = rec.gaps.iter().map(|g| g.dropped_frames).sum();
        out.insert(
            name,
            json!({"frames": rec.frames, "drops": dropped, "gaps": rec.gaps.len(),
                   "fault": rec.fault, "complete": rec.complete}),
        );
    }
    Value::Object(out)
}

/// Every recording under `streams`, in the order they began: its folder name, its
/// record and its file's frames.
fn read_all(streams: &Path) -> Vec<(String, RecorderSidecar, u64)> {
    let Ok(entries) = std::fs::read_dir(streams) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.iter()
        .filter_map(|d| {
            let name = d.file_name()?.to_string_lossy().into_owned();
            let rec: RecorderSidecar =
                serde_json::from_str(&std::fs::read_to_string(d.join(STREAM_FILE)).ok()?).ok()?;
            let rows = rec.rows.len().max(rec.n_channels).max(1) as u64;
            let bytes = std::fs::metadata(d.join("telemetry.f32")).map_or(0, |m| m.len());
            Some((name, rec, bytes / (4 * rows)))
        })
        .collect()
}

/// The run's recordings as `recorder.recordings` lists them: each recording's record
/// and its file's length, in the order they began. `in_progress` names the one still
/// being written (listed with `complete` false).
pub fn list(streams: &Path, in_progress: Option<&str>) -> Vec<Value> {
    read_all(streams)
        .into_iter()
        .map(|(name, rec, frames)| {
            let rows = rec.rows.len().max(rec.n_channels).max(1) as u64;
            let complete = rec.complete && in_progress != Some(name.as_str());
            json!({
                "name": name,
                "folder": format!("{STREAMS}/{name}"),
                "trigger": if rec.trigger.is_empty() { "manual".to_owned() } else { rec.trigger.clone() },
                "requested_fs_hz": num(rec.requested_fs_hz),
                "fs_hz": num(rec.fs_hz),
                "group_delay_frames": num(rec.group_delay_frames),
                "rows": rec.rows,
                "first_station_frame": rec.first_station_frame,
                "trigger_station_frame": rec.trigger_station_frame,
                "pre_frames": rec.pre_frames,
                "pre_frames_asked": rec.pre_frames_asked,
                "frames": frames,
                "seconds": num(round(frames as f64 / rec.fs_hz.max(1e-9), 6)),
                "bytes": frames * rows * 4,
                "gap_free": rec.gaps.is_empty() && rec.fault.is_none(),
                "gaps": rec.gaps.iter().map(|g| json!({"at_frame": g.at_frame, "dropped_frames": g.dropped_frames})).collect::<Vec<_>>(),
                "marks": rec.marks.len(),
                "complete": complete,
                "fault": rec.fault,
            })
        })
        .collect()
}
