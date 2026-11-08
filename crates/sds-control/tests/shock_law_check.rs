//! Classical shock laws on the wire: the catalog's classical world, `laws.check`'s
//! refusals, each check case in the protocol's words, the built-in law passing the
//! check, the misbehaving fixture failing it, a law file's result recorded against
//! its hash, and an unchecked law file refused at a hardware arm.

mod support;

use hertzforge_hal::{DiscoveredChannel, DiscoveredDevice};
use hertzforge_random_laws::CheckCase;
use hertzforge_shock_laws::{ClassicalCaseEnd, ClassicalCaseMeasure, ClassicalCheckCase};
use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::shock::law_check;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use support::schema::Schema;
use support::{options, serve, Client};

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
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

fn config() -> Value {
    json!({
        "pulse": {"shape": "half_sine", "amplitude_g": 20, "duration_ms": 11},
        "acquisition": {"fs_hz": 20480},
        "schedule": {"mode": "manual", "levels_db": [-12, 0], "pulses": [1]},
        "identification": {"duration_s": 0.5},
        "limits": {"drive_vpk_ceiling": 0.5, "drive_vpk_abort": 0.8}
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_classical_world_lists_the_built_in_law_first() {
    let schema = Schema::load();
    let svc = Service::with_options(options(
        no_driver(),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let shock = c.ok("laws.list", json!({"world": "shock_classical"})).await;
    schema.assert_valid(&shock, Some("lawsListResult"));
    let laws = shock["laws"].as_array().unwrap();
    assert_eq!(laws[0]["id"], "fd_correct");
    assert!(laws.iter().all(|l| l["world"] == "shock_classical"));
    let golden: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../schema/golden/response_laws_list_shock_classical.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(laws[0], golden["result"]["laws"][0]);
    let all = c.ok("laws.list", json!({})).await;
    schema.assert_valid(&all, Some("lawsListResult"));
    let worlds: Vec<&str> = all["laws"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["world"].as_str().unwrap())
        .collect();
    assert!(worlds.contains(&"shock_classical") && worlds.contains(&"mimo"));
    let fdc = all["laws"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["id"] == "fd_correct")
        .count();
    assert_eq!(fdc, 1, "the built-in classical law is listed once");
    let mimo = c.ok("laws.list", json!({"world": "mimo"})).await;
    assert!(mimo["laws"]
        .as_array()
        .unwrap()
        .iter()
        .all(|l| l["world"] == "mimo"));
    let e = c.refused("laws.list", json!({"world": "sine"})).await;
    assert_eq!(e["reason"], "bad_params");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_classical_check_is_refused_by_name_before_it_runs() {
    let svc = Service::with_options(options(
        no_driver(),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    let e = obs.refused("laws.check", json!({"id": "fd_correct"})).await;
    assert_eq!(e["reason"], "not_controller");
    for bad in [
        json!({"beta": 2}),
        json!({"gain": 1}),
        json!({"beta": "half"}),
    ] {
        let e = c
            .refused("laws.check", json!({"id": "fd_correct", "params": bad}))
            .await;
        assert_eq!(e["reason"], "law_params_invalid", "{bad}: {e}");
    }
    let e = c
        .refused("laws.check", json!({"id": "fd_correct", "params": []}))
        .await;
    assert_eq!(e["reason"], "bad_params");
    // Not while a run is live.
    c.ok("shock.configure", json!({"config": config()})).await;
    c.ok("shock.arm", json!({})).await;
    let e = c.refused("laws.check", json!({"id": "fd_correct"})).await;
    assert_eq!(e["reason"], "run_active");
    c.ok("shock.stop", json!({})).await;
}

fn measure(end: ClassicalCaseEnd, first: Option<(u32, usize)>) -> ClassicalCaseMeasure {
    ClassicalCaseMeasure {
        end,
        pulses: 5,
        first_pass: first,
        pulses_passed: 0,
        full_level_pulses: 0,
        full_level_passed: 0,
        calls: 6,
        misses: 0,
        refused: 0,
        capped_bins: 0,
        capped_peaks: 0,
        worst_call_ms: None,
        budget_ms: 250.0,
        max_drive_vpk: 0.1234,
        drive_ceiling_vpk: 0.5,
    }
}

#[test]
fn every_case_end_is_said_in_the_protocols_words_and_validates() {
    let schema = Schema::load();
    let ends = [
        (ClassicalCaseEnd::Completed, "completed", None),
        (
            ClassicalCaseEnd::NotConverged,
            "not_converged",
            Some("not_converged"),
        ),
        (ClassicalCaseEnd::Divergence, "aborted", Some("divergence")),
        (
            ClassicalCaseEnd::Aborted("overshoot".into()),
            "aborted",
            Some("overshoot"),
        ),
        (
            ClassicalCaseEnd::LawFault("first_drive".into()),
            "law_fault",
            Some("first_drive"),
        ),
        (ClassicalCaseEnd::Held, "held", None),
        (
            ClassicalCaseEnd::Fault("the station went away".into()),
            "fault",
            Some("the station went away"),
        ),
    ];
    for case in ClassicalCheckCase::ALL {
        for (end, outcome, trip) in &ends {
            let v = CheckCase {
                name: case.name().into(),
                passed: false,
                detail: format!("ended {outcome}"),
            };
            let j = law_check::case_json(case, &measure(end.clone(), None), &v);
            schema.assert_valid(&j, Some("shockLawCheckCase"));
            assert_eq!(j["end"], *outcome);
            assert_eq!(j["trip"], trip.map_or(Value::Null, Value::from));
            assert_eq!(j["first_pass_pulse"], Value::Null);
            assert_eq!(j["worst_call_ms"], Value::Null);
        }
    }
    let j = law_check::case_json(
        ClassicalCheckCase::OneMode,
        &measure(ClassicalCaseEnd::Completed, Some((3, 0))),
        &CheckCase {
            name: "one mode".into(),
            passed: true,
            detail: "completed".into(),
        },
    );
    assert_eq!(j["first_pass_pulse"], 3);
    assert_eq!(j["requirement"], "pass_by");
    // A record passes exactly when it has cases and every one passed.
    let pass = |p: bool| CheckCase {
        name: "x".into(),
        passed: p,
        detail: "x".into(),
    };
    let r = law_check::check_record(
        "a".repeat(64),
        "2026-11-07T00:00:00Z".into(),
        vec![pass(true), pass(true)],
    );
    assert!(r.passed);
    let r = law_check::check_record(
        "a".repeat(64),
        "2026-11-07T00:00:00Z".into(),
        vec![pass(true), pass(false)],
    );
    assert!(!r.passed);
    assert!(!law_check::check_record("a".repeat(64), String::new(), Vec::new()).passed);
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

/// One law check runs at a time in a service process (`law_check_running`), and
/// the tests that run one share this process: they take turns.
static ONE_CHECK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_built_in_law_passes_every_case_and_is_not_recorded() {
    let _turn = ONE_CHECK.lock().await;
    let schema = Schema::load();
    let svc = Service::with_options(options(
        no_driver(),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let r = c.ok("laws.check", json!({"id": "fd_correct"})).await;
    schema.assert_valid(&r, Some("lawsCheckResult"));
    assert_eq!(r["report"]["passed"], true, "{r}");
    assert_eq!(r["report"]["recorded"], false);
    assert_eq!(r["report"]["cases"].as_array().unwrap().len(), 7);
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
    // The controller's own engine, so an arm reaches the station.
    let mut o = options(
        FixedScanner(Ok(test_rig())),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    );
    o.shock_engine = None;
    let svc = Service::with_options(o);
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let bad = c
        .ok(
            "laws.check",
            json!({"path": law_file("law_fixture_shock_misbehave").display().to_string(),
                   "params": {"mode": "nan"}}),
        )
        .await;
    assert_eq!(bad["report"]["passed"], false);
    assert_eq!(bad["report"]["recorded"], true);
    assert_eq!(bad["law"]["check"]["hardware"], false);
    assert!(bad["report"]["cases"]
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k["refused"].as_u64().unwrap() > 0));
    let good = c
        .ok(
            "laws.check",
            json!({"path": law_file("hertzforge_law_shock_fdc").display().to_string()}),
        )
        .await;
    assert_eq!(good["report"]["passed"], true, "{good}");
    assert_eq!(good["law"]["check"]["status"], "passed");
    assert_eq!(good["law"]["check"]["hardware"], true);
    // On a hardware station a law whose check failed cannot arm: refused before
    // any hardware is touched. The law that passed gets past that gate to the
    // station's own refusal (no driver can open the test rig's devices).
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
    let unchecked = bad["law"]["id"].as_str().unwrap().to_owned();
    let mut cfg = config();
    cfg["control"] = json!({"law": {"id": unchecked}});
    c.ok("shock.configure", json!({"config": cfg})).await;
    let e = c.refused("shock.arm", json!({})).await;
    assert_eq!(e["reason"], "law_not_checked", "{e}");
    let checked = good["law"]["id"].as_str().unwrap().to_owned();
    let mut cfg = config();
    cfg["control"] = json!({"law": {"id": checked}});
    c.ok("shock.configure", json!({"config": cfg})).await;
    let e = c.refused("shock.arm", json!({})).await;
    assert!(
        e["reason"] == "arm_refused" || e["reason"] == "ni_driver_unavailable",
        "{e}"
    );
}

/// A fictional NI rig: one input module, one output module, no driver to open
/// them.
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
