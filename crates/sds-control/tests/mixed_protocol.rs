//! Mixed Mode through the protocol, on a real loopback WebSocket: configure and its
//! dry run, the configuration and its views, reset and clear, the controller
//! rules, every configuration refusal by name, the verbs refused before a run is
//! running, arming on the simulator and a stop from armed, the
//! hardware law gate of each role, the method list and the shutdown.

mod support;

use hertzforge_random_laws::{CheckStatus, LawCatalog, LawWorld};
use hertzforge_station::StationBuildError;
use sds_control::mixed::config::{self, CheckContext};
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use sds_control::{laws, table};
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

fn row(index: u32, role: &str, description: &str) -> Value {
    let units = if role == "drive" { "V" } else { "g" };
    json!({
        "index": index, "enabled": true, "description": description,
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": if units == "V" { 1000 } else { 100 },
        "range_max": if units == "V" { 10 } else { 100 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

fn golden_config() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden/request_mixed_configure.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    v["params"]["config"].clone()
}

/// The golden configuration at the simulator's rate.
fn config() -> Value {
    let mut c = golden_config();
    c["acquisition"]["fs_hz"] = json!(16384);
    c["acquisition"]["n_pts"] = json!(32768);
    c
}

async fn controller(c: &mut Client) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": [
            row(1, "control", "Control"), row(2, "drive", "Shaker"), row(3, "monitor", "Fixture")
        ]}),
    )
    .await;
}

fn service() -> std::sync::Arc<Service> {
    Service::with_options(options(
        no_driver(),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mixed_configure_its_dry_run_the_configuration_reset_and_clear() {
    let schema = Schema::load();
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    assert_eq!(
        c.ok("mixed.state", json!({})).await["state"],
        "unconfigured"
    );
    let r = obs
        .call(
            "mixed.configure",
            json!({"config": config(), "dry_run": true}),
        )
        .await;
    let dry = &r["result"];
    assert!(dry["errors"].as_array().unwrap().is_empty(), "{dry}");
    assert_eq!(dry["derived"]["grid"]["fs_hz"], 16384);
    assert!(
        obs.decoded_response(&r["id"]).is_some(),
        "views travel binary"
    );
    schema.assert_valid(&r, None);
    assert_eq!(
        obs.refused("mixed.configure", json!({"config": config()}))
            .await["reason"],
        "not_controller"
    );
    let r = c.ok("mixed.configure", json!({"config": config()})).await;
    schema.assert_valid(&r, Some("mixedConfigureResult"));
    assert_eq!(r["state"]["state"], "configured");
    assert_eq!(r["state"]["run_id"], "mixed-0001");
    assert_eq!(r["state"]["parts"].as_array().unwrap().len(), 6);
    assert_eq!(r["derived"]["law"]["tones"]["live"], json!(["blend_w"]));
    assert_eq!(r["state"]["law"]["tones"]["live"], json!({"blend_w": 0.5}));
    let r = obs.call("mixed.config", json!({})).await;
    assert_eq!(r["result"]["run_id"], "mixed-0001");
    let views = obs.decoded_response(&r["id"]).unwrap().expanded();
    let titles: Vec<&str> = views["result"]["views"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Reference", "Tones"]);
    // Content errors are items; the run is not replaced.
    let mut bad = config();
    bad["limits"]["drive_vpk_abort"] = json!(1);
    let e = c.refused("mixed.configure", json!({"config": bad})).await;
    assert_eq!(e["reason"], "config_invalid");
    assert_eq!(e["detail"]["errors"][0]["reason"], "abort_below_ceiling");
    schema.assert_valid(&e["detail"], Some("report"));
    // A wrong shape names the field at the request's own path.
    let mut shape = config();
    shape["parts"][1]["profile"] = shape["parts"][2]["profile"].clone();
    let e = c.refused("mixed.configure", json!({"config": shape})).await;
    assert_eq!(e["reason"], "bad_params");
    assert_eq!(e["detail"]["param"], "config.parts[1].amplitude_g");
    let mut shape = config();
    shape["parts"][0]["breakpoints"][1]["freq_hz"] = json!("eighty");
    let e = c.refused("mixed.configure", json!({"config": shape})).await;
    assert_eq!(
        e["detail"]["param"],
        "config.parts[0].breakpoints[1].freq_hz"
    );
    let s = c.ok("mixed.reset", json!({})).await;
    assert_eq!(s["run_id"], "mixed-0002");
    let s = c.ok("mixed.clear", json!({})).await;
    assert_eq!(s["state"], "unconfigured");
    schema.assert_valid(&s, Some("mixedState"));
    assert_eq!(
        c.refused("mixed.start", json!({})).await["reason"],
        "not_configured"
    );
}

fn catalog() -> (LawCatalog, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "sds-control-mixed-protocol-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    (LawCatalog::open(&dir).unwrap(), dir)
}

fn table() -> hertzforge_channel_table::ChannelTable {
    let mut t = table::Table::new().table;
    let mut c1 = table::new_row(1);
    c1.role = hertzforge_channel_table::ChannelRole::Control;
    let mut c2 = table::new_row(2);
    c2.role = hertzforge_channel_table::ChannelRole::Drive;
    c2.units = hertzforge_channel_table::Units::VoltageV;
    let mut c3 = table::new_row(3);
    c3.role = hertzforge_channel_table::ChannelRole::Monitor;
    t.channels = vec![c1, c2, c3];
    t
}

/// The first error of `c`, as `(reason, field)`.
fn first_error(c: &Value) -> (String, String) {
    let w = config::parse(c).unwrap_or_else(|e| panic!("{e:?}"));
    let (cat, dir) = catalog();
    let floor = |i: &str| laws::mixed_lookup(Some(&cat), i, LawWorld::Single);
    let tones = |i: &str| laws::mixed_lookup(Some(&cat), i, LawWorld::Sine);
    let t = table();
    let ctx = CheckContext {
        table: &t,
        floor_law: &floor,
        tone_law: &tones,
    };
    let (r, d) = w.check(&ctx);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(d.is_none(), "refused configurations derive nothing");
    let i = r.errors.first().expect("an error").to_json();
    (
        i["reason"].as_str().unwrap().to_owned(),
        i["field"].as_str().unwrap_or("").to_owned(),
    )
}

fn with(edit: impl FnOnce(&mut Value)) -> Value {
    let mut c = golden_config();
    edit(&mut c);
    c
}

#[test]
fn every_configuration_refusal_is_named_and_located() {
    let tone = |id: &str, hz: f64| {
        json!({"id": id, "kind": "tone", "schedule": {"kind": "fixed", "freq_hz": hz},
               "amplitude_g": 0.1})
    };
    let band = |id: &str, hz: f64| {
        json!({"id": id, "kind": "narrowband", "schedule": {"kind": "fixed", "freq_hz": hz},
               "level_g2_hz": 0.01, "width_hz": 4})
    };
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "no_floor",
            "parts",
            with(|c| {
                c["parts"].as_array_mut().unwrap().remove(0);
            }),
        ),
        (
            "too_many_floors",
            "parts",
            with(|c| {
                let mut f = c["parts"][0].clone();
                f["id"] = json!("floor2");
                c["parts"].as_array_mut().unwrap().push(f);
            }),
        ),
        (
            "unknown_part_kind",
            "parts[1].kind",
            with(|c| c["parts"][1]["kind"] = json!("pulse_train")),
        ),
        (
            "duplicate_part_id",
            "parts[2].id",
            with(|c| c["parts"][2]["id"] = json!("t1")),
        ),
        (
            "part_out_of_band",
            "parts[1]",
            with(|c| c["parts"][1]["schedule"]["freq_hz"] = json!(2500)),
        ),
        (
            "part_out_of_band",
            "parts[3]",
            with(|c| c["parts"][3]["schedule"]["ratio"] = json!(4)),
        ),
        (
            "parts_too_close",
            "parts[1]",
            with(|c| c["parts"][1]["schedule"]["freq_hz"] = json!(300.5)),
        ),
        (
            "too_many_tones",
            "parts",
            with(|c| {
                for k in 0..12 {
                    c["parts"]
                        .as_array_mut()
                        .unwrap()
                        .push(tone(&format!("x{k}"), 1500.0 + 20.0 * k as f64));
                }
            }),
        ),
        (
            "too_many_bands",
            "parts",
            with(|c| {
                for k in 0..11 {
                    c["parts"]
                        .as_array_mut()
                        .unwrap()
                        .push(band(&format!("y{k}"), 1000.0 + 10.0 * k as f64));
                }
            }),
        ),
        (
            "follow_unknown_part",
            "parts[3].schedule.of",
            with(|c| c["parts"][3]["schedule"]["of"] = json!("nobody")),
        ),
        (
            "follow_unknown_part",
            "parts[3].schedule.of",
            with(|c| c["parts"][3]["schedule"]["of"] = json!("floor")),
        ),
        (
            "follow_self",
            "parts[3].schedule.of",
            with(|c| c["parts"][3]["schedule"]["of"] = json!("t3")),
        ),
        (
            "follow_loop",
            "parts[2].schedule.of",
            with(|c| {
                c["parts"][2]["schedule"] = json!({"kind": "follow", "of": "t3", "ratio": 0.5});
            }),
        ),
        (
            "schedule_unsupported",
            "parts[1].schedule.kind",
            with(|c| c["parts"][1]["schedule"] = json!({"kind": "segments"})),
        ),
        (
            "profile_not_covering",
            "parts[2].profile",
            with(|c| c["parts"][2]["schedule"]["band_hz"] = json!([300, 700])),
        ),
        (
            "tick_out_of_range",
            "acquisition.tick_ms",
            with(|c| c["acquisition"]["tick_ms"] = json!(1)),
        ),
        (
            "abort_below_ceiling",
            "limits.drive_vpk_abort",
            with(|c| c["limits"]["drive_vpk_abort"] = json!(3)),
        ),
        (
            "law_unknown",
            "control.law.floor",
            with(|c| c["control"]["law"]["floor"]["id"] = json!("no_such_law")),
        ),
        (
            "law_wrong_world",
            "control.law.floor",
            with(|c| c["control"]["law"]["floor"]["id"] = json!("servo_blend")),
        ),
        (
            "law_wrong_world",
            "control.law.tones",
            with(|c| c["control"]["law"]["tones"]["id"] = json!("hinv")),
        ),
        (
            "law_param_unknown",
            "control.law.tones.params.nope",
            with(|c| c["control"]["law"]["tones"]["params"]["nope"] = json!(1)),
        ),
        (
            "law_param_invalid",
            "control.law.tones.params.blend_w",
            with(|c| c["control"]["law"]["tones"]["params"]["blend_w"] = json!(5)),
        ),
        (
            "out_of_range",
            "control.tracking",
            with(|c| c["control"]["tracking"] = json!({"kind": "proportional"})),
        ),
        (
            "out_of_range",
            "exclusion_hz",
            with(|c| {
                c["acquisition"]["n_pts"] = json!(8192);
                c["exclusion_hz"] = json!(1.5);
            }),
        ),
        (
            "out_of_range",
            "parts[0].enabled",
            with(|c| c["parts"][0]["enabled"] = json!(false)),
        ),
        (
            "out_of_range",
            "parts[1].id",
            with(|c| c["parts"][1]["id"] = json!("has space")),
        ),
        (
            "out_of_range",
            "exclusion_hz",
            with(|c| c["exclusion_hz"] = json!(60)),
        ),
        (
            "out_of_range",
            "control.part_ramp_s",
            with(|c| c["control"]["part_ramp_s"] = json!(0)),
        ),
        (
            "out_of_range",
            "parts[5].width_share",
            with(|c| c["parts"][5]["width_share"] = json!(1.5)),
        ),
        (
            "frequency_not_ascending",
            "parts[0].breakpoints[2]",
            with(|c| c["parts"][0]["breakpoints"][2]["freq_hz"] = json!(50)),
        ),
        (
            "limit_unknown_channel",
            "",
            with(|c| c["limits"]["channels"][0]["channel"] = json!(9)),
        ),
    ];
    for (reason, field, c) in cases {
        let (r, f) = first_error(&c);
        assert_eq!(
            (r.as_str(), f.as_str()),
            (reason, field),
            "{reason} at {field}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_run_verbs_are_refused_by_name_before_a_run_is_running() {
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok("mixed.configure", json!({"config": config()})).await;
    for (m, p) in [
        ("mixed.proceed", json!({})),
        ("mixed.level", json!({"delta_db": -1})),
        ("mixed.hold", json!({})),
        ("mixed.resume", json!({})),
        ("mixed.part", json!({"id": "b2", "on": false})),
        ("mixed.adjust", json!({"key": "blend_w", "value": 0.6})),
    ] {
        let e = c.refused(m, p.clone()).await;
        assert_eq!(e["reason"], "wrong_state", "{m}");
        assert_eq!(obs.refused(m, p).await["reason"], "not_controller", "{m}");
    }
    // The mode may be set in any configured state.
    assert_eq!(
        c.ok("mixed.mode", json!({"mode": "manual"})).await["mode"],
        "manual"
    );
    let e = c
        .refused("mixed.part", json!({"id": "nobody", "on": true}))
        .await;
    assert_eq!(
        (e["reason"].as_str(), e["detail"]["why"].as_str()),
        (Some("part_refused"), Some("unknown_part"))
    );
    let e = c
        .refused("mixed.part", json!({"id": "floor", "on": false}))
        .await;
    assert_eq!(e["detail"]["why"], "floor");
    let e = c
        .refused("mixed.level", json!({"delta_db": -1, "to_db": 0}))
        .await;
    assert_eq!(e["reason"], "bad_params");
    // Any screen may stop or abort; a configured run is not live, so nothing moves.
    assert_eq!(obs.ok("mixed.stop", json!({})).await["state"], "configured");
    assert_eq!(
        obs.ok("mixed.abort", json!({})).await["state"],
        "configured"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn arming_on_the_simulator_opens_the_station_and_a_stop_from_armed_ends_at_once() {
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok("mixed.configure", json!({"config": config()})).await;
    let s = c.ok("mixed.arm", json!({})).await;
    assert_eq!(s["state"], "armed", "{s}");
    assert_eq!(s["grid"]["actual"], true, "{s}");
    assert_eq!(
        c.refused("mixed.arm", json!({})).await["reason"],
        "run_active"
    );
    let s = c.ok("mixed.stop", json!({})).await;
    assert_eq!(s["state"], "finished", "{s}");
    assert_eq!(s["end"]["outcome"], "operator_stop", "{s}");
    assert_eq!(
        c.refused("mixed.arm", json!({})).await["reason"],
        "run_finished"
    );
}

#[test]
fn a_role_law_drives_hardware_only_with_its_own_check_and_its_mixed_check() {
    let builtin = laws::builtin_sine_entry();
    assert_eq!(
        laws::mixed_clearance(None, &builtin, LawWorld::Sine),
        Ok(())
    );
    let mut imported = builtin.clone();
    imported.builtin = false;
    imported.check = CheckStatus::Unchecked;
    imported.identity.sha256 = "ab".repeat(32);
    // With no record to read, an imported law is not cleared in either role.
    assert!(laws::mixed_clearance(None, &imported, LawWorld::Sine).is_err());
    let mut floor = imported.clone();
    floor.world = LawWorld::Single;
    assert!(laws::mixed_clearance(None, &floor, LawWorld::Single).is_err());
    // A law of the other role's world is never cleared in this one.
    assert!(laws::mixed_clearance(None, &floor, LawWorld::Sine).is_err());
    // The role facts say so in `hardware`, and why.
    let facts = laws::mixed_role_facts(
        &floor,
        LawWorld::Single,
        laws::mixed_clearance(None, &floor, LawWorld::Single),
    );
    assert!(!facts.hardware);
    assert!(facts.not_cleared.is_some());
    assert_eq!(facts.world, Ok(()));
    assert!(laws::mixed_role_facts(&floor, LawWorld::Sine, Ok(()))
        .world
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_mixed_method_is_served_and_listed() {
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    let r = c.call("mixed.nope", json!({})).await;
    assert_eq!(r["error"]["code"], "unknown_method");
    let words = r["error"]["message"].as_str().unwrap().to_owned();
    for m in [
        "configure",
        "config",
        "arm",
        "start",
        "proceed",
        "level",
        "mode",
        "hold",
        "resume",
        "part",
        "adjust",
        "stop",
        "abort",
        "reset",
        "clear",
        "state",
        "log",
        "subscribe",
    ] {
        let method = format!("mixed.{m}");
        assert!(words.contains(&method), "{method} is not listed");
        let r = c.call(&method, json!({})).await;
        assert_ne!(
            r["error"]["code"], "unknown_method",
            "{method} is not served"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mixed_shutdown_refuses_the_drive_verbs_and_leaves_hold_stop_and_abort() {
    let svc = service();
    let addr = serve(svc.clone()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok("mixed.configure", json!({"config": config()})).await;
    let said = svc.shutdown().await;
    assert!(said.is_none(), "nothing was live: {said:?}");
    for (m, p) in [
        ("mixed.arm", json!({})),
        ("mixed.start", json!({})),
        ("mixed.proceed", json!({})),
        ("mixed.level", json!({"delta_db": 1})),
        ("mixed.mode", json!({"mode": "manual"})),
        ("mixed.resume", json!({})),
        ("mixed.part", json!({"id": "b2", "on": true})),
        ("mixed.adjust", json!({"key": "blend_w", "value": 0.2})),
    ] {
        assert_eq!(c.refused(m, p).await["reason"], "shutting_down", "{m}");
    }
    assert_ne!(
        c.refused("mixed.hold", json!({})).await["reason"],
        "shutting_down"
    );
    c.ok("mixed.stop", json!({})).await;
    c.ok("mixed.abort", json!({})).await;
    c.ok("mixed.config", json!({})).await;
    c.ok("mixed.log", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subscription_is_answered_with_its_defaults_filled() {
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    let r = c
        .ok(
            "mixed.subscribe",
            json!({"plots": [{"id": "a", "quantity": "amplitude"}, {"id": "t", "quantity": "time"}]}),
        )
        .await;
    assert_eq!(r["plots"][0]["parts"], "all");
    assert_eq!(r["plots"][1]["points"], 1024);
    let e = c
        .refused(
            "mixed.subscribe",
            json!({"plots": [{"id": "a", "quantity": "control", "parts": "all"}]}),
        )
        .await;
    assert_eq!(e["detail"]["param"], "plots[0].parts");
    assert_eq!(
        c.ok("mixed.subscribe", json!({"default": true})).await["default"],
        true
    );
}
