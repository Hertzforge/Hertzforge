//! Classical shock through the protocol, on a real loopback WebSocket, with the
//! simulated shock engine: configure and its checks, the run from identification
//! through the review hold to the end, manual firing and its refusals, the
//! controller rules, the run exclusion against Random and MIMO both ways, the
//! shutdown, and firing held while nobody holds control, then the orphan safe stop.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
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

/// A control row and a drive row: classical shock's table, and Random's.
fn table() -> Value {
    json!([row(1, "control", "Control"), row(2, "drive", "Shaker")])
}

/// Two controls and two drives, for a MIMO run.
fn two_by_two() -> Value {
    json!([
        row(1, "control", "Control X"),
        row(2, "control", "Control Y"),
        row(3, "drive", "Shaker X"),
        row(4, "drive", "Shaker Y"),
    ])
}

/// A 20 g, 11 ms half-sine at 20480 Hz, the default ladder, `spacing_s` between
/// pulses, a short identification.
fn config(mode: &str, review: bool, spacing_s: f64) -> Value {
    json!({
        "title": "Protocol Shock",
        "pulse": {"shape": "half_sine", "amplitude_g": 20, "duration_ms": 11},
        "acquisition": {"fs_hz": 20480},
        "schedule": {"mode": mode, "levels_db": [-12, -9, -6, -3, 0], "pulses": [1, 1, 1, 1, 3]},
        "spacing": {"cooling_s": spacing_s, "settling_s": 0, "compute_s": 0},
        "identification": {"duration_s": 0.5, "nfft": 2048, "review": review},
        "limits": {"drive_vpk_ceiling": 0.5, "drive_vpk_abort": 0.8}
    })
}

async fn controller(c: &mut Client, rows: Value) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok("channels.set", json!({"revision": rev, "channels": rows}))
        .await;
}

/// Wait for a `shock.state` event of run `run_id` that satisfies `pred`.
async fn wait_run(
    c: &mut Client,
    run_id: &Value,
    what: &str,
    pred: impl Fn(&Value) -> bool,
) -> Value {
    let i = c
        .wait_for(what, LONG, |e| {
            e.topic() == "shock.state" && &e.data()["run_id"] == run_id && pred(e.data())
        })
        .await;
    c.events[i].data().clone()
}

async fn wait_shock(c: &mut Client, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let i = c
        .wait_for(what, LONG, |e| e.topic() == "shock.state" && pred(e.data()))
        .await;
    c.events[i].data().clone()
}

fn service(speed: f64, timeout: Duration, heartbeat: Duration) -> std::sync::Arc<Service> {
    Service::with_options(options(no_driver(), speed, timeout, heartbeat))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_auto_run_goes_from_configure_through_review_to_completed() {
    let schema = Schema::load();
    let svc = service(0.0, Duration::from_secs(30), Duration::from_secs(5));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c, table()).await;

    let none = obs.ok("shock.state", json!({})).await;
    schema.assert_valid(&none, Some("shockState"));
    assert_eq!(none["state"], "unconfigured");

    // The preview and a dry run need no control; configuring does.
    let preview = obs
        .ok(
            "shock.preview",
            json!({"pulse": config("auto", true, 0.05)["pulse"], "fs_hz": 20480}),
        )
        .await;
    schema.assert_valid(&preview, Some("shockPreviewResult"));
    let titles: Vec<&str> = preview["views"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Pulse", "Velocity", "Displacement"]);
    assert_eq!(preview["derived"]["quiet_window"]["inside"], true);
    let e = obs
        .refused(
            "shock.preview",
            json!({"pulse": {"shape": "half_sine", "amplitude_g": -1, "duration_ms": 11}}),
        )
        .await;
    assert_eq!(e["reason"], "bad_params");
    assert_eq!(e["detail"]["param"], "pulse.amplitude_g");
    let dry = obs
        .ok(
            "shock.configure",
            json!({"config": config("auto", true, 0.05), "dry_run": true}),
        )
        .await;
    schema.assert_valid(&dry, Some("shockDryRunResult"));
    assert!(dry["derived"].is_object() && dry["errors"] == json!([]));
    let e = obs
        .refused(
            "shock.configure",
            json!({"config": config("auto", true, 0.05)}),
        )
        .await;
    assert_eq!(e["reason"], "not_controller");
    let set = c
        .ok(
            "shock.configure",
            json!({"config": config("auto", true, 0.05)}),
        )
        .await;
    schema.assert_valid(&set, Some("shockConfigureResult"));
    assert_eq!(set["state"]["state"], "configured");
    assert_eq!(set["state"]["law"]["id"], "fd_correct");
    let cfg = obs.ok("shock.config", json!({})).await;
    schema.assert_valid(&cfg, Some("shockConfigResult"));
    assert_eq!(cfg["run_id"], set["state"]["run_id"]);

    // Start identifies, then holds for review; only the controller proceeds.
    assert_eq!(
        obs.refused("shock.start", json!({})).await["reason"],
        "not_controller"
    );
    let started = c.ok("shock.start", json!({})).await;
    schema.assert_valid(&started, Some("shockState"));
    let review = wait_shock(&mut c, "review", |d| d["state"] == "review").await;
    assert_eq!(review["phase"], "awaiting_review");
    assert_eq!(review["identification"]["passed"], true);
    assert_eq!(review["grid"]["actual"], true);
    assert_eq!(
        obs.refused("shock.proceed", json!({})).await["reason"],
        "not_controller"
    );
    c.ok("shock.proceed", json!({})).await;
    let end = wait_shock(&mut c, "finished", |d| d["state"] == "finished").await;
    schema.assert_valid(&end, Some("shockState"));
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    assert_eq!(end["pulses"]["fired"], 7);
    assert_eq!(end["end"]["pulse"], 6);

    // Every pulse has a row, in order, at the schedule's levels.
    let pulses = obs.ok("shock.pulses", json!({})).await;
    schema.assert_valid(&pulses, Some("shockPulsesResult"));
    let rows = pulses["pulses"].as_array().unwrap();
    assert_eq!(pulses["next"], 7);
    let levels: Vec<f64> = rows
        .iter()
        .map(|r| r["scheduled_db"].as_f64().unwrap())
        .collect();
    assert_eq!(levels, [-12.0, -9.0, -6.0, -3.0, 0.0, 0.0, 0.0]);
    assert!(rows
        .iter()
        .enumerate()
        .all(|(i, r)| r["index"] == i as u64 && r["passed"] == true));
    let later = obs.ok("shock.pulses", json!({"since": 5})).await;
    assert_eq!(later["pulses"].as_array().unwrap().len(), 2);
    let one = obs
        .ok("shock.pulse", json!({"index": 6, "points": 256}))
        .await;
    schema.assert_valid(&one, Some("shockPulseResult"));
    let titles: Vec<&str> = one["views"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Pulse", "Drive", "Velocity", "Displacement"]);
    let e = obs.refused("shock.pulse", json!({"index": 7})).await;
    assert_eq!(e["code"], "not_found");

    // The events: a state on each change, a pulse event per pulse to every
    // connection, live frames, and the log.
    obs.pump(Duration::from_millis(300)).await;
    for ev in obs.of_topic("shock.state") {
        schema.assert_valid(ev.data(), Some("shockState"));
    }
    let pulse_events = obs.of_topic("shock.pulse");
    assert_eq!(pulse_events.len(), 7);
    for ev in &pulse_events {
        assert!(ev.decoded.is_some(), "shock.pulse is a binary frame");
        schema.assert_valid(ev.data(), Some("shockPulseEvent"));
    }
    let live = obs.of_topic("shock.live");
    assert!(!live.is_empty());
    for ev in &live {
        assert!(ev.decoded.is_some(), "shock.live is a binary frame");
        schema.assert_valid(ev.data(), Some("shockLive"));
    }
    assert_eq!(live.last().unwrap().data()["state"], "finished");
    let log = obs.ok("shock.log", json!({})).await;
    let kinds: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.iter().filter(|k| **k == "pulse").count(), 7);

    // A finished run arms again only after a reset; clear forgets it.
    assert_eq!(
        c.refused("shock.arm", json!({})).await["reason"],
        "run_finished"
    );
    let reset = c.ok("shock.reset", json!({})).await;
    assert_eq!(reset["state"], "configured");
    assert_ne!(reset["run_id"], set["state"]["run_id"]);
    let cleared = c.ok("shock.clear", json!({})).await;
    assert_eq!(cleared["state"], "unconfigured");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_configuration_with_errors_is_refused_with_its_items() {
    let schema = Schema::load();
    let svc = service(0.0, Duration::from_secs(30), Duration::from_secs(5));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, table()).await;
    let mut bad = config("auto", true, 0.05);
    bad["limits"]["drive_vpk_abort"] = json!(0.4);
    bad["schedule"]["levels_db"] = json!([-6, -12, 0]);
    bad["schedule"]["pulses"] = json!([1, 1]);
    let e = c.refused("shock.configure", json!({"config": bad})).await;
    assert_eq!(e["reason"], "config_invalid");
    schema.assert_valid(&e["detail"], Some("report"));
    let fields: Vec<(&str, &str)> = e["detail"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["reason"].as_str().unwrap(), i["field"].as_str().unwrap()))
        .collect();
    assert!(
        fields.contains(&("levels_not_ascending", "schedule.levels_db[1]")),
        "{fields:?}"
    );
    assert!(
        fields.contains(&("pulses_length_mismatch", "schedule.pulses")),
        "{fields:?}"
    );
    assert!(
        fields.contains(&("out_of_range", "limits.drive_vpk_abort")),
        "{fields:?}"
    );
    // A wrong shape is refused naming the field.
    let mut shape = config("auto", true, 0.05);
    shape["schedule"]["advance"] = json!("sometimes");
    let e = c.refused("shock.configure", json!({"config": shape})).await;
    assert_eq!(e["reason"], "bad_params");
    assert_eq!(e["detail"]["param"], "config.schedule.advance");
    let mut no_ceiling = config("auto", true, 0.05);
    no_ceiling["limits"] = json!({"drive_vpk_abort": 0.8});
    let e = c
        .refused("shock.configure", json!({"config": no_ceiling}))
        .await;
    assert_eq!(e["detail"]["param"], "config.limits.drive_vpk_ceiling");
    // A Random law is not a classical shock law.
    let mut law = config("auto", true, 0.05);
    law["control"] = json!({"law": {"id": "pinv_closed"}});
    let e = c.refused("shock.configure", json!({"config": law})).await;
    assert_eq!(e["detail"]["errors"][0]["reason"], "law_wrong_world");
    // Nothing was configured.
    assert_eq!(
        c.ok("shock.state", json!({})).await["state"],
        "unconfigured"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_firing_and_its_refusals_then_the_stop() {
    let svc = service(0.0, Duration::from_secs(30), Duration::from_secs(5));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c, table()).await;

    c.ok(
        "shock.configure",
        json!({"config": config("manual", false, 1.0)}),
    )
    .await;
    c.ok("shock.start", json!({})).await;
    let waiting = wait_shock(&mut c, "waiting", |d| {
        d["state"] == "running" && d["phase"] == "waiting" && d["prediction"].is_object()
    })
    .await;
    assert_eq!(waiting["level_db"], -12);
    assert_eq!(waiting["pulses"]["of_level"], 0);

    // The observer may not fire or move the level.
    assert_eq!(
        obs.refused("shock.fire", json!({})).await["reason"],
        "not_controller"
    );
    assert_eq!(
        obs.refused("shock.level", json!({"delta_db": 1})).await["reason"],
        "not_controller"
    );

    // One pulse, and the next refused until the time between pulses has passed.
    c.ok("shock.fire", json!({})).await;
    let e = c.refused("shock.fire", json!({})).await;
    assert_eq!(e["reason"], "not_yet");
    assert!(e["detail"]["seconds"].as_f64().unwrap() > 0.0, "{e}");

    // Level moves: one step at a time, a rung, never past the table.
    assert_eq!(
        c.ok("shock.level", json!({"delta_db": 1})).await["level_db"],
        -11
    );
    assert_eq!(
        c.ok("shock.level", json!({"delta_db": 1})).await["level_db"],
        -10
    );
    for bad in [
        json!({"delta_db": 2}),
        json!({"delta_db": 1, "rung": 1}),
        json!({}),
    ] {
        assert_eq!(c.refused("shock.level", bad).await["reason"], "bad_params");
    }
    assert_eq!(
        c.refused("shock.level", json!({"rung": 9})).await["reason"],
        "not_a_rung"
    );
    assert_eq!(c.ok("shock.level", json!({"rung": 4})).await["level_db"], 0);
    assert_eq!(
        c.refused("shock.level", json!({"delta_db": 1})).await["reason"],
        "not_a_rung"
    );

    // A train below the time between pulses is refused; then a pulse at 0 dB.
    assert_eq!(
        c.refused("shock.train", json!({"count": 2, "spacing_s": 0.5}))
            .await["reason"],
        "bad_params"
    );
    assert_eq!(
        c.refused("shock.burst", json!({})).await["reason"],
        "wrong_state"
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    c.ok("shock.fire", json!({})).await;
    let fired = wait_shock(&mut c, "two pulses", |d| d["pulses"]["fired"] == 2).await;
    assert_eq!(fired["last_pulse"]["scheduled_db"], 0);

    // An observer may stop; nothing fires after.
    let stopped = obs.ok("shock.stop", json!({})).await;
    assert!(
        ["stopping", "finished"].contains(&stopped["state"].as_str().unwrap()),
        "{stopped}"
    );
    let end = wait_shock(&mut c, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_stop");
    assert!(end["end"]["message"]
        .as_str()
        .unwrap()
        .contains("Stopped by"));
    obs.pump(Duration::from_millis(300)).await;
    assert_eq!(
        obs.ok("shock.pulses", json!({})).await["next"],
        2,
        "nothing fired after the stop"
    );
    assert_eq!(
        c.refused("shock.fire", json!({})).await["reason"],
        "wrong_state",
        "a fire after the end"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_mode_and_the_drive_ceiling_on_a_level_move() {
    let svc = service(0.0, Duration::from_secs(30), Duration::from_secs(5));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, table()).await;

    // In auto the operator neither fires nor moves the level.
    c.ok(
        "shock.configure",
        json!({"config": config("auto", true, 0.05)}),
    )
    .await;
    c.ok("shock.start", json!({})).await;
    wait_shock(&mut c, "review", |d| d["state"] == "review").await;
    for (m, p) in [
        ("shock.fire", json!({})),
        ("shock.level", json!({"delta_db": 1})),
        ("shock.train", json!({})),
    ] {
        let e = c.refused(m, p).await;
        assert_eq!(e["reason"], "wrong_mode", "{m}: {e}");
        assert_eq!(e["detail"]["mode"], "auto");
    }
    c.ok("shock.abort", json!({})).await;
    let end = wait_shock(&mut c, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_abort");
    assert_eq!(end["phase"], "aborted");
    assert_eq!(end["pulses"]["fired"], 0);

    // In manual with a drive abort under the full-level drive (0.4 V at 0 dB):
    // -3 dB is allowed, the full level is refused with both numbers.
    let mut cfg = config("manual", false, 0.05);
    cfg["limits"] = json!({"drive_vpk_ceiling": 0.35, "drive_vpk_abort": 0.38});
    let id = c.ok("shock.configure", json!({"config": cfg})).await["state"]["run_id"].clone();
    c.ok("shock.start", json!({})).await;
    wait_run(&mut c, &id, "waiting", |d| {
        d["phase"] == "waiting" && d["prediction"].is_object()
    })
    .await;
    assert_eq!(
        c.ok("shock.level", json!({"rung": 3})).await["level_db"],
        -3
    );
    let e = c.refused("shock.level", json!({"rung": 4})).await;
    assert_eq!(e["reason"], "drive_ceiling");
    assert_eq!(e["detail"]["vpk"], 0.4);
    assert_eq!(e["detail"]["abort_vpk"], 0.38);
    assert_eq!(e["detail"]["level_db"], 0);
    c.ok("shock.stop", json!({})).await;
    wait_run(&mut c, &id, "finished", |d| d["state"] == "finished").await;

    // In auto the same limits end the run before any pulse.
    let mut cfg = config("auto", false, 0.05);
    cfg["limits"] = json!({"drive_vpk_ceiling": 0.35, "drive_vpk_abort": 0.38});
    let id = c.ok("shock.configure", json!({"config": cfg})).await["state"]["run_id"].clone();
    c.ok("shock.start", json!({})).await;
    let end = wait_run(&mut c, &id, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "aborted");
    assert_eq!(end["end"]["trip"]["kind"], "drive_vpk");
    assert_eq!(end["end"]["trip"]["predicted"], true);
    assert_eq!(end["end"]["trip"]["pulse"], 0);
    assert_eq!(end["pulses"]["fired"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fire_in_flight_refuses_another_and_an_observer_abort_ends_the_run() {
    // Paced: a 20480 Hz record of 8192 samples takes 0.4 s, at a tenth of real
    // time 4 s.
    let svc = service(0.1, Duration::from_secs(30), Duration::from_secs(5));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c, table()).await;
    let mut cfg = config("manual", false, 0.05);
    cfg["identification"]["duration_s"] = json!(0.5);
    c.ok("shock.configure", json!({"config": cfg})).await;
    c.ok("shock.start", json!({})).await;
    wait_shock(&mut c, "waiting", |d| {
        d["phase"] == "waiting" && d["prediction"].is_object()
    })
    .await;
    c.ok("shock.fire", json!({})).await;
    let firing = wait_shock(&mut c, "firing", |d| d["phase"] == "firing").await;
    assert_eq!(firing["next_pulse_s"], Value::Null);
    let e = c.refused("shock.fire", json!({})).await;
    assert_eq!(e["reason"], "not_yet", "{e}");
    let t0 = Instant::now();
    obs.ok("shock.abort", json!({})).await;
    let end = wait_shock(&mut c, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_abort");
    assert!(t0.elapsed() < Duration::from_secs(10));
    assert!(end["end"]["message"]
        .as_str()
        .unwrap()
        .contains("Aborted by"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_shock_run_and_random_or_mimo_exclude_each_other() {
    let svc = service(0.0, Duration::from_secs(30), Duration::from_secs(5));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, table()).await;

    // Shock live (armed): Random, MIMO, a simulation and the table are refused.
    c.ok(
        "shock.configure",
        json!({"config": config("manual", true, 0.05)}),
    )
    .await;
    let armed = c.ok("shock.arm", json!({})).await;
    assert_eq!(armed["state"], "armed");
    let random = sim_config("auto", json!([{"db": 0, "dwell_s": null}]), 2.0);
    let e = c
        .refused("random.configure", json!({"config": random}))
        .await;
    assert_eq!(e["reason"], "run_active", "{e}");
    let setup = {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../schema/golden/request_mimo_random_setup.json");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        v["params"]["setup"].clone()
    };
    let e = c
        .refused("mimo_random.setup", json!({"setup": setup}))
        .await;
    assert_eq!(e["reason"], "run_active", "{e}");
    let e = c.refused("sim.start", json!({"profile": "flat"})).await;
    assert!(
        e["reason"] == "run_active" || e["code"] == "not_found",
        "{e}"
    );
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    let e = c
        .refused(
            "channels.set",
            json!({"revision": rev, "channels": table()}),
        )
        .await;
    assert_eq!(e["reason"], "run_active");
    c.ok("shock.stop", json!({})).await;
    wait_shock(&mut c, "finished", |d| d["state"] == "finished").await;

    // Random live (armed): classical shock is refused.
    c.ok("random.configure", json!({"config": random})).await;
    c.ok("random.arm", json!({})).await;
    let e = c
        .refused(
            "shock.configure",
            json!({"config": config("manual", true, 0.05)}),
        )
        .await;
    assert_eq!(e["reason"], "run_active", "{e}");
    c.ok("random.stop", json!({})).await;
    c.wait_state("finished", LONG).await;
    c.ok("random.clear", json!({})).await;

    // MIMO live: classical shock is refused, and a configured shock run cannot arm.
    c.ok(
        "shock.configure",
        json!({"config": config("manual", true, 0.05)}),
    )
    .await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": two_by_two()}),
    )
    .await;
    c.ok("mimo_random.setup", json!({"setup": setup})).await;
    c.ok("mimo_random.start", json!({})).await;
    let e = c
        .refused(
            "shock.configure",
            json!({"config": config("manual", true, 0.05)}),
        )
        .await;
    assert_eq!(e["reason"], "run_active", "{e}");
    let e = c.refused("shock.arm", json!({})).await;
    assert_eq!(e["reason"], "run_active", "{e}");
    c.ok("mimo_random.abort", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_refuses_the_drive_verbs_and_safe_stops_the_run() {
    let svc = service(0.0, Duration::from_secs(30), Duration::from_secs(5));
    let addr = serve(svc.clone()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, table()).await;
    c.ok(
        "shock.configure",
        json!({"config": config("manual", false, 0.05)}),
    )
    .await;
    c.ok("shock.start", json!({})).await;
    wait_shock(&mut c, "waiting", |d| {
        d["phase"] == "waiting" && d["prediction"].is_object()
    })
    .await;
    let said = svc.shutdown().await;
    assert!(said.is_some_and(|s| s.contains("safe_stop")));
    for (m, p) in [
        ("shock.arm", json!({})),
        ("shock.start", json!({})),
        ("shock.proceed", json!({})),
        ("shock.fire", json!({})),
        ("shock.level", json!({"delta_db": 1})),
        ("shock.train", json!({})),
        ("shock.burst", json!({})),
    ] {
        assert_eq!(c.refused(m, p).await["reason"], "shutting_down", "{m}");
    }
    let s = c.ok("shock.state", json!({})).await;
    assert_eq!(s["end"]["outcome"], "safe_stop");
    assert!(s["end"]["message"].as_str().unwrap().contains("shut down"));
    // Reading still works.
    c.ok("shock.pulses", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn firing_holds_when_the_controller_goes_resumes_on_a_reclaim_and_safe_stops_after() {
    let schema = Schema::load();
    let timeout = Duration::from_millis(1500);
    let svc = service(0.0, timeout, Duration::from_secs(5));
    let addr = serve(svc).await;
    let mut obs = Client::connect(addr).await;

    // A run firing every 0.3 s; the controller's connection drops after two pulses.
    let mut c = Client::connect(addr).await;
    let lease = c.ok("session.claim", json!({"name": "Shell"})).await["lease"].clone();
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": table()}),
    )
    .await;
    let mut cfg = config("auto", false, 0.3);
    cfg["schedule"]["pulses"] = json!([2, 2, 2, 2, 3]);
    c.ok("shock.configure", json!({"config": cfg.clone()}))
        .await;
    c.ok("shock.start", json!({})).await;
    obs.wait_for("two pulses", LONG, |e| {
        e.topic() == "shock.state" && e.data()["pulses"]["fired"] == 2
    })
    .await;
    drop(c);
    let dropped_at = Instant::now();
    obs.pump(Duration::from_millis(900)).await;
    let held = obs.ok("shock.pulses", json!({})).await["next"]
        .as_u64()
        .unwrap();
    obs.pump(Duration::from_millis(400)).await;
    assert_eq!(
        obs.ok("shock.pulses", json!({})).await["next"]
            .as_u64()
            .unwrap(),
        held,
        "no pulse while nobody holds control"
    );
    assert!(
        held <= 3,
        "at most the pulse in flight finished after the drop: {held}"
    );
    let s = obs.ok("shock.state", json!({})).await;
    schema.assert_valid(&s, Some("shockState"));
    assert_eq!(s["state"], "running", "{s}");
    assert!(dropped_at.elapsed() < timeout);

    // A reclaim within the grace resumes the run.
    let mut c2 = Client::connect(addr).await;
    let back = c2
        .ok("session.claim", json!({"name": "Shell", "lease": lease}))
        .await;
    assert_eq!(back["resumed"], true, "{back}");
    let end = wait_shock(&mut c2, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "completed");
    assert_eq!(end["pulses"]["fired"], 11);

    // Without a reclaim the orphan clock safe-stops it.
    c2.ok("shock.configure", json!({"config": cfg})).await;
    c2.ok("shock.start", json!({})).await;
    obs.wait_for("a pulse of the second run", LONG, |e| {
        e.topic() == "shock.state"
            && e.data()["run_id"] != end["run_id"]
            && e.data()["pulses"]["fired"] == 1
    })
    .await;
    drop(c2);
    let i = obs
        .wait_for("the safe stop", LONG, |e| {
            e.topic() == "shock.state"
                && e.data()["run_id"] != end["run_id"]
                && e.data()["state"] == "finished"
        })
        .await;
    let fin = obs.events[i].data().clone();
    assert_eq!(fin["end"]["outcome"], "safe_stop", "{fin}");
    assert!(fin["pulses"]["fired"].as_u64().unwrap() <= 2);
}
