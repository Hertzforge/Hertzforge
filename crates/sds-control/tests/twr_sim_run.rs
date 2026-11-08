//! TWR end to end through the wire on the controller's own service: the real
//! `TwrService`, unpaced, on the control service's simulated station, through a
//! loopback WebSocket client (the plan's sim-1 to sim-5 and sim-9). Judged on the
//! service's own stamps (`t_s`, `start_frame`, `end.at_s`), never on when a frame
//! arrived.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::{Service, ServiceOptions};
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::time::Duration;
use support::schema::Schema;
use support::{options, serve, Client};

const LONG: Duration = Duration::from_secs(120);

fn opts(speed: f64, timeout: Duration) -> ServiceOptions {
    let mut o = options(
        FixedScanner(Err(StationBuildError::NiDriverUnavailable(
            "not installed on this test host".into(),
        ))),
        speed,
        timeout,
        Duration::from_secs(5),
    );
    // The controller's own TWR service, not the simulated engine.
    o.twr_engine = None;
    o
}

fn row(index: u32, role: &str) -> Value {
    let units = if role == "drive" { "V" } else { "g" };
    json!({
        "index": index, "enabled": true, "description": role,
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": if units == "V" { 1000 } else { 100 },
        "range_max": if units == "V" { 10 } else { 100 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

async fn controller(c: &mut Client) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": [row(1, "control"), row(2, "drive")]}),
    )
    .await;
}

/// A field-like reference: three decaying tones over a low floor, 1 s at 12800 Hz.
async fn import(c: &mut Client) -> Value {
    let fs = 12_800.0;
    let mut text = String::new();
    for i in 0..12_800 {
        let t = i as f64 / fs;
        let tone = |f: f64, tau: f64, a: f64| {
            a * (-t / tau).exp() * (2.0 * std::f64::consts::PI * f * t).sin()
        };
        let v = tone(150.0, 0.3, 0.4) + tone(420.0, 0.2, 0.3) + tone(900.0, 0.1, 0.2);
        text.push_str(&format!("{v}\n"));
    }
    c.ok(
        "twr.reference.import",
        json!({"text": text, "fs_hz": fs, "name": "field"}),
    )
    .await["reference"]["sha256"]
        .clone()
}

fn config(sha: &Value, mode: &str) -> Value {
    json!({
        "title": "Wire TWR",
        "reference": {"sha256": sha},
        "identification": {"drive_vrms": 0.1, "duration_s": 2, "review": false},
        "schedule": {"mode": mode, "levels_db": [-12, -6, 0]},
        "run": {"schedule": "loop", "count": 2},
        "limits": {"drive_vpk_ceiling": 5}
    })
}

async fn wait_twr(c: &mut Client, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let i = c
        .wait_for(what, LONG, |e| e.topic() == "twr.state" && pred(e.data()))
        .await;
    c.events[i].data().clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the TWR core: the law seam under control and its guards"]
async fn sim_1_an_auto_run_completes_with_every_level_gated() {
    let schema = Schema::load();
    let addr = serve(Service::with_options(opts(0.0, Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let sha = import(&mut c).await;
    c.ok("twr.configure", json!({"config": config(&sha, "auto")}))
        .await;
    c.ok("twr.start", json!({})).await;
    let end = wait_twr(&mut c, "finished", |d| d["state"] == "finished").await;
    schema.assert_valid(&end, Some("twrState"));
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    let plays = c.ok("twr.plays", json!({})).await;
    let rows = plays["plays"].as_array().unwrap();
    for level in [-12.0, -6.0, 0.0] {
        assert!(
            rows.iter()
                .any(|r| r["level_db"] == level && r["gated"] == true && r["kind"] == "iteration"),
            "{level} dB gated"
        );
    }
    let playback: Vec<&Value> = rows.iter().filter(|r| r["kind"] == "playback").collect();
    assert_eq!(playback.len(), 2);
    assert!(playback
        .iter()
        .all(|r| r["rms_error_pct"].as_f64().unwrap() <= 10.0));
    // Station time: each play starts where the last ended, and t_s is that clock.
    assert!(rows
        .windows(2)
        .all(|w| w[1]["start_frame"].as_u64().unwrap()
            == w[0]["start_frame"].as_u64().unwrap() + w[0]["frames"].as_u64().unwrap()));
    let last = rows.last().unwrap();
    let frames = last["start_frame"].as_u64().unwrap() + last["frames"].as_u64().unwrap();
    assert!((end["t_s"].as_f64().unwrap() - frames as f64 / 12_800.0).abs() < 1e-3);
    assert_eq!(end["last_play"], *last);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the TWR core: the law seam under control and its guards"]
async fn sim_2_manual_verbs_then_a_stop_inside_a_play() {
    let addr = serve(Service::with_options(opts(1.0, Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let sha = import(&mut c).await;
    c.ok("twr.configure", json!({"config": config(&sha, "manual")}))
        .await;
    c.ok("twr.start", json!({})).await;
    wait_twr(&mut c, "waiting", |d| d["phase"] == "waiting").await;
    for n in 1..=2 {
        c.ok("twr.iterate", json!({})).await;
        wait_twr(&mut c, "a play", |d| {
            d["plays"]["done"] == n && d["phase"] == "waiting"
        })
        .await;
    }
    for want in [-11, -10, -9] {
        assert_eq!(
            c.ok("twr.level", json!({"delta_db": 1})).await["level_db"],
            want
        );
    }
    assert_eq!(c.ok("twr.level", json!({"rung": 2})).await["level_db"], 0);
    assert_eq!(
        c.refused("twr.play", json!({})).await["reason"],
        "not_converged"
    );
    let mut n = 3;
    loop {
        c.ok("twr.iterate", json!({})).await;
        let s = wait_twr(&mut c, "a play", |d| {
            d["plays"]["done"] == n && d["phase"] == "waiting"
        })
        .await;
        if s["last_play"]["gated"] == true {
            break;
        }
        n += 1;
        assert!(n < 12, "0 dB did not gate");
    }
    c.ok("twr.play", json!({})).await;
    c.ok("twr.freeze", json!({"frozen": true})).await;
    c.wait_for("into a playback play", LONG, |e| {
        e.topic() == "twr.live"
            && e.data()["play"]["kind"] == "playback"
            && e.data()["play"]["frames_done"].as_u64().unwrap_or(0) > 2000
    })
    .await;
    c.ok("twr.stop", json!({})).await;
    let end = wait_twr(&mut c, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_stop", "{end}");
    assert!(end["end"]["frames_done"].as_u64().unwrap() > 2000);
    let rows = c.ok("twr.plays", json!({})).await["next"].as_u64().unwrap();
    assert_eq!(end["end"]["play"], rows, "the cancelled play has no row");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the TWR core: the law seam under control and its guards"]
async fn sim_3_an_observer_aborts_inside_a_play_within_a_block() {
    let addr = serve(Service::with_options(opts(1.0, Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    let sha = import(&mut c).await;
    c.ok("twr.configure", json!({"config": config(&sha, "auto")}))
        .await;
    c.ok("twr.start", json!({})).await;
    obs.wait_for("into a play", LONG, |e| {
        e.topic() == "twr.live" && e.data()["play"]["frames_done"].as_u64().unwrap_or(0) > 2000
    })
    .await;
    obs.ok("twr.abort", json!({})).await;
    let end = wait_twr(&mut obs, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_abort", "{end}");
    assert!(end["end"]["frames_done"].as_u64().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the TWR core: the predicted full-level drive at proceed"]
async fn sim_4_a_ceiling_under_the_full_level_drive_ends_auto_and_refuses_manual() {
    let addr = serve(Service::with_options(opts(0.0, Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let sha = import(&mut c).await;
    let mut cfg = config(&sha, "auto");
    cfg["limits"]["drive_vpk_ceiling"] = json!(0.3);
    cfg["identification"]["drive_vrms"] = json!(0.05);
    c.ok("twr.configure", json!({"config": cfg.clone()})).await;
    c.ok("twr.start", json!({})).await;
    let end = wait_twr(&mut c, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "aborted", "{end}");
    assert_eq!(end["end"]["trip"]["kind"], "drive_vpk");
    assert_eq!(end["end"]["trip"]["predicted"], true);
    assert_eq!(
        c.ok("twr.plays", json!({})).await["next"],
        0,
        "no play after identification"
    );
    cfg["schedule"]["mode"] = json!("manual");
    c.ok("twr.configure", json!({"config": cfg})).await;
    c.ok("twr.start", json!({})).await;
    wait_twr(&mut c, "waiting", |d| {
        d["phase"] == "waiting" && d["plays"]["done"] == 0
    })
    .await;
    let e = c.refused("twr.level", json!({"rung": 2})).await;
    assert_eq!(e["reason"], "drive_ceiling", "{e}");
    assert!(e["detail"]["vpk"].as_f64().unwrap() > 0.3);
    assert_eq!(e["detail"]["ceiling_vpk"], 0.3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the TWR core: the law seam under control and its guards"]
async fn sim_5_a_closed_controller_and_a_shutdown_each_end_in_the_safe_stop() {
    let svc = Service::with_options(opts(1.0, Duration::from_millis(2000)));
    let addr = serve(svc.clone()).await;
    let mut obs = Client::connect(addr).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let sha = import(&mut c).await;
    c.ok("twr.configure", json!({"config": config(&sha, "auto")}))
        .await;
    c.ok("twr.start", json!({})).await;
    obs.wait_for("a play", LONG, |e| {
        e.topic() == "twr.state" && e.data()["plays"]["done"] == 1
    })
    .await;
    drop(c);
    let end = wait_twr(&mut obs, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "safe_stop", "{end}");

    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok("twr.configure", json!({"config": config(&sha, "auto")}))
        .await;
    c.ok("twr.start", json!({})).await;
    c.wait_for("a play of the second run", LONG, |e| {
        e.topic() == "twr.state" && e.data()["plays"]["done"] == 1
    })
    .await;
    let said = svc.shutdown().await;
    assert!(said.is_some_and(|s| s.contains("safe_stop")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the TWR core: the review hold in the core"]
async fn sim_9_a_drive_too_low_for_the_coherence_holds_until_reset() {
    let addr = serve(Service::with_options(opts(0.0, Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let sha = import(&mut c).await;
    let mut cfg = config(&sha, "auto");
    cfg["identification"]["drive_vrms"] = json!(0.000001);
    c.ok("twr.configure", json!({"config": cfg})).await;
    c.ok("twr.start", json!({})).await;
    let held = wait_twr(&mut c, "held", |d| d["state"] == "held").await;
    assert_eq!(held["phase"], "coherence_hold");
    assert_eq!(
        c.refused("twr.proceed", json!({})).await["reason"],
        "coherence_hold"
    );
    assert_eq!(c.ok("twr.reset", json!({})).await["state"], "configured");
}
