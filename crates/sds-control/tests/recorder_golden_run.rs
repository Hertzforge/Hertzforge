//! The golden Recorder run folder, `schema/golden/runs/recorder/`, against today's
//! code: a short Recorder run on the simulator, with the simulator's fixed seed and its
//! stated source, gives the golden folder once what depends on the wall clock is
//! normalized. One level-triggered recording with a pre-window, an overload mark inside
//! it, then a stop. The analysis side converts the same folder in
//! `tests/test_run_folder_golden_recorder.py`.
//!
//! What depends on when the stop arrived is normalized with the times: the frames
//! acquired, the end's station time, each row's statistics over the whole run and the
//! station frame a log entry was written at (a mark's own frame is kept). The recording
//! itself (its frames, its trigger, its pre-window, its mark) does not depend on it.
//! The folder holds no gap: on the simulator a gap needs a disk that falls behind,
//! which cannot be made the same every time; gaps are tested on the core's writer
//! (`hertzforge-recorder`'s tests) and on the reader (`tests/test_run_folder.py`).
//!
//! `scripts/make_golden_runs.sh` rewrites the folder (this test with
//! `HERTZFORGE_WRITE_GOLDEN_RUNS=1`); otherwise the test only compares.
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
use support::{options, serve, Client};

/// The most the golden folder may hold, bytes.
const MOST_BYTES: usize = 64 * 1024;

/// The wall time every normalized folder starts at.
const START_UNIX: f64 = 1_794_240_000.25;
const UTC_OFFSET_S: i64 = -25_200;
/// The frames acquired, as normalized.
const STATION_FRAMES: u64 = 2000;

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schema/golden/runs/recorder")
}

fn row(index: u32, role: &str, description: &str) -> Value {
    json!({
        "index": index, "enabled": true, "description": description, "role": role,
        "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": "g", "sensitivity_mv_per_eu": 100, "range_max": 0.5,
        "polarity": "positive", "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

/// Two recorded rows at 1000 Hz: the stated source's markers on row 1 and its 250 Hz
/// tone on row 2. Row 1's range of 0.5 g puts its overload level (0.45 g) under the
/// markers' 0.5 g peak, so the first marker latches it. A level trigger on row 1 at
/// 0.25 g, a 0.1 s pre-window and 0.2 s after the trigger, once.
fn golden_config() -> Value {
    json!({
        "title": "Golden Recorder",
        "operator": "Operator",
        "article": "Bracket",
        "notes": "the golden run folder",
        "specification": {"document": "Example procedure", "clause": "4.2"},
        "acquisition": {"fs_hz": 1000},
        "trigger": {"mode": "level", "row": 1, "level": 0.25, "slope": "rising",
                    "pre_s": 0.1, "record_s": 0.2, "count": 1, "holdoff_s": 0},
        "analysis": {"block": 256, "window": "hann", "averaging": "linear", "averages": 4,
                     "decimation": 1, "frf": {"reference": 1, "response": 2}},
        "capture": {"run_name": "golden_recorder", "keep_free_gb": 1},
        "metadata": [{"key": "run_number", "label": "Run Number", "value": "7"},
                     {"key": "sn", "label": "SN", "value": "0042"}]
    })
}

/// Run the golden run on a fresh service and return its folder once complete.
async fn golden_run() -> PathBuf {
    let scan = FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )));
    let o = options(scan, 1.0, Duration::from_secs(60), Duration::from_secs(5));
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rows = vec![row(1, "control", "Markers"), row(2, "monitor", "Tone")];
    c.ok("channels.set", json!({"revision": 1, "channels": rows}))
        .await;
    c.ok("recorder.configure", json!({"config": golden_config()}))
        .await;
    c.ok("recorder.start", json!({})).await;
    // Armed at once: the first marker is 0.5 s in, so the pre-window has filled.
    c.ok("recorder.arm", json!({})).await;
    let t0 = Instant::now();
    loop {
        let s = c.ok("recorder.state", json!({})).await;
        if s["recordings"] == 1 && s["phase"] == "monitoring" {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(30), "{s}");
        c.pump(Duration::from_millis(20)).await;
    }
    c.ok("recorder.stop", json!({})).await;
    let t0 = Instant::now();
    let end = loop {
        let s = c.ok("recorder.state", json!({})).await;
        if s["state"] == "finished" {
            break s;
        }
        assert!(t0.elapsed() < Duration::from_secs(30), "{s}");
        c.pump(Duration::from_millis(20)).await;
    };
    assert_eq!(end["end"]["outcome"], "operator_stop", "{end}");
    let folder = PathBuf::from(end["record"]["folder"].as_str().unwrap());
    // The finished state is published once the record has written its end.
    assert!(folder.join("end.json").is_file(), "{folder:?}");
    assert_eq!(read_json(&folder.join("run.json"))["complete"], true);
    folder
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if !p.file_name().unwrap().to_string_lossy().starts_with('.') {
                out.push(p.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    out.sort();
    out
}

fn set(v: &mut Value, path: &[&str], to: Value) {
    let mut at = v;
    for k in &path[..path.len() - 1] {
        match at.get_mut(*k) {
            Some(next) => at = next,
            None => return,
        }
    }
    if let Some(m) = at.as_object_mut() {
        if m.contains_key(path[path.len() - 1]) {
            m.insert(path[path.len() - 1].to_owned(), to);
        }
    }
}

fn pretty(v: &Value) -> Vec<u8> {
    let mut out = serde_json::to_vec_pretty(v).unwrap();
    out.push(b'\n');
    out
}

/// The free space as normalized.
const FREE_BYTES: u64 = 107_374_182_400;

/// A folder's files with what depends on the wall clock, the host, the build and when
/// the stop arrived replaced by fixed values.
fn normalized(folder: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for rel in files(folder) {
        let name = rel.to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(folder.join(&rel)).unwrap();
        let fixed = match name.as_str() {
            "run.json" => {
                let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                set(&mut v, &["run_uid"], json!("golden-recorder"));
                set(&mut v, &["start", "unix"], json!(START_UNIX));
                set(&mut v, &["start", "utc_offset_s"], json!(UTC_OFFSET_S));
                set(
                    &mut v,
                    &["build"],
                    json!({"version": "0.0.0", "commit": "0".repeat(40), "dirty": false}),
                );
                pretty(&v)
            }
            "setup.json" => {
                let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                set(
                    &mut v,
                    &["derived", "disk", "free_bytes"],
                    json!(FREE_BYTES),
                );
                let above = v["derived"]["disk"]["floor_bytes"]
                    .as_u64()
                    .zip(v["derived"]["disk"]["bytes_per_s"].as_u64())
                    .map(|(floor, rate)| (FREE_BYTES - floor) / rate.max(1));
                set(
                    &mut v,
                    &["derived", "disk", "seconds_above_floor"],
                    json!(above),
                );
                pretty(&v)
            }
            "end.json" => {
                let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                set(&mut v, &["run_uid"], json!("golden-recorder"));
                set(&mut v, &["at_unix"], json!(START_UNIX + 60.0));
                set(
                    &mut v,
                    &["end", "at_s"],
                    json!(STATION_FRAMES as f64 / 1000.0),
                );
                set(&mut v, &["station_frames"], json!(STATION_FRAMES));
                if let Some(rows) = v["rows"].as_array_mut() {
                    for r in rows {
                        for k in ["mean", "rms", "peak", "crest", "kurtosis"] {
                            set(r, &[k], json!(0.0));
                        }
                    }
                }
                pretty(&v)
            }
            "log.jsonl" => {
                let mut text = Vec::new();
                for (k, line) in String::from_utf8(bytes).unwrap().lines().enumerate() {
                    let mut v: Value = serde_json::from_str(line).unwrap();
                    set(&mut v, &["at_unix"], json!(START_UNIX + k as f64));
                    // A mark's frame is its own; any other entry's is when it was
                    // written, which follows the wall clock.
                    if v.get("data").is_none() && !v["frame"].is_null() {
                        set(&mut v, &["frame"], json!(STATION_FRAMES));
                    }
                    // The folder's path is the host's.
                    if let Some(t) = v["text"].as_str() {
                        if t.starts_with("records in ") {
                            set(&mut v, &["text"], json!("records in golden_recorder"));
                        }
                    }
                    text.extend(serde_json::to_vec(&v).unwrap());
                    text.push(b'\n');
                }
                text
            }
            n if n.ends_with("/stream.json") => {
                let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                set(
                    &mut v,
                    &["writer", "free_space_at_start_bytes"],
                    json!(FREE_BYTES),
                );
                pretty(&v)
            }
            n if n.ends_with("/capture.json") => {
                let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                set(
                    &mut v,
                    &["start_unix_secs"],
                    json!(START_UNIX.floor() as u64),
                );
                set(&mut v, &["free_space_at_start_bytes"], json!(FREE_BYTES));
                pretty(&v)
            }
            _ => bytes,
        };
        out.insert(name, fixed);
    }
    out
}

/// Numbers within a part in a million (or 1e-9 near zero).
fn close(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= 1e-6 * a.abs().max(b.abs()) + 1e-9 || (a.is_nan() && b.is_nan())
}

fn same_json(a: &Value, b: &Value, path: &str) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => assert!(
            close(x.as_f64().unwrap(), y.as_f64().unwrap()),
            "{path}: {x} against the golden {y}"
        ),
        (Value::Object(x), Value::Object(y)) => {
            let mut kx: Vec<&String> = x.keys().collect();
            let mut ky: Vec<&String> = y.keys().collect();
            kx.sort();
            ky.sort();
            assert_eq!(kx, ky, "{path}: members differ");
            for (k, v) in x {
                same_json(v, &y[k], &format!("{path}.{k}"));
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            assert_eq!(x.len(), y.len(), "{path}: lengths differ");
            for (i, (u, v)) in x.iter().zip(y).enumerate() {
                same_json(u, v, &format!("{path}[{i}]"));
            }
        }
        _ => assert_eq!(a, b, "{path}: differs"),
    }
}

fn same_samples(name: &str, got: &[u8], want: &[u8]) {
    assert_eq!(
        got.len(),
        want.len(),
        "{name}: {} bytes against the golden {}",
        got.len(),
        want.len()
    );
    for (k, (a, b)) in got.chunks(4).zip(want.chunks(4)).enumerate() {
        let x = f64::from(f32::from_le_bytes(a.try_into().unwrap()));
        let y = f64::from(f32::from_le_bytes(b.try_into().unwrap()));
        assert!(
            x == y || (x - y).abs() <= 1e-5 * x.abs().max(y.abs()) + 1e-9,
            "{name}: value {k} is {x}, the golden {y}"
        );
    }
}

fn compare(got: &BTreeMap<String, Vec<u8>>, want: &BTreeMap<String, Vec<u8>>) {
    let a: Vec<&String> = got.keys().collect();
    let b: Vec<&String> = want.keys().collect();
    assert_eq!(a, b, "the folder's files differ from the golden folder's");
    for (name, bytes) in got {
        let gold = &want[name];
        if name.ends_with(".json") {
            same_json(
                &serde_json::from_slice(bytes).unwrap(),
                &serde_json::from_slice(gold).unwrap(),
                name,
            );
        } else if name.ends_with(".jsonl") {
            let (x, y) = (
                String::from_utf8(bytes.clone()).unwrap(),
                String::from_utf8(gold.clone()).unwrap(),
            );
            let (x, y): (Vec<&str>, Vec<&str>) = (x.lines().collect(), y.lines().collect());
            assert_eq!(x.len(), y.len(), "{name}: lines differ\n{x:#?}\n{y:#?}");
            for (k, (u, v)) in x.iter().zip(&y).enumerate() {
                same_json(
                    &serde_json::from_str(u).unwrap(),
                    &serde_json::from_str(v).unwrap(),
                    &format!("{name}:{}", k + 1),
                );
            }
        } else if name.ends_with(".f32") {
            same_samples(name, bytes, gold);
        } else {
            assert_eq!(bytes, gold, "{name} differs");
        }
    }
}

fn write_golden(files: &BTreeMap<String, Vec<u8>>) {
    let dir = golden_dir();
    let _ = std::fs::remove_dir_all(&dir);
    for (name, bytes) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn todays_code_writes_the_golden_recorder_run_folder() {
    let folder = golden_run().await;
    let got = normalized(&folder);
    let total: usize = got.values().map(Vec::len).sum();
    assert!(
        total <= MOST_BYTES,
        "the golden folder holds {total} bytes, over {MOST_BYTES}"
    );
    // The recording, before any comparison: a pre-window of 0.1 s and 0.2 s after
    // the trigger at the first marker's crossing, its overload mark inside it.
    let side = read_json(&folder.join("streams/rec_0001/stream.json"));
    assert_eq!(side["kind"], "recording");
    assert_eq!(side["pre_frames"], 100, "{side}");
    assert_eq!(side["frames"], 300, "{side}");
    assert_eq!(
        side["trigger_station_frame"].as_u64().unwrap()
            - side["first_station_frame"].as_u64().unwrap(),
        100
    );
    assert_eq!(side["marks"].as_array().unwrap().len(), 1, "{side}");
    assert_eq!(side["marks"][0]["kind"], "overload");
    if std::env::var_os("HERTZFORGE_WRITE_GOLDEN_RUNS").is_some() {
        write_golden(&got);
        println!("wrote {} ({total} bytes)", golden_dir().display());
        return;
    }
    let want = normalized(&golden_dir());
    compare(&got, &want);
}

/// The run is the same run every time: two runs of the golden setup give the same
/// folder, so the golden folder is a fact about the code, not about one run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_golden_recorder_run_repeats_exactly() {
    let a = normalized(&golden_run().await);
    let b = normalized(&golden_run().await);
    for (name, bytes) in &a {
        assert_eq!(Some(bytes), b.get(name), "{name} differs between two runs");
    }
    assert_eq!(a.len(), b.len());
}
