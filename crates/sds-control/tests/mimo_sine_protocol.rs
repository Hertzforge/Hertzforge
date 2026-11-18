//! MIMO Sine through the protocol: every refusal of a configuration by name,
//! the simulator's structure by name and size (a run never arms on the default
//! one-drive simulator), control and run exclusivity, the controlled stop when
//! nobody holds control and at shutdown, and the drive verbs refused while the
//! service shuts down.

mod support;

use sds_control::server::Service;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use support::msine::{self, config, controller, dwell, stepped, sweep};
use support::{serve, Client};

fn base() -> Value {
    config("sine-linear-4x4", 4, 0, 4, dwell(80.0, 2.0), "matrix")
}

/// The item of a refused configuration with reason `want` (else the first),
/// and its reason.
async fn refused_item(c: &mut Client, cfg: Value, want: &str) -> (String, Value) {
    let e = c
        .refused("mimo_sine.configure", json!({"config": cfg}))
        .await;
    if e["reason"] == "config_invalid" {
        let items = e["detail"]["errors"].as_array().unwrap();
        let item = items
            .iter()
            .find(|i| i["reason"] == want)
            .unwrap_or(&items[0])
            .clone();
        (item["reason"].as_str().unwrap().to_owned(), item)
    } else {
        (e["reason"].as_str().unwrap().to_owned(), e)
    }
}

fn with(mut cfg: Value, path: &[&str], v: Value) -> Value {
    let mut at = &mut cfg;
    for k in &path[..path.len() - 1] {
        at = if let Ok(i) = k.parse::<usize>() {
            &mut at[i]
        } else {
            &mut at[*k]
        };
    }
    let last = path[path.len() - 1];
    if let Ok(i) = last.parse::<usize>() {
        at[i] = v;
    } else {
        at[last] = v;
    }
    cfg
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_every_refusal_of_a_configuration_is_named() {
    let addr = serve(Service::with_options(msine::opts(Duration::from_secs(600)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, 4, 0, 4).await;
    let b = base();
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "controls_out_of_range",
            with(b.clone(), &["channels", "controls"], json!([])),
            Some("channels.controls"),
        ),
        (
            "drive_unknown_channel",
            with(
                b.clone(),
                &["channels", "drives", "1", "channel"],
                json!(99),
            ),
            None,
        ),
        (
            "control_unknown_channel",
            with(
                b.clone(),
                &["channels", "controls", "0", "channel"],
                json!(9),
            ),
            Some("channels.controls[0].channel"),
        ),
        (
            "abort_below_ceiling",
            with(
                b.clone(),
                &["channels", "drives", "2", "abort_vpk"],
                json!(1),
            ),
            Some("channels.drives[2].abort_vpk"),
        ),
        (
            "phase_missing",
            with(
                b.clone(),
                &["channels", "controls", "1", "phase"],
                json!([]),
            ),
            Some("channels.controls[1].phase"),
        ),
        (
            "phase_invalid",
            with(
                b.clone(),
                &["channels", "controls", "1", "phase"],
                json!([{"freq_hz": 10, "phase_deg": 270}]),
            ),
            Some("channels.controls[1].phase[0]"),
        ),
        (
            "profile_not_covering",
            with(
                b.clone(),
                &["channels", "controls", "0", "profile"],
                json!({"rows": [{"freq_hz": 100, "quantity": "accel_g", "value": 1},
                                {"freq_hz": 200, "quantity": "accel_g", "value": 1}],
                       "alarm_db": [1.5, 1.5], "abort_db": [6, 6]}),
            ),
            Some("channels.controls[0].profile"),
        ),
        (
            "band_above_grid",
            with(b.clone(), &["test"], dwell(7500.0, 1.0)),
            Some("test.freq_hz"),
        ),
        (
            "steps_empty",
            with(b.clone(), &["test"], stepped(&[], 1.0)),
            Some("test.frequencies_hz"),
        ),
        (
            "levels_not_ascending",
            with(
                b.clone(),
                &["schedule"],
                json!({"levels_db": [0, -6], "start_db": 0}),
            ),
            Some("schedule.levels_db"),
        ),
        (
            "start_not_a_level",
            with(
                b.clone(),
                &["schedule"],
                json!({"levels_db": [-6, 0], "start_db": -3}),
            ),
            Some("schedule.start_db"),
        ),
        (
            "tick_out_of_range",
            with(b.clone(), &["acquisition"], json!({"tick_ms": 1})),
            Some("acquisition.tick_ms"),
        ),
        (
            "law_unknown",
            with(b.clone(), &["control", "law", "id"], json!("nope")),
            Some("control.law.id"),
        ),
        (
            "law_param_unknown",
            with(
                b.clone(),
                &["control", "law", "params"],
                json!({"speed": 2}),
            ),
            Some("control.law.params.speed"),
        ),
        (
            "law_param_invalid",
            with(
                b.clone(),
                &["control", "law", "params"],
                json!({"carry": 3}),
            ),
            Some("control.law.params.carry"),
        ),
        (
            "law_test_type_not_accepted",
            with(b.clone(), &["control", "law", "id"], json!("paired"))
                .pipe(|c| with(c, &["test"], sweep(30.0, 60.0, 1.0))),
            Some("control.law.id"),
        ),
        (
            "structure_unknown",
            with(
                b.clone(),
                &["simulator", "structure"],
                json!("sine-flat-9x9"),
            ),
            Some("simulator.structure"),
        ),
        (
            "structure_wrong_size",
            with(
                b.clone(),
                &["simulator", "structure"],
                json!("sine-tall-6x3"),
            ),
            Some("simulator.structure"),
        ),
        (
            "out_of_range",
            with(b.clone(), &["identification"], json!({"resolution_hz": 10})),
            Some("identification.resolution_hz"),
        ),
        (
            "out_of_range",
            with(b.clone(), &["requests"], json!({"max_hold_s": 5})),
            Some("requests.max_hold_s"),
        ),
        (
            "out_of_range",
            with(b.clone(), &["control", "max_step_db"], json!(9)),
            Some("control.max_step_db"),
        ),
        (
            "bad_params",
            with(b.clone(), &["cola"], json!({"enabled": true})),
            None,
        ),
        (
            "bad_params",
            with(b.clone(), &["capture"], json!({"enabled": true})),
            None,
        ),
    ];
    for (reason, cfg, field) in cases {
        let (got, item) = refused_item(&mut c, cfg, reason).await;
        assert_eq!(got, reason, "{item}");
        if let Some(f) = field {
            assert_eq!(item["field"], f, "{reason}: {item}");
        }
    }
    // A profile row's item is located by its field: the control, its profile, the row.
    let bad = with(
        b.clone(),
        &["channels", "controls", "2", "profile", "rows", "1", "value"],
        json!(-1),
    );
    let (got, item) = refused_item(&mut c, bad, "value_not_positive").await;
    assert_eq!(got, "value_not_positive");
    assert_eq!(item["field"], "channels.controls[2].profile.rows[1]");
    // Nothing was replaced: no run is configured.
    let s = c.ok("mimo_sine.state", json!({})).await;
    assert_eq!(s["state"], "unconfigured");
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl Pipe for Value {}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_a_run_never_arms_on_the_default_one_drive_simulator() {
    let addr = serve(Service::with_options(msine::opts(Duration::from_secs(600)))).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    // The service's own starting table: one control, one drive. No sine
    // structure has that size, so the configuration is refused by name.
    let mut cfg = msine::config("sine-linear-4x4", 1, 0, 1, dwell(80.0, 1.0), "matrix");
    cfg["simulator"] = json!({"structure": null});
    cfg["channels"]["drives"][0]["channel"] = json!(2);
    let table = c.ok("channels.get", json!({})).await;
    let rows: Vec<Value> = table["channels"].as_array().unwrap().to_vec();
    let enabled: Vec<&Value> = rows.iter().filter(|r| r["enabled"] == true).collect();
    assert!(enabled.len() <= 3, "{enabled:?}");
    let e = c
        .refused("mimo_sine.configure", json!({"config": cfg}))
        .await;
    let reasons: Vec<&str> = e["detail"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["reason"].as_str())
        .collect();
    assert!(reasons.contains(&"structure_wrong_size"), "{reasons:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_control_is_needed_and_one_run_at_a_time() {
    let addr = serve(Service::with_options(msine::paced(
        5.0,
        Duration::from_secs(600),
    )))
    .await;
    let mut c = Client::connect(addr).await;
    let mut other = Client::connect(addr).await;
    controller(&mut c, 4, 0, 4).await;
    let e = other
        .refused("mimo_sine.configure", json!({"config": base()}))
        .await;
    assert_eq!(e["reason"], "not_controller", "{e}");
    // A dry run needs no control.
    let dry = other
        .ok(
            "mimo_sine.configure",
            json!({"config": base(), "dry_run": true}),
        )
        .await;
    assert_eq!(dry["errors"], json!([]));
    assert!(other.decoded_response(&json!(other.last_id)).is_some());
    c.ok(
        "mimo_sine.configure",
        json!({"config": with(base(), &["test"], dwell(80.0, 60.0))}),
    )
    .await;
    c.ok("mimo_sine.start", json!({})).await;
    // Another module, or MIMO Sine again, is refused while it is live.
    let e = c
        .refused("mimo_sine.configure", json!({"config": base()}))
        .await;
    assert_eq!(e["reason"], "run_active");
    let e = c.refused("mimo_sine.clear", json!({})).await;
    assert_eq!(e["reason"], "run_active");
    // Any connection may stop it.
    other.ok("mimo_sine.abort", json!({})).await;
    let end = msine::state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_abort");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_a_run_nobody_holds_control_of_ends_in_the_safe_stop() {
    let addr = serve(Service::with_options(msine::paced(
        5.0,
        Duration::from_secs(2),
    )))
    .await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, 4, 0, 4).await;
    c.ok(
        "mimo_sine.configure",
        json!({"config": with(base(), &["test"], dwell(80.0, 600.0))}),
    )
    .await;
    c.ok("mimo_sine.start", json!({})).await;
    msine::phase(&mut c, "engaged").await;
    c.ok("session.release", json!({})).await;
    let end = msine::state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "safe_stop", "{}", end["end"]);
    assert_eq!(end["end"]["ramp_s"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_the_shutdown_ends_a_driving_run_with_its_ramp() {
    let svc: Arc<Service> = Service::with_options(msine::paced(5.0, Duration::from_secs(600)));
    let addr = serve(svc.clone()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, 4, 0, 4).await;
    c.ok(
        "mimo_sine.configure",
        json!({"config": with(base(), &["test"], dwell(80.0, 600.0))}),
    )
    .await;
    c.ok("mimo_sine.start", json!({})).await;
    msine::phase(&mut c, "engaged").await;
    let said = svc.shutdown().await.expect("a run was live");
    assert!(said.contains("safe_stop"), "{said}");
    let s = c.ok("mimo_sine.state", json!({})).await;
    assert_eq!(s["end"]["outcome"], "safe_stop");
    for (m, p) in [
        ("mimo_sine.arm", json!({})),
        ("mimo_sine.start", json!({})),
        ("mimo_sine.proceed", json!({})),
        ("mimo_sine.level", json!({"delta_db": 1})),
        ("mimo_sine.mode", json!({"mode": "manual"})),
        ("mimo_sine.resume", json!({})),
        ("mimo_sine.jog", json!({"delta_hz": 1})),
        ("mimo_sine.goto", json!({"hz": 200})),
    ] {
        assert_eq!(c.refused(m, p).await["reason"], "shutting_down", "{m}");
    }
    c.ok("mimo_sine.stop", json!({})).await;
    c.ok("mimo_sine.abort", json!({})).await;
    c.ok("mimo_sine.log", json!({})).await;
}
