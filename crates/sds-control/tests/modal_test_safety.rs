//! The Modal Test's gates through the control service: a table taken from a file is
//! refused at the start until the operator applies it, and the station is not opened
//! before.

mod support;

use serde_json::json;
use support::modal::{base_options, impact_config, rows, state};
use support::{serve, Client};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_start_refuses_a_restored_table_until_it_is_applied() {
    let svc = sds_control::server::Service::with_options(base_options(8.0));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    let r = c
        .ok(
            "channels.set",
            json!({"revision": rev, "channels": rows(false),
                   "source": {"kind": "file", "name": "bench.csv"}}),
        )
        .await;
    let rev = r["revision"].as_u64().unwrap();
    c.ok("modal_test.configure", json!({"config": impact_config()}))
        .await;
    let e = c.refused("modal_test.start", json!({})).await;
    assert_eq!(e["reason"], "table_not_applied", "{e}");
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("the file \"bench.csv\""),
        "{e}"
    );
    let s = state(&mut c).await;
    assert_eq!(s["state"], "configured", "nothing started: {s}");
    assert_eq!(
        s["record"],
        serde_json::Value::Null,
        "no record opened: {s}"
    );
    c.ok("channels.apply", json!({"revision": rev})).await;
    c.ok("modal_test.start", json!({})).await;
    c.ok("modal_test.abort", json!({})).await;
}
