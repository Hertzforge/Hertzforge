//! MIMO Random and the law catalog through the protocol, on a real loopback WebSocket,
//! with the simulated MIMO driver: the setup and its validation, the run from
//! identification through the review hold to the end, the gate override, the
//! controller rules, and the run exclusion against single-output Random.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::time::Duration;
use support::schema::Schema;
use support::{options, serve, sim_config, Client};

const LONG: Duration = Duration::from_secs(30);

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

fn row(index: u32, role: &str, description: &str) -> Value {
    let units = if role == "drive" { "V" } else { "g" };
    json!({
        "index": index, "enabled": true, "description": description,
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": if units == "V" { 1000 } else { 100 },
        "range_max": if units == "V" { 10 } else { 100 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

/// The golden setup's table: controls 1 and 2, drives 3 and 4.
fn two_by_two() -> Value {
    json!([
        row(1, "control", "Control X"),
        row(2, "control", "Control Y"),
        row(3, "drive", "Shaker X"),
        row(4, "drive", "Shaker Y"),
    ])
}

fn golden_setup() -> Value {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden/request_mimo_random_setup.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    v["params"]["setup"].clone()
}

/// Claim control, select the simulator and write `rows`.
async fn controller(c: &mut Client, rows: Value) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok("channels.set", json!({"revision": rev, "channels": rows}))
        .await;
}

async fn wait_mimo(c: &mut Client, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let i = c
        .wait_for(what, LONG, |e| {
            e.topic() == "mimo_random.status" && pred(e.data())
        })
        .await;
    c.events[i].data().clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_on_the_simulator_runs_end_to_end_through_the_protocol() {
    let schema = Schema::load();
    let svc = Service::with_options(options(
        no_driver(),
        40.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c, two_by_two()).await;

    // The law catalog: both worlds, each entry valid; the MIMO list holds MIMO laws.
    let laws = c.ok("laws.list", json!({})).await;
    schema.assert_valid(&laws, Some("lawsListResult"));
    let mimo = obs.ok("laws.list", json!({"world": "mimo"})).await;
    assert!(mimo["laws"]
        .as_array()
        .unwrap()
        .iter()
        .all(|l| l["world"] == "mimo"));
    assert_eq!(mimo["laws"][0]["id"], "pinv_closed");

    // Unconfigured: the status says so, and nothing to report.
    let none = obs.ok("mimo_random.status", json!({})).await;
    schema.assert_valid(&none, Some("mimoStatus"));
    assert_eq!(none["state"], "unconfigured");
    assert_eq!(
        obs.ok("mimo_random.report", json!({})).await["report"],
        Value::Null
    );

    // A dry run needs no control; a real setup does.
    let setup = golden_setup();
    let dry = obs
        .ok(
            "mimo_random.setup",
            json!({"setup": setup, "dry_run": true}),
        )
        .await;
    schema.assert_valid(&dry["validation"], Some("mimoValidation"));
    assert_eq!(dry["validation"]["ok"], true);
    let e = obs
        .refused("mimo_random.setup", json!({"setup": setup}))
        .await;
    assert_eq!(e["reason"], "not_controller");
    let set = c.ok("mimo_random.setup", json!({"setup": setup})).await;
    schema.assert_valid(&set, Some("mimoSetupResult"));
    assert_eq!(set["state"]["state"], "configured");
    assert_eq!(
        set["state"]["shape"]["labels"],
        json!(["Control X", "Control Y"])
    );
    let v = obs.ok("mimo_random.validate", json!({})).await;
    assert_eq!(
        v["validation"], set["validation"],
        "the present setup's report"
    );
    obs.wait_for("the validate event", LONG, |e| {
        e.topic() == "mimo_random.validate"
    })
    .await;

    // Start: identification, then the review hold with the gates and a prediction.
    let e = obs.refused("mimo_random.start", json!({})).await;
    assert_eq!(e["reason"], "not_controller");
    let started = c.ok("mimo_random.start", json!({})).await;
    schema.assert_valid(&started, Some("mimoStatus"));
    let review = wait_mimo(&mut obs, "the review hold", |d| {
        d["phase"] == "awaiting_review"
    })
    .await;
    schema.assert_valid(&review, Some("mimoStatus"));
    assert_eq!(review["state"], "review");
    assert_eq!(review["gates"]["passed"], true);
    assert_eq!(review["prediction"]["iteration"], 1);
    assert!(obs
        .events
        .iter()
        .any(|e| e.topic() == "mimo_random.status" && e.data()["phase"] == "sysid"));

    // A setup now is refused: the run is live.
    let e = c
        .refused("mimo_random.setup", json!({"setup": setup}))
        .await;
    assert_eq!(e["reason"], "run_active");

    // Iterate the prediction with another convergence; then proceed.
    let p = c
        .ok(
            "mimo_random.iterate_prediction",
            json!({"law": {"id": "pinv_closed", "params": {"convergence": 0.25}}}),
        )
        .await;
    schema.assert_valid(&p, Some("mimoPredictionResult"));
    assert_eq!(p["prediction"]["iteration"], 2);
    let e = c
        .refused(
            "mimo_random.iterate_prediction",
            json!({"law": {"id": "pinv_closed", "params": {"convergence": 9}}}),
        )
        .await;
    assert_eq!(e["reason"], "bad_params");
    let e = c
        .refused("mimo_random.override", json!({"ack": "no need"}))
        .await;
    assert_eq!(e["reason"], "override_not_needed");
    let engaged = c.ok("mimo_random.proceed", json!({})).await;
    assert_eq!(engaged["state"], "running");
    let full = wait_mimo(&mut obs, "full level", |d| d["phase"] == "full_level").await;
    schema.assert_valid(&full, Some("mimoStatus"));
    assert_eq!(full["control_error_db"].as_array().unwrap().len(), 2);
    assert_eq!(full["drive_rms_v"].as_array().unwrap().len(), 2);
    assert_eq!(full["caps"].as_array().unwrap().len(), 2);
    assert_eq!(
        obs.ok("mimo_random.report", json!({})).await["report"],
        Value::Null
    );

    // Any screen may stop it: the observer does, and the run ramps down and ends.
    let stopping = obs.ok("mimo_random.stop", json!({})).await;
    assert!(
        matches!(stopping["state"].as_str(), Some("stopping" | "finished")),
        "{stopping}"
    );
    let done = wait_mimo(&mut obs, "the end", |d| d["state"] == "finished").await;
    schema.assert_valid(&done, Some("mimoStatus"));
    assert_eq!(done["end"]["outcome"], "operator_stop");
    let i = obs
        .wait_for("the report", LONG, |e| e.topic() == "mimo_random.report")
        .await;
    let report = obs.events[i].data()["report"].clone();
    schema.assert_valid(&report, Some("mimoReport"));
    assert_eq!(report["controls"].as_array().unwrap().len(), 2);
    let again = c.ok("mimo_random.report", json!({})).await;
    assert_eq!(again["report"], report);
    // Every status event on the way validated as a message.
    for e in obs
        .events
        .iter()
        .filter(|e| e.topic() == "mimo_random.status")
    {
        schema.assert_valid(&e.msg, None);
        schema.assert_valid(e.data(), Some("mimoStatus"));
    }
    // A finished run does not start again.
    let e = c.refused("mimo_random.start", json!({})).await;
    assert_eq!(e["reason"], "run_finished");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_gate_holds_until_the_operator_overrides_in_words() {
    let schema = Schema::load();
    let svc = Service::with_options(options(
        no_driver(),
        40.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, two_by_two()).await;
    let mut setup = golden_setup();
    setup["identification"]["coherence_floor"] = json!(0.99);
    c.ok("mimo_random.setup", json!({"setup": setup})).await;
    c.ok("mimo_random.start", json!({})).await;
    let held = wait_mimo(&mut c, "the gate hold", |d| d["phase"] == "gate_failed").await;
    assert_eq!(held["gates"]["passed"], false);
    let e = c.refused("mimo_random.proceed", json!({})).await;
    assert_eq!(e["reason"], "gate_failed");
    schema.assert_valid(&json!({"type": "response", "id": 1, "error": e}), None);
    assert!(e["message"].as_str().unwrap().contains("override"));
    let e = c
        .refused("mimo_random.override", json!({"ack": "   "}))
        .await;
    assert_eq!(
        (e["reason"].as_str(), e["detail"]["param"].as_str()),
        (Some("bad_params"), Some("ack"))
    );
    let words = "Low coherence below 30 Hz is the fixture; accepted.";
    let s = c.ok("mimo_random.override", json!({"ack": words})).await;
    assert_eq!(s["state"], "running");
    assert_eq!(s["override"]["text"], words);
    assert!(
        s["override"]["by"].as_str().unwrap().starts_with("Shell"),
        "{s}"
    );
    let a = c.ok("mimo_random.abort", json!({})).await;
    assert_eq!(a["end"]["outcome"], "operator_abort");
    assert_eq!(a["phase"], "aborted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_test_at_a_time_across_random_and_mimo() {
    let svc = Service::with_options(options(
        no_driver(),
        4.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, two_by_two()).await;
    // A Random configuration may sit beside a MIMO setup; neither runs yet.
    let cfg = sim_config("auto", json!([{"db": 0}]), 30.0);
    c.ok("random.configure", json!({"config": cfg})).await;
    c.ok("mimo_random.setup", json!({"setup": golden_setup()}))
        .await;
    c.ok("mimo_random.start", json!({})).await;
    // While MIMO is live, Random cannot configure or start, and no simulation starts.
    let e = c.refused("random.configure", json!({"config": cfg})).await;
    assert_eq!(e["reason"], "run_active");
    let e = c.refused("random.start", json!({})).await;
    assert_eq!(e["reason"], "run_active");
    let e = c
        .refused("sim.start", json!({"profile": "ramp_plateau", "speed": 0}))
        .await;
    assert_eq!(e["reason"], "run_active");
    // Once it ends, Random may go.
    c.ok("mimo_random.abort", json!({})).await;
    c.ok("random.configure", json!({"config": cfg})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_invalid_setup_names_every_problem_and_replaces_nothing() {
    let schema = Schema::load();
    let svc = Service::with_options(options(
        no_driver(),
        40.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, two_by_two()).await;
    let mut setup = golden_setup();
    setup["channels"]["controls"][0]["channel"] = json!(9);
    setup["law"] = json!({"id": "hinv"});
    let e = c
        .refused("mimo_random.setup", json!({"setup": setup}))
        .await;
    assert_eq!(e["reason"], "setup_invalid");
    schema.assert_valid(&e["detail"], Some("mimoValidation"));
    let reasons: Vec<&str> = e["detail"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["reason"].as_str().unwrap())
        .collect();
    assert!(reasons.contains(&"control_unknown_channel"), "{reasons:?}");
    assert!(reasons.contains(&"law_wrong_world"), "{reasons:?}");
    assert_eq!(
        c.ok("mimo_random.status", json!({})).await["state"],
        "unconfigured",
        "nothing was replaced"
    );
    // A malformed setup is refused by the field.
    let e = c
        .refused("mimo_random.setup", json!({"setup": {"channels": {}}}))
        .await;
    assert_eq!(e["reason"], "bad_params");
    assert_eq!(e["detail"]["param"], "setup.channels.controls");
    // A law file reaches the service by an absolute path, and a file that is not a law
    // is refused in words.
    let e = c.refused("laws.import", json!({"path": "law.wasm"})).await;
    assert_eq!(e["reason"], "bad_params");
    let bogus = std::env::temp_dir().join(format!("sds-not-a-law-{}.wasm", std::process::id()));
    std::fs::write(&bogus, b"not a law").unwrap();
    let e = c
        .refused("laws.import", json!({"path": bogus.to_str().unwrap()}))
        .await;
    assert_eq!(e["reason"], "law_import_failed");
    assert!(e["message"].as_str().unwrap().contains("Choose a law file"));
    let _ = std::fs::remove_file(&bogus);
    let mut obs = Client::connect(addr).await;
    let e = obs
        .refused("laws.import", json!({"path": "/x/y.wasm"}))
        .await;
    assert_eq!(e["reason"], "not_controller");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unattended_mimo_run_takes_the_safe_stop() {
    let svc = Service::with_options(options(
        no_driver(),
        5.0,
        Duration::from_millis(600),
        Duration::from_millis(200),
    ));
    let addr = serve(svc).await;
    let mut obs = Client::connect(addr).await;
    {
        let mut c = Client::connect(addr).await;
        controller(&mut c, two_by_two()).await;
        let mut setup = golden_setup();
        setup["identification"]["review"] = json!(false);
        setup["schedule"]["full_level_s"] = json!(600);
        c.ok("mimo_random.setup", json!({"setup": setup})).await;
        c.ok("mimo_random.start", json!({})).await;
        // Gone without a goodbye.
        drop(c);
    }
    let done = wait_mimo(&mut obs, "the safe stop", |d| d["state"] == "finished").await;
    assert_eq!(done["end"]["outcome"], "safe_stop");
}
