//! The golden Playback run folder, `schema/golden/runs/playback/`: a short run on the
//! simulator (one input, one drive, 2048 Hz), a half-second burst at -6 dB with an
//! eighth of a second of pre-roll and of post-roll, played to its end. The simulator's
//! seed is fixed and the drive file is made the same way every time; what follows the
//! wall clock is normalized: the wall times, the run's unique identity, the build
//! fields, the folders' paths, the station frames and the read-ahead's fill. The
//! streams are cut to their first [`CUT_FRAMES`] frames (`cut_from_frames`), which hold
//! the lead-in, the pre-roll and the burst.
//!
//! `scripts/make_golden_runs.sh` rewrites the folder (this test with
//! `HERTZFORGE_WRITE_GOLDEN_RUNS=1`); otherwise the test only compares.

mod support;

use hertzforge_playback::file::write_folder;
use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use support::golden_runs::{compare, files, pretty, read_json, set, write_golden};
use support::{options, serve, Client};

/// The most the golden folder may hold, bytes.
const MOST_BYTES: usize = 64 * 1024;
/// The frames of each stream the golden folder keeps.
const CUT_FRAMES: u64 = 1536;
/// The golden rate.
const FS: f64 = 2048.0;
const START_UNIX: f64 = 1_794_240_000.25;
const UTC_OFFSET_S: i64 = -25_200;
const STATION_FRAMES: u64 = 4096;

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schema/golden/runs/playback")
}

fn row(index: u32, role: &str, description: &str) -> Value {
    let drive = role == "drive";
    json!({
        "index": index, "enabled": true, "description": description, "role": role,
        "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": if drive { "V" } else { "g" },
        "sensitivity_mv_per_eu": if drive { 1000 } else { 100 },
        "range_max": if drive { 10 } else { 100 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": if drive { Value::Null } else { json!(50.0) },
                  "velocity_ips": null, "displacement_mil": null}
    })
}

/// The golden drive: a half-second burst whose acceleration, velocity and displacement
/// are zero at both ends, peak 0.4 V.
fn golden_drive(dir: &Path) -> (String, String) {
    let n = (FS * 0.5) as usize;
    let t_len = n as f64 / FS;
    let w = 2.0 * std::f64::consts::PI * 50.0;
    let p = std::f64::consts::PI / t_len;
    let a: Vec<f64> = (0..n)
        .map(|k| {
            let t = k as f64 / FS;
            let (s, c) = (w * t).sin_cos();
            2.0 * p * p * (2.0 * p * t).cos() * s + 2.0 * p * (2.0 * p * t).sin() * w * c
                - (p * t).sin().powi(2) * w * w * s
        })
        .collect();
    let m = a.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    let s: Vec<f32> = a.iter().map(|x| (x * 0.4 / m) as f32).collect();
    let mut extra = serde_json::Map::new();
    extra.insert(
        "source".into(),
        json!({"path": "burst.csv", "sha256": "0".repeat(64), "format": "csv",
               "selections": ["drive"]}),
    );
    extra.insert("tool".into(), json!("hertzforge convert"));
    extra.insert("created_utc".into(), json!("2026-11-15T00:00:00Z"));
    let folder = dir.join("drive");
    let sha = write_folder(&folder, FS, &["drive"], &s, extra).unwrap();
    (folder.display().to_string(), sha)
}

async fn golden_run() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sds-playback-golden-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let (folder, sha) = golden_drive(&dir);
    let scan = FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )));
    let o = options(scan, 4.0, Duration::from_secs(60), Duration::from_secs(5));
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    c.ok(
        "channels.set",
        json!({"revision": 1, "channels": [row(1, "control", "Control"), row(2, "drive", "Drive")]}),
    )
    .await;
    let config = json!({
        "title": "Golden Playback",
        "drive": {"folder": folder, "sha256": sha},
        "map": [2], "level_db": -6, "ceiling_v": 1.0,
        "pre_roll_s": 0.125, "post_roll_s": 0.125,
        "operator": "A. Operator", "article": "Bracket, SN 0042",
        "specification": {"document": "Test procedure", "clause": "4.2"},
        "metadata": [{"key": "run_number", "label": "Run Number", "value": "3"}],
    });
    c.ok("playback.configure", json!({"config": config})).await;
    c.ok("playback.start", json!({})).await;
    let t0 = Instant::now();
    let end = loop {
        let s = c.ok("playback.state", json!({})).await;
        if s["state"] == "finished" {
            break s;
        }
        assert!(t0.elapsed() < Duration::from_secs(30), "{s}");
        c.pump(Duration::from_millis(20)).await;
    };
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    let rec = PathBuf::from(end["record"]["folder"].as_str().unwrap());
    let t0 = Instant::now();
    while !(rec.join("end.json").is_file() && read_json(&rec.join("run.json"))["complete"] == true) {
        assert!(t0.elapsed() < Duration::from_secs(30), "{rec:?}");
        c.pump(Duration::from_millis(20)).await;
    }
    rec
}

fn normalize_state(v: &mut Value) {
    set(v, &["record", "folder"], json!("golden_playback"));
    set(v, &["progress", "buffered_s"], json!(0.0));
}

fn normalized(folder: &Path, golden: bool) -> BTreeMap<String, Vec<u8>> {
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
                set(&mut v, &["run_uid"], json!("golden-playback"));
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
            "setup.json" => {
                let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                set(&mut v, &["config", "drive", "folder"], json!("drive"));
                set(&mut v, &["resolved", "drive", "folder"], json!("drive"));
                pretty(&v)
            }
            "end.json" => {
                let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                set(&mut v, &["run_uid"], json!("golden-playback"));
                set(&mut v, &["at_unix"], json!(START_UNIX + 60.0));
                set(&mut v, &["station_frames"], json!(STATION_FRAMES));
                for s in ["inputs", "drives"] {
                    set(&mut v, &["streams", s, "frames"], json!(CUT_FRAMES));
                }
                if let Some(st) = v.get_mut("state") {
                    normalize_state(st);
                }
                pretty(&v)
            }
            "log.jsonl" => {
                let mut text = Vec::new();
                for (k, line) in String::from_utf8(bytes).unwrap().lines().enumerate() {
                    let mut v: Value = serde_json::from_str(line).unwrap();
                    set(&mut v, &["at_unix"], json!(START_UNIX + k as f64));
                    // The armed entry is made as the station delivers its first frames, so
                    // whether it has one is a race.
                    if v["data"]["event"] == "armed" {
                        set(&mut v, &["frame"], Value::Null);
                    } else if !v["frame"].is_null() {
                        set(&mut v, &["frame"], json!(STATION_FRAMES));
                    }
                    if v["text"].as_str().is_some_and(|t| t.starts_with("records in ")) {
                        set(&mut v, &["text"], json!("records in golden_playback"));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn todays_code_writes_the_golden_playback_run_folder() {
    let folder = golden_run().await;
    // The run itself, before any comparison: the burst at -6 dB on the drive, zero
    // before it and after it.
    let drive: Vec<f32> = std::fs::read(folder.join("streams/drives/telemetry.f32"))
        .unwrap()
        .chunks(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let top = drive.iter().fold(0f32, |m, x| m.max(x.abs()));
    assert!((f64::from(top) - 0.4 * 10f64.powf(-0.3)).abs() < 1e-4, "the drive's peak is {top}");
    assert!(drive[..256].iter().all(|x| *x == 0.0));
    assert_eq!(*drive.last().unwrap(), 0.0);
    let got = normalized(&folder, false);
    let total: usize = got.values().map(Vec::len).sum();
    assert!(total <= MOST_BYTES, "the golden folder holds {total} bytes, over {MOST_BYTES}");
    if std::env::var_os("HERTZFORGE_WRITE_GOLDEN_RUNS").is_some() {
        write_golden(&golden_dir(), &got);
        println!("wrote {} ({total} bytes)", golden_dir().display());
        return;
    }
    let want = normalized(&golden_dir(), true);
    compare(&got, &want);
}

/// Two runs of the golden setup give the same folder.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_golden_playback_run_repeats_exactly() {
    let a = normalized(&golden_run().await, false);
    let b = normalized(&golden_run().await, false);
    for (name, bytes) in &a {
        assert_eq!(Some(bytes), b.get(name), "{name} differs between two runs");
    }
    assert_eq!(a.len(), b.len());
}
