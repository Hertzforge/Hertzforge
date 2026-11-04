//! The reference sub-family (`mimo_random.reference.*`) against its goldens in
//! `schema/golden/` and over a real loopback WebSocket: the goldens follow the
//! schema, the rigid-body and import answers are the golden ones, and a breakpoint
//! reference whose pairs cannot all hold comes back over the wire with its preview
//! views (as a binary frame), the lines that fail and the repair, and is accepted
//! only when the repair is.
//!
//! The response goldens are this crate's own output: with `SDS_WRITE_REFERENCE_GOLDEN=1`
//! the test writes them instead of comparing (one-space indent, as the others).

mod support;

use hertzforge_station::StationBuildError;
use sds_control::mimo::reference;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::time::Duration;
use support::schema::Schema;
use support::{options, serve, Client};

fn path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden")
        .join(name)
}

fn golden(name: &str) -> Value {
    let text = std::fs::read_to_string(path(name)).unwrap_or_else(|e| panic!("read {name}: {e}"));
    serde_json::from_str(&text).unwrap()
}

fn params(v: &Value) -> Map<String, Value> {
    v["params"].as_object().cloned().unwrap()
}

/// Write `v` as a golden (one-space indent), or compare it with the golden: same
/// keys in the same order, numbers within `tol` relative.
fn golden_or_write(name: &str, v: &Value, tol: f64) {
    if std::env::var_os("SDS_WRITE_REFERENCE_GOLDEN").is_some() {
        let mut out = String::new();
        pretty(v, 0, &mut out);
        out.push('\n');
        std::fs::write(path(name), out).unwrap();
        return;
    }
    close(&golden(name), v, tol, "$");
}

fn close(a: &Value, b: &Value, tol: f64, at: &str) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            assert!(
                (x - y).abs() <= tol * x.abs().max(y.abs()).max(1.0),
                "{at}: {x} against {y}"
            );
        }
        (Value::Object(x), Value::Object(y)) => {
            assert_eq!(
                x.keys().collect::<Vec<_>>(),
                y.keys().collect::<Vec<_>>(),
                "keys at {at}"
            );
            for (k, v) in x {
                close(v, &y[k], tol, &format!("{at}.{k}"));
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            assert_eq!(x.len(), y.len(), "length at {at}");
            for (i, (u, v)) in x.iter().zip(y).enumerate() {
                close(u, v, tol, &format!("{at}[{i}]"));
            }
        }
        _ => assert_eq!(a, b, "{at}"),
    }
}

#[test]
fn the_rigid_body_answer_is_the_golden_one() {
    let req = golden("request_mimo_random_reference_build_rigid_body.json");
    let result = reference::build_rigid_body(&params(&req)).unwrap();
    assert_eq!(result["ok"], true, "{result}");
    // Seven readings, six unknowns: one spare reading's worth of checking, shared.
    let shares: f64 = result["sensors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["check_share"].as_f64().unwrap())
        .sum();
    assert!((shares - 1.0).abs() < 1e-9, "{shares}");
    let message = json!({"type": "response", "id": 42, "result": result});
    golden_or_write(
        "response_mimo_random_reference_build_rigid_body.json",
        &message,
        1e-9,
    );
}

#[test]
fn a_specification_is_refused_on_its_name_alone_wherever_it_is() {
    // The golden import names a Unix path, which is relative on Windows. A .mat
    // or .hforge is never read here, so its refusal must not depend on the
    // path's shape: a bare file name gets the same answer as the golden.
    let mut req = golden("request_mimo_random_reference_import.json");
    req["params"]["path"] = json!("qualification.mat");
    let dir = std::env::temp_dir().join(format!("sds-ref-relative-{}", std::process::id()));
    let (result, arrays) = reference::import(&params(&req), &dir).unwrap();
    assert!(arrays.is_empty());
    assert_eq!(result["ok"], false, "{result}");
    assert_eq!(result["errors"][0]["reason"], "reference_file", "{result}");
    let message = result["errors"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("converted to a reference folder"),
        "{message}"
    );
}

#[test]
fn an_import_of_a_file_it_does_not_read_is_the_golden_answer() {
    let req = golden("request_mimo_random_reference_import.json");
    let dir = std::env::temp_dir().join(format!("sds-ref-golden-{}", std::process::id()));
    let (result, arrays) = reference::import(&params(&req), &dir).unwrap();
    assert!(arrays.is_empty(), "a refused import carries no preview");
    assert_eq!(result["ok"], false);
    let message = json!({"type": "response", "id": 43, "result": result});
    golden_or_write("response_mimo_random_reference_import.json", &message, 0.0);
}

#[test]
fn the_reference_goldens_follow_the_schema() {
    let schema = Schema::load();
    for (name, path, def) in [
        (
            "request_mimo_random_reference_build_breakpoints.json",
            "params",
            "mimoReferenceBuildParams",
        ),
        (
            "request_mimo_random_reference_build_rigid_body.json",
            "params",
            "mimoRigidParams",
        ),
        (
            "response_mimo_random_reference_build_rigid_body.json",
            "result",
            "mimoRigidResult",
        ),
        (
            "request_mimo_random_reference_import.json",
            "params",
            "mimoReferenceImportParams",
        ),
        (
            "response_mimo_random_reference_import.json",
            "result",
            "mimoReferenceResult",
        ),
    ] {
        if std::env::var_os("SDS_WRITE_REFERENCE_GOLDEN").is_some() && name.starts_with("response")
        {
            continue;
        }
        let g = golden(name);
        schema.assert_valid(&g, None);
        schema.assert_valid(&g[path], Some(def));
    }
}

/// The golden setup's extensions (a drive's peak and shaker, a limit, the caps and
/// budgets) follow the schema; out of range they are refused.
#[test]
fn the_setup_extensions_follow_the_schema() {
    let schema = Schema::load();
    let mut setup = golden("request_mimo_random_setup.json")["params"]["setup"].clone();
    setup["channels"]["drives"][0]["peak_v"] = json!(6);
    setup["channels"]["drives"][0]["shaker"] = json!({"name": "a shaker"});
    setup["channels"]["drives"][0]["moving_mass_lb"] = json!(12.5);
    setup["channels"]["limits"] = json!([{"channel": 5, "ceiling": 0.2, "abort_margin_db": 3}]);
    setup["channels"]["article_mass_lb"] = json!(40);
    setup["control"]["frf_blend"] = json!(0.5);
    setup["control"]["cap_up_db"] = json!(3.5);
    setup["control"]["cap_down_db"] = json!(6);
    setup["control"]["law_budget"] = json!(0.25);
    setup["control"]["law_misses"] = json!(3);
    schema.assert_valid(&setup, Some("mimoSetup"));
    let mut bad = setup.clone();
    bad["control"]["cap_up_db"] = json!(7);
    assert!(schema.check(&bad, Some("mimoSetup")).is_err());
    let mut bad = setup;
    bad["channels"]["limits"][0]["ceiling"] = json!(0);
    assert!(schema.check(&bad, Some("mimoSetup")).is_err());
}

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_breakpoint_reference_comes_over_the_wire_with_its_preview_and_repair() {
    let schema = Schema::load();
    let svc = Service::with_options(options(
        no_driver(),
        1.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    // An observer may build: nothing is written but a reference too large to send.
    let mut c = Client::connect(addr).await;
    let req = golden("request_mimo_random_reference_build_breakpoints.json");
    let r = c
        .ok(
            "mimo_random.reference.build_breakpoints",
            req["params"].clone(),
        )
        .await;
    schema.assert_valid(&r, Some("mimoReferenceResult"));
    assert_eq!(r["ok"], false);
    assert!(r["reference"].is_null());
    assert_eq!(r["errors"][0]["reason"], "reference_not_psd");
    assert!(r["psd"]["violations"].as_u64().unwrap() > 0);
    assert!(r["psd"]["min_eigenvalue_ratio"].as_f64().unwrap() < -0.1);
    let pairs = r["psd"]["repair"]["pairs"].as_array().unwrap();
    assert!(!pairs.is_empty());
    assert!(pairs.iter().all(|p| p["after"].as_f64().unwrap() < 0.9));
    let views = r["views"].as_array().unwrap();
    assert_eq!(views.len(), 4, "PSDs, eigenvalues, coherence, phase");
    assert_eq!(views[2]["title"], "Coherence, Rear and Side");
    let id = json!(c_last_id(&c));
    assert!(
        c.decoded_response(&id).is_some(),
        "the preview's arrays travel as a binary frame"
    );

    let mut accept = req["params"].clone();
    accept["repair"] = json!(true);
    let r = c
        .ok("mimo_random.reference.build_breakpoints", accept)
        .await;
    assert_eq!(r["ok"], true, "{}", r["errors"]);
    assert_eq!(r["reference"]["kind"], "inline");
    assert_eq!(r["psd"]["repair_accepted"], true);
    // Coherence before and after repair are both drawn.
    assert_eq!(r["views"][2]["series"].as_array().unwrap().len(), 2);

    let rigid = golden("request_mimo_random_reference_build_rigid_body.json");
    let g = c
        .ok(
            "mimo_random.reference.build_rigid_body",
            rigid["params"].clone(),
        )
        .await;
    assert_eq!(g["transform"]["rows"], 6);
}

fn c_last_id(c: &Client) -> i64 {
    c.last_id
}

/// JSON with a one-space indent, keys in order (the goldens' layout).
fn pretty(v: &Value, depth: usize, out: &mut String) {
    let pad = |d: usize| " ".repeat(d);
    match v {
        Value::Object(m) if !m.is_empty() => {
            out.push_str("{\n");
            for (i, (k, x)) in m.iter().enumerate() {
                out.push_str(&pad(depth + 1));
                out.push_str(&Value::from(k.as_str()).to_string());
                out.push_str(": ");
                pretty(x, depth + 1, out);
                out.push_str(if i + 1 < m.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(depth));
            out.push('}');
        }
        Value::Array(a) if !a.is_empty() => {
            out.push_str("[\n");
            for (i, x) in a.iter().enumerate() {
                out.push_str(&pad(depth + 1));
                pretty(x, depth + 1, out);
                out.push_str(if i + 1 < a.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(depth));
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}
