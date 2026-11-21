//! A Modal Test's run folder, item by item: the manifest, the table, the setup, the
//! log, the streams, every result and the end; a test cut off part way leaves its
//! finished points on the disk; and a writer that stalls on every write leaves the
//! hits as they would have been.

mod support;

use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use support::modal::*;

fn names(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_owned())
        .collect()
}

fn shape(side: &Value, k: usize) -> Value {
    side["arrays"][k]["shape"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_record_holds_every_item_of_a_roving_hammer_test() {
    let mut c = rig(false, 8.0).await;
    let cfg = impact_config();
    c.ok("modal_test.configure", json!({"config": cfg.clone()}))
        .await;
    c.ok("modal_test.start", json!({})).await;
    c.ok("modal_test.arm", json!({})).await;
    until(&mut c, Duration::from_secs(60), "three points", |s| {
        s["counts"]["totals"]["points_measured"] == 3
    })
    .await;
    c.ok("modal_test.end", json!({})).await;
    let fin = until(&mut c, Duration::from_secs(20), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
    let folder = PathBuf::from(fin["record"]["folder"].as_str().unwrap());
    record_done(&mut c, &folder).await;

    // run.json
    let run = read(&folder.join("run.json"));
    assert_eq!(run["test"], "modal_test");
    assert_eq!(run["source"], "SIM");
    assert_eq!(run["title"], "Roving Hammer");
    assert_eq!(run["operator"], "A. Operator");
    assert_eq!(run["article"], "Beam, SN 0007");
    assert_eq!(run["run_id"], fin["run_id"]);
    assert_eq!(run["grid"]["fs_hz"], 1024);
    assert_eq!(run["grid"]["analysis_block"], 1024);
    assert_eq!(run["grid"]["df_hz"], 1);
    assert_eq!(run["recording"], json!({"streams": "on"}));
    assert_eq!(run["complete"], true);
    assert!(run["station"].is_object() && run["build"].is_object());
    assert_eq!(names(&run["streams"]), vec!["inputs"]);
    assert_eq!(
        names(&run["results"]),
        vec![
            "modal_frames",
            "modal_frf",
            "modal_coherence",
            "modal_reference_spectra",
            "modal_outcome"
        ]
    );

    // table.json and setup.json
    let table = read(&folder.join("table.json"));
    assert_eq!(table["rows"].as_array().unwrap().len(), 3, "{table}");
    assert_eq!(table["rows"][0]["analysis_role"], "reference");
    assert_eq!(run["channel_table"]["sha256"], table["sha256"]);
    let setup = read(&folder.join("setup.json"));
    assert_eq!(setup["config"], cfg);
    let r = &setup["resolved"];
    assert_eq!(r["control"], "no drive: impact mode writes no drive sample");
    assert_eq!(r["inputs"], json!([1, 2, 3]));
    assert_eq!(r["drives"], json!([]));
    assert_eq!(
        r["windows"]["force"],
        json!({"pre_frames": 51, "width_frames": 102, "fall_frames": 10})
    );
    assert_eq!(r["structure"]["modes"].as_array().unwrap().len(), 3);
    assert_eq!(setup["check"]["mode"], "impact");
    assert_eq!(setup["check"]["points"].as_array().unwrap().len(), 3);

    // log.jsonl
    let log = lines(&folder.join("log.jsonl"));
    let kinds = |k: &str| log.iter().filter(|e| e["kind"] == k).count();
    assert_eq!(kinds("hit"), 9, "{log:?}");
    assert_eq!(kinds("result"), 3, "{log:?}");
    assert_eq!(kinds("outcome"), 1, "{log:?}");
    assert!(log.iter().any(|e| e["data"]["event"] == "started"));

    // streams
    let s = read(&folder.join("streams/inputs/stream.json"));
    assert_eq!(s["channels"].as_array().unwrap().len(), 3);
    assert!(s["frames"].as_u64().unwrap() >= 3 * 3 * 1024, "{s}");

    // modal_frames: the nine hits, each accepted, each at its point.
    let frames = lines(&folder.join("results/modal_frames.jsonl"));
    assert_eq!(frames.len(), 9);
    for (k, f) in frames.iter().enumerate() {
        assert_eq!(f["event"], "taken");
        assert_eq!(f["status"], "accepted");
        assert_eq!(f["point"], k / 3);
        assert_eq!(f["n"], k % 3 + 1);
        assert_eq!(f["seq"], k + 1);
        assert!(f["report"]["peak"].as_f64().unwrap() > 10.0);
    }
    let side = read(&folder.join("results/modal_frames.json"));
    assert_eq!(side["unit"], "hits");
    assert_eq!(shape(&side, 0), json!([2]));
    assert_eq!(f64s(&folder.join("results/modal_frames.bin")).len(), 2 * 9);

    // modal_frf, modal_coherence, modal_reference_spectra: a record per point.
    let nl = 251;
    let frf = read(&folder.join("results/modal_frf.json"));
    assert_eq!(frf["kind"], "frf");
    assert_eq!(frf["layout"], json!(["response", "reference", "line"]));
    assert_eq!(
        frf["abscissa"],
        json!({"kind": "frequency", "unit": "Hz", "start": 0, "step": 1})
    );
    assert_eq!(frf["arrays"][0]["dtype"], "c16");
    assert_eq!(frf["arrays"][0]["unit"], "g/lbf");
    assert_eq!(shape(&frf, 0), json!([2, 1, nl]));
    let recs = lines(&folder.join("results/modal_frf.jsonl"));
    for (k, l) in recs.iter().enumerate() {
        assert_eq!(l["measurement"], k + 1);
        assert_eq!(l["point"], k);
        assert_eq!(l["replaces"], Value::Null);
        assert_eq!(l["averages"], 3);
        assert_eq!(l["estimator"], "H1");
        assert_eq!(l["rows"]["references"][0]["node"], k + 1);
        assert_eq!(l["rows"]["references"][0]["direction"], "Z+");
    }
    let coh = read(&folder.join("results/modal_coherence.json"));
    assert_eq!(coh["arrays"][0]["name"], "coherence");
    assert_eq!(shape(&coh, 0), json!([2, nl]));
    assert_eq!(lines(&folder.join("results/modal_coherence.jsonl")), recs);
    let c = f64s(&folder.join("results/modal_coherence.bin"));
    assert_eq!(c.len(), 3 * 2 * nl);
    assert!(c.iter().all(|x| (0.0..=1.0 + 1e-9).contains(x)));
    let spec = read(&folder.join("results/modal_reference_spectra.json"));
    assert_eq!(spec["arrays"][0]["name"], "autospectra");
    assert_eq!(spec["arrays"][0]["unit"], "lbf^2/Hz");
    assert_eq!(shape(&spec, 0), json!([1, nl]));
    assert_eq!(spec["arrays"].as_array().unwrap().len(), 1);

    // modal_outcome and end.json
    let out = read(&folder.join("results/modal_outcome.json"));
    let totals = json!({"accepted": 9, "taken": 9, "points_measured": 3, "points_planned": 3});
    assert_eq!(out["outcome"], "ended");
    assert_eq!(out["mode"], "impact");
    assert_eq!(out["emergency_stop"], false);
    assert_eq!(out["totals"], totals);
    assert_eq!(
        f64s(&folder.join("results/modal_outcome.bin")),
        vec![9.0, 9.0, 3.0, 3.0]
    );
    let end = read(&folder.join("end.json"));
    assert_eq!(end["end"]["outcome"], "ended");
    assert_eq!(end["totals"], totals);
    assert_eq!(end["state"]["state"], "finished");
    let _ = std::fs::remove_dir_all(folder.parent().unwrap());
}

/// The record is on the disk as the test goes: with one point of three measured, the
/// folder already holds its results and says the run has not ended, so a run cut off
/// there leaves a folder with every finished point.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_a_test_cut_off_part_way_leaves_its_finished_points_on_disk() {
    let mut c = rig(false, 2.0).await;
    c.ok("modal_test.configure", json!({"config": impact_config()}))
        .await;
    c.ok("modal_test.start", json!({})).await;
    c.ok("modal_test.arm", json!({})).await;
    let s = until(&mut c, Duration::from_secs(60), "one point", |s| {
        s["counts"]["totals"]["points_measured"] == 1
    })
    .await;
    let folder = PathBuf::from(s["record"]["folder"].as_str().unwrap());
    let t0 = Instant::now();
    while !folder
        .join("results/modal_reference_spectra.jsonl")
        .is_file()
    {
        assert!(t0.elapsed() < Duration::from_secs(10));
        c.pump(Duration::from_millis(20)).await;
    }
    assert_eq!(read(&folder.join("run.json"))["complete"], false);
    assert!(!folder.join("end.json").exists());
    let (recs, frf, _, _) = frf_records(&folder);
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["point"], 0);
    assert_eq!(frf[0].len(), 2);
    assert!(lines(&folder.join("results/modal_frames.jsonl")).len() >= 3);
    assert!(read(&folder.join("streams/inputs/stream.json"))["channels"].is_array());
    c.ok("modal_test.abort", json!({})).await;
    let fin = until(&mut c, Duration::from_secs(20), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
    assert_eq!(fin["end"]["outcome"], "aborted", "{fin}");
    record_done(&mut c, &folder).await;
    let out = read(&folder.join("results/modal_outcome.json"));
    assert_eq!(out["totals"]["points_measured"], 1, "{out}");
    let _ = std::fs::remove_dir_all(folder.parent().unwrap());
}

/// No file work waits on the run's thread or the loop's: with every write held a
/// second, the hits are taken at the simulator's period, one after another, and the
/// point is measured as soon as without the wait.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modal_test_a_stalled_writer_leaves_the_hits_as_they_were() {
    let mut o = base_options(8.0);
    o.record.write_delay = Duration::from_secs(1);
    let mut c = rig_with(false, o).await;
    let mut cfg = impact_config();
    cfg["roving"]["points"]["count"] = json!(1);
    cfg["averages"] = json!(4);
    c.ok("modal_test.configure", json!({"config": cfg})).await;
    c.ok("modal_test.start", json!({})).await;
    let t0 = Instant::now();
    c.ok("modal_test.arm", json!({})).await;
    let s = until(&mut c, Duration::from_secs(60), "the point", |s| {
        s["counts"]["totals"]["points_measured"] == 1
    })
    .await;
    let took = t0.elapsed();
    // Four hits at the simulator's period of 1.5 s, at eight times real time: under a
    // second of wall time. Every write held a second on the run's thread would add a
    // second per hit and per result.
    assert!(took < Duration::from_secs(4), "{took:?}");
    let starts: Vec<u64> = s["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["start_frame"].as_u64().unwrap())
        .collect();
    assert_eq!(starts.len(), 4, "{s}");
    assert!(starts.windows(2).all(|w| w[1] - w[0] == 1536), "{starts:?}");
    c.ok("modal_test.abort", json!({})).await;
    until(&mut c, Duration::from_secs(20), "the end", |s| {
        s["state"] == "finished"
    })
    .await;
}
