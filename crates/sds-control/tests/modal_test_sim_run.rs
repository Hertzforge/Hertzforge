//! The Modal Test end to end through the wire on the simulator: a three-point roving
//! hammer and a two-shaker burst random test, each against the simulated structure's
//! own FRF at its resonances; the refusals; impact mode writing no drive sample; the
//! shaker's stop ramp and the abort's zero.

mod support;

use hertzforge_modal_test::structure::{accelerance, Dof, MODES};
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

/// A measured row: `units`, a range, a DOF and its part in an FRF.
fn row(index: u32, units: &str, range: f64, node: u32, part: &str) -> Value {
    json!({
        "index": index, "enabled": true, "description": format!("{part} {index}"),
        "role": "monitor", "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": 10, "range_max": range,
        "polarity": "positive", "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null},
        "node": node, "direction": "Z+", "analysis_role": part,
    })
}

fn drive_row(index: u32) -> Value {
    json!({
        "index": index, "enabled": true, "description": format!("shaker {index}"),
        "role": "drive", "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": "V", "sensitivity_mv_per_eu": 1000, "range_max": 10, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

/// Row 1 the hammer at node 1, rows 2 and 3 responses at nodes 4 and 7; with
/// `shakers`, row 4 a second force at node 8 and rows 5 and 6 the shakers' drives.
fn rows(shakers: bool) -> Vec<Value> {
    let mut r = vec![
        row(1, "lbf", 100.0, 1, "reference"),
        row(2, "g", 50.0, 4, "response"),
        row(3, "g", 50.0, 7, "response"),
    ];
    if shakers {
        r.push(row(4, "lbf", 100.0, 8, "reference"));
        r.push(drive_row(5));
        r.push(drive_row(6));
    }
    r
}

async fn rig(shakers: bool, speed: f64) -> Client {
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
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": rows(shakers)}),
    )
    .await;
    c
}

fn impact_config() -> Value {
    json!({
        "title": "Roving Hammer", "operator": "A. Operator", "article": "Beam, SN 0007",
        "mode": "impact", "fs_hz": 1024, "block": 1024, "band": {"hi_hz": 250},
        "averages": 3,
        "roving": {"channels": [1], "points": {"start": 1, "count": 3}},
        "impact": {"holdoff_blocks": 0},
    })
}

fn shaker_config(source: &str) -> Value {
    json!({
        "title": "Two Shakers", "mode": "shaker", "fs_hz": 1024, "block": 1024,
        "band": {"hi_hz": 250}, "averages": 12,
        "shaker": {"drives": [{"row": 5, "level_vrms": 0.05}, {"row": 6, "level_vrms": 0.05}],
                   "source": source, "burst_pct": 40, "ramp_s": 0.5, "ceiling_v": 1.0},
    })
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn f64s(path: &Path) -> Vec<f64> {
    std::fs::read(path)
        .unwrap()
        .chunks(8)
        .map(|b| f64::from_le_bytes(b.try_into().unwrap()))
        .collect()
}

fn f32s(path: &Path) -> Vec<f32> {
    std::fs::read(path)
        .unwrap()
        .chunks(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}

async fn state(c: &mut Client) -> Value {
    c.ok("modal_test.state", json!({})).await
}

async fn until(c: &mut Client, wait: Duration, what: &str, f: impl Fn(&Value) -> bool) -> Value {
    let t0 = Instant::now();
    loop {
        let s = state(c).await;
        if f(&s) {
            return s;
        }
        assert!(t0.elapsed() < wait, "waiting for {what}: {s}");
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

/// The measured FRF of every record of `modal_frf`: `[record][resp][ref][line]`.
/// One record's FRF: `[resp][ref][line]` of (re, im).
type Frf = Vec<Vec<Vec<(f64, f64)>>>;

fn frf_records(folder: &Path) -> (Vec<Value>, Vec<Frf>, f64, f64) {
    let side = read(&folder.join("results/modal_frf.json"));
    let shape: Vec<usize> = side["arrays"][0]["shape"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as usize)
        .collect();
    let (nr, nf, nl) = (shape[0], shape[1], shape[2]);
    let v = f64s(&folder.join("results/modal_frf.bin"));
    let per = nr * nf * nl * 2;
    let recs = lines(&folder.join("results/modal_frf.jsonl"));
    assert_eq!(v.len(), per * recs.len());
    let out = v
        .chunks(per)
        .map(|c| {
            (0..nr)
                .map(|i| {
                    (0..nf)
                        .map(|j| {
                            (0..nl)
                                .map(|l| {
                                    let k = ((i * nf + j) * nl + l) * 2;
                                    (c[k], c[k + 1])
                                })
                                .collect()
                        })
                        .collect()
                })
                .collect()
        })
        .collect();
    let start = side["abscissa"]["start"].as_f64().unwrap();
    let step = side["abscissa"]["step"].as_f64().unwrap();
    (recs, out, start, step)
}

/// The largest relative error of `h` against the structure's FRF from `b` to `a` at
/// the resonances.
fn worst_at_resonances(h: &[(f64, f64)], a: &Dof, b: &Dof, start: f64, step: f64) -> f64 {
    MODES
        .iter()
        .map(|(f, _)| {
            let l = ((f - start) / step).round() as usize;
            let want = accelerance(a, b, start + l as f64 * step);
            (h[l].0 - want.re).hypot(h[l].1 - want.im) / want.norm()
        })
        .fold(0.0, f64::max)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_three_point_roving_hammer_matches_the_structure() {
    let mut c = rig(false, 8.0).await;
    let r = c
        .ok("modal_test.configure", json!({"config": impact_config()}))
        .await;
    assert_eq!(r["state"]["state"], "configured", "{r}");
    assert_eq!(r["state"]["source"], "SIM");
    let s = c.ok("modal_test.start", json!({})).await;
    assert_eq!(s["state"], "idle", "{s}");
    c.ok("modal_test.arm", json!({})).await;
    let end = until(&mut c, Duration::from_secs(60), "three points", |s| {
        s["counts"]["totals"]["points_measured"] == 3
    })
    .await;
    assert_eq!(end["counts"]["totals"]["accepted"], 9, "{end}");
    c.ok("modal_test.end", json!({})).await;
    let fin = until(&mut c, Duration::from_secs(20), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
    assert_eq!(fin["end"]["outcome"], "ended", "{fin}");
    let folder = PathBuf::from(fin["record"]["folder"].as_str().unwrap());
    record_done(&mut c, &folder).await;
    let (recs, frf, start, step) = frf_records(&folder);
    assert_eq!(recs.len(), 3);
    // The windows are stored with the FRF: the force window from the trigger sample.
    let side = read(&folder.join("results/modal_frf.json"));
    assert_eq!(
        side["windows"]["force"],
        json!({"pre_frames": 51, "width_frames": 102, "fall_frames": 10})
    );
    assert_eq!(side["windows"]["exponential"], Value::Null);
    let resp = [Dof::new(4, "Z+"), Dof::new(7, "Z+")];
    for (k, rec) in recs.iter().enumerate() {
        let p = rec["point"].as_u64().unwrap() as usize;
        assert_eq!(p, k);
        let hammer = Dof::new(p as u32 + 1, "Z+");
        for (i, a) in resp.iter().enumerate() {
            let e = worst_at_resonances(&frf[k][i][0], a, &hammer, start, step);
            eprintln!("point {p} response {i}: worst error at the resonances {e:.5}");
            assert!(e < 0.02, "point {p} response {i}: {e}");
        }
    }
    let _ = std::fs::remove_dir_all(folder.parent().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_two_shaker_burst_random_matrix_h1_matches_the_structure() {
    let mut c = rig(true, 16.0).await;
    c.ok(
        "modal_test.configure",
        json!({"config": shaker_config("burst_random")}),
    )
    .await;
    c.ok("modal_test.start", json!({})).await;
    c.ok("modal_test.drive_start", json!({})).await;
    until(&mut c, Duration::from_secs(60), "the point", |s| {
        s["counts"]["totals"]["points_measured"] == 1
    })
    .await;
    // The point done, the drive ramps down on its own.
    until(&mut c, Duration::from_secs(20), "the drive idle", |s| {
        s["drive_phase"] == "idle"
    })
    .await;
    c.ok("modal_test.end", json!({})).await;
    let fin = until(&mut c, Duration::from_secs(20), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
    assert_eq!(fin["end"]["outcome"], "ended", "{fin}");
    let folder = PathBuf::from(fin["record"]["folder"].as_str().unwrap());
    record_done(&mut c, &folder).await;
    let (recs, frf, start, step) = frf_records(&folder);
    assert_eq!(recs.len(), 1);
    let refs = [Dof::new(1, "Z+"), Dof::new(8, "Z+")];
    let resp = [Dof::new(4, "Z+"), Dof::new(7, "Z+")];
    for (i, a) in resp.iter().enumerate() {
        for (j, b) in refs.iter().enumerate() {
            let e = worst_at_resonances(&frf[0][i][j], a, b, start, step);
            eprintln!("response {i} reference {j}: worst error at the resonances {e:.5}");
            assert!(e < 0.02, "response {i} reference {j}: {e}");
        }
    }
    // Several references: the multiple coherence and the condition number.
    let coh = read(&folder.join("results/modal_coherence.json"));
    assert_eq!(coh["arrays"][0]["name"], "multiple_coherence");
    let spec = read(&folder.join("results/modal_reference_spectra.json"));
    assert_eq!(spec["arrays"][1]["name"], "condition_number");
    let _ = std::fs::remove_dir_all(folder.parent().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_refusals_are_named() {
    let mut c = rig(true, 8.0).await;
    let e = c.refused("modal_test.start", json!({})).await;
    assert_eq!(e["reason"], "not_configured", "{e}");
    // Impact mode with two force rows.
    let e = c
        .refused("modal_test.configure", json!({"config": impact_config()}))
        .await;
    assert_eq!(e["reason"], "config_invalid", "{e}");
    assert!(e.to_string().contains("reference_count"), "{e}");
    // A level over the ceiling.
    let mut s = shaker_config("burst_random");
    s["shaker"]["drives"][0]["level_vrms"] = json!(2.0);
    let e = c
        .refused("modal_test.configure", json!({"config": s}))
        .await;
    assert!(e.to_string().contains("level_over_ceiling"), "{e}");
    // An unknown key.
    let mut s = shaker_config("burst_random");
    s["bogus"] = json!(1);
    c.refused("modal_test.configure", json!({"config": s}))
        .await;
    // A dry run reports and configures nothing.
    let d = c
        .ok(
            "modal_test.configure",
            json!({"config": shaker_config("burst_random"), "dry_run": true}),
        )
        .await;
    assert_eq!(d["errors"].as_array().unwrap().len(), 0, "{d}");
    assert_eq!(state(&mut c).await["state"], "none");
    // Calls before the start.
    c.ok(
        "modal_test.configure",
        json!({"config": shaker_config("burst_random")}),
    )
    .await;
    let e = c.refused("modal_test.accept", json!({})).await;
    assert_eq!(e["reason"], "not_live", "{e}");
    c.ok("modal_test.start", json!({})).await;
    let e = c.refused("modal_test.start", json!({})).await;
    assert_eq!(e["reason"], "wrong_state", "{e}");
    let e = c
        .refused("modal_test.sim_hit", json!({"kind": "clean"}))
        .await;
    assert_eq!(e["reason"], "not_in_mode", "{e}");
    c.ok("modal_test.abort", json!({})).await;
    until(&mut c, Duration::from_secs(20), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_impact_mode_writes_no_drive_sample() {
    let mut c = rig(false, 8.0).await;
    let mut cfg = impact_config();
    cfg["roving"]["points"]["count"] = json!(1);
    cfg["averages"] = json!(1);
    c.ok("modal_test.configure", json!({"config": cfg})).await;
    c.ok("modal_test.start", json!({})).await;
    let e = c.refused("modal_test.drive_start", json!({})).await;
    assert_eq!(e["reason"], "not_in_mode", "{e}");
    c.ok("modal_test.arm", json!({})).await;
    until(&mut c, Duration::from_secs(30), "the point", |s| {
        s["counts"]["totals"]["points_measured"] == 1
    })
    .await;
    c.ok("modal_test.end", json!({})).await;
    let fin = until(&mut c, Duration::from_secs(20), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
    assert_eq!(fin["drive_phase"], "idle");
    let folder = PathBuf::from(fin["record"]["folder"].as_str().unwrap());
    record_done(&mut c, &folder).await;
    assert!(folder.join("streams/inputs/stream.json").is_file());
    assert!(!folder.join("streams/drives").exists());
    let setup = read(&folder.join("setup.json"));
    assert_eq!(setup["resolved"]["drives"], json!([]));
    let _ = std::fs::remove_dir_all(folder.parent().unwrap());
}

/// The drive stream's per-frame peak over both shakers.
fn drive_peaks(folder: &Path) -> Vec<f32> {
    let d = f32s(&folder.join("streams/drives/telemetry.f32"));
    d.chunks(2).map(|f| f[0].abs().max(f[1].abs())).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_shaker_stop_ramps_down_and_abort_zeroes_at_once() {
    // The stop: the drive ramps 40 dB over the shared ramp, then holds zero.
    let mut c = rig(true, 8.0).await;
    c.ok(
        "modal_test.configure",
        json!({"config": shaker_config("random")}),
    )
    .await;
    c.ok("modal_test.start", json!({})).await;
    c.ok("modal_test.drive_start", json!({})).await;
    until(&mut c, Duration::from_secs(30), "driving", |s| {
        s["drive_phase"] == "driving"
    })
    .await;
    c.ok("modal_test.drive_stop", json!({})).await;
    until(&mut c, Duration::from_secs(30), "the drive idle", |s| {
        s["drive_phase"] == "idle"
    })
    .await;
    c.ok("modal_test.end", json!({})).await;
    let fin = until(&mut c, Duration::from_secs(20), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
    assert_eq!(fin["end"]["emergency_stop"], false, "{fin}");
    let folder = PathBuf::from(fin["record"]["folder"].as_str().unwrap());
    record_done(&mut c, &folder).await;
    let p = drive_peaks(&folder);
    let last = p.iter().rposition(|x| *x != 0.0).unwrap();
    let top = p.iter().fold(0f32, |m, x| m.max(*x));
    // The ramp's last half second is under a tenth of the drive's peak: it ramped,
    // and every frame after it is zero.
    let tail = p[last.saturating_sub(512)..=last]
        .iter()
        .fold(0f32, |m, x| m.max(*x));
    assert!(tail < 0.1 * top, "tail {tail} against {top}");
    assert!(p[last + 1..].iter().all(|x| *x == 0.0));
    let _ = std::fs::remove_dir_all(folder.parent().unwrap());

    // The abort: zero at once.
    let mut c = rig(true, 8.0).await;
    c.ok(
        "modal_test.configure",
        json!({"config": shaker_config("random")}),
    )
    .await;
    c.ok("modal_test.start", json!({})).await;
    c.ok("modal_test.drive_start", json!({})).await;
    until(&mut c, Duration::from_secs(30), "driving", |s| {
        s["drive_phase"] == "driving"
    })
    .await;
    c.ok("modal_test.abort", json!({})).await;
    let fin = until(&mut c, Duration::from_secs(20), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
    assert_eq!(fin["end"]["outcome"], "aborted", "{fin}");
    assert_eq!(fin["end"]["emergency_stop"], true, "{fin}");
    let folder = PathBuf::from(fin["record"]["folder"].as_str().unwrap());
    record_done(&mut c, &folder).await;
    let p = drive_peaks(&folder);
    let last = p.iter().rposition(|x| *x != 0.0).unwrap();
    let top = p.iter().fold(0f32, |m, x| m.max(*x));
    // Driving at the level up to the abort: the last frames before zero are near the
    // drive's level, not ramped.
    let before = p[last.saturating_sub(64)..=last]
        .iter()
        .fold(0f32, |m, x| m.max(*x));
    assert!(before > 0.3 * top, "before {before} against {top}");
    assert!(p[last + 1..].iter().all(|x| *x == 0.0));
    let _ = std::fs::remove_dir_all(folder.parent().unwrap());
}

async fn taken(c: &mut Client, n: usize) -> Value {
    let s = until(c, Duration::from_secs(30), "the hits", |s| {
        s["counts"]["totals"]["taken"].as_u64() >= Some(n as u64)
    })
    .await;
    c.ok("modal_test.disarm", json!({})).await;
    let s2 = state(c).await;
    assert_eq!(s2["counts"]["totals"]["taken"], n as u64, "{s}");
    s2
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_hits_carry_their_status_and_the_counts_follow() {
    let mut c = rig(false, 2.0).await;
    let mut cfg = impact_config();
    cfg["roving"]["points"]["count"] = json!(1);
    cfg["averages"] = json!(3);
    cfg["impact"]["target"] = json!({"peak": 25, "tolerance_pct": 10});
    c.ok("modal_test.configure", json!({"config": cfg})).await;
    c.ok("modal_test.start", json!({})).await;
    // The automatic rule: a double hit and an overload are rejected.
    c.ok("modal_test.sim_hit", json!({"kind": "double"})).await;
    c.ok("modal_test.sim_hit", json!({"kind": "overload"}))
        .await;
    c.ok("modal_test.arm", json!({})).await;
    let s = taken(&mut c, 2).await;
    let hits = s["hits"].as_array().unwrap();
    assert_eq!(
        (hits[0]["status"].as_str(), hits[0]["reason"].as_str()),
        (Some("rejected"), Some("double_hit")),
        "{s}"
    );
    assert_eq!(hits[0]["double_hit"], true);
    assert_eq!(
        (hits[1]["status"].as_str(), hits[1]["reason"].as_str()),
        (Some("rejected"), Some("overload")),
        "{s}"
    );
    assert_eq!(hits[1]["overload"], true);
    assert_eq!(
        s["counts"]["point"],
        json!({"index": 0, "accepted": 0, "rejected": 2, "asked": 3})
    );
    // The operator accepts the latest (overloaded) hit anyway.
    let r = c.ok("modal_test.accept", json!({})).await;
    assert_eq!(r["counts"]["point"]["accepted"], 1, "{r}");
    assert_eq!(r["state"]["hits"][1]["status"], "accepted");
    // A clean hit under the target: low, and accepted all the same.
    c.ok("modal_test.arm", json!({})).await;
    let s = taken(&mut c, 3).await;
    let h = &s["hits"][2];
    assert_eq!(h["status"], "accepted", "{s}");
    assert_eq!(h["target_verdict"], "low", "{s}");
    let peak = h["peak"].as_f64().unwrap();
    let diff = h["target_diff_pct"].as_f64().unwrap();
    assert!(
        peak < 22.5 && (diff - (peak - 25.0) / 25.0 * 100.0).abs() < 1e-6,
        "{h}"
    );
    assert_eq!(s["latest"]["report"]["target"], 25.0, "{s}");
    assert_eq!(s["counts"]["point"]["accepted"], 2);
    // The operator rejects it, then undoes the last accepted.
    let r = c.ok("modal_test.reject", json!({})).await;
    assert_eq!(r["state"]["hits"][2]["reason"], "operator", "{r}");
    assert_eq!(r["counts"]["point"]["accepted"], 1);
    let r = c.ok("modal_test.undo", json!({})).await;
    assert_eq!(r["counts"]["point"]["accepted"], 0, "{r}");
    assert_eq!(
        r["counts"]["totals"],
        json!({"accepted": 0, "taken": 3, "points_measured": 0, "points_planned": 1})
    );
    // Use this hit as the target.
    c.ok("modal_test.target_from_hit", json!({})).await;
    c.ok("modal_test.abort", json!({})).await;
    let fin = until(&mut c, Duration::from_secs(20), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
    let folder = PathBuf::from(fin["record"]["folder"].as_str().unwrap());
    record_done(&mut c, &folder).await;
    // Every take and every change of status is a line of modal_frames.
    let f = lines(&folder.join("results/modal_frames.jsonl"));
    let events: Vec<&str> = f.iter().map(|l| l["event"].as_str().unwrap()).collect();
    assert_eq!(
        events.iter().filter(|e| **e == "taken").count(),
        3,
        "{events:?}"
    );
    assert!(
        events.iter().filter(|e| **e == "status").count() >= 3,
        "{events:?}"
    );
    let out = read(&folder.join("results/modal_outcome.json"));
    assert_eq!(out["totals"]["taken"], 3, "{out}");
    let _ = std::fs::remove_dir_all(folder.parent().unwrap());
}
