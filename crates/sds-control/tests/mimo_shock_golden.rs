//! The MIMO Shock goldens in `schema/golden/`, built by this crate's own builders:
//! a configuration parses and checks to the golden derived values and state, the
//! binary frames encode to the golden bytes exactly, the text frames are the
//! objects the service sends.

use hertzforge_channel_table::{ChannelRole, ChannelTable};
use sds_control::codec::{self, Arrays};
use sds_control::mimo_shock::config::{self, CheckContext};
use sds_control::mimo_shock::run::unreachable_message;
use sds_control::mimo_shock::subscribe::{self, DataContext};
use sds_control::mimo_shock::view::{
    self, ControlFigures, LawReading, MsView, PulseFigures, SrsFigures,
};
use sds_control::protocol;
use sds_control::random::view::State;
use sds_control::shock::view::StationRef;
use sds_control::table;
use serde_json::{json, Value};
use std::path::PathBuf;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden")
        .join(name)
}

fn read_json(name: &str) -> Value {
    let text = std::fs::read_to_string(golden(name)).unwrap_or_else(|e| panic!("read {name}: {e}"));
    serde_json::from_str(&text).unwrap()
}

fn read_bin(name: &str) -> Vec<u8> {
    std::fs::read(golden(name)).unwrap_or_else(|e| panic!("read {name}: {e}"))
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

fn bytes_equal(name: &str, got: &[u8], want: &[u8]) {
    if got != want {
        let at = got.iter().zip(want).position(|(a, b)| a != b);
        panic!(
            "{name}: {} bytes encoded vs {} golden; first difference at {at:?}\n got: {}\nwant: {}",
            got.len(),
            want.len(),
            String::from_utf8_lossy(&got[..got.len().min(600)]),
            String::from_utf8_lossy(&want[..want.len().min(600)]),
        );
    }
}

/// Controls 1 to 4, drives 5 to 8, as the golden configuration names them.
fn table4() -> ChannelTable {
    let mut t = table::Table::new().table;
    t.channels = (1..=8)
        .map(|i| {
            let mut c = table::new_row(i);
            c.role = if i <= 4 {
                ChannelRole::Control
            } else {
                ChannelRole::Drive
            };
            c
        })
        .collect();
    t
}

fn builtin(id: &str) -> Option<config::LawFacts> {
    let dir = std::env::temp_dir().join(format!(
        "sds-control-mimo-shock-golden-{}-{id}",
        std::process::id()
    ));
    let cat = hertzforge_random_laws::LawCatalog::open(&dir).unwrap();
    let f = sds_control::mimo_shock::laws::facts(&cat, id);
    let _ = std::fs::remove_dir_all(&dir);
    f
}

fn check(config: &Value) -> (table::Report, Option<config::Derived>) {
    let w = config::parse(config).unwrap();
    let t = table4();
    let ctx = CheckContext {
        table: &t,
        law: &builtin,
        timeout_s: 30.0,
    };
    w.check(&ctx)
}

fn sim_station() -> StationRef {
    StationRef {
        kind: "sim",
        class: "sim",
        devices: Vec::new(),
    }
}

/// A view from a golden state object (the per-run parts as the golden has them).
fn view_of(s: &Value) -> MsView {
    let leak =
        |x: &Value| -> &'static str { Box::leak(x.as_str().unwrap().to_owned().into_boxed_str()) };
    MsView {
        run_id: s["run_id"].as_str().unwrap().into(),
        title: s["title"].as_str().unwrap().into(),
        state: match s["state"].as_str().unwrap() {
            "configured" => State::Configured,
            "running" => State::Running,
            "review" => State::Review,
            other => panic!("{other}"),
        },
        phase: leak(&s["phase"]),
        station: sim_station(),
        plant: s["station"]["plant"].as_str().map(str::to_owned),
        fs_hz: s["grid"]["fs_hz"].as_f64().unwrap(),
        record_len: s["grid"]["record_len"].as_u64().unwrap() as usize,
        actual: s["grid"]["actual"].as_bool().unwrap(),
        clock: s["clock"].as_str().map(|_| "shared"),
        target: leak(&s["target"]),
        timing: leak(&s["timing"]),
        verdict: leak(&s["verdict"]),
        scaled_aborts: s["scaled_aborts"].as_bool().unwrap(),
        mode: hertzforge_shock_common::ShockRunMode::Auto,
        levels_db: s["levels_db"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_f64().unwrap())
            .collect(),
        level_db: s["level_db"].as_f64(),
        rung: s["rung"].as_u64().map(|x| x as usize),
        pulses: view::Counts {
            fired: s["pulses"]["fired"].as_u64().unwrap(),
            passed: s["pulses"]["passed"].as_u64().unwrap(),
            at_level: s["pulses"]["at_level"].as_u64().unwrap() as u32,
            of_level: s["pulses"]["of_level"].as_u64().unwrap() as u32,
        },
        next_pulse_s: s["next_pulse_s"].as_f64(),
        elapsed_s: s["elapsed_s"].as_f64(),
        identification: Some(s["identification"].clone()).filter(|v| !v.is_null()),
        review: Some(s["review"].clone()).filter(|v| !v.is_null()),
        last_pulse: Some(s["last_pulse"].clone()).filter(|v| !v.is_null()),
        law: view::LawRef {
            id: s["law"]["id"].as_str().unwrap().into(),
            name: s["law"]["name"].as_str().unwrap().into(),
            version: s["law"]["version"].as_str().unwrap().into(),
            sha256: String::new(),
            params: s["law"]["params"].clone(),
        },
        misses: 0,
        end: None,
        streaming: true,
    }
}

#[test]
fn mimo_shock_golden_the_configuration_checks_to_its_derived_values_and_state() {
    let req = read_json("request_mimo_shock_configure.json");
    let resp = read_json("response_mimo_shock_configure.json");
    let (report, derived) = check(&req["params"]["config"]);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let d = derived.expect("derived");
    same(&d.to_json(), &resp["result"]["derived"], "derived");
    assert!(report.warnings.is_empty());
    let w = config::parse(&req["params"]["config"]).unwrap();
    let facts = builtin("inverse").unwrap();
    let v = sds_control::mimo_shock::configured_view("ms-0001", &w, &d, sim_station(), &facts);
    same(
        &view::state_json(Some(&v)),
        &resp["result"]["state"],
        "state",
    );
}

#[test]
fn mimo_shock_golden_the_spectrum_configuration_checks_clean() {
    let req = read_json("request_mimo_shock_configure_spectrum.json");
    let (report, derived) = check(&req["params"]["config"]);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let d = derived.expect("derived");
    assert_eq!(d.record_len, 16384);
    assert_eq!(d.controls.len(), 4);
}

#[test]
fn mimo_shock_golden_the_invalid_configuration_names_its_items() {
    let want = read_json("response_error_mimo_shock_config_invalid.json");
    let mut cfg = read_json("request_mimo_shock_configure.json")["params"]["config"].clone();
    cfg["target"]["pulses"].as_array_mut().unwrap().pop();
    cfg["verdict"] = json!("global");
    let (report, derived) = check(&cfg);
    assert!(derived.is_none());
    let e = config::config_invalid(&report);
    let got = protocol::error_response(77, &e);
    same(&got, &want, "error");
}

#[test]
fn mimo_shock_golden_the_running_and_review_states_are_the_service_objects() {
    for name in [
        "event_mimo_shock_state.json",
        "event_mimo_shock_state_review.json",
        "event_mimo_shock_state_unscaled.json",
    ] {
        let g = read_json(name);
        let v = view_of(&g["data"]);
        same(&view::state_json(Some(&v)), &g["data"], name);
        same(
            &protocol::event(
                "mimo_shock.state",
                g["seq"].as_u64().unwrap(),
                view::state_json(Some(&v)),
            ),
            &g,
            name,
        );
    }
    let r = read_json("event_mimo_shock_state_review.json");
    let held = &r["data"]["review"];
    assert_eq!(
        held["hold"]["message"].as_str().unwrap(),
        unreachable_message(&[1, 3], &[4.2, 12.6])
    );
}

#[test]
fn mimo_shock_golden_the_unreachable_refusal_is_the_service_error() {
    let want = read_json("response_error_mimo_shock_unreachable.json");
    let review = read_json("event_mimo_shock_state_review.json")["data"]["review"].clone();
    let e = protocol::MethodError::named(
        protocol::ErrorCode::Refused,
        "unreachable_target",
        unreachable_message(&[1, 3], &[4.2, 12.6]),
    )
    .with_detail(json!({"controls": review["controls"]}));
    same(&protocol::error_response(74, &e), &want, "error");
}

#[test]
fn mimo_shock_golden_the_subscription_parses_to_its_result() {
    let req = read_json("request_mimo_shock_subscribe.json");
    let resp = read_json("response_mimo_shock_subscribe.json");
    let p = req["params"].as_object().unwrap();
    let sub = subscribe::parse(p).unwrap().unwrap();
    same(
        &subscribe::result_json(Some(&sub)),
        &resp["result"],
        "result",
    );
}

fn figs(ctl: &[u32], drives: &[u32], band: bool, srs: bool) -> PulseFigures {
    let target = vec![0.0, -0.5, 0.0, 7.5, 10.0, 7.5, 0.0, -0.5];
    PulseFigures {
        fs_hz: 1000.0,
        controls: ctl
            .iter()
            .map(|&c| ControlFigures {
                channel: c,
                unit: "g".into(),
                target: target.clone(),
                band: band.then(|| {
                    (
                        vec![1.5, 1.0, 1.5, 9.0, 11.5, 9.0, 1.5, 1.0],
                        vec![-1.5, -2.0, -1.5, 6.0, 8.5, 6.0, -1.5, -2.0],
                    )
                }),
                measured: vec![0.0, -0.25, 0.25, 7.25, 9.75, 7.75, 0.25, -0.5],
                srs: srs.then(|| SrsFigures {
                    freqs_hz: vec![100.0, 200.0, 400.0],
                    target: vec![12.5, 15.0, 11.0],
                    hi: vec![17.5, 21.0, 15.5],
                    lo: vec![9.0, 10.5, 7.75],
                    measured: vec![12.25, 15.5, 10.5],
                }),
            })
            .collect(),
        drives: drives
            .iter()
            .map(|&d| {
                (
                    d,
                    vec![0.0, -0.0125, 0.0, 0.1875, 0.25, 0.1875, 0.0, -0.0125],
                )
            })
            .collect(),
    }
}

fn encode(topic: &str, seq: u64, data: Value, arrays: &Arrays) -> Vec<u8> {
    codec::encode(&protocol::event(topic, seq, data), arrays).unwrap()
}

#[test]
fn mimo_shock_golden_the_pulse_frames_encode_to_the_golden_bytes() {
    let g = read_json("event_mimo_shock_pulse.expected.json");
    let mut arrays = Arrays::new();
    let views = view::pulse_views(
        &figs(&[1, 2, 3, 4], &[5, 6, 7, 8], true, false),
        None,
        &mut arrays,
    );
    let got = encode(
        "mimo_shock.pulse",
        211,
        json!({"run_id": "ms-0002", "pulse": g["data"]["pulse"], "views": views}),
        &arrays,
    );
    bytes_equal(
        "event_mimo_shock_pulse",
        &got,
        &read_bin("event_mimo_shock_pulse.bin"),
    );

    let g = read_json("event_mimo_shock_pulse_spectrum.expected.json");
    let mut arrays = Arrays::new();
    let views = view::pulse_views(&figs(&[1, 2], &[5, 6], false, true), None, &mut arrays);
    let got = encode(
        "mimo_shock.pulse",
        212,
        json!({"run_id": "ms-0004", "pulse": g["data"]["pulse"], "views": views}),
        &arrays,
    );
    bytes_equal(
        "event_mimo_shock_pulse_spectrum",
        &got,
        &read_bin("event_mimo_shock_pulse_spectrum.bin"),
    );
}

#[test]
fn mimo_shock_golden_the_live_and_data_frames_encode_to_the_golden_bytes() {
    let state = read_json("event_mimo_shock_state.json")["data"].clone();
    let v = view_of(&state);
    let got = encode("mimo_shock.live", 209, view::live_json(&v), &Arrays::new());
    bytes_equal(
        "event_mimo_shock_live",
        &got,
        &read_bin("event_mimo_shock_live.bin"),
    );

    let req = read_json("request_mimo_shock_subscribe.json");
    let sub = subscribe::parse(req["params"].as_object().unwrap())
        .unwrap()
        .unwrap();
    let f = figs(&[1, 2, 3, 4], &[5, 6, 7, 8], true, false);
    let rows: Vec<Value> = [
        (-12.0, true),
        (-9.0, false),
        (-9.0, true),
        (-6.0, true),
        (-6.0, false),
    ]
    .iter()
    .map(|(l, p)| json!({"level_db": l, "passed": p}))
    .collect();
    let law = vec![
        LawReading {
            label: "Drive rms at full level".into(),
            value: 0.5,
            unit: "V".into(),
        },
        LawReading {
            label: "Waveform error".into(),
            value: 0.125,
            unit: String::new(),
        },
    ];
    let ctx = DataContext {
        view: &v,
        last: Some(&f),
        identification: None,
        law: &law,
        rows: &rows,
    };
    let mut arrays = Arrays::new();
    let data = subscribe::data_json(&ctx, &sub, &mut arrays);
    let got = encode("mimo_shock.data", 213, data, &arrays);
    bytes_equal(
        "event_mimo_shock_data",
        &got,
        &read_bin("event_mimo_shock_data.bin"),
    );
}

#[path = "support/schema.rs"]
mod schema;

/// Every MIMO Shock golden validates against the protocol schema, as a message
/// and as its own payload.
#[test]
fn mimo_shock_golden_every_frame_validates_against_the_schema() {
    let s = schema::Schema::load();
    let text = [
        (
            "request_mimo_shock_configure",
            "params",
            Some("mimoShockConfigureParams"),
        ),
        (
            "request_mimo_shock_configure_spectrum",
            "params",
            Some("mimoShockConfigureParams"),
        ),
        (
            "response_mimo_shock_configure",
            "result",
            Some("mimoShockConfigureResult"),
        ),
        ("event_mimo_shock_state", "data", Some("mimoShockState")),
        (
            "event_mimo_shock_state_review",
            "data",
            Some("mimoShockState"),
        ),
        (
            "event_mimo_shock_state_unscaled",
            "data",
            Some("mimoShockState"),
        ),
        (
            "request_mimo_shock_proceed",
            "params",
            Some("mimoShockProceedParams"),
        ),
        (
            "request_mimo_shock_subscribe",
            "params",
            Some("mimoShockSubscribeParams"),
        ),
        (
            "response_mimo_shock_subscribe",
            "result",
            Some("mimoShockSubscribeResult"),
        ),
        ("request_mimo_shock_fire", "params", None),
        ("response_error_mimo_shock_unreachable", "error", None),
        ("response_error_mimo_shock_config_invalid", "error", None),
    ];
    let mut names: Vec<String> = std::fs::read_dir(golden(""))
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.contains("mimo_shock") && !n.ends_with(".expected.json"))
        .map(|n| n.split('.').next().unwrap().to_owned())
        .collect();
    names.sort();
    names.dedup();
    let binary = [
        ("event_mimo_shock_live", "mimoShockLive"),
        ("event_mimo_shock_pulse", "mimoShockPulseEvent"),
        ("event_mimo_shock_pulse_spectrum", "mimoShockPulseEvent"),
        ("event_mimo_shock_data", "mimoShockData"),
    ];
    let mut covered: Vec<String> = text
        .iter()
        .map(|t| t.0.to_owned())
        .chain(binary.iter().map(|b| b.0.to_owned()))
        .collect();
    covered.sort();
    assert_eq!(names, covered, "every MIMO Shock golden is checked here");
    for (name, at, def) in text {
        let m = read_json(&format!("{name}.json"));
        s.assert_valid(&m, None);
        if let Some(d) = def {
            s.assert_valid(&m[at], Some(d));
        }
    }
    for (name, def) in binary {
        let m = codec::decode(&read_bin(&format!("{name}.bin")))
            .unwrap()
            .message;
        s.assert_valid(&m, None);
        s.assert_valid(&m["data"], Some(def));
    }
}
