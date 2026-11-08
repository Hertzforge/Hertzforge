//! The Sine goldens in `schema/golden/`, built by this crate's own builders: the
//! configuration parses and checks to the golden derived values, a configuration
//! with errors is refused as the golden error, the import reads to the golden
//! profile, the catalog's Sine entry is the golden one, the binary frames decode and
//! re-encode byte for byte, and the text frames are compact text frames.

use hertzforge_channel_table::{ChannelRole, ChannelTable, Units};
use sds_control::codec::{self, Arrays, Frame};
use sds_control::sine::config::{self, CheckContext};
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

/// A control row (1), a response (2), the fixture's accelerometer (3, the limit
/// channel) and a drive (4), as the golden configuration names them.
fn table() -> ChannelTable {
    let mut t = table::Table::new().table;
    let mut c1 = table::new_row(1);
    c1.role = ChannelRole::Control;
    c1.description = "Control".into();
    let mut c2 = table::new_row(2);
    c2.role = ChannelRole::Monitor;
    c2.description = "Response".into();
    let mut c3 = table::new_row(3);
    c3.role = ChannelRole::Monitor;
    c3.description = "Fixture".into();
    let mut c4 = table::new_row(4);
    c4.role = ChannelRole::Drive;
    c4.units = Units::VoltageV;
    t.channels = vec![c1, c2, c3, c4];
    t
}

fn check(config: &Value) -> (table::Report, Option<config::Derived>) {
    let w = config::parse(config).unwrap();
    let t = table();
    let law = |id: &str| sds_control::laws::sine_lookup(None, id);
    let ctx = CheckContext {
        table: &t,
        law: &law,
        timeout_s: 30.0,
        station_outputs: 1,
    };
    w.check(&ctx)
}

fn golden_config() -> Value {
    let request = read_json("request_sine_configure.json");
    let parsed = protocol::parse_request(&request).unwrap();
    assert_eq!(parsed.method, "sine.configure");
    parsed.params["config"].clone()
}

#[test]
fn the_configuration_checks_to_the_golden_derived_values() {
    let (report, derived) = check(&golden_config());
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let want = read_json("response_sine_configure.json");
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
}

#[test]
fn a_configuration_with_errors_is_refused_as_the_golden_error() {
    let mut config = golden_config();
    config["limits"]["drive_vpk_abort"] = json!(0.4);
    config["limits"]["channels"][0]["channel"] = json!(1);
    config["acquisition"]["tick_ms"] = json!(1);
    let (report, derived) = check(&config);
    assert!(derived.is_none());
    let e = config::config_invalid(&report);
    let want = read_json("response_error_sine_config_invalid.json");
    same(&protocol::error_response(79, &e), &want, "error");
}

#[test]
fn the_import_reads_the_golden_rows_to_the_golden_profile() {
    let text = "# the golden profile\nfreq_hz,displacement (in pk-pk),accel (g),alarm,abort\n\
                20,0.0097,,,\n45,,1,1,2\n2000,,1,,\n";
    let want = read_json("response_sine_profile_import.json");
    let got = sds_control::sine::profile::import_text(text, "text");
    same(&got, &want["result"], "import");
    // The file at a path reads the same, and a relative path is refused.
    let dir = std::env::temp_dir().join(format!("sds-sine-golden-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("profile.csv");
    std::fs::write(&file, text).unwrap();
    let params = json!({"path": file.to_str().unwrap()});
    let got = sds_control::sine::profile::import(params.as_object().unwrap()).unwrap();
    same(&got, &want["result"], "import by path");
    let _ = std::fs::remove_dir_all(dir);
    let relative = json!({"path": "profile.csv"});
    let e = sds_control::sine::profile::import(relative.as_object().unwrap()).unwrap_err();
    assert_eq!(e.reason, Some("bad_params"));
}

#[test]
fn the_sine_law_entry_is_the_golden_one() {
    let dir = std::env::temp_dir().join(format!(
        "sds-control-sine-golden-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let cat = hertzforge_random_laws::LawCatalog::open(&dir).unwrap();
    let list = sds_control::laws::list_json(&cat, Some(hertzforge_random_laws::LawWorld::Sine));
    let _ = std::fs::remove_dir_all(&dir);
    let want = read_json("response_laws_list_sine.json");
    same(&list, &want["result"], "laws.list sine");
    same(
        &want["result"]["laws"][0],
        &read_json("response_laws_check_sine.json")["result"]["law"],
        "the checked law's entry",
    );
}

const BINARY: [&str; 3] = [
    "event_sine_live",
    "event_sine_data",
    "response_sine_profile_validate",
];

#[test]
fn the_binary_goldens_decode_to_their_expected_json_and_re_encode_byte_for_byte() {
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
fn every_sine_text_golden_is_a_compact_text_frame() {
    for name in [
        "request_sine_configure",
        "response_sine_configure",
        "event_sine_state",
        "request_sine_subscribe",
        "response_sine_subscribe",
        "request_sine_level",
        "request_sine_adjust",
        "response_sine_profile_import",
        "response_laws_list_sine",
        "response_laws_check_sine",
        "response_error_sine_config_invalid",
    ] {
        let text = std::fs::read_to_string(golden(&format!("{name}.json"))).unwrap();
        let message = codec::decode_text(&text).unwrap();
        match codec::encode_frame(&message, &Arrays::new()).unwrap() {
            Frame::Text(t) => same(&serde_json::from_str(&t).unwrap(), &message, name),
            Frame::Binary(_) => panic!("{name}: a message without arrays must be a text frame"),
        }
    }
}
