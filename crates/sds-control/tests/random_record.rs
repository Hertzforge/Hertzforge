//! Every Random run leaves a run folder (docs/run-folder.md): the record written at
//! arm, the log as it is made, the streams and the end, on the
//! simulator through the protocol.
//!
//! Every run here is SIMULATED (`hertzforge-hal-sim` through the station factory);
//! nothing in this file is evidence about hardware.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use support::schema::Schema;
use support::{options, serve, sim_config, Client};

const LONG: Duration = Duration::from_secs(120);

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn read_lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Wait until the record says the run is complete.
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

/// A short auto run: two levels, a second at full level.
fn short_config() -> Value {
    let mut c = sim_config("auto", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 1.0);
    c["operator"] = json!("A. Operator");
    c["article"] = json!("Bracket, serial 7");
    c["notes"] = json!("first article");
    c["specification"] = json!({"document": "Test procedure 12", "clause": "4.2"});
    c
}

async fn run_to_end(c: &mut Client, config: Value) -> (Value, PathBuf) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    c.ok("random.configure", json!({ "config": config })).await;
    c.ok("random.start", json!({})).await;
    let end = c.wait_state("finished", LONG).await;
    let folder = PathBuf::from(
        end["record"]["folder"]
            .as_str()
            .expect("the state names the folder"),
    );
    (end, folder)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_random_run_leaves_a_complete_self_describing_folder() {
    let schema = Schema::load();
    let svc = Service::with_options(options(
        no_driver(),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let config = short_config();
    let (end, folder) = run_to_end(&mut c, config.clone()).await;
    schema.assert_valid(&end, Some("randomState"));
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    assert_eq!(end["record"]["streams"], "on");
    assert_eq!(end["record"]["problem"], Value::Null);
    wait_complete(&folder, LONG);

    let run = read_json(&folder.join("run.json"));
    assert_eq!(run["schema"], "hertzforge-run/1");
    assert_eq!(run["test"], "random");
    assert_eq!(run["source"], "SIM");
    assert_eq!(run["run_id"], end["run_id"]);
    assert_eq!(run["title"], config["title"]);
    assert_eq!(run["operator"], "A. Operator");
    assert_eq!(run["article"], "Bracket, serial 7");
    assert_eq!(run["notes"], "first article");
    assert_eq!(run["specification"]["clause"], "4.2");
    assert_eq!(run["sim"]["model"], "resonator_bank");
    assert!(run["sim"]["seed"].is_u64(), "{run}");
    assert_eq!(run["station"]["class"], "sim");
    assert_eq!(run["complete"], true);
    assert_eq!(run["recording"]["streams"], "on");
    assert!(run["run_uid"].as_str().unwrap().len() > 16);
    assert!(run["start"]["unix"].is_f64());
    let names: Vec<&str> = run["streams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["inputs", "drives"]);

    // The table as armed, every row in the protocol's spelling.
    let table = read_json(&folder.join("table.json"));
    let now = c.ok("channels.get", json!({})).await;
    assert_eq!(table["rows"], now["channels"]);
    assert_eq!(table["revision"], now["revision"]);
    assert_eq!(table["sha256"].as_str().unwrap().len(), 64);

    // The setup: the configuration exactly as accepted, and what it resolved to.
    let setup = read_json(&folder.join("setup.json"));
    assert_eq!(setup["config"], config);
    let points = setup["resolved"]["profile"]["breakpoints"]
        .as_array()
        .unwrap();
    assert_eq!(points.len(), 4);
    assert!(
        points.iter().all(|p| p["psd_g2_hz"].is_number()),
        "{points:?}"
    );
    assert_eq!(setup["resolved"]["schedule"]["levels"][1]["dwell_s"], 1);
    assert_eq!(setup["feasibility"]["verdict"], "not_evaluated");

    // The log: every entry the service gave, in order, with the record's own members.
    let log = read_lines(&folder.join("log.jsonl"));
    let wire = c.ok("random.log", json!({"since": 0})).await;
    let wire = wire["entries"].as_array().unwrap();
    assert_eq!(log.len(), wire.len(), "{log:?}");
    for (a, b) in log.iter().zip(wire) {
        assert_eq!(
            (&a["seq"], &a["kind"], &a["text"]),
            (&b["seq"], &b["kind"], &b["text"])
        );
    }
    let levels: Vec<&Value> = log.iter().filter(|e| e["kind"] == "level").collect();
    assert!(!levels.is_empty());
    assert!(
        levels.iter().all(|e| e["data"]["level_db"].is_number()),
        "{levels:?}"
    );
    assert!(log.iter().any(|e| e["frame"].is_u64()));

    // The streams: every enabled input and the drive, the same length.
    let inputs = read_json(&folder.join("streams/inputs/stream.json"));
    let drives = read_json(&folder.join("streams/drives/stream.json"));
    assert_eq!(
        inputs["channels"],
        json!([{"index": 1, "unit": "g", "description": "Control"}])
    );
    assert_eq!(drives["channels"][0]["unit"], "V");
    assert_eq!(inputs["complete"], true);
    let frames = inputs["frames"].as_u64().unwrap();
    assert!(frames > 8192, "{inputs}");
    let size = std::fs::metadata(folder.join("streams/inputs/telemetry.f32"))
        .unwrap()
        .len();
    assert_eq!(size, frames * 4);
    assert_eq!(inputs["gaps"], json!([]));
    assert_eq!(drives["output_latency_frames"]["how"], "declared");

    let ended = read_json(&folder.join("end.json"));
    assert_eq!(ended["end"], end["end"]);
    assert_eq!(ended["run_uid"], run["run_uid"]);
    assert_eq!(ended["streams"]["inputs"]["drops"], 0);
}

/// The arm gate: a record that cannot be written refuses the arm, naming the path
/// and the reason, and nothing is armed. Red if the arm went ahead without a record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_data_folder_that_cannot_be_written_refuses_the_arm() {
    let mut o = options(
        no_driver(),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    );
    // The data folder is a file: no folder can be made in it.
    let blocked = o.data_root.join("not-a-folder");
    std::fs::create_dir_all(&o.data_root).unwrap();
    std::fs::write(&blocked, b"a file").unwrap();
    o.capture_root = Some(blocked.clone());
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    c.ok("random.configure", json!({ "config": short_config() }))
        .await;
    let e = c.refused("random.arm", json!({})).await;
    assert_eq!(e["reason"], "record_unwritable", "{e}");
    let message = e["message"].as_str().unwrap();
    assert!(
        message.contains(&blocked.display().to_string()),
        "{message}"
    );
    assert_eq!(e["detail"]["path"], blocked.display().to_string());
    assert!(!e["detail"]["reason"].as_str().unwrap().is_empty());
    let s = c.ok("random.state", json!({})).await;
    assert_eq!(s["state"], "configured", "{s}");
    assert!(s.get("record").is_none(), "{s}");
    // random.start arms first, and is refused the same way.
    let e = c.refused("random.start", json!({})).await;
    assert_eq!(e["reason"], "record_unwritable", "{e}");
}

/// `record.streams: false` keeps the record and leaves out the streams, and the
/// manifest says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn record_streams_false_writes_the_record_without_streams() {
    let addr = serve(Service::with_options(options(
        no_driver(),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    )))
    .await;
    let mut c = Client::connect(addr).await;
    let mut config = short_config();
    config["record"] = json!({"streams": false});
    let (end, folder) = run_to_end(&mut c, config).await;
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    assert_eq!(end["record"]["streams"], "off");
    wait_complete(&folder, LONG);
    let run = read_json(&folder.join("run.json"));
    assert_eq!(run["recording"]["streams"], "off");
    assert_eq!(run["recording"]["why"], "switched off in the setup");
    assert_eq!(run["streams"], json!([]));
    assert!(!folder.join("streams").exists());
    let log = read_lines(&folder.join("log.jsonl"));
    assert!(log.iter().any(|e| e["kind"] == "record"), "{log:?}");
}

/// Below the free-space floor at arm, the run goes on without its streams, and its
/// state, its log and its manifest say why.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn below_the_free_space_floor_the_run_goes_on_without_streams() {
    let mut o = options(
        no_driver(),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    );
    o.record.free_space_floor_bytes = u64::MAX / 4;
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    let (end, folder) = run_to_end(&mut c, short_config()).await;
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    assert_eq!(end["record"]["streams"], "off");
    wait_complete(&folder, LONG);
    let run = read_json(&folder.join("run.json"));
    assert!(
        run["recording"]["why"].as_str().unwrap().contains("free"),
        "{run}"
    );
    let log = read_lines(&folder.join("log.jsonl"));
    let note = log
        .iter()
        .find(|e| e["kind"] == "record")
        .expect("a record entry");
    assert!(
        note["text"].as_str().unwrap().contains("not recorded"),
        "{note}"
    );
}

/// The run's timing does not depend on the disk: a record whose writer waits two
/// seconds before every write leaves the loop's timing and the run's outcome as
/// they are with no wait. The writer falls behind (the run ends before its record
/// does) and nothing reaches the loop. Simulated, paced at real time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_writer_stalled_on_every_write_leaves_the_loop_and_the_outcome_unchanged() {
    async fn timed(delay: Duration) -> (Value, PathBuf, f64) {
        let mut o = options(
            no_driver(),
            1.0,
            Duration::from_secs(120),
            Duration::from_secs(5),
        );
        o.record.write_delay = delay;
        let addr = serve(Service::with_options(o)).await;
        let mut c = Client::connect(addr).await;
        c.ok("session.claim", json!({"name": "Shell"})).await;
        c.ok("station.select", json!({"kind": "sim"})).await;
        let config = sim_config("auto", json!([{"db": -6, "dwell_s": 1}, {"db": 0}]), 2.0);
        c.ok("random.configure", json!({ "config": config })).await;
        c.ok("random.arm", json!({})).await;
        let t0 = Instant::now();
        c.ok("random.start", json!({})).await;
        let end = c.wait_state("finished", Duration::from_secs(300)).await;
        let wall = t0.elapsed().as_secs_f64();
        let folder = PathBuf::from(end["record"]["folder"].as_str().unwrap());
        (end, folder, wall)
    }
    let (free, _, free_wall) = timed(Duration::ZERO).await;
    let (stalled, folder, stalled_wall) = timed(Duration::from_secs(2)).await;
    println!(
        "no wait: ended at {} s, elapsed {} s, wall {free_wall:.2} s; two seconds a write: ended at {} s, \
         elapsed {} s, wall {stalled_wall:.2} s",
        free["end"]["at_s"], free["elapsed_s"], stalled["end"]["at_s"], stalled["elapsed_s"]
    );
    assert_eq!(free["end"]["outcome"], "completed", "{free}");
    assert_eq!(
        stalled["end"]["outcome"], free["end"]["outcome"],
        "{stalled}"
    );
    assert_eq!(stalled["end"]["level_db"], free["end"]["level_db"]);
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
