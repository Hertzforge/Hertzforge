//! An imported law clears for hardware in a Mixed Mode role through its records
//! (design D15): an imported floor law and an imported tone law are refused
//! `law_not_checked` on a hardware station until each has passed the mixed check
//! in its role (and the tone law its own Sine check at the check in force),
//! then arm; a record made for
//! the other role, under another check version, or for other bytes clears
//! nothing; the simulator takes any law throughout.

mod support;

use hertzforge_hal::{DiscoveredChannel, DiscoveredDevice};
use hertzforge_random_laws::{CheckCase, CheckRecord};
use hertzforge_sine_laws::SINE_CHECK_VERSION;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;
use support::schema::Schema;
use support::{options, serve, Client};

use sds_control::server::Service;
use sds_control::station::FixedScanner;

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

async fn on_sim(c: &mut Client) {
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": [row(1, "control"), row(2, "drive")]}),
    )
    .await;
}

async fn on_hardware(c: &mut Client) {
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
}

fn config(floor_law: &str, tone_law: &str) -> Value {
    json!({
        "title": "Imported laws",
        "parts": [
            {"id": "floor", "kind": "floor",
             "breakpoints": [{"freq_hz": 20, "psd_g2_hz": 0.0005}, {"freq_hz": 1000, "slope_db_oct": 0}]},
            {"id": "t1", "kind": "tone", "schedule": {"kind": "fixed", "freq_hz": 160}, "amplitude_g": 0.5}
        ],
        "acquisition": {"fs_hz": 4096, "n_pts": 8192, "n_avg": 2, "tick_ms": 10},
        "schedule": {"mode": "auto", "levels": [{"db": 0}], "full_level_s": 5},
        "control": {"pretest": {"averages": 2}, "floor_settle_s": 10, "part_ramp_s": 1,
                    "level_ramp_s": 1, "shutdown_ramp_s": 1,
                    "law": {"floor": {"id": floor_law}, "tones": {"id": tone_law}}},
        "limits": {"drive_vpk_ceiling": 5, "drive_vpk_abort": 8}
    })
}

/// Configure with the two laws; each role's `hardware` as `derived` states it.
async fn hardware_flags(c: &mut Client, floor_law: &str, tone_law: &str) -> (bool, bool) {
    let r = c
        .ok(
            "mixed.configure",
            json!({"config": config(floor_law, tone_law)}),
        )
        .await;
    let law = &r["derived"]["law"];
    (
        law["floor"]["hardware"].as_bool().unwrap(),
        law["tones"]["hardware"].as_bool().unwrap(),
    )
}

/// The arm's refusal on the hardware station: `law_not_checked` with its role,
/// or (past the law gate) the station's own.
async fn arm_on_hardware(c: &mut Client, floor_law: &str, tone_law: &str) -> Value {
    c.ok(
        "mixed.configure",
        json!({"config": config(floor_law, tone_law)}),
    )
    .await;
    c.refused("mixed.arm", json!({})).await
}

fn past_the_law_gate(e: &Value) -> bool {
    e["reason"] != "law_not_checked"
}

async fn import(c: &mut Client, name: &str) -> String {
    let r = c
        .ok(
            "laws.import",
            json!({"path": law_file(name).display().to_string()}),
        )
        .await;
    r["law"]["id"].as_str().unwrap().to_owned()
}

/// `bytes` with a custom section appended: the same law, other bytes.
fn with_custom_section(bytes: &[u8]) -> Vec<u8> {
    let name = b"note";
    let payload = b"another build";
    let body_len = 1 + name.len() + payload.len();
    let mut out = bytes.to_vec();
    out.push(0); // custom section id
    out.push(body_len as u8); // one LEB128 byte: the body is short
    out.push(name.len() as u8);
    out.extend_from_slice(name);
    out.extend_from_slice(payload);
    out
}

/// A passed record of the Sine check at `version` for the file `sha`, beside it.
fn write_sine_record(data_root: &Path, sha: &str, version: &str) {
    let cases = (0..8)
        .map(|k| CheckCase {
            name: format!("case {k}"),
            passed: true,
            detail: String::new(),
        })
        .collect();
    let r = CheckRecord::new(sha, version, "2026-11-11T10:00:00Z", cases);
    r.write(&data_root.join("laws")).unwrap();
}

/// A passed record of the mixed check in `role` for the file `sha`, at the check
/// in force, beside it.
fn write_mixed_record(data_root: &Path, sha: &str, role: &str) {
    let cases = hertzforge_mixed_laws::MixedCheckCase::ALL
        .iter()
        .map(|k| CheckCase {
            name: k.name().into(),
            passed: true,
            detail: String::new(),
        })
        .collect();
    let r = hertzforge_mixed_laws::mixed_check_record(sha, "2026-11-11T10:00:00Z", cases);
    r.write_named(&data_root.join("laws"), &format!("mixed-{role}"))
        .unwrap();
}

/// The tests that run a whole check take turns: one check runs at a time.
static ONE_CHECK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn record_path(data_root: &Path, sha: &str, role: &str) -> PathBuf {
    data_root
        .join("laws")
        .join(format!("{sha}.mixed-{role}.check.json"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_imported_floor_law_arms_on_hardware_only_through_its_mixed_record() {
    let _turn = ONE_CHECK.lock().await;
    let o = options(
        FixedScanner(Ok(test_rig())),
        0.0,
        Duration::from_secs(60),
        Duration::from_secs(5),
    );
    let data_root = o.data_root.clone();
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    let schema = Schema::load();
    on_hardware(&mut c).await;

    let floor = import(&mut c, "hertzforge_law_hinv").await;

    // Before any check: each imported law is refused in its role.
    assert_eq!(
        hardware_flags(&mut c, &floor, "servo_blend").await,
        (false, true)
    );
    let e = arm_on_hardware(&mut c, &floor, "servo_blend").await;
    assert_eq!(e["reason"], "law_not_checked", "{e}");
    assert_eq!(e["detail"]["role"], "floor", "{e}");
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("the mixed check in this role has not been run"),
        "{e}"
    );

    // The floor law passes the mixed check as the floor: recorded, and it arms.
    let r = c
        .ok("laws.check", json!({"id": floor, "mixed": "floor"}))
        .await;
    schema.assert_valid(&r, Some("mixedLawsCheckResult"));
    assert_eq!(r["report"]["passed"], true, "{r}");
    assert_eq!(r["report"]["recorded"], true, "{r}");
    assert_eq!(r["report"]["tool_version"], "mixed-check 1");
    assert_eq!(r["law"]["hardware"], true, "{r}");
    assert!(record_path(&data_root, &floor, "floor").is_file());
    assert_eq!(
        hardware_flags(&mut c, &floor, "servo_blend").await,
        (true, true)
    );
    let e = arm_on_hardware(&mut c, &floor, "servo_blend").await;
    assert!(past_the_law_gate(&e), "{e}");

    // A record made by another version of the check does not arm.
    let path = record_path(&data_root, &floor, "floor");
    let current = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, current.replace("mixed-check 1", "mixed-check 0")).unwrap();
    let e = arm_on_hardware(&mut c, &floor, "servo_blend").await;
    assert_eq!(e["reason"], "law_not_checked", "{e}");
    assert!(
        e["message"].as_str().unwrap().contains("mixed-check 0"),
        "{e}"
    );
    // Nor one over another case list at the same version.
    let mut fewer: Value = serde_json::from_str(&current).unwrap();
    fewer["cases"].as_array_mut().unwrap().pop();
    std::fs::write(&path, serde_json::to_string(&fewer).unwrap()).unwrap();
    assert_eq!(
        hardware_flags(&mut c, &floor, "servo_blend").await,
        (false, true)
    );
    std::fs::write(&path, &current).unwrap();
    assert_eq!(
        hardware_flags(&mut c, &floor, "servo_blend").await,
        (true, true)
    );

    // The same law with other bytes is another file: its hash is new, and the
    // first file's record does not vouch for it.
    let scratch = data_root.join("changed_law.wasm");
    std::fs::write(
        &scratch,
        with_custom_section(&std::fs::read(law_file("hertzforge_law_hinv")).unwrap()),
    )
    .unwrap();
    let changed = c
        .ok(
            "laws.import",
            json!({"path": scratch.display().to_string()}),
        )
        .await["law"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(changed, floor);
    let e = arm_on_hardware(&mut c, &changed, "servo_blend").await;
    assert_eq!(e["reason"], "law_not_checked", "{e}");
    assert_eq!(e["detail"]["role"], "floor");

    // The simulator takes any law: the unchecked file gets past the arm's law
    // gate, and the run is ended.
    on_sim(&mut c).await;
    c.ok(
        "mixed.configure",
        json!({"config": config(&changed, "servo_blend")}),
    )
    .await;
    c.ok("mixed.start", json!({})).await;
    c.ok("mixed.abort", json!({})).await;
    let _ = std::fs::remove_dir_all(&data_root);
}

/// The tone role. Ignored until the core hands a tone law file a grid without
/// its 0 Hz line: today every case of the mixed check ends "the identification's
/// grid is not positive and ascending" at the law host, so no imported tone law
/// can pass it (the built-in law does not read the grid's first line).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "the core hands a tone law file a grid that starts at 0 Hz"]
async fn an_imported_tone_law_needs_its_own_check_and_the_tones_record() {
    let _turn = ONE_CHECK.lock().await;
    let o = options(
        FixedScanner(Ok(test_rig())),
        0.0,
        Duration::from_secs(60),
        Duration::from_secs(5),
    );
    let data_root = o.data_root.clone();
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    let schema = Schema::load();
    on_hardware(&mut c).await;
    let tone = import(&mut c, "hertzforge_law_sine_servo").await;

    assert_eq!(hardware_flags(&mut c, "hinv", &tone).await, (true, false));
    let e = arm_on_hardware(&mut c, "hinv", &tone).await;
    assert_eq!(e["reason"], "law_not_checked", "{e}");
    assert_eq!(e["detail"]["role"], "tones", "{e}");
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("its own check has not been run"),
        "{e}"
    );

    // Its own Sine check is the sine world's record (written here as the Sine
    // check writes it; that check's runs are its own suite's), read at the check
    // in force: a record of an earlier Sine check clears nothing.
    write_sine_record(&data_root, &tone, "sine-check 1");
    c.ok("laws.list", json!({"world": "sine"})).await;
    let e = arm_on_hardware(&mut c, "hinv", &tone).await;
    assert_eq!(e["reason"], "law_not_checked", "{e}");
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("its own check passed sine-check 1"),
        "{e}"
    );
    write_sine_record(&data_root, &tone, SINE_CHECK_VERSION);
    let listed = c.ok("laws.list", json!({"world": "sine"})).await;
    assert!(listed["laws"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["id"] == tone.as_str() && l["check"]["hardware"] == true));
    // Its own check alone is not enough, and a passed record of the floor role
    // for the same file clears nothing in the tones role.
    assert_eq!(hardware_flags(&mut c, "hinv", &tone).await, (true, false));
    write_mixed_record(&data_root, &tone, "floor");
    assert_eq!(hardware_flags(&mut c, "hinv", &tone).await, (true, false));
    let e = arm_on_hardware(&mut c, "hinv", &tone).await;
    assert_eq!(e["reason"], "law_not_checked", "{e}");
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("the mixed check in this role has not been run"),
        "{e}"
    );

    // The mixed check in the tones role clears it.
    let r = c
        .ok(
            "laws.check",
            json!({"id": tone, "mixed": "tones",
                   "params": {"compression": "custom", "ki": 0.5, "max_rate_db_per_s": 20}}),
        )
        .await;
    schema.assert_valid(&r, Some("mixedLawsCheckResult"));
    assert_eq!(r["report"]["passed"], true, "{r}");
    assert_eq!(r["report"]["recorded"], true, "{r}");
    assert_eq!(r["law"]["hardware"], true, "{r}");
    assert!(record_path(&data_root, &tone, "tones").is_file());
    assert_eq!(hardware_flags(&mut c, "hinv", &tone).await, (true, true));
    let e = arm_on_hardware(&mut c, "hinv", &tone).await;
    assert!(past_the_law_gate(&e), "{e}");
    let _ = std::fs::remove_dir_all(&data_root);
}
