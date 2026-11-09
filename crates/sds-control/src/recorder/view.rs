//! The Recorder run's state and live frame (schema/PROTOCOL.md, "Recorder Run State",
//! "Recorder Display"): what a run shows, held as one view the run's thread composes
//! ten times a second, and its JSON.

use super::config::{round, Derived, Mode, WireRecorder};
use crate::codec::Arrays;
use crate::explorer::view::StationRef;
use crate::protocol::{num, opt_num};
use crate::views::{self, Row, Status};
use serde_json::{json, Map, Value};

/// A Recorder run's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Configured,
    Starting,
    Running,
    Stopping,
    Finished,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Configured => "configured",
            State::Starting => "starting",
            State::Running => "running",
            State::Stopping => "stopping",
            State::Finished => "finished",
        }
    }

    /// Starting, running or stopping: the station is reserved.
    pub fn is_live(self) -> bool {
        matches!(self, State::Starting | State::Running | State::Stopping)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub requested_fs_hz: f64,
    pub fs_hz: f64,
    pub block_frames: usize,
    pub batch_frames: usize,
    pub group_delay_frames: f64,
    pub actual: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Trigger {
    pub armed: bool,
    pub pre_frames: u64,
    pub held_frames: u64,
    pub fires: u64,
    pub last_station_frame: Option<u64>,
    pub saved: u64,
    pub left: Option<u64>,
    pub holdoff_left_s: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    pub name: String,
    pub trigger: String,
    pub first_station_frame: u64,
    pub trigger_station_frame: u64,
    pub pre_frames: u64,
    pub pre_frames_asked: u64,
    pub frames_fed: u64,
    pub frames_on_disk: u64,
    pub target_frames: Option<u64>,
    pub seconds: f64,
    pub length_s: Option<f64>,
    pub backlog_s: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Disk {
    pub free_bytes: Option<u64>,
    pub floor_bytes: u64,
    pub seconds_above_floor: Option<u64>,
    pub fault: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Health {
    pub batches: u64,
    pub blocks_dropped: u64,
    pub driver_warnings: u64,
    pub station_errors: u64,
    pub pass_last_ms: Option<f64>,
    pub pass_worst_ms: Option<f64>,
    pub batch_gap_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct End {
    pub outcome: &'static str,
    pub message: String,
    pub at_s: Option<f64>,
    pub recordings: u64,
}

impl End {
    /// A finished run's phase.
    pub fn phase(&self) -> &'static str {
        match self.outcome {
            "operator_stop" | "safe_stop" => "stopped",
            "operator_abort" => "aborted",
            _ => "faulted",
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "outcome": self.outcome,
            "message": self.message,
            "at_s": opt_num(self.at_s),
            "recordings": self.recordings,
        })
    }
}

/// A recorded row as the live frame shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveRow {
    pub index: u32,
    pub description: String,
    pub role: &'static str,
    pub unit: String,
    pub rms: Option<f64>,
    pub peak: Option<f64>,
    pub overload: bool,
    pub marks: u64,
}

/// Everything a Recorder run shows.
#[derive(Debug, Clone, PartialEq)]
pub struct RecorderView {
    pub run_id: String,
    pub state: State,
    pub phase: &'static str,
    pub title: String,
    pub source: Option<&'static str>,
    pub station: StationRef,
    pub grid: Grid,
    pub clock: Option<&'static str>,
    pub rows: Vec<u32>,
    pub mode: Mode,
    pub t_s: Option<f64>,
    pub station_frames: Option<u64>,
    pub trigger: Option<Trigger>,
    pub recording: Option<Recording>,
    pub recordings: u64,
    pub marks: u64,
    pub disk: Option<Disk>,
    pub health: Option<Health>,
    pub stop: Option<&'static str>,
    pub end: Option<End>,
    pub live_rows: Vec<LiveRow>,
}

/// The recorded rows as the live frame lists them, before anything is read.
pub fn table_rows(table: &hertzforge_channel_table::ChannelTable) -> Vec<LiveRow> {
    super::config::recorded_rows(table)
        .into_iter()
        .map(|c| LiveRow {
            index: c.index,
            description: c.description.clone(),
            role: crate::table::role_name(c.role),
            unit: crate::table::unit_name(&c.units),
            rms: None,
            peak: None,
            overload: false,
            marks: 0,
        })
        .collect()
}

/// A configured run's view.
pub fn configured_view(
    run_id: &str,
    w: &WireRecorder,
    d: &Derived,
    station: StationRef,
    table: &hertzforge_channel_table::ChannelTable,
) -> RecorderView {
    let source = if station.kind == "sim" || station.class == "ni_simulated" {
        "SIM"
    } else {
        "HARDWARE"
    };
    RecorderView {
        run_id: run_id.to_owned(),
        state: State::Configured,
        phase: "idle",
        title: w.title.clone(),
        source: Some(source),
        station,
        grid: Grid {
            requested_fs_hz: w.fs_hz,
            fs_hz: d.fs_hz,
            block_frames: d.block_frames,
            batch_frames: d.batch_frames,
            group_delay_frames: d.group_delay_frames,
            actual: false,
        },
        clock: None,
        rows: d.rows.clone(),
        mode: w.trigger.mode,
        t_s: None,
        station_frames: None,
        trigger: (w.trigger.mode != Mode::Start).then_some(Trigger {
            armed: false,
            pre_frames: d.pre_frames,
            held_frames: 0,
            fires: 0,
            last_station_frame: None,
            saved: 0,
            left: match w.trigger.mode {
                Mode::Level => w.trigger.count,
                _ => None,
            },
            holdoff_left_s: 0.0,
        }),
        recording: None,
        recordings: 0,
        marks: 0,
        disk: Some(Disk {
            free_bytes: d.free_bytes,
            floor_bytes: d.floor_bytes,
            seconds_above_floor: d.seconds_above_floor(),
            fault: None,
        }),
        health: None,
        stop: None,
        end: None,
        live_rows: table_rows(table),
    }
}

fn grid_json(g: &Grid) -> Value {
    json!({
        "requested_fs_hz": num(g.requested_fs_hz),
        "fs_hz": num(g.fs_hz),
        "block_frames": g.block_frames,
        "batch_frames": g.batch_frames,
        "group_delay_frames": num(round(g.group_delay_frames, 2)),
        "actual": g.actual,
    })
}

pub fn trigger_json(t: Option<&Trigger>) -> Value {
    match t {
        None => Value::Null,
        Some(t) => json!({
            "armed": t.armed,
            "pre_frames": t.pre_frames,
            "held_frames": t.held_frames,
            "fires": t.fires,
            "last_station_frame": t.last_station_frame,
            "saved": t.saved,
            "left": t.left,
            "holdoff_left_s": num(round(t.holdoff_left_s, 3)),
        }),
    }
}

pub fn recording_json(r: Option<&Recording>) -> Value {
    match r {
        None => Value::Null,
        Some(r) => json!({
            "name": r.name,
            "trigger": r.trigger,
            "first_station_frame": r.first_station_frame,
            "trigger_station_frame": r.trigger_station_frame,
            "pre_frames": r.pre_frames,
            "pre_frames_asked": r.pre_frames_asked,
            "frames_fed": r.frames_fed,
            "frames_on_disk": r.frames_on_disk,
            "target_frames": r.target_frames,
            "seconds": num(round(r.seconds, 3)),
            "length_s": opt_num(r.length_s),
            "backlog_s": num(round(r.backlog_s, 3)),
        }),
    }
}

pub fn disk_json(d: Option<&Disk>) -> Value {
    match d {
        None => Value::Null,
        Some(d) => json!({
            "free_bytes": d.free_bytes,
            "floor_bytes": d.floor_bytes,
            "seconds_above_floor": d.seconds_above_floor,
            "fault": d.fault,
        }),
    }
}

pub fn health_json(h: Option<&Health>) -> Value {
    match h {
        None => Value::Null,
        Some(h) => json!({
            "batches": h.batches,
            "blocks_dropped": h.blocks_dropped,
            "driver_warnings": h.driver_warnings,
            "station_errors": h.station_errors,
            "pass_ms": {
                "last": opt_num(h.pass_last_ms.map(|x| round(x, 1))),
                "worst": opt_num(h.pass_worst_ms.map(|x| round(x, 1))),
            },
            "batch_gap_ms": opt_num(h.batch_gap_ms.map(|x| round(x, 1))),
        }),
    }
}

/// `recorder.state`: the run's state object, or the unconfigured one.
pub fn state_json(v: Option<&RecorderView>) -> Value {
    let Some(v) = v else {
        return json!({
            "run_id": null, "state": "unconfigured", "phase": "idle", "title": null,
            "source": null, "station": null, "grid": null, "clock": null, "outputs": "none",
            "rows": null, "mode": null, "t_s": null, "station_frames": null,
            "trigger": null, "recording": null, "recordings": 0, "marks": 0, "disk": null,
            "health": null, "stop": null, "end": null,
        });
    };
    let mut m = Map::new();
    m.insert("run_id".into(), Value::from(v.run_id.as_str()));
    m.insert("state".into(), Value::from(v.state.name()));
    m.insert("phase".into(), Value::from(v.phase));
    m.insert("title".into(), Value::from(v.title.as_str()));
    m.insert("source".into(), v.source.map_or(Value::Null, Value::from));
    m.insert("station".into(), v.station.to_json());
    m.insert("grid".into(), grid_json(&v.grid));
    m.insert("clock".into(), v.clock.map_or(Value::Null, Value::from));
    m.insert("outputs".into(), Value::from("none"));
    m.insert("rows".into(), json!(v.rows));
    m.insert("mode".into(), Value::from(v.mode.name()));
    m.insert("t_s".into(), opt_num(v.t_s.map(|t| round(t, 3))));
    m.insert("station_frames".into(), json!(v.station_frames));
    m.insert("trigger".into(), trigger_json(v.trigger.as_ref()));
    m.insert("recording".into(), recording_json(v.recording.as_ref()));
    m.insert("recordings".into(), Value::from(v.recordings));
    m.insert("marks".into(), Value::from(v.marks));
    m.insert("disk".into(), disk_json(v.disk.as_ref()));
    m.insert("health".into(), health_json(v.health.as_ref()));
    m.insert("stop".into(), v.stop.map_or(Value::Null, Value::from));
    m.insert(
        "end".into(),
        v.end.as_ref().map_or(Value::Null, End::to_json),
    );
    Value::Object(m)
}

/// What a `recorder.state` event is sent on a change of: the state, the phase, the stop
/// and the end.
pub fn state_key(v: Option<&RecorderView>) -> String {
    match v {
        None => "unconfigured".into(),
        Some(v) => format!(
            "{}|{}|{}|{:?}|{:?}",
            v.run_id,
            v.state.name(),
            v.phase,
            v.stop,
            v.end.as_ref().map(|e| (e.outcome, e.message.clone()))
        ),
    }
}

fn row_json(r: &LiveRow) -> Value {
    json!({
        "index": r.index,
        "description": r.description,
        "role": r.role,
        "unit": r.unit,
        "rms": opt_num(r.rms.map(|x| round(x, 4))),
        "peak": opt_num(r.peak.map(|x| round(x, 4))),
        "overload": r.overload,
        "marks": r.marks,
    })
}

/// The fields `recorder.live` and `recorder.data` share.
pub fn live_head(v: &RecorderView) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("run_id".into(), Value::from(v.run_id.as_str()));
    m.insert("source".into(), Value::from(v.source.unwrap_or("SIM")));
    m.insert("class".into(), Value::from(v.station.class));
    m.insert("state".into(), Value::from(v.state.name()));
    m.insert("phase".into(), Value::from(v.phase));
    m.insert("t_s".into(), opt_num(v.t_s.map(|t| round(t, 3))));
    m.insert("station_frames".into(), json!(v.station_frames));
    m.insert(
        "grid".into(),
        json!({
            "requested_fs_hz": num(v.grid.requested_fs_hz),
            "fs_hz": num(v.grid.fs_hz),
            "group_delay_frames": num(round(v.grid.group_delay_frames, 2)),
        }),
    );
    m.insert(
        "rows".into(),
        Value::Array(v.live_rows.iter().map(row_json).collect()),
    );
    m.insert("trigger".into(), trigger_json(v.trigger.as_ref()));
    m.insert("recording".into(), recording_json(v.recording.as_ref()));
    m.insert("recordings".into(), Value::from(v.recordings));
    m.insert("marks".into(), Value::from(v.marks));
    m.insert("disk".into(), disk_json(v.disk.as_ref()));
    m.insert("health".into(), health_json(v.health.as_ref()));
    m
}

/// The **Run** reading.
pub fn run_reading(v: &RecorderView) -> Value {
    let info = Status::Info;
    let none = |label: &str| Row::text(label, "none", info);
    let rec = v.recording.as_ref();
    let rows = vec![
        Row::number("Rate", v.grid.fs_hz, Some("Hz"), info),
        Row::number("Asked", v.grid.requested_fs_hz, Some("Hz"), info),
        Row::number(
            "Group delay",
            round(v.grid.group_delay_frames, 2),
            Some("frames"),
            info,
        ),
        match rec {
            Some(r) => Row::text("Recording", &r.name, info),
            None => Row::text("Recording", "none", info),
        },
        match rec {
            Some(r) => Row::number("Recorded", round(r.seconds, 3), Some("s"), info),
            None => none("Recorded"),
        },
        match rec {
            Some(r) => Row::number("Backlog", round(r.backlog_s, 3), Some("s"), info),
            None => none("Backlog"),
        },
        Row::number("Saved", v.recordings as f64, None, info),
        Row::number(
            "Fires",
            v.trigger.as_ref().map_or(0, |t| t.fires) as f64,
            None,
            info,
        ),
        Row::number("Marks", v.marks as f64, None, info),
        match v.disk.as_ref().and_then(|d| d.seconds_above_floor) {
            Some(s) => Row::number("Free above floor", s as f64, Some("s"), info),
            None => none("Free above floor"),
        },
    ];
    views::reading("Run", rows)
}

/// `recorder.live`: the head and the Run reading.
pub fn live_json(v: &RecorderView, _arrays: &mut Arrays) -> Value {
    let mut m = live_head(v);
    m.insert("views".into(), Value::Array(vec![run_reading(v)]));
    Value::Object(m)
}
