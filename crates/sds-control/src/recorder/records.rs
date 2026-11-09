//! The run folder (schema/PROTOCOL.md, "Recorder Records"): `run.json` at start,
//! `log.jsonl` and `marks.jsonl` a line at a time, `end.json` at the end, and
//! `recordings/rec_NNNN/`, which the record thread writes. Everything here is written
//! by the run's own thread, never by the thread that reads the station. A run whose
//! name is taken gets the first free `<name>-<n>`.

use crate::protocol::num;
use hertzforge_recorder::recording::RecorderSidecar;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The recordings' folder inside a run folder.
pub const RECORDINGS: &str = "recordings";

/// The run folder.
pub struct Records {
    folder: PathBuf,
}

impl Records {
    /// Make the run folder `name` under `root`, or the first free `name-<n>`, with its
    /// recordings folder.
    pub fn open(root: &Path, name: &str) -> std::io::Result<Records> {
        for n in 1..10_000u32 {
            let p = if n == 1 {
                root.join(name)
            } else {
                root.join(format!("{name}-{n}"))
            };
            if p.exists() {
                continue;
            }
            std::fs::create_dir_all(p.join(RECORDINGS))?;
            return Ok(Records { folder: p });
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("no free run folder named {name} under {}", root.display()),
        ))
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Where the recordings go.
    pub fn recordings(&self) -> PathBuf {
        self.folder.join(RECORDINGS)
    }

    fn write(&self, file: &str, v: &Value) {
        let text = serde_json::to_string_pretty(v).unwrap_or_default();
        let _ = std::fs::write(self.folder.join(file), text + "\n");
    }

    fn append(&self, file: &str, lines: &[Value]) {
        if lines.is_empty() {
            return;
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.folder.join(file))
        {
            for l in lines {
                let _ = writeln!(f, "{l}");
            }
        }
    }

    pub fn write_run(&self, run: &Value) {
        self.write("run.json", run);
        // The two line files exist from the start, empty until something happens.
        for f in ["log.jsonl", "marks.jsonl"] {
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.folder.join(f));
        }
    }

    pub fn log(&self, entries: &[Value]) {
        self.append("log.jsonl", entries);
    }

    pub fn marks(&self, marks: &[Value]) {
        self.append("marks.jsonl", marks);
    }

    pub fn write_end(&self, end: &Value) {
        self.write("end.json", end);
    }
}

/// The run's recordings as `recorder.recordings` lists them: each recording's record
/// and its file's length, in the order they began. `in_progress` names the one still
/// being written (listed with `complete` false).
pub fn list(recordings: &Path, in_progress: Option<&str>) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(recordings) else {
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
                serde_json::from_str(&std::fs::read_to_string(d.join("recorder.json")).ok()?)
                    .ok()?;
            let rows = rec.rows.len().max(rec.n_channels).max(1) as u64;
            let bytes = std::fs::metadata(d.join("telemetry.f32")).map_or(0, |m| m.len());
            let frames = bytes / (4 * rows);
            let complete = rec.complete && in_progress != Some(name.as_str());
            Some(json!({
                "name": name,
                "folder": format!("{RECORDINGS}/{name}"),
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
                "seconds": num(super::config::round(frames as f64 / rec.fs_hz.max(1e-9), 6)),
                "bytes": frames * rows * 4,
                "gap_free": rec.gaps.is_empty() && rec.fault.is_none(),
                "gaps": rec.gaps.iter().map(|g| json!({"at_frame": g.at_frame, "dropped_frames": g.dropped_frames})).collect::<Vec<_>>(),
                "marks": rec.marks.len(),
                "complete": complete,
                "fault": rec.fault,
            }))
        })
        .collect()
}
