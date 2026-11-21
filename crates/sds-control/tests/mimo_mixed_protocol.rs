//! MIMO Mixed Mode over the wire: every refusal by its name (the module's verbs
//! before a configuration, a verb this module does not build, bad parameters, a
//! verb in the wrong state, a part that cannot be switched, a screen without
//! control, a shutting-down service); MIMO Random's reference verbs under this
//! module's names; laws imported for each role run on the simulator; an
//! unchecked imported law is refused on a hardware station, role by role; and a
//! run ends safely when it is stopped, aborted, left without a controller or shut
//! down.
//!
//! The runs are SIMULATED; nothing here is evidence about hardware.

mod support;

use hertzforge_hal::{DiscoveredChannel, DiscoveredDevice};
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use support::mmix::{self, controller, opts, refused_as};
use support::{options, serve, Client};

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden")
        .join(name)
}

fn writing() -> bool {
    std::env::var_os("MIMO_MIXED_GOLDEN_WRITE").is_some()
}

/// An error golden: the service's own refusal over the wire (the request's id
/// set to the golden's), written with `MIMO_MIXED_GOLDEN_WRITE=1`, otherwise
/// compared.
fn error_golden(name: &str, id: i64, error: &Value) {
    let got = json!({"type": "response", "id": id, "error": error});
    let path = golden(&format!("{name}.json"));
    if writing() {
        let pretty = serde_json::to_string_pretty(&got).unwrap();
        let mut out = String::new();
        for line in pretty.lines() {
            let body = line.trim_start_matches(' ');
            out.push_str(&" ".repeat((line.len() - body.len()) / 2));
            out.push_str(body);
            out.push('\n');
        }
        std::fs::write(&path, out).unwrap();
        return;
    }
    let want: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(got, want, "{name}");
}

async fn client_with(
    o: sds_control::server::ServiceOptions,
) -> (std::sync::Arc<Service>, std::net::SocketAddr, Client) {
    let svc = Service::with_options(o);
    let addr = serve(svc.clone()).await;
    (svc, addr, Client::connect(addr).await)
}

async fn client() -> (std::net::SocketAddr, Client) {
    let (_, addr, c) = client_with(opts(Duration::from_secs(600))).await;
    (addr, c)
}

fn config() -> Value {
    mmix::manual(mmix::floor_tones_band(), -6.0)
}

/// Configure `cfg`, start, review, proceed: the run engaged.
async fn run_to_engaged(c: &mut Client, cfg: &Value) -> Value {
    c.ok("mimo_mixed.configure", json!({"config": cfg})).await;
    c.ok("mimo_mixed.start", json!({})).await;
    mmix::phase(c, "awaiting_review").await;
    c.ok("mimo_mixed.proceed", json!({})).await;
    mmix::phase(c, "engaged").await
}

// ---- refusals by name ----------------------------------------------------------------

/// Every verb this module names but does not build.
const NOT_BUILT: [(&str, &str); 4] = [
    ("mimo_mixed.adjust", "adjust_not_built"),
    ("mimo_mixed.exclude", "exclude_not_built"),
    (
        "mimo_mixed.identification.save",
        "identification_save_not_built",
    ),
    (
        "mimo_mixed.identification.load",
        "identification_load_not_built",
    ),
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_mixed_refuses_by_name_before_a_configuration_and_for_what_it_does_not_build() {
    let (_, mut c) = client().await;
    controller(&mut c).await;
    let s = c.ok("mimo_mixed.state", json!({})).await;
    assert_eq!(s["state"], "unconfigured", "{s}");
    let r = c.ok("mimo_mixed.config", json!({})).await;
    assert!(r["config"].is_null() && r["run_id"].is_null(), "{r}");
    for m in [
        "mimo_mixed.arm",
        "mimo_mixed.start",
        "mimo_mixed.proceed",
        "mimo_mixed.hold",
        "mimo_mixed.resume",
        "mimo_mixed.reset",
    ] {
        refused_as(&mut c, m, json!({}), "not_configured").await;
    }
    refused_as(
        &mut c,
        "mimo_mixed.level",
        json!({"delta_db": 1}),
        "not_configured",
    )
    .await;
    refused_as(
        &mut c,
        "mimo_mixed.part",
        json!({"id": "t1", "on": false}),
        "not_configured",
    )
    .await;
    refused_as(
        &mut c,
        "mimo_mixed.mode",
        json!({"mode": "manual"}),
        "not_configured",
    )
    .await;
    refused_as(
        &mut c,
        "mimo_mixed.override",
        json!({"ack": "Accepted."}),
        "not_configured",
    )
    .await;
    for (m, reason) in NOT_BUILT {
        let e = refused_as(&mut c, m, json!({}), reason).await;
        assert!(
            e["message"].as_str().is_some_and(|s| !s.contains("  ")),
            "{e}"
        );
        if m == "mimo_mixed.adjust" {
            error_golden("response_error_mimo_mixed_not_built", 97, &e);
        }
    }
    // The list the service holds is the list here.
    assert_eq!(sds_control::mimo_mixed::NOT_BUILT.len(), NOT_BUILT.len());
    for (m, r) in NOT_BUILT {
        assert!(sds_control::mimo_mixed::NOT_BUILT.contains(&(m, r)), "{m}");
    }
    // A configuration with a problem is refused with the report, and nothing is
    // configured.
    let mut bad = config();
    bad["drives"][0]["channel_index"] = json!(12);
    let e = refused_as(
        &mut c,
        "mimo_mixed.configure",
        json!({"config": bad}),
        "config_invalid",
    )
    .await;
    error_golden("response_error_mimo_mixed_config_invalid", 92, &e);
    // One channel is one drive: a second row for it is refused by name.
    let mut twice = config();
    twice["drives"][1]["channel_index"] = twice["drives"][0]["channel_index"].clone();
    let e = refused_as(
        &mut c,
        "mimo_mixed.configure",
        json!({"config": twice}),
        "config_invalid",
    )
    .await;
    assert!(
        e["detail"]["errors"].as_array().is_some_and(|v| v
            .iter()
            .any(|i| i["reason"] == "duplicate_drive" && i["field"] == "drives[1]")),
        "{e}"
    );
    let e = refused_as(
        &mut c,
        "mimo_mixed.configure",
        json!({"config": "not an object"}),
        "bad_params",
    )
    .await;
    assert_eq!(e["detail"]["param"], "config", "{e}");
    let s = c.ok("mimo_mixed.state", json!({})).await;
    assert_eq!(s["state"], "unconfigured", "{s}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_mixed_refuses_bad_parameters_the_wrong_state_and_a_screen_without_control() {
    let (_, addr, mut c) = client_with(opts(Duration::from_secs(600))).await;
    controller(&mut c).await;
    c.ok("mimo_mixed.configure", json!({"config": config()}))
        .await;
    // Bad parameters are refused before the state is looked at.
    for (m, p) in [
        ("mimo_mixed.level", json!({})),
        ("mimo_mixed.level", json!({"delta_db": 1, "to_db": 0})),
        ("mimo_mixed.level", json!({"to_db": "full"})),
        ("mimo_mixed.part", json!({"on": false})),
        ("mimo_mixed.part", json!({"id": "t1"})),
        ("mimo_mixed.part", json!({"id": "t1", "on": "off"})),
        ("mimo_mixed.log", json!({"since": -1})),
        (
            "mimo_mixed.subscribe",
            json!({"rate_hz": 99, "plots": [{"id": "a", "quantity": "control"}]}),
        ),
        (
            "mimo_mixed.subscribe",
            json!({"plots": [{"id": "a", "quantity": "nothing"}]}),
        ),
        ("mimo_mixed.subscribe", json!({"plots": []})),
    ] {
        let e = c.refused(m, p.clone()).await;
        assert_eq!(e["reason"], "bad_params", "{m} {p}: {e}");
    }
    // The wrong state, by name.
    for m in ["mimo_mixed.proceed", "mimo_mixed.hold", "mimo_mixed.resume"] {
        refused_as(&mut c, m, json!({}), "wrong_state").await;
    }
    refused_as(
        &mut c,
        "mimo_mixed.level",
        json!({"delta_db": 1}),
        "wrong_state",
    )
    .await;
    // Another screen sees the run and cannot drive it.
    let mut obs = Client::connect(addr).await;
    for m in [
        "mimo_mixed.arm",
        "mimo_mixed.start",
        "mimo_mixed.reset",
        "mimo_mixed.clear",
    ] {
        refused_as(&mut obs, m, json!({}), "not_controller").await;
    }
    refused_as(
        &mut obs,
        "mimo_mixed.configure",
        json!({"config": config()}),
        "not_controller",
    )
    .await;
    assert_eq!(
        obs.ok("mimo_mixed.state", json!({})).await["state"],
        "configured"
    );
    obs.ok("mimo_mixed.config", json!({})).await;
    obs.ok("mimo_mixed.log", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_mixed_a_running_test_refuses_the_floor_switched_off_and_the_level_in_auto() {
    let (_, mut c) = client().await;
    controller(&mut c).await;
    run_to_engaged(&mut c, &config()).await;
    let e = refused_as(
        &mut c,
        "mimo_mixed.part",
        json!({"id": "floor", "on": false}),
        "part_refused",
    )
    .await;
    assert_eq!(e["detail"]["why"], "floor", "{e}");
    error_golden("response_error_mimo_mixed_part_refused", 96, &e);
    // A run cannot be armed or configured over while it plays.
    refused_as(&mut c, "mimo_mixed.arm", json!({}), "run_active").await;
    refused_as(
        &mut c,
        "mimo_mixed.configure",
        json!({"config": config()}),
        "run_active",
    )
    .await;
    refused_as(&mut c, "mimo_mixed.clear", json!({}), "run_active").await;
    c.ok("mimo_mixed.abort", json!({})).await;
    let end = mmix::state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_abort", "{}", end["end"]);
    refused_as(&mut c, "mimo_mixed.start", json!({}), "run_finished").await;
    refused_as(&mut c, "mimo_mixed.proceed", json!({}), "wrong_state").await;
    // Reset gives a fresh run of the same configuration; clear forgets it.
    let s = c.ok("mimo_mixed.reset", json!({})).await;
    assert_eq!(s["state"], "configured", "{s}");
    assert_eq!(s["run_id"], "mimo-mixed-0002", "{s}");
    let s = c.ok("mimo_mixed.clear", json!({})).await;
    assert_eq!(s["state"], "unconfigured", "{s}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_mixed_shutdown_refuses_the_drive_verbs_and_leaves_hold_stop_and_abort() {
    let (svc, _, mut c) = client_with(opts(Duration::from_secs(600))).await;
    controller(&mut c).await;
    c.ok("mimo_mixed.configure", json!({"config": config()}))
        .await;
    let said = svc.shutdown().await;
    assert!(said.is_none(), "nothing was live: {said:?}");
    for (m, p) in [
        ("mimo_mixed.arm", json!({})),
        ("mimo_mixed.start", json!({})),
        ("mimo_mixed.proceed", json!({})),
        ("mimo_mixed.level", json!({"delta_db": 1})),
        ("mimo_mixed.resume", json!({})),
        ("mimo_mixed.part", json!({"id": "t1", "on": true})),
    ] {
        refused_as(&mut c, m, p, "shutting_down").await;
    }
    let e = c.refused("mimo_mixed.hold", json!({})).await;
    assert_ne!(e["reason"], "shutting_down", "{e}");
    c.ok("mimo_mixed.stop", json!({})).await;
    c.ok("mimo_mixed.abort", json!({})).await;
    c.ok("mimo_mixed.config", json!({})).await;
    c.ok("mimo_mixed.log", json!({})).await;
}

// ---- MIMO Random's reference verbs ---------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_mixed_reference_verbs_answer_as_mimo_randoms_do() {
    let (_, mut c) = client().await;
    controller(&mut c).await;
    let read = |n: &str| -> Value {
        serde_json::from_str(&std::fs::read_to_string(golden(n)).unwrap()).unwrap()
    };
    for (golden_name, verb) in [
        (
            "request_mimo_random_reference_build_rigid_body.json",
            "build_rigid_body",
        ),
        (
            "request_mimo_random_reference_build_breakpoints.json",
            "build_breakpoints",
        ),
    ] {
        let params = read(golden_name)["params"].clone();
        let theirs = c
            .call(&format!("mimo_random.reference.{verb}"), params.clone())
            .await;
        let ours = c
            .call(&format!("mimo_mixed.reference.{verb}"), params.clone())
            .await;
        assert!(theirs.get("error").is_none(), "{verb}: {theirs}");
        assert_eq!(ours.get("result"), theirs.get("result"), "{verb}");
    }
    let e = c.refused("mimo_mixed.reference.validate", json!({})).await;
    let f = c.refused("mimo_random.reference.validate", json!({})).await;
    assert_eq!(e, f);
}

// ---- the laws, each role -------------------------------------------------------------

fn law_file(name: &str) -> PathBuf {
    let file = format!("{name}.wasm");
    let exe = std::env::current_exe().unwrap();
    exe.ancestors()
        .skip(1)
        .map(|d| d.join("wasm32-wasip2").join("release").join(&file))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{file} is not built; run scripts/build_laws.sh"))
}

async fn import(c: &mut Client, name: &str) -> Value {
    c.ok(
        "laws.import",
        json!({"path": law_file(name).display().to_string()}),
    )
    .await["law"]
        .clone()
}

/// Both roles' laws imported from files, each with a parameter set: the check
/// runs each in its role, and the run plays them on the simulator through the
/// whole flow, its record naming the files' bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_mixed_imported_laws_for_each_role_run_on_the_simulator() {
    let (_, mut c) = client().await;
    controller(&mut c).await;
    let floor = import(&mut c, "hertzforge_law_mimo_pinv").await;
    let tones = import(&mut c, "hertzforge_law_mimo_sine_matrix").await;
    let (floor_id, tones_id) = (
        floor["id"].as_str().unwrap().to_owned(),
        tones["id"].as_str().unwrap().to_owned(),
    );
    assert_eq!(floor["builtin"], false, "{floor}");
    assert_eq!(tones["builtin"], false, "{tones}");
    let floor_params = json!({"convergence": 0.25});
    let tones_params = json!({"carry": 0.5});
    let mut cfg = config();
    cfg["law"]["floor"] = json!({"id": floor_id, "params": floor_params});
    cfg["law"]["tones"] = json!({"id": tones_id, "params": tones_params});
    let r = c.ok("mimo_mixed.configure", json!({"config": cfg})).await;
    assert_eq!(r["derived"]["law"]["floor"]["id"], floor_id.as_str(), "{r}");
    assert_eq!(r["derived"]["law"]["tones"]["id"], tones_id.as_str(), "{r}");
    assert!(r["derived"]["law"]["floor"]["sha256"].is_string(), "{r}");
    c.ok("mimo_mixed.start", json!({})).await;
    mmix::phase(&mut c, "awaiting_review").await;
    c.ok("mimo_mixed.proceed", json!({})).await;
    mmix::phase(&mut c, "engaged").await;
    c.ok("mimo_mixed.level", json!({"to_db": 0.0})).await;
    let s = mmix::until(&mut c, "full level", |s| {
        s["level_db"] == 0.0 && s["floor"]["loops"].as_u64().unwrap_or(0) > 4
    })
    .await;
    assert_eq!(s["law"]["floor"]["id"], floor_id.as_str(), "{}", s["law"]);
    assert_eq!(s["law"]["tones"]["id"], tones_id.as_str(), "{}", s["law"]);
    for p in ["t1", "t2"] {
        let row = s["parts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == p)
            .unwrap();
        assert_eq!(row["on"], true, "{row}");
    }
    c.ok("mimo_mixed.stop", json!({})).await;
    let end = mmix::state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_stop", "{}", end["end"]);
    let folder = mmix::folder(&end);
    mmix::complete(&folder).await;
    let law: Value =
        serde_json::from_str(&std::fs::read_to_string(folder.join("law/law.json")).unwrap())
            .unwrap();
    assert_eq!(law["floor"]["law"]["sha256"], floor["sha256"], "{law}");
    assert_eq!(law["tones"]["law"]["sha256"], tones["sha256"], "{law}");
    assert_eq!(law["floor"]["choice"]["params"], floor_params, "{law}");
    assert_eq!(law["tones"]["choice"]["params"], tones_params, "{law}");
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

/// The 4 by 4 table bound to the rig.
async fn on_hardware(c: &mut Client) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok(
        "station.select",
        json!({"kind": "ni", "devices": ["TestRig9Mod1", "TestRig9Mod2"]}),
    )
    .await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    let rows: Vec<Value> = (1..=8u32)
        .map(|i| {
            let (role, dev, ch) = if i <= 4 {
                ("control", "TestRig9Mod1", format!("ai{}", i - 1))
            } else {
                ("drive", "TestRig9Mod2", format!("ao{}", i - 5))
            };
            let mut r = support::msine::row(i, role);
            r["physical"] = json!({"device": dev, "channel": ch});
            r
        })
        .collect();
    c.ok("channels.set", json!({"revision": rev, "channels": rows}))
        .await;
}

/// On a hardware station an imported law that has not passed its role's check
/// is refused `law_not_checked`, naming the role; built-in laws pass the law gate
/// (and the station, with no driver, refuses on its own); the same laws on the
/// simulator arm.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_mixed_an_unchecked_imported_law_is_refused_on_a_hardware_station() {
    let o = options(
        FixedScanner(Ok(test_rig())),
        0.0,
        Duration::from_secs(600),
        Duration::from_secs(5),
    );
    let (_, _, mut c) = client_with(o).await;
    on_hardware(&mut c).await;
    let floor = import(&mut c, "hertzforge_law_mimo_pinv").await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let tones = import(&mut c, "hertzforge_law_mimo_sine_matrix").await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut arm = async |f: &str, t: &str| -> Value {
        let mut cfg = config();
        cfg["law"]["floor"] = json!({"id": f});
        cfg["law"]["tones"] = json!({"id": t});
        c.ok("mimo_mixed.configure", json!({"config": cfg})).await;
        let e = c.refused("mimo_mixed.arm", json!({})).await;
        c.ok("mimo_mixed.clear", json!({})).await;
        e
    };
    let e = arm(&floor, "matrix").await;
    assert_eq!(e["reason"], "law_not_checked", "{e}");
    assert_eq!(e["detail"]["role"], "floor", "{e}");
    let e = arm("pinv_closed", &tones).await;
    assert_eq!(e["reason"], "law_not_checked", "{e}");
    assert_eq!(e["detail"]["role"], "tones", "{e}");
    let e = arm("pinv_closed", "matrix").await;
    assert_ne!(
        e["reason"], "law_not_checked",
        "built-in laws pass the law gate: {e}"
    );
    drop(arm);
    // The simulator takes the same unchecked laws.
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    let rows: Vec<Value> = (1..=8u32)
        .map(|i| support::msine::row(i, if i <= 4 { "control" } else { "drive" }))
        .collect();
    c.ok("channels.set", json!({"revision": rev, "channels": rows}))
        .await;
    let mut cfg = config();
    cfg["law"]["floor"] = json!({"id": floor});
    cfg["law"]["tones"] = json!({"id": tones});
    c.ok("mimo_mixed.configure", json!({"config": cfg})).await;
    let s = c.ok("mimo_mixed.arm", json!({})).await;
    assert_eq!(s["state"], "armed", "{s}");
    c.ok("mimo_mixed.abort", json!({})).await;
}

/// The rule at the library: on hardware each role's imported law must have
/// cleared its role; the simulator takes any law.
#[test]
fn mimo_mixed_the_hardware_rule_holds_per_role_at_the_library() {
    use sds_control::mimo_mixed::config;
    let dir = std::env::temp_dir().join(format!(
        "sds-control-mimo-mixed-rule-{}",
        std::process::id()
    ));
    let mut cat = hertzforge_random_laws::LawCatalog::open(&dir).unwrap();
    let floor = cat
        .import(&law_file("hertzforge_law_mimo_pinv"))
        .unwrap()
        .id
        .clone();
    let tones = cat
        .import(&law_file("hertzforge_law_mimo_sine_matrix"))
        .unwrap()
        .id
        .clone();
    let table = {
        let mut t = sds_control::table::Table::new().table;
        t.channels = (1..=8)
            .map(|i| {
                let mut r = sds_control::table::new_row(i);
                r.role = if i <= 4 {
                    hertzforge_channel_table::ChannelRole::Control
                } else {
                    hertzforge_channel_table::ChannelRole::Drive
                };
                if i > 4 {
                    r.units = hertzforge_channel_table::Units::VoltageV;
                }
                r
            })
            .collect();
        t
    };
    let core = |f: &str, t: &str| {
        let mut cfg = config();
        cfg["law"]["floor"] = json!({"id": f});
        cfg["law"]["tones"] = json!({"id": t});
        let w = config::parse(&cfg).unwrap();
        let (r, c) = config::check(&w, &table, &cat);
        c.unwrap_or_else(|| panic!("{:?}", r.errors)).core
    };
    for (f, t, role) in [
        (floor.as_str(), "matrix", Some("floor")),
        ("pinv_closed", tones.as_str(), Some("tones")),
        (floor.as_str(), tones.as_str(), Some("floor")),
        ("pinv_closed", "matrix", None),
    ] {
        let c = core(f, t);
        let hw = config::resolve_laws(&cat, &c, true);
        match role {
            Some(role) => {
                let e = hw
                    .err()
                    .unwrap_or_else(|| panic!("{f}+{t} armed on hardware"));
                assert_eq!(e.reason, Some("law_not_checked"), "{f}+{t}: {e:?}");
                assert_eq!(e.detail.as_ref().unwrap()["role"], role, "{f}+{t}");
            }
            None => assert!(hw.is_ok(), "built-in laws resolve on hardware"),
        }
        assert!(
            config::resolve_laws(&cat, &c, false).is_ok(),
            "{f}+{t}: the simulator takes any law"
        );
    }
    drop(cat);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- the safe endings ----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_mixed_a_stop_ramps_every_part_down_and_ends_operator_stop() {
    let (_, mut c) = client().await;
    controller(&mut c).await;
    run_to_engaged(&mut c, &config()).await;
    let s = c.ok("mimo_mixed.stop", json!({})).await;
    assert!(
        s["state"] == "stopping" && s["phase"] == "ramping_down" || s["state"] == "finished",
        "{s}"
    );
    let end = mmix::state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_stop", "{}", end["end"]);
    assert!(
        end["end"]["ramp_s"].as_f64().is_some_and(|r| r > 0.0),
        "{}",
        end["end"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_mixed_an_abort_ends_operator_abort_without_the_ramp() {
    let (addr, mut c) = client().await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    run_to_engaged(&mut c, &config()).await;
    // Any screen may abort.
    obs.ok("mimo_mixed.abort", json!({})).await;
    let end = mmix::state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_abort", "{}", end["end"]);
    assert!(end["end"]["ramp_s"].is_null(), "{}", end["end"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_mixed_a_dropped_controller_ends_in_the_safe_stop() {
    let svc = Service::with_options(opts(Duration::from_secs(2)));
    let addr = serve(svc).await;
    let mut obs = Client::connect(addr).await;
    {
        let mut c = Client::connect(addr).await;
        controller(&mut c).await;
        run_to_engaged(&mut c, &config()).await;
    }
    let i = obs
        .wait_for("the safe stop", mmix::LONG, |e| {
            e.topic() == "mimo_mixed.state" && e.data()["state"] == "finished"
        })
        .await;
    assert_eq!(obs.events[i].data()["end"]["outcome"], "safe_stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_mixed_the_shutdown_ends_a_driving_run_with_its_ramp() {
    let (svc, _, mut c) = client_with(opts(Duration::from_secs(600))).await;
    controller(&mut c).await;
    run_to_engaged(&mut c, &config()).await;
    let said = svc.shutdown().await.expect("a run was live");
    assert!(said.contains("safe_stop"), "{said}");
    let s = c.ok("mimo_mixed.state", json!({})).await;
    assert_eq!(s["state"], "finished", "{s}");
    assert_eq!(s["end"]["outcome"], "safe_stop", "{}", s["end"]);
    assert!(s["end"]["ramp_s"].as_f64().is_some(), "{}", s["end"]);
}

/// A call that may take minutes (the law check runs every case on the
/// simulator): its result.
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

/// A passed record of MIMO Random's own check for the file `sha`, beside it: the
/// floor law's own world's check (MIMO Random's to test; written here as the
/// Mixed Mode records test writes the Sine one).
/// A passed check of the law's own world for `role`'s law (MIMO Random's for
/// the floor, MIMO Sine's at the check in force for the tones), beside it.
fn write_own_record(data_root: &std::path::Path, sha: &str, role: &str) {
    if role == "floor" {
        return write_mimo_record(data_root, sha);
    }
    use hertzforge_random_laws::{CheckCase, CheckRecord};
    let cases = (0..4)
        .map(|k| CheckCase {
            name: format!("case {k}"),
            passed: true,
            detail: String::new(),
        })
        .collect();
    CheckRecord::new(
        sha,
        hertzforge_mimo_sine_laws::MIMO_SINE_CHECK_VERSION,
        "2026-11-21T10:00:00Z",
        cases,
    )
    .write(&data_root.join("laws"))
    .unwrap();
}

fn write_mimo_record(data_root: &std::path::Path, sha: &str) {
    use hertzforge_random_laws::{CheckCase, CheckRecord, MIMO_CHECK_VERSION};
    let cases = (0..4)
        .map(|k| CheckCase {
            name: format!("case {k}"),
            passed: true,
            detail: String::new(),
        })
        .collect();
    CheckRecord::new(sha, MIMO_CHECK_VERSION, "2026-11-21T10:00:00Z", cases)
        .write(&data_root.join("laws"))
        .unwrap();
}

/// On hardware an imported floor law needs its own world's check and the MIMO
/// Mixed check in the floor role. With the first alone the arm is refused
/// `law_not_checked`. `laws.check` in the role runs every case of the laws
/// crate's check with the law in that role and records the result against the
/// file; a record that did not pass clears nothing, and a passing one clears the
/// floor role alone. A floor law checked in the tones role is refused by name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_mixed_the_check_in_each_role_clears_that_role_on_a_hardware_station() {
    // One law check runs at a time in the process: the roles one after the other.
    the_check_clears_its_role("floor", "hertzforge_law_mimo_pinv").await;
    the_check_clears_its_role("tones", "hertzforge_law_mimo_sine_matrix").await;
}

/// The law file `name` imported for `role` ("floor" or "tones"), with its own
/// world's check passed, is refused on hardware until `laws.check` in that
/// role runs the MIMO Mixed check; the check's required cases pass, it is
/// recorded against the file with the informational swept-tone case beside
/// the verdict, and the law then clears its role and no other.
async fn the_check_clears_its_role(role: &str, name: &str) {
    let o = options(
        FixedScanner(Ok(test_rig())),
        0.0,
        Duration::from_secs(1800),
        Duration::from_secs(5),
    );
    let data_root = o.data_root.clone();
    let (svc, _, mut c) = client_with(o).await;
    on_hardware(&mut c).await;
    let law = import(&mut c, name).await;
    let id = law["id"].as_str().unwrap().to_owned();
    let sha = law["sha256"].as_str().unwrap().to_owned();
    write_own_record(&data_root, &sha, role);
    // The catalog reads a law's own check record as it opens: a fresh service on
    // the same data root sees the record just written.
    drop(c);
    drop(svc);
    let mut o = options(
        FixedScanner(Ok(test_rig())),
        0.0,
        Duration::from_secs(1800),
        Duration::from_secs(5),
    );
    o.data_root = data_root.clone();
    let (_svc, _, mut c) = client_with(o).await;
    on_hardware(&mut c).await;
    let mut cfg = config();
    cfg["law"][role] = json!({"id": id});
    let r = c.ok("mimo_mixed.configure", json!({"config": cfg})).await;
    assert_eq!(r["derived"]["law"][role]["hardware"], false, "{r}");
    let e = c.refused("mimo_mixed.arm", json!({})).await;
    assert_eq!(e["reason"], "law_not_checked", "{e}");
    assert_eq!(e["detail"]["role"], role, "{e}");
    let t0 = std::time::Instant::now();
    let r = long_call(&mut c, "laws.check", json!({"mimo_mixed": role, "id": id})).await;
    eprintln!("the {role} check took {:?}", t0.elapsed());
    let report = &r["report"];
    let cases = report["cases"].as_array().unwrap();
    for case in cases {
        eprintln!(
            "  {} required {} passed {}: {}",
            case["case"], case["required"], case["passed"], case["detail"]
        );
    }
    assert_eq!(report["role"], role, "{report}");
    assert_eq!(report["tool_version"], "mimo-mixed-check 2", "{report}");
    assert_eq!(cases.len(), 10, "{report}");
    let required: Vec<_> = cases.iter().filter(|k| k["required"] == true).collect();
    assert_eq!(required.len(), 9, "{report}");
    let swept = cases
        .iter()
        .find(|k| k["case"] == "swept_tone_through_mode")
        .unwrap();
    assert_eq!(swept["required"], false, "{swept}");
    assert_eq!(report["recorded"], true, "{report}");
    assert!(
        required.iter().all(|k| k["passed"] == true),
        "a required case failed: {report}"
    );
    assert_eq!(report["passed"], true, "{report}");
    assert_eq!(r["law"]["hardware"], true, "{}", r["law"]);
    let path = data_root
        .join("laws")
        .join(format!("{sha}.mimo-mixed-{role}.check.json"));
    let record: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(record["passed"], true, "{record}");
    assert_eq!(record["cases"].as_array().unwrap().len(), 9, "{record}");
    assert_eq!(
        record["informational"][0]["name"], "swept_tone_through_mode",
        "{record}"
    );
    let r = c.ok("mimo_mixed.configure", json!({"config": cfg})).await;
    assert_eq!(r["derived"]["law"][role]["hardware"], true, "{r}");
    // Past the law gate: the station, with no driver, refuses on its own.
    let e = c.refused("mimo_mixed.arm", json!({})).await;
    assert_ne!(e["reason"], "law_not_checked", "{e}");
    // The record clears its role alone: the same file is no law of the other.
    let other = if role == "floor" { "tones" } else { "floor" };
    let e = c
        .refused("laws.check", json!({"mimo_mixed": other, "id": id}))
        .await;
    assert_eq!(e["reason"], "law_wrong_world", "{e}");
    assert_eq!(e["detail"]["role"], other, "{e}");
}
