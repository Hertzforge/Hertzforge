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
