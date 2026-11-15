//! A restored channel table (schema/PROTOCOL.md, "Restored Tables"): the table the
//! service keeps across restarts, and one taken from a project or a file, is shown
//! as restored and refused at every module's arm until the operator applies it.
//!
//! Each module's test is falsified by taking `inner.table.arm_gate()?` out of that
//! module's arm path in `src/server*.rs`: its `..._arm_refuses_a_restored_table...`
//! test fails with "<verb> was not refused".

#[path = "support/mimo_runs.rs"]
mod mimo_runs;
mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::{Service, ServiceOptions};
use sds_control::station::FixedScanner;
use sds_control::table_keep;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use support::{options, serve, Client};

fn opts() -> ServiceOptions {
    options(
        FixedScanner(Err(StationBuildError::NiDriverUnavailable(
            "not installed on this test host".into(),
        ))),
        0.0,
        Duration::from_secs(60),
        Duration::from_secs(5),
    )
}

fn row(index: u32, role: &str, description: &str) -> Value {
    let drive = role == "drive";
    json!({
        "index": index, "enabled": true, "description": description, "role": role,
        "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": if drive { "V" } else { "g" },
        "sensitivity_mv_per_eu": if drive { 1000 } else { 100 },
        "range_max": if drive { 10 } else { 100 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

fn three() -> Vec<Value> {
    vec![
        row(1, "control", "Control"),
        row(2, "monitor", "Monitor"),
        row(3, "drive", "Drive"),
    ]
}

async fn claim(c: &mut Client) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
}

/// Set `rows` as a table taken from a file; answers the revision it is at.
async fn restore(c: &mut Client, rows: Vec<Value>) -> u64 {
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    let r = c
        .ok(
            "channels.set",
            json!({"revision": rev, "channels": rows,
                   "source": {"kind": "file", "name": "bench.csv"}}),
        )
        .await;
    r["revision"].as_u64().unwrap()
}

/// `verb` is refused `table_not_applied`, naming the file; after the operator
/// applies the table at `rev` it is not refused for the table.
async fn gate(c: &mut Client, verb: &str, rev: u64) {
    let e = c.refused(verb, json!({})).await;
    assert_eq!(e["reason"], "table_not_applied", "{verb}: {e}");
    assert_eq!(e["code"], "refused", "{verb}: {e}");
    let words = e["message"].as_str().unwrap();
    assert!(
        words.contains("the file \"bench.csv\"") && words.contains(" UTC "),
        "{verb}: the refusal names the source and the time: {words}"
    );
    assert_eq!(e["detail"]["restored"]["source"], "file");
    c.ok("channels.apply", json!({"revision": rev})).await;
    let r = c.call(verb, json!({})).await;
    assert!(
        r.get("error").is_none(),
        "{verb}: an applied table was refused: {}",
        r["error"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn random_arm_refuses_a_restored_table_until_it_is_applied() {
    let addr = serve(Service::with_options(opts())).await;
    let mut c = Client::connect(addr).await;
    claim(&mut c).await;
    let rev = restore(&mut c, three()).await;
    c.ok(
        "random.configure",
        json!({"config": support::sim_config("auto", json!([{"db": 0}]), 1.0)}),
    )
    .await;
    gate(&mut c, "random.arm", rev).await;
    c.ok("random.abort", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_random_start_refuses_a_restored_table_until_it_is_applied() {
    let addr = serve(Service::with_options(opts())).await;
    let mut c = Client::connect(addr).await;
    claim(&mut c).await;
    let rows: Vec<Value> = mimo_runs::four_by_four().as_array().unwrap().clone();
    let rev = restore(&mut c, rows).await;
    c.ok(
        "mimo_random.setup",
        json!({"setup": mimo_runs::setup(30.0)}),
    )
    .await;
    gate(&mut c, "mimo_random.start", rev).await;
    c.ok("mimo_random.abort", json!({})).await;
}

fn shock_config() -> Value {
    json!({
        "title": "Gate", "pulse": {"shape": "half_sine", "amplitude_g": 1, "duration_ms": 11},
        "acquisition": {"fs_hz": 2048,
                        "srs": {"f_lo_hz": 20, "f_hi_hz": 800, "bands_per_octave": 3,
                                "band_tol_db": 3}},
        "schedule": {"mode": "auto", "levels_db": [-6, 0], "pulses": [1, 1], "advance": "count"},
        "spacing": {"cooling_s": 0.05, "settling_s": 0, "compute_s": 0},
        "identification": {"duration_s": 0.5, "nfft": 512, "band_hz": [4, 800], "review": false},
        "control": {"band_hz": [4, 800]},
        "limits": {"drive_vpk_ceiling": 2, "drive_vpk_abort": 3}
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn classical_shock_arm_refuses_a_restored_table_until_it_is_applied() {
    let addr = serve(Service::with_options(opts())).await;
    let mut c = Client::connect(addr).await;
    claim(&mut c).await;
    let rev = restore(&mut c, three()).await;
    c.ok("shock.configure", json!({"config": shock_config()}))
        .await;
    gate(&mut c, "shock.arm", rev).await;
    c.ok("shock.abort", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn srs_arm_refuses_a_restored_table_until_it_is_applied() {
    let addr = serve(Service::with_options(opts())).await;
    let mut c = Client::connect(addr).await;
    claim(&mut c).await;
    let rev = restore(&mut c, three()).await;
    c.ok(
        "srs.configure",
        json!({"config": {
            "title": "Gate", "target": {"kind": "flat", "accel_g": 2},
            "bands": {"f_lo_hz": 200, "f_hi_hz": 800, "bands_per_octave": 3, "q": 10},
            "acquisition": {"fs_hz": 2048, "ring_out_s": 0.02},
            "schedule": {"mode": "auto", "levels_db": [-6, 0], "pulses": [1, 1],
                         "advance": "count"},
            "spacing": {"cooling_s": 0.05, "settling_s": 0, "compute_s": 0},
            "identification": {"nfft": 256, "review": false},
            "limits": {"drive_vpk_ceiling": 5, "drive_vpk_abort": 8}
        }}),
    )
    .await;
    gate(&mut c, "srs.arm", rev).await;
    c.ok("srs.abort", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_arm_refuses_a_restored_table_until_it_is_applied() {
    let addr = serve(Service::with_options(opts())).await;
    let mut c = Client::connect(addr).await;
    claim(&mut c).await;
    let mut rows: Vec<Value> = (1..=4).map(|i| row(i, "control", "Control")).collect();
    rows.extend((5..=8).map(|i| row(i, "drive", "Drive")));
    for r in rows.iter_mut().take(4) {
        r["sensitivity_mv_per_eu"] = json!(1000);
        r["range_max"] = json!(1000);
    }
    let rev = restore(&mut c, rows).await;
    c.ok(
        "mimo_shock.configure",
        json!({"config": {
            "title": "Gate",
            "channels": {
                "controls": [{"channel": 1}, {"channel": 2}, {"channel": 3}, {"channel": 4}],
                "drives": (5..=8).map(|i| json!({"channel": i, "ceiling_vpk": 6, "abort_vpk": 9}))
                    .collect::<Vec<_>>(),
            },
            "target": {"kind": "waveform", "pulses": (0..4).map(|_| json!({
                "shape": "half_sine", "amplitude_g": 20, "duration_ms": 11})).collect::<Vec<_>>()},
            "acquisition": {"fs_hz": 12800, "record_len": 8192},
            "schedule": {"mode": "manual", "levels_db": [-12, -6, 0]},
            "spacing": {"cooling_s": 0.5, "settling_s": 0, "compute_s": 0},
            "identification": {"drive_vrms": 1, "frames": 40, "review": true},
        }}),
    )
    .await;
    gate(&mut c, "mimo_shock.arm", rev).await;
    c.ok("mimo_shock.abort", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sine_arm_refuses_a_restored_table_until_it_is_applied() {
    let addr = serve(Service::with_options(opts())).await;
    let mut c = Client::connect(addr).await;
    claim(&mut c).await;
    let rev = restore(&mut c, three()).await;
    c.ok(
        "sine.configure",
        json!({"config": {
            "title": "Gate",
            "profile": {"rows": [{"freq_hz": 250, "quantity": "accel_g", "value": 0.2},
                                 {"freq_hz": 350, "quantity": "accel_g", "value": 0.4}],
                        "alarm_db": [1.5, 1.5], "abort_db": [3, 3]},
            "test": {"kind": "sweep", "band_hz": [300, 320],
                     "rate": {"law": "log", "oct_per_min": 3}, "start_hz": 300,
                     "direction": "up", "end": {"sweeps": 2}},
            "acquisition": {"fs_hz": 4096, "tick_ms": 100},
            "schedule": {"mode": "auto", "levels_db": [-6, 0], "per_level": {"sweeps": 1},
                         "full_level": {"sweeps": 1}},
            "identification": {"review": false},
            "limits": {"drive_vpk_ceiling": 5, "drive_vpk_abort": 8}
        }}),
    )
    .await;
    gate(&mut c, "sine.arm", rev).await;
    c.ok("sine.abort", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mixed_arm_refuses_a_restored_table_until_it_is_applied() {
    let addr = serve(Service::with_options(opts())).await;
    let mut c = Client::connect(addr).await;
    claim(&mut c).await;
    let rev = restore(&mut c, three()).await;
    c.ok(
        "mixed.configure",
        json!({"config": {
            "title": "Gate",
            "parts": [
                {"id": "floor", "kind": "floor",
                 "breakpoints": [{"freq_hz": 100, "psd_g2_hz": 0.0005},
                                 {"freq_hz": 500, "slope_db_oct": 0}],
                 "alarm_db": [3, 3], "abort_db": [6, 6]},
                {"id": "t1", "name": "Blade", "kind": "tone",
                 "schedule": {"kind": "fixed", "freq_hz": 200}, "amplitude_g": 0.5}
            ],
            "exclusion_hz": 32,
            "acquisition": {"fs_hz": 4096, "n_pts": 256, "n_avg": 8, "tick_ms": 100},
            "schedule": {"mode": "auto", "levels": [{"db": -6, "dwell_s": 1}, {"db": 0}],
                         "full_level_s": 1},
            "control": {"pretest": {"averages": 2}, "review_after_sysid": false},
            "limits": {"drive_vpk_ceiling": 5, "drive_vpk_abort": 8}
        }}),
    )
    .await;
    gate(&mut c, "mixed.arm", rev).await;
    c.ok("mixed.abort", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn twr_arm_refuses_a_restored_table_until_it_is_applied() {
    let addr = serve(Service::with_options(opts())).await;
    let mut c = Client::connect(addr).await;
    claim(&mut c).await;
    let rev = restore(&mut c, three()).await;
    let fs = 2048.0;
    let text: String = (0..160)
        .map(|i| {
            let t = i as f64 / fs;
            let v = 0.4 * (-t / 0.03f64).exp() * (2.0 * std::f64::consts::PI * 150.0 * t).sin();
            format!("{v}\n")
        })
        .collect();
    let sha = c
        .ok(
            "twr.reference.import",
            json!({"text": text, "fs_hz": fs, "name": "field"}),
        )
        .await["reference"]["sha256"]
        .clone();
    c.ok(
        "twr.configure",
        json!({"config": {
            "title": "Gate", "reference": {"sha256": sha}, "pads": {"pre": 32, "post": 64},
            "identification": {"drive_vrms": 0.1, "duration_s": 0.5, "nfft": 256,
                               "band_hz": [20, 800], "review": false},
            "schedule": {"mode": "auto", "levels_db": [-6, 0], "gate_pct": 5.5},
            "run": {"schedule": "once"},
            "limits": {"drive_vpk_ceiling": 5}
        }}),
    )
    .await;
    gate(&mut c, "twr.arm", rev).await;
    c.ok("twr.abort", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explorer_start_refuses_a_restored_table_until_it_is_applied() {
    let mut o = opts();
    o.sim_speed = 1.0;
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    claim(&mut c).await;
    let rev = restore(
        &mut c,
        vec![
            row(1, "control", "Control"),
            row(2, "drive", "Drive"),
            row(3, "monitor", "Fixture"),
        ],
    )
    .await;
    c.ok(
        "explorer.configure",
        json!({"config": {
            "title": "Gate", "acquisition": {"fs_hz": 2048.0},
            "band_hz": [20, 800], "start_hz": 200, "max_drive_vpk": 0.5,
            "slew": {"freq_oct_per_s": 1, "drive_db_per_s": 200},
            "display": {"nfft": 1024}
        }}),
    )
    .await;
    gate(&mut c, "explorer.start", rev).await;
    c.ok("explorer.abort", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recorder_start_refuses_a_restored_table_until_it_is_applied() {
    let mut o = opts();
    o.sim_speed = 1.0;
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    claim(&mut c).await;
    let rev = restore(
        &mut c,
        vec![row(1, "control", "Markers"), row(2, "monitor", "Tone")],
    )
    .await;
    c.ok(
        "recorder.configure",
        json!({"config": {
            "title": "Gate", "acquisition": {"fs_hz": 1000},
            "trigger": {"mode": "manual"},
            "capture": {"run_name": "gate"}
        }}),
    )
    .await;
    gate(&mut c, "recorder.start", rev).await;
    c.ok("recorder.stop", json!({})).await;
}

// ---- the kept table ---------------------------------------------------------------

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sds-control-kept-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn keeping(root: &Path) -> ServiceOptions {
    let mut o = opts();
    o.data_root = root.to_path_buf();
    o.capture_root = Some(root.join("runs"));
    o.keep_table = true;
    o
}

/// Wait until the kept file holds `revision`.
fn wait_kept(root: &Path, revision: u64) -> Value {
    let t0 = Instant::now();
    loop {
        if let Ok(b) = std::fs::read(table_keep::path(root)) {
            if let Ok(v) = serde_json::from_slice::<Value>(&b) {
                if v["table"]["revision"] == revision {
                    return v;
                }
            }
        }
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "the table was not kept"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A restart keeps the table, every column of it, and shows it as restored.
///
/// Falsified by not calling `inner.table.keep(now)` in `channels_set`: "the table
/// was not kept".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kept_table_a_restart_restores_it_whole_and_unapplied() {
    let root = scratch("restart");
    let mut rows = three();
    rows[0]["node"] = json!(12);
    rows[0]["direction"] = json!("X+");
    rows[0]["serial_number"] = json!("SN-1");
    rows[0]["extra"] = json!({"Cable": "C7"});
    rows[1]["sensitivity_mv_per_eu"] = json!(9.87);
    rows[1]["polarity"] = json!("negative");
    rows[1]["abort"]["accel_g"] = json!(30);
    let first = {
        let addr = serve(Service::with_options(keeping(&root))).await;
        let mut c = Client::connect(addr).await;
        claim(&mut c).await;
        let r = c
            .ok("channels.set", json!({"revision": 1, "channels": rows}))
            .await;
        assert_eq!(r["revision"], 2);
        let got = c.ok("channels.get", json!({})).await;
        assert!(got.get("restored").is_none(), "a typed table is applied");
        wait_kept(&root, 2);
        got
    };
    // The next start.
    let addr = serve(Service::with_options(keeping(&root))).await;
    let mut c = Client::connect(addr).await;
    let got = c.ok("channels.get", json!({})).await;
    assert_eq!(
        got["channels"], first["channels"],
        "the restart lost a column"
    );
    assert_eq!(got["revision"], 2);
    assert_eq!(got["restored"]["source"], "kept");
    assert_eq!(got["restored"]["name"], table_keep::FILE);
    support::schema::Schema::load().assert_valid(&got, Some("channelTable"));
    claim(&mut c).await;
    c.ok(
        "random.configure",
        json!({"config": support::sim_config("auto", json!([{"db": 0}]), 1.0)}),
    )
    .await;
    let e = c.refused("random.start", json!({})).await;
    assert_eq!(e["reason"], "table_not_applied", "{e}");
    assert!(e["message"]
        .as_str()
        .unwrap()
        .contains("the table this service kept"));
    let _ = std::fs::remove_dir_all(&root);
}

/// A damaged kept file is not used: the service starts with its starting table and
/// says why.
///
/// Falsified by skipping the SHA-256 comparison in `table_keep::read`: the damaged
/// table is restored and "a damaged file was used" fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kept_table_a_damaged_file_is_refused_and_named() {
    let root = scratch("damaged");
    std::fs::create_dir_all(&root).unwrap();
    let mut v: Value = serde_json::from_slice(&table_keep::contents(
        &sds_control::table::Table::new().table,
        6,
        1.0,
    ))
    .unwrap();
    v["table"]["rows"][0]["sensitivity_mv_per_eu"] = json!(1.0);
    std::fs::write(table_keep::path(&root), serde_json::to_vec(&v).unwrap()).unwrap();
    let addr = serve(Service::with_options(keeping(&root))).await;
    let mut c = Client::connect(addr).await;
    let got = c.ok("channels.get", json!({})).await;
    assert_eq!(got["revision"], 1, "a damaged file was used");
    assert!(got.get("restored").is_none(), "a damaged file was used");
    let why = got["kept_problem"].as_str().unwrap();
    assert!(why.contains("damaged"), "{why}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A write that fails is reported and stops nothing: the table is in use and the
/// run arms and runs to its end.
///
/// Falsified by making `channels_set` refuse when the keeper reports a problem:
/// the second `channels.set` fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kept_table_a_failed_write_is_reported_and_the_run_still_runs() {
    let base = scratch("unwritable");
    std::fs::create_dir_all(&base).unwrap();
    // The data root is a file, so nothing can be written under it.
    let root = base.join("not-a-folder");
    std::fs::write(&root, b"a file").unwrap();
    let mut o = keeping(&root);
    o.capture_root = Some(base.join("runs"));
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    claim(&mut c).await;
    c.ok("channels.set", json!({"revision": 1, "channels": three()}))
        .await;
    let t0 = Instant::now();
    let why = loop {
        let got = c.ok("channels.get", json!({})).await;
        if let Some(w) = got["kept_problem"].as_str() {
            break w.to_owned();
        }
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "the failed write was not reported"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(why.contains("could not be kept"), "{why}");
    c.ok("channels.set", json!({"revision": 2, "channels": three()}))
        .await;
    c.ok(
        "random.configure",
        json!({"config": support::sim_config("auto", json!([{"db": 0}]), 0.5)}),
    )
    .await;
    c.ok("random.start", json!({})).await;
    let end = c.wait_state("finished", Duration::from_secs(120)).await;
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    let _ = std::fs::remove_dir_all(&base);
}

/// The source a client names, the apply verb's refusals, and the table a client
/// sends with no source arriving applied.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restored_table_source_and_apply_are_checked_by_name() {
    let addr = serve(Service::with_options(opts())).await;
    let mut c = Client::connect(addr).await;
    claim(&mut c).await;
    for bad in [
        json!({"kind": "kept", "name": "x"}),
        json!({"kind": "project"}),
        json!({"kind": "file", "name": "  "}),
        json!("bench.csv"),
    ] {
        let e = c
            .refused(
                "channels.set",
                json!({"revision": 1, "channels": three(), "source": bad}),
            )
            .await;
        assert_eq!(e["reason"], "bad_params", "{bad}: {e}");
    }
    let e = c.refused("channels.apply", json!({"revision": 1})).await;
    assert_eq!(e["reason"], "nothing_to_apply", "{e}");
    let rev = restore(&mut c, three()).await;
    let e = c
        .refused("channels.apply", json!({"revision": rev - 1}))
        .await;
    assert_eq!(e["reason"], "stale_revision", "{e}");
    // Edited in this session, the same table is applied as it is set.
    let r = c
        .ok(
            "channels.set",
            json!({"revision": rev, "channels": three()}),
        )
        .await;
    let got = c.ok("channels.get", json!({})).await;
    assert_eq!(got["revision"], r["revision"]);
    assert!(
        got.get("restored").is_none(),
        "an edited table stayed restored"
    );
    // An observer cannot apply.
    let rev = restore(&mut c, three()).await;
    let mut o = Client::connect(addr).await;
    let e = o.refused("channels.apply", json!({"revision": rev})).await;
    assert_eq!(e["reason"], "not_controller", "{e}");
}

/// The control leg of the round trip project to service to project: the rows the
/// analysis side's `project.channel_table` answers for the golden table (every
/// optional member on every row; `tests/service/test_project_channel_table.py`
/// reads the same file) come back from the service unchanged when taken from a
/// project, when applied, and after a restart.
///
/// Falsified by leaving `extra` out of `row_json`: the first comparison fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn project_table_rows_survive_restore_apply_and_a_restart() {
    let golden: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../schema/golden/request_tables_save_channels.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let rows = golden["params"]["channels"].clone();
    let root = scratch("project");
    let rev = {
        let addr = serve(Service::with_options(keeping(&root))).await;
        let mut c = Client::connect(addr).await;
        claim(&mut c).await;
        let r = c
            .ok(
                "channels.set",
                json!({"revision": 1, "channels": rows,
                       "source": {"kind": "project", "name": "Bench table"}}),
            )
            .await;
        let got = c.ok("channels.get", json!({})).await;
        assert_eq!(got["channels"], rows, "taken from the project");
        assert_eq!(got["restored"]["source"], "project");
        assert_eq!(got["restored"]["name"], "Bench table");
        c.ok("channels.apply", json!({"revision": r["revision"]}))
            .await;
        let got = c.ok("channels.get", json!({})).await;
        assert_eq!(got["channels"], rows, "applied");
        r["revision"].as_u64().unwrap()
    };
    wait_kept(&root, rev);
    let addr = serve(Service::with_options(keeping(&root))).await;
    let mut c = Client::connect(addr).await;
    let got = c.ok("channels.get", json!({})).await;
    assert_eq!(got["channels"], rows, "after a restart");
    let _ = std::fs::remove_dir_all(&root);
}

/// Decided by the project's owner: a negative sensitivity is refused, polarity being
/// the row's own column. Every way a table reaches the service refuses it with the
/// row, the column and the value in the words: typed in this session, taken from a
/// project, opened from a file, and the kept table read at start (which is not used,
/// and `kept_problem` says why). Nothing is written, so nothing can be armed on.
///
/// Falsified by dropping the `NegativeSensitivity` check in the channel table's
/// `validate_report`: the first `channels.set` is not refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn negative_sensitivity_is_refused_from_every_source_and_at_start() {
    let mut rows = three();
    rows[1]["sensitivity_mv_per_eu"] = json!(-100);
    let addr = serve(Service::with_options(opts())).await;
    let mut c = Client::connect(addr).await;
    claim(&mut c).await;
    for source in [
        Value::Null,
        json!({"kind": "project", "name": "Bracket table"}),
        json!({"kind": "file", "name": "bench.csv"}),
    ] {
        let e = c
            .refused(
                "channels.set",
                json!({"revision": 1, "channels": rows.clone(), "source": source}),
            )
            .await;
        assert_eq!(e["reason"], "table_invalid", "{source}: {e}");
        let item = &e["detail"]["errors"][0];
        assert_eq!(item["reason"], "negative_sensitivity", "{source}: {e}");
        assert_eq!(item["channel"], 2, "{source}: {e}");
        let words = item["message"].as_str().unwrap();
        assert!(
            words.contains("channel 2")
                && words.contains("sensitivity_mv_per_eu")
                && words.contains("-100"),
            "{source}: {words}"
        );
    }
    let got = c.ok("channels.get", json!({})).await;
    assert_eq!(got["revision"], 1, "a refused table was written");

    let root = scratch("negative");
    std::fs::create_dir_all(&root).unwrap();
    let mut table = sds_control::table::Table::new().table;
    table.channels[0].sensitivity_mv_per_eu = -100.0;
    std::fs::write(
        table_keep::path(&root),
        table_keep::contents(&table, 6, 1.0),
    )
    .unwrap();
    let addr = serve(Service::with_options(keeping(&root))).await;
    let mut c = Client::connect(addr).await;
    let got = c.ok("channels.get", json!({})).await;
    assert_eq!(got["revision"], 1, "a kept negative sensitivity was used");
    assert!(got.get("restored").is_none(), "{got}");
    let why = got["kept_problem"].as_str().unwrap();
    assert!(
        why.contains("negative") && why.contains("-100"),
        "the kept problem names the value: {why}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
