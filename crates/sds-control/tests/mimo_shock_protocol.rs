//! The `mimo_shock.*` methods' refusals, by name, in every state, and the
//! setup fields the configuration checks: what a client is told and when.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::time::Duration;
use support::{options, serve, Client};

const LONG: Duration = Duration::from_secs(300);

async fn ms_state(c: &mut Client, state: &str) -> Value {
    let i = c
        .wait_for(&format!("mimo_shock.state {state}"), LONG, |e| {
            e.topic() == "mimo_shock.state" && e.data()["state"] == state
        })
        .await;
    c.events[i].data().clone()
}

fn svc() -> std::sync::Arc<Service> {
    Service::with_options(options(
        FixedScanner(Err(StationBuildError::NiDriverUnavailable(
            "not installed on this test host".into(),
        ))),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ))
}

fn row(index: u32, role: &str) -> Value {
    json!({
        "index": index, "enabled": true, "description": role,
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": if role == "drive" { "V" } else { "g" }, "sensitivity_mv_per_eu": 1000,
        "range_max": if role == "drive" { 10 } else { 1000 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

async fn table(c: &mut Client, roles: &[&str]) {
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    let rows: Vec<Value> = roles
        .iter()
        .enumerate()
        .map(|(i, r)| row(i as u32 + 1, r))
        .collect();
    c.ok("channels.set", json!({"revision": rev, "channels": rows}))
        .await;
}

async fn controller(c: &mut Client) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    table(
        c,
        &[
            "control", "control", "control", "control", "drive", "drive", "drive", "drive",
        ],
    )
    .await;
}

fn config(mode: &str) -> Value {
    json!({
        "title": "Refusals",
        "channels": {
            "controls": [{"channel": 1}, {"channel": 2}, {"channel": 3}, {"channel": 4}],
            "drives": (5..=8).map(|i| json!({"channel": i, "ceiling_vpk": 6, "abort_vpk": 9}))
                .collect::<Vec<_>>(),
        },
        "target": {"kind": "waveform", "pulses": (0..4).map(|_| json!({
            "shape": "half_sine", "amplitude_g": 20, "duration_ms": 11})).collect::<Vec<_>>()},
        // The band is the default for the pulses (a 5 Hz edge is refused for an
        // 11 ms pulse: see the band test below).
        "acquisition": {"fs_hz": 12800, "record_len": 8192},
        "schedule": {"mode": mode, "levels_db": [-12, -6, 0]},
        "spacing": {"cooling_s": 0.5, "settling_s": 0, "compute_s": 0},
        "identification": {"drive_vrms": 1, "frames": 40, "review": true},
    })
}

async fn reason(c: &mut Client, method: &str, params: Value) -> String {
    c.refused(method, params).await["reason"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_before_a_configuration_every_step_is_refused_by_name() {
    let addr = serve(svc()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    for m in [
        "mimo_shock.arm",
        "mimo_shock.start",
        "mimo_shock.proceed",
        "mimo_shock.fire",
        "mimo_shock.reset",
    ] {
        assert_eq!(reason(&mut c, m, json!({})).await, "not_configured", "{m}");
    }
    assert_eq!(
        c.ok("mimo_shock.state", json!({})).await["state"],
        "unconfigured"
    );
    // Stop and abort answer the state and refuse nothing.
    assert_eq!(
        c.ok("mimo_shock.stop", json!({})).await["state"],
        "unconfigured"
    );
    assert_eq!(
        reason(&mut c, "mimo_shock.pulse", json!({"index": 0})).await,
        "not_configured"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_a_configuration_is_refused_by_shape_and_by_content() {
    let addr = serve(svc()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let mut cfg = config("auto");
    cfg.as_object_mut().unwrap().remove("channels");
    let e = c
        .refused("mimo_shock.configure", json!({"config": cfg}))
        .await;
    assert_eq!(e["reason"], "bad_params", "{e}");
    let mut cfg = config("auto");
    cfg["target"]["pulses"].as_array_mut().unwrap().pop();
    cfg["verdict"] = json!("global");
    cfg["channels"]["drives"][0]["abort_vpk"] = json!(5);
    cfg["control"] = json!({"law": {"id": "shape"}});
    let e = c
        .refused("mimo_shock.configure", json!({"config": cfg}))
        .await;
    assert_eq!(e["reason"], "config_invalid");
    let reasons: Vec<&str> = e["detail"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["reason"].as_str().unwrap())
        .collect();
    for want in [
        "out_of_range",
        "targets_length_mismatch",
        "verdict_needs_srs",
        "law_wrong_target",
    ] {
        assert!(reasons.contains(&want), "{want} not in {reasons:?}");
    }
    // A drive row the run does not name is refused (every enabled drive row is
    // an output).
    table(
        &mut c,
        &[
            "control", "control", "control", "control", "drive", "drive", "drive", "drive", "drive",
        ],
    )
    .await;
    let e = c
        .refused("mimo_shock.configure", json!({"config": config("auto")}))
        .await;
    assert!(e["detail"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["reason"] == "drive_unknown_channel" && i["channel"] == 9));
    // Unknown law and parameter.
    table(
        &mut c,
        &[
            "control", "control", "control", "control", "drive", "drive", "drive", "drive",
        ],
    )
    .await;
    let mut cfg = config("auto");
    cfg["control"] = json!({"law": {"id": "inverse", "params": {"gain": 7, "nope": 1}}});
    let e = c
        .refused("mimo_shock.configure", json!({"config": cfg}))
        .await;
    let reasons: Vec<&str> = e["detail"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["reason"].as_str().unwrap())
        .collect();
    assert!(reasons.contains(&"law_param_invalid") && reasons.contains(&"law_param_unknown"));
    let mut cfg = config("auto");
    cfg["control"] = json!({"law": {"id": "no_such_law"}});
    let e = c
        .refused("mimo_shock.configure", json!({"config": cfg}))
        .await;
    assert_eq!(e["detail"]["errors"][0]["reason"], "law_unknown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_an_observer_may_read_and_stop_but_not_drive() {
    let addr = serve(svc()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let mut o = Client::connect(addr).await;
    // A dry run needs no control and returns the target views.
    let r = o
        .ok(
            "mimo_shock.configure",
            json!({"config": config("auto"), "dry_run": true}),
        )
        .await;
    assert!(r["errors"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(r["views"].as_array().unwrap().len(), 4);
    assert_eq!(
        reason(
            &mut o,
            "mimo_shock.configure",
            json!({"config": config("auto")})
        )
        .await,
        "not_controller"
    );
    c.ok("mimo_shock.configure", json!({"config": config("auto")}))
        .await;
    for m in [
        "mimo_shock.arm",
        "mimo_shock.start",
        "mimo_shock.reset",
        "mimo_shock.clear",
    ] {
        assert_eq!(reason(&mut o, m, json!({})).await, "not_controller", "{m}");
    }
    assert_eq!(
        o.ok("mimo_shock.state", json!({})).await["state"],
        "configured"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_refusals_follow_the_run_through_its_states() {
    let addr = serve(svc()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok("mimo_shock.configure", json!({"config": config("manual")}))
        .await;
    assert_eq!(
        reason(&mut c, "mimo_shock.proceed", json!({})).await,
        "wrong_state"
    );
    assert_eq!(c.ok("mimo_shock.arm", json!({})).await["state"], "armed");
    assert_eq!(
        reason(&mut c, "mimo_shock.arm", json!({})).await,
        "run_active"
    );
    // Another family is refused while this run is live.
    let e = c
        .refused(
            "shock.configure",
            json!({"config": {"pulse": {"shape": "half_sine", "amplitude_g": 1, "duration_ms": 11},
                              "schedule": {"mode": "auto", "levels_db": [0], "pulses": [1]},
                              "limits": {"drive_vpk_ceiling": 1, "drive_vpk_abort": 2}}}),
        )
        .await;
    assert_eq!(e["reason"], "run_active", "{e}");
    assert_eq!(
        reason(
            &mut c,
            "mimo_shock.configure",
            json!({"config": config("manual")})
        )
        .await,
        "run_active"
    );
    c.ok("mimo_shock.start", json!({})).await;
    let s = ms_state(&mut c, "review").await;
    assert!(s["identification"]["passed"] == true, "{s}");
    assert_eq!(
        reason(&mut c, "mimo_shock.fire", json!({})).await,
        "wrong_state"
    );
    assert_eq!(
        reason(&mut c, "mimo_shock.proceed", json!({"acknowledge": ""})).await,
        "bad_params"
    );
    c.ok("mimo_shock.proceed", json!({})).await;
    c.ok("mimo_shock.fire", json!({})).await;
    c.wait_for("the pulse", LONG, |e| e.topic() == "mimo_shock.pulse")
        .await;
    assert_eq!(
        reason(&mut c, "mimo_shock.fire", json!({})).await,
        "not_yet"
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    c.ok("mimo_shock.fire", json!({})).await;
    c.ok("mimo_shock.stop", json!({})).await;
    let s = ms_state(&mut c, "finished").await;
    assert_eq!(s["end"]["outcome"], "operator_stop");
    assert_eq!(
        reason(&mut c, "mimo_shock.arm", json!({})).await,
        "run_finished"
    );
    assert_eq!(
        c.ok("mimo_shock.reset", json!({})).await["state"],
        "configured"
    );
    assert_eq!(
        c.ok("mimo_shock.clear", json!({})).await["state"],
        "unconfigured"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_auto_refuses_the_manual_fire_and_shutdown_refuses_the_drive() {
    let service = svc();
    let addr = serve(service.clone()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok("mimo_shock.configure", json!({"config": config("auto")}))
        .await;
    c.ok("mimo_shock.arm", json!({})).await;
    c.ok("mimo_shock.start", json!({})).await;
    ms_state(&mut c, "review").await;
    c.ok("mimo_shock.proceed", json!({})).await;
    assert_eq!(
        reason(&mut c, "mimo_shock.fire", json!({})).await,
        "wrong_mode"
    );
    let said = service.shutdown().await;
    assert!(said.is_some_and(|s| s.contains("ms-0001")));
    for m in [
        "mimo_shock.arm",
        "mimo_shock.start",
        "mimo_shock.proceed",
        "mimo_shock.fire",
    ] {
        assert_eq!(reason(&mut c, m, json!({})).await, "shutting_down", "{m}");
    }
    let s = c.ok("mimo_shock.state", json!({})).await;
    assert_eq!(s["end"]["outcome"], "safe_stop", "{s}");
    assert_eq!(s["end"]["reason"], "shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_the_simulator_refuses_a_table_not_the_plants_size() {
    let addr = serve(svc()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let mut cfg = config("auto");
    cfg["simulator"] = json!({"plant": "shock-jointed-6x3"});
    c.ok("mimo_shock.configure", json!({"config": cfg})).await;
    let e = c.refused("mimo_shock.arm", json!({})).await;
    assert_eq!(e["reason"], "arm_refused", "{e}");
    assert_eq!(e["detail"]["plant"], "shock-jointed-6x3");
    let mut cfg = config("auto");
    cfg["simulator"] = json!({"plant": "no-such-plant"});
    let e = c
        .refused("mimo_shock.configure", json!({"config": cfg}))
        .await;
    assert_eq!(e["detail"]["errors"][0]["reason"], "plant_unknown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_the_law_list_names_the_builtins_first_with_their_targets() {
    let addr = serve(svc()).await;
    let mut c = Client::connect(addr).await;
    let r = c.ok("laws.list", json!({"world": "shock_mimo"})).await;
    let laws = r["laws"].as_array().unwrap();
    let ids: Vec<&str> = laws.iter().map(|l| l["id"].as_str().unwrap()).collect();
    assert_eq!(&ids[..3], ["inverse", "shape", "paired"]);
    assert!(laws.iter().all(|l| l["world"] == "shock_mimo"));
    assert_eq!(laws[1]["targets"], json!(["srs"]));
    assert_eq!(laws[0]["targets"], json!(["waveform", "srs"]));
    assert!(laws[0]["params"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["key"] == "gain"));
    // Every world lists them too.
    let all = c.ok("laws.list", json!({})).await;
    assert!(all["laws"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["world"] == "shock_mimo" && l["id"] == "paired"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_scaled_aborts_are_a_setting_and_their_absence_is_stated() {
    let addr = serve(svc()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let s = c
        .ok("mimo_shock.configure", json!({"config": config("auto")}))
        .await["state"]
        .clone();
    assert_eq!(s["scaled_aborts"], true);
    assert!(s["notice"].is_null());
    let mut cfg = config("auto");
    cfg["control"] = json!({"scaled_aborts": false});
    let s = c.ok("mimo_shock.configure", json!({"config": cfg})).await["state"].clone();
    assert_eq!(s["scaled_aborts"], false);
    assert!(s["notice"]
        .as_str()
        .unwrap()
        .contains("Level-scaled aborts are off"));
    let mut cfg = config("auto");
    cfg["control"] = json!({"scaled_aborts": "no"});
    assert_eq!(
        reason(&mut c, "mimo_shock.configure", json!({"config": cfg})).await,
        "bad_params"
    );
    // Not while a run is live.
    c.ok("mimo_shock.arm", json!({})).await;
    assert_eq!(
        reason(
            &mut c,
            "mimo_shock.configure",
            json!({"config": config("auto")})
        )
        .await,
        "run_active"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_a_band_that_eats_the_velocity_tolerance_is_refused_and_the_default_keeps_it() {
    // The NI bench, 2026-11-11: an 11 ms 516 half-sine inside a 5 Hz to 2.5 kHz
    // band is -9 % of the ideal pulse's dV before anything is played, against
    // the 10 % tolerance, and the ladder stopped at its first level. The run is
    // refused by name before it arms; the default band for a classical target
    // starts low enough to keep the target's own margin.
    let addr = serve(svc()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let errors = |v: &Value| -> Vec<Value> { v["errors"].as_array().cloned().unwrap_or_default() };
    let mut cfg = config("auto");
    cfg["acquisition"]["band_hz"] = json!([5, 2500]);
    let dry = c
        .ok(
            "mimo_shock.configure",
            json!({"config": cfg, "dry_run": true}),
        )
        .await;
    let cut: Vec<Value> = errors(&dry)
        .into_iter()
        .filter(|i| i["reason"] == "band_cuts_velocity_change")
        .collect();
    assert_eq!(cut.len(), 4, "{dry}");
    assert!(
        cut[0]["message"].as_str().unwrap().contains("5 Hz"),
        "{}",
        cut[0]
    );
    let cfg = config("auto");
    let dry = c
        .ok(
            "mimo_shock.configure",
            json!({"config": cfg, "dry_run": true}),
        )
        .await;
    assert!(errors(&dry).is_empty(), "{dry}");
}
