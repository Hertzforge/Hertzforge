//! Mixed Mode end to end on the simulator, through the protocol: a sine-on-random
//! run from start to completed with the floor first, a stop, an abort, a level
//! step by hand, a part switched off and on live, hold and resume, and a dropped
//! controller ending in the safe stop. Random-on-random and the combined test wait
//! on the core's narrowbands closed loop and are marked so.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::time::Duration;
use support::{options, serve, Client};

const LONG: Duration = Duration::from_secs(240);

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

async fn controller(c: &mut Client) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": [row(1, "control", "Control"),
                                             row(2, "drive", "Shaker")]}),
    )
    .await;
}

fn service(timeout: Duration) -> std::sync::Arc<Service> {
    paced(timeout, 0.0)
}

/// A service whose simulator runs `speed` times faster than the wall (0: unpaced).
fn paced(timeout: Duration, speed: f64) -> std::sync::Arc<Service> {
    Service::with_options(options(
        FixedScanner(Err(StationBuildError::NiDriverUnavailable(
            "not installed on this test host".into(),
        ))),
        speed,
        timeout,
        Duration::from_secs(5),
    ))
}

fn floor() -> Value {
    json!({"id": "floor", "kind": "floor",
           "breakpoints": [{"freq_hz": 20, "psd_g2_hz": 0.0005}, {"freq_hz": 1000, "slope_db_oct": 0}]})
}

fn tone(id: &str, hz: f64, g: f64) -> Value {
    json!({"id": id, "kind": "tone", "schedule": {"kind": "fixed", "freq_hz": hz}, "amplitude_g": g})
}

fn band(id: &str, schedule: Value) -> Value {
    json!({"id": id, "kind": "narrowband", "schedule": schedule, "level_g2_hz": 0.002, "width_hz": 20})
}

fn config(parts: Vec<Value>, schedule: Value) -> Value {
    json!({
        "title": "Simulated Mixed Mode",
        "parts": parts,
        "acquisition": {"fs_hz": 4096, "n_pts": 8192, "n_avg": 2, "tick_ms": 10},
        "schedule": schedule,
        "control": {"pretest": {"averages": 2}, "floor_settle_s": 10, "part_ramp_s": 1,
                    "level_ramp_s": 1, "shutdown_ramp_s": 1},
        "limits": {"drive_vpk_ceiling": 5, "drive_vpk_abort": 8}
    })
}

fn auto(full_s: f64) -> Value {
    json!({"mode": "auto", "levels": [{"db": -6, "dwell_s": 2}, {"db": 0}], "full_level_s": full_s})
}

fn manual() -> Value {
    json!({"mode": "manual", "levels": [{"db": -6, "dwell_s": 0}, {"db": 0}], "full_level_s": 0})
}

/// Configure, start, proceed at the review hold; returns at the review's end.
async fn run_to_engaged(c: &mut Client, cfg: Value) {
    let r = c.ok("mixed.configure", json!({"config": cfg})).await;
    assert_eq!(r["state"]["state"], "configured", "{r}");
    c.ok("mixed.start", json!({})).await;
    wait_state(c, "review").await;
    c.ok("mixed.proceed", json!({})).await;
    wait_phase(c, "engaged").await;
}

async fn wait_state(c: &mut Client, state: &str) -> Value {
    let i = c
        .wait_for(&format!("mixed.state {state}"), LONG, |e| {
            e.topic() == "mixed.state" && e.data()["state"] == state
        })
        .await;
    c.events[i].data().clone()
}

async fn wait_phase(c: &mut Client, phase: &str) -> Value {
    let i = c
        .wait_for(&format!("mixed.state phase {phase}"), LONG, |e| {
            e.topic() == "mixed.state" && e.data()["phase"] == phase
        })
        .await;
    c.events[i].data().clone()
}

fn phases_seen(c: &Client) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in c.of_topic("mixed.state") {
        let p = e.data()["phase"].as_str().unwrap_or("").to_owned();
        if out.last() != Some(&p) {
            out.push(p);
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sine_on_random_runs_floor_first_to_completed() {
    // Paced, so the five-a-second live frames sample the run at full level.
    let addr = serve(paced(Duration::from_secs(30), 8.0)).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    run_to_engaged(
        &mut c,
        config(vec![floor(), tone("t1", 160.0, 0.5)], auto(20.0)),
    )
    .await;
    let end = wait_state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    assert!(end["end"]["ramp_s"].as_f64().is_some(), "{end}");
    // The end says the level the test ran at, not where the ramp down left it.
    assert_eq!(end["end"]["level_db"], 0, "{end}");
    let seen = phases_seen(&c);
    let at = |p: &str| seen.iter().position(|s| s == p);
    assert!(
        at("floor_only").is_some() && at("floor_only") < at("engaged"),
        "the floor comes first: {seen:?}"
    );
    // Every live frame is this run's; the tone is tracked at full level.
    let lives = c.of_topic("mixed.live");
    assert!(!lives.is_empty());
    assert!(lives.iter().all(|e| e.data()["run_id"] == "mixed-0001"));
    let at_full: Vec<f64> = lives
        .iter()
        .filter(|e| matches!(e.data()["phase"].as_str(), Some("engaged" | "full_level")))
        .filter_map(|e| {
            let p = e.data()["parts"]
                .as_array()?
                .iter()
                .find(|p| p["id"] == "t1")?;
            if p["settled"] == true {
                p["error_db"].as_f64()
            } else {
                None
            }
        })
        .collect();
    let last = (
        phases_seen(&c),
        lives
            .iter()
            .map(|e| {
                (
                    e.data()["parts_s"].clone(),
                    e.data()["level_db"].clone(),
                    e.data()["phase"].clone(),
                )
            })
            .collect::<Vec<_>>(),
    );
    assert!(!at_full.is_empty(), "no settled tone reading: {last:?}");
    let worst = at_full.iter().fold(0.0f64, |a, b| a.max(b.abs()));
    assert!(
        worst <= 1.5,
        "the settled tone within 1.5 dB: worst {worst}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_auto_ladder_reaches_full_level() {
    let addr = serve(paced(Duration::from_secs(30), 8.0)).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    run_to_engaged(
        &mut c,
        config(vec![floor(), tone("t1", 160.0, 0.5)], auto(10.0)),
    )
    .await;
    let full = wait_phase(&mut c, "full_level").await;
    assert_eq!(full["level_db"], 0, "{full}");
    let end = wait_state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_ramps_every_part_down_and_ends_operator_stop() {
    let addr = serve(service(Duration::from_secs(30))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    run_to_engaged(
        &mut c,
        config(vec![floor(), tone("t1", 160.0, 0.5)], auto(60.0)),
    )
    .await;
    c.ok("mixed.stop", json!({})).await;
    let end = wait_state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_stop", "{end}");
    assert!(end["end"]["ramp_s"].as_f64().is_some(), "{end}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_abort_ends_operator_abort_without_the_ramp() {
    let addr = serve(service(Duration::from_secs(30))).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    run_to_engaged(
        &mut c,
        config(vec![floor(), tone("t1", 160.0, 0.5)], auto(60.0)),
    )
    .await;
    // Any screen may abort.
    obs.ok("mixed.abort", json!({})).await;
    let end = wait_state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_abort", "{end}");
    assert!(end["end"]["ramp_s"].is_null(), "{end}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_level_hold_and_a_part_switched_live_reach_the_core() {
    let addr = serve(service(Duration::from_secs(30))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    run_to_engaged(
        &mut c,
        config(
            vec![floor(), tone("t1", 160.0, 0.5), tone("t2", 320.0, 0.25)],
            manual(),
        ),
    )
    .await;
    let r = c.ok("mixed.level", json!({"to_db": 0})).await;
    assert_eq!(r["level_db"], 0);
    assert_eq!(
        c.refused("mixed.level", json!({"to_db": -3})).await["reason"],
        "not_a_rung"
    );
    let s = c.ok("mixed.hold", json!({})).await;
    assert_eq!(s["held"], true, "{s}");
    let s = c.ok("mixed.resume", json!({})).await;
    assert_eq!(s["held"], false, "{s}");
    let r = c.ok("mixed.part", json!({"id": "t2", "on": false})).await;
    assert_eq!(r, json!({"id": "t2", "on": false}));
    let i = c
        .wait_for("t2 off", LONG, |e| {
            e.topic() == "mixed.state"
                && e.data()["parts"]
                    .as_array()
                    .and_then(|ps| ps.iter().find(|p| p["id"] == "t2"))
                    .is_some_and(|p| p["on"] == false)
        })
        .await;
    assert!(i > 0);
    // Asking for the state a part is in answers with it, unchanged.
    let r = c.ok("mixed.part", json!({"id": "t2", "on": false})).await;
    assert_eq!(r["on"], false);
    c.ok("mixed.part", json!({"id": "t2", "on": true})).await;
    let e = c
        .refused("mixed.adjust", json!({"key": "kp", "value": 0.4}))
        .await;
    assert_eq!(e["detail"]["why"], "not_live");
    let r = c
        .ok("mixed.adjust", json!({"key": "blend_w", "value": 0.6}))
        .await;
    assert_eq!(r["value"], 0.6);
    c.ok("mixed.stop", json!({})).await;
    let end = wait_state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_stop", "{end}");
    let log = c.ok("mixed.log", json!({})).await;
    let parts: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "part")
        .filter_map(|e| e["text"].as_str())
        .collect();
    assert!(
        parts.iter().any(|t| t.contains("t2") && t.contains("off")),
        "{parts:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_band_switched_off_live_ramps_out_and_back() {
    let addr = serve(service(Duration::from_secs(30))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    run_to_engaged(
        &mut c,
        config(
            vec![
                floor(),
                band("b1", json!({"kind": "fixed", "freq_hz": 300})),
            ],
            manual(),
        ),
    )
    .await;
    let band_on = |want: bool| {
        move |e: &support::Event| {
            e.topic() == "mixed.state"
                && e.data()["parts"]
                    .as_array()
                    .and_then(|ps| ps.iter().find(|p| p["id"] == "b1"))
                    .is_some_and(|p| p["on"] == want)
        }
    };
    let r = c.ok("mixed.part", json!({"id": "b1", "on": false})).await;
    assert_eq!(r["on"], false);
    // A band's samples are synthesized ahead, so its switch lands about a quarter
    // second of station time late: the wait allows for it.
    c.wait_for("b1 off", LONG, band_on(false)).await;
    c.ok("mixed.part", json!({"id": "b1", "on": true})).await;
    c.wait_for("b1 on again", LONG, band_on(true)).await;
    c.ok("mixed.stop", json!({})).await;
    let end = wait_state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_stop", "{end}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_random_parts_rms_abort_trips_and_names_no_part() {
    let addr = serve(service(Duration::from_secs(30))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let mut cfg = config(vec![floor(), tone("t1", 160.0, 0.5)], auto(20.0));
    cfg["control"]["drive_ceiling_vrms"] = json!(0.02);
    cfg["control"]["drive_abort_vrms"] = json!(0.02);
    run_to_engaged_or_end(&mut c, cfg).await;
    let end = wait_state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "aborted", "{end}");
    assert_eq!(end["end"]["trip"]["kind"], "random_vrms", "{end}");
    assert!(end["end"]["trip"]["part"].is_null(), "{end}");
}

/// As [`run_to_engaged`], but the run may end before it engages.
async fn run_to_engaged_or_end(c: &mut Client, cfg: Value) {
    let r = c.ok("mixed.configure", json!({"config": cfg})).await;
    assert_eq!(r["state"]["state"], "configured", "{r}");
    c.ok("mixed.start", json!({})).await;
    wait_state(c, "review").await;
    c.ok("mixed.proceed", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recording_runs_with_the_test_and_says_where_it_is() {
    let addr = serve(service(Duration::from_secs(30))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let mut cfg = config(vec![floor(), tone("t1", 160.0, 0.5)], auto(4.0));
    cfg["capture"] = json!({"rule": "entire_test", "run_name": "mixed_sim_capture"});
    run_to_engaged(&mut c, cfg).await;
    let end = wait_state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    let log = c.ok("mixed.log", json!({})).await;
    let rec: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["text"].as_str())
        .filter(|t| t.starts_with("recording to"))
        .collect();
    assert_eq!(rec.len(), 1, "{log}");
    let dir = std::path::PathBuf::from(rec[0].trim_start_matches("recording to "));
    assert!(
        dir.ends_with("mixed_sim_capture") || dir.to_string_lossy().contains("mixed_sim_capture"),
        "{dir:?}"
    );
    assert!(
        dir.join("law.json").exists() || dir.parent().is_some_and(|p| p.join("law.json").exists()),
        "{dir:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_controller_ends_in_the_safe_stop() {
    let addr = serve(service(Duration::from_secs(2))).await;
    let mut obs = Client::connect(addr).await;
    {
        let mut c = Client::connect(addr).await;
        controller(&mut c).await;
        run_to_engaged(
            &mut c,
            config(vec![floor(), tone("t1", 160.0, 0.5)], manual()),
        )
        .await;
    }
    let i = obs
        .wait_for("the safe stop", LONG, |e| {
            e.topic() == "mixed.state" && e.data()["state"] == "finished"
        })
        .await;
    assert_eq!(obs.events[i].data()["end"]["outcome"], "safe_stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn random_on_random_runs_to_completed() {
    let addr = serve(service(Duration::from_secs(30))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    run_to_engaged(
        &mut c,
        config(
            vec![
                floor(),
                band("b1", json!({"kind": "fixed", "freq_hz": 300})),
            ],
            auto(6.0),
        ),
    )
    .await;
    let end = wait_state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_combined_test_runs_to_completed() {
    let addr = serve(service(Duration::from_secs(30))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    run_to_engaged(
        &mut c,
        config(
            vec![
                floor(),
                tone("t1", 160.0, 0.5),
                band(
                    "b1",
                    json!({"kind": "sweep", "band_hz": [400, 600],
                           "rate": {"law": "linear", "hz_per_s": 10}, "end": null}),
                ),
            ],
            auto(6.0),
        ),
    )
    .await;
    let end = wait_state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
}

/// At a level below full, the composite's reference is the reference at that
/// level (the level the Control view's reference is drawn at), and every
/// enabled input, a monitor too, has an rms in the live frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mixed_total_reference_is_at_the_level_and_every_input_has_its_rms() {
    let addr = serve(service(Duration::from_secs(30))).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": [row(1, "control", "Control"),
                                             row(2, "drive", "Shaker"),
                                             row(3, "monitor", "Fixture")]}),
    )
    .await;
    let r = c
        .ok(
            "mixed.configure",
            json!({"config": config(vec![floor(), tone("t1", 160.0, 0.5)], manual())}),
        )
        .await;
    let full = r["state"]["total"]["reference_rms_g"]
        .as_f64()
        .expect("full-level reference");
    c.ok("mixed.start", json!({})).await;
    wait_state(&mut c, "review").await;
    c.ok("mixed.proceed", json!({})).await;
    wait_phase(&mut c, "engaged").await;
    c.pump(Duration::from_secs(3)).await;
    let lives = c.of_topic("mixed.live");
    let last = lives
        .iter()
        .rev()
        .map(|e| e.data())
        .find(|d| d["phase"] == "engaged" && d["level_db"] == -6)
        .expect("a live frame at -6 dB");
    let at = last["total"]["reference_rms_g"].as_f64().unwrap();
    let want = full * 10f64.powf(-6.0 / 20.0);
    assert!(
        (at / want - 1.0).abs() < 2e-3,
        "reference {at} at -6 dB, want {want} (full {full})"
    );
    let chans = last["channels"].as_array().unwrap();
    assert_eq!(chans.len(), 2, "{chans:?}");
    for ch in chans {
        assert!(ch["rms"].as_f64().is_some_and(|x| x > 0.0), "no rms: {ch}");
    }
    c.ok("mixed.abort", json!({})).await;
}
