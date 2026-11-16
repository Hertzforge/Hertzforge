//! The Playback goldens in `schema/golden/`, against this crate's own code: the
//! configuration parses, the state and the live frame are this crate's view builders'
//! own output for one moment of one run (the live frame byte for byte), and a peak over
//! the ceiling is refused as the golden error.

use hertzforge_playback::file::write_folder;
use hertzforge_playback::plan::{refusals, DriveRow, StationOffer};
use hertzforge_playback::{check_file, DriveFile};
use sds_control::codec;
use sds_control::playback::config;
use sds_control::playback::run::{self, Display, PlaybackView, RecordRef, Row, State};
use sds_control::protocol;
use serde_json::Value;
use std::collections::VecDeque;
use std::path::PathBuf;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden")
        .join(name)
}

fn read_json(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(golden(name)).unwrap()).unwrap()
}

/// Structural equality with numbers compared by value and keys in order.
fn same(a: &Value, b: &Value, path: &str) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            assert_eq!(x.as_f64(), y.as_f64(), "number differs at {path}")
        }
        (Value::Object(x), Value::Object(y)) => {
            let kx: Vec<&String> = x.keys().collect();
            let ky: Vec<&String> = y.keys().collect();
            assert_eq!(kx, ky, "keys differ at {path}");
            for (k, v) in x {
                same(v, &y[k], &format!("{path}.{k}"));
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            assert_eq!(x.len(), y.len(), "lengths differ at {path}");
            for (i, (u, v)) in x.iter().zip(y).enumerate() {
                same(u, v, &format!("{path}[{i}]"));
            }
        }
        _ => assert_eq!(a, b, "values differ at {path}"),
    }
}

fn running_view() -> PlaybackView {
    PlaybackView {
        run_id: "playback-0002".into(),
        state: State::Running,
        phase: "playing",
        title: "Golden Playback".into(),
        source: "SIM",
        station_class: "sim",
        fs_hz: 2048.0,
        samples: 1024,
        sha256: "f666797717864a172ae04b75aba761c0a6745264592de24a0241f9bd22819f02".into(),
        level_db: -6.0,
        ceiling_v: 1.0,
        pre_roll_frames: 256,
        post_roll_frames: 256,
        played: 512,
        generated: 768,
        buffered_frames: 512,
        inputs: vec![Row {
            index: 1,
            role: "input",
            unit: "g".into(),
            description: "Control".into(),
        }],
        drives: vec![Row {
            index: 2,
            role: "drive",
            unit: "V".into(),
            description: "Drive".into(),
        }],
        map: vec![2],
        overloads: vec![false],
        record: Some(RecordRef {
            folder: "/home/operator/Hertzforge_Data/playback".into(),
            streams: true,
            problem: None,
        }),
        end: None,
    }
}

#[test]
fn the_golden_configuration_parses() {
    let request = read_json("request_playback_configure.json");
    let parsed = protocol::parse_request(&request).unwrap();
    assert_eq!(parsed.method, "playback.configure");
    let w = config::parse(&request["params"]["config"]).unwrap();
    assert_eq!(w.map, vec![2]);
    assert_eq!(w.level_db, -6.0);
    assert_eq!(w.pre_roll_s, 0.125);
    assert_eq!(w.run_name, "playback");
    assert_eq!(w.sha256.len(), 64);
    // a key the configuration does not have is refused by its path
    let mut bad = request["params"]["config"].clone();
    bad["level_v"] = Value::from(1);
    let e = config::parse(&bad).unwrap_err();
    assert!(e.message.contains("config.level_v"), "{}", e.message);
    for name in ["request_playback_start.json", "request_playback_log.json"] {
        assert!(protocol::parse_request(&read_json(name)).is_ok(), "{name}");
    }
}

#[test]
fn the_state_is_the_golden_state() {
    let golden = read_json("event_playback_state.json");
    same(
        &run::state_json(Some(&running_view())),
        &golden["data"],
        "state",
    );
    assert_eq!(
        run::state_json(None),
        serde_json::json!({"run_id": null, "state": "unconfigured"})
    );
}

#[test]
fn the_live_frame_is_the_golden_frame_byte_for_byte() {
    let want = std::fs::read(golden("event_playback_live.bin")).unwrap();
    let expected = read_json("event_playback_live.expected.json");
    let d = Display {
        rate_hz: 2048.0,
        first_s: 0.125,
        taken: 0,
        channels: vec![
            VecDeque::from(vec![0.0, 0.25, -0.5, 0.125, 0.0625, -0.25]),
            VecDeque::from(vec![0.0, 0.125, -0.25, 0.0625, 0.03125, -0.125]),
        ],
    };
    let mut arrays = codec::Arrays::new();
    let data = run::live_json(&running_view(), &d, &mut arrays);
    let frame = codec::encode(&protocol::event("playback.live", 301, data), &arrays).unwrap();
    assert_eq!(frame, want, "the live frame differs from the golden");
    let decoded = codec::decode(&want).unwrap();
    assert_eq!(decoded.message["topic"], expected["topic"]);
    assert_eq!(decoded.message["data"]["time"]["rate_hz"], 2048);
}

#[test]
fn a_peak_over_the_ceiling_is_refused_as_the_golden_error() {
    let dir = std::env::temp_dir().join(format!("sds-playback-golden-err-{}", std::process::id()));
    let folder = dir.join("drive");
    // a short burst, 2 V at its peak
    let n = 512usize;
    let s: Vec<f64> = (0..n)
        .map(|k| {
            let t = k as f64 / n as f64;
            let h = (std::f64::consts::PI * t).sin().powi(2);
            2.0 * h * (2.0 * std::f64::consts::PI * 8.0 * t).sin()
        })
        .collect();
    let peak = s.iter().fold(0f64, |m, x| m.max(x.abs()));
    let s: Vec<f64> = s.iter().map(|x| x * 2.0 / peak).collect();
    write_folder(&folder, 2048.0, &["drive"], &s, serde_json::Map::new()).unwrap();
    let file = DriveFile::open(&folder).unwrap();
    let check = check_file(&file, None).unwrap();
    let setup = hertzforge_playback::PlaySetup {
        level_db: -6.0,
        ceiling_v: 1.0,
        pre_roll_s: 0.125,
        post_roll_s: 0.125,
        map: vec![2],
    };
    let rows = [DriveRow {
        index: 2,
        range_max: 10.0,
    }];
    let offer = StationOffer {
        rate_hz: None,
        nearest_hz: Vec::new(),
        outputs: 1,
    };
    let r: Vec<_> = refusals(&file, &check, &setup, &rows, &offer)
        .into_iter()
        .filter(|r| r.reason == "peak_over_ceiling")
        .collect();
    let report = sds_control::table::Report {
        errors: config::items(&r),
        warnings: Vec::new(),
    };
    let e = config::config_invalid(&report, None);
    let got = protocol::error_response(134, &e);
    same(
        &got,
        &read_json("response_error_playback_config_invalid.json"),
        "error",
    );
    let _ = std::fs::remove_dir_all(&dir);
}
