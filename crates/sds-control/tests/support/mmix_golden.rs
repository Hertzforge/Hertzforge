//! The golden MIMO Mixed Mode run: its configuration, the run on a fresh service
//! and the folder normalized, for the golden run test.
#![allow(dead_code)]

use super::golden_runs::{files, lines_at, pretty, read_json, set};
use super::mmix::{self, controller};
use super::{serve, Client};
use sds_control::server::Service;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The most the golden folder may hold, bytes: twice the other modes' bound.
/// Even this short run keeps about 56 KB of text (the setup with its resolved
/// controller is 19 KB alone, and seventeen result descriptions), and its three
/// level histories each fill a whole record of 32 points.
pub const MOST_BYTES: usize = 128 * 1024;
/// The frames of each stream the golden folder keeps.
pub const CUT_FRAMES: u64 = 128;
/// The wall time every normalized folder starts at.
pub const START_UNIX: f64 = 1_794_240_000.25;
pub const UTC_OFFSET_S: i64 = -25_200;

pub fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schema/golden/runs/mimo_mixed")
}

/// A short run on the linear 4 by 4: 4096 Hz in frames of 512 points (8 Hz
/// lines), the floor from 112 to 224 Hz, a 1 g tone at 144 Hz and a 24 Hz band
/// at 192 Hz, the automatic schedule from -3 dB to three seconds at full level.
pub fn golden_config() -> Value {
    let mut cfg = mmix::floor_tones_band();
    cfg["estimation"]["fs_hz"] = json!(4096.0);
    cfg["estimation"]["n_pts"] = json!(512);
    // Two lines either side of a tone on the coarser grid.
    cfg["exclusion_hz"] = json!(16.0);
    let mut reference = mmix::reference(4, 1e-3);
    reference["freqs_hz"] = json!([112.0, 224.0]);
    cfg["reference"] = reference;
    let parts = cfg["parts"].as_array_mut().unwrap();
    parts.retain(|p| p["id"] != "t2");
    for p in parts.iter_mut() {
        match p["id"].as_str() {
            Some("t1") => p["body"]["schedule"]["hz"] = json!(144.0),
            Some("b1") => {
                p["body"]["schedule"]["hz"] = json!(192.0);
                p["body"]["width"]["hz"] = json!(24.0);
            }
            _ => {}
        }
    }
    cfg["schedule"] =
        json!({"mode": "auto", "start_db": -3.0, "table_db": [-3.0, 0.0], "dwell_s": [0.5]});
    cfg["full_level_duration_s"] = json!(3.0);
    // A tone law call every 50 ms and a tracker that settles in time for the
    // tone's law to be called at full level, so the law-call record holds both
    // laws in a folder that stays small.
    cfg["control_tick_s"] = json!(0.05);
    cfg["tracking_hz"] = json!(2.0);
    cfg["title"] = json!("Golden MIMO Mixed Mode");
    cfg["notes"] = json!("the golden run folder");
    cfg["specification"] = json!({"document": "Example procedure", "clause": "3.2"});
    cfg
}

/// Run the golden run on a fresh service; its folder once complete, and its
/// final state.
pub async fn golden_run() -> (PathBuf, Value) {
    let mut o = mmix::opts(Duration::from_secs(60));
    // Unpaced, the simulator outruns any disk: buffers for the whole run, so
    // no frame is dropped and the run is the same every time.
    o.record.stream_buffer_s = 600.0;
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok("mimo_mixed.configure", json!({"config": golden_config()}))
        .await;
    c.ok("mimo_mixed.start", json!({})).await;
    mmix::phase(&mut c, "awaiting_review").await;
    c.ok("mimo_mixed.proceed", json!({})).await;
    let end = mmix::state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "completed", "{}", end["end"]);
    let folder = mmix::folder(&end);
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
    (folder, end)
}

/// Each law call's `call_us` (a wall-clock figure) set to zero in every record of
/// `control_trace`.
fn zero_call_times(bin: Vec<u8>, side: &Value) -> Vec<u8> {
    let arrays = side["arrays"].as_array().unwrap();
    // The bytes of one element: the digits of its dtype (`f8`, `c16`).
    let width = |a: &Value| -> usize {
        let dtype = a["dtype"].as_str().unwrap();
        dtype
            .trim_start_matches(char::is_alphabetic)
            .parse()
            .unwrap()
    };
    let size = |a: &Value| -> usize {
        a["shape"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_u64().unwrap() as usize)
            .product::<usize>()
            * width(a)
    };
    let record: usize = arrays.iter().map(size).sum();
    let call = arrays.iter().find(|a| a["name"] == "call_us").unwrap();
    let (at, len) = (call["offset"].as_u64().unwrap() as usize, size(call));
    let mut out = bin;
    for r in (0..out.len()).step_by(record.max(1)) {
        out[r + at..r + at + len].fill(0);
    }
    out
}

/// A folder's files normalized: the wall times, the run's unique identity and the
/// build fixed, the laws' call times zeroed, each stream cut to its first
/// [`CUT_FRAMES`] frames and saying so.
pub fn normalized(folder: &Path, golden: bool) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut widths = BTreeMap::new();
    for rel in files(folder) {
        let name = rel.to_string_lossy().replace('\\', "/");
        if let Some(stream) = name
            .strip_prefix("streams/")
            .and_then(|s| s.strip_suffix("/stream.json"))
        {
            let v = read_json(&folder.join(&rel));
            widths.insert(
                stream.to_owned(),
                v["channels"].as_array().map_or(0, Vec::len),
            );
        }
    }
    for rel in files(folder) {
        let name = rel.to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(folder.join(&rel)).unwrap();
        let fixed = match name.as_str() {
            "run.json" => {
                let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                set(&mut v, &["run_uid"], json!("golden-mimo-mixed"));
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
                set(&mut v, &["run_uid"], json!("golden-mimo-mixed"));
                set(&mut v, &["at_unix"], json!(START_UNIX + 60.0));
                if !golden {
                    if let Some(streams) = v["streams"].as_object_mut() {
                        for s in streams.values_mut() {
                            let n = s["frames"].as_u64().unwrap_or(0);
                            s["frames"] = json!(n.min(CUT_FRAMES));
                        }
                    }
                }
                pretty(&v)
            }
            "log.jsonl" => lines_at(bytes, START_UNIX),
            "results/control_trace.bin" if !golden => {
                let side = read_json(&folder.join("results/control_trace.json"));
                zero_call_times(bytes, &side)
            }
            n if n.ends_with("/stream.json") && !golden => {
                let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                let frames = v["frames"].as_u64().unwrap();
                v["frames"] = json!(frames.min(CUT_FRAMES));
                v["cut_from_frames"] = json!(frames);
                pretty(&v)
            }
            n if n.ends_with("/telemetry.f32") && !golden => {
                let stream = n
                    .trim_start_matches("streams/")
                    .trim_end_matches("/telemetry.f32");
                let keep = CUT_FRAMES as usize * widths[stream] * 4;
                bytes[..keep.min(bytes.len())].to_vec()
            }
            _ => bytes,
        };
        out.insert(name, fixed);
    }
    out
}
