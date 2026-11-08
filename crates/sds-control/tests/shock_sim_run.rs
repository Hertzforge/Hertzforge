//! Classical shock end to end on the simulator, through the wire, on the
//! controller's own classical shock service: the sds-control Sim station (200 and
//! 550 Hz at Q 2.5), unpaced, through a WebSocket client.
//!
//! Every run here is the closed loop (identification, the drive in volts, the law
//! and its guards). The law files are the ones scripts/build_laws.sh builds; the
//! template's law is built from the SDK template's own source as the law host's
//! `shock-template` fixture.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::{Service, ServiceOptions};
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use support::schema::Schema;
use support::{options, serve, sim_config, Client};

const LONG: Duration = Duration::from_secs(120);

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

/// The service on the controller's own classical shock service.
fn real(timeout: Duration) -> ServiceOptions {
    let mut o = options(no_driver(), 0.0, timeout, Duration::from_secs(5));
    o.shock_engine = None;
    o.mimo_driver = None;
    o
}

fn row(index: u32, role: &str) -> Value {
    let units = if role == "drive" { "V" } else { "g" };
    json!({
        "index": index, "enabled": true, "description": role,
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": 1000,
        "range_max": 10, "polarity": "positive", "control_group": null,
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

/// A 1 g half-sine of 11 ms at 20480 Hz, the default ladder advancing on a pass,
/// unpaced spacing, the default law.
fn config(mode: &str) -> Value {
    json!({
        "title": "Simulated Shock",
        "pulse": {"shape": "half_sine", "amplitude_g": 1, "duration_ms": 11},
        "acquisition": {"fs_hz": 20480},
        "schedule": {"mode": mode, "levels_db": [-12, -9, -6, -3, 0], "pulses": [1, 1, 1, 1, 3],
                     "advance": "on_pass", "max_pulses_per_level": 5},
        "spacing": {"cooling_s": 0.05, "settling_s": 0, "compute_s": 0},
        "identification": {"duration_s": 2, "nfft": 2048, "review": false},
        "limits": {"drive_vpk_ceiling": 2, "drive_vpk_abort": 3}
    })
}

async fn run_id(c: &mut Client, cfg: Value) -> Value {
    c.ok("shock.configure", json!({"config": cfg})).await["state"]["run_id"].clone()
}

async fn wait_end(c: &mut Client, id: &Value) -> Value {
    let i = c
        .wait_for("the run's end", LONG, |e| {
            e.topic() == "shock.state"
                && &e.data()["run_id"] == id
                && e.data()["state"] == "finished"
        })
        .await;
    c.events[i].data().clone()
}

async fn rows(c: &mut Client) -> Vec<Value> {
    c.ok("shock.pulses", json!({})).await["pulses"]
        .as_array()
        .unwrap()
        .clone()
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

/// AUTO with the default law completes; every full-level pulse passes; the
/// first pass by pulse 3; full-level error at most 5 %.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_with_the_default_law_completes_inside_the_standard() {
    let schema = Schema::load();
    let addr = serve(Service::with_options(real(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let id = run_id(&mut c, config("auto")).await;
    c.ok("shock.start", json!({})).await;
    let end = wait_end(&mut c, &id).await;
    schema.assert_valid(&end, Some("shockState"));
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    let rows = rows(&mut c).await;
    let first_pass = rows.iter().position(|r| r["passed"] == true).unwrap();
    assert!(first_pass <= 2, "first pass at pulse {}", first_pass + 1);
    for r in rows.iter().filter(|r| r["scheduled_db"] == 0) {
        assert_eq!(r["passed"], true, "{r}");
        assert!(r["error_pct"].as_f64().unwrap() <= 5.0, "{r}");
        assert!(r["drive_vpk"].as_f64().unwrap() > 0.0);
    }
}

/// MANUAL fire, two steps, a rung, two fires, an early fire refused, Stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_moves_and_an_early_fire_then_the_stop() {
    let addr = serve(Service::with_options(real(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let mut cfg = config("manual");
    cfg["spacing"]["cooling_s"] = json!(0.5);
    let id = run_id(&mut c, cfg).await;
    c.ok("shock.start", json!({})).await;
    c.wait_for("waiting", LONG, |e| {
        e.topic() == "shock.state"
            && e.data()["phase"] == "waiting"
            && e.data()["prediction"].is_object()
    })
    .await;
    c.ok("shock.fire", json!({})).await;
    assert_eq!(
        c.ok("shock.level", json!({"delta_db": 1})).await["level_db"],
        -11
    );
    assert_eq!(
        c.ok("shock.level", json!({"delta_db": 1})).await["level_db"],
        -10
    );
    assert_eq!(c.ok("shock.level", json!({"rung": 4})).await["level_db"], 0);
    tokio::time::sleep(Duration::from_millis(600)).await;
    c.ok("shock.fire", json!({})).await;
    assert_eq!(
        c.refused("shock.fire", json!({})).await["reason"],
        "not_yet"
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    c.ok("shock.fire", json!({})).await;
    c.ok("shock.stop", json!({})).await;
    let end = wait_end(&mut c, &id).await;
    assert_eq!(end["end"]["outcome"], "operator_stop");
    c.pump(Duration::from_millis(300)).await;
    let rows = rows(&mut c).await;
    assert_eq!(rows.len(), 3);
    let events = c.of_topic("shock.pulse");
    assert_eq!(events.len(), 3);
    for (e, r) in events.iter().zip(&rows) {
        assert_eq!(
            &e.data()["pulse"],
            r,
            "shock.pulse rows are the stored rows"
        );
    }
    let log = c.ok("shock.log", json!({})).await;
    assert!(log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "level"));
}

/// An observer's abort in the wait ends the run at once; nothing after.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_observer_abort_in_the_wait() {
    let addr = serve(Service::with_options(real(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    let mut cfg = config("auto");
    cfg["spacing"]["cooling_s"] = json!(3);
    let id = run_id(&mut c, cfg).await;
    c.ok("shock.start", json!({})).await;
    obs.wait_for("the first pulse", LONG, |e| {
        e.topic() == "shock.state"
            && e.data()["pulses"]["fired"] == 1
            && e.data()["phase"] == "waiting"
    })
    .await;
    let t0 = Instant::now();
    obs.ok("shock.abort", json!({})).await;
    let end = wait_end(&mut obs, &id).await;
    assert!(t0.elapsed() < Duration::from_millis(500));
    assert_eq!(end["end"]["outcome"], "operator_abort");
    obs.pump(Duration::from_secs(1)).await;
    assert_eq!(rows(&mut obs).await.len(), 1);
}

/// A shaker displacement limit under the full-level prediction aborts at the
/// first full-level pulse, predicted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_displacement_limit_under_the_prediction_aborts_predicted() {
    let addr = serve(Service::with_options(real(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let mut cfg = config("auto");
    let dry = c
        .ok("shock.configure", json!({"config": cfg, "dry_run": true}))
        .await;
    let x0 = dry["derived"]["reference"]["displacement_pk_pk_in"]
        .as_f64()
        .unwrap();
    cfg["feasibility"] = json!({
        "shaker": {"name": "Bench shaker", "manufacturer": "", "model": "", "notes": "",
                   "force_random": null, "force_sine": null, "force_shock": {"value": 4000, "basis": "Peak"},
                   "max_velocity_ips": {"value": 70, "basis": "Peak"},
                   "max_displacement_in_pp": {"value": 0.9 * x0, "basis": "Peak"},
                   "max_accel_bare_g": {"value": 100, "basis": "Peak"},
                   "armature_mass_lb": 45, "overturning_moment_limit_lbf_in": null, "custom": []},
        "moving_mass_lb": {"armature": 45, "slip_table": 0, "drive_adapter": 0, "fixturing": 0,
                           "test_article": 5}
    });
    let id = run_id(&mut c, cfg).await;
    assert_eq!(
        c.refused("shock.arm", json!({})).await["reason"],
        "feasibility_exceeded"
    );
    c.ok(
        "shock.arm",
        json!({"override_ack": "displacement checked on the bench"}),
    )
    .await;
    c.ok("shock.start", json!({})).await;
    let end = wait_end(&mut c, &id).await;
    let trip = &end["end"]["trip"];
    assert_eq!(end["end"]["outcome"], "aborted", "{end}");
    assert_eq!(trip["kind"], "kinematic");
    assert_eq!(trip["axis"], "displacement");
    assert_eq!(trip["predicted"], true);
    assert_eq!(trip["at_level_db"], 0);
}

/// The controlling socket closed holds firing and safe-stops at the grace;
/// separately, the service's shutdown safe-stops the run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_controller_lost_and_the_shutdown_safe_stop() {
    let timeout = Duration::from_millis(1500);
    let svc = Service::with_options(real(timeout));
    let addr = serve(svc.clone()).await;
    let mut obs = Client::connect(addr).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let mut cfg = config("auto");
    cfg["spacing"]["cooling_s"] = json!(0.3);
    let id = run_id(&mut c, cfg.clone()).await;
    c.ok("shock.start", json!({})).await;
    obs.wait_for("a pulse", LONG, |e| {
        e.topic() == "shock.state" && e.data()["pulses"]["fired"] == 1
    })
    .await;
    drop(c);
    let end = wait_end(&mut obs, &id).await;
    assert_eq!(end["end"]["outcome"], "safe_stop");
    assert!(end["pulses"]["fired"].as_u64().unwrap() <= 2);

    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let id = run_id(&mut c, cfg).await;
    c.ok("shock.start", json!({})).await;
    c.wait_for("a pulse", LONG, |e| {
        e.topic() == "shock.state" && e.data()["run_id"] == id && e.data()["pulses"]["fired"] == 1
    })
    .await;
    svc.shutdown().await;
    let s = c.ok("shock.state", json!({})).await;
    assert_eq!(s["end"]["outcome"], "safe_stop");
}

/// An imported law whose first correction answers and every later one runs
/// past its budget: three misses in a row end the run (`law_fault`,
/// `over_budget`, 3), and the two misses before the last each refire the drive
/// in force.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_law_ends_the_run_after_three_misses() {
    let addr = serve(Service::with_options(real(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let law = import(&mut c, "law_fixture_shock_misbehave").await;
    let mut cfg = config("auto");
    cfg["control"] = json!({"law": {"id": law, "params": {"mode": "sleep_after"}}});
    let id = run_id(&mut c, cfg).await;
    c.ok("shock.start", json!({})).await;
    let end = wait_end(&mut c, &id).await;
    assert_eq!(end["end"]["outcome"], "law_fault", "{end}");
    assert_eq!(end["end"]["trip"]["reason"], "over_budget");
    assert_eq!(end["end"]["trip"]["consecutive"], 3);
    assert_eq!(end["law"]["misses"], 3, "{end}");
    let rows = rows(&mut c).await;
    assert_eq!(
        rows.len(),
        4,
        "the seed's pulse, the corrected one, then two refires"
    );
    assert_eq!(rows[1]["law"]["used"], true, "{}", rows[1]);
    for r in &rows[2..4] {
        assert_eq!(r["law"]["used"], false, "{r}");
        assert_eq!(r["law"]["miss"], "over_budget");
    }
}

/// The built-in law against the imported `shock-fdc`: the same outcome and
/// pulse count, drives within relative 1e-9.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_built_in_law_and_its_law_file_drive_alike() {
    let addr = serve(Service::with_options(real(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let file = import(&mut c, "hertzforge_law_shock_fdc").await;
    let mut drives: Vec<Vec<Vec<f64>>> = Vec::new();
    let mut ends = Vec::new();
    for law in ["fd_correct".to_owned(), file] {
        let mut cfg = config("auto");
        cfg["control"] = json!({"law": {"id": law}});
        let id = run_id(&mut c, cfg).await;
        c.ok("shock.start", json!({})).await;
        let end = wait_end(&mut c, &id).await;
        ends.push((
            end["end"]["outcome"].clone(),
            end["pulses"]["fired"].clone(),
        ));
        let n = end["pulses"]["fired"].as_u64().unwrap();
        let mut run = Vec::new();
        for i in 0..n {
            let r = c
                .call("shock.pulse", json!({"index": i, "points": 8192}))
                .await;
            let d = c.decoded_response(&r["id"]).unwrap();
            let drive = &r["result"]["views"][1]["series"][0]["y"];
            run.push(d.array(drive).unwrap().to_f64());
        }
        drives.push(run);
    }
    assert_eq!(ends[0], ends[1]);
    for (a, b) in drives[0].iter().zip(&drives[1]) {
        let peak = a.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() <= 1e-9 * peak, "{x} against {y}");
        }
    }
}

/// Exclusion against Random and MIMO both ways on the real services, and the
/// drive verbs refused once shutdown begins.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exclusion_and_shutting_down() {
    let svc = Service::with_options(real(Duration::from_secs(30)));
    let addr = serve(svc.clone()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    run_id(&mut c, config("manual")).await;
    c.ok("shock.arm", json!({})).await;
    let random = sim_config("auto", json!([{"db": 0, "dwell_s": null}]), 2.0);
    assert_eq!(
        c.refused("random.configure", json!({"config": random}))
            .await["reason"],
        "run_active"
    );
    c.ok("shock.stop", json!({})).await;
    c.ok("random.configure", json!({"config": random})).await;
    c.ok("random.arm", json!({})).await;
    assert_eq!(
        c.refused("shock.configure", json!({"config": config("manual")}))
            .await["reason"],
        "run_active"
    );
    c.ok("random.stop", json!({})).await;
    svc.shutdown().await;
    for m in ["shock.arm", "shock.start", "shock.fire"] {
        assert_eq!(c.refused(m, json!({})).await["reason"], "shutting_down");
    }
}

/// The template law with its correction off and its model 6 dB low cannot
/// pass: `not_converged` at pulse 5, -12 dB.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_law_that_does_not_correct_ends_not_converged() {
    let addr = serve(Service::with_options(real(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let law = import(&mut c, "law_fixture_shock_template").await;
    let mut cfg = config("auto");
    cfg["control"] =
        json!({"law": {"id": law, "params": {"correct": false, "model_error_db": -6}}});
    let id = run_id(&mut c, cfg).await;
    c.ok("shock.start", json!({})).await;
    let end = wait_end(&mut c, &id).await;
    assert_eq!(end["end"]["outcome"], "not_converged", "{end}");
    assert_eq!(end["end"]["trip"]["limit"], "per_level");
    assert_eq!(end["end"]["pulse"], 4);
    assert_eq!(end["end"]["level_db"], -12);
}

/// A drive abort under the identified full-level drive ends the run before
/// any pulse, predicted. The identification plays well under the ceiling (the
/// full-level drive of this 1 g pulse is about 2.1 V pk on the Sim station).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_drive_abort_under_the_prediction_ends_the_run_before_a_pulse() {
    let addr = serve(Service::with_options(real(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let mut cfg = config("auto");
    cfg["limits"] = json!({"drive_vpk_ceiling": 0.5, "drive_vpk_abort": 0.6});
    let id = run_id(&mut c, cfg).await;
    c.ok("shock.start", json!({})).await;
    let end = wait_end(&mut c, &id).await;
    assert_eq!(end["end"]["outcome"], "aborted", "{end}");
    assert_eq!(end["end"]["trip"]["kind"], "drive_vpk");
    assert_eq!(end["end"]["trip"]["predicted"], true);
    assert_eq!(end["end"]["trip"]["pulse"], 0);
    assert_eq!(end["end"]["trip"]["abort_vpk"], 0.6);
    assert!(end["end"]["trip"]["vpk"].as_f64().unwrap() > 0.6, "{end}");
    assert!(
        end["end"]["message"]
            .as_str()
            .unwrap()
            .contains("at full level; the drive abort is 0.6 V pk"),
        "{end}"
    );
    assert_eq!(end["pulses"]["fired"], 0);
}
