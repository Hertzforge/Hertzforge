//! The capture LIST verb (captures are daemon-local with a LIST verb; fetch-to-Mac
//! stays deferred).
//!
//! # What this reports, and what it refuses to guess
//!
//! **The FILE is the authority on frame count, not the sidecar.** `capture.json` is
//! rewritten on a flush cadence, so a capture killed between flushes has a sidecar
//! whose `frames_written` is stale by up to one interval - while `telemetry.f32`'s
//! length is exactly what was written. The lister therefore derives `frames` from the
//! file size and uses the sidecar only for what the file cannot say (channel count,
//! rate, station, completion). This is `hertzforge-recorder`'s own rule (the file size is
//! the authority on frame count) applied at the listing layer.
//!
//! **A capture with no readable sidecar is still listed**, with the fields it cannot
//! answer left at their honest-absence values (`n_channels: 0`, `duration_s: 0.0`,
//! `station_label: ""`). Omitting it would be worse: a directory full of data that
//! does not appear in the list reads as "nothing was captured".
//!
//! **`complete`, `station_label` and `run_id` are carried** because a list of names
//! and sizes would let a remote operator treat a truncated driver-simulated run as a
//! measurement. Those three fields are the difference between a file listing and
//! evidence.

use std::path::Path;

use hertzforge_protocol::CaptureInfo;

/// Every capture under `data_root`, newest name last (directory order, sorted).
///
/// Never fails: an unreadable data root answers with an empty list, because "I cannot
/// read the directory" and "there are no captures" are both correctly rendered as an
/// empty list plus whatever the daemon logged, and returning an error here would make
/// a remote UI show a modal for a case that is normal on a fresh install.
pub fn list(data_root: &Path) -> Vec<CaptureInfo> {
    let Ok(entries) = std::fs::read_dir(data_root) else {
        return Vec::new();
    };
    let mut out: Vec<CaptureInfo> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| describe(&e.path()))
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Describe one run directory, or `None` when it holds no telemetry at all (an
/// unrelated directory under the data root - `config/`, `Route_Maps/`, ...).
pub fn describe(dir: &Path) -> Option<CaptureInfo> {
    let telemetry = dir.join(hertzforge_capture::TELEMETRY_FILE);
    let bytes = std::fs::metadata(&telemetry).ok()?.len();
    let name = dir.file_name()?.to_string_lossy().to_string();

    let sidecar = std::fs::read_to_string(dir.join(hertzforge_capture::SIDECAR_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<hertzforge_capture::CaptureSidecar>(&t).ok());

    let (n_channels, rate_hz, complete, started_unix) = match &sidecar {
        Some(s) => (
            s.config.n_channels as u32,
            s.config.sample_rate_hz,
            s.complete,
            s.start_unix_secs,
        ),
        // Honest absence: no sidecar means we know the bytes and nothing else.
        None => (0, 0.0, false, 0),
    };

    // THE FILE is the authority (see the module docs).
    let frames = if n_channels == 0 {
        0
    } else {
        bytes / (n_channels as u64 * 4)
    };
    let duration_s = if rate_hz > 0.0 {
        frames as f64 / rate_hz
    } else {
        0.0
    };

    // The station and run id ride the capture's `run_config_json`, which the daemon
    // writes as its provenance block. A capture written by an older build (or by a
    // path that did not record one) leaves these empty rather than guessed.
    let (station_label, run_id) = sidecar
        .as_ref()
        .map(|s| provenance_of(&s.run_config_json))
        .unwrap_or_default();

    Some(CaptureInfo {
        name,
        frames,
        duration_s,
        n_channels,
        bytes,
        started_unix,
        complete,
        station_label,
        run_id,
    })
}

/// Pull `stationLabel` / `runId` out of a capture's opaque `run_config_json`, without
/// a schema: the field may not be there, and inventing a value would be worse than
/// leaving it blank.
fn provenance_of(run_config_json: &str) -> (String, String) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(run_config_json) else {
        return (String::new(), String::new());
    };
    let get = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    (get("stationLabel"), get("runId"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "hertzforge-backendd-caps-{}-{}",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_capture(
        root: &Path,
        name: &str,
        n_channels: usize,
        frames: usize,
        complete: bool,
        claimed_frames: u64,
    ) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let payload = vec![0u8; n_channels * frames * 4];
        std::fs::write(dir.join(hertzforge_capture::TELEMETRY_FILE), payload).unwrap();
        let sidecar = serde_json::json!({
            "config": {
                "n_channels": n_channels,
                "sample_rate_hz": 51200.0,
                "decimation": 1,
                "ring_capacity": 64
            },
            "channel_table": { "schema_version": 2, "channels": [], "reference": null },
            "start_unix_secs": 1_753_400_000u64,
            "run_config_json": "{\"stationLabel\":\"NI-SIM (driver-simulated)\",\"runId\":\"7\"}",
            "chunks_written": 1,
            "chunks_dropped": 0,
            "frames_written": claimed_frames,
            "complete": complete
        });
        std::fs::write(
            dir.join(hertzforge_capture::SIDECAR_FILE),
            serde_json::to_string(&sidecar).unwrap(),
        )
        .unwrap();
    }

    /// THE FILE IS THE AUTHORITY. The sidecar here CLAIMS 10 frames; the telemetry
    /// file holds 1000. The list reports 1000.
    ///
    /// RED-MUTATION: report `sidecar.frames_written` instead and this reds - which is
    /// the state where a capture killed between flushes is reported as almost empty
    /// and an operator concludes the run did not record.
    #[test]
    fn the_frame_count_comes_from_the_file_not_the_sidecar() {
        let root = tmp("authority");
        write_capture(&root, "rec_0001", 4, 1000, false, 10);

        let list = list(&root);
        assert_eq!(list.len(), 1);
        let c = &list[0];
        assert_eq!(c.frames, 1000, "the file, not the sidecar's claim of 10");
        assert_eq!(c.n_channels, 4);
        assert_eq!(c.bytes, 4 * 1000 * 4);
        assert!((c.duration_s - 1000.0 / 51200.0).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The three EVIDENCE fields survive: an incomplete driver-simulated capture is
    /// visibly both of those things.
    #[test]
    fn the_list_carries_completion_station_and_run_id() {
        let root = tmp("evidence");
        write_capture(&root, "rec_0001", 2, 100, false, 100);
        write_capture(&root, "rec_0002", 2, 100, true, 100);

        let list = list(&root);
        assert_eq!(list.len(), 2);
        assert!(!list[0].complete, "a killed capture is not complete");
        assert!(list[1].complete);
        for c in &list {
            assert_eq!(c.station_label, "NI-SIM (driver-simulated)");
            assert_eq!(c.run_id, "7");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A capture with NO sidecar is still listed, with honest zeros rather than
    /// invented values - and a directory with no telemetry at all is not a capture.
    #[test]
    fn a_sidecar_less_capture_is_listed_honestly_and_a_non_capture_is_not_listed() {
        let root = tmp("partial");
        let bare = root.join("rec_bare");
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::write(bare.join(hertzforge_capture::TELEMETRY_FILE), vec![0u8; 64]).unwrap();
        // Not a capture: the daemon's own config directory.
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(root.join("config").join("hardware.json"), "{}").unwrap();

        let list = list(&root);
        assert_eq!(
            list.len(),
            1,
            "only the telemetry-bearing directory: {list:?}"
        );
        let c = &list[0];
        assert_eq!(c.name, "rec_bare");
        assert_eq!(c.bytes, 64);
        assert_eq!(c.n_channels, 0, "unknown, stated as zero");
        assert_eq!(c.frames, 0);
        assert_eq!(c.duration_s, 0.0);
        assert!(!c.complete);
        assert_eq!(c.station_label, "");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An unreadable data root is an empty list, not an error a UI has to model.
    #[test]
    fn a_missing_data_root_lists_nothing_without_failing() {
        assert!(list(Path::new("/nonexistent/hertzforge/data/root")).is_empty());
    }
}
