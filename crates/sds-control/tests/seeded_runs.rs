//! Setups seeded from analysis objects, run on the simulator: each
//! `schema/golden/seeds/<module>.json` (a short setup an analysis object filled part
//! of, written and checked by the analysis side's `tests/service/test_seeds.py`) is
//! configured with that module's own configure request, checked as a typed setup is,
//! started, and run to a completed end. The run's folder was accepted with the seeded
//! configuration.
//!
//! With `HERTZFORGE_WRITE_SEEDED_RUNS=1` the run folder is written to
//! `schema/golden/seeded_runs/<module>/` (its streams off, so it is small), where the
//! analysis side converts it and finds the source object's requirement in it.
//!
//! The runs are SIMULATED; nothing here is evidence about hardware.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use support::golden_runs::{files, read_json};
use support::{options, serve, Client};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The request prefix of a module's verbs.
fn prefix(module: &str) -> &str {
    match module {
        "classical_shock" => "shock",
        other => other,
    }
}

/// Configure, start and run `module`'s seeded setup to its end; the run's folder.
async fn seeded_run(module: &str) -> PathBuf {
    let seeded = read_json(&root().join(format!("schema/golden/seeds/{module}.json")));
    let scan = FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )));
    let mut o = options(scan, 0.0, Duration::from_secs(600), Duration::from_secs(5));
    if module == "mimo_random" {
        // the controller's own MIMO driver on the simulator's 4 by 4 plant, as the
        // golden MIMO Random run has it
        o.mimo_driver = None;
    }
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    c.ok(
        "channels.set",
        json!({"revision": 1, "channels": seeded["channels"]}),
    )
    .await;
    let mut config = seeded["config"].clone();
    // a seed's imports are stored first, and the setup takes what they stored
    for import in seeded["imports"].as_array().unwrap() {
        let stored = c
            .ok(import["method"].as_str().unwrap(), import["params"].clone())
            .await;
        let at = import["set"].as_str().unwrap();
        config[at] = json!({"sha256": stored["reference"]["sha256"]});
    }
    let p = prefix(module);
    let (verb, member) = if module == "mimo_random" {
        (format!("{p}.setup"), "setup")
    } else {
        (format!("{p}.configure"), "config")
    };
    let dry = c
        .ok(&verb, json!({member: config.clone(), "dry_run": true}))
        .await;
    assert!(
        dry["errors"].as_array().is_none_or(|e| e.is_empty()),
        "{module}: the dry run refused the seeded setup: {dry}"
    );
    c.ok(&verb, json!({member: config.clone()})).await;
    c.ok(&format!("{p}.start"), json!({})).await;
    let topic = if module == "mimo_random" {
        format!("{p}.status")
    } else {
        format!("{p}.state")
    };
    let i = c
        .wait_for("the run's end", Duration::from_secs(600), |e| {
            e.topic() == topic && (e.data()["state"] == "finished" || e.data()["state"] == "review")
        })
        .await;
    let end = c.events[i].data().clone();
    assert_eq!(end["end"]["outcome"], "completed", "{module}: {end}");
    let folder = PathBuf::from(end["record"]["folder"].as_str().unwrap());
    let t0 = Instant::now();
    while !(folder.join("end.json").is_file()
        && read_json(&folder.join("run.json"))["complete"] == true)
    {
        assert!(
            t0.elapsed() < Duration::from_secs(60),
            "the record did not complete"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let accepted = read_json(&folder.join("setup.json"));
    assert_eq!(
        accepted["config"]["title"], config["title"],
        "{module}: the run was not accepted with the seeded setup"
    );
    folder
}

/// The run folder copied to `schema/golden/seeded_runs/<module>/` when asked.
fn keep(module: &str, folder: &Path) {
    if std::env::var("HERTZFORGE_WRITE_SEEDED_RUNS").as_deref() != Ok("1") {
        return;
    }
    let to = root().join(format!("schema/golden/seeded_runs/{module}"));
    let _ = std::fs::remove_dir_all(&to);
    for rel in files(folder) {
        let path = to.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy(folder.join(&rel), path).unwrap();
    }
}

async fn check(module: &str) {
    let folder = seeded_run(module).await;
    keep(module, &folder);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seeded_random_setup_runs_on_the_simulator() {
    check("random").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seeded_sine_setup_runs_on_the_simulator() {
    check("sine").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seeded_srs_setup_runs_on_the_simulator() {
    check("srs").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seeded_classical_shock_setup_runs_on_the_simulator() {
    check("classical_shock").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seeded_twr_setup_runs_on_the_simulator() {
    check("twr").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seeded_mixed_setup_runs_on_the_simulator() {
    check("mixed").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seeded_mimo_random_setup_runs_on_the_simulator() {
    check("mimo_random").await;
}
