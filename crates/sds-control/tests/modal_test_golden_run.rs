//! The golden Modal Test run folders, `schema/golden/runs/modal_test/`: on the
//! simulator, at 1024 Hz with a 512-frame block,
//!
//! - `impact_roving`: a roving hammer over three points, two hits a point, the force
//!   and exponential windows on; and
//! - `shaker_burst_random`: two shakers driven with burst random at one point, matrix
//!   H1 over four frames.
//!
//! The simulator's hits fall on a fixed grid of its frames and its noise is a fixed
//! function of the frame, so each folder repeats. The shaker run is at real time, so
//! its drive start lands within the first block's half second and the ramp begins on
//! the second block every time. What follows the wall clock is
//! normalized: the wall times, the run's unique identity, the build fields, the
//! folders' paths, and the frames the station had delivered when the run ended. Each
//! stream is kept whole to [`CUT_FRAMES`] frames (`cut_from_frames` gives the frames
//! it held).
//!
//! `scripts/make_golden_runs.sh` rewrites the folders (these tests with
//! `HERTZFORGE_WRITE_GOLDEN_RUNS=1`); otherwise the tests only compare.

mod support;

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use support::golden_runs::{compare, files, pretty, read_json, set, write_golden};
use support::modal::{base_options, record_done, rig_with, until};
use support::Client;

/// The most a golden folder may hold, bytes.
const MOST_BYTES: usize = 64 * 1024;
/// The frames of each stream a golden folder keeps.
const CUT_FRAMES: u64 = 1024;
const START_UNIX: f64 = 1_794_240_000.25;
const UTC_OFFSET_S: i64 = -25_200;
const STATION_FRAMES: u64 = 16384;

fn golden_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden/runs/modal_test")
        .join(name)
}

fn impact_config() -> Value {
    json!({
        "title": "Golden Roving Hammer", "operator": "A. Operator",
        "article": "Beam, SN 0007",
        "specification": {"document": "Test procedure", "clause": "5.1"},
        "metadata": [{"key": "run_number", "label": "Run Number", "value": "1"}],
        "mode": "impact", "fs_hz": 1024, "block": 512, "band": {"hi_hz": 200},
        "averages": 2,
        "roving": {"channels": [1], "points": {"start": 1, "count": 3}},
        "impact": {"holdoff_blocks": 0, "exponential_window": {"on": true, "end_pct": 10}},
    })
}

fn shaker_config() -> Value {
    json!({
        "title": "Golden Two Shakers", "operator": "A. Operator",
        "article": "Beam, SN 0007",
        "mode": "shaker", "fs_hz": 1024, "block": 512, "band": {"hi_hz": 200},
        "averages": 4,
        "shaker": {"drives": [{"row": 5, "level_vrms": 0.05}, {"row": 6, "level_vrms": 0.05}],
                   "source": "burst_random", "burst_pct": 40, "ramp_s": 0.5,
                   "ceiling_v": 1.0, "seed": 7},
    })
}

async fn finish(c: &mut Client) -> PathBuf {
    c.ok("modal_test.end", json!({})).await;
    let fin = until(c, Duration::from_secs(30), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
    assert_eq!(fin["end"]["outcome"], "ended", "{fin}");
    let folder = PathBuf::from(fin["record"]["folder"].as_str().unwrap());
    record_done(c, &folder).await;
    folder
}

async fn impact_run() -> PathBuf {
    let mut c = rig_with(false, base_options(2.0)).await;
    c.ok("modal_test.configure", json!({"config": impact_config()}))
        .await;
    c.ok("modal_test.start", json!({})).await;
    // Armed before the first hit, half a second of the simulator's time after the start.
    c.ok("modal_test.arm", json!({})).await;
    until(&mut c, Duration::from_secs(60), "three points", |s| {
        s["counts"]["totals"]["points_measured"] == 3
    })
    .await;
    finish(&mut c).await
}

async fn shaker_run() -> PathBuf {
    let mut c = rig_with(true, base_options(1.0)).await;
    c.ok("modal_test.configure", json!({"config": shaker_config()}))
        .await;
    c.ok("modal_test.start", json!({})).await;
    c.ok("modal_test.drive_start", json!({})).await;
    until(&mut c, Duration::from_secs(60), "the point", |s| {
        s["counts"]["totals"]["points_measured"] == 1 && s["drive_phase"] == "idle"
    })
    .await;
    finish(&mut c).await
}

fn normalized(folder: &Path, uid: &str, golden: bool) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut widths = BTreeMap::new();
    for name in ["inputs", "drives"] {
        let side = folder.join("streams").join(name).join("stream.json");
        if side.is_file() {
            let n = read_json(&side)["channels"].as_array().unwrap().len();
            widths.insert(name.to_owned(), n);
        }
    }
    for rel in files(folder) {
        let name = rel.to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(folder.join(&rel)).unwrap();
        let fixed = match name.as_str() {
            "run.json" => {
                let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                set(&mut v, &["run_uid"], json!(uid));
                for k in ["armed", "start"] {
                    set(&mut v, &[k, "unix"], json!(START_UNIX));
                    set(&mut v, &[k, "utc_offset_s"], json!(UTC_OFFSET_S));
                }
                set(
                    &mut v,
                    &["build"],
                    json!({"version": "0.0.0", "commit": "0".repeat(40), "dirty": false}),
                );
                pretty(&v)
            }
            "end.json" => {
                let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                set(&mut v, &["run_uid"], json!(uid));
                set(&mut v, &["at_unix"], json!(START_UNIX + 60.0));
                set(&mut v, &["station_frames"], json!(STATION_FRAMES));
                for s in ["inputs", "drives"] {
                    set(&mut v, &["streams", s, "frames"], json!(CUT_FRAMES));
                }
                set(
                    &mut v,
                    &["end", "at_s"],
                    json!(STATION_FRAMES as f64 / 1024.0),
                );
                set(
                    &mut v,
                    &["state", "end", "at_s"],
                    json!(STATION_FRAMES as f64 / 1024.0),
                );
                set(&mut v, &["state", "record", "folder"], json!(uid));
                pretty(&v)
            }
            "log.jsonl" => {
                let mut text = Vec::new();
                for (k, line) in String::from_utf8(bytes).unwrap().lines().enumerate() {
                    let mut v: Value = serde_json::from_str(line).unwrap();
                    set(&mut v, &["at_unix"], json!(START_UNIX + k as f64));
                    if !v["frame"].is_null() {
                        set(&mut v, &["frame"], json!(STATION_FRAMES));
                    }
                    if v["text"]
                        .as_str()
                        .is_some_and(|t| t.starts_with("records in "))
                    {
                        set(&mut v, &["text"], json!(format!("records in {uid}")));
                    }
                    text.extend(serde_json::to_vec(&v).unwrap());
                    text.push(b'\n');
                }
                text
            }
            n if n.ends_with("/stream.json") => {
                let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                v["frames"] = json!(CUT_FRAMES);
                v["cut_from_frames"] = json!(STATION_FRAMES);
                pretty(&v)
            }
            n if n.ends_with("/telemetry.f32") && !golden => {
                let stream = n
                    .trim_start_matches("streams/")
                    .trim_end_matches("/telemetry.f32");
                let keep = CUT_FRAMES as usize * widths[stream] * 4;
                assert!(bytes.len() >= keep, "{n} holds {} bytes", bytes.len());
                bytes[..keep].to_vec()
            }
            _ => bytes,
        };
        out.insert(name, fixed);
    }
    out
}

fn check(name: &str, got: &BTreeMap<String, Vec<u8>>) {
    let total: usize = got.values().map(Vec::len).sum();
    assert!(
        total <= MOST_BYTES,
        "the golden folder holds {total} bytes, over {MOST_BYTES}"
    );
    if std::env::var_os("HERTZFORGE_WRITE_GOLDEN_RUNS").is_some() {
        write_golden(&golden_dir(name), got);
        println!("wrote {} ({total} bytes)", golden_dir(name).display());
        return;
    }
    let want = normalized(&golden_dir(name), &format!("golden-{name}"), true);
    compare(got, &want);
}

fn done(folder: &Path) {
    let _ = std::fs::remove_dir_all(folder.parent().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_todays_code_writes_the_golden_impact_run_folder() {
    let folder = impact_run().await;
    let got = normalized(&folder, "golden-impact_roving", false);
    done(&folder);
    check("impact_roving", &got);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_todays_code_writes_the_golden_shaker_run_folder() {
    let folder = shaker_run().await;
    let got = normalized(&folder, "golden-shaker_burst_random", false);
    done(&folder);
    check("shaker_burst_random", &got);
}

/// Two runs of each golden setup give the same folder.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_the_golden_runs_repeat_exactly() {
    for shaker in [false, true] {
        let mut runs = Vec::new();
        for _ in 0..2 {
            let f = if shaker {
                shaker_run().await
            } else {
                impact_run().await
            };
            runs.push(normalized(&f, "golden", false));
            done(&f);
        }
        for (name, bytes) in &runs[0] {
            assert_eq!(
                Some(bytes),
                runs[1].get(name),
                "{name} differs between two runs (shaker {shaker})"
            );
        }
        assert_eq!(runs[0].len(), runs[1].len());
    }
}
