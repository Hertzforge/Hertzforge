//! Sine on the controller's own `SineService`, on the control service's simulated
//! station, through a WebSocket client: the end-to-end rows sim-1 to sim-8 of the
//! Sine plan. Sweep rates and holds are judged on the service's own station time
//! (`t_s` in `sine.live`), never on when a frame arrived.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use support::{options, serve, Client};

const LONG: Duration = Duration::from_secs(120);

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

fn service(speed: f64, timeout: Duration) -> std::sync::Arc<Service> {
    Service::with_options(options(
        FixedScanner(Err(StationBuildError::NiDriverUnavailable(
            "not installed on this test host".into(),
        ))),
        speed,
        timeout,
        Duration::from_secs(5),
    ))
}

/// A flat 0.2 g profile, 20 to 2000 Hz, with `test`; no review hold.
fn config(test: Value, schedule: Value) -> Value {
    json!({
        "title": "Simulated Sine",
        "profile": {"rows": [{"freq_hz": 20, "quantity": "accel_g", "value": 0.2},
                             {"freq_hz": 2000, "quantity": "accel_g", "value": 0.2}]},
        "test": test,
        "schedule": schedule,
        "identification": {"review": false},
        "limits": {"drive_vpk_ceiling": 5, "drive_vpk_abort": 8}
    })
}

/// sim-1's sweep: 50 to 800 Hz at 8 oct/min, one sweep at each of -12, -6, 0 dB.
fn sim1() -> Value {
    config(
        json!({"kind": "sweep", "band_hz": [50, 800], "rate": {"law": "log", "oct_per_min": 8},
               "start_hz": 50, "direction": "up", "end": {"sweeps": 3}}),
        json!({"mode": "auto", "levels_db": [-12, -6, 0], "per_level": {"sweeps": 1},
               "full_level": {"sweeps": 1}}),
    )
}

/// The `sine.live` frames seen so far, as their numbers.
fn lives(c: &Client) -> Vec<Value> {
    c.of_topic("sine.live")
        .iter()
        .map(|e| e.data().clone())
        .collect()
}

async fn finished(c: &mut Client) -> Value {
    let i = c
        .wait_for("the run's end", LONG, |e| {
            e.topic() == "sine.state" && e.data()["state"] == "finished"
        })
        .await;
    c.events[i].data().clone()
}

async fn phase(c: &mut Client, p: &str) -> Value {
    let i = c
        .wait_for(p, LONG, |e| {
            e.topic() == "sine.state" && e.data()["phase"] == p
        })
        .await;
    c.events[i].data().clone()
}

/// The sweep rate over consecutive frames of one leg, oct/min, from `t_s`.
fn leg_rates(frames: &[Value]) -> Vec<f64> {
    frames
        .windows(2)
        .filter(|w| {
            w[0]["direction"] == w[1]["direction"]
                && w[0]["held"] == false
                && w[1]["held"] == false
                && w[0]["t_s"].as_f64() < w[1]["t_s"].as_f64()
        })
        .filter_map(|w| {
            let (t0, t1) = (w[0]["t_s"].as_f64()?, w[1]["t_s"].as_f64()?);
            let (f0, f1) = (
                w[0]["frequency_hz"].as_f64()?,
                w[1]["frequency_hz"].as_f64()?,
            );
            (t1 - t0 > 0.5).then(|| (f1 / f0).log2() / (t1 - t0) * 60.0)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the Sine core: the law seam (law_guards and control_tick_s served by \
            SineService::new) and the built-in law resolved by resolve_sine"]
async fn sim_1_a_sweep_completes_within_its_band_at_its_rate_and_ramps_down() {
    let addr = serve(service(10.0, Duration::from_secs(30))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok("sine.configure", json!({"config": sim1()})).await;
    c.ok("sine.start", json!({})).await;
    let end = finished(&mut c).await;
    assert_eq!(end["end"]["outcome"], "completed", "{}", end["end"]);
    let ramp = end["end"]["ramp_s"].as_f64().unwrap();
    assert!(
        ramp > 0.0 && ramp < 2.0,
        "the completion ramps down: {ramp}"
    );
    let frames = lives(&c);
    // At full level after the settle, the error is within 1 dB on 99% of frames and
    // never past 1.5 dB.
    let full: Vec<f64> = frames
        .iter()
        .filter(|f| f["phase"] == "full_level")
        .skip(5)
        .filter_map(|f| f["error_db"].as_f64())
        .collect();
    assert!(full.len() > 20, "{} full-level frames", full.len());
    let within = full.iter().filter(|e| e.abs() <= 1.0).count() as f64 / full.len() as f64;
    assert!(within >= 0.99, "{within}");
    assert!(full.iter().all(|e| e.abs() <= 1.5));
    // The sweep rate on the station's clock.
    let rates = leg_rates(&frames);
    assert!(!rates.is_empty());
    for r in rates {
        assert!((r.abs() - 8.0).abs() <= 0.08, "{r} oct/min");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the Sine core: the law seam and the built-in law resolved; the hold of \
            the level clock (Command::Hold holds the ladder too)"]
async fn sim_2_hold_resume_reverse_jog_goto_level_adjust_and_stop() {
    let addr = serve(service(10.0, Duration::from_secs(30))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let cfg = config(
        json!({"kind": "sweep", "band_hz": [100, 800], "rate": {"law": "log", "oct_per_min": 2},
               "end": {"sweeps": 20}}),
        json!({"mode": "manual", "levels_db": [-12, -6, 0], "start_db": -6}),
    );
    c.ok("sine.configure", json!({"config": cfg})).await;
    c.ok("sine.start", json!({})).await;
    phase(&mut c, "engaged").await;
    c.pump(Duration::from_secs(1)).await;
    // Hold keeps the frequency within 0.01 Hz while station time goes on.
    c.ok("sine.hold", json!({})).await;
    let n0 = lives(&c).len();
    c.pump(Duration::from_secs(2)).await;
    let held: Vec<Value> = lives(&c)[n0..]
        .iter()
        .filter(|f| f["held"] == true)
        .cloned()
        .collect();
    assert!(held.len() >= 3);
    let f0 = held[0]["frequency_hz"].as_f64().unwrap();
    assert!(held
        .iter()
        .all(|f| (f["frequency_hz"].as_f64().unwrap() - f0).abs() <= 0.01));
    assert!(held.last().unwrap()["t_s"].as_f64() > held[0]["t_s"].as_f64());
    // Go to while held lands within 0.5 Hz.
    let r = c.ok("sine.goto", json!({"hz": 300})).await;
    assert!((r["frequency_hz"].as_f64().unwrap() - 300.0).abs() <= 0.5);
    c.ok("sine.resume", json!({})).await;
    // Reverse turns the sweep within one batch.
    let before = c.ok("sine.state", json!({})).await["direction"].clone();
    c.ok("sine.reverse", json!({})).await;
    let after = c.ok("sine.state", json!({})).await["direction"].clone();
    assert_ne!(before, after);
    let r = c.ok("sine.jog", json!({"delta_hz": 10})).await;
    assert!(r["frequency_hz"].as_f64().is_some());
    // A manual level step and a live blend change.
    let r = c.ok("sine.level", json!({"delta_db": -1})).await;
    assert_eq!(r["level_db"], -7);
    let r = c
        .ok("sine.adjust", json!({"key": "blend_w", "value": 0.2}))
        .await;
    assert_eq!(r, json!({"key": "blend_w", "value": 0.2}));
    assert_eq!(
        c.ok("sine.state", json!({})).await["law"]["live"]["blend_w"],
        0.2
    );
    let log = c.ok("sine.log", json!({})).await;
    assert!(log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "law"));
    // Stop: the drive falls monotonically at no more than 132 dB/s of station time.
    c.pump(Duration::from_millis(500)).await;
    let n0 = lives(&c).len();
    c.ok("sine.stop", json!({})).await;
    let end = finished(&mut c).await;
    assert_eq!(end["end"]["outcome"], "operator_stop");
    assert_eq!(end["end"]["level_db"], -7);
    let ramp: Vec<(f64, f64)> = lives(&c)[n0..]
        .iter()
        .filter(|f| f["phase"] == "ramping_down")
        .filter_map(|f| Some((f["t_s"].as_f64()?, f["drive_vpk"].as_f64()?)))
        .collect();
    for w in ramp.windows(2) {
        assert!(w[1].1 <= w[0].1 * (1.0 + 1e-9), "monotonic: {w:?}");
        if w[1].1 > 0.0 && w[1].0 > w[0].0 {
            let slope = 20.0 * (w[0].1 / w[1].1).log10() / (w[1].0 - w[0].0);
            assert!(slope <= 132.0, "{slope} dB/s");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the Sine core: the law seam and the built-in law resolved"]
async fn sim_3_an_observer_aborts_within_one_batch() {
    let addr = serve(service(10.0, Duration::from_secs(30))).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok("sine.configure", json!({"config": sim1()})).await;
    c.ok("sine.start", json!({})).await;
    phase(&mut c, "engaged").await;
    let at = c.ok("sine.state", json!({})).await;
    obs.ok("sine.abort", json!({})).await;
    let end = finished(&mut c).await;
    assert_eq!(end["end"]["outcome"], "operator_abort");
    assert_eq!(end["end"]["ramp_s"], Value::Null);
    let t_end = end["end"]["at_s"].as_f64().unwrap();
    let t_at = at["elapsed_s"].as_f64().unwrap_or(0.0);
    assert!(t_end >= t_at);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the Sine core: the law seam and the built-in law resolved"]
async fn sim_4_the_shakers_displacement_limit_trips_the_kinematic_abort() {
    let addr = serve(service(10.0, Duration::from_secs(30))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let mut cfg = config(
        json!({"kind": "dwell", "freq_hz": 20, "end": {"seconds": 60}}),
        json!({"mode": "auto", "levels_db": [-12, -6, 0], "per_level": {"seconds": 5},
               "full_level": {"seconds": 5}}),
    );
    cfg["feasibility"] = json!({
        "shaker": {"name": "Bench shaker", "manufacturer": "", "model": "", "notes": "",
                   "force_random": null, "force_sine": {"value": 2000, "basis": "Peak"},
                   "force_shock": {"value": 4000, "basis": "Peak"},
                   "max_velocity_ips": {"value": 70, "basis": "Peak"},
                   "max_displacement_in_pp": {"value": 0.008, "basis": "Peak"},
                   "max_accel_bare_g": {"value": 100, "basis": "Peak"},
                   "armature_mass_lb": 45, "overturning_moment_limit_lbf_in": null,
                   "custom": []},
        "moving_mass_lb": {"armature": 45, "slip_table": 0, "drive_adapter": 3,
                           "fixturing": 10, "test_article": 12}});
    c.ok("sine.configure", json!({"config": cfg})).await;
    let e = c.refused("sine.arm", json!({})).await;
    assert_eq!(e["reason"], "feasibility_exceeded");
    c.ok(
        "sine.arm",
        json!({"override_ack": "a displacement abort is the test"}),
    )
    .await;
    c.ok("sine.start", json!({})).await;
    let end = finished(&mut c).await;
    assert_eq!(end["end"]["outcome"], "aborted", "{}", end["end"]);
    let trip = &end["end"]["trip"];
    assert_eq!(trip["kind"], "kinematic");
    assert_eq!(trip["axis"], "displacement");
    assert_eq!(trip["at_level_db"], 0);
    let v = trip["value"].as_f64().unwrap();
    assert!((v - 0.00978).abs() / 0.00978 < 0.03, "{v}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the Sine core: the law seam and the built-in law resolved"]
async fn sim_5_a_lost_controller_and_a_shutdown_each_safe_stop_with_the_ramp() {
    let svc = service(4.0, Duration::from_millis(1500));
    let addr = serve(svc.clone()).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok("sine.configure", json!({"config": sim1()})).await;
    c.ok("sine.start", json!({})).await;
    phase(&mut c, "engaged").await;
    drop(c);
    let end = finished(&mut obs).await;
    assert_eq!(end["end"]["outcome"], "safe_stop", "{}", end["end"]);
    assert!(end["end"]["ramp_s"].as_f64().unwrap() > 0.0);
    // The shutdown takes the same controlled stop.
    let svc = service(4.0, Duration::from_secs(30));
    let addr = serve(svc.clone()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok("sine.configure", json!({"config": sim1()})).await;
    c.ok("sine.start", json!({})).await;
    phase(&mut c, "engaged").await;
    let said = svc.shutdown().await.unwrap();
    assert!(said.contains("safe_stop"), "{said}");
    let s = c.ok("sine.state", json!({})).await;
    assert_eq!(s["end"]["outcome"], "safe_stop");
    assert!(s["end"]["message"].as_str().unwrap().contains("shut down"));
    assert!(s["end"]["ramp_s"].as_f64().unwrap() > 0.0);
}

/// The law file `<name>.wasm` built by scripts/build_laws.sh.
fn law_file(name: &str) -> PathBuf {
    let file = format!("{name}.wasm");
    let exe = std::env::current_exe().unwrap();
    exe.ancestors()
        .skip(1)
        .take(4)
        .map(|d| d.join("wasm32-wasip2").join("release").join(&file))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{file} is not built; run scripts/build_laws.sh"))
}

async fn import(c: &mut Client, name: &str) -> String {
    let r = c
        .ok(
            "laws.import",
            json!({"path": law_file(name).display().to_string()}),
        )
        .await;
    r["law"]["id"].as_str().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the Sine core: the law seam, the host's sine-single world and the law file \
            law_fixture_sine_misbehave"]
async fn sim_6_a_law_that_runs_late_three_times_ends_law_fault_with_the_drive_held_then_ramped() {
    let addr = serve(service(0.0, Duration::from_secs(30))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let id = import(&mut c, "law_fixture_sine_misbehave").await;
    let mut cfg = sim1();
    cfg["control"] = json!({"law": {"id": id, "params": {"mode": "sleep_after"}}});
    c.ok("sine.configure", json!({"config": cfg})).await;
    c.ok("sine.start", json!({})).await;
    let end = finished(&mut c).await;
    assert_eq!(end["end"]["outcome"], "law_fault", "{}", end["end"]);
    let trip = &end["end"]["trip"];
    assert_eq!(trip["kind"], "law_fault");
    assert_eq!(trip["reason"], "over_budget");
    assert_eq!(trip["consecutive"], 3);
    assert!(end["end"]["ramp_s"].as_f64().unwrap() > 0.0);
    let log = c.ok("sine.log", json!({})).await;
    let misses = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "law")
        .count();
    assert!(misses >= 3, "every unusable call is logged");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the Sine core: the law seam, the host's sine-single world and the law files \
            law_fixture_sine_template and hertzforge_law_sine_servo"]
async fn sim_7_the_template_law_passes_sim_1_and_the_built_in_matches_its_law_file() {
    let addr = serve(service(0.0, Duration::from_secs(30))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let template = import(&mut c, "law_fixture_sine_template").await;
    let mut cfg = sim1();
    cfg["acquisition"] = json!({"tick_ms": 10});
    cfg["control"] = json!({"law": {"id": template}});
    c.ok("sine.configure", json!({"config": cfg})).await;
    c.ok("sine.start", json!({})).await;
    let end = finished(&mut c).await;
    assert_eq!(end["end"]["outcome"], "completed", "{}", end["end"]);
    // The built-in and its law file: the same outcome at the same station time.
    let file = import(&mut c, "hertzforge_law_sine_servo").await;
    let mut ends = Vec::new();
    for id in ["servo_blend".to_owned(), file] {
        let mut cfg = sim1();
        cfg["control"] = json!({"law": {"id": id}});
        c.ok("sine.configure", json!({"config": cfg})).await;
        c.ok("sine.start", json!({})).await;
        let i = c
            .wait_for("the run's end", LONG, |e| {
                e.topic() == "sine.state"
                    && e.data()["state"] == "finished"
                    && e.data()["law"]["id"] == json!(id)
            })
            .await;
        ends.push(c.events[i].data()["end"].clone());
    }
    assert_eq!(ends[0]["outcome"], ends[1]["outcome"]);
    assert_eq!(ends[0]["at_s"], ends[1]["at_s"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the Sine core: the law seam and the built-in law resolved"]
async fn sim_8_a_live_sine_run_excludes_the_others_and_the_shutdown_refuses_its_verbs() {
    let svc = service(4.0, Duration::from_secs(30));
    let addr = serve(svc.clone()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok("sine.configure", json!({"config": sim1()})).await;
    c.ok("sine.start", json!({})).await;
    phase(&mut c, "engaged").await;
    for (m, p) in [
        ("random.configure", json!({"config": {}})),
        ("srs.configure", json!({"config": {}})),
        ("shock.configure", json!({"config": {}})),
        ("sim.start", json!({"profile": "flat"})),
    ] {
        let e = c.refused(m, p).await;
        assert!(
            e["reason"] == "run_active" || e["reason"] == "bad_params",
            "{m}: {e}"
        );
    }
    assert_eq!(
        c.refused("sine.configure", json!({"config": sim1()})).await["reason"],
        "run_active"
    );
    let shut = tokio::spawn({
        let svc = svc.clone();
        async move { svc.shutdown().await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    for (m, p) in [
        ("sine.level", json!({"delta_db": 1})),
        ("sine.adjust", json!({"key": "blend_w", "value": 0.2})),
        ("sine.goto", json!({"hz": 300})),
    ] {
        assert_eq!(c.refused(m, p).await["reason"], "shutting_down", "{m}");
    }
    let said = shut.await.unwrap();
    assert!(said.is_some_and(|s| s.contains("safe_stop")));
}
