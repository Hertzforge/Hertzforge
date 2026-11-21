//! The MIMO Mixed Mode goldens in `schema/golden/`, built by this crate's own
//! builders: the golden configuration parses and checks to the golden derived
//! values; the state and the frames are encoded from the controller's status and
//! the display's spectra and histories (made here by hand, dyadic so exact in f32
//! and f64) to the golden bytes; the subscription parses to the golden result.
//! The refusals' goldens are the service's own words over the wire
//! (`mimo_mixed_protocol.rs`).
//!
//! `MIMO_MIXED_GOLDEN_WRITE=1` rewrites the frames from the builders; otherwise
//! the test only compares. Every frame is also held to `schema/protocol.schema.json`.

mod support;

use hertzforge_channel_table::{ChannelRole, ChannelTable, Units};
use hertzforge_mimo_mixed::service::{
    BandStatus, DriveStatus, LimitStatus, MimoMixedPhase, MimoMixedState, MimoMixedStatus,
    ToneStatus,
};
use sds_control::codec::{self, Arrays};
use sds_control::mimo_mixed::config;
use sds_control::mimo_mixed::subscribe;
use sds_control::mimo_mixed::view::{
    self, BandHistory, IdentView, MimoMixedView, RecordRef, Spectra, State, StationRef, ToneHistory,
};
use sds_control::protocol;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use support::schema::Schema;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden")
        .join(name)
}

fn writing() -> bool {
    std::env::var_os("MIMO_MIXED_GOLDEN_WRITE").is_some()
}

fn read_json(name: &str) -> Value {
    let text = std::fs::read_to_string(golden(name)).unwrap_or_else(|e| panic!("read {name}: {e}"));
    serde_json::from_str(&text).unwrap()
}

/// The goldens' own layout: one-space indent, a closing newline.
fn write_json(name: &str, v: &Value) {
    // Two-space pretty JSON with each line's indent halved (a string's own line
    // breaks are escaped, so every line break here is the layout's).
    let pretty = serde_json::to_string_pretty(v).unwrap();
    let mut out = String::new();
    for line in pretty.lines() {
        let body = line.trim_start_matches(' ');
        let indent = (line.len() - body.len()) / 2;
        out.push_str(&" ".repeat(indent));
        out.push_str(body);
        out.push('\n');
    }
    std::fs::write(golden(name), out).unwrap();
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

/// Controls 1 to 4, drives 5 to 8.
fn table() -> ChannelTable {
    let mut t = sds_control::table::Table::new().table;
    t.channels = (1..=8)
        .map(|i| {
            let mut r = sds_control::table::new_row(i);
            if i <= 4 {
                r.role = ChannelRole::Control;
                r.description = format!("Control {i}");
            } else {
                r.role = ChannelRole::Drive;
                r.description = format!("Shaker {}", i - 4);
                r.units = Units::VoltageV;
            }
            r
        })
        .collect();
    t
}

fn catalog_dir() -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "sds-control-mimo-mixed-golden-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// The golden request: a floor, two fixed tones and a band, by hand from -6 dB.
fn request_configure() -> Value {
    let cfg = support::mmix::manual(support::mmix::floor_tones_band(), -6.0);
    request(91, "mimo_mixed.configure", json!({"config": cfg}))
}

fn checked() -> (
    config::WireMimoMixed,
    sds_control::table::Report,
    config::Checked,
) {
    let request = read_json("request_mimo_mixed_configure.json");
    let w = config::parse(&request["params"]["config"]).unwrap();
    let dir = catalog_dir();
    let cat = hertzforge_random_laws::LawCatalog::open(&dir).unwrap();
    let (r, c) = config::check(&w, &table(), &cat);
    drop(cat);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    (w, r, c.expect("the golden configuration checks"))
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"type": "request", "id": id, "method": method, "params": params})
}

fn sim() -> StationRef {
    StationRef {
        kind: "sim",
        class: "sim",
        devices: Vec::new(),
    }
}

fn configured(run_id: &str) -> MimoMixedView {
    let (w, _, c) = checked();
    view::configured_view(
        run_id,
        &w.title,
        &c.core,
        &c.derived,
        sim(),
        c.core.reference.unit.clone(),
    )
}

/// The four lines the frames show: 100 to 112 Hz on the 4 Hz grid.
const BAND: (usize, usize) = (25, 29);

fn spectra() -> Spectra {
    let v = |a: [f64; 4]| a.to_vec();
    let per = |a: [f64; 4]| (0..4).map(|_| v(a)).collect::<Vec<_>>();
    Spectra {
        composite: per([0.0009765625, 0.5, 0.001953125, 0.0009765625]),
        residual: per([0.0009765625, 0.0009765625, 0.001953125, 0.0009765625]),
        cross: (0..3)
            .map(|i| {
                (
                    i,
                    i + 1,
                    v([0.00048828125, 0.25, 0.0009765625, 0.00048828125]),
                )
            })
            .collect(),
        reference: per([0.0009765625; 4]),
        alarm: (0..4)
            .map(|_| (v([0.00048828125; 4]), v([0.001953125; 4])))
            .collect(),
        abort: (0..4)
            .map(|_| (v([0.000244140625; 4]), v([0.00390625; 4])))
            .collect(),
        drive: per([0.0001220703125, 0.0625, 0.000244140625, 0.0001220703125]),
    }
}

/// A run at full level past an overridden gate: the floor equalized, both
/// tones and the band in, the second tone held by the shared ceiling and the
/// band by the drive ceilings, a limit channel 3.25 dB under its ceiling.
fn running() -> MimoMixedView {
    let mut v = configured("mimo-mixed-0002");
    v.state = State::Running;
    v.phase = view::phase_name(MimoMixedPhase::FullLevel);
    v.grid.actual = true;
    v.grid.band = BAND;
    v.level_db = Some(0.0);
    v.t_s = Some(96.25);
    let tone = |id: &str, hz: f64, limiting: bool, scale: f64| ToneStatus {
        id: id.into(),
        frequency_hz: hz,
        part_level: 1.0,
        engaged: true,
        usable: true,
        limiting,
        scale,
        error_db: vec![0.125, -0.25, 0.0625, 0.5],
        alarm: vec![false; 4],
        measured: vec![1.015625, 0.96875, 1.0078125, 1.0625],
        target: vec![1.0; 4],
    };
    v.status = Some(MimoMixedStatus {
        state: MimoMixedState::Running,
        phase: MimoMixedPhase::FullLevel,
        t_s: 96.25,
        level_db: 0.0,
        floor_error_db: vec![-0.25, 0.125, 0.5, -0.0625],
        floor_equalized: true,
        loops: 377,
        floor_misses: 0,
        tones: vec![
            tone("t1", 150.0, false, 1.0),
            tone("t2", 700.0, true, 0.875),
        ],
        bands: vec![BandStatus {
            id: "b1".into(),
            folded: false,
            center_hz: 300.0,
            part_level: 1.0,
            error_db: vec![0.25, -0.5, 0.125, 0.0],
            alarm: vec![false; 4],
            limiting: true,
        }],
        drives: (0..4)
            .map(|j| DriveStatus {
                random_rms_v: 0.5 + 0.125 * j as f64,
                peak_v: 2.25 + 0.25 * j as f64,
                tone_sum_v: 0.75,
                abort_vpk: 4.0,
            })
            .collect(),
        underruns: 0,
        dropped: 0,
        clamped: 0,
        limits: vec![LimitStatus {
            channel_index: 9,
            over_db: -3.25,
            at_hz: 152.0,
        }],
        end: None,
    });
    v.overrides.push(view::Override {
        by: "bench".into(),
        text: "Coherence low at the band edges; accepted.".into(),
        reason: "gate_failed",
    });
    v.held_peak_v = vec![2.5, 2.75, 3.0, 3.25];
    v.identification = Some(IdentView {
        level_db: -20.0,
        steps: 2,
        frames: 64,
        lag_frames: 265,
        band_coherence: vec![0.9937, 0.9943, 0.993, 0.9923],
        gate: 0.5,
        passed: true,
        drive_vrms: vec![0.01278; 4],
    });
    v.record = Some(RecordRef {
        folder: "/home/operator/Hertzforge_Data/20261121-140000_mimo_mixed_mimo-mixed-0002".into(),
        streams: true,
        problem: None,
    });
    v.streaming = true;
    v.spectra = Some(Arc::new(spectra()));
    let t = vec![95.75, 96.0, 96.25];
    v.tones = Arc::new(vec![ToneHistory {
        id: "t2".into(),
        t: t.clone(),
        freq_hz: vec![700.0; 3],
        amplitude: (0..4).map(|_| vec![0.875, 0.9375, 1.0]).collect(),
        phase_deg: (0..4).map(|i| vec![45.0 * i as f64; 3]).collect(),
        error_db: (0..4).map(|_| vec![-1.0, -0.5, 0.0]).collect(),
    }]);
    v.bands = Arc::new(vec![BandHistory {
        id: "b1".into(),
        t,
        error_db: (0..4).map(|_| vec![0.5, 0.25, 0.125]).collect(),
    }]);
    v
}

fn subscription() -> subscribe::Subscription {
    let r = read_json("request_mimo_mixed_subscribe.json");
    subscribe::parse(r["params"].as_object().unwrap())
        .unwrap()
        .unwrap()
}

// ---- the goldens -------------------------------------------------------------------

fn request_subscribe() -> Value {
    request(
        93,
        "mimo_mixed.subscribe",
        json!({"rate_hz": 2, "plots": [
            {"id": "ctl", "quantity": "control"},
            {"id": "res", "quantity": "residual", "channels": [1, 2]},
            {"id": "crs", "quantity": "cross"},
            {"id": "drv", "quantity": "drive"},
            {"id": "amp", "quantity": "tone_amplitude", "parts": ["t2"]},
            {"id": "phs", "quantity": "tone_phase", "parts": ["t2"]},
            {"id": "bnd", "quantity": "band_level"},
        ]}),
    )
}

fn response_configure() -> Value {
    let (_, r, c) = checked();
    let v = configured("mimo-mixed-0001");
    protocol::response(
        91,
        json!({
            "state": view::state_json(Some(&v)),
            "derived": c.derived.to_json(),
            "warnings": r.warnings.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
        }),
    )
}

fn text(name: &str) -> Value {
    match name {
        "request_mimo_mixed_configure" => request_configure(),
        "response_mimo_mixed_configure" => response_configure(),
        "event_mimo_mixed_state" => {
            protocol::event("mimo_mixed.state", 290, view::state_json(Some(&running())))
        }
        "request_mimo_mixed_level" => request(94, "mimo_mixed.level", json!({"delta_db": -3})),
        "request_mimo_mixed_part" => {
            request(95, "mimo_mixed.part", json!({"id": "t2", "on": false}))
        }
        "request_mimo_mixed_subscribe" => request_subscribe(),
        "response_mimo_mixed_subscribe" => {
            protocol::response(93, subscribe::result_json(Some(&subscription())))
        }
        other => panic!("no builder for {other}"),
    }
}

/// In order: the subscription's response reads its request.
const TEXT: [&str; 7] = [
    "request_mimo_mixed_configure",
    "request_mimo_mixed_subscribe",
    "request_mimo_mixed_level",
    "request_mimo_mixed_part",
    "response_mimo_mixed_configure",
    "event_mimo_mixed_state",
    "response_mimo_mixed_subscribe",
];

fn binary(name: &str) -> (Value, Arrays) {
    let mut arrays = Arrays::new();
    let v = running();
    let m = match name {
        "event_mimo_mixed_live" => {
            protocol::event("mimo_mixed.live", 291, view::live_json(&v, &mut arrays))
        }
        "event_mimo_mixed_data" => protocol::event(
            "mimo_mixed.data",
            292,
            subscribe::data_json(&v, &subscription(), &mut arrays),
        ),
        other => panic!("no builder for {other}"),
    };
    (m, arrays)
}

const BINARY: [&str; 2] = ["event_mimo_mixed_live", "event_mimo_mixed_data"];

fn write_all() {
    for name in TEXT {
        write_json(&format!("{name}.json"), &text(name));
    }
    for name in BINARY {
        let (m, a) = binary(name);
        let bytes = codec::encode(&m, &a).unwrap();
        std::fs::write(golden(&format!("{name}.bin")), &bytes).unwrap();
        let expanded = codec::decode(&bytes).unwrap().expanded();
        write_json(&format!("{name}.expected.json"), &expanded);
    }
}

#[test]
fn mimo_mixed_golden_every_text_golden_is_its_builders_output() {
    if writing() {
        write_all();
    }
    for name in TEXT {
        same(&text(name), &read_json(&format!("{name}.json")), name);
    }
}

#[test]
fn mimo_mixed_golden_encoding_reproduces_the_golden_bytes() {
    if writing() {
        return;
    }
    for name in BINARY {
        let (message, arrays) = binary(name);
        let got = codec::encode(&message, &arrays).unwrap();
        let want = std::fs::read(golden(&format!("{name}.bin"))).unwrap();
        assert!(
            got == want,
            "{name}: {} bytes encoded vs {} golden",
            got.len(),
            want.len()
        );
        let decoded = codec::decode(&want).unwrap();
        same(
            &decoded.expanded(),
            &read_json(&format!("{name}.expected.json")),
            name,
        );
        let order = codec::header_array_order(&want).unwrap();
        let order: Vec<&str> = order.iter().map(String::as_str).collect();
        assert_eq!(codec::reencode(&decoded, &order).unwrap(), want, "{name}");
    }
}

#[test]
fn mimo_mixed_golden_every_frame_follows_the_schema() {
    if writing() {
        return;
    }
    let schema = Schema::load();
    let mut names: Vec<String> = std::fs::read_dir(golden(""))
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.contains("mimo_mixed") && n.ends_with(".json"))
        .filter(|n| !n.ends_with(".expected.json") || n.starts_with("event_"))
        .collect();
    names.sort();
    assert!(names.len() >= 9, "{names:?}");
    for n in names {
        let v = read_json(&n);
        let kind = v["type"]
            .as_str()
            .unwrap_or_else(|| panic!("{n} has a type"));
        assert!(matches!(kind, "request" | "response" | "event"), "{n}");
        if let Some(m) = v["method"].as_str() {
            assert!(m.starts_with("mimo_mixed."), "{n}: {m}");
        }
        if !n.ends_with(".expected.json") {
            assert!(
                !serde_json::to_string(&v).unwrap().contains("$array"),
                "{n}"
            );
        }
        if let Err(e) = schema.check(&v, None) {
            panic!("{n} does not follow the schema:\n{}", e.join("\n"));
        }
    }
}

/// Every MIMO Mixed Mode golden's payload validates against its own def.
#[test]
fn mimo_mixed_golden_every_payload_validates_against_its_def() {
    if writing() {
        return;
    }
    let s = Schema::load();
    let text = [
        (
            "request_mimo_mixed_configure",
            "params",
            "mimoMixedConfigureParams",
        ),
        ("request_mimo_mixed_level", "params", "mimoMixedLevelParams"),
        ("request_mimo_mixed_mode", "params", "mimoMixedModeParams"),
        (
            "request_mimo_mixed_override",
            "params",
            "mimoOverrideParams",
        ),
        ("request_mimo_mixed_part", "params", "mimoMixedPartParams"),
        (
            "request_mimo_mixed_subscribe",
            "params",
            "mimoMixedSubscribeParams",
        ),
        (
            "response_mimo_mixed_configure",
            "result",
            "mimoMixedConfigureResult",
        ),
        (
            "response_mimo_mixed_subscribe",
            "result",
            "mimoMixedSubscribeResult",
        ),
        ("event_mimo_mixed_state", "data", "mimoMixedState"),
    ];
    let binary = [
        ("event_mimo_mixed_live", "mimoMixedLive"),
        ("event_mimo_mixed_data", "mimoMixedData"),
    ];
    for (name, at, def) in text {
        s.assert_valid(&read_json(&format!("{name}.json"))[at], Some(def));
    }
    for (name, def) in binary {
        let bytes = std::fs::read(golden(&format!("{name}.bin"))).unwrap();
        let m = codec::decode(&bytes).unwrap().message;
        s.assert_valid(&m["data"], Some(def));
    }
    // The unconfigured state, and a configured one before anything plays.
    s.assert_valid(&view::state_json(None), Some("mimoMixedState"));
    s.assert_valid(
        &view::state_json(Some(&configured("mimo-mixed-0001"))),
        Some("mimoMixedState"),
    );
}

#[test]
fn mimo_mixed_golden_the_verbs_parse() {
    for (name, method) in [
        ("request_mimo_mixed_level", "mimo_mixed.level"),
        ("request_mimo_mixed_mode", "mimo_mixed.mode"),
        ("request_mimo_mixed_override", "mimo_mixed.override"),
        ("request_mimo_mixed_part", "mimo_mixed.part"),
        ("request_mimo_mixed_subscribe", "mimo_mixed.subscribe"),
        ("request_mimo_mixed_configure", "mimo_mixed.configure"),
    ] {
        let r = protocol::parse_request(&read_json(&format!("{name}.json"))).unwrap();
        assert_eq!(r.method, method);
    }
}
