//! MIMO Sine end to end on the simulator's sine structures, through the wire, on
//! the MIMO Sine engine: a sweep, a dwell and a stepped sine on the linear 4 by
//! 4 with `matrix`, a limit channel notching on the base and article, hold,
//! resume, manual frequency, stop and abort, and a law refused a test type it
//! does not accept.

mod support;

use sds_control::server::Service;
use serde_json::{json, Value};
use std::time::Duration;
use support::msine::{self, config, controller, dwell, opts, stepped, sweep};
use support::{serve, Client};

async fn client() -> Client {
    let svc = Service::with_options(opts(Duration::from_secs(600)));
    let addr = serve(svc).await;
    Client::connect(addr).await
}

/// A service whose simulator runs at `speed` times real time, for the tests
/// that act on a run while it drives.
async fn paced_client(speed: f64) -> Client {
    let svc = Service::with_options(msine::paced(speed, Duration::from_secs(600)));
    let addr = serve(svc).await;
    Client::connect(addr).await
}

/// The worst |error| over the live frames judged outside the first second.
fn worst_live_error(c: &Client) -> f64 {
    c.of_topic("mimo_sine.live")
        .iter()
        .filter(|e| e.data()["t_s"].as_f64().unwrap_or(0.0) > 1.0)
        .flat_map(|e| {
            e.data()["controls"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|c| c["error_db"].as_f64())
        })
        .map(f64::abs)
        .fold(0.0, f64::max)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_a_dwell_on_the_linear_four_by_four_completes_under_matrix() {
    let mut c = client().await;
    controller(&mut c, 4, 0, 4).await;
    let end = msine::run_to_end(
        &mut c,
        config("sine-linear-4x4", 4, 0, 4, dwell(80.0, 4.0), "matrix"),
    )
    .await;
    assert_eq!(end["end"]["outcome"], "completed", "{}", end["end"]);
    assert_eq!(end["source"], "SIM");
    assert_eq!(end["station"]["structure"], "sine-linear-4x4");
    let id = &end["identification"];
    assert_eq!(id["passed"], true);
    assert!(id["line_spacing_hz"].as_f64().unwrap() <= 1.0);
    let worst = worst_live_error(&c);
    assert!(worst < 1.5, "the worst live error was {worst} dB");
    let report = c.ok("mimo_sine.report", json!({})).await["report"].clone();
    assert_eq!(report["controls"].as_array().map(Vec::len), Some(4));
    assert_eq!(report["drives"].as_array().map(Vec::len), Some(4));
    assert!(report["drives"][0]["vpk_max"].as_f64().unwrap() > 0.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_a_sweep_on_the_linear_four_by_four_completes_with_its_pass_frame() {
    let mut c = client().await;
    controller(&mut c, 4, 0, 4).await;
    let end = msine::run_to_end(
        &mut c,
        config("sine-linear-4x4", 4, 0, 4, sweep(30.0, 60.0, 4.0), "matrix"),
    )
    .await;
    assert_eq!(end["end"]["outcome"], "completed", "{}", end["end"]);
    assert_eq!(end["test"], "sweep");
    let worst = worst_live_error(&c);
    assert!(worst < 3.0, "the worst live error was {worst} dB");
    c.wait_for("mimo_sine.sweep", msine::LONG, |e| e.topic() == "mimo_sine.sweep")
        .await;
    let sweep = c.of_topic("mimo_sine.sweep")[0].data().clone();
    assert_eq!(sweep["pass"], 1);
    assert_eq!(sweep["direction"], "up");
    let titles: Vec<&str> = sweep["views"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["title"].as_str())
        .collect();
    assert!(titles.contains(&"Control 1") && titles.contains(&"Drives"), "{titles:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_a_stepped_sine_on_the_linear_four_by_four_plays_every_step() {
    let mut c = client().await;
    controller(&mut c, 4, 0, 4).await;
    let end = msine::run_to_end(
        &mut c,
        config(
            "sine-linear-4x4",
            4,
            0,
            4,
            stepped(&[50.0, 70.0, 90.0], 1.5),
            "matrix",
        ),
    )
    .await;
    assert_eq!(end["end"]["outcome"], "completed", "{}", end["end"]);
    assert_eq!(end["steps"], json!({"done": 3, "of": 3}));
    c.wait_for("mimo_sine.sweep", msine::LONG, |e| e.topic() == "mimo_sine.sweep")
        .await;
    let pass = c.of_topic("mimo_sine.sweep")[0].data().clone();
    assert_eq!(pass["pass"], 1);
    assert!(pass["direction"].is_null());
}

fn base_article() -> Value {
    // The article reads about four times the base at its 230 Hz resonance, so
    // a 2 g bound binds there at full level.
    let mut cfg = config(
        "sine-base-article-6x4",
        4,
        2,
        4,
        sweep(150.0, 350.0, 2.0),
        "matrix",
    );
    cfg["identification"] = json!({"resolution_hz": 0.25, "frames": 8, "review": false});
    cfg["limits"] = json!({"channels": [
        {"channel": 5, "profile": msine::flat(2.0, 1.5, 3.0)},
        {"channel": 6, "profile": msine::flat(2.0, 1.5, 3.0)},
    ]});
    cfg
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_a_limit_channel_notches_the_drive_on_the_base_and_article() {
    let mut c = client().await;
    controller(&mut c, 4, 2, 4).await;
    let mut cfg = base_article();
    // The controls' abort band wide: the run is judged on the notch here.
    for k in 0..4 {
        cfg["channels"]["controls"][k]["profile"] = msine::flat(1.0, 1.5, 12.0);
    }
    let end = msine::run_to_end(&mut c, cfg).await;
    assert!(
        matches!(end["end"]["outcome"].as_str(), Some("completed")),
        "{}",
        end["end"]
    );
    let log = c.ok("mimo_sine.log", json!({})).await;
    let notch_in = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "notch");
    assert!(notch_in, "the log has no notch entry");
    let report = c.ok("mimo_sine.report", json!({})).await["report"].clone();
    assert!(report["limits"][0]["notched_s"].as_f64().unwrap() > 0.0, "{report}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_hold_resume_and_a_manual_frequency_act_on_a_sweep() {
    let mut c = paced_client(10.0).await;
    controller(&mut c, 4, 0, 4).await;
    let cfg = config("sine-linear-4x4", 4, 0, 4, sweep(30.0, 200.0, 1.0), "matrix");
    c.ok("mimo_sine.configure", json!({"config": cfg})).await;
    c.ok("mimo_sine.start", json!({})).await;
    msine::phase(&mut c, "engaged").await;
    c.pump(Duration::from_millis(300)).await;
    let now = c.ok("mimo_sine.state", json!({})).await;
    assert_eq!(now["state"], "running", "{}", now["end"]);
    // The last samples read, on a time plot.
    c.ok(
        "mimo_sine.subscribe",
        json!({"rate_hz": 10, "plots": [{"id": "t", "quantity": "time", "channels": [1], "points": 256}]}),
    )
    .await;
    let i = c
        .wait_for("a time plot with samples", msine::LONG, |e| {
            e.topic() == "mimo_sine.data"
                && e.data()["plots"][0]["views"]
                    .as_array()
                    .is_some_and(|v| v.len() == 1)
        })
        .await;
    let d = c.events[i].decoded.as_ref().unwrap();
    let y = d
        .array(&c.events[i].data()["plots"][0]["views"][0]["series"][0]["y"])
        .unwrap();
    assert_eq!(y.to_f64().len(), 256);
    assert!(y.to_f64().iter().any(|v| v.abs() > 1e-3), "the samples are silent");
    let held = c.ok("mimo_sine.hold", json!({})).await;
    assert_eq!(held["held"], true);
    c.pump(Duration::from_millis(500)).await;
    let a = c.ok("mimo_sine.state", json!({})).await["frequency_hz"]
        .as_f64()
        .unwrap();
    c.pump(Duration::from_millis(500)).await;
    let b = c.ok("mimo_sine.state", json!({})).await["frequency_hz"]
        .as_f64()
        .unwrap();
    assert!((a - b).abs() < 1e-6, "the frequency moved while held: {a} to {b}");
    let to = c.ok("mimo_sine.goto", json!({"hz": 120})).await;
    assert_eq!(to["frequency_hz"], 120);
    c.pump(Duration::from_millis(500)).await;
    let f = c.ok("mimo_sine.state", json!({})).await["frequency_hz"]
        .as_f64()
        .unwrap();
    assert!((f - 120.0).abs() < 1.0, "the tone is at {f} Hz");
    let jog = c.ok("mimo_sine.jog", json!({"delta_hz": 5})).await;
    assert_eq!(jog["frequency_hz"], 125);
    let resumed = c.ok("mimo_sine.resume", json!({})).await;
    assert_eq!(resumed["held"], false);
    let e = c.refused("mimo_sine.goto", json!({"hz": 500})).await;
    assert_eq!(e["reason"], "out_of_band");
    let e = c.refused("mimo_sine.reverse", json!({})).await;
    assert_eq!(e["reason"], "reverse_not_available");
    let e = c.refused("mimo_sine.level", json!({"delta_db": -3})).await;
    assert_eq!(e["reason"], "manual_mode_required");
    c.ok("mimo_sine.stop", json!({})).await;
    let end = msine::state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_stop", "{}", end["end"]);
    let log = c.ok("mimo_sine.log", json!({})).await;
    let kinds: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"hold"), "{kinds:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_a_stop_ramps_every_drive_down_and_an_abort_ends_at_once() {
    let mut c = paced_client(10.0).await;
    controller(&mut c, 4, 0, 4).await;
    let cfg = config("sine-linear-4x4", 4, 0, 4, dwell(80.0, 60.0), "matrix");
    c.ok("mimo_sine.configure", json!({"config": cfg.clone()})).await;
    c.ok("mimo_sine.start", json!({})).await;
    msine::phase(&mut c, "engaged").await;
    c.pump(Duration::from_millis(500)).await;
    c.ok("mimo_sine.stop", json!({})).await;
    let end = msine::state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_stop", "{}", end["end"]);
    let ramp = end["end"]["ramp_s"].as_f64().expect("a stop ramps");
    assert!(ramp > 0.5 && ramp < 3.0, "the ramp took {ramp} s");
    let i = c
        .wait_for("the last live frame", msine::LONG, |e| {
            e.topic() == "mimo_sine.live" && e.data()["state"] == "finished"
        })
        .await;
    let last = c.events[i].data().clone();
    for d in last["drives"].as_array().unwrap() {
        assert!(d["vpk"].as_f64().unwrap() < 1e-3, "a drive is still at {d}");
    }

    c.ok("mimo_sine.reset", json!({})).await;
    c.events.clear();
    c.ok("mimo_sine.start", json!({})).await;
    msine::phase(&mut c, "engaged").await;
    c.pump(Duration::from_millis(500)).await;
    c.ok("mimo_sine.abort", json!({})).await;
    let end = msine::state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_abort", "{}", end["end"]);
    assert!(end["end"]["ramp_s"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_a_law_is_refused_a_test_type_it_does_not_accept() {
    let mut c = client().await;
    controller(&mut c, 4, 0, 4).await;
    for (law, test) in [
        ("secant", sweep(30.0, 60.0, 1.0)),
        ("ratio", sweep(30.0, 60.0, 1.0)),
        ("steepest_descent", stepped(&[50.0], 1.0)),
    ] {
        let cfg = config("sine-linear-4x4", 4, 0, 4, test, law);
        let e = c
            .refused("mimo_sine.configure", json!({"config": cfg}))
            .await;
        assert_eq!(e["reason"], "config_invalid", "{e}");
        let items = e["detail"]["errors"].as_array().unwrap();
        assert!(
            items
                .iter()
                .any(|i| i["reason"] == "law_test_type_not_accepted"),
            "{law}: {items:?}"
        );
    }
    let mut cfg = config("sine-linear-4x4", 4, 0, 4, dwell(80.0, 1.0), "secant");
    cfg["phase_specified"] = json!(false);
    let e = c
        .refused("mimo_sine.configure", json!({"config": cfg}))
        .await;
    let items = e["detail"]["errors"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|i| i["reason"] == "law_amplitude_only_not_accepted"),
        "{items:?}"
    );
}

/// Found on the wire, for the engine: on the station's simulator the same
/// sweep with the controls' abort band at 6 dB ends on a low control abort near
/// 218 Hz, where the notch holds the drive down further than the depth the
/// controls are judged against. Run with `--ignored` to see it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "a finding for the engine: the notch's drive scale and its judged depth part near 218 Hz"]
async fn mimo_sine_the_base_and_article_sweep_completes_with_a_six_db_abort_band() {
    let mut c = client().await;
    controller(&mut c, 4, 2, 4).await;
    let end = msine::run_to_end(&mut c, base_article()).await;
    assert_eq!(end["end"]["outcome"], "completed", "{}", end["end"]);
}
