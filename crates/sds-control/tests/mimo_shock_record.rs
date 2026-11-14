//! Every MIMO Shock run leaves a run folder (docs/run-folder.md): the record
//! written at arm, the log as it is made, the streams of shots, each pulse's
//! results as it is judged, every law call, and the end, on the simulator through
//! the protocol.
//!
//! Every run here is SIMULATED (`hertzforge-hal-sim` through the station factory);
//! nothing in this file is evidence about hardware.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::{Service, ServiceOptions};
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use support::{options, serve, Client};

const LONG: Duration = Duration::from_secs(300);

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

fn opts() -> ServiceOptions {
    let mut o = options(
        no_driver(),
        0.0,
        Duration::from_secs(60),
        Duration::from_secs(5),
    );
    o.mimo_driver = None;
    o
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn read_lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn wait_complete(folder: &Path, wait: Duration) {
    let t0 = Instant::now();
    loop {
        let done = folder.join("end.json").is_file()
            && read_json(&folder.join("run.json"))["complete"] == true;
        if done {
            return;
        }
        assert!(
            t0.elapsed() < wait,
            "the record of {} did not complete",
            folder.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn row(index: u32, role: &str) -> Value {
    let drive = role == "drive";
    json!({
        "index": index, "enabled": true, "description": format!("{role} {index}"),
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": if drive { "V" } else { "g" }, "sensitivity_mv_per_eu": 1000,
        "range_max": if drive { 10 } else { 1000 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

fn channels() -> Value {
    json!({
        "controls": (1..=4).map(|i| json!({"channel": i})).collect::<Vec<_>>(),
        "drives": (5..=8)
            .map(|i| json!({"channel": i, "ceiling_vpk": 6, "abort_vpk": 9}))
            .collect::<Vec<_>>(),
        "limits": [],
    })
}

/// A short SRS run at four controls on the linear 4 x 4: records of 64 samples at
/// 4096 Hz, two levels, one passing pulse at full level, judged by the global
/// verdict.
fn srs_config(cooling_s: f64) -> Value {
    json!({
        "title": "Recorded MIMO Shock",
        "operator": "Operator",
        "article": "Bracket",
        "notes": "two levels",
        "specification": {"document": "Example procedure", "clause": "4.2"},
        "channels": channels(),
        "target": {"kind": "srs",
                   "bands": {"f_lo_hz": 1250, "f_hi_hz": 1600, "bands_per_octave": 3, "q": 10},
                   "tolerance_db": [-9, 9],
                   "controls": (0..4).map(|_| json!({
                       "target": {"kind": "flat", "accel_g": 2}})).collect::<Vec<_>>()},
        "verdict": "global",
        "acquisition": {"fs_hz": 4096, "record_len": 64, "band_hz": [700, 2000],
                        "lead_s": 0.002},
        "schedule": {"mode": "auto", "levels_db": [-6, 0], "full_level_pulses": 1},
        "spacing": {"cooling_s": cooling_s, "settling_s": 0, "compute_s": 0},
        "identification": {"drive_vrms": 1, "frames": 40, "review": false},
        "control": {"law_budget": 1},
        "simulator": {"plant": "shock-linear-4x4"},
    })
}

/// Four classical half-sines on the linear 4 x 4 at full level only.
fn waveform_config() -> Value {
    let pulse = |g: f64| json!({"shape": "half_sine", "amplitude_g": g, "duration_ms": 11, "polarity": "positive"});
    json!({
        "title": "Recorded Half-Sine",
        "channels": channels(),
        "target": {"kind": "waveform", "pulses": [pulse(20.0), pulse(18.0), pulse(20.0), pulse(18.0)]},
        "acquisition": {"fs_hz": 12800, "record_len": 8192, "band_hz": [2, 2500]},
        "schedule": {"mode": "auto", "levels_db": [0], "full_level_pulses": 1},
        "spacing": {"cooling_s": 0.05, "settling_s": 0, "compute_s": 0},
        "identification": {"drive_vrms": 1, "frames": 40, "review": false},
        "control": {"law": {"id": "inverse"}, "law_budget": 1},
        "simulator": {"plant": "shock-linear-4x4"},
    })
}

async fn controller(c: &mut Client) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    let rows: Vec<Value> = (1..=4)
        .map(|i| row(i, "control"))
        .chain((5..=8).map(|i| row(i, "drive")))
        .collect();
    c.ok("channels.set", json!({"revision": rev, "channels": rows}))
        .await;
}

async fn wait_end(c: &mut Client) -> Value {
    let i = c
        .wait_for("the run's end", LONG, |e| {
            e.topic() == "mimo_shock.state" && e.data()["state"] == "finished"
        })
        .await;
    c.events[i].data().clone()
}

async fn run_to_end(c: &mut Client, config: Value) -> (Value, PathBuf) {
    controller(c).await;
    c.ok("mimo_shock.configure", json!({ "config": config }))
        .await;
    c.ok("mimo_shock.start", json!({})).await;
    let end = wait_end(c).await;
    let folder = PathBuf::from(end["record"]["folder"].as_str().unwrap());
    (end, folder)
}

fn arrays(d: &Value) -> Vec<Value> {
    d["arrays"].as_array().unwrap().clone()
}

fn array<'a>(d: &'a Value, name: &str) -> &'a Value {
    d["arrays"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == name)
        .unwrap_or_else(|| panic!("no array {name} in {d}"))
}

/// The standard, item by item, for a MIMO Shock run on the simulator: the table,
/// the station, the setup as accepted and resolved, the limits, the law, the
/// timeline with its data, every enabled input and every drive as shots (the
/// identification's and every pulse's), every pulse's results the last included,
/// every law call, the provenance and a unit on every array.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mimo_shock_run_leaves_a_complete_self_describing_folder() {
    let addr = serve(Service::with_options(opts())).await;
    let mut c = Client::connect(addr).await;
    let config = srs_config(0.05);
    let (end, folder) = run_to_end(&mut c, config.clone()).await;
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    let state_record = end["record"].clone();
    assert_eq!(state_record["streams"], "on", "{end}");
    assert!(state_record["problem"].is_null(), "{end}");
    wait_complete(&folder, LONG);
    let rows = c.ok("mimo_shock.pulses", json!({})).await["pulses"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(rows.len(), 2, "{rows:?}");

    // 1 the table, every row
    let table = read_json(&folder.join("table.json"));
    assert_eq!(table["rows"].as_array().unwrap().len(), 8);
    assert_eq!(table["sha256"].as_str().unwrap().len(), 64);
    // 2 the station and 9 the provenance
    let run = read_json(&folder.join("run.json"));
    assert_eq!(run["schema"], "hertzforge-run/1");
    assert_eq!(run["test"], "mimo_shock");
    assert_eq!(run["source"], "SIM");
    assert_eq!(run["sim"]["plant"], "shock-linear-4x4", "{run}");
    assert_eq!(run["station"]["kind"], "sim");
    assert_eq!(run["grid"]["fs_hz"], 4096.0);
    assert_eq!(run["grid"]["record_len"], 64);
    assert_eq!(run["operator"], "Operator");
    assert_eq!(run["article"], "Bracket");
    assert_eq!(run["notes"], "two levels");
    assert_eq!(run["specification"]["clause"], "4.2");
    assert!(run["build"]["commit"].is_string());
    assert!(run["armed"]["unix"].as_f64().unwrap() > 1.7e9);
    assert!(run["start"]["utc_offset_s"].is_number() || run["start"]["utc_offset_s"].is_null());
    assert_eq!(run["recording"]["streams"], "on");
    assert_eq!(run["complete"], true);
    assert_eq!(run["run_id"], end["run_id"]);
    // 3 the setup as accepted, whole, and resolved
    let setup = read_json(&folder.join("setup.json"));
    assert_eq!(setup["config"], config);
    let r = &setup["resolved"];
    assert_eq!(r["channels"]["controls"].as_array().unwrap().len(), 4);
    assert_eq!(r["channels"]["drives"][0]["ceiling_vpk"], 6.0);
    assert_eq!(r["target"]["kind"], "srs");
    assert_eq!(
        r["target"]["controls"][0]["points"]
            .as_array()
            .unwrap()
            .len(),
        r["target"]["natural_hz"].as_array().unwrap().len()
    );
    assert!(
        r["channels"]["controls"][0]["reference_peak"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    let levels: Vec<f64> = r["schedule"]["levels_db"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    assert_eq!(levels, vec![-6.0, 0.0]);
    assert_eq!(r["schedule"]["full_level_pulses"], 1);
    assert_eq!(r["identification"]["frames"], 40);
    assert_eq!(r["acquisition"]["quiet_frames"], 64);
    assert_eq!(r["verdict"], "global");
    assert!(setup["derived"]["grid"].is_object());
    // 4 the limits and aborts, 5 the law and its parameters
    assert_eq!(r["limits"]["scaled_aborts"], true);
    assert_eq!(r["limits"]["max_growth_db"], 6.0);
    let law = read_json(&folder.join("law/law.json"));
    assert_eq!(law["id"], "shape");
    assert_eq!(law["builtin"], true);
    assert!(law["params"].as_object().is_some_and(|p| !p.is_empty()));
    assert_eq!(law["guards"]["law_budget"], 1.0);
    // 7 the timeline, as it was made, with frames and data
    let log = read_lines(&folder.join("log.jsonl"));
    for (k, e) in log.iter().enumerate() {
        assert_eq!(e["seq"], k as u64, "{e}");
    }
    let pulses: Vec<&Value> = log.iter().filter(|e| e["kind"] == "pulse").collect();
    assert_eq!(pulses.len(), 2);
    assert_eq!(pulses[1]["data"]["pulse"], 1);
    assert!(pulses[0]["frame"].as_u64().unwrap() > 0);
    assert!(log
        .iter()
        .any(|e| e["kind"] == "station" && e["data"]["fs_hz"] == 4096.0));
    let last = log.last().unwrap();
    assert_eq!(last["data"]["outcome"], "completed", "{last}");
    // 8 the data: the streams, shot for shot
    let side = |s: &str| read_json(&folder.join(format!("streams/{s}/stream.json")));
    let (i, d) = (side("inputs"), side("drives"));
    assert_eq!(i["channels"].as_array().unwrap().len(), 4);
    assert_eq!(d["channels"][0]["unit"], "V");
    assert_eq!(i["channels"][0]["unit"], "g");
    let shots = i["shots"].as_array().unwrap();
    assert_eq!(shots.len(), 4, "{i}");
    assert_eq!(shots.len(), d["shots"].as_array().unwrap().len());
    assert_eq!(shots[0]["step"], "probe");
    assert_eq!(shots[1]["step"], "estimate");
    assert_eq!(shots[2]["what"], "pulse");
    assert_eq!(shots[3]["pulse"], 1);
    assert_eq!(shots[3]["level_db"], 0.0);
    for s in shots {
        assert_eq!(s["pre_frames"], 64, "{s}");
    }
    // a pulse's shot is its record between the quiet frames
    assert_eq!(shots[2]["frames"], 64 + 64 + 64);
    let bytes = std::fs::metadata(folder.join("streams/inputs/telemetry.f32"))
        .unwrap()
        .len();
    assert_eq!(bytes, i["frames"].as_u64().unwrap() * 4 * 4);
    // the results, the last pulse's included
    let res = |n: &str| read_json(&folder.join(format!("results/{n}.json")));
    let lines = |n: &str| read_lines(&folder.join(format!("results/{n}.jsonl")));
    let frf = res("identification_frf");
    assert_eq!(frf["kind"], "frf");
    assert_eq!(frf["convention"], "file");
    assert_eq!(array(&frf, "values")["dtype"], "c16");
    assert_eq!(array(&frf, "values")["shape"][0], 4);
    assert_eq!(array(&frf, "values")["shape"][1], 4);
    assert_eq!(array(&frf, "values")["unit"], "g/V");
    assert_eq!(res("identification_coherence")["kind"], "coherence");
    assert_eq!(res("srs_reference")["kind"], "srs_reference");
    assert_eq!(
        array(&res("srs_reference"), "reference")["shape"][0],
        json!(4)
    );
    assert_eq!(res("synthesis")["kind"], "mimo_shock_synthesis");
    assert_eq!(res("seed")["kind"], "mimo_shock_seed");
    assert_eq!(lines("pulse_srs").len(), 2);
    assert_eq!(lines("pulse_drives").len(), 2);
    let judged = lines("pulses");
    assert_eq!(judged.len(), 2);
    for (j, r) in judged.iter().zip(&rows) {
        assert_eq!(j["passed"], r["passed"]);
        assert_eq!(j["level_db"], r["level_db"]);
    }
    let calls = lines("law_calls");
    assert_eq!(calls[0]["call"], "seed");
    assert!(calls.len() >= 2, "{calls:?}");
    assert_eq!(calls[1]["call"], "iterate");
    assert_eq!(calls[1]["after_pulse"], 0);
    // 10 a unit on every array
    let manifest_results = run["results"].as_array().unwrap();
    assert!(manifest_results.len() >= 10, "{run}");
    for m in manifest_results {
        let n = m["name"].as_str().unwrap();
        for a in arrays(&res(n)) {
            assert!(a["unit"].is_string(), "{n}: {a}");
        }
    }
    let e = read_json(&folder.join("end.json"));
    assert_eq!(e["end"], end["end"]);
    assert_eq!(e["streams"]["inputs"]["drops"], 0);
    assert_eq!(e["summary"]["pulses_fired"], 2);
}

/// A waveform target is recorded as the reference each control is judged against
/// (`target`, a transient reference over every control), and has no SRS results.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mimo_shock_waveform_run_records_its_target_as_a_transient_reference() {
    let mut o = opts();
    o.record.stream_buffer_s = 120.0;
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    let (end, folder) = run_to_end(&mut c, waveform_config()).await;
    assert!(end["pulses"]["fired"].as_u64().unwrap() >= 1, "{end}");
    wait_complete(&folder, LONG);
    let t = read_json(&folder.join("results/target.json"));
    assert_eq!(t["kind"], "transient_reference");
    assert_eq!(t["channels"]["responses"], json!([1, 2, 3, 4]));
    assert_eq!(array(&t, "reference")["shape"], json!([4, 8192]));
    assert_eq!(t["standard"], "mil_std_810_516");
    assert!(!folder.join("results/srs_reference.json").exists());
    assert!(!folder.join("results/pulse_srs.json").exists());
    let setup = read_json(&folder.join("setup.json"));
    assert_eq!(setup["resolved"]["target"]["kind"], "waveform");
    assert!(
        read_lines(&folder.join("results/pulses.jsonl")).len()
            == end["pulses"]["fired"].as_u64().unwrap() as usize
    );
}

/// The arm gate: a record that cannot be written refuses the arm, naming the path
/// and the reason, and nothing is armed. Red if the arm went ahead without a record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_data_folder_that_cannot_be_written_refuses_the_mimo_shock_arm() {
    let mut o = opts();
    let blocked = o.data_root.join("not-a-folder");
    std::fs::create_dir_all(&o.data_root).unwrap();
    std::fs::write(&blocked, b"a file").unwrap();
    o.capture_root = Some(blocked.clone());
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok(
        "mimo_shock.configure",
        json!({ "config": srs_config(0.05) }),
    )
    .await;
    let e = c.refused("mimo_shock.arm", json!({})).await;
    assert_eq!(e["reason"], "record_unwritable", "{e}");
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains(&blocked.display().to_string()),
        "{e}"
    );
    assert_eq!(e["detail"]["path"], blocked.display().to_string());
    let s = c.ok("mimo_shock.state", json!({})).await;
    assert_eq!(s["state"], "configured", "{s}");
    assert!(s.get("record").is_none(), "{s}");
    let e = c.refused("mimo_shock.start", json!({})).await;
    assert_eq!(e["reason"], "record_unwritable", "{e}");
}

/// `record.streams: false` keeps the record and its results and leaves out the
/// streams, and the manifest and the log say so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_record_streams_false_writes_the_record_without_streams() {
    let addr = serve(Service::with_options(opts())).await;
    let mut c = Client::connect(addr).await;
    let mut cfg = srs_config(0.05);
    cfg["record"] = json!({"streams": false});
    let (end, folder) = run_to_end(&mut c, cfg).await;
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    assert_eq!(end["record"]["streams"], "off");
    wait_complete(&folder, LONG);
    let run = read_json(&folder.join("run.json"));
    assert_eq!(run["recording"]["why"], "switched off in the setup");
    assert_eq!(run["streams"], json!([]));
    assert!(!folder.join("streams").exists());
    assert_eq!(read_lines(&folder.join("results/pulses.jsonl")).len(), 2);
    let log = read_lines(&folder.join("log.jsonl"));
    assert!(log.iter().any(|e| e["kind"] == "record"), "{log:?}");
}

/// The module's own capture is gone: a configuration that asks for it is refused
/// by name rather than left to believe it is capturing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mimo_shock_configuration_with_capture_is_refused_by_name() {
    let addr = serve(Service::with_options(opts())).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let mut cfg = srs_config(0.05);
    cfg["capture"] = json!({"auto_start": true, "run_name": "kept"});
    let e = c
        .refused("mimo_shock.configure", json!({ "config": cfg }))
        .await;
    assert_eq!(e["reason"], "bad_params", "{e}");
    assert_eq!(e["detail"]["param"], "config.capture", "{e}");
}

/// The firing does not depend on the disk: a record whose writer waits two seconds
/// before every write leaves the pulses' starts, the levels fired, their verdicts
/// and the outcome as they are with no wait. The writer falls behind (the run ends
/// long before its record does) and nothing reaches the firing thread. Simulated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_writer_stalled_on_every_write_leaves_the_mimo_shock_firing_and_the_outcome_unchanged() {
    async fn timed(delay: Duration) -> (Value, Vec<Value>, PathBuf, f64) {
        let mut o = opts();
        o.record.write_delay = delay;
        let addr = serve(Service::with_options(o)).await;
        let mut c = Client::connect(addr).await;
        controller(&mut c).await;
        c.ok("mimo_shock.configure", json!({ "config": srs_config(0.5) }))
            .await;
        c.ok("mimo_shock.arm", json!({})).await;
        let t0 = Instant::now();
        c.ok("mimo_shock.start", json!({})).await;
        let end = wait_end(&mut c).await;
        let wall = t0.elapsed().as_secs_f64();
        let rows = c.ok("mimo_shock.pulses", json!({})).await["pulses"]
            .as_array()
            .unwrap()
            .clone();
        let folder = PathBuf::from(end["record"]["folder"].as_str().unwrap());
        (end, rows, folder, wall)
    }
    let (free, free_rows, _, free_wall) = timed(Duration::ZERO).await;
    let (stalled, stalled_rows, folder, stalled_wall) = timed(Duration::from_secs(2)).await;
    let starts = |rows: &[Value]| -> Vec<f64> {
        rows.iter()
            .map(|r| r["at_s"].as_f64().unwrap())
            .collect::<Vec<_>>()
    };
    println!(
        "no wait: pulses at {:?} s, elapsed {} s, wall {free_wall:.2} s; two seconds a write: \
         pulses at {:?} s, elapsed {} s, wall {stalled_wall:.2} s",
        starts(&free_rows),
        free["elapsed_s"],
        starts(&stalled_rows),
        stalled["elapsed_s"]
    );
    assert_eq!(free["end"]["outcome"], "completed", "{free}");
    assert_eq!(
        stalled["end"]["outcome"], free["end"]["outcome"],
        "{stalled}"
    );
    assert_eq!(free_rows.len(), stalled_rows.len());
    for (a, b) in free_rows.iter().zip(&stalled_rows) {
        assert_eq!(a["level_db"], b["level_db"], "{a} against {b}");
        assert_eq!(a["passed"], b["passed"], "{a} against {b}");
        let (x, y) = (a["at_s"].as_f64().unwrap(), b["at_s"].as_f64().unwrap());
        assert!((x - y).abs() < 0.25, "a pulse at {x} s against {y} s");
    }
    let (a, b) = (
        free["elapsed_s"].as_f64().unwrap(),
        stalled["elapsed_s"].as_f64().unwrap(),
    );
    assert!((a - b).abs() < 0.5, "elapsed {a} s against {b} s");
    assert!(
        (free_wall - stalled_wall).abs() < 1.0,
        "wall {free_wall} s against {stalled_wall} s"
    );
    // The record is still being written: the run did not wait for it.
    assert!(!folder.join("end.json").exists(), "the writer kept up");
}
