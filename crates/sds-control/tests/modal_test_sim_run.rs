//! The Modal Test end to end through the wire on the simulator: a three-point roving
//! hammer and a two-shaker burst random test, each against the simulated structure's
//! own FRF at its resonances; the refusals; impact mode writing no drive sample; the
//! shaker's stop ramp and the abort's zero.

mod support;

use hertzforge_modal_test::structure::Dof;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;
use support::modal::*;
use support::Client;

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
    // Impact mode has no drive: its end does not speak of one.
    let words = fin["end"]["message"].as_str().unwrap();
    assert!(words.starts_with("the test was ended"), "{words}");
    assert!(!words.contains("drive"), "{words}");
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
    let words = fin["end"]["message"].as_str().unwrap();
    assert!(
        words.starts_with("the test was ended with the drive at zero"),
        "{words}"
    );
    let folder = PathBuf::from(fin["record"]["folder"].as_str().unwrap());
    record_done(&mut c, &folder).await;
    let (recs, frf, start, step) = frf_records(&folder);
    assert_eq!(recs.len(), 1);
    // Several references take matrix H1, whatever the setup's estimator.
    assert_eq!(recs[0]["estimator"], "matrix H1", "{}", recs[0]);
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

/// A two-shaker burst random point at the given block: the state at the point's end
/// and the run folder.
async fn burst_point(block: u32) -> (Value, PathBuf) {
    let mut c = rig(true, 16.0).await;
    let mut cfg = shaker_config("burst_random");
    cfg["block"] = json!(block);
    cfg["averages"] = json!(4);
    c.ok("modal_test.configure", json!({"config": cfg})).await;
    c.ok("modal_test.start", json!({})).await;
    c.ok("modal_test.drive_start", json!({})).await;
    let st = until(&mut c, Duration::from_secs(60), "the point", |s| {
        s["counts"]["totals"]["points_measured"] == 1
    })
    .await;
    until(&mut c, Duration::from_secs(20), "the drive idle", |s| {
        s["drive_phase"] == "idle"
    })
    .await;
    c.ok("modal_test.end", json!({})).await;
    let fin = until(&mut c, Duration::from_secs(20), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
    let folder = PathBuf::from(fin["record"]["folder"].as_str().unwrap());
    record_done(&mut c, &folder).await;
    (st, folder)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_a_short_burst_block_warns_response_not_decayed_and_a_long_one_does_not() {
    // At 1024 Hz a 512-frame block is half a second: with the burst on for 40 percent of
    // it, the 42 Hz mode is still ringing at the frame's end. At 1024 frames it is not.
    for (block, warns) in [(512u32, true), (1024, false)] {
        let (st, folder) = burst_point(block).await;
        let frames = lines(&folder.join("results/modal_frames.jsonl"));
        let accepted: Vec<&Value> = frames
            .iter()
            .filter(|f| f["event"] == "taken" && f["status"] == "accepted")
            .collect();
        assert_eq!(accepted.len(), 4, "block {block}");
        let worst = accepted
            .iter()
            .map(|f| f["decay"]["ratio"].as_f64().unwrap())
            .fold(0.0, f64::max);
        eprintln!("block {block}: worst decay ratio {worst:.5}");
        assert_eq!(worst > 0.02, warns, "block {block}: {worst}");
        for f in &accepted {
            assert!(f["decay"]["row"] == 2 || f["decay"]["row"] == 3, "{f}");
        }
        // The measurement and the live state carry the worst over the accepted frames.
        let rec = &lines(&folder.join("results/modal_frf.jsonl"))[0];
        assert_eq!(rec["decay"]["ratio"].as_f64().unwrap(), worst, "{rec}");
        assert_eq!(rec["decay"]["warning"], warns, "{rec}");
        assert_eq!(st["counts"]["point"]["decay"]["warning"], warns, "{st}");
        // The timeline: once for the measurement, when first seen.
        let log = lines(&folder.join("log.jsonl"));
        let seen = log
            .iter()
            .filter(|e| e["data"]["event"] == "response_not_decayed")
            .count();
        assert_eq!(seen, usize::from(warns), "block {block}");
        let _ = std::fs::remove_dir_all(folder.parent().unwrap());
    }
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
    // A decay threshold out of its range, by its field.
    let mut s = shaker_config("burst_random");
    s["decay_warn_pct"] = json!(0);
    let e = c
        .refused("modal_test.configure", json!({"config": s}))
        .await;
    assert_eq!(
        e["detail"]["errors"][0]["field"], "config.decay_warn_pct",
        "{e}"
    );
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
        json!({"index": 0, "accepted": 0, "rejected": 2, "asked": 3, "decay": null})
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

fn point_accepted(s: &Value) -> u64 {
    s["counts"]["point"]["accepted"].as_u64().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_shaker_pause_keeps_the_frames_and_reject_undo_take_the_latest_only() {
    let mut c = rig(true, 8.0).await;
    c.ok(
        "modal_test.configure",
        json!({"config": shaker_config("burst_random")}),
    )
    .await;
    c.ok("modal_test.start", json!({})).await;
    c.ok("modal_test.drive_start", json!({})).await;
    until(&mut c, Duration::from_secs(60), "three frames", |s| {
        point_accepted(s) >= 3
    })
    .await;
    // A drive stop part way: the frames taken stay.
    c.ok("modal_test.drive_stop", json!({})).await;
    let s = until(&mut c, Duration::from_secs(30), "the drive idle", |s| {
        s["drive_phase"] == "idle"
    })
    .await;
    let a = point_accepted(&s);
    assert!((3..12).contains(&a), "{s}");
    assert_eq!(s["counts"]["point"]["asked"], 12);
    assert_eq!(s["counts"]["totals"]["points_measured"], 0);
    // Reject and undo each take the latest frame only.
    let r = c.ok("modal_test.reject", json!({})).await;
    assert_eq!(point_accepted(&r["state"]), a - 1, "{r}");
    let r = c.ok("modal_test.undo", json!({})).await;
    assert_eq!(point_accepted(&r["state"]), a - 2, "{r}");
    // A later drive start goes on with the same average: a pause, not a new start.
    c.ok("modal_test.drive_start", json!({})).await;
    let s = until(&mut c, Duration::from_secs(30), "measuring again", |s| {
        s["state"] == "measuring" && point_accepted(s) > a - 2
    })
    .await;
    // While the drive is on, a reject takes the latest frame out and averaging goes on.
    let b = point_accepted(&s);
    let r = c.ok("modal_test.reject", json!({})).await;
    assert!(point_accepted(&r["state"]) <= b, "{r}");
    let s = until(&mut c, Duration::from_secs(60), "the point", |s| {
        s["counts"]["totals"]["points_measured"] == 1
    })
    .await;
    assert_eq!(point_accepted(&s), 12, "{s}");
    let taken = s["hits"].as_array().unwrap();
    assert_eq!(
        taken.iter().filter(|t| t["status"] == "accepted").count(),
        12
    );
    assert_eq!(taken.iter().filter(|t| t["status"] == "undone").count(), 1);
    // Clearing the point's whole measurement is its own call.
    until(&mut c, Duration::from_secs(30), "the drive idle", |s| {
        s["drive_phase"] == "idle"
    })
    .await;
    let r = c.ok("modal_test.clear_point", json!({})).await;
    assert_eq!(point_accepted(&r["state"]), 0, "{r}");
    assert_eq!(r["state"]["counts"]["totals"]["points_measured"], 0);
    c.ok("modal_test.abort", json!({})).await;
    let fin = until(&mut c, Duration::from_secs(20), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
    let folder = PathBuf::from(fin["record"]["folder"].as_str().unwrap());
    record_done(&mut c, &folder).await;
    let _ = std::fs::remove_dir_all(folder.parent().unwrap());
}

/// The last `modal_test.live` frame, its message and decoded arrays.
fn last_live(c: &Client) -> (Value, &support::Event) {
    let e = *c.of_topic("modal_test.live").last().expect("a live frame");
    (e.data().clone(), e)
}

fn named(chans: &Value) -> Vec<(u64, String)> {
    chans
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["index"].as_u64().unwrap(),
                c["role"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_live_frame_names_its_channels_and_shows_the_latest_hit() {
    let mut c = rig(false, 8.0).await;
    c.ok("modal_test.configure", json!({"config": impact_config()}))
        .await;
    c.ok("modal_test.start", json!({})).await;
    c.ok("modal_test.arm", json!({})).await;
    let s = until(&mut c, Duration::from_secs(60), "a hit", |s| {
        s["counts"]["totals"]["taken"].as_u64().unwrap_or(0) >= 1
    })
    .await;
    // A live frame after the hit.
    c.pump(Duration::from_millis(1500)).await;
    let (live, ev) = last_live(&c);
    let want = [(1, "reference"), (2, "response"), (3, "response")];
    let want: Vec<(u64, String)> = want.iter().map(|(i, r)| (*i, r.to_string())).collect();
    assert_eq!(named(&live["time"]["channels"]), want, "{live}");
    assert_eq!(live["time"]["channels"][0]["unit"], "lbf");
    // The pairs follow point_dofs: responses by references.
    let dofs = &s["point_dofs"];
    let (nr, nf) = (
        dofs["responses"].as_array().unwrap().len(),
        dofs["references"].as_array().unwrap().len(),
    );
    assert_eq!(live["frf"]["pairs"].as_array().unwrap().len(), nr * nf);
    let hit = &live["hit"];
    assert!(hit.is_object(), "{live}");
    assert_eq!(named(&hit["channels"]), want);
    // A block of 1024: every sample sent, at the sample rate.
    assert_eq!(hit["samples"], 1024, "{hit}");
    assert_eq!(hit["decimation"], 1, "{hit}");
    assert_eq!(hit["step_s"], 1.0 / 1024.0, "{hit}");
    let d = ev.decoded.as_ref().unwrap();
    for ch in hit["channels"].as_array().unwrap() {
        assert_eq!(d.array(&ch["values"]).unwrap().to_f64().len(), 1024);
    }
    assert_eq!(hit["windows"]["force"]["pre_frames"], 51, "{hit}");
    assert_eq!(hit["force_spectrum"]["unit"], "lbf^2/Hz");
    assert!(hit["force_spectrum"]["tip_drop_db"].is_number(), "{hit}");
    let f = d
        .array(&hit["force_spectrum"]["frequency_hz"])
        .unwrap()
        .to_f64();
    let v = d.array(&hit["force_spectrum"]["values"]).unwrap().to_f64();
    assert_eq!(f.len(), v.len());
    assert!(f.len() <= 1024 && !f.is_empty());
    assert_eq!(live["references"], Value::Null, "{live}");
    c.ok("modal_test.abort", json!({})).await;
    let fin = until(&mut c, Duration::from_secs(20), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
    let folder = PathBuf::from(fin["record"]["folder"].as_str().unwrap());
    record_done(&mut c, &folder).await;
    let _ = std::fs::remove_dir_all(folder.parent().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_shaker_live_frame_names_the_drives_and_shows_the_reference_spectra() {
    let mut c = rig(true, 8.0).await;
    c.ok(
        "modal_test.configure",
        json!({"config": shaker_config("burst_random")}),
    )
    .await;
    c.ok("modal_test.start", json!({})).await;
    c.ok("modal_test.drive_start", json!({})).await;
    let s = until(&mut c, Duration::from_secs(60), "two frames", |s| {
        point_accepted(s) >= 2
    })
    .await;
    c.pump(Duration::from_millis(1500)).await;
    let (live, ev) = last_live(&c);
    let want = [
        (1, "reference"),
        (2, "response"),
        (3, "response"),
        (4, "reference"),
        (5, "drive"),
        (6, "drive"),
    ];
    let want: Vec<(u64, String)> = want.iter().map(|(i, r)| (*i, r.to_string())).collect();
    assert_eq!(named(&live["time"]["channels"]), want, "{live}");
    assert_eq!(live["time"]["channels"][4]["unit"], "V");
    let dofs = &s["point_dofs"];
    let (nr, nf) = (
        dofs["responses"].as_array().unwrap().len(),
        dofs["references"].as_array().unwrap().len(),
    );
    assert_eq!((nr, nf), (2, 2));
    let pairs = live["frf"]["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), nr * nf);
    assert_eq!(pairs[1]["response"], 0);
    assert_eq!(pairs[1]["reference"], 1);
    assert_eq!(live["hit"], Value::Null, "{live}");
    let r = &live["references"];
    let auto = r["autospectra"].as_array().unwrap();
    let idx: Vec<u64> = auto.iter().map(|a| a["index"].as_u64().unwrap()).collect();
    assert_eq!(idx, vec![1, 4], "{r}");
    assert_eq!(auto[0]["unit"], "lbf^2/Hz");
    let d = ev.decoded.as_ref().unwrap();
    let f = d.array(&r["frequency_hz"]).unwrap().to_f64();
    assert!(!f.is_empty() && f.len() <= 1024);
    for a in auto {
        assert_eq!(d.array(&a["values"]).unwrap().to_f64().len(), f.len());
    }
    c.ok("modal_test.abort", json!({})).await;
    let fin = until(&mut c, Duration::from_secs(20), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
    let folder = PathBuf::from(fin["record"]["folder"].as_str().unwrap());
    record_done(&mut c, &folder).await;
    let _ = std::fs::remove_dir_all(folder.parent().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_shaker_live_frame_spans_the_whole_frame_and_the_burst_ends_where_the_source_says(
) {
    use hertzforge_modal_test::setup::SourceKind;
    use hertzforge_modal_test::source::{Source, SourceBuild};
    let mut c = rig(true, 8.0).await;
    let mut cfg = shaker_config("burst_random");
    cfg["block"] = json!(2048);
    c.ok("modal_test.configure", json!({"config": cfg})).await;
    c.ok("modal_test.start", json!({})).await;
    c.ok("modal_test.drive_start", json!({})).await;
    let s = until(&mut c, Duration::from_secs(60), "two frames", |s| {
        point_accepted(s) >= 2
    })
    .await;
    c.pump(Duration::from_millis(1500)).await;
    let (live, traces) = {
        let (live, ev) = last_live(&c);
        let d = ev.decoded.as_ref().unwrap();
        let traces: Vec<Vec<f64>> = live["frame"]["channels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|ch| d.array(&ch["values"]).unwrap().to_f64())
            .collect();
        (live, traces)
    };
    let f = &live["frame"];
    assert_eq!(f["samples"], 2048, "{f}");
    assert_eq!(f["status"], "accepted", "{f}");
    // Named as `time` names every channel: inputs, then the drives.
    assert_eq!(named(&f["channels"]), named(&live["time"]["channels"]));
    // The frame's tile: its decay check beside the trace.
    let start = f["start_frame"].as_u64().unwrap();
    let st = state(&mut c).await;
    let tile = st["hits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["seq"] == f["seq"])
        .cloned()
        .unwrap_or_else(|| panic!("{f} not in {}", s["hits"]));
    assert_eq!(tile["start_frame"], start);
    assert_eq!(f["decay"], tile["decay"], "{f}");
    // The burst ends where the source says, the input delay after the drive played it.
    let n = 2048usize;
    let src = Source::new(SourceBuild {
        kind: SourceKind::BurstRandom,
        fs_hz: 1024.0,
        block: n,
        lo_line: 0,
        hi_line: 500,
        levels: vec![0.05],
        burst_pct: 40.0,
        delay_frames: 2,
        seed: 1,
    });
    let m = src.burst_len() as u64;
    assert_eq!(f["burst_end_frame"].as_u64().unwrap(), start + m, "{f}");
    let k = f["decimation"].as_u64().unwrap() as usize;
    assert_eq!(k, 4);
    assert_eq!(f["step_s"].as_f64().unwrap(), 4.0 / 2048.0);
    c.ok("modal_test.abort", json!({})).await;
    let fin = until(&mut c, Duration::from_secs(20), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
    let folder = PathBuf::from(fin["record"]["folder"].as_str().unwrap());
    record_done(&mut c, &folder).await;
    // Each trace is the min/max of the recorded channel over the whole frame, no more.
    let side = read(&folder.join("streams/inputs/stream.json"));
    let w = side["channels"].as_array().unwrap().len();
    let first = side["first_station_frame"].as_u64().unwrap_or(0);
    let data = f32s(&folder.join("streams/inputs/telemetry.f32"));
    let a = (start - first) as usize;
    for ch in 0..w {
        let x: Vec<f64> = (a..a + n).map(|i| f64::from(data[i * w + ch])).collect();
        let want = sds_control::modal_test::run::min_max(&x, k);
        let got = &traces[ch];
        assert_eq!(got.len(), 2 * n / k, "channel {ch}");
        let want32: Vec<f64> = want.iter().map(|v| f64::from(*v as f32)).collect();
        assert_eq!(*got, want32, "channel {ch}");
    }
    // The reference (row 1, its drive times 100) is on until the burst's end and quiet
    // after it, inside the frame.
    let r: Vec<f64> = (a..a + n).map(|i| f64::from(data[i * w]).abs()).collect();
    let top = r.iter().fold(0.0f64, |p, v| p.max(*v));
    let m = m as usize;
    let after = r[m..].iter().fold(0.0f64, |p, v| p.max(*v));
    let before = r[m - m / 10..m - m / 20]
        .iter()
        .fold(0.0f64, |p, v| p.max(*v));
    assert!(after < 1e-3 * top, "after the burst {after} of {top}");
    assert!(
        before > 1e-2 * top,
        "the taper before the end {before} of {top}"
    );
    let _ = std::fs::remove_dir_all(folder.parent().unwrap());
}
