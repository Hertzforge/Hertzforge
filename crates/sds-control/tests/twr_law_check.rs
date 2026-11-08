//! `laws.check` of a TWR law through the protocol: the refusals given before any
//! case runs, every case's result in the protocol's words, and (on the TWR core)
//! the built-in law passing every case unrecorded, a misbehaving law failing, a
//! good law file recorded, and an unchecked law file refused at a hardware arm.

mod support;

use hertzforge_hal::{DiscoveredChannel, DiscoveredDevice};
use hertzforge_random_laws::CheckCase;
use hertzforge_station::StationBuildError;
use hertzforge_twr_laws::{TwrCaseEnd, TwrCaseMeasure, TwrCheckCase};
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
    let mut o = options(
        FixedScanner(Err(StationBuildError::NiDriverUnavailable(
            "not installed on this test host".into(),
        ))),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    );
    o.twr_engine = None;
    Service::with_options(o)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_twr_check_is_refused_by_name_before_it_runs() {
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    let e = obs
        .refused("laws.check", json!({"id": "twr_correct"}))
        .await;
    assert_eq!(e["reason"], "not_controller");
    for bad in [json!({"beta": 9}), json!({"zeta": 1})] {
        let e = c
            .refused("laws.check", json!({"id": "twr_correct", "params": bad}))
            .await;
        assert_eq!(e["reason"], "law_params_invalid", "{bad}: {e}");
    }
    let e = c
        .refused("laws.check", json!({"id": "twr_correct", "params": []}))
        .await;
    assert_eq!(e["reason"], "bad_params");
}

fn measure(end: TwrCaseEnd) -> TwrCaseMeasure {
    TwrCaseMeasure {
        end,
        plays: 7,
        rungs: vec![(-12.0, Some(1)), (0.0, None)],
        full_level_error_pct: Some(3.5),
        lag_frames: Some(4),
        lag_measured: true,
        calls: 2,
        misses: 0,
        refused: 0,
        clamped_bins: 0,
        worst_peak_rise_db: Some(1.5),
        worst_bin_rise_db: None,
        worst_call_ms: Some(2.5),
        budget_ms: 1000.0,
        max_drive_vpk: 1.25,
        drive_ceiling_vpk: 10.0,
    }
}

#[test]
fn every_case_end_is_said_in_the_protocols_words_and_validates() {
    let schema = Schema::load();
    for (end, name, trip) in [
        (TwrCaseEnd::Completed, "completed", Value::Null),
        (
            TwrCaseEnd::NotConverged,
            "not_converged",
            json!("not_converged"),
        ),
        (TwrCaseEnd::Divergence, "aborted", json!("divergence")),
        (
            TwrCaseEnd::Aborted("envelope".into()),
            "aborted",
            json!("envelope"),
        ),
        (
            TwrCaseEnd::LawFault("over_budget".into()),
            "law_fault",
            json!("over_budget"),
        ),
        (TwrCaseEnd::Held, "held", Value::Null),
        (
            TwrCaseEnd::Fault("the run failed".into()),
            "fault",
            json!("the run failed"),
        ),
    ] {
        for case in TwrCheckCase::ALL {
            let v = CheckCase {
                name: case.name().into(),
                passed: name == "completed",
                detail: "words".into(),
            };
            let j = sds_control::twr::law_check::case_json(case, &measure(end.clone()), &v);
            assert_eq!(j["end"], name);
            assert_eq!(j["trip"], trip);
            assert_eq!(j["requirement"], case.requirement().as_str());
            assert_eq!(
                j["rungs"],
                json!([{"level_db": -12, "plays_to_gate": 1}, {"level_db": 0, "plays_to_gate": null}])
            );
            schema.assert_valid(&j, Some("twrLawCheckCase"));
        }
    }
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
async fn the_built_in_twr_law_passes_every_case_and_is_not_recorded() {
    let _turn = ONE_CHECK.lock().await;
    let schema = Schema::load();
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let r = c.ok("laws.check", json!({"id": "twr_correct"})).await;
    schema.assert_valid(&r, Some("lawsCheckResult"));
    assert_eq!(r["report"]["passed"], true, "{r}");
    assert_eq!(r["report"]["recorded"], false);
    assert_eq!(r["report"]["tool_version"], "twr-check 1");
    let cases = r["report"]["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 8);
    for k in cases {
        assert_eq!(k["misses"], 0, "{k}");
        assert_eq!(k["refused"], 0, "{k}");
        assert!(k["worst_call_ms"].as_f64().unwrap() <= k["budget_ms"].as_f64().unwrap());
    }
    // The deep queue's lag is the measured one.
    assert_eq!(cases[4]["lag_frames"], 41, "{}", cases[4]);
    assert_eq!(cases[4]["lag_measured"], true);
    let events = c.of_topic("laws.check");
    assert_eq!(
        events.len(),
        16,
        "one as each case starts and one as it ends"
    );
    for e in events {
        schema.assert_valid(e.data(), Some("lawsCheckEvent"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_misbehaving_twr_law_fails_a_good_one_is_recorded_and_an_unchecked_one_cannot_arm_on_hardware(
) {
    let _turn = ONE_CHECK.lock().await;
    let mut o = options(
        FixedScanner(Ok(test_rig())),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    );
    o.twr_engine = None;
    let data_root = o.data_root.clone();
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let bad = c
        .ok(
            "laws.check",
            json!({"path": law_file("law_fixture_twr_misbehave").display().to_string(),
                   "params": {"mode": "nan"}}),
        )
        .await;
    assert_eq!(bad["report"]["passed"], false);
    assert_eq!(bad["report"]["recorded"], true);
    assert_eq!(bad["law"]["world"], "twr");
    assert_eq!(bad["law"]["check"]["hardware"], false);
    let good = c
        .ok(
            "laws.check",
            json!({"path": law_file("law_fixture_twr_template").display().to_string()}),
        )
        .await;
    assert_eq!(good["report"]["passed"], true, "{good}");
    assert_eq!(good["law"]["check"]["status"], "passed");
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
    let text: String = (0..4096)
        .map(|i| format!("{}\n", ((i % 64) as f64 - 32.0) / 128.0))
        .collect();
    let sha = c
        .ok(
            "twr.reference.import",
            json!({"text": text, "fs_hz": 12800}),
        )
        .await["reference"]["sha256"]
        .clone();
    let cfg = |id: &Value| {
        json!({"reference": {"sha256": sha}, "control": {"law": {"id": id}},
               "limits": {"drive_vpk_ceiling": 0.5}})
    };
    c.ok("twr.configure", json!({"config": cfg(&bad["law"]["id"])}))
        .await;
    let e = c.refused("twr.arm", json!({})).await;
    assert_eq!(e["reason"], "law_not_checked", "{e}");
    c.ok("twr.configure", json!({"config": cfg(&good["law"]["id"])}))
        .await;
    // The checked law passes the gate to the station's own refusal: the station is
    // brought up as the run arms (open, configure at the asked rate, the arm gate),
    // and there the rig's missing driver refuses the arm before anything is driven.
    let e = c.refused("twr.arm", json!({})).await;
    assert_eq!(e["reason"], "arm_refused", "{e}");
    assert!(
        e["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("The station refused to arm: ")),
        "{e}"
    );
    assert_eq!(c.ok("twr.plays", json!({})).await["plays"], json!([]));
    c.ok("twr.clear", json!({})).await;

    // A record of an earlier check: the same file, passed, but at a check the
    // current one has replaced, is refused by name with both versions.
    let sha = good["law"]["id"].as_str().unwrap().to_owned();
    let record = data_root.join("laws").join(format!("{sha}.check.json"));
    let mut r: Value = serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    assert_eq!(r["tool_version"], "twr-check 1");
    r["tool_version"] = json!("twr-check 0");
    std::fs::write(&record, serde_json::to_vec(&r).unwrap()).unwrap();
    drop(c);
    let mut o = options(
        FixedScanner(Ok(test_rig())),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    );
    o.twr_engine = None;
    o.data_root = data_root.clone();
    o.capture_root = Some(data_root.clone());
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let law = c.ok("laws.list", json!({"world": "twr"})).await["laws"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == sha)
        .cloned()
        .unwrap();
    assert_eq!(law["check"]["hardware"], false, "{law}");
    c.ok(
        "station.select",
        json!({"kind": "ni", "devices": ["TestRig9Mod1", "TestRig9Mod2"]}),
    )
    .await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": [
            bound(1, "control", "TestRig9Mod1", "ai0"),
            bound(2, "drive", "TestRig9Mod2", "ao0"),
        ]}),
    )
    .await;
    let sha_ref = c
        .ok(
            "twr.reference.import",
            json!({"text": text, "fs_hz": 12800}),
        )
        .await["reference"]["sha256"]
        .clone();
    c.ok(
        "twr.configure",
        json!({"config": {"reference": {"sha256": sha_ref}, "control": {"law": {"id": sha}},
                          "limits": {"drive_vpk_ceiling": 0.5}}}),
    )
    .await;
    let e = c.refused("twr.arm", json!({})).await;
    assert_eq!(e["reason"], "law_not_checked", "{e}");
    assert_eq!(
        (
            e["detail"]["tool_version"].as_str(),
            e["detail"]["current"].as_str()
        ),
        (Some("twr-check 0"), Some("twr-check 1")),
        "{e}"
    );
    let _ = std::fs::remove_dir_all(&data_root);
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
