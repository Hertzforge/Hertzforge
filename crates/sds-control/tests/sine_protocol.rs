//! Sine through the protocol, on a real loopback WebSocket: configure and its dry
//! run, the configuration and its views, reset and clear, the controller rules,
//! the refusals the service gives before the run thread is asked (`not_a_sweep`,
//! `out_of_band`, each `adjust_refused` reason the law's declaration settles), the
//! subscriptions, the run exclusion against SRS shock, and the shutdown. The run
//! itself on the controller is `sine_sim_run.rs`.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::time::Duration;
use support::schema::Schema;
use support::{options, serve, Client};

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

/// A control row, a fixture accelerometer and a drive row.
fn table() -> Value {
    json!([
        row(1, "control", "Control"),
        row(2, "monitor", "Fixture"),
        row(3, "drive", "Shaker")
    ])
}

/// A flat 0.2 g sweep, 100 to 400 Hz, on the simulator's rate.
fn config(test: Value) -> Value {
    json!({
        "title": "Protocol Sine",
        "profile": {"rows": [{"freq_hz": 50, "quantity": "accel_g", "value": 0.2},
                             {"freq_hz": 1000, "quantity": "accel_g", "value": 0.2}]},
        "test": test,
        "schedule": {"levels_db": [-6, 0], "per_level": {"seconds": 2},
                     "full_level": {"seconds": 2}},
        "identification": {"review": true},
        "limits": {"drive_vpk_ceiling": 2, "drive_vpk_abort": 4,
                   "channels": [{"channel": 2, "profile": {"rows": [
                       {"freq_hz": 50, "quantity": "accel_g", "value": 1},
                       {"freq_hz": 1000, "quantity": "accel_g", "value": 1}]}}]}
    })
}

fn sweep() -> Value {
    json!({"kind": "sweep", "band_hz": [100, 400], "rate": {"law": "log", "oct_per_min": 8},
           "end": {"duration_s": 30}})
}

fn dwell() -> Value {
    json!({"kind": "dwell", "freq_hz": 250, "end": {"seconds": 30}})
}

async fn controller(c: &mut Client) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": table()}),
    )
    .await;
}

fn service() -> std::sync::Arc<Service> {
    Service::with_options(options(
        no_driver(),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn configure_its_dry_run_the_configuration_reset_and_clear() {
    let schema = Schema::load();
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    assert_eq!(c.ok("sine.state", json!({})).await["state"], "unconfigured");
    // A dry run is open to an observer and carries the three profile views.
    let r = obs
        .call(
            "sine.configure",
            json!({"config": config(sweep()), "dry_run": true}),
        )
        .await;
    let dry = &r["result"];
    assert!(dry["errors"].as_array().unwrap().is_empty(), "{dry}");
    assert_eq!(dry["derived"]["grid"]["fs_hz"], 16384);
    assert!(
        obs.decoded_response(&r["id"]).is_some(),
        "the views travel as a binary frame"
    );
    schema.assert_valid(&r, None);
    // Configuring needs control.
    assert_eq!(
        obs.refused("sine.configure", json!({"config": config(sweep())}))
            .await["reason"],
        "not_controller"
    );
    let r = c
        .ok("sine.configure", json!({"config": config(sweep())}))
        .await;
    schema.assert_valid(&r, Some("sineConfigureResult"));
    assert_eq!(r["state"]["state"], "configured");
    assert_eq!(r["state"]["run_id"], "sine-0001");
    assert_eq!(r["state"]["law"]["live"], json!({"blend_w": 0.5}));
    assert_eq!(r["derived"]["law"]["live"], json!(["blend_w"]));
    // The configuration with its views, to every connection.
    let r = obs.call("sine.config", json!({})).await;
    assert_eq!(r["result"]["run_id"], "sine-0001");
    assert_eq!(r["result"]["config"]["title"], "Protocol Sine");
    let views = obs.decoded_response(&r["id"]).unwrap().expanded();
    let titles: Vec<&str> = views["result"]["views"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Profile", "Velocity", "Displacement"]);
    // Content errors are items; the run is not replaced.
    let mut bad = config(sweep());
    bad["limits"]["drive_vpk_abort"] = json!(1);
    let e = c.refused("sine.configure", json!({"config": bad})).await;
    assert_eq!(e["reason"], "config_invalid");
    assert_eq!(e["detail"]["errors"][0]["reason"], "abort_below_ceiling");
    assert_eq!(e["detail"]["errors"][0]["field"], "limits.drive_vpk_abort");
    schema.assert_valid(&e["detail"], Some("report"));
    // A wrong shape names the field.
    let mut shape = config(sweep());
    shape["test"]["kind"] = json!("random");
    let e = c.refused("sine.configure", json!({"config": shape})).await;
    assert_eq!(e["reason"], "bad_params");
    assert_eq!(e["detail"]["param"], "config.test.kind");
    // Reset configures the present configuration again; clear forgets it.
    let s = c.ok("sine.reset", json!({})).await;
    assert_eq!(s["run_id"], "sine-0002");
    assert_eq!(s["state"], "configured");
    let s = c.ok("sine.clear", json!({})).await;
    assert_eq!(s["state"], "unconfigured");
    schema.assert_valid(&s, Some("sineState"));
    assert_eq!(
        c.refused("sine.start", json!({})).await["reason"],
        "not_configured"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_sweep_verbs_and_the_adjustments_are_refused_by_name_before_the_run_is_asked() {
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok("sine.configure", json!({"config": config(dwell())}))
        .await;
    for (m, p) in [
        ("sine.reverse", json!({})),
        ("sine.jog", json!({"delta_hz": 5})),
        ("sine.goto", json!({"hz": 300})),
    ] {
        let e = c.refused(m, p.clone()).await;
        assert_eq!(e["reason"], "not_a_sweep", "{m}");
        assert_eq!(e["detail"]["test"], "dwell", "{m}");
        // An observer is refused control first.
        assert_eq!(obs.refused(m, p).await["reason"], "not_controller", "{m}");
    }
    c.ok("sine.configure", json!({"config": config(sweep())}))
        .await;
    let e = c.refused("sine.goto", json!({"hz": 5000})).await;
    assert_eq!(e["reason"], "out_of_band");
    assert_eq!(e["detail"]["band_hz"], json!([100, 400]));
    for (key, value, why) in [
        ("kp", json!(0.4), "not_live"),
        ("blend_w", json!(2), "out_of_range"),
        ("no_such_key", json!(1), "unknown_key"),
    ] {
        let e = c
            .refused("sine.adjust", json!({"key": key, "value": value}))
            .await;
        assert_eq!(e["reason"], "adjust_refused", "{key}");
        assert_eq!(e["detail"]["why"], why, "{key}");
        assert_eq!(e["detail"]["key"], key, "{key}");
    }
    // A live parameter in range is the run thread's to apply: before the run drives,
    // it is refused by state, never silently taken.
    let e = c
        .refused("sine.adjust", json!({"key": "blend_w", "value": 0.2}))
        .await;
    assert_eq!(e["reason"], "wrong_state");
    for (m, p) in [
        ("sine.level", json!({"delta_db": -1})),
        ("sine.hold", json!({})),
        ("sine.resume", json!({})),
        ("sine.proceed", json!({})),
    ] {
        assert_eq!(obs.refused(m, p.clone()).await["reason"], "not_controller");
        let e = c.refused(m, p).await;
        assert!(
            matches!(
                e["reason"].as_str(),
                Some("wrong_state" | "manual_mode_required")
            ),
            "{m}: {e}"
        );
    }
    // Stop and abort are open to every connection, and change nothing that is not
    // live.
    let s = obs.ok("sine.stop", json!({})).await;
    assert_eq!(s["state"], "configured");
    let s = obs.ok("sine.abort", json!({})).await;
    assert_eq!(s["state"], "configured");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subscription_parses_to_its_result_and_refuses_what_a_quantity_does_not_take() {
    let schema = Schema::load();
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    let r = c
        .ok(
            "sine.subscribe",
            json!({"rate_hz": 2, "plots": [{"id": "p", "quantity": "phase", "channels": [1]},
                                           {"id": "t", "quantity": "time"}]}),
        )
        .await;
    schema.assert_valid(&r, Some("sineSubscribeResult"));
    assert_eq!(r["plots"][1]["points"], 1024);
    assert_eq!(r["plots"][1]["channels"], "all");
    let e = c
        .refused(
            "sine.subscribe",
            json!({"plots": [{"id": "p", "quantity": "thd", "channels": [1]}]}),
        )
        .await;
    assert_eq!(e["reason"], "bad_params");
    assert_eq!(e["detail"]["param"], "plots[0].channels");
    let r = c.ok("sine.subscribe", json!({"default": true})).await;
    assert_eq!(r["default"], true);
    assert_eq!(r["plots"], Value::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_law_catalog_lists_the_sine_world_with_its_live_parameters() {
    let schema = Schema::load();
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    let r = c.ok("laws.list", json!({"world": "sine"})).await;
    schema.assert_valid(&r, Some("lawsListResult"));
    let laws = r["laws"].as_array().unwrap();
    assert_eq!(laws[0]["id"], "servo_blend");
    assert_eq!(laws[0]["live"], json!(["blend_w"]));
    let all = c.ok("laws.list", json!({})).await;
    let worlds: Vec<&str> = all["laws"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["world"].as_str().unwrap())
        .collect();
    // Sine's built-in comes after every older world's, and TWR's after it.
    assert_eq!(&worlds[worlds.len() - 2..], &["sine", "twr"]);
    assert!(all["laws"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["world"] != "sine")
        .all(|l| l.get("live").is_none()));
    assert_eq!(
        c.refused("laws.list", json!({"world": "sinus"})).await["reason"],
        "bad_params"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_srs_run_refuses_a_sine_configure_and_start() {
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok("sine.configure", json!({"config": config(sweep())}))
        .await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": [row(1, "control", "Control"),
                                             row(2, "drive", "Shaker")]}),
    )
    .await;
    c.ok(
        "srs.configure",
        json!({"config": {
            "title": "Protocol SRS", "target": {"kind": "flat", "accel_g": 50},
            "bands": {"f_lo_hz": 100, "f_hi_hz": 2000}, "acquisition": {"fs_hz": 20480},
            "schedule": {"mode": "manual", "levels_db": [0], "pulses": [1]},
            "identification": {"nfft": 1024, "review": true},
            "limits": {"drive_vpk_ceiling": 5, "drive_vpk_abort": 8}}}),
    )
    .await;
    c.ok("srs.start", json!({})).await;
    let e = c.refused("sine.start", json!({})).await;
    assert_eq!(e["reason"], "run_active");
    let e = c
        .refused("sine.configure", json!({"config": config(sweep())}))
        .await;
    assert_eq!(e["reason"], "run_active");
    c.ok("srs.abort", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_refuses_the_drive_verbs_and_leaves_hold_stop_and_abort() {
    let svc = service();
    let addr = serve(svc.clone()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok("sine.configure", json!({"config": config(sweep())}))
        .await;
    let said = svc.shutdown().await;
    assert!(said.is_none(), "nothing was live: {said:?}");
    for (m, p) in [
        ("sine.arm", json!({})),
        ("sine.start", json!({})),
        ("sine.proceed", json!({})),
        ("sine.level", json!({"delta_db": 1})),
        ("sine.mode", json!({"mode": "manual"})),
        ("sine.resume", json!({})),
        ("sine.reverse", json!({})),
        ("sine.jog", json!({"delta_hz": 1})),
        ("sine.goto", json!({"hz": 200})),
        ("sine.adjust", json!({"key": "blend_w", "value": 0.2})),
    ] {
        assert_eq!(c.refused(m, p).await["reason"], "shutting_down", "{m}");
    }
    assert_ne!(
        c.refused("sine.hold", json!({})).await["reason"],
        "shutting_down"
    );
    c.ok("sine.stop", json!({})).await;
    c.ok("sine.abort", json!({})).await;
    c.ok("sine.config", json!({})).await;
    c.ok("sine.log", json!({})).await;
}
