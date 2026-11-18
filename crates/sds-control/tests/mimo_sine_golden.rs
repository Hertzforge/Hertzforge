//! The MIMO Sine goldens in `schema/golden/`, built by this crate's own
//! builders: a configuration parses and checks to the golden derived values
//! and state, an invalid one to the golden items, the states and the live,
//! sweep and data frames are the objects the service sends (the binary frames
//! decoded and compared by value, array by array), the subscription parses to
//! its golden result,
//!
//! `HERTZFORGE_PRINT_GOLDEN=1` prints what the builders give, for writing
//! `schema/make_golden.py`.

mod support;

use hertzforge_channel_table::{ChannelRole, ChannelTable};
use sds_control::codec::{self, Arrays};
use sds_control::mimo_sine::config::{self, CheckContext};
use sds_control::mimo_sine::laws as mlaws;
use sds_control::mimo_sine::run::configured_view;
use sds_control::mimo_sine::subscribe;
use sds_control::mimo_sine::view::{self, Chan, Figures, LawCounts, Live, MsView, Pass};
use sds_control::protocol;
use sds_control::random::view::{RecordRef, State};
use sds_control::sine::view::StationRef;
use sds_control::table;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden")
        .join(name)
}

fn read_json(name: &str) -> Value {
    let text = std::fs::read_to_string(golden(name)).unwrap_or_else(|e| panic!("read {name}: {e}"));
    serde_json::from_str(&text).unwrap()
}

fn printing() -> bool {
    std::env::var_os("HERTZFORGE_PRINT_GOLDEN").is_some()
}

fn show(name: &str, v: &Value) {
    if printing() {
        println!("==== {name}\n{}", serde_json::to_string(v).unwrap());
    }
}

/// Structural equality with numbers compared by value and keys in order.
fn same(a: &Value, b: &Value, path: &str) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            assert!(
                x == y || (x - y).abs() <= 1e-12 * x.abs().max(y.abs()),
                "number differs at {path}: {x} vs {y}"
            )
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

/// A frame's message with every array reference replaced by its values, as
/// `.expected.json` has it.
fn resolved(message: &Value, arrays: &Arrays) -> Value {
    let bytes = codec::encode(message, arrays).unwrap();
    codec::decode(&bytes).unwrap().expanded()
}

/// Controls 1 to 4, the article's two responses 5 and 6, drives 7 to 10.
fn table_base_article() -> ChannelTable {
    let mut t = table::Table::new().table;
    t.channels = (1..=10)
        .map(|i| {
            let mut c = table::new_row(i);
            c.role = if i <= 4 {
                ChannelRole::Control
            } else if i <= 6 {
                ChannelRole::Monitor
            } else {
                ChannelRole::Drive
            };
            c
        })
        .collect();
    t
}

fn builtin(id: &str) -> Option<mlaws::LawFacts> {
    let dir = std::env::temp_dir().join(format!(
        "sds-control-mimo-sine-golden-{}-{id}",
        std::process::id()
    ));
    let cat = hertzforge_random_laws::LawCatalog::open(&dir).unwrap();
    let f = mlaws::facts(&cat, id);
    let _ = std::fs::remove_dir_all(&dir);
    f
}

fn check(cfg: &Value) -> (table::Report, Option<config::Derived>, config::WireMsine) {
    let w = config::parse(cfg).unwrap();
    let t = table_base_article();
    let ctx = CheckContext {
        table: &t,
        law: &builtin,
        timeout_s: 30.0,
        sim: true,
    };
    let (r, d) = w.check(&ctx);
    (r, d, w)
}

fn sim_station() -> StationRef {
    StationRef {
        kind: "sim",
        class: "sim",
        devices: Vec::new(),
    }
}

fn chans(ids: &[u32], unit: &str) -> Vec<Chan> {
    ids.iter()
        .map(|&c| Chan {
            channel: c,
            description: format!("Channel {c}"),
            unit: unit.into(),
        })
        .collect()
}

/// The running view the binary goldens are drawn from: the golden configured
/// view, then the golden running state's own members.
fn running_view() -> MsView {
    let req = read_json("request_mimo_sine_configure.json");
    let (_r, d, w) = check(&req["params"]["config"]);
    let d = d.expect("the golden configuration checks clean");
    let facts = builtin("matrix").unwrap();
    let values = facts
        .values
        .iter()
        .map(|x| (x.key.clone(), x.label.clone(), x.unit.clone()))
        .collect();
    let law = sds_control::mimo_sine::law_state(&facts, &w.setup.law.params);
    let mut v = configured_view("msine-0002", &w, &d, sim_station(), law, values);
    let s = read_json("response_mimo_sine_state.json")["result"].clone();
    v.state = State::Running;
    v.phase = "engaged";
    v.grid.actual = true;
    v.clock = Some("shared");
    v.level_db = s["level_db"].as_f64();
    v.rung = s["rung"].as_u64().map(|x| x as usize);
    v.frequency_hz = s["frequency_hz"].as_f64();
    v.direction = Some("up");
    v.sweeps = Some((
        s["sweeps"]["done"].as_u64().unwrap(),
        s["sweeps"]["of"].as_u64(),
    ));
    v.elapsed_s = s["elapsed_s"].as_f64();
    v.at_level_s = s["at_level_s"].as_f64();
    v.remaining_s = s["remaining_s"].as_f64();
    v.alarm = s["alarm"].as_bool().unwrap();
    v.notch = s["notch"].as_bool().unwrap();
    v.notch_scale = s["notch_scale"].as_f64().unwrap();
    v.identification = Some(s["identification"].clone());
    v.review = Some(s["review"].clone());
    v.law_built = true;
    v.counts = LawCounts {
        misses: 0,
        capped_ticks: s["law"]["capped_ticks"].as_u64().unwrap(),
        over_ceiling: s["law"]["over_ceiling"].as_u64().unwrap(),
        rotation_max_deg: s["law"]["rotation_max_deg"].as_f64().unwrap(),
        asked: s["law"]["requests"]["asked"].as_u64().unwrap(),
        granted: s["law"]["requests"]["granted"].as_u64().unwrap(),
    };
    v.record = Some(RecordRef {
        folder: s["record"]["folder"].as_str().unwrap().into(),
        streams: true,
        problem: None,
    });
    v.streaming = true;
    v.controls = chans(&[1, 2, 3, 4], "g");
    v.limits = chans(&[5, 6], "g");
    v.drives = chans(&[7, 8, 9, 10], "V");
    v.live = Some(Arc::new(live()));
    v.figures = Some(Arc::new(figures()));
    v
}

/// The live tick the frames show: every value exact in binary.
fn live() -> Live {
    Live {
        t_s: 81.25,
        controls: (0..4)
            .map(|k| {
                let a = 0.5 + 0.0625 * k as f64;
                [a, -12.5, 0.5, -10.0, 0.25, -2.5, 0.0]
            })
            .collect(),
        limits: vec![[1.25, 171.0, 3.0, -7.5], [1.5, 170.0, 3.0, -6.0]],
        drives: (0..4).map(|k| [0.25 + 0.125 * k as f64, 33.0]).collect(),
        opposition_ratio: 1.0625,
        largest_share: 0.3125,
        rotation_deg: 0.75,
        request: None,
        values: vec![0.125, 0.0, 0.0, 0.0],
        law_faulted: false,
        sweep_hz_per_s: 1.5,
        sweep_oct_per_min: 2.0,
        commanded_db: -6.0,
    }
}

/// Figures on three points of the sweep grid, every value exact in binary;
/// the third point not reached yet.
fn figures() -> Figures {
    let x = vec![100.0, 200.0, 400.0];
    let ctl = |a: f64| vec![[a, -12.5, 0.5, -10.0, 0.25, 0.0], [a * 2.0, -15.0, 0.5, -10.0, -0.5, 0.0], [f64::NAN; 6]];
    Figures {
        against_time: false,
        x,
        control: (0..4).map(|k| ctl(0.5 + 0.0625 * k as f64)).collect(),
        limit: vec![
            vec![[1.25, 3.0], [2.5, 3.0], [f64::NAN; 2]],
            vec![[1.5, 3.0], [2.75, 3.0], [f64::NAN; 2]],
        ],
        drive: (0..4)
            .map(|k| vec![[0.25 + 0.125 * k as f64, 33.0], [0.5, 30.0], [f64::NAN; 2]])
            .collect(),
        readings: vec![[1.0625, 0.3125], [1.125, 0.375], [f64::NAN; 2]],
    }
}

#[test]
fn mimo_sine_golden_the_configuration_checks_to_its_derived_values_and_state() {
    let req = read_json("request_mimo_sine_configure.json");
    let (r, d, w) = check(&req["params"]["config"]);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let d = d.unwrap();
    let facts = builtin("matrix").unwrap();
    let law = sds_control::mimo_sine::law_state(&facts, &w.setup.law.params);
    let v = configured_view("msine-0002", &w, &d, sim_station(), law, Vec::new());
    let got = json!({
        "state": view::state_json(Some(&v)),
        "derived": d.to_json(),
        "warnings": r.warnings.iter().map(|i| i.to_json()).collect::<Vec<_>>(),
    });
    show("response_mimo_sine_configure.result", &got);
    if printing() {
        return;
    }
    same(
        &got,
        &read_json("response_mimo_sine_configure.json")["result"],
        "result",
    );
}

#[test]
fn mimo_sine_golden_the_invalid_configuration_names_its_items() {
    let mut cfg = read_json("request_mimo_sine_configure.json")["params"]["config"].clone();
    cfg["channels"]["drives"][1]["abort_vpk"] = json!(1);
    cfg["control"]["law"]["id"] = json!("secant");
    let (r, d, _w) = check(&cfg);
    assert!(d.is_none());
    let err = sds_control::sine::config::config_invalid(&r);
    let got = json!({"code": "bad_request", "reason": err.reason, "message": err.message,
                     "detail": err.detail});
    show("response_error_mimo_sine_config_invalid.error", &got);
    if printing() {
        return;
    }
    same(
        &got,
        &read_json("response_error_mimo_sine_config_invalid.json")["error"],
        "error",
    );
}

#[test]
fn mimo_sine_golden_the_states_are_the_service_objects() {
    let v = running_view();
    let got = view::state_json(Some(&v));
    show("response_mimo_sine_state.result", &got);
    if printing() {
        return;
    }
    same(&got, &read_json("response_mimo_sine_state.json")["result"], "state");
    let mut review = running_view();
    review.state = State::Review;
    review.phase = "awaiting_review";
    review.level_db = None;
    review.rung = None;
    review.frequency_hz = None;
    review.direction = None;
    review.sweeps = Some((0, Some(2)));
    review.elapsed_s = None;
    review.at_level_s = None;
    review.remaining_s = None;
    review.notch = false;
    review.notch_scale = 1.0;
    review.counts = LawCounts::default();
    let g = read_json("event_mimo_sine_state.json");
    same(&view::state_json(Some(&review)), &g["data"], "review");
}

#[test]
fn mimo_sine_golden_the_live_sweep_and_data_frames_are_the_service_frames() {
    let v = running_view();
    let mut a = Arrays::new();
    let live = protocol::event("mimo_sine.live", 309, view::live_json(&v, &mut a));
    let got = resolved(&live, &a);
    show("event_mimo_sine_live", &got);
    let pass = Pass {
        pass: 1,
        level_db: -6.0,
        direction: Some("up"),
        figures: figures(),
    };
    let mut a = Arrays::new();
    let sweep = protocol::event("mimo_sine.sweep", 310, view::sweep_json(&v, &pass, &mut a));
    let got_sweep = resolved(&sweep, &a);
    show("event_mimo_sine_sweep", &got_sweep);
    let sub = subscribe::parse(
        read_json("request_mimo_sine_subscribe.json")["params"]
            .as_object()
            .unwrap(),
    )
    .unwrap()
    .unwrap();
    let mut a = Arrays::new();
    let data = protocol::event("mimo_sine.data", 311, subscribe::data_json(&v, &sub, &mut a));
    let got_data = resolved(&data, &a);
    show("event_mimo_sine_data", &got_data);
    if printing() {
        return;
    }
    same(&got, &read_json("event_mimo_sine_live.expected.json"), "live");
    same(
        &got_sweep,
        &read_json("event_mimo_sine_sweep.expected.json"),
        "sweep",
    );
    same(
        &got_data,
        &read_json("event_mimo_sine_data.expected.json"),
        "data",
    );
}

#[test]
fn mimo_sine_golden_the_subscription_parses_to_its_result() {
    let req = read_json("request_mimo_sine_subscribe.json");
    let sub = subscribe::parse(req["params"].as_object().unwrap()).unwrap();
    let got = subscribe::result_json(sub.as_ref());
    show("response_mimo_sine_subscribe.result", &got);
    if printing() {
        return;
    }
    same(
        &got,
        &read_json("response_mimo_sine_subscribe.json")["result"],
        "result",
    );
}

#[test]
fn mimo_sine_golden_every_text_golden_is_a_compact_text_frame() {
    let mut names: Vec<String> = std::fs::read_dir(golden(""))
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| {
            n.contains("mimo_sine")
                && n.ends_with(".json")
                && !n.ends_with(".expected.json")
        })
        .collect();
    names.sort();
    assert!(names.len() >= 12, "{names:?}");
    for n in names {
        let text = std::fs::read_to_string(golden(&n)).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        let kind = v["type"].as_str().unwrap();
        assert!(matches!(kind, "request" | "response" | "event"), "{n}");
        // Every MIMO Sine method a request golden names is one the service
        // serves.
        if let Some(m) = v["method"].as_str() {
            assert!(m.starts_with("mimo_sine."), "{n}: {m}");
        }
    }
}
