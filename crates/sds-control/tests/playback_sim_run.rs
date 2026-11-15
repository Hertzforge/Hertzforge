//! Playback end to end through the wire on the simulator: one, two and four drives (the
//! four on the four-by-four modal plant), a long file streamed through the service, the
//! refusals the service makes before and at start, a stop and an abort, and the run's
//! record checked item by item.

mod support;

use hertzforge_playback::file::write_folder;
use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use support::{options, serve, Client};

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

fn row(index: u32, role: &str, accel_limit: Option<f64>) -> Value {
    let drive = role == "drive";
    json!({
        "index": index, "enabled": true, "description": format!("{role} {index}"),
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": if drive { "V" } else { "g" },
        "sensitivity_mv_per_eu": if drive { 1000 } else { 100 },
        "range_max": if drive { 10 } else { 100 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": accel_limit, "velocity_ips": null, "displacement_mil": null}
    })
}

fn temp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "sds-playback-{}-{}-{name}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Zero acceleration, velocity and displacement at both ends (the displacement
/// `sin²(πt/T) sin(2πft)` differentiated twice), scaled to `peak` volts.
pub fn burst(fs: f64, seconds: f64, f_hz: f64, peak: f64) -> Vec<f32> {
    let n = (fs * seconds).round() as usize;
    let t_len = n as f64 / fs;
    let w = 2.0 * std::f64::consts::PI * (f_hz * t_len).round().max(1.0) / t_len;
    let p = std::f64::consts::PI / t_len;
    let a: Vec<f64> = (0..n)
        .map(|k| {
            let t = k as f64 / fs;
            let (s, c) = (w * t).sin_cos();
            2.0 * p * p * (2.0 * p * t).cos() * s + 2.0 * p * (2.0 * p * t).sin() * w * c
                - (p * t).sin().powi(2) * w * w * s
        })
        .collect();
    let m = a.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    a.iter().map(|x| (x * peak / m) as f32).collect()
}

/// A drive folder of `chans` at `fs`: its path and SHA-256.
fn drive(dir: &Path, fs: f64, chans: &[Vec<f32>]) -> (String, String) {
    let folder = dir.join("drive");
    let names: Vec<String> = (0..chans.len()).map(|k| format!("drive {k}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut s = Vec::with_capacity(chans[0].len() * chans.len());
    for i in 0..chans[0].len() {
        for c in chans {
            s.push(c[i]);
        }
    }
    let mut extra = serde_json::Map::new();
    extra.insert(
        "source".into(),
        json!({"path": "made in the test", "format": "test"}),
    );
    extra.insert("tool".into(), json!("playback_sim_run"));
    let sha = write_folder(&folder, fs, &names, &s, extra).unwrap();
    (folder.display().to_string(), sha)
}

async fn rig(inputs: u32, drives: u32, speed: f64, limit: Option<f64>) -> Client {
    let o = options(
        no_driver(),
        speed,
        Duration::from_secs(60),
        Duration::from_secs(5),
    );
    let svc = Service::with_options(o);
    svc.start_background();
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let mut rows: Vec<Value> = (1..=inputs).map(|k| row(k, "control", limit)).collect();
    rows.extend((1..=drives).map(|k| row(inputs + k, "drive", None)));
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok("channels.set", json!({"revision": rev, "channels": rows}))
        .await;
    c
}

fn config(folder: &str, sha: &str, map: Vec<u32>, level_db: f64) -> Value {
    json!({
        "title": "Simulated Playback",
        "drive": {"folder": folder, "sha256": sha},
        "map": map, "level_db": level_db, "ceiling_v": 1.0,
        "pre_roll_s": 0.25, "post_roll_s": 0.5,
        "operator": "A. Operator", "article": "Bracket, SN 0042",
        "specification": {"document": "Test procedure", "clause": "4.2"},
    })
}

async fn finished(c: &mut Client, wait: Duration) -> Value {
    let t0 = Instant::now();
    loop {
        let s = c.ok("playback.state", json!({})).await;
        if s["state"] == "finished" {
            return s;
        }
        assert!(t0.elapsed() < wait, "{s}");
        c.pump(Duration::from_millis(20)).await;
    }
}

async fn record_done(c: &mut Client, folder: &Path) {
    let t0 = Instant::now();
    while !(folder.join("end.json").is_file() && read(&folder.join("run.json"))["complete"] == true)
    {
        assert!(t0.elapsed() < Duration::from_secs(30), "{folder:?}");
        c.pump(Duration::from_millis(20)).await;
    }
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn f32s(path: &Path) -> Vec<f32> {
    std::fs::read(path)
        .unwrap()
        .chunks(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn playback_runs_end_to_end_on_one_two_and_four_drives() {
    let fs = 4096.0;
    for (inputs, drives) in [(1u32, 1u32), (2, 2), (4, 4)] {
        let dir = temp(&format!("n{drives}"));
        let freqs = [60.0, 140.0, 260.0, 410.0];
        let chans: Vec<Vec<f32>> = (0..drives as usize)
            .map(|k| burst(fs, 1.0, freqs[k], 0.4))
            .collect();
        let (folder, sha) = drive(&dir, fs, &chans);
        let mut c = rig(inputs, drives, 8.0, None).await;
        let map: Vec<u32> = (1..=drives).map(|k| inputs + k).collect();
        let r = c
            .ok(
                "playback.configure",
                json!({"config": config(&folder, &sha, map.clone(), -6.0)}),
            )
            .await;
        assert_eq!(r["check"]["pass"], true, "{}", r["check"]);
        assert_eq!(r["state"]["state"], "configured");
        assert_eq!(
            r["check"]["previews"].as_array().unwrap().len(),
            drives as usize
        );
        let s = c.ok("playback.start", json!({})).await;
        assert!(
            ["running", "finished"].contains(&s["state"].as_str().unwrap()),
            "{s}"
        );
        let end = finished(&mut c, Duration::from_secs(60)).await;
        assert_eq!(end["end"]["outcome"], "completed", "{end}");
        assert_eq!(end["end"]["played"], 4096);
        let rec = PathBuf::from(end["record"]["folder"].as_str().unwrap());
        record_done(&mut c, &rec).await;
        let run = read(&rec.join("run.json"));
        assert_eq!(run["test"], "playback");
        let d = read(&rec.join("streams/drives/stream.json"));
        assert_eq!(d["channels"].as_array().unwrap().len(), drives as usize);
        // every drive as played: each file channel on its own row at the level
        let played = f32s(&rec.join("streams/drives/telemetry.f32"));
        let w = drives as usize;
        let g = 10f64.powf(-6.0 / 20.0);
        for k in 0..w {
            let top = played
                .iter()
                .skip(k)
                .step_by(w)
                .fold(0f32, |m, x| m.max(x.abs()));
            assert!(
                (f64::from(top) - 0.4 * g).abs() < 1e-3,
                "drive {k} peaks at {top}"
            );
        }
        // every input recorded, and on the modal plant each answers
        let ins = f32s(&rec.join("streams/inputs/telemetry.f32"));
        let wi = inputs as usize;
        assert!(
            ins.len() / wi >= 4096 + 1024,
            "{} input frames",
            ins.len() / wi
        );
        for k in 0..wi {
            let top = ins
                .iter()
                .skip(k)
                .step_by(wi)
                .fold(0f32, |m, x| m.max(x.abs()));
            assert!(top > 1e-3, "input {k} peaks at {top}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_run_record_holds_every_item() {
    let fs = 4096.0;
    let dir = temp("record");
    let (folder, sha) = drive(
        &dir,
        fs,
        &[burst(fs, 1.0, 80.0, 0.5), burst(fs, 1.0, 190.0, 0.3)],
    );
    let mut c = rig(2, 2, 8.0, Some(80.0)).await;
    c.ok(
        "playback.configure",
        json!({"config": config(&folder, &sha, vec![4, 3], -3.0)}),
    )
    .await;
    c.ok("playback.start", json!({})).await;
    let end = finished(&mut c, Duration::from_secs(60)).await;
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    let rec = PathBuf::from(end["record"]["folder"].as_str().unwrap());
    record_done(&mut c, &rec).await;

    // the manifest: the test, the source, the simulator, the station, the grid, the
    // build, the times, the table's stamp, the streams and the results
    let run = read(&rec.join("run.json"));
    assert_eq!(run["schema"], "hertzforge-run/1");
    assert_eq!(run["test"], "playback");
    assert_eq!(run["source"], "SIM");
    assert_eq!(run["title"], "Simulated Playback");
    assert_eq!(run["operator"], "A. Operator");
    assert_eq!(run["article"], "Bracket, SN 0042");
    assert_eq!(run["specification"]["clause"], "4.2");
    assert!(run["sim"]["seed"].is_u64(), "{}", run["sim"]);
    assert_eq!(run["station"]["kind"], "sim");
    assert_eq!(run["grid"]["fs_hz"], 4096.0);
    assert!(run["build"]["version"].is_string());
    assert!(run["armed"]["unix"].as_f64().unwrap() > 0.0);
    assert!(run["start"].get("utc_offset_s").is_some());
    assert!(run["channel_table"]["sha256"].is_string());
    assert_eq!(run["complete"], true);
    let names = |k: &str| -> Vec<String> {
        run[k]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(names("streams"), vec!["inputs", "drives"]);
    let mut results = names("results");
    results.sort();
    assert_eq!(results, vec!["playback_check", "playback_outcome"]);

    // the table as used, every row
    let table = read(&rec.join("table.json"));
    assert_eq!(table["rows"].as_array().unwrap().len(), 4);
    assert_eq!(table["rows"][0]["abort"]["accel_g"], 80.0);

    // the setup as accepted: the configuration, the sidecar, the check, the map, the
    // level, the ceiling, the rolls, the stop, the aborts, open loop and no law
    let setup = read(&rec.join("setup.json"));
    assert_eq!(setup["config"]["drive"]["sha256"], sha);
    let r = &setup["resolved"];
    assert!(r["control"]
        .as_str()
        .unwrap()
        .starts_with("open loop, no control law"));
    assert_eq!(r["drive"]["sha256"], sha);
    assert_eq!(r["drive"]["sidecar"]["format"], "hertzforge-drive");
    assert_eq!(r["drive"]["sidecar"]["tool"], "playback_sim_run");
    assert_eq!(r["drive"]["channels"][0]["drive_row"], 4);
    assert_eq!(r["map"], json!([4, 3]));
    assert_eq!(r["level_db"], -3.0);
    assert_eq!(r["ceiling_v"], 1.0);
    assert_eq!(r["pre_roll_s"], 0.25);
    assert_eq!(r["post_roll_s"], 0.5);
    assert_eq!(r["stop"]["depth_db"], 40.0);
    assert_eq!(r["aborts"]["drive"]["ceiling_v"], 1.0);
    assert_eq!(r["aborts"]["channels"][0]["accel_g"], 80.0);
    assert_eq!(r["inputs"], json!([1, 2]));
    assert_eq!(r["drives"], json!([3, 4]));
    assert_eq!(setup["check"]["pass"], true);
    assert_eq!(setup["check"]["channels"].as_array().unwrap().len(), 2);
    assert!(
        setup["check"]["channels"][0]["velocity_v_s"]["peak"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert!(!rec.join("law").exists(), "Playback runs no control law");

    // the timeline: the check, armed, the rolls, playing, the outcome
    let log: Vec<Value> = std::fs::read_to_string(rec.join("log.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let events: Vec<String> = log
        .iter()
        .filter_map(|e| e["data"]["event"].as_str().map(str::to_owned))
        .collect();
    for want in ["check_passed", "armed", "pre_roll", "playing", "post_roll"] {
        assert!(events.iter().any(|e| e == want), "no {want} in {events:?}");
    }
    let outcome = log
        .iter()
        .find(|e| e["kind"] == "outcome")
        .expect("an outcome entry");
    assert_eq!(outcome["data"]["outcome"], "completed");

    // every input and every drive as played, with their units
    let si = read(&rec.join("streams/inputs/stream.json"));
    assert_eq!(si["kind"], "inputs");
    assert_eq!(si["channels"][0]["unit"], "g");
    let sd = read(&rec.join("streams/drives/stream.json"));
    assert_eq!(sd["kind"], "drives");
    assert_eq!(sd["channels"][0]["unit"], "V");
    assert_eq!(sd["complete"], true);
    assert!(sd["frames"].as_u64().unwrap() >= 4096);

    // the results
    let chk = read(&rec.join("results/playback_check.json"));
    assert_eq!(chk["kind"], "drive_check");
    assert_eq!(chk["report"]["pass"], true);
    let arrays: Vec<&str> = chk["arrays"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    for want in [
        "peak",
        "velocity_end",
        "displacement_end",
        "drive_min",
        "displacement_max",
    ] {
        assert!(arrays.contains(&want), "{arrays:?}");
    }
    assert!(chk["arrays"]
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a["unit"].is_string()));
    let out = read(&rec.join("results/playback_outcome.json"));
    assert_eq!(out["ended"], "completed");
    assert_eq!(out["samples"], 4096);
    let bin = std::fs::read(rec.join("results/playback_outcome.bin")).unwrap();
    assert_eq!(f64::from_le_bytes(bin[..8].try_into().unwrap()), 4096.0);

    // the end
    let e = read(&rec.join("end.json"));
    assert_eq!(e["end"]["outcome"], "completed");
    assert_eq!(e["played"]["samples"], 4096);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_long_file_streams_through_the_service() {
    // 60 s at 25.6 kHz on four drives, at forty times real time on the simulator: the
    // read-ahead keeps up and every sample plays. The streams are not kept (the record
    // says so), to spare the disk 200 MB.
    let fs = 25_600.0;
    let dir = temp("long");
    let chans: Vec<Vec<f32>> = [37.0, 91.0, 155.0, 233.0]
        .iter()
        .map(|f| burst(fs, 60.0, *f, 0.3))
        .collect();
    let n = chans[0].len() as u64;
    let (folder, sha) = drive(&dir, fs, &chans);
    drop(chans);
    let mut c = rig(4, 4, 40.0, None).await;
    let mut cfg = config(&folder, &sha, vec![5, 6, 7, 8], -6.0);
    cfg["record"] = json!({"streams": false});
    c.ok("playback.configure", json!({"config": cfg})).await;
    c.ok("playback.start", json!({})).await;
    let end = finished(&mut c, Duration::from_secs(240)).await;
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    assert_eq!(end["end"]["played"], n);
    let rec = PathBuf::from(end["record"]["folder"].as_str().unwrap());
    record_done(&mut c, &rec).await;
    assert_eq!(read(&rec.join("run.json"))["recording"]["streams"], "off");
    let log = std::fs::read_to_string(rec.join("log.jsonl")).unwrap();
    assert!(!log.contains("underrun"), "the read-ahead ran dry");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_service_refuses_what_its_checks_refuse() {
    let fs = 4096.0;
    let dir = temp("refuse");
    // a sine of whole cycles: its displacement walks away
    let sine: Vec<f32> = (0..4096)
        .map(|k| (0.5 * (2.0 * std::f64::consts::PI * 32.0 * k as f64 / fs).sin()) as f32)
        .collect();
    let (bad, bad_sha) = drive(&dir.join("bad"), fs, &[sine]);
    let (good, sha) = drive(&dir.join("good"), fs, &[burst(fs, 1.0, 80.0, 0.5)]);
    let mut c = rig(1, 1, 8.0, None).await;
    let e = c.refused("playback.start", json!({})).await;
    assert_eq!(e["reason"], "not_configured", "{e}");
    // ends not at zero: the dry run shows the report, the configure refuses it
    let d = c
        .ok(
            "playback.configure",
            json!({"config": config(&bad, &bad_sha, vec![2], 0.0), "dry_run": true}),
        )
        .await;
    assert_eq!(d["check"]["pass"], false);
    let reasons: Vec<&str> = d["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["reason"].as_str().unwrap())
        .collect();
    assert_eq!(reasons, vec!["ends_not_zero"]);
    assert!(d["errors"][0]["message"]
        .as_str()
        .unwrap()
        .contains("displacement"));
    let e = c
        .refused(
            "playback.configure",
            json!({"config": config(&bad, &bad_sha, vec![2], 0.0)}),
        )
        .await;
    assert_eq!(e["reason"], "config_invalid");
    assert_eq!(e["detail"]["check"]["pass"], false);
    // a pinned hash that is not the file's
    let e = c
        .refused(
            "playback.configure",
            json!({"config": config(&good, &"a".repeat(64), vec![2], 0.0)}),
        )
        .await;
    assert_eq!(
        e["detail"]["errors"][0]["reason"], "drive_pin_mismatch",
        "{e}"
    );
    // the peak over the ceiling, a missing and a repeated drive row
    let e = c
        .refused(
            "playback.configure",
            json!({"config": config(&good, &sha, vec![1], 3.0)}),
        )
        .await;
    let reasons: Vec<&str> = e["detail"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["reason"].as_str().unwrap())
        .collect();
    assert!(
        reasons.contains(&"out_of_range") && reasons.contains(&"map_missing_row"),
        "{reasons:?}"
    );
    // a file changed after its check is refused at start, and nothing is driven
    c.ok(
        "playback.configure",
        json!({"config": config(&good, &sha, vec![2], 0.0)}),
    )
    .await;
    let bin = PathBuf::from(&good).join("drive.bin");
    let mut bytes = std::fs::read(&bin).unwrap();
    bytes[400] ^= 0x10;
    std::fs::write(&bin, &bytes).unwrap();
    let e = c.refused("playback.start", json!({})).await;
    assert_eq!(e["reason"], "drive_changed", "{e}");
    let s = c.ok("playback.state", json!({})).await;
    assert_eq!(s["end"]["outcome"], "arm_refused");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_ramps_and_an_abort_zeroes() {
    let fs = 4096.0;
    let dir = temp("stopabort");
    let (folder, sha) = drive(&dir, fs, &[burst(fs, 20.0, 80.0, 0.5)]);
    for abort in [false, true] {
        let mut c = rig(1, 1, 4.0, None).await;
        c.ok(
            "playback.configure",
            json!({"config": config(&folder, &sha, vec![2], 0.0)}),
        )
        .await;
        c.ok("playback.start", json!({})).await;
        let t0 = Instant::now();
        loop {
            let s = c.ok("playback.state", json!({})).await;
            if s["phase"] == "playing" && s["progress"]["played_s"].as_f64().unwrap() > 3.0 {
                break;
            }
            assert!(t0.elapsed() < Duration::from_secs(30), "{s}");
            c.pump(Duration::from_millis(20)).await;
        }
        c.ok(
            if abort {
                "playback.abort"
            } else {
                "playback.stop"
            },
            json!({}),
        )
        .await;
        let end = finished(&mut c, Duration::from_secs(30)).await;
        let want = if abort { "aborted" } else { "stopped" };
        assert_eq!(end["end"]["outcome"], want, "{end}");
        assert_eq!(end["end"]["emergency_stop"], abort);
        let rec = PathBuf::from(end["record"]["folder"].as_str().unwrap());
        record_done(&mut c, &rec).await;
        // a stop's drive ramps to 0 V; an abort's ends where the emergency stop took it
        let played = f32s(&rec.join("streams/drives/telemetry.f32"));
        if !abort {
            assert_eq!(*played.last().unwrap(), 0.0);
        }
        let out = read(&rec.join("results/playback_outcome.json"));
        assert_eq!(out["ended"], if abort { "aborted" } else { "stopped" });
        let log = std::fs::read_to_string(rec.join("log.jsonl")).unwrap();
        assert!(
            log.contains(if abort { "\"abort\"" } else { "\"stop\"" }),
            "{log}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
