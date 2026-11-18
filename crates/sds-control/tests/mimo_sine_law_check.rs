//! MIMO Sine laws through `laws.check` and through a run: a built-in law's
//! check runs every case its manifest accepts and records nothing; a law
//! file's is recorded against its exact file, and the hardware arming gate
//! reads that record; a law file arms and runs on the simulator.

mod support;

use hertzforge_random_laws::LawCatalog;
use sds_control::mimo_sine::laws as mlaws;
use sds_control::server::Service;
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::time::Duration;
use support::msine::{self, config, controller, dwell};
use support::{serve, Client};

fn law_file(name: &str) -> PathBuf {
    let file = format!("{name}.wasm");
    let exe = std::env::current_exe().unwrap();
    exe.ancestors()
        .map(|d| d.join("wasm32-wasip2").join("release").join(&file))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{file} is not built; run scripts/build_laws.sh"))
}

/// The service runs one law check at a time in a process (a second is refused
/// while one runs), so this binary's checks take turns.
static ONE_CHECK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A call that may take minutes (the check runs every case).
async fn long_call(c: &mut Client, method: &str, params: Value) -> Value {
    use futures_util::SinkExt;
    let id = 9000 + c.events.len() as i64;
    let req = json!({"type": "request", "id": id, "method": method, "params": params});
    c.ws.send(tokio_tungstenite::tungstenite::Message::Text(
        req.to_string(),
    ))
    .await
    .unwrap();
    loop {
        let m = c
            .read(Duration::from_secs(1800))
            .await
            .expect("an answer within 30 minutes");
        if m["type"] == "response" && m["id"] == id {
            assert!(m.get("error").is_none(), "{method} failed: {}", m["error"]);
            return m["result"].clone();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_sine_laws_check_runs_every_case_and_records_a_law_file() {
    let _turn = ONE_CHECK.lock().await;
    let addr = serve(Service::with_options(msine::opts(Duration::from_secs(600)))).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Check"})).await;
    // A built-in, named in its world: every case it accepts, nothing recorded.
    let r = long_call(
        &mut c,
        "laws.check",
        json!({"id": "ratio", "world": "sine_mimo"}),
    )
    .await;
    let report = &r["report"];
    let cases = report["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    assert!(cases
        .iter()
        .all(|k| k["scored"].is_boolean() && k["case"].is_string()));
    assert_eq!(report["recorded"], false);
    assert_eq!(r["law"]["check"]["status"], "built_in");
    assert_eq!(r["law"]["world"], "sine_mimo");
    let events = c.of_topic("laws.check");
    assert_eq!(
        events
            .iter()
            .filter(|e| e.data()["state"] == "done")
            .count(),
        cases.len()
    );
    // A law file: imported unchecked, its check recorded against its file.
    let file = c
        .ok(
            "laws.import",
            json!({"path": law_file("hertzforge_law_mimo_sine_ratio").display().to_string()}),
        )
        .await["law"]["id"]
        .clone();
    let listed = c.ok("laws.list", json!({"world": "sine_mimo"})).await;
    let entry = listed["laws"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == file)
        .unwrap()
        .clone();
    assert_eq!(entry["check"]["status"], "unchecked");
    assert_eq!(entry["test_types"], json!(["dwell", "stepped"]));
    assert_eq!(entry["default"], false);
    let r = long_call(&mut c, "laws.check", json!({"id": file})).await;
    assert_eq!(r["report"]["recorded"], true);
    let status = r["law"]["check"]["status"].as_str().unwrap();
    assert_eq!(
        status,
        if r["report"]["passed"] == true {
            "passed"
        } else {
            "failed"
        }
    );
}

/// The arming gate reads the catalog's record: an unchecked law file is
/// refused on a hardware station, taken on the simulator.
#[test]
fn mimo_sine_an_unchecked_law_file_is_refused_on_hardware_and_taken_on_the_simulator() {
    let dir = std::env::temp_dir().join(format!("msine-gate-{}", std::process::id()));
    let mut cat = LawCatalog::open(&dir).unwrap();
    let e = cat
        .import(&law_file("hertzforge_law_mimo_sine_ratio"))
        .expect("imports");
    let given = Map::new();
    let hw = mlaws::resolve(
        &cat,
        &e.id,
        &given,
        Duration::from_millis(5),
        Duration::from_secs(120),
        true,
    );
    assert_eq!(
        hw.err().and_then(|e| e.reason),
        Some("law_not_checked"),
        "an unchecked file arms on hardware"
    );
    assert!(mlaws::resolve(
        &cat,
        &e.id,
        &given,
        Duration::from_millis(5),
        Duration::from_secs(120),
        false
    )
    .is_ok());
    // A passed record clears it.
    let rec = hertzforge_random_laws::CheckRecord::new(
        &e.id,
        hertzforge_mimo_sine_laws::check::MIMO_SINE_CHECK_VERSION,
        "2026-11-18T00:00:00Z",
        vec![hertzforge_random_laws::CheckCase {
            name: "slow_sweep".into(),
            passed: true,
            detail: String::new(),
        }],
    );
    cat.record_check(&rec).unwrap();
    assert!(mlaws::resolve(
        &cat,
        &e.id,
        &given,
        Duration::from_millis(5),
        Duration::from_secs(120),
        true
    )
    .is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_a_law_file_arms_and_runs_a_dwell() {
    let addr = serve(Service::with_options(msine::opts(Duration::from_secs(600)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, 4, 0, 4).await;
    let id = c
        .ok(
            "laws.import",
            json!({"path": law_file("hertzforge_law_mimo_sine_ratio").display().to_string()}),
        )
        .await["law"]["id"]
        .clone();
    let mut cfg = config(
        "sine-linear-4x4",
        4,
        0,
        4,
        dwell(80.0, 3.0),
        id.as_str().unwrap(),
    );
    cfg["acquisition"] = json!({"tick_ms": 10});
    let end = msine::run_to_end(&mut c, cfg).await;
    assert_eq!(end["end"]["outcome"], "completed", "{}", end["end"]);
    assert_eq!(end["law"]["id"], id);
    let folder = msine::folder(&end);
    msine::complete(&folder).await;
    let law: Value =
        serde_json::from_slice(&std::fs::read(folder.join("law/law.json")).unwrap()).unwrap();
    assert_eq!(law["builtin"], false);
    assert_eq!(law["sha256"], id);
}

/// The five built-in laws, each with the cases the laws crate plans for it.
const BUILTINS: [&str; 5] = ["matrix", "paired", "steepest_descent", "secant", "ratio"];

/// One law's check report, one line: its verdict and every case's number.
fn summary(id: &str, report: &Value) -> String {
    let cases = report["cases"].as_array().unwrap();
    let failed: Vec<String> = cases
        .iter()
        .filter(|k| k["scored"] == true && k["passed"] != true)
        .map(|k| format!("{} ({}: {})", k["case"], k["failed"], k["detail"]))
        .collect();
    format!(
        "{id}: passed {} over {} cases, failed [{}]",
        report["passed"],
        cases.len(),
        failed.join("; ")
    )
}

/// Every built-in law passes its check through the service's own method, and
/// `paired` runs its pair cases (dwells, noise, stepped and the ladder) on the
/// weakly coupled structure, as the laws crate plans them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_sine_check_every_builtin_passes_through_the_service() {
    let _turn = ONE_CHECK.lock().await;
    let addr = serve(Service::with_options(msine::opts(Duration::from_secs(
        1800,
    ))))
    .await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Check"})).await;
    let mut lines = Vec::new();
    let mut all = true;
    for id in BUILTINS {
        let r = long_call(
            &mut c,
            "laws.check",
            json!({"id": id, "world": "sine_mimo"}),
        )
        .await;
        let report = &r["report"];
        lines.push(summary(id, report));
        all &= report["passed"] == true;
        if id == "paired" {
            let cases = report["cases"].as_array().unwrap();
            for name in ["measurement_noise", "stepped", "level_ladder"] {
                let k = cases.iter().find(|k| k["case"] == name).unwrap();
                assert!(
                    k["structure"].as_str().unwrap().contains("sine-weak-4x4"),
                    "paired's {name} did not run on the weakly coupled structure: {k}"
                );
            }
        }
    }
    eprintln!("{}", lines.join("\n"));
    assert!(all, "{}", lines.join("\n"));
}

/// Every built-in law as a law file passes its check through the service, the
/// check is recorded against its file, and the hardware arming gate reads the
/// record and lets each file arm.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_sine_check_every_law_file_passes_through_the_service_and_may_arm() {
    let _turn = ONE_CHECK.lock().await;
    let opts = msine::opts(Duration::from_secs(1800));
    let root = opts.data_root.clone();
    let addr = serve(Service::with_options(opts)).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Check"})).await;
    let mut lines = Vec::new();
    let mut ids = Vec::new();
    let mut all = true;
    for name in BUILTINS {
        let path = law_file(&format!("hertzforge_law_mimo_sine_{name}"));
        let id = c
            .ok("laws.import", json!({"path": path.display().to_string()}))
            .await["law"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let r = long_call(&mut c, "laws.check", json!({"id": id})).await;
        lines.push(summary(name, &r["report"]));
        all &= r["report"]["passed"] == true;
        assert_eq!(r["report"]["recorded"], true);
        assert_eq!(
            r["law"]["check"]["status"],
            if r["report"]["passed"] == true {
                "passed"
            } else {
                "failed"
            }
        );
        ids.push(id);
    }
    eprintln!("{}", lines.join("\n"));
    assert!(all, "{}", lines.join("\n"));
    let cat = LawCatalog::open(&root).unwrap();
    for id in &ids {
        let hw = mlaws::resolve(
            &cat,
            id,
            &Map::new(),
            Duration::from_millis(5),
            Duration::from_secs(120),
            true,
        );
        assert!(
            hw.is_ok(),
            "{id} with a passed record was refused on hardware"
        );
    }
}

/// The hardware arming gate refuses a law file whose record is missing or
/// failing, and takes it once a passed record is written for its file.
#[test]
fn mimo_sine_the_arming_gate_refuses_a_missing_or_failing_record() {
    let dir = std::env::temp_dir().join(format!("msine-gate2-{}", std::process::id()));
    let mut cat = LawCatalog::open(&dir).unwrap();
    let e = cat
        .import(&law_file("hertzforge_law_mimo_sine_paired"))
        .expect("imports");
    let arm = |cat: &LawCatalog| {
        mlaws::resolve(
            cat,
            &e.id,
            &Map::new(),
            Duration::from_millis(5),
            Duration::from_secs(120),
            true,
        )
        .err()
        .and_then(|e| e.reason)
    };
    assert_eq!(arm(&cat), Some("law_not_checked"), "a missing record armed");
    let record = |passed: bool| {
        hertzforge_random_laws::CheckRecord::new(
            &e.id,
            hertzforge_mimo_sine_laws::check::MIMO_SINE_CHECK_VERSION,
            "2026-11-18T00:00:00Z",
            vec![
                hertzforge_random_laws::CheckCase {
                    name: "stepped".into(),
                    passed: true,
                    detail: String::new(),
                },
                hertzforge_random_laws::CheckCase {
                    name: "measurement_noise".into(),
                    passed,
                    detail: String::new(),
                },
            ],
        )
    };
    cat.record_check(&record(false)).unwrap();
    assert_eq!(arm(&cat), Some("law_not_checked"), "a failing record armed");
    cat.record_check(&record(true)).unwrap();
    assert_eq!(arm(&cat), None, "a passed record was refused");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Each case's line in the report carries the law's call times against the
/// setup's budget, which the service's check keeps: the refused answers and
/// the late ones among them, the calls, the slowest and the median call, and
/// how many were over the budget. A case that could not run has them null.
#[test]
fn mimo_sine_a_case_line_carries_the_law_call_times() {
    use hertzforge_mimo_sine_laws::check::{self as mcheck, MimoSineCheckCase};
    use sds_control::mimo_sine::law_check::{case_json, station};
    let law = hertzforge_mimo_sine::laws::builtin("ratio").unwrap();
    let case = MimoSineCheckCase::DwellAwayFromResonance;
    let r = mcheck::check_case(case, law, &mut station);
    let m = r.measure.as_ref().expect("the case ran");
    let line = case_json(&r.verdict, Some(m), "ratio");
    assert_eq!(line["case"], case.name());
    let calls = line["calls"].as_u64().unwrap();
    assert!(calls > 500, "{line}");
    assert!(line["late"].as_u64().unwrap() <= line["misses"].as_u64().unwrap());
    assert!(line["over_budget"].as_u64().unwrap() <= calls);
    // The setup's budget: a quarter of the 10 ms update (164 frames at 16384 Hz).
    assert_eq!(line["budget_ms"], json!(2.502));
    assert!(line["worst_call_ms"].as_f64().unwrap() >= line["median_call_ms"].as_f64().unwrap());
    let none = case_json(&r.verdict, None, "ratio");
    for k in [
        "misses",
        "late",
        "calls",
        "worst_call_ms",
        "median_call_ms",
        "over_budget",
        "budget_ms",
    ] {
        assert!(none[k].is_null(), "{k}: {none}");
    }
}
