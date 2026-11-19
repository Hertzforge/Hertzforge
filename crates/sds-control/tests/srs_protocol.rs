//! SRS shock through the protocol, on a real loopback WebSocket, with the simulated
//! SRS engine: configure, the preview and the imports, the run from identification
//! through the review hold to the end, manual firing and its refusals, the
//! controller rules, the run exclusion against Random, MIMO and classical shock both
//! ways, the shutdown, and firing held while nobody holds control, then the orphan
//! safe stop.

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

/// A control row and a drive row: SRS shock's table, and Random's.
fn table() -> Value {
    json!([row(1, "control", "Control"), row(2, "drive", "Shaker")])
}

/// A control row, a fixture accelerometer and a drive row.
fn with_fixture() -> Value {
    let mut fixture = row(2, "monitor", "Fixture");
    fixture["index"] = json!(2);
    json!([
        row(1, "control", "Control"),
        fixture,
        row(3, "drive", "Shaker")
    ])
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

/// A flat 50 g target, 100 to 2000 Hz, at 20480 Hz; a three-level ladder;
/// `spacing_s` between pulses; a short identification.
fn config(mode: &str, review: bool, spacing_s: f64) -> Value {
    json!({
        "title": "Protocol SRS",
        "target": {"kind": "flat", "accel_g": 50},
        "bands": {"f_lo_hz": 100, "f_hi_hz": 2000},
        "acquisition": {"fs_hz": 20480},
        "schedule": {"mode": mode, "levels_db": [-12, -6, 0], "pulses": [2, 2, 3]},
        "spacing": {"cooling_s": spacing_s, "settling_s": 0, "compute_s": 0},
        "identification": {"nfft": 1024, "review": review},
        "limits": {"drive_vpk_ceiling": 5, "drive_vpk_abort": 8}
    })
}

/// A classical shock configuration, for the exclusion.
fn shock_config() -> Value {
    json!({
        "pulse": {"shape": "half_sine", "amplitude_g": 20, "duration_ms": 11},
        "acquisition": {"fs_hz": 20480},
        "schedule": {"mode": "manual", "levels_db": [0], "pulses": [1]},
        "identification": {"duration_s": 0.5, "nfft": 2048},
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

/// Wait for an `srs.state` event of run `run_id` that satisfies `pred`.
async fn wait_run(
    c: &mut Client,
    run_id: &Value,
    what: &str,
    pred: impl Fn(&Value) -> bool,
) -> Value {
    let i = c
        .wait_for(what, LONG, |e| {
            e.topic() == "srs.state" && &e.data()["run_id"] == run_id && pred(e.data())
        })
        .await;
    c.events[i].data().clone()
}

async fn wait_srs(c: &mut Client, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let i = c
        .wait_for(what, LONG, |e| e.topic() == "srs.state" && pred(e.data()))
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

    let none = obs.ok("srs.state", json!({})).await;
    schema.assert_valid(&none, Some("srsState"));
    assert_eq!(none["state"], "unconfigured");

    // The preview and a dry run need no control; configuring does.
    let cfg = config("auto", true, 0.05);
    let preview = obs
        .ok(
            "srs.preview",
            json!({"target": cfg["target"], "bands": cfg["bands"], "fs_hz": 20480,
                   "envelope": {"alarm_db": [2, 2], "abort_db": [6, 6]}}),
        )
        .await;
    schema.assert_valid(&preview, Some("srsPreviewResult"));
    let titles: Vec<&str> = preview["views"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        ["Target", "Synthesis", "Pulse", "Velocity", "Displacement"]
    );
    assert_eq!(preview["derived"]["synthesis"]["components"], 14);
    for (bad, param) in [
        (
            json!({"target": {"kind": "flat", "accel_g": -1}, "bands": cfg["bands"]}),
            "target.accel_g",
        ),
        (
            json!({"target": cfg["target"], "bands": {"f_lo_hz": 100, "f_hi_hz": 9000},
                   "fs_hz": 8192}),
            "bands.f_hi_hz",
        ),
        (
            json!({"target": cfg["target"], "bands": cfg["bands"],
                   "envelope": {"alarm_db": [6, 6], "abort_db": [3, 3]}}),
            "envelope.abort_db",
        ),
    ] {
        let e = obs.refused("srs.preview", bad).await;
        assert_eq!(e["reason"], "bad_params", "{e}");
        assert_eq!(e["detail"]["param"], param, "{e}");
    }
    let dry = obs
        .ok("srs.configure", json!({"config": cfg, "dry_run": true}))
        .await;
    schema.assert_valid(&dry, Some("srsDryRunResult"));
    assert!(dry["derived"].is_object() && dry["errors"] == json!([]));
    assert_eq!(dry["views"].as_array().unwrap().len(), 5);
    let e = obs.refused("srs.configure", json!({"config": cfg})).await;
    assert_eq!(e["reason"], "not_controller");
    let set = c.ok("srs.configure", json!({"config": cfg})).await;
    schema.assert_valid(&set, Some("srsConfigureResult"));
    assert_eq!(set["state"]["state"], "configured");
    assert_eq!(set["state"]["law"]["id"], "srs_ratio");
    assert_eq!(set["state"]["run_id"], "srs-0001");
    let config = obs.ok("srs.config", json!({})).await;
    schema.assert_valid(&config, Some("srsConfigResult"));
    assert_eq!(config["run_id"], set["state"]["run_id"]);

    // Start identifies, then holds for review; only the controller proceeds.
    assert_eq!(
        obs.refused("srs.start", json!({})).await["reason"],
        "not_controller"
    );
    let started = c.ok("srs.start", json!({})).await;
    schema.assert_valid(&started, Some("srsState"));
    let review = wait_srs(&mut c, "review", |d| d["state"] == "review").await;
    assert_eq!(review["phase"], "awaiting_review");
    assert_eq!(review["identification"]["passed"], true);
    assert_eq!(review["identification"]["excitation"], "shaped");
    assert_eq!(review["grid"]["actual"], true);
    assert_eq!(
        obs.refused("srs.proceed", json!({})).await["reason"],
        "not_controller"
    );
    c.ok("srs.proceed", json!({})).await;
    let end = wait_srs(&mut c, "finished", |d| d["state"] == "finished").await;
    schema.assert_valid(&end, Some("srsState"));
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    assert_eq!(end["pulses"]["fired"], 7);
    assert_eq!(end["pulses"]["passed"], 7);
    assert_eq!(end["end"]["pulse"], 6);
    assert_eq!(end["notch"], Value::Null, "no limit channels, no notch");

    // Every pulse has a row, in order, at the schedule's levels.
    let pulses = obs.ok("srs.pulses", json!({})).await;
    schema.assert_valid(&pulses, Some("srsPulsesResult"));
    let rows = pulses["pulses"].as_array().unwrap();
    assert_eq!(pulses["next"], 7);
    let levels: Vec<f64> = rows
        .iter()
        .map(|r| r["scheduled_db"].as_f64().unwrap())
        .collect();
    assert_eq!(levels, [-12.0, -12.0, -6.0, -6.0, 0.0, 0.0, 0.0]);
    assert!(rows
        .iter()
        .enumerate()
        .all(|(i, r)| r["index"] == i as u64 && r["passed"] == true && r["origin"] == "ladder"));
    let one = obs
        .ok("srs.pulse", json!({"index": 6, "points": 256}))
        .await;
    schema.assert_valid(&one, Some("srsPulseResult"));
    let titles: Vec<&str> = one["views"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        ["SRS", "Pulse", "Drive", "Velocity", "Displacement"]
    );
    let e = obs.refused("srs.pulse", json!({"index": 7})).await;
    assert_eq!(e["code"], "not_found");

    // The events: a state on each change, a pulse event per pulse to every
    // connection, live frames, and the log.
    obs.pump(Duration::from_millis(300)).await;
    for ev in obs.of_topic("srs.state") {
        schema.assert_valid(ev.data(), Some("srsState"));
    }
    let pulse_events = obs.of_topic("srs.pulse");
    assert_eq!(pulse_events.len(), 7);
    for ev in &pulse_events {
        assert!(ev.decoded.is_some(), "srs.pulse is a binary frame");
        schema.assert_valid(ev.data(), Some("srsPulseEvent"));
        let titles: Vec<&str> = ev.data()["views"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, ["SRS", "Pulse", "Drive"]);
    }
    let live = obs.of_topic("srs.live");
    assert!(!live.is_empty());
    for ev in &live {
        assert!(ev.decoded.is_some(), "srs.live is a binary frame");
        schema.assert_valid(ev.data(), Some("srsLive"));
    }
    assert_eq!(live.last().unwrap().data()["state"], "finished");
    let log = obs.ok("srs.log", json!({})).await;
    let kinds: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.iter().filter(|k| **k == "pulse").count(), 7);

    // A finished run arms again only after a reset; clear forgets it.
    assert_eq!(
        c.refused("srs.arm", json!({})).await["reason"],
        "run_finished"
    );
    let reset = c.ok("srs.reset", json!({})).await;
    assert_eq!(reset["state"], "configured");
    assert_ne!(reset["run_id"], set["state"]["run_id"]);
    let cleared = c.ok("srs.clear", json!({})).await;
    assert_eq!(cleared["state"], "unconfigured");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subscription_gets_its_plots_with_limit_channels_and_their_gains() {
    let schema = Schema::load();
    let svc = service(0.0, Duration::from_secs(30), Duration::from_secs(5));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, with_fixture()).await;
    let mut cfg = config("auto", false, 0.05);
    cfg["limit_channels"] = json!({"headroom_db": 1, "channels": [
        {"channel": 2, "bound": {"kind": "flat", "accel_g": 80}, "frf": null}]});
    c.ok("srs.configure", json!({"config": cfg})).await;
    let sub = c
        .ok(
            "srs.subscribe",
            json!({"rate_hz": 10, "plots": [
                {"id": "srs", "quantity": "srs"},
                {"id": "comp", "quantity": "components"},
                {"id": "lim", "quantity": "limit_channels"},
                {"id": "frf", "quantity": "limit_frf"},
                {"id": "id", "quantity": "identification"},
                {"id": "pulse", "quantity": "pulse", "points": 256}]}),
        )
        .await;
    schema.assert_valid(&sub, Some("srsSubscribeResult"));
    c.ok("srs.start", json!({})).await;
    let end = wait_srs(&mut c, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    assert_eq!(end["notch"]["notched_bands"], 0);
    c.pump(Duration::from_millis(300)).await;
    let data = c.of_topic("srs.data");
    assert!(!data.is_empty());
    for ev in &data {
        schema.assert_valid(ev.data(), Some("srsData"));
    }
    let last = data.last().unwrap().data();
    let views = |id: &str| -> Vec<String> {
        last["plots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap()["views"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["title"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(views("srs"), ["SRS"]);
    assert_eq!(views("comp"), ["Components", "Gains"]);
    assert_eq!(views("lim"), ["Limit ch2"]);
    assert_eq!(views("frf"), ["Gain ch2"]);
    assert_eq!(views("id"), ["Identification Gain", "Coherence"]);
    assert_eq!(views("pulse"), ["Pulse", "Drive"]);
    // A point count on a plot that is not a time history is refused, as the shell
    // must know.
    let e = c
        .refused(
            "srs.subscribe",
            json!({"plots": [{"id": "s", "quantity": "srs", "points": 512}]}),
        )
        .await;
    assert_eq!(e["reason"], "bad_params");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_configuration_with_errors_is_refused_with_its_items() {
    let schema = Schema::load();
    let svc = service(0.0, Duration::from_secs(30), Duration::from_secs(5));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, table()).await;
    let mut bad = config("auto", true, 0.05);
    bad["limits"]["drive_vpk_abort"] = json!(4);
    bad["schedule"]["levels_db"] = json!([-6, -12, 0]);
    bad["target"] = json!({"kind": "breakpoints", "points": [[100, 40], [1500, 100]]});
    bad["limit_channels"] = json!({"headroom_db": 1, "channels": [
        {"channel": 9, "bound": {"kind": "flat", "accel_g": 50}}]});
    let e = c.refused("srs.configure", json!({"config": bad})).await;
    assert_eq!(e["reason"], "config_invalid");
    schema.assert_valid(&e["detail"], Some("report"));
    let fields: Vec<(&str, &str)> = e["detail"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["reason"].as_str().unwrap(), i["field"].as_str().unwrap()))
        .collect();
    for want in [
        ("levels_not_ascending", "schedule.levels_db[1]"),
        ("out_of_range", "limits.drive_vpk_abort"),
        ("target_not_covering", "target.points"),
        (
            "limit_unknown_channel",
            "limit_channels.channels[0].channel",
        ),
    ] {
        assert!(fields.contains(&want), "{want:?} in {fields:?}");
    }
    // A wrong shape is refused naming the field.
    let mut shape = config("auto", true, 0.05);
    shape["synthesis"] = json!({"engine": "fourier"});
    let e = c.refused("srs.configure", json!({"config": shape})).await;
    assert_eq!(e["reason"], "bad_params");
    assert_eq!(e["detail"]["param"], "config.synthesis.engine");
    let mut no_ceiling = config("auto", true, 0.05);
    no_ceiling["limits"] = json!({"drive_vpk_abort": 0.8});
    let e = c
        .refused("srs.configure", json!({"config": no_ceiling}))
        .await;
    assert_eq!(e["detail"]["param"], "config.limits.drive_vpk_ceiling");
    // A classical shock law is not an SRS law, and an unknown one is unknown.
    for (id, reason) in [("fd_correct", "law_wrong_world"), ("nope", "law_unknown")] {
        let mut law = config("auto", true, 0.05);
        law["control"] = json!({"law": {"id": id}});
        let e = c.refused("srs.configure", json!({"config": law})).await;
        assert_eq!(e["detail"]["errors"][0]["reason"], reason, "{e}");
    }
    // The built-in law's own clamp plus 1 dB bounds the core's gain cap.
    let mut cap = config("auto", true, 0.05);
    cap["control"] = json!({"law": {"id": "srs_ratio", "params": {"max_db_per_pulse": 5}},
                            "max_gain_rise_db": 5});
    let e = c.refused("srs.configure", json!({"config": cap})).await;
    assert_eq!(
        e["detail"]["errors"][0]["field"], "control.max_gain_rise_db",
        "{e}"
    );
    // Nothing was configured.
    assert_eq!(c.ok("srs.state", json!({})).await["state"], "unconfigured");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_firing_and_its_refusals_then_the_stop() {
    let svc = service(0.0, Duration::from_secs(30), Duration::from_secs(5));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c, table()).await;

    c.ok(
        "srs.configure",
        json!({"config": config("manual", false, 1.0)}),
    )
    .await;
    c.ok("srs.start", json!({})).await;
    let waiting = wait_srs(&mut c, "waiting", |d| {
        d["state"] == "running" && d["phase"] == "waiting" && d["prediction"].is_object()
    })
    .await;
    assert_eq!(waiting["level_db"], -12);
    assert_eq!(waiting["pulses"]["of_level"], 0);

    // The observer may not fire or move the level.
    assert_eq!(
        obs.refused("srs.fire", json!({})).await["reason"],
        "not_controller"
    );
    assert_eq!(
        obs.refused("srs.level", json!({"delta_db": 1})).await["reason"],
        "not_controller"
    );

    // One pulse, and the next refused until the time between pulses has passed.
    c.ok("srs.fire", json!({})).await;
    let e = c.refused("srs.fire", json!({})).await;
    assert_eq!(e["reason"], "not_yet");
    assert!(e["detail"]["seconds"].as_f64().unwrap() > 0.0, "{e}");

    // Level moves: one step at a time, a rung, never past the table.
    assert_eq!(
        c.ok("srs.level", json!({"delta_db": 1})).await["level_db"],
        -11
    );
    for bad in [
        json!({"delta_db": 2}),
        json!({"delta_db": 1, "rung": 1}),
        json!({}),
    ] {
        assert_eq!(c.refused("srs.level", bad).await["reason"], "bad_params");
    }
    assert_eq!(
        c.refused("srs.level", json!({"rung": 9})).await["reason"],
        "not_a_rung"
    );
    assert_eq!(c.ok("srs.level", json!({"rung": 2})).await["level_db"], 0);
    assert_eq!(
        c.refused("srs.level", json!({"delta_db": 1})).await["reason"],
        "not_a_rung"
    );
    // SRS shock has no trains or bursts.
    assert_eq!(
        c.call("srs.train", json!({})).await["error"]["code"],
        "unknown_method"
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    c.ok("srs.fire", json!({})).await;
    let fired = wait_srs(&mut c, "two pulses", |d| d["pulses"]["fired"] == 2).await;
    assert_eq!(fired["last_pulse"]["scheduled_db"], 0);

    // An observer may stop; nothing fires after.
    let stopped = obs.ok("srs.stop", json!({})).await;
    assert!(
        ["stopping", "finished"].contains(&stopped["state"].as_str().unwrap()),
        "{stopped}"
    );
    let end = wait_srs(&mut c, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_stop");
    assert!(end["end"]["message"]
        .as_str()
        .unwrap()
        .contains("Stopped by"));
    obs.pump(Duration::from_millis(300)).await;
    assert_eq!(
        obs.ok("srs.pulses", json!({})).await["next"],
        2,
        "nothing fired after the stop"
    );
    assert_eq!(
        c.refused("srs.fire", json!({})).await["reason"],
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
        "srs.configure",
        json!({"config": config("auto", true, 0.05)}),
    )
    .await;
    c.ok("srs.start", json!({})).await;
    wait_srs(&mut c, "review", |d| d["state"] == "review").await;
    for (m, p) in [
        ("srs.fire", json!({})),
        ("srs.level", json!({"delta_db": 1})),
    ] {
        let e = c.refused(m, p).await;
        assert_eq!(e["reason"], "wrong_mode", "{m}: {e}");
        assert_eq!(e["detail"]["mode"], "auto");
    }
    c.ok("srs.abort", json!({})).await;
    let end = wait_srs(&mut c, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_abort");
    assert_eq!(end["phase"], "aborted");
    assert_eq!(end["pulses"]["fired"], 0);

    // The simulated wire plays the synthesis at 0.02 V per g: the full-level drive.
    let cfg = config("manual", false, 0.05);
    let preview = c
        .ok(
            "srs.preview",
            json!({"target": cfg["target"], "bands": cfg["bands"], "fs_hz": 20480}),
        )
        .await;
    let full = preview["derived"]["reference"]["peak_g"].as_f64().unwrap()
        * sds_control::srs::sim::GAIN_V_PER_G;
    let (ceiling, abort) = (0.85 * full, 0.95 * full);

    // In manual with a drive abort under the full-level drive: -6 dB is allowed,
    // the full level is refused with both numbers.
    let mut cfg = config("manual", false, 0.05);
    cfg["limits"] = json!({"drive_vpk_ceiling": ceiling, "drive_vpk_abort": abort});
    let id = c.ok("srs.configure", json!({"config": cfg})).await["state"]["run_id"].clone();
    c.ok("srs.start", json!({})).await;
    wait_run(&mut c, &id, "waiting", |d| {
        d["phase"] == "waiting" && d["prediction"].is_object()
    })
    .await;
    assert_eq!(c.ok("srs.level", json!({"rung": 1})).await["level_db"], -6);
    let e = c.refused("srs.level", json!({"rung": 2})).await;
    assert_eq!(e["reason"], "drive_ceiling");
    assert!(
        (e["detail"]["vpk"].as_f64().unwrap() - full).abs() < 1e-3,
        "{e}"
    );
    assert!((e["detail"]["abort_vpk"].as_f64().unwrap() - abort).abs() < 1e-9);
    assert_eq!(e["detail"]["level_db"], 0);
    c.ok("srs.stop", json!({})).await;
    wait_run(&mut c, &id, "finished", |d| d["state"] == "finished").await;

    // In auto the same limits end the run before any pulse.
    let mut cfg = config("auto", false, 0.05);
    cfg["limits"] = json!({"drive_vpk_ceiling": ceiling, "drive_vpk_abort": abort});
    let id = c.ok("srs.configure", json!({"config": cfg})).await["state"]["run_id"].clone();
    c.ok("srs.start", json!({})).await;
    let end = wait_run(&mut c, &id, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "aborted");
    assert_eq!(end["end"]["trip"]["kind"], "drive_vpk");
    assert_eq!(end["end"]["trip"]["predicted"], true);
    assert_eq!(end["end"]["trip"]["pulse"], 0);
    assert_eq!(end["pulses"]["fired"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fire_in_flight_refuses_another_and_an_observer_abort_ends_the_run() {
    // Paced: a 20480 Hz record of about 4000 samples takes 0.2 s, at a twentieth of
    // real time 4 s.
    let svc = service(0.05, Duration::from_secs(30), Duration::from_secs(5));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c, table()).await;
    let mut cfg = config("manual", false, 0.05);
    cfg["identification"]["nfft"] = json!(64);
    c.ok("srs.configure", json!({"config": cfg})).await;
    c.ok("srs.start", json!({})).await;
    wait_srs(&mut c, "waiting", |d| {
        d["phase"] == "waiting" && d["prediction"].is_object()
    })
    .await;
    c.ok("srs.fire", json!({})).await;
    let firing = wait_srs(&mut c, "firing", |d| d["phase"] == "firing").await;
    assert_eq!(firing["next_pulse_s"], Value::Null);
    let e = c.refused("srs.fire", json!({})).await;
    assert_eq!(e["reason"], "not_yet", "{e}");
    let t0 = Instant::now();
    obs.ok("srs.abort", json!({})).await;
    let end = wait_srs(&mut c, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_abort");
    assert!(t0.elapsed() < Duration::from_secs(10));
    assert!(end["end"]["message"]
        .as_str()
        .unwrap()
        .contains("Aborted by"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_srs_run_and_random_mimo_or_classical_shock_exclude_each_other() {
    let svc = service(0.0, Duration::from_secs(30), Duration::from_secs(5));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, table()).await;

    // SRS live (armed): Random, MIMO, classical shock, a simulation and the table
    // are refused.
    c.ok(
        "srs.configure",
        json!({"config": config("manual", true, 0.05)}),
    )
    .await;
    let armed = c.ok("srs.arm", json!({})).await;
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
    let e = c
        .refused("shock.configure", json!({"config": shock_config()}))
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
    // A law check is a run of its own.
    let e = c.refused("laws.check", json!({"id": "srs_ratio"})).await;
    assert_eq!(e["reason"], "run_active", "{e}");
    c.ok("srs.stop", json!({})).await;
    wait_srs(&mut c, "finished", |d| d["state"] == "finished").await;

    // Random live (armed): SRS shock is refused.
    c.ok("random.configure", json!({"config": random})).await;
    c.ok("random.arm", json!({})).await;
    let e = c
        .refused(
            "srs.configure",
            json!({"config": config("manual", true, 0.05)}),
        )
        .await;
    assert_eq!(e["reason"], "run_active", "{e}");
    c.ok("random.stop", json!({})).await;
    c.wait_state("finished", LONG).await;
    c.ok("random.clear", json!({})).await;

    // Classical shock live (armed): SRS shock is refused, and a configured SRS run
    // cannot arm.
    c.ok(
        "srs.configure",
        json!({"config": config("manual", true, 0.05)}),
    )
    .await;
    c.ok("shock.configure", json!({"config": shock_config()}))
        .await;
    c.ok("shock.arm", json!({})).await;
    let e = c
        .refused(
            "srs.configure",
            json!({"config": config("manual", true, 0.05)}),
        )
        .await;
    assert_eq!(e["reason"], "run_active", "{e}");
    let e = c.refused("srs.arm", json!({})).await;
    assert_eq!(e["reason"], "run_active", "{e}");
    c.ok("shock.stop", json!({})).await;
    c.wait_for("shock finished", LONG, |e| {
        e.topic() == "shock.state" && e.data()["state"] == "finished"
    })
    .await;
    c.ok("shock.clear", json!({})).await;

    // MIMO live: SRS shock is refused, and the configured SRS run cannot arm.
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
            "srs.configure",
            json!({"config": config("manual", true, 0.05)}),
        )
        .await;
    assert_eq!(e["reason"], "run_active", "{e}");
    let e = c.refused("srs.arm", json!({})).await;
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
        "srs.configure",
        json!({"config": config("manual", false, 0.05)}),
    )
    .await;
    c.ok("srs.start", json!({})).await;
    wait_srs(&mut c, "waiting", |d| {
        d["phase"] == "waiting" && d["prediction"].is_object()
    })
    .await;
    let said = svc.shutdown().await;
    assert!(said.is_some_and(|s| s.contains("safe_stop")));
    for (m, p) in [
        ("srs.arm", json!({})),
        ("srs.start", json!({})),
        ("srs.proceed", json!({})),
        ("srs.fire", json!({})),
        ("srs.level", json!({"delta_db": 1})),
    ] {
        assert_eq!(c.refused(m, p).await["reason"], "shutting_down", "{m}");
    }
    let s = c.ok("srs.state", json!({})).await;
    assert_eq!(s["end"]["outcome"], "safe_stop");
    assert!(s["end"]["message"].as_str().unwrap().contains("shut down"));
    // Reading and computing still work.
    c.ok("srs.pulses", json!({})).await;
    c.ok("srs.config", json!({})).await;
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
    cfg["schedule"]["pulses"] = json!([3, 3, 3]);
    c.ok("srs.configure", json!({"config": cfg.clone()})).await;
    c.ok("srs.start", json!({})).await;
    obs.wait_for("two pulses", LONG, |e| {
        e.topic() == "srs.state" && e.data()["pulses"]["fired"] == 2
    })
    .await;
    drop(c);
    let dropped_at = Instant::now();
    obs.pump(Duration::from_millis(900)).await;
    let held = obs.ok("srs.pulses", json!({})).await["next"]
        .as_u64()
        .unwrap();
    obs.pump(Duration::from_millis(400)).await;
    assert_eq!(
        obs.ok("srs.pulses", json!({})).await["next"]
            .as_u64()
            .unwrap(),
        held,
        "no pulse while nobody holds control"
    );
    assert!(
        held <= 3,
        "at most the pulse in flight finished after the drop: {held}"
    );
    let s = obs.ok("srs.state", json!({})).await;
    schema.assert_valid(&s, Some("srsState"));
    assert_eq!(s["state"], "running", "{s}");
    assert!(dropped_at.elapsed() < timeout);

    // A reclaim within the grace resumes the run.
    let mut c2 = Client::connect(addr).await;
    let back = c2
        .ok("session.claim", json!({"name": "Shell", "lease": lease}))
        .await;
    assert_eq!(back["resumed"], true, "{back}");
    let end = wait_srs(&mut c2, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "completed");
    assert_eq!(end["pulses"]["fired"], 9);

    // Without a reclaim the orphan clock safe-stops it.
    c2.ok("srs.configure", json!({"config": cfg})).await;
    c2.ok("srs.start", json!({})).await;
    obs.wait_for("a pulse of the second run", LONG, |e| {
        e.topic() == "srs.state"
            && e.data()["run_id"] != end["run_id"]
            && e.data()["pulses"]["fired"] == 1
    })
    .await;
    drop(c2);
    let i = obs
        .wait_for("the safe stop", LONG, |e| {
            e.topic() == "srs.state"
                && e.data()["run_id"] != end["run_id"]
                && e.data()["state"] == "finished"
        })
        .await;
    let fin = obs.events[i].data().clone();
    assert_eq!(fin["end"]["outcome"], "safe_stop", "{fin}");
    assert!(fin["pulses"]["fired"].as_u64().unwrap() <= 2);
}
