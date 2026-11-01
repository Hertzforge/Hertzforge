//! Closed-loop Random through the protocol, on the simulator, over a real loopback
//! WebSocket: the session, the station, the channel table, the configuration, the
//! run's state machine and outcomes, and the live frames checked against the schema
//! and the golden frame (schema/PROTOCOL.md, "Hardware and Random Control").
//!
//! Every run here is SIMULATED (`hertzforge-hal-sim` through the station factory);
//! nothing in this file is evidence about hardware. The NI cases use a fixed scan of
//! fictional devices, so no test can open real hardware on a bench computer.

mod support;

use hertzforge_hal::{DiscoveredChannel, DiscoveredDevice};
use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use sds_control::views;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use support::schema::Schema;
use support::{options, serve, sim_config, Client};

const LONG: Duration = Duration::from_secs(60);

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

/// A fictional chassis with a four-input and a four-output module. The names are
/// not ones NI hardware uses, so a test run on a bench computer cannot reach a real
/// device.
fn fictional_rig(simulated: bool) -> Vec<DiscoveredDevice> {
    let dev = |id: &str, product: &str, n_in: usize, n_out: usize, slot: Option<u32>| {
        let mut d = DiscoveredDevice::new(id, product);
        d.serial = if simulated {
            String::new()
        } else {
            format!("T-{id}")
        };
        d.simulated = simulated;
        if slot.is_some() {
            d.parent = Some("TestRig9".into());
            d.slot = slot;
        }
        d.inputs = (0..n_in)
            .map(|k| DiscoveredChannel::new(format!("{id}/ai{k}"), format!("ai{k}")))
            .collect();
        d.outputs = (0..n_out)
            .map(|k| DiscoveredChannel::new(format!("{id}/ao{k}"), format!("ao{k}")))
            .collect();
        d
    };
    vec![
        dev("TestRig9", "test chassis", 0, 0, None),
        dev("TestRig9Mod1", "test input module", 4, 0, Some(1)),
        dev("TestRig9Mod2", "test output module", 0, 4, Some(2)),
    ]
}

fn golden_live() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../schema/golden/event_random_live.expected.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn keys(v: &Value) -> Vec<String> {
    v.as_object()
        .map_or_else(Vec::new, |o| o.keys().cloned().collect())
}

/// A row for the channel table with every field.
fn row(index: u32, role: &str, units: &str, physical: Value) -> Value {
    json!({
        "index": index, "enabled": true, "description": format!("{role} {index}"),
        "role": role, "physical": physical, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": if units == "V" { 1000 } else { 100 },
        "range_max": if units == "V" { 10 } else { 100 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

/// Claim control, select the simulator, and configure `config`.
async fn configured(c: &mut Client, config: Value) -> Value {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    c.ok("random.configure", json!({ "config": config })).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn random_on_the_simulator_runs_end_to_end_through_the_protocol() {
    let schema = Schema::load();
    let svc = Service::with_options(options(
        no_driver(),
        4.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;

    // Every connection starts as an observer.
    let s = c.ok("session.state", json!({})).await;
    schema.assert_valid(&s, Some("sessionState"));
    assert_eq!(s["role"], "observer");
    assert_eq!(s["controller"], Value::Null);
    let claim = c.ok("session.claim", json!({"name": "Shell"})).await;
    schema.assert_valid(&claim, Some("sessionClaimResult"));
    assert_eq!(claim["role"], "controller");

    // Stations: the simulator is selected; NI says why it is unavailable.
    let list = c.ok("station.list", json!({})).await;
    schema.assert_valid(&list, Some("stationList"));
    assert_eq!(list["selected"]["class"], "sim");
    assert_eq!(
        list["stations"][1]["unavailable"]["reason"],
        "ni_driver_unavailable"
    );
    let sel = c.ok("station.select", json!({"kind": "sim"})).await;
    assert_eq!(sel["selected"]["source"], "SIM");

    // The channel table: two unbound single-ended rows; add a monitor input.
    let t = c.ok("channels.get", json!({})).await;
    schema.assert_valid(&t, Some("channelTable"));
    assert_eq!(t["revision"], 1);
    for r in t["channels"].as_array().unwrap() {
        assert_eq!(r["interface"], "single_ended", "IEPE is off by default");
        assert_eq!(r["physical"], Value::Null);
    }
    let mut rows = t["channels"].as_array().unwrap().clone();
    rows.push(row(3, "monitor", "g", Value::Null));
    let dry = c
        .ok(
            "channels.set",
            json!({"revision": 1, "channels": rows, "dry_run": true}),
        )
        .await;
    assert_eq!(dry["revision"], 1, "a dry run writes nothing");
    let set = c
        .ok("channels.set", json!({"revision": 1, "channels": rows}))
        .await;
    schema.assert_valid(&set, Some("channelsSetResult"));
    assert_eq!(set["revision"], 2);
    let stale = c
        .refused("channels.set", json!({"revision": 1, "channels": rows}))
        .await;
    assert_eq!(stale["reason"], "stale_revision");
    assert_eq!(stale["detail"], json!({"current": 2, "provided": 1}));

    // Configure: a dry run, then the real one.
    let config = sim_config("auto", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 2.0);
    let dry = c
        .ok(
            "random.configure",
            json!({"config": config, "dry_run": true}),
        )
        .await;
    schema.assert_valid(&dry, Some("randomDryRunResult"));
    assert!(dry["errors"].as_array().unwrap().is_empty(), "{dry}");
    // The dry run carries the profile on the requested grid, stepped, to draw.
    let d = c
        .decoded_response(&json!(c.last_id))
        .expect("a binary dry-run result");
    let reference = &dry["views"][0];
    assert_eq!(reference["title"], "Reference");
    let lines = d.array(&reference["series"][0]["x"]).unwrap().to_f64();
    assert_eq!(
        lines.len() as u64,
        dry["derived"]["grid"]["lines"].as_u64().unwrap()
    );
    for s in reference["series"].as_array().unwrap() {
        assert_eq!(s["shape"], "steps");
        assert_eq!(
            d.array(&s["edges"]).unwrap().to_f64(),
            views::bin_edges(&lines)
        );
    }
    let r = d.array(&reference["series"][0]["y"]).unwrap().to_f64();
    let hi = d.array(&reference["series"][1]["y"]).unwrap().to_f64();
    assert!(
        (hi[60] / r[60] - 10f64.powf(0.3)).abs() < 1e-9,
        "the alarm band is +3 dB"
    );
    let none = c.ok("random.config", json!({})).await;
    schema.assert_valid(&none, Some("randomConfigResult"));
    assert_eq!(none["run_id"], Value::Null);
    let configured = c.ok("random.configure", json!({ "config": config })).await;
    schema.assert_valid(&configured, Some("randomConfigureResult"));
    let state = &configured["state"];
    assert_eq!(state["state"], "configured");
    assert_eq!(state["source"], "SIM");
    assert_eq!(state["grid"]["actual"], false);
    assert_eq!(configured["derived"]["grid"]["df_hz"], 16);
    assert_eq!(configured["derived"]["control_channels"], json!([1]));
    let run_id = state["run_id"].as_str().unwrap().to_owned();

    // A second screen reads back what the run was configured with.
    let mut second = Client::connect(addr).await;
    let back = second.ok("random.config", json!({})).await;
    schema.assert_valid(&back, Some("randomConfigResult"));
    assert_eq!(back["run_id"], run_id.as_str());
    assert_eq!(back["config"], config);
    assert_eq!(back["derived"], configured["derived"]);
    assert_eq!(back["views"][0]["series"].as_array().unwrap().len(), 5);
    drop(second);

    // Start answers at once: running, system identification.
    let started = c.ok("random.start", json!({})).await;
    schema.assert_valid(&started, Some("randomState"));
    assert_eq!(started["state"], "running");
    assert_eq!(started["phase"], "sysid");
    assert_eq!(started["grid"]["actual"], true);
    assert_eq!(started["clock"], "shared");
    assert_eq!(started["level_db"], -20, "the identification level");

    let end = c.wait_state("finished", LONG).await;
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    c.wait_for("the final live frame", LONG, |e| {
        e.topic() == "random.live" && e.data()["state"] == "finished"
    })
    .await;
    assert_eq!(end["phase"], "stopped");
    assert_eq!(end["end"]["trip"], Value::Null);

    // Every state event is a valid state object; the phases follow the controller.
    let phases: Vec<String> = c
        .of_topic("random.state")
        .iter()
        .map(|e| {
            schema.assert_valid(e.data(), Some("randomState"));
            schema.assert_valid(&e.msg, None);
            assert_eq!(e.data()["run_id"], run_id.as_str());
            e.data()["phase"].as_str().unwrap().to_owned()
        })
        .collect();
    let mut seen: Vec<&str> = Vec::new();
    for p in &phases {
        if seen.last() != Some(&p.as_str()) {
            seen.push(p);
        }
    }
    for want in [
        "sysid",
        "coherence_gate",
        "engaged",
        "full_level",
        "stopped",
    ] {
        assert!(seen.contains(&want), "{want} missing from {seen:?}");
    }
    let order = |p: &str| seen.iter().position(|x| *x == p).unwrap();
    assert!(order("sysid") < order("coherence_gate"));
    assert!(order("coherence_gate") < order("engaged"));
    assert!(order("engaged") < order("full_level"));

    // The live frames: valid, stepped on the run's own lines, in the golden's shape.
    let golden = golden_live();
    let live = c.of_topic("random.live");
    assert!(live.len() >= 3, "{} live frames", live.len());
    for e in &live {
        schema.assert_valid(&e.msg, None);
        schema.assert_valid(e.data(), Some("randomLive"));
        let d = e.decoded.as_ref().unwrap();
        assert_eq!(
            keys(e.data()),
            keys(&golden["data"]),
            "live keys in contract order"
        );
        let v = e.data()["views"].as_array().unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0]["title"], "Control");
        assert_eq!(v[1]["title"], "Drive");
        assert_eq!(v[2]["view"], "reading");
        let control = v[0]["series"].as_array().unwrap();
        let roles: Vec<&str> = control
            .iter()
            .map(|s| s["role"].as_str().unwrap())
            .collect();
        let tail = ["reference", "tolerance", "tolerance", "abort", "abort"];
        assert!(
            roles == tail || roles[..1] == ["measured"] && roles[1..] == tail,
            "{roles:?}"
        );
        let lines = d.array(&control[0]["x"]).unwrap().to_f64();
        assert_eq!(
            lines.len() as u64,
            e.data()["lines"]["in_band"].as_u64().unwrap()
        );
        for s in control.iter().chain(v[1]["series"].as_array().unwrap()) {
            assert_eq!(s["shape"], "steps");
            assert_eq!(d.array(&s["x"]).unwrap().to_f64(), lines);
            assert_eq!(
                d.array(&s["edges"]).unwrap().to_f64(),
                views::bin_edges(&lines),
                "each line drawn flat across its own bin"
            );
        }
        assert_eq!(e.data()["source"], "SIM");
        assert_eq!(e.data()["class"], "sim");
        let chans: Vec<u64> = e.data()["channels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["index"].as_u64().unwrap())
            .collect();
        assert_eq!(chans, [1, 3], "every enabled input, in table order");
    }
    let last = live.last().unwrap().data();
    assert_eq!(
        last["state"], "finished",
        "one more frame with the final state"
    );
    assert_eq!(last["level_db"], Value::Null);

    // The loop controlled: at full level the control rms is on the reference.
    let at_full: Vec<&Value> = live
        .iter()
        .map(|e| e.data())
        .filter(|d| d["phase"] == "full_level" && d["control_rms_g"].is_number())
        .collect();
    let f = at_full.last().expect("a live frame at full level");
    let err_db = 20.0
        * (f["control_rms_g"].as_f64().unwrap() / f["reference_rms_g"].as_f64().unwrap()).log10();
    assert!(
        err_db.abs() < 1.5,
        "control {err_db:+.2} dB from the reference at full level"
    );
    let monitor = &f["channels"][1];
    assert!(monitor["rms"].as_f64().unwrap() > 0.0, "{monitor}");

    // The run log: from configure, the gate and the levels.
    let log = c.ok("random.log", json!({})).await;
    schema.assert_valid(&log, Some("randomLogResult"));
    let kinds: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    for want in ["state", "gate", "level", "station", "note"] {
        assert!(kinds.contains(&want), "{want} missing from {kinds:?}");
    }
    let tail = c.ok("random.log", json!({"since": log["next"]})).await;
    assert!(tail["entries"].as_array().unwrap().is_empty());
    for e in c.of_topic("random.log") {
        schema.assert_valid(e.data(), Some("randomLogEvent"));
        assert_eq!(e.data()["run_id"], run_id.as_str());
    }

    // Finished: arming again is refused by name; reset makes a new run.
    let e = c.refused("random.arm", json!({})).await;
    assert_eq!(e["reason"], "run_finished");
    let reset = c.ok("random.reset", json!({})).await;
    assert_eq!(reset["state"], "configured");
    assert_ne!(reset["run_id"], run_id.as_str());
    let cleared = c.ok("random.clear", json!({})).await;
    schema.assert_valid(&cleared, Some("randomState"));
    assert_eq!(cleared["state"], "unconfigured");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_levels_mode_and_the_controlled_stop() {
    let schema = Schema::load();
    let svc = Service::with_options(options(
        no_driver(),
        2.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let config = sim_config(
        "manual",
        json!([{"db": -9, "dwell_s": 1}, {"db": -6, "dwell_s": 1}, {"db": 0}]),
        60.0,
    );
    configured(&mut c, config).await;
    let e = c.refused("random.level", json!({"delta_db": 1})).await;
    assert_eq!(e["reason"], "wrong_state", "no level before the run drives");
    c.ok("random.start", json!({})).await;
    c.wait_phase("engaged", LONG).await;
    // A step is taken from the level the loop runs at, so let the soft start arrive.
    c.wait_for("the soft start at -9 dB", LONG, |e| {
        e.topic() == "random.live"
            && e.data()["level_db"]
                .as_f64()
                .is_some_and(|l| (l + 9.0).abs() < 0.05)
    })
    .await;

    let r = c.ok("random.level", json!({"delta_db": 1})).await;
    schema.assert_valid(&r, Some("randomLevelResult"));
    assert_eq!(r["level_db"], -8);
    let r = c.ok("random.level", json!({"to_db": -6})).await;
    assert_eq!(r["level_db"], -6);
    let e = c.refused("random.level", json!({"to_db": -5})).await;
    assert_eq!(e["reason"], "not_a_rung");
    assert_eq!(e["detail"]["levels_db"], json!([-9, -6, 0]));
    let e = c.refused("random.level", json!({})).await;
    assert_eq!(e["reason"], "bad_params");
    let state = c.ok("random.state", json!({})).await;
    assert_eq!(state["level_db"], -6, "the commanded level");

    // Hand the ladder to auto: level verbs need manual.
    assert_eq!(
        c.ok("random.mode", json!({"mode": "auto"})).await["mode"],
        "auto"
    );
    let e = c.refused("random.level", json!({"delta_db": 1})).await;
    assert_eq!(e["reason"], "manual_mode_required");
    c.ok("random.mode", json!({"mode": "manual"})).await;

    // The live display walks to the commanded level over the ramp.
    c.wait_for("a live frame at -6 dB", LONG, |e| {
        e.topic() == "random.live"
            && e.data()["level_db"]
                .as_f64()
                .is_some_and(|l| (l + 6.0).abs() < 0.05)
    })
    .await;

    // The controlled stop ramps down, then ends operator_stop.
    let stopped = c.ok("random.stop", json!({})).await;
    schema.assert_valid(&stopped, Some("randomState"));
    assert_eq!(stopped["state"], "stopping", "{stopped}");
    assert_eq!(stopped["phase"], "ramping_down");
    let end = c.wait_state("finished", LONG).await;
    assert_eq!(end["end"]["outcome"], "operator_stop");
    let message = end["end"]["message"].as_str().unwrap();
    assert!(message.contains("Shell (controller)"), "{message}");
    assert!(
        end["end"]["level_db"].is_number(),
        "it ended from a driven level"
    );
    // The ramp took more than one loop: the live level fell before the end.
    let falling = c
        .of_topic("random.live")
        .iter()
        .filter(|e| e.data()["state"] == "stopping")
        .count();
    assert!(falling >= 1, "no live frame while stopping");
    // Stop on a finished run answers with the state, unchanged.
    let again = c.ok("random.stop", json!({})).await;
    assert_eq!(again["end"]["outcome"], "operator_stop");
}

/// The controlled stop starts at the next output block, not the next control
/// loop, and walks down smoothly (2026-11-01, bench finding 2). In real time,
/// with a one-second control loop and a 2 s, 40 dB ramp, the live level is
/// below the stop level within 0.6 s of the request and falls in many small
/// steps. Red before the fix: the level held for the rest of the loop in
/// progress and the next one, then fell 26 dB per loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_controlled_stop_starts_at_once_and_walks_down_smoothly() {
    let svc = Service::with_options(options(
        no_driver(),
        1.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut config = sim_config("manual", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 60.0);
    config["acquisition"]["n_avg"] = json!(16); // a one-second control loop
    config["control"]["shutdown_ramp_s"] = json!(2.0);
    config["control"]["soft_start_db"] = json!(0);
    configured(&mut c, config).await;
    c.ok("random.start", json!({})).await;
    c.wait_phase("engaged", LONG).await;
    c.pump(Duration::from_millis(2300)).await; // two loops at -6 dB, then mid-loop

    let t0 = Instant::now();
    let stopping = c.ok("random.stop", json!({})).await;
    assert_eq!(stopping["state"], "stopping", "{stopping}");
    let mut seen: Vec<(f64, f64)> = Vec::new();
    let deadline = t0 + LONG;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Some(m) = c.read(left).await else {
            panic!("the stop did not finish");
        };
        if m["type"] == "event" && m["topic"] == "random.state" && m["data"]["state"] == "finished"
        {
            break;
        }
        if let Some(e) = c.events.last() {
            if e.topic() == "random.live" && e.data()["state"] == "stopping" {
                if let Some(l) = e.data()["level_db"].as_f64() {
                    seen.push((t0.elapsed().as_secs_f64(), l));
                }
            }
        }
    }
    println!("live levels while stopping: {seen:?}");
    let first_down = seen.iter().find(|(_, l)| *l < -6.5).map(|(t, _)| *t);
    assert!(
        first_down.is_some_and(|t| t < 0.6),
        "the drive was still at -6 dB {first_down:?} s after the stop"
    );
    let mut distinct: Vec<f64> = seen.iter().map(|(_, l)| *l).collect();
    distinct.dedup();
    assert!(
        distinct.len() >= 6,
        "the level fell in {} steps: {seen:?}",
        distinct.len()
    );
    let worst = distinct
        .windows(2)
        .map(|w| w[0] - w[1])
        .fold(0.0f64, f64::max);
    assert!(worst < 10.0, "a {worst:.1} dB step between two live frames");
    let end = c.wait_state("finished", LONG).await;
    assert_eq!(end["end"]["outcome"], "operator_stop", "{end}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_observer_aborts_and_the_answer_comes_when_the_loop_has_ended() {
    let svc = Service::with_options(options(
        no_driver(),
        2.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut o = Client::connect(addr).await;
    configured(
        &mut c,
        sim_config("manual", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 60.0),
    )
    .await;
    c.ok("random.start", json!({})).await;
    c.wait_phase("engaged", LONG).await;

    let e = o.refused("random.level", json!({"delta_db": 1})).await;
    assert_eq!(e["reason"], "not_controller");
    let t0 = Instant::now();
    let r = o.ok("random.abort", json!({})).await;
    let took = t0.elapsed();
    assert_eq!(
        r["state"], "finished",
        "abort answers when the loop has ended: {r}"
    );
    assert_eq!(r["phase"], "stopped");
    assert_eq!(r["end"]["outcome"], "operator_abort");
    assert!(
        r["end"]["message"].as_str().unwrap().contains("observer"),
        "the end names who aborted: {}",
        r["end"]["message"]
    );
    assert!(took < Duration::from_secs(2), "abort took {took:?}");
    let e = c.refused("random.start", json!({})).await;
    assert_eq!(e["reason"], "run_finished");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_abort_during_system_identification_ends_it_at_the_next_block() {
    // Real time, a long identification: 32 averages of a half-second loop.
    let svc = Service::with_options(options(
        no_driver(),
        1.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut config = sim_config("manual", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 60.0);
    config["acquisition"]["n_avg"] = json!(4);
    config["control"]["pretest"]["averages"] = json!(40);
    configured(&mut c, config).await;
    let s = c.ok("random.start", json!({})).await;
    assert_eq!(s["phase"], "sysid");
    c.pump(Duration::from_millis(500)).await;
    let t0 = Instant::now();
    let r = c.ok("random.abort", json!({})).await;
    assert_eq!(r["end"]["outcome"], "operator_abort", "{r}");
    assert!(
        t0.elapsed() < Duration::from_secs(2),
        "the abort waited for system identification ({:?})",
        t0.elapsed()
    );
}

/// System identification runs under the run's own aborts (2026-11-01, bench
/// finding 1): a control-row accel abort below the identification's peaks ends
/// the run `aborted` during identification, at loop 0 and the identification
/// level, before anything drives at test level. Red before the fix: the run
/// passed identification and tripped later, on the ladder.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kinematic_abort_trips_during_system_identification() {
    let svc = Service::with_options(options(
        no_driver(),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let t = c.ok("channels.get", json!({})).await;
    let mut rows = t["channels"].as_array().unwrap().clone();
    // The identification's control response is the reference rms -20 dB (about
    // 0.6 g rms for this profile): a 0.5 g abort sits under its peaks.
    rows[0]["abort"]["accel_g"] = json!(0.5);
    c.ok(
        "channels.set",
        json!({"revision": t["revision"], "channels": rows}),
    )
    .await;
    let config = sim_config("manual", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 60.0);
    c.ok("random.configure", json!({ "config": config })).await;
    c.ok("random.start", json!({})).await;
    let end = c.wait_state("finished", LONG).await;
    assert_eq!(end["end"]["outcome"], "aborted", "{end}");
    let trip = &end["end"]["trip"];
    assert_eq!(trip["kind"], "kinematic");
    assert_eq!(trip["channel"], 1);
    assert_eq!(
        trip["at_loop"], 0,
        "it tripped before any control loop: {trip}"
    );
    assert_eq!(trip["at_level_db"], -20);
    assert_eq!(end["end"]["level_db"], -20);
    let message = end["end"]["message"].as_str().unwrap();
    assert!(
        message.contains("during system identification"),
        "{message}"
    );
    // Nothing was ever engaged.
    assert!(
        !c.of_topic("random.state")
            .iter()
            .any(|e| e.data()["phase"] == "engaged"),
        "the run engaged after the identification tripped"
    );
}

/// A level the identification's ceiling cannot reach ends the run by name,
/// `sysid_level_unreachable`, with the facts in the message, before anything
/// drives at test level.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_identification_level_the_ceiling_cannot_reach_ends_the_run_by_name() {
    let svc = Service::with_options(options(
        no_driver(),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut config = sim_config("manual", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 60.0);
    config["control"]["pretest"]["max_vrms"] = json!(0.002);
    config["control"]["pretest"]["start_vrms"] = json!(0.001);
    configured(&mut c, config).await;
    c.ok("random.start", json!({})).await;
    let end = c.wait_state("finished", LONG).await;
    assert_eq!(end["end"]["outcome"], "sysid_level_unreachable", "{end}");
    assert_eq!(end["phase"], "stopped");
    assert_eq!(end["end"]["trip"], Value::Null);
    let message = end["end"]["message"].as_str().unwrap();
    assert!(
        message.contains("could not reach its level") && message.contains("0.002 V rms"),
        "{message}"
    );
    assert!(message.contains("never drove at test level"), "{message}");
    let log = c.ok("random.log", json!({})).await;
    let steps = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("identification level step"))
        })
        .count();
    assert!(steps >= 1, "the level steps are in the run log: {log}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn observers_read_stop_and_abort_and_nothing_else() {
    let svc = Service::with_options(options(
        no_driver(),
        2.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut o = Client::connect(addr).await;
    let config = sim_config("manual", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 60.0);
    configured(&mut c, config.clone()).await;
    let t = c.ok("channels.get", json!({})).await;

    let e = o.refused("session.claim", json!({})).await;
    assert_eq!(e["reason"], "controller_held");
    assert_eq!(e["detail"]["holder"], "Shell");
    for (method, params) in [
        ("station.select", json!({"kind": "sim"})),
        (
            "channels.set",
            json!({"revision": t["revision"], "channels": t["channels"]}),
        ),
        ("random.configure", json!({"config": config})),
        ("random.arm", json!({})),
        ("random.start", json!({})),
        ("random.proceed", json!({})),
        ("random.level", json!({"delta_db": 1})),
        ("random.mode", json!({"mode": "auto"})),
        ("random.reset", json!({})),
        ("random.clear", json!({})),
    ] {
        let e = o.refused(method, params).await;
        assert_eq!(e["reason"], "not_controller", "{method}: {e}");
        assert_eq!(e["detail"]["holder"], "Shell", "{method}");
    }
    // Reads and dry runs need no control.
    assert_eq!(o.ok("session.state", json!({})).await["role"], "observer");
    o.ok("station.list", json!({})).await;
    o.ok("channels.get", json!({})).await;
    o.ok("random.state", json!({})).await;
    o.ok("random.log", json!({})).await;
    o.ok(
        "channels.set",
        json!({"revision": t["revision"], "channels": t["channels"], "dry_run": true}),
    )
    .await;
    o.ok(
        "random.configure",
        json!({"config": config, "dry_run": true}),
    )
    .await;

    // Stop is open to an observer too.
    c.ok("random.start", json!({})).await;
    c.wait_phase("engaged", LONG).await;
    let r = o.ok("random.stop", json!({})).await;
    assert!(r["state"] == "stopping" || r["state"] == "finished", "{r}");
    let end = o.wait_state("finished", LONG).await;
    assert_eq!(end["end"]["outcome"], "operator_stop");
    assert!(end["end"]["message"].as_str().unwrap().contains("observer"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_controller_resumes_with_its_lease_and_the_run_goes_on() {
    let schema = Schema::load();
    let svc = Service::with_options(options(
        no_driver(),
        2.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut o = Client::connect(addr).await;
    configured(
        &mut c,
        sim_config("manual", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 60.0),
    )
    .await;
    let lease = c.ok("session.claim", json!({"name": "Shell"})).await["lease"]
        .as_str()
        .unwrap()
        .to_owned();
    c.ok("random.start", json!({})).await;
    c.wait_phase("engaged", LONG).await;

    // Gone without a goodbye: the TCP connection closes with no close frame.
    drop(c);
    let i = o
        .wait_for("the absent holder", LONG, |e| {
            e.topic() == "session.changed" && e.data()["controller"]["present"] == false
        })
        .await;
    let changed = o.events[i].data().clone();
    schema.assert_valid(&changed, Some("sessionChanged"));
    assert_eq!(changed["controller"]["name"], "Shell");
    assert_eq!(changed["role"], "observer", "the observer's own copy");
    let orphan = changed["orphan_s"].as_f64().unwrap();
    assert!(orphan > 25.0 && orphan <= 30.0, "{orphan}");

    // The same lease resumes control; the run never stopped.
    let mut back = Client::connect(addr).await;
    let r = back.ok("session.claim", json!({"lease": lease})).await;
    assert_eq!(r["resumed"], true);
    let i = back
        .wait_for("the holder's own copy", LONG, |e| {
            e.topic() == "session.changed" && e.data()["role"] == "controller"
        })
        .await;
    assert_eq!(back.events[i].data()["controller"]["present"], true);
    let i = o
        .wait_for("the observer's copy of the resume", LONG, |e| {
            e.topic() == "session.changed"
                && e.data()["controller"]["present"] == true
                && e.data()["role"] == "observer"
        })
        .await;
    assert_eq!(o.events[i].data()["controller"]["name"], "Shell");
    assert_eq!(r["lease"], lease.as_str());
    let state = back.ok("random.state", json!({})).await;
    assert_eq!(state["state"], "running");
    assert_eq!(
        back.ok("session.state", json!({})).await["orphan_s"],
        Value::Null
    );
    back.ok("random.abort", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unattended_run_ends_with_the_safe_stop_and_its_ramp() {
    let svc = Service::with_options(options(
        no_driver(),
        2.0,
        Duration::from_millis(1500),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut o = Client::connect(addr).await;
    configured(
        &mut c,
        sim_config("manual", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 60.0),
    )
    .await;
    c.ok("random.start", json!({})).await;
    c.wait_phase("engaged", LONG).await;
    drop(c);

    let stopping = o.wait_state("stopping", LONG).await;
    assert_eq!(
        stopping["phase"], "ramping_down",
        "the safe stop ramps the level down"
    );
    let end = o.wait_state("finished", LONG).await;
    assert_eq!(end["end"]["outcome"], "safe_stop", "{end}");
    assert_eq!(end["end"]["trip"], Value::Null);
    // The seat was given up with the run.
    let s = o.ok("session.state", json!({})).await;
    assert_eq!(s["controller"], Value::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_goodbye_frees_control_at_once_and_keeps_the_run_for_the_timeout() {
    let svc = Service::with_options(options(
        no_driver(),
        2.0,
        Duration::from_millis(1500),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut o = Client::connect(addr).await;
    configured(
        &mut c,
        sim_config("manual", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 60.0),
    )
    .await;
    c.ok("random.start", json!({})).await;
    c.wait_phase("engaged", LONG).await;
    c.ws.close(None).await.unwrap();
    let i = o
        .wait_for("control free", LONG, |e| {
            e.topic() == "session.changed" && e.data()["controller"].is_null()
        })
        .await;
    assert!(o.events[i].data()["orphan_s"].as_f64().unwrap() > 1.0);
    assert_eq!(o.ok("random.state", json!({})).await["state"], "running");
    let end = o.wait_state("finished", LONG).await;
    assert_eq!(end["end"]["outcome"], "safe_stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_silent_connection_is_gone_after_three_heartbeats() {
    let svc = Service::with_options(options(
        no_driver(),
        1.0,
        Duration::from_secs(30),
        Duration::from_millis(100),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut o = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    // `o` reads (and so answers the pings); `c` stops reading entirely.
    o.pump(Duration::from_millis(250)).await;
    let s = o.ok("session.state", json!({})).await;
    assert_eq!(s["controller"]["present"], true, "pinged clients stay");
    let i = o
        .wait_for("the silent holder dropped", Duration::from_secs(5), |e| {
            e.topic() == "session.changed" && e.data()["controller"]["present"] == false
        })
        .await;
    assert_eq!(o.events[i].data()["controller"]["name"], "Shell");
    drop(c);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sim_and_random_share_the_one_run_slot() {
    let svc = Service::with_options(options(
        no_driver(),
        2.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    configured(
        &mut c,
        sim_config("manual", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 60.0),
    )
    .await;
    c.ok(
        "sim.start",
        json!({"profile": "ramp_plateau", "speed": 1, "duration_s": 60}),
    )
    .await;
    let e = c.refused("random.start", json!({})).await;
    assert_eq!(e["reason"], "run_active");
    c.ok("sim.stop", json!({})).await;
    c.ok("random.start", json!({})).await;
    let e = c
        .refused("sim.start", json!({"profile": "ramp_plateau", "speed": 1}))
        .await;
    assert_eq!(e["reason"], "run_active");
    let e = c.refused("station.select", json!({"kind": "sim"})).await;
    assert_eq!(e["reason"], "run_active");
    c.ok("random.abort", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ni_selection_binding_and_arm_are_refused_by_name_and_never_simulated() {
    let schema = Schema::load();
    let svc = Service::with_options(options(
        FixedScanner(Ok(fictional_rig(false))),
        1.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;

    let list = c.ok("station.list", json!({"rescan": true})).await;
    schema.assert_valid(&list, Some("stationList"));
    let devices = list["stations"][1]["devices"].as_array().unwrap();
    assert_eq!(devices[1]["timing_family"], "TestRig9");
    assert_eq!(
        devices[1]["inputs"][0],
        json!({"id": "ai0", "label": "ai0"})
    );

    let e = c
        .refused(
            "station.select",
            json!({"kind": "ni", "devices": ["NoSuchDev"]}),
        )
        .await;
    assert_eq!(e["reason"], "devices_absent");
    let e = c.refused("station.select", json!({"kind": "ni"})).await;
    assert_eq!(e["reason"], "bad_params");
    let sel = c
        .ok(
            "station.select",
            json!({"kind": "ni", "devices": ["TestRig9Mod1", "TestRig9Mod2"]}),
        )
        .await;
    assert_eq!(sel["selected"]["class"], "ni_real");
    assert_eq!(sel["selected"]["source"], "HARDWARE");

    // Unbound rows are refused on NI.
    let t = c.ok("channels.get", json!({})).await;
    let reasons: Vec<&str> = t["report"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["reason"].as_str().unwrap())
        .collect();
    assert_eq!(reasons, ["channel_unbound", "channel_unbound"]);
    let config = sim_config("manual", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 60.0);
    let e = c
        .refused("random.configure", json!({"config": config}))
        .await;
    assert_eq!(e["reason"], "table_invalid");
    schema.assert_valid(&e["detail"], Some("report"));

    // Bound: input to an input, drive to an output.
    let rows = json!([
        row(
            1,
            "control",
            "g",
            json!({"device": "TestRig9Mod1", "channel": "ai0"})
        ),
        row(
            2,
            "drive",
            "V",
            json!({"device": "TestRig9Mod2", "channel": "ao0"})
        ),
    ]);
    let wrong = json!([
        row(
            1,
            "control",
            "g",
            json!({"device": "TestRig9Mod2", "channel": "ao1"})
        ),
        row(
            2,
            "drive",
            "V",
            json!({"device": "TestRig9Mod2", "channel": "ao0"})
        ),
    ]);
    let e = c
        .refused(
            "channels.set",
            json!({"revision": t["revision"], "channels": wrong}),
        )
        .await;
    assert_eq!(
        e["detail"]["errors"][0]["reason"],
        "physical_channel_wrong_direction"
    );
    c.ok(
        "channels.set",
        json!({"revision": t["revision"], "channels": rows}),
    )
    .await;
    let configured = c.ok("random.configure", json!({ "config": config })).await;
    assert_eq!(configured["state"]["source"], "HARDWARE");
    assert_eq!(configured["state"]["station"]["class"], "ni_real");

    // No NI driver can open these devices: the arm is refused by name, and the
    // run never becomes a simulated one.
    let e = c.refused("random.arm", json!({})).await;
    assert!(
        e["reason"] == "arm_refused" || e["reason"] == "ni_driver_unavailable",
        "{e}"
    );
    let s = c.ok("random.state", json!({})).await;
    assert_eq!(s["state"], "configured");
    assert_eq!(s["station"]["class"], "ni_real");
    assert!(c.of_topic("random.live").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn driver_simulated_ni_is_never_hardware() {
    let svc = Service::with_options(options(
        FixedScanner(Ok(fictional_rig(true))),
        1.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({})).await;
    let sel = c
        .ok(
            "station.select",
            json!({"kind": "ni", "devices": ["TestRig9Mod1", "TestRig9Mod2"]}),
        )
        .await;
    assert_eq!(sel["selected"]["class"], "ni_simulated");
    assert_eq!(sel["selected"]["source"], "SIM");
}

#[test]
fn the_service_refuses_a_non_loopback_host_at_startup() {
    for host in ["0.0.0.0", "192.168.1.20", "::"] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_sds-control"))
            .args(["--host", host, "--port", "0"])
            .output()
            .expect("the binary runs");
        assert_eq!(out.status.code(), Some(2), "--host {host}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("loopback"), "--host {host}: {err}");
    }
    for host in ["127.0.0.1", "::1", "localhost", "127.0.0.9"] {
        assert!(sds_control::server::check_loopback(host).is_ok(), "{host}");
    }
}
