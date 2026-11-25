//! The golden MIMO Shock run folder, `schema/golden/runs/mimo_shock/`, against
//! today's code: a short automatic SRS run on the simulator's linear 4 by 4 plant,
//! unpaced, with the simulator's fixed seed, gives the golden folder once its wall
//! times, build fields and each law call's measured duration are normalized, and
//! each shot of its streams cut to its first [`CUT_FRAMES`] frames, its
//! `stream.json` and the end's counts saying so (`cut_from_frames`, the frames the
//! run recorded, on the stream and on each shot). The cut is the golden folder's
//! alone: the identification's shot is many records long. The analysis side
//! converts the same folder in `tests/test_run_folder_golden_mimo_shock.py`.
//!
//! `scripts/make_golden_runs.sh` rewrites the folder (this test with
//! `HERTZFORGE_WRITE_GOLDEN_RUNS=1`); otherwise the test only compares, within the
//! tolerances the Random golden run uses.
//!
//! The run is SIMULATED; nothing here is evidence about hardware.

mod support;

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

/// The frames each shot keeps in the golden folder.
const CUT_FRAMES: u64 = 80;

/// The wall time every normalized folder starts at.
const START_UNIX: f64 = 1_794_240_000.25;
const UTC_OFFSET_S: i64 = -25_200;

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schema/golden/runs/mimo_shock")
}

fn row(index: u32, role: &str, description: &str) -> Value {
    let drive = role == "drive";
    json!({
        "index": index, "enabled": true, "description": description, "role": role,
        "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": if drive { "V" } else { "g" },
        "sensitivity_mv_per_eu": 1000,
        "range_max": if drive { 10 } else { 1000 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

/// The table: four controls, then four drives.
pub fn golden_rows() -> Vec<Value> {
    (1..=4)
        .map(|i| row(i, "control", &format!("Control {i}")))
        .chain((5..=8).map(|i| row(i, "drive", &format!("Drive {}", i - 4))))
        .collect()
}

/// A short run: a flat 2 g SRS at four controls, 1250 to 1600 Hz at 1/3 octave, on
/// records of 64 samples at 4096 Hz, two levels and one passing pulse at full
/// level, the default law.
pub fn golden_config() -> Value {
    json!({
        "title": "Golden MIMO Shock",
        "operator": "Operator",
        "article": "Bracket",
        "notes": "the golden run folder",
        "specification": {"document": "Example procedure", "clause": "4.2"},
        "channels": {
            "controls": (1..=4).map(|i| json!({"channel": i})).collect::<Vec<_>>(),
            "drives": (5..=8)
                .map(|i| json!({"channel": i, "ceiling_vpk": 6, "abort_vpk": 9}))
                .collect::<Vec<_>>(),
            "limits": [],
        },
        "target": {"kind": "srs",
                   "bands": {"f_lo_hz": 1250, "f_hi_hz": 1600, "bands_per_octave": 3, "q": 10},
                   "tolerance_db": [-9, 9],
                   "controls": (0..4).map(|_| json!({
                       "target": {"kind": "flat", "accel_g": 2}})).collect::<Vec<_>>()},
        "verdict": "global",
        "acquisition": {"fs_hz": 4096, "record_len": 64, "band_hz": [700, 2000],
                        "lead_s": 0.002},
        "schedule": {"mode": "auto", "levels_db": [-6, 0], "max_pulses_per_level": 3,
                     "full_level_pulses": 1},
        "spacing": {"cooling_s": 0.05, "settling_s": 0, "compute_s": 0},
        "identification": {"drive_vrms": 1, "frames": 40, "review": false},
        "control": {"law_budget": 1},
    })
}

/// Run the golden run on a fresh service and return its folder once complete.
async fn golden_run() -> PathBuf {
    let scan = FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )));
    let mut o = options(scan, 0.0, Duration::from_secs(60), Duration::from_secs(5));
    o.mimo_driver = None;
    // Unpaced, the simulator outruns any disk: buffers for the whole run, so no
    // frame is dropped and the run is the same every time.
    o.record.stream_buffer_s = 120.0;
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    c.ok(
        "channels.set",
        json!({"revision": 1, "channels": golden_rows()}),
    )
    .await;
    c.ok("mimo_shock.configure", json!({"config": golden_config()}))
        .await;
    c.ok("mimo_shock.start", json!({})).await;
    let i = c
        .wait_for("the run's end", Duration::from_secs(120), |e| {
            e.topic() == "mimo_shock.state" && e.data()["state"] == "finished"
        })
        .await;
    let end = c.events[i].data().clone();
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    let folder = PathBuf::from(end["record"]["folder"].as_str().unwrap());
    let t0 = Instant::now();
    while !(folder.join("end.json").is_file()
        && read_json(&folder.join("run.json"))["complete"] == true)
    {
        assert!(
            t0.elapsed() < Duration::from_secs(60),
            "the record did not complete"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    folder
}

/// Each line of a JSON-lines file with its wall times fixed: `at_unix` to
/// `start + line`, and the times a run measures from the wall clock (a pulse's
/// start from arm, a law call's duration, the end's time).
fn lines_fixed(bytes: Vec<u8>) -> Vec<u8> {
    let mut text = Vec::new();
    for (k, line) in String::from_utf8(bytes).unwrap().lines().enumerate() {
        let mut v: Value = serde_json::from_str(line).unwrap();
        set(&mut v, &["at_unix"], json!(START_UNIX + k as f64));
        set(&mut v, &["at_s"], json!(k as f64));
        set(&mut v, &["call_ms"], json!(0.0));
        set(&mut v, &["data", "end", "at_s"], json!(30.0));
        text.extend(serde_json::to_vec(&v).unwrap());
        text.push(b'\n');
    }
    text
}

/// A folder's files with what depends on the wall clock, the host and the build
/// replaced by fixed values, and each shot cut to its first [`CUT_FRAMES`] frames.
fn normalized(folder: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for rel in files(folder) {
        let name = rel.to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(folder.join(&rel)).unwrap();
        let fixed = match name.as_str() {
            "run.json" => {
                let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                set(&mut v, &["run_uid"], json!("golden-mimo-shock"));
                set(&mut v, &["armed", "unix"], json!(START_UNIX));
                set(&mut v, &["armed", "utc_offset_s"], json!(UTC_OFFSET_S));
                set(&mut v, &["start", "unix"], json!(START_UNIX));
                set(&mut v, &["start", "utc_offset_s"], json!(UTC_OFFSET_S));
                set(
                    &mut v,
                    &["build"],
                    json!({"version": "0.0.0", "commit": "0".repeat(40), "dirty": false}),
                );
                pretty(&v)
            }
            "end.json" => {
                let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                set(&mut v, &["run_uid"], json!("golden-mimo-shock"));
                set(&mut v, &["at_unix"], json!(START_UNIX + 60.0));
                set(&mut v, &["end", "at_s"], json!(30.0));
                set(&mut v, &["summary", "elapsed_s"], json!(30.0));
                for s in ["inputs", "drives"] {
                    if let Some(f) = v["streams"][s]["frames"].as_u64() {
                        if v["streams"][s].get("cut_from_frames").is_none() {
                            let side = read_json(&folder.join(format!("streams/{s}/stream.json")));
                            let kept: u64 = side["shots"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|shot| shot["frames"].as_u64().unwrap().min(CUT_FRAMES))
                                .sum();
                            v["streams"][s]["frames"] = json!(kept);
                            v["streams"][s]["cut_from_frames"] = json!(f);
                        }
                    }
                }
                pretty(&v)
            }
            n if n.ends_with(".jsonl") => lines_fixed(bytes),
            "streams/inputs/telemetry.f32" | "streams/drives/telemetry.f32" => {
                let side = read_json(&folder.join(name.replace("telemetry.f32", "stream.json")));
                if side.get("cut_from_frames").is_some() {
                    bytes
                } else {
                    let width = side["channels"].as_array().map_or(1, Vec::len) * 4;
                    let mut out = Vec::new();
                    for shot in side["shots"].as_array().unwrap() {
                        let at = shot["first_frame"].as_u64().unwrap() as usize * width;
                        let n = shot["frames"].as_u64().unwrap().min(CUT_FRAMES) as usize * width;
                        out.extend_from_slice(
                            &bytes[at.min(bytes.len())..(at + n).min(bytes.len())],
                        );
                    }
                    out
                }
            }
            "streams/inputs/stream.json" | "streams/drives/stream.json" => {
                let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                if v.get("cut_from_frames").is_none() {
                    let frames = v["frames"].as_u64().unwrap();
                    let mut at = 0;
                    for shot in v["shots"].as_array_mut().unwrap() {
                        let n = shot["frames"].as_u64().unwrap();
                        shot["cut_from_frames"] = json!(n);
                        shot["frames"] = json!(n.min(CUT_FRAMES));
                        shot["first_frame"] = json!(at);
                        at += n.min(CUT_FRAMES);
                    }
                    v["frames"] = json!(at);
                    v["cut_from_frames"] = json!(frames);
                }
                pretty(&v)
            }
            _ => bytes,
        };
        out.insert(name, fixed);
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn todays_code_writes_the_golden_mimo_shock_run_folder() {
    let folder = golden_run().await;
    let got = normalized(&folder);
    let total: usize = got.values().map(Vec::len).sum();
    assert!(
        total <= MOST_BYTES,
        "the golden folder holds {total} bytes, over {MOST_BYTES}: {:?}",
        got.iter().map(|(k, v)| (k, v.len())).collect::<Vec<_>>()
    );
    if std::env::var_os("HERTZFORGE_WRITE_GOLDEN_RUNS").is_some() {
        write_golden(&golden_dir(), &got);
        println!("wrote {} ({total} bytes)", golden_dir().display());
        return;
    }
    let want = normalized(&golden_dir());
    compare(&got, &want);
}

/// The run is the same run every time: two runs of the golden setup give the same
/// folder, so the golden folder is a fact about the code, not about one run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_golden_mimo_shock_run_repeats_exactly() {
    let a = normalized(&golden_run().await);
    let b = normalized(&golden_run().await);
    for (name, bytes) in &a {
        assert_eq!(Some(bytes), b.get(name), "{name} differs between two runs");
    }
    assert_eq!(a.len(), b.len());
}
