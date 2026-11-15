//! The MIMO Shock law check through the wire: `laws.check` on a MIMO Shock law
//! runs every case of the check on the simulator's shock plants, reports each
//! case's verdict with the number that decided it, and records a law file's
//! result against its exact file (which is what clears it for hardware). The
//! table each law gets is printed, as it comes out.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use support::{options, serve, Client};

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

/// A call that may take many minutes (the check runs every case).
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

fn print_table(id: &str, report: &Value) {
    println!("law {id}: passed {}", report["passed"]);
    for c in report["cases"].as_array().unwrap() {
        println!(
            "  {:<36} {:<5} {:<20} {:>10}  {}",
            c["name"].as_str().unwrap(),
            if c["passed"] == true { "pass" } else { "fail" },
            c["failed"].as_str().unwrap_or(""),
            c["number"].to_string(),
            c["detail"].as_str().unwrap()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_laws_check_runs_every_case_and_records_a_law_file() {
    let o = options(
        FixedScanner(Err(StationBuildError::NiDriverUnavailable(
            "not installed on this test host".into(),
        ))),
        0.0,
        Duration::from_secs(600),
        Duration::from_secs(5),
    );
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Check"})).await;
    for id in ["inverse", "shape", "paired"] {
        let r = long_call(&mut c, "laws.check", json!({"id": id})).await;
        let report = &r["report"];
        print_table(id, report);
        let kinds = r["law"]["targets"].as_array().unwrap().len();
        assert_eq!(report["cases"].as_array().unwrap().len(), 8 * kinds);
        assert_eq!(
            report["recorded"], false,
            "a built-in's result is not recorded"
        );
        assert_eq!(r["law"]["check"]["status"], "built_in");
    }
    // A law file: its result is recorded against its exact file.
    let file = c
        .ok(
            "laws.import",
            json!({"path": law_file("hertzforge_law_mimo_shock_paired").display().to_string()}),
        )
        .await["law"]["id"]
        .clone();
    assert_eq!(
        c.ok("laws.list", json!({"world": "shock_mimo"})).await["laws"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["id"] == file)
            .unwrap()["check"]["status"],
        "unchecked"
    );
    let r = long_call(&mut c, "laws.check", json!({"id": file})).await;
    print_table("paired (law file)", &r["report"]);
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
    let events = c.of_topic("laws.check");
    assert!(events.iter().any(|e| e.data()["state"] == "done"));
}
