//! The run folder (schema/PROTOCOL.md, "Explorer Records"): `explorer.json` once the run
//! has armed, `intents.jsonl` a line per intent the run took, `survey.json` and
//! `end.json` at the end. Written by the run's own thread, never by the pump. No
//! samples are recorded. A run whose name is taken gets the first free `<name>-<n>`.

use super::view::{state_json, ExplorerView};
use crate::protocol::num;
use hertzforge_explorer_host::{Adoption, IntentKind, IntentRecord, SweepDirection};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The run folder, or nothing when it could not be made (the run goes on; the log says
/// so through the missing "records in" line).
pub struct Records {
    folder: Option<PathBuf>,
}

fn adoption(a: &Adoption) -> Value {
    json!({"asked": num(a.asked), "adopted": num(a.adopted), "clamped": a.clamped})
}

/// One intent as a line of `intents.jsonl`.
pub fn intent_json(i: &IntentRecord, fs_hz: f64) -> Value {
    let mut v = json!({
        "seq": i.seq,
        "t_s": num(super::config::round(i.input_frame as f64 / fs_hz, 6)),
        "frame": i.frame,
        "input_frame": i.input_frame,
    });
    let m = v.as_object_mut().expect("an object");
    match &i.kind {
        IntentKind::Steer { frequency, level } => {
            m.insert("verb".into(), Value::from("steer"));
            m.insert(
                "hz".into(),
                frequency.as_ref().map_or(Value::Null, adoption),
            );
            m.insert("vpk".into(), level.as_ref().map_or(Value::Null, adoption));
        }
        IntentKind::Sweep {
            direction,
            rate_oct_per_min,
        } => {
            m.insert("verb".into(), Value::from("sweep"));
            m.insert(
                "direction".into(),
                Value::from(match direction {
                    SweepDirection::Up => "up",
                    SweepDirection::Down => "down",
                }),
            );
            m.insert("rate_oct_per_min".into(), adoption(rate_oct_per_min));
        }
        IntentKind::Hold { hz } => {
            m.insert("verb".into(), Value::from("hold"));
            m.insert("hz".into(), num(*hz));
        }
        IntentKind::Stop { cause } => {
            m.insert("verb".into(), Value::from("stop"));
            m.insert("cause".into(), Value::from(cause.as_str()));
        }
    }
    v
}

impl Records {
    /// Make the run folder `name` under `root`, or the first free `name-<n>`.
    pub fn open(root: &Path, name: &str) -> Records {
        let folder = (1..10_000u32).find_map(|n| {
            let p = if n == 1 {
                root.join(name)
            } else {
                root.join(format!("{name}-{n}"))
            };
            if p.exists() {
                return None;
            }
            std::fs::create_dir_all(&p).ok().map(|_| p)
        });
        Records { folder }
    }

    pub fn folder(&self) -> Option<&Path> {
        self.folder.as_deref()
    }

    fn write(&self, file: &str, v: &Value) {
        if let Some(f) = &self.folder {
            let text = serde_json::to_string_pretty(v).unwrap_or_default();
            let _ = std::fs::write(f.join(file), text + "\n");
        }
    }

    /// `explorer.json`: the configuration, the station, the rates and the derived values.
    pub fn write_head(&self, config: &Value, derived: &Value, v: &ExplorerView) {
        self.write(
            "explorer.json",
            &json!({
                "run_id": v.run_id,
                "config": config,
                "station": v.station.to_json(),
                "requested_fs_hz": num(v.grid.requested_fs_hz),
                "fs_hz": num(v.grid.fs_hz),
                "block_frames": v.grid.block_frames,
                "lookahead_frames": v.grid.lookahead_frames,
                "pipeline_frames": v.pipeline_frames,
                "derived": derived,
            }),
        );
    }

    /// A line of `intents.jsonl`.
    pub fn intent(&self, i: &IntentRecord, fs_hz: f64) {
        if let Some(f) = &self.folder {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(f.join("intents.jsonl"))
            {
                let _ = writeln!(file, "{}", intent_json(i, fs_hz));
            }
        }
    }

    /// `survey.json` and `end.json`.
    pub fn finish(&self, points: &[(f64, f64)], peak: Option<(f64, f64)>, v: &ExplorerView) {
        self.write(
            "survey.json",
            &json!({
                "channel": v.control.as_ref().map(|c| c.index),
                "unit": v.control_unit,
                "points": points.iter().map(|(f, l)| json!([num(*f), num(*l)])).collect::<Vec<_>>(),
                "peak": peak.map_or(Value::Null, |(f, l)| json!({"hz": num(f), "level": num(l)})),
            }),
        );
        let state = state_json(Some(v));
        self.write(
            "end.json",
            &json!({"run_id": v.run_id, "end": state["end"], "state": state}),
        );
    }
}
