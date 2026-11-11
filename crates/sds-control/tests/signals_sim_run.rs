//! Signals through the protocol on the simulator, over a real loopback WebSocket: a
//! window subscribes before anything runs, a Random run on the simulator streams its
//! control input and its drive as time views on the station's clock, the run's end sends
//! one last frame, and a window that subscribes after the end is sent nothing
//! (schema/PROTOCOL.md, "Signals").
//!
//! The tap is one per process, so this file holds one test: a second test running
//! beside it in this binary would replace the tap under it.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::time::Duration;
use support::schema::Schema;
use support::{options, serve, sim_config, Client};

const LONG: Duration = Duration::from_secs(90);

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

fn signals(e: &support::Event) -> bool {
    e.topic() == "signals.data"
}

/// The f64 values of the array a reference names, in `e`'s decoded frame.
fn values(e: &support::Event, reference: &Value) -> Vec<f64> {
    let id = reference["$array"].as_str().expect("an array reference");
    let d = e.decoded.as_ref().expect("a binary frame");
    d.arrays.get(id).expect("the named array").to_f64()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signals_of_a_random_run_on_the_simulator_reach_a_subscribed_window() {
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

    // Nothing acquires yet: the subscription is taken, nothing is offered, nothing sent.
    let params = json!({"rate_hz": 10, "plots": [
        {"id": "raw", "channels": [1, 2, 99], "window_s": 0.05, "points": 1024,
         "arrange": "split"},
        {"id": "env", "channels": "all", "window_s": 2, "points": 256},
    ]});
    schema.assert_valid(&params, Some("signalsSubscribeParams"));
    let sub = o.ok("signals.subscribe", params).await;
    schema.assert_valid(&sub, Some("signalsSubscribeResult"));
    assert_eq!(sub["available"], false);
    assert_eq!(sub["channels"], json!([]));
    assert_eq!(sub["plots"][1]["arrange"], "overlay");
    let e = o
        .refused(
            "signals.subscribe",
            json!({"plots": [{"id": "x", "points": 9}]}),
        )
        .await;
    assert_eq!(e["detail"]["param"], "plots[0].points");
    o.pump(Duration::from_millis(300)).await;
    assert!(!o.events.iter().any(signals), "no frames before a run");

    // A Random run on the simulator: the default table's control row 1 and drive row 2.
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let config = sim_config("auto", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 2.0);
    c.ok("random.configure", json!({ "config": config })).await;
    c.ok("random.start", json!({})).await;

    // Frames while the run acquires, with the drive moving.
    let i = o
        .wait_for("signals with the drive moving", LONG, |e| {
            signals(e)
                && e.data()["available"] == true
                && e.data()["plots"][0]["views"][1]["series"][0]["y"].is_object()
                && values(e, &e.data()["plots"][0]["views"][1]["series"][0]["y"])
                    .iter()
                    .any(|v| v.abs() > 1e-6)
        })
        .await;
    let ev = &o.events[i];
    let d = ev.data();
    schema.assert_valid(d, Some("signalsData"));
    assert_eq!(d["fs_hz"], 8192);
    assert_eq!(
        d["channels"],
        json!([
            {"index": 1, "name": "Control", "unit": "g", "kind": "input"},
            {"index": 2, "name": "Drive", "unit": "V", "kind": "drive"}
        ])
    );
    let raw = &d["plots"][0];
    assert_eq!(raw["channels"], json!([1, 2]), "row 99 is not in the run");
    let views = raw["views"].as_array().unwrap();
    assert_eq!(views.len(), 2);
    assert_eq!(views[0]["title"], "Channel 1: Control");
    assert_eq!(views[0]["y"]["unit"], "g");
    assert_eq!(views[0]["series"][0]["role"], "measured");
    assert_eq!(views[1]["y"]["unit"], "V");
    assert_eq!(views[1]["series"][0]["role"], "drive");
    // Every sample of the last 0.05 s, one station frame apart, ending at the newest.
    let frames = d["frames"].as_u64().unwrap();
    let x = values(ev, &views[0]["series"][0]["x"]);
    assert_eq!(x.len(), 410, "0.05 s at 8192 Hz");
    for w in x.windows(2) {
        assert!(((w[1] - w[0]) * 8192.0 - 1.0).abs() < 1e-9, "{w:?}");
    }
    assert!((x[409] * 8192.0 - (frames - 1) as f64).abs() < 1e-6);
    assert_eq!(raw["first_frame"].as_u64().unwrap(), frames - 410);
    let y = values(ev, &views[0]["series"][0]["y"]);
    assert!(views[0]["series"][0].get("y_min").is_none());
    assert!(y.iter().any(|v| v.abs() > 1e-6), "the control input moves");
    // The long window: the envelope at 256 points, one view for each unit.
    let env = &d["plots"][1];
    assert_eq!(env["channels"], json!([1, 2]));
    assert_eq!(env["views"].as_array().unwrap().len(), 2);
    let s = &env["views"][0]["series"][0];
    assert!(values(ev, &s["x"]).len() <= 256);
    assert!(frames > 256 && s["y_min"].is_object(), "an envelope");

    // Frames come at the rate asked at most, each with more of the run.
    let count = o.events.iter().filter(|e| signals(e)).count();
    o.pump(Duration::from_secs(2)).await;
    let live: Vec<&support::Event> = o
        .events
        .iter()
        .filter(|e| signals(e) && e.data()["available"] == true)
        .collect();
    assert!(live.len() > count, "frames go on while the run acquires");
    for w in live.windows(2) {
        assert!(w[1].data()["frames"].as_u64() > w[0].data()["frames"].as_u64());
    }
    let span = live.last().unwrap().at - live[0].at;
    let rate = (live.len() - 1) as f64 / span.as_secs_f64();
    assert!(rate <= 12.0, "{rate:.1} frames a second against 10 asked");

    // The run ends: one last frame, then nothing.
    c.wait_state("finished", LONG).await;
    let last = o
        .wait_for("the last signals frame", LONG, |e| {
            signals(e) && e.data()["available"] == false
        })
        .await;
    schema.assert_valid(o.events[last].data(), Some("signalsData"));
    o.pump(Duration::from_millis(500)).await;
    assert_eq!(
        o.events
            .iter()
            .skip(last + 1)
            .filter(|e| signals(e))
            .count(),
        0,
        "no frames once the run has ended"
    );

    // A window that subscribes now is offered nothing and sent nothing.
    let mut late = Client::connect(addr).await;
    let r = late
        .ok("signals.subscribe", json!({"plots": [{"id": "a"}]}))
        .await;
    assert_eq!(r["available"], false);
    late.pump(Duration::from_millis(300)).await;
    assert!(!late.events.iter().any(signals));
    assert_eq!(o.ok("signals.unsubscribe", json!({})).await, json!({}));
}
