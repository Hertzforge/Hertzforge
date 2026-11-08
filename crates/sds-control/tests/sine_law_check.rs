//! `laws.check` of a Sine law through the protocol: the refusals given before any
//! case runs, every case's result in the protocol's words, and (on the Sine core)
//! the built-in law passing every case unrecorded, a misbehaving law failing, a
//! good law file recorded, and an unchecked law file refused at a hardware arm.

mod support;

use hertzforge_hal::{DiscoveredChannel, DiscoveredDevice};
use hertzforge_random_laws::CheckCase;
use hertzforge_sine_laws::{SineCaseEnd, SineCaseMeasure, SineCheckCase};
use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use support::schema::Schema;
use support::{options, serve, Client};

fn row(index: u32, role: &str) -> Value {
    let units = if role == "drive" { "V" } else { "g" };
    json!({
        "index": index, "enabled": true, "description": "",
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": if units == "V" { 1000 } else { 100 },
        "range_max": if units == "V" { 10 } else { 100 }, "polarity": "positive",
        "control_group": null,
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

fn service() -> std::sync::Arc<Service> {
    Service::with_options(options(
        FixedScanner(Err(StationBuildError::NiDriverUnavailable(
            "not installed on this test host".into(),
        ))),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ))
}

fn config() -> Value {
    json!({
        "profile": {"rows": [{"freq_hz": 50, "quantity": "accel_g", "value": 0.2},
                             {"freq_hz": 1000, "quantity": "accel_g", "value": 0.2}]},
        "test": {"kind": "dwell", "freq_hz": 100, "end": {"seconds": 5}},
        "schedule": {"levels_db": [0], "full_level": {"seconds": 5}},
        "limits": {"drive_vpk_ceiling": 0.5, "drive_vpk_abort": 0.8}
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sine_check_is_refused_by_name_before_it_runs() {
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    let e = obs
        .refused("laws.check", json!({"id": "servo_blend"}))
        .await;
    assert_eq!(e["reason"], "not_controller");
    for bad in [
        json!({"blend_w": 9}),
        json!({"zeta": 1}),
        json!({"compression": "medium"}),
    ] {
        let e = c
            .refused("laws.check", json!({"id": "servo_blend", "params": bad}))
            .await;
        assert_eq!(e["reason"], "law_params_invalid", "{bad}: {e}");
    }
    let e = c
        .refused("laws.check", json!({"id": "servo_blend", "params": []}))
        .await;
    assert_eq!(e["reason"], "bad_params");
}

fn measure(end: SineCaseEnd) -> SineCaseMeasure {
    SineCaseMeasure {
        end,
        ticks: 100,
        judged_ticks: 90,
        worst_error_db: Some(0.4),
        worst_error_hz: Some(251.0),
        worst_away_error_db: None,
        rungs: Vec::new(),
        worst_limit_over_db: None,
        worst_release_overshoot_db: None,
        calls: 100,
        misses: 0,
        refused: 0,
        capped_ticks: 0,
        worst_call_ms: Some(0.01),
        budget_ms: 2.5,
        max_drive_vpk: 0.4,
        drive_ceiling_vpk: 10.0,
        worst_rise_db: 0.5,
    }
}

#[test]
fn every_case_end_is_said_in_the_protocols_words_and_validates() {
    let schema = Schema::load();
    for (end, name, trip) in [
        (SineCaseEnd::Completed, "completed", Value::Null),
        (
            SineCaseEnd::Aborted("divergence".into()),
            "aborted",
            json!("divergence"),
        ),
        (
            SineCaseEnd::LawFault("over_budget".into()),
            "law_fault",
            json!("over_budget"),
        ),
        (SineCaseEnd::Held, "held", Value::Null),
        (
            SineCaseEnd::Fault("the loop failed".into()),
            "fault",
            json!("the loop failed"),
        ),
    ] {
        for case in SineCheckCase::ALL {
            let v = CheckCase {
                name: case.name().into(),
                passed: name == "completed",
                detail: "words".into(),
            };
            let j = sds_control::sine::law_check::case_json(case, &measure(end.clone()), &v);
            assert_eq!(j["end"], name);
            assert_eq!(j["trip"], trip);
            assert_eq!(j["requirement"], case.requirement().as_str());
            assert_eq!(j["rungs"], Value::Null);
            schema.assert_valid(&j, Some("sineLawCheckCase"));
        }
    }
    let mut m = measure(SineCaseEnd::Completed);
    m.rungs = vec![(-20.0, Some(0.25)), (0.0, None)];
    let v = CheckCase {
        name: "level ladder".into(),
        passed: false,
        detail: "a rung was not judged".into(),
    };
    let j = sds_control::sine::law_check::case_json(SineCheckCase::Ladder, &m, &v);
    assert_eq!(j["rungs"], json!([[-20, 0.25], [0, null]]));
    schema.assert_valid(&j, Some("sineLawCheckCase"));
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

/// The tests that run a whole check take turns: one check runs at a time.
static ONE_CHECK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_built_in_law_passes_every_case_and_is_not_recorded() {
    let _turn = ONE_CHECK.lock().await;
    let schema = Schema::load();
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let r = c.ok("laws.check", json!({"id": "servo_blend"})).await;
    schema.assert_valid(&r, Some("lawsCheckResult"));
    assert_eq!(r["report"]["passed"], true, "{r}");
    assert_eq!(r["report"]["recorded"], false);
    assert_eq!(r["report"]["tool_version"], "sine-check 1");
    let cases = r["report"]["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 7);
    for k in cases {
        assert_eq!(k["misses"], 0, "{k}");
        assert_eq!(k["refused"], 0, "{k}");
        assert!(k["worst_call_ms"].as_f64().unwrap() <= k["budget_ms"].as_f64().unwrap());
    }
    let events = c.of_topic("laws.check");
    assert_eq!(
        events.len(),
        14,
        "one as each case starts and one as it ends"
    );
    for e in events {
        schema.assert_valid(e.data(), Some("lawsCheckEvent"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_misbehaving_law_fails_a_good_one_is_recorded_and_an_unchecked_one_cannot_arm_on_hardware(
) {
    let _turn = ONE_CHECK.lock().await;
    let svc = Service::with_options(options(
        FixedScanner(Ok(test_rig())),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let bad = c
        .ok(
            "laws.check",
            json!({"path": law_file("law_fixture_sine_misbehave").display().to_string(),
                   "params": {"mode": "nan"}}),
        )
        .await;
    assert_eq!(bad["report"]["passed"], false);
    assert_eq!(bad["report"]["recorded"], true);
    assert_eq!(bad["law"]["world"], "sine");
    assert_eq!(bad["law"]["check"]["hardware"], false);
    assert!(bad["report"]["cases"]
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k["refused"].as_u64().unwrap() > 0));
    let good = c
        .ok(
            "laws.check",
            json!({"path": law_file("law_fixture_sine_template").display().to_string()}),
        )
        .await;
    assert_eq!(good["report"]["passed"], true, "{good}");
    assert_eq!(good["law"]["check"]["status"], "passed");
    assert_eq!(good["law"]["check"]["hardware"], true);
    // On a hardware station an unchecked law is refused before any hardware is
    // touched; the checked one gets past that gate to the station's own refusal.
    c.ok(
        "station.select",
        json!({"kind": "ni", "devices": ["TestRig9Mod1", "TestRig9Mod2"]}),
    )
    .await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    let bound = |index: u32, role: &str, device: &str, channel: &str| {
        let mut r = row(index, role);
        r["physical"] = json!({"device": device, "channel": channel});
        r
    };
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": [
            bound(1, "control", "TestRig9Mod1", "ai0"),
            bound(2, "drive", "TestRig9Mod2", "ao0"),
        ]}),
    )
    .await;
    let mut cfg = config();
    cfg["acquisition"] = json!({"tick_ms": 10});
    cfg["control"] = json!({"law": {"id": bad["law"]["id"]}});
    c.ok("sine.configure", json!({"config": cfg})).await;
    let e = c.refused("sine.arm", json!({})).await;
    assert_eq!(e["reason"], "law_not_checked", "{e}");
    let mut cfg = config();
    cfg["control"] = json!({"law": {"id": good["law"]["id"]}});
    c.ok("sine.configure", json!({"config": cfg})).await;
    let e = c.refused("sine.arm", json!({})).await;
    assert!(
        e["reason"] == "arm_refused" || e["reason"] == "ni_driver_unavailable",
        "{e}"
    );
}

/// A fictional NI rig: one input module, one output module, no driver to open them.
fn test_rig() -> Vec<DiscoveredDevice> {
    let dev = |id: &str, product: &str, n_in: usize, n_out: usize, slot: Option<u32>| {
        let mut d = DiscoveredDevice::new(id, product);
        d.serial = format!("T-{id}");
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
