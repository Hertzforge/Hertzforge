//! The Explorer goldens in `schema/golden/`, against this crate's own code where it
//! exists: the configuration parses and checks to the golden derived values on the NI
//! station at 12 800 Hz, a configuration with errors is refused as the golden error, the
//! subscription parses to the golden result, the binary frames decode to their expected
//! JSON and re-encode byte for byte, and the text frames are compact text frames.
//!
//! The state, live, data and survey goldens are the contract the run's views are built
//! to; their builders are checked against them byte for byte where they are written.

use hertzforge_channel_table::{ChannelRole, ChannelTable, Units};
use sds_control::codec::{self, Arrays, Frame};
use sds_control::explorer::config::{self, CheckContext, StationFigures};
use sds_control::explorer::subscribe;
use sds_control::{protocol, table};
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

/// A control row (1) in g, a drive row (2) on `drive_range` V, a monitor row (3).
fn table(drive_range: f64) -> ChannelTable {
    let mut t = table::Table::new().table;
    let mut c = table::new_row(1);
    c.role = ChannelRole::Control;
    c.description = "Control".into();
    let mut d = table::new_row(2);
    d.role = ChannelRole::Drive;
    d.units = Units::VoltageV;
    d.range_max = drive_range;
    let mut m = table::new_row(3);
    m.role = ChannelRole::Monitor;
    m.description = "Fixture".into();
    t.channels = vec![c, d, m];
    t
}

fn golden_config() -> Value {
    let request = read_json("request_explorer_configure.json");
    let parsed = protocol::parse_request(&request).unwrap();
    assert_eq!(parsed.method, "explorer.configure");
    parsed.params["config"].clone()
}

fn check(config: &Value, drive_range: f64) -> (table::Report, Option<config::Derived>) {
    let w = config::parse(config).unwrap();
    w.check(&CheckContext {
        table: &table(drive_range),
        timeout_s: sds_control::session::TIMEOUT.as_secs_f64(),
        station: StationFigures::Ni,
    })
}

#[test]
fn explorer_the_configuration_checks_to_the_golden_derived_values() {
    let (report, derived) = check(&golden_config(), 10.0);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let want = read_json("response_explorer_configure.json");
    same(
        &derived.unwrap().to_json(),
        &want["result"]["derived"],
        "derived",
    );
    same(
        &Value::Array(report.warnings.iter().map(|i| i.to_json()).collect()),
        &want["result"]["warnings"],
        "warnings",
    );
    // The configured state states the derived grid and pipeline.
    let state = &want["result"]["state"];
    let d = check(&golden_config(), 10.0).1.unwrap();
    assert_eq!(state["pipeline_frames"], json!(d.pipeline_frames));
    assert_eq!(state["grid"]["block_frames"], json!(d.block_frames));
    assert_eq!(state["grid"]["lookahead_frames"], json!(d.lookahead_frames));
    assert_eq!(state["max_drive_vpk"], golden_config()["max_drive_vpk"]);
    assert_eq!(state["slew"], golden_config()["slew"]);
}

#[test]
fn explorer_a_configuration_with_errors_is_refused_as_the_golden_error() {
    let mut config = golden_config();
    config["band_hz"] = json!([20, 6000]);
    config["max_drive_vpk"] = json!(1.5);
    config["slew"]["drive_db_per_s"] = json!(1);
    let (report, derived) = check(&config, 1.0);
    assert!(derived.is_none());
    let e = config::config_invalid(&report);
    let want = read_json("response_error_explorer_config_invalid.json");
    same(&protocol::error_response(99, &e), &want, "error");
}

#[test]
fn explorer_the_subscription_parses_to_the_golden_result() {
    let request = read_json("request_explorer_subscribe.json");
    let parsed = protocol::parse_request(&request).unwrap();
    assert_eq!(parsed.method, "explorer.subscribe");
    let sub = subscribe::parse(&parsed.params).unwrap().unwrap();
    same(
        &subscribe::result_json(Some(&sub)),
        &read_json("response_explorer_subscribe.json")["result"],
        "subscribe result",
    );
    // The default subscription.
    let d = subscribe::parse(&serde_json::from_value(json!({"default": true})).unwrap()).unwrap();
    assert!(d.is_none());
    assert_eq!(subscribe::result_json(None)["default"], json!(true));
}

#[test]
fn explorer_a_subscription_refuses_a_key_a_quantity_does_not_take() {
    let refused = |plots: Value, param: &str| {
        let params = serde_json::from_value(json!({"plots": plots})).unwrap();
        let e = subscribe::parse(&params).expect_err(param);
        assert_eq!(e.reason, Some("bad_params"), "{param}");
        assert_eq!(e.detail.unwrap()["param"], json!(param));
    };
    refused(
        json!([{"id": "p", "quantity": "spectrum", "points": 512}]),
        "plots[0].points",
    );
    refused(
        json!([{"id": "s", "quantity": "survey", "channels": [1]}]),
        "plots[0].channels",
    );
    refused(
        json!([{"id": "r", "quantity": "readings", "points": 64}]),
        "plots[0].points",
    );
    refused(
        json!([{"id": "s", "quantity": "survey", "points": 32}]),
        "plots[0].points",
    );
    refused(
        json!([{"id": "x", "quantity": "phase"}]),
        "plots[0].quantity",
    );
    refused(
        json!([{"id": "a", "quantity": "time"}, {"id": "a", "quantity": "time"}]),
        "plots[1].id",
    );
}

const BINARY: [&str; 2] = ["event_explorer_live", "event_explorer_data"];

#[test]
fn explorer_the_binary_goldens_decode_to_their_expected_json_and_re_encode_byte_for_byte() {
    for name in BINARY {
        let bytes = read_bin(&format!("{name}.bin"));
        let decoded = codec::decode(&bytes).unwrap();
        same(
            &decoded.expanded(),
            &read_json(&format!("{name}.expected.json")),
            name,
        );
        let order = codec::header_array_order(&bytes).unwrap();
        let order: Vec<&str> = order.iter().map(String::as_str).collect();
        assert_eq!(codec::reencode(&decoded, &order).unwrap(), bytes, "{name}");
    }
}

#[test]
fn explorer_every_text_golden_is_a_compact_text_frame() {
    for name in [
        "request_explorer_configure",
        "response_explorer_configure",
        "request_explorer_start",
        "response_explorer_start",
        "request_explorer_steer",
        "response_explorer_steer",
        "request_explorer_sweep",
        "response_explorer_sweep",
        "event_explorer_state",
        "request_explorer_subscribe",
        "response_explorer_subscribe",
        "response_explorer_survey",
        "response_error_explorer_config_invalid",
    ] {
        let text = std::fs::read_to_string(golden(&format!("{name}.json"))).unwrap();
        let message = codec::decode_text(&text).unwrap();
        match codec::encode_frame(&message, &Arrays::new()).unwrap() {
            Frame::Text(t) => same(&serde_json::from_str(&t).unwrap(), &message, name),
            Frame::Binary(_) => panic!("{name}: a message without arrays must be a text frame"),
        }
    }
}
