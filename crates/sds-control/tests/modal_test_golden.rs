//! The Modal Test goldens in `schema/golden/`, against this crate's own code: the
//! configuration parses, the state is the golden impact run folder's last state, the
//! live frame is this crate's view builder's own output (byte for byte), and a force
//! window past the block is refused as the golden error.

use sds_control::codec;
use sds_control::modal_test::config;
use sds_control::modal_test::run::{self, Display, HitShow, ModalView, Named};
use sds_control::playback::config::config_invalid;
use sds_control::protocol;
use sds_control::table::{parse_rows, Report};
use serde_json::{json, Value};
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

/// The rows of the golden impact run: the hammer and two responses.
fn rows() -> Value {
    let row = |index: u32, units: &str, range: u32, node: u32, part: &str| {
        json!({
            "index": index, "enabled": true, "description": format!("{part} {index}"),
            "role": "monitor", "physical": null, "interface": "single_ended",
            "iepe_bias_v": null, "units": units, "sensitivity_mv_per_eu": 10,
            "range_max": range, "polarity": "positive", "control_group": null,
            "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null},
            "node": node, "direction": "Z+", "analysis_role": part,
        })
    };
    json!([
        row(1, "lbf", 100, 1, "reference"),
        row(2, "g", 50, 4, "response"),
        row(3, "g", 50, 7, "response"),
    ])
}

#[test]
fn modal_test_the_golden_configuration_parses_and_is_the_golden_runs() {
    let request = read_json("request_modal_test_configure.json");
    let parsed = protocol::parse_request(&request).unwrap();
    assert_eq!(parsed.method, "modal_test.configure");
    let cfg = &request["params"]["config"];
    let w = config::parse(cfg).unwrap();
    assert_eq!(w.setup.block, 512);
    let table = parse_rows(&rows()).unwrap();
    let (r, plan) = config::check(&w, &table);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(plan.unwrap().measurements.len(), 3);
    // the configuration the golden impact run folder was made with
    let setup: Value = serde_json::from_slice(
        &std::fs::read(golden("runs/modal_test/impact_roving/setup.json")).unwrap(),
    )
    .unwrap();
    same(cfg, &setup["config"], "config");
    // a key the configuration does not have is refused by its path
    let mut bad = cfg.clone();
    bad["impact"]["window"] = json!(true);
    let e = config::parse(&bad).unwrap_err();
    assert!(e.message.contains("config.impact.window"), "{}", e.message);
    for name in [
        "request_modal_test_start.json",
        "request_modal_test_arm.json",
        "request_modal_test_select.json",
        "request_modal_test_sim_hit.json",
    ] {
        assert!(protocol::parse_request(&read_json(name)).is_ok(), "{name}");
    }
}

#[test]
fn modal_test_the_golden_state_is_the_golden_runs_last_state() {
    let golden = read_json("event_modal_test_state.json");
    assert_eq!(golden["topic"], "modal_test.state");
    let end: Value = serde_json::from_slice(
        &std::fs::read(self::golden("runs/modal_test/impact_roving/end.json")).unwrap(),
    )
    .unwrap();
    let mut state = end["state"].clone();
    state["record"]["folder"] = golden["data"]["record"]["folder"].clone();
    same(&state, &golden["data"], "state");
    assert_eq!(
        run::state_json(None),
        json!({"run_id": null, "state": "none"})
    );
}

fn live_view() -> ModalView {
    ModalView {
        run_id: "modal-test-0001".into(),
        state: "armed",
        mode: "impact",
        source: "SIM",
        station_class: "sim",
        title: "Golden Roving Hammer".into(),
        drive_phase: "idle",
        counts: Value::Null,
        hits: json!([]),
        latest: Value::Null,
        point: 1,
        point_dofs: Value::Null,
        record: None,
        end: None,
    }
}

#[test]
fn modal_test_the_live_frame_is_the_golden_frame_byte_for_byte() {
    let want = std::fs::read(golden("event_modal_test_live.bin")).unwrap();
    let expected = read_json("event_modal_test_live.expected.json");
    let named = |index, role, unit: &str| Named {
        index,
        role,
        unit: unit.into(),
    };
    let names = vec![
        named(1, "reference", "lbf"),
        named(2, "response", "g"),
        named(3, "response", "g"),
    ];
    let block = vec![
        vec![0.0, 18.5, 2.25, 0.0],
        vec![0.0, 0.5, -1.25, 0.75],
        vec![0.0, -0.25, 0.625, -0.375],
    ];
    let d = Display {
        rate_hz: 1024.0,
        names: names.clone(),
        channels: block.clone(),
        freqs: vec![0.0, 2.0, 4.0, 6.0],
        mag: vec![
            vec![vec![0.03125, 0.0625, 0.125, 0.25]],
            vec![vec![0.015625, 0.03125, 0.0625, 0.125]],
        ],
        phase_deg: vec![
            vec![vec![0.0, -90.0, -180.0, 90.0]],
            vec![vec![180.0, 90.0, 0.0, -90.0]],
        ],
        coherence: vec![vec![0.5, 0.75, 0.875, 1.0], vec![0.25, 0.5, 0.75, 1.0]],
        averages: 2,
        frame: None,
        hit: Some(HitShow {
            point: 1,
            n: 2,
            seq: 4,
            start_frame: 1536,
            start_s: 1.5,
            samples: 4,
            decimation: 1,
            step_s: 1.0 / 1024.0,
            names,
            channels: block,
            force_window: Some((1, 2, 1)),
            exponential: Some((0.125, 0.0625)),
            force_unit: "lbf".into(),
            force_auto: vec![4.0, 2.0, 1.0, 0.5],
            tip_drop_db: 9.5,
        }),
        ref_names: Vec::new(),
        ref_auto: Vec::new(),
    };
    let mut arrays = codec::Arrays::new();
    let data = run::live_json(&live_view(), &d, &mut arrays);
    let frame = codec::encode(&protocol::event("modal_test.live", 412, data), &arrays).unwrap();
    assert_eq!(frame, want, "the live frame differs from the golden");
    let decoded = codec::decode(&want).unwrap();
    assert_eq!(decoded.message["topic"], expected["topic"]);
    assert_eq!(decoded.message["data"]["time"]["rate_hz"], 1024);
}

#[test]
fn modal_test_a_force_window_past_the_block_is_refused_as_the_golden_error() {
    let mut cfg = read_json("request_modal_test_configure.json")["params"]["config"].clone();
    cfg["impact"]["force_window"] = json!({"width_pct": 99});
    let w = config::parse(&cfg).unwrap();
    let table = parse_rows(&rows()).unwrap();
    let (r, plan) = config::check(&w, &table);
    assert!(plan.is_none());
    let report = Report {
        errors: r.errors,
        warnings: Vec::new(),
    };
    let e = config_invalid(&report, Some(&Value::Null));
    let got = protocol::error_response(214, &e);
    same(
        &got,
        &read_json("response_error_modal_test_config_invalid.json"),
        "error",
    );
}
