//! The codec against the shared golden frames in `schema/golden/`.
//!
//! Three checks per binary golden: messages built here encode to the golden bytes
//! exactly; decoding the golden bytes matches `*.expected.json`; and decoding then
//! re-encoding reproduces the golden bytes.

use sds_control::codec::{self, Array, ArrayData, Arrays, Frame};
use sds_control::views::{self, Axis, Role, Row, Scale, Series, Status};
use serde_json::{json, Value};
use std::path::PathBuf;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden")
        .join(name)
}

fn read_bin(name: &str) -> Vec<u8> {
    std::fs::read(golden(name)).unwrap_or_else(|e| panic!("read {name}: {e}"))
}

fn read_json(name: &str) -> Value {
    let text = std::fs::read_to_string(golden(name)).unwrap_or_else(|e| panic!("read {name}: {e}"));
    serde_json::from_str(&text).unwrap()
}

/// Structural equality with numbers compared by value (Python writes `20.0` where
/// a decoded `u32` is `20`; both are the number twenty) and object keys compared in
/// order.
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

/// `response_spectrum`, built with this crate's view builders (the builders assign
/// `a0`..`a3` in series order, then `counts` takes `a4`).
fn response_spectrum() -> (Value, Arrays) {
    let mut arrays = Arrays::new();
    let spectrum = views::spectrum(
        &mut arrays,
        "PSD Against Specification",
        &Axis::new("Frequency", Some("Hz"), Scale::Log),
        &Axis::new("PSD", Some("g²/Hz"), Scale::Log),
        vec![
            Series::new(
                "Control",
                Role::Measured,
                vec![20.0, 80.0, 350.0, 1000.0, 2000.0],
                vec![0.0101, 0.0398, 0.0403, 0.0396, 0.0099],
            ),
            Series::new(
                "Specification",
                Role::Reference,
                vec![20.0, 80.0, 1000.0, 2000.0],
                vec![0.01, 0.04, 0.04, 0.01],
            ),
        ],
        Some("frequency"),
    );
    let reading = views::reading(
        "Overall",
        vec![
            Row::number("Control rms", 6.06, Some("g"), Status::Pass),
            Row::text("Lines in tolerance", "all", Status::Info),
        ],
    );
    let counts = arrays.push(Array::vector(ArrayData::U32(vec![3, 1, 4])));
    let message = json!({
        "type": "response", "id": 7,
        "result": {"views": [spectrum, reading], "counts": counts},
    });
    (message, arrays)
}

/// `event_shape`, built by hand with named ids.
fn event_shape() -> (Value, Arrays) {
    let mut arrays = Arrays::new();
    let n = arrays.insert(
        "n",
        Array::new(
            vec![4, 3],
            ArrayData::F32(vec![
                0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0,
            ]),
        )
        .unwrap(),
    );
    let t = arrays.insert(
        "t",
        Array::new(vec![2, 3], ArrayData::U32(vec![0, 1, 2, 0, 2, 3])).unwrap(),
    );
    let s = arrays.insert(
        "s",
        Array::new(
            vec![4, 3],
            ArrayData::F32(vec![
                0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 1.0, 0.0, 0.0, 0.25,
            ]),
        )
        .unwrap(),
    );
    let message = json!({
        "type": "event", "topic": "demo.shape", "seq": 42,
        "data": {"views": [
            {"view": "shape3d", "title": "Two Triangles", "unit": "m",
             "nodes": n, "triangles": t,
             "modes": [{"index": 1, "frequency_hz": 12.5, "damping": null, "shape": s}]},
        ]},
    });
    (message, arrays)
}

/// `event_random_live`, built with this crate's view builders: every series
/// stepped on the run's lines, ids `a0`.. in traversal order. Whole numbers in
/// the data are integers, as the golden writes them.
fn event_random_live() -> (Value, Arrays) {
    let lines = vec![100.0, 106.25, 112.5, 118.75];
    let flat = |v: f64| vec![v; 4];
    let mut arrays = Arrays::new();
    let freq = Axis::new("Frequency", Some("Hz"), Scale::Log);
    let control = views::spectrum(
        &mut arrays,
        "Control",
        &freq,
        &Axis::new("PSD", Some("g²/Hz"), Scale::Log),
        vec![
            Series::new(
                "Control",
                Role::Measured,
                lines.clone(),
                vec![0.0398, 0.0402, 0.0411, 0.0389],
            )
            .stepped(),
            Series::new("Reference", Role::Reference, lines.clone(), flat(0.04)).stepped(),
            Series::new("Alarm high", Role::Tolerance, lines.clone(), flat(0.0798)).stepped(),
            Series::new("Alarm low", Role::Tolerance, lines.clone(), flat(0.02005)).stepped(),
            Series::new("Abort high", Role::Abort, lines.clone(), flat(0.1592)).stepped(),
            Series::new("Abort low", Role::Abort, lines.clone(), flat(0.01005)).stepped(),
        ],
        Some("frequency"),
    );
    let drive = views::spectrum(
        &mut arrays,
        "Drive",
        &freq,
        &Axis::new("PSD", Some("V²/Hz"), Scale::Log),
        vec![Series::new(
            "Drive",
            Role::Drive,
            lines.clone(),
            vec![0.00021, 0.00025, 0.00031, 0.00028],
        )
        .stepped()],
        Some("frequency"),
    );
    let reading = views::reading(
        "Run",
        vec![
            Row::number("Control rms", 6.02, Some("g"), Status::Pass),
            Row::number("Reference rms", 6.06, Some("g"), Status::Info),
            Row::number("Drive rms", 0.412, Some("V"), Status::Info),
            Row::text("Level", "full level", Status::Info),
            Row::text("Lines in alarm", "2 of 317", Status::Warn),
            Row::number("Elapsed", 63.5, Some("s"), Status::Info),
        ],
    );
    let message = json!({
        "type": "event", "topic": "random.live", "seq": 318,
        "data": {
            "run_id": "r-0007", "source": "HARDWARE", "class": "ni_real",
            "state": "running", "phase": "full_level", "mode": "auto",
            "elapsed_s": 63.5, "level_db": 0, "at_level_s": 12.5,
            "remaining_at_level_s": 287.5, "loop": 118,
            "control_rms_g": 6.02, "reference_rms_g": 6.06, "drive_rms_v": 0.412,
            "lines": {"in_band": 317, "alarm": 2, "abort": 0},
            "channels": [
                {"index": 1, "description": "Control", "role": "control", "rms": 6.02,
                 "unit": "g", "overload": false},
                {"index": 2, "description": "Fixture corner", "role": "monitor", "rms": 7.35,
                 "unit": "g", "overload": false},
            ],
            "alarms": [{"kind": "envelope", "lines": 2, "worst_db": 3.4}],
            "views": [control, drive, reading],
        },
    });
    (message, arrays)
}

/// The text goldens of the hardware and Random control messages.
const CONTROL_TEXT: [&str; 9] = [
    "request_random_configure",
    "response_random_configure",
    "response_station_list",
    "response_channels_get",
    "event_random_state",
    "event_session_changed",
    "event_random_log",
    "response_error_controller_held",
    "response_error_table_invalid",
];

#[test]
fn encoding_reproduces_the_golden_bytes() {
    for (name, (message, arrays)) in [
        ("response_spectrum", response_spectrum()),
        ("event_shape", event_shape()),
        ("event_random_live", event_random_live()),
    ] {
        let want = read_bin(&format!("{name}.bin"));
        let got = codec::encode(&message, &arrays).unwrap();
        if got != want {
            let at = got.iter().zip(&want).position(|(a, b)| a != b);
            panic!(
                "{name}: {} bytes encoded vs {} golden; first difference at {at:?}\n got: {}\nwant: {}",
                got.len(),
                want.len(),
                String::from_utf8_lossy(&got[..got.len().min(200)]),
                String::from_utf8_lossy(&want[..want.len().min(200)]),
            );
        }
    }
}

#[test]
fn decoding_matches_the_expected_json() {
    for name in ["response_spectrum", "event_shape", "event_random_live"] {
        let decoded = codec::decode(&read_bin(&format!("{name}.bin"))).unwrap();
        assert!(
            decoded.message.get("arrays").is_none(),
            "{name}: arrays list dropped"
        );
        same(
            &decoded.expanded(),
            &read_json(&format!("{name}.expected.json")),
            name,
        );
    }
    // Typed, not just numerically equal: dtypes and shapes survive.
    let d = codec::decode(&read_bin("event_shape.bin")).unwrap();
    assert_eq!(d.arrays["n"].shape, vec![4, 3]);
    assert!(matches!(d.arrays["n"].data, ArrayData::F32(_)));
    assert!(matches!(d.arrays["t"].data, ArrayData::U32(_)));
    let d = codec::decode(&read_bin("response_spectrum.bin")).unwrap();
    let unit = &d.message["result"]["views"][0]["y"]["unit"];
    assert_eq!(unit, "g²/Hz", "UTF-8 survives the round trip");
    let x = d
        .array(&d.message["result"]["views"][0]["series"][0]["x"])
        .unwrap();
    assert_eq!(x.to_f64(), vec![20.0, 80.0, 350.0, 1000.0, 2000.0]);
}

#[test]
fn decode_then_encode_round_trips_byte_for_byte() {
    for name in ["response_spectrum", "event_shape", "event_random_live"] {
        let bytes = read_bin(&format!("{name}.bin"));
        let order = codec::header_array_order(&bytes).unwrap();
        let order: Vec<&str> = order.iter().map(String::as_str).collect();
        let decoded = codec::decode(&bytes).unwrap();
        assert_eq!(codec::reencode(&decoded, &order).unwrap(), bytes, "{name}");
    }
}

#[test]
fn the_golden_text_frames_parse_and_re_serialize() {
    for name in ["hello.json", "response_error.json"] {
        let text = std::fs::read_to_string(golden(name)).unwrap();
        let message = codec::decode_text(&text).unwrap();
        // A text frame on the wire is the compact form of the same message.
        match codec::encode_frame(&message, &Arrays::new()).unwrap() {
            Frame::Text(t) => same(&serde_json::from_str(&t).unwrap(), &message, name),
            Frame::Binary(_) => panic!("{name}: a message without arrays must be a text frame"),
        }
    }
    let hello = read_json("hello.json");
    let ours = sds_control::protocol::hello();
    let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(keys(&ours), keys(&hello), "hello keys in contract order");
    assert_eq!(ours["service"], "control");
    assert_eq!(ours["protocol"], hello["protocol"]);
}

#[test]
fn the_random_live_frame_carries_stepped_series_on_shared_lines() {
    let d = codec::decode(&read_bin("event_random_live.bin")).unwrap();
    let views = d.message["data"]["views"].as_array().unwrap();
    assert_eq!(views.len(), 3);
    let roles: Vec<&str> = views[0]["series"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        [
            "measured",
            "reference",
            "tolerance",
            "tolerance",
            "abort",
            "abort"
        ]
    );
    let lines = d.array(&views[0]["series"][0]["x"]).unwrap().to_f64();
    for view in &views[..2] {
        for s in view["series"].as_array().unwrap() {
            assert_eq!(s["shape"], "steps");
            assert_eq!(d.array(&s["x"]).unwrap().to_f64(), lines);
            let edges = d.array(&s["edges"]).unwrap().to_f64();
            assert_eq!(
                edges,
                views::bin_edges(&lines),
                "edges are the lines' own bins"
            );
        }
    }
    assert_eq!(d.message["data"]["source"], "HARDWARE");
    assert_eq!(d.message["data"]["class"], "ni_real");
}

#[test]
fn the_control_text_goldens_parse_and_re_serialize_compactly() {
    for name in CONTROL_TEXT {
        let text = std::fs::read_to_string(golden(&format!("{name}.json"))).unwrap();
        let message = codec::decode_text(&text).unwrap();
        match codec::encode_frame(&message, &Arrays::new()).unwrap() {
            Frame::Text(t) => {
                assert_eq!(
                    t,
                    serde_json::to_string(&message).unwrap(),
                    "{name}: compact"
                );
                same(&serde_json::from_str(&t).unwrap(), &message, name);
            }
            Frame::Binary(_) => panic!("{name}: a message without arrays must be a text frame"),
        }
    }
}

#[test]
fn the_configure_request_parses_and_refusals_carry_a_reason() {
    let request = read_json("request_random_configure.json");
    let parsed = sds_control::protocol::parse_request(&request).unwrap();
    assert_eq!(parsed.method, "random.configure");
    assert_eq!(
        parsed.params["config"]["acquisition"]["n_pts"],
        json!(2048),
        "params arrive whole"
    );
    for name in [
        "response_error_controller_held",
        "response_error_table_invalid",
    ] {
        let error = &read_json(&format!("{name}.json"))["error"];
        let reason = error["reason"].as_str().unwrap();
        assert!(
            !reason.is_empty() && reason.bytes().all(|b| b == b'_' || b.is_ascii_lowercase()),
            "{name}: reason {reason:?} is a snake_case name"
        );
        assert!(error["detail"].is_object(), "{name}: detail");
    }
}
