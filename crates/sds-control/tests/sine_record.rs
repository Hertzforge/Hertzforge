//! Every Sine run leaves a run folder (docs/run-folder.md): the record written at
//! arm, the log as it is made, the streams, the results and the end, on the
//! simulator through the protocol, checked item by item against "What a Run's
//! Record Holds" (docs/run-data.md).
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
use support::{options, serve, Client};

const LONG: Duration = Duration::from_secs(300);

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

/// An up and a down sweep across 300 to 400 Hz at 6 oct/min, at -6 dB then at 0 dB,
/// with the operator's words, and a limit on the monitor.
fn short_config() -> Value {
    json!({
        "title": "Recorded Sine",
        "operator": "A. Operator",
        "article": "Bracket, serial 7",
        "notes": "first article",
        "specification": {"document": "Test procedure 12", "clause": "4.2"},
        "profile": {"rows": [{"freq_hz": 250, "quantity": "accel_g", "value": 0.2},
                             {"freq_hz": 450, "quantity": "accel_g", "value": 0.2}]},
        "test": {"kind": "sweep", "band_hz": [300, 400],
                 "rate": {"law": "log", "oct_per_min": 6}, "start_hz": 300,
                 "direction": "up", "end": {"sweeps": 2}},
        "acquisition": {"fs_hz": 4096, "tick_ms": 20},
        "schedule": {"mode": "auto", "levels_db": [-6, 0], "per_level": {"sweeps": 1},
                     "full_level": {"sweeps": 1}},
        "identification": {"review": false},
        "limits": {"drive_vpk_ceiling": 5, "drive_vpk_abort": 8,
                   "channels": [{"channel": 2, "profile": {
                       "rows": [{"freq_hz": 250, "quantity": "accel_g", "value": 2},
                                {"freq_hz": 450, "quantity": "accel_g", "value": 2}]}}]}
    })
}

async fn setup(c: &mut Client) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rows = vec![
        row(1, "control", "Control"),
        row(2, "monitor", "Monitor"),
        row(3, "drive", "Drive"),
    ];
    c.ok("channels.set", json!({"revision": 1, "channels": rows}))
        .await;
}

async fn sine_state(c: &mut Client, state: &str) -> Value {
    let i = c
        .wait_for(&format!("sine.state {state}"), LONG, |e| {
            e.topic() == "sine.state" && e.data()["state"] == state
        })
        .await;
    c.events[i].data().clone()
}

async fn run_to_end(c: &mut Client, config: Value) -> (Value, PathBuf) {
    setup(c).await;
    c.ok("sine.configure", json!({ "config": config })).await;
    c.ok("sine.start", json!({})).await;
    let end = sine_state(c, "finished").await;
    let folder = PathBuf::from(
        end["record"]["folder"]
            .as_str()
            .expect("the state names the folder"),
    );
    (end, folder)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sine_run_leaves_a_complete_self_describing_folder() {
    let schema = Schema::load();
    let mut o = options(
        no_driver(),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    );
    // Unpaced, the simulator outruns any disk: buffers for the whole run, so no
    // frame is dropped.
    o.record.stream_buffer_s = 600.0;
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    let config = short_config();
    let (end, folder) = run_to_end(&mut c, config.clone()).await;
    schema.assert_valid(&end, Some("sineState"));
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    assert_eq!(end["record"]["streams"], "on");
    assert_eq!(end["record"]["problem"], Value::Null);
    wait_complete(&folder, LONG);
    assert!(folder
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains("_sine_"));

    // 9. Provenance: the source, the operator, the article, the specification,
    // the build and the times.
    let run = read_json(&folder.join("run.json"));
    assert_eq!(run["schema"], "hertzforge-run/1");
    assert_eq!(run["test"], "sine");
    assert_eq!(run["source"], "SIM");
    assert_eq!(run["run_id"], end["run_id"]);
    assert_eq!(run["title"], "Recorded Sine");
    assert_eq!(run["operator"], "A. Operator");
    assert_eq!(run["article"], "Bracket, serial 7");
    assert_eq!(run["notes"], "first article");
    assert_eq!(run["specification"]["document"], "Test procedure 12");
    assert_eq!(run["specification"]["clause"], "4.2");
    assert!(run["build"]["commit"].is_string(), "{run}");
    assert!(run["armed"]["unix"].is_f64());
    assert!(run["start"]["unix"].is_f64());
    assert!(run["start"]["station_frame"].is_u64());
    assert_eq!(run["complete"], true);
    // 2. The station: the simulated plant and its seed, the rate.
    assert_eq!(run["sim"]["model"], "resonator_bank");
    assert!(run["sim"]["seed"].is_u64(), "{run}");
    assert_eq!(run["station"]["class"], "sim");
    assert_eq!(run["grid"]["fs_hz"], 4096);
    assert_eq!(run["recording"]["streams"], "on");
    let names: Vec<&str> = run["streams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["inputs", "drives"]);

    // 1. The channel table as used, every column.
    let table = read_json(&folder.join("table.json"));
    let now = c.ok("channels.get", json!({})).await;
    assert_eq!(table["rows"], now["channels"]);
    assert_eq!(table["revision"], now["revision"]);
    assert_eq!(table["sha256"].as_str().unwrap().len(), 64);

    // 3, 4, 6. The setup as accepted, whole, and what it resolved to: the profile
    // with its quantities, the sweep, the ladder, the limits and aborts, the notch,
    // the control settings and the feasibility inputs.
    let setup = read_json(&folder.join("setup.json"));
    assert_eq!(setup["config"], config);
    let r = &setup["resolved"];
    let points = r["profile"]["breakpoints"].as_array().unwrap();
    assert_eq!(points.len(), 2);
    assert_eq!(points[0]["quantity"], "accel_g_pk");
    assert_eq!(points[0]["accel_g_pk"], 0.2);
    assert_eq!(r["test"]["kind"], "sweep");
    assert_eq!(r["test"]["rate"]["law"], "log");
    assert_eq!(r["test"]["rate"]["oct_per_min"], 6);
    assert_eq!(r["test"]["direction"], "up");
    assert_eq!(r["test"]["sweeps"], 2);
    assert_eq!(r["schedule"]["levels_db"], json!([-6, 0]));
    assert_eq!(r["limits"][0]["channel"], 2);
    assert_eq!(r["aborts"]["drive"]["abort_vpk"], 8);
    assert!(r["aborts"]["notch"]["drive_floor"].is_number());
    assert!(r["control"]["law_budget"].is_number());
    assert!(r["controller"]["profile"].is_object());
    assert_eq!(setup["feasibility"]["verdict"], "not_evaluated");
    assert_eq!(setup["derived"]["grid"]["actual"], true);

    // 5. The control settings: the law's identity, parameters and guards.
    let law = read_json(&folder.join("law/law.json"));
    assert_eq!(law["id"], "servo_blend");
    assert_eq!(law["builtin"], true);
    assert!(law["guards"]["max_step_db"].is_number(), "{law}");

    // 7. The timeline: every entry the service gave, in order, with the station
    // frame and structured data for states, levels and actions.
    let log = read_lines(&folder.join("log.jsonl"));
    let wire = c.ok("sine.log", json!({"since": 0})).await;
    let wire = wire["entries"].as_array().unwrap();
    assert_eq!(log.len(), wire.len(), "{log:?}");
    for (a, b) in log.iter().zip(wire) {
        assert_eq!(
            (&a["seq"], &a["kind"], &a["text"]),
            (&b["seq"], &b["kind"], &b["text"])
        );
    }
    let levels: Vec<&Value> = log.iter().filter(|e| e["kind"] == "level").collect();
    assert!(!levels.is_empty(), "{log:?}");
    assert!(
        levels.iter().all(|e| e["data"]["level_db"].is_number()),
        "{levels:?}"
    );
    assert!(log
        .iter()
        .filter(|e| e["kind"] == "state")
        .skip(1)
        .all(|e| e["data"]["to"].is_string()));
    assert!(log.iter().any(|e| e["data"]["action"] == "start"));
    assert!(log
        .iter()
        .any(|e| e["kind"] == "gate" && e["data"]["passed"] == true));
    assert!(log.iter().any(|e| e["frame"].is_u64()));
    assert_eq!(log.last().unwrap()["data"]["outcome"], "completed");

    // 8. The data: every enabled input and the drive, in their units.
    let inputs = read_json(&folder.join("streams/inputs/stream.json"));
    let drives = read_json(&folder.join("streams/drives/stream.json"));
    assert_eq!(inputs["channels"][0]["index"], 1);
    assert_eq!(inputs["channels"][1]["index"], 2);
    assert_eq!(
        drives["channels"],
        json!([{"index": 3, "unit": "V", "description": "Drive"}])
    );
    assert_eq!(inputs["complete"], true);
    let frames = inputs["frames"].as_u64().unwrap();
    // the identification's burst alone is 131 072 frames
    assert!(frames > 131_072, "{inputs}");
    let size = std::fs::metadata(folder.join("streams/inputs/telemetry.f32"))
        .unwrap()
        .len();
    assert_eq!(size, frames * 2 * 4);
    assert_eq!(inputs["gaps"], json!([]));

    // 8, 10. The results, each array with a unit.
    for name in [
        "reference",
        "limit_ch2",
        "identification_frf",
        "identification_coherence",
        "channel_levels",
        "control_trace",
    ] {
        let side = read_json(&folder.join(format!("results/{name}.json")));
        for a in side["arrays"].as_array().unwrap() {
            assert!(a["unit"].is_string(), "{name}: {a}");
        }
    }
    let frf = read_json(&folder.join("results/identification_frf.json"));
    assert_eq!(frf["kind"], "frf");
    assert_eq!(frf["arrays"][0]["unit"], "g/V");
    let levels = read_json(&folder.join("results/channel_levels.json"));
    assert_eq!(levels["channels"]["responses"], json!([1, 2]));
    let kept = read_lines(&folder.join("results/channel_levels.jsonl"));
    assert!(!kept.is_empty());
    let trace = read_lines(&folder.join("results/control_trace.jsonl"));
    assert!(
        trace
            .iter()
            .map(|r| r["calls"].as_u64().unwrap())
            .sum::<u64>()
            > 10
    );

    // 7. The outcome with its reason.
    let ended = read_json(&folder.join("end.json"));
    assert_eq!(ended["end"], end["end"]);
    assert_eq!(ended["run_uid"], run["run_uid"]);
    assert_eq!(ended["streams"]["inputs"]["drops"], 0);
}

/// The arm gate: a record that cannot be written refuses the arm, naming the path
/// and the reason, and nothing is armed. Red if the arm went ahead without a record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_data_folder_that_cannot_be_written_refuses_the_sine_arm() {
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
    setup(&mut c).await;
    c.ok("sine.configure", json!({ "config": short_config() }))
        .await;
    let e = c.refused("sine.arm", json!({})).await;
    assert_eq!(e["reason"], "record_unwritable", "{e}");
    let message = e["message"].as_str().unwrap();
    assert!(
        message.contains(&blocked.display().to_string()),
        "{message}"
    );
    assert_eq!(e["detail"]["path"], blocked.display().to_string());
    assert!(!e["detail"]["reason"].as_str().unwrap().is_empty());
    let s = c.ok("sine.state", json!({})).await;
    assert_eq!(s["state"], "configured", "{s}");
    assert!(s.get("record").is_none(), "{s}");
    // sine.start arms first, and is refused the same way.
    let e = c.refused("sine.start", json!({})).await;
    assert_eq!(e["reason"], "record_unwritable", "{e}");
}

/// `record.streams: false` keeps the record and leaves out the streams, and the
/// manifest says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sine_record_streams_false_writes_the_record_without_streams() {
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
    assert!(folder.join("results/channel_levels.json").is_file());
    let log = read_lines(&folder.join("log.jsonl"));
    assert!(log.iter().any(|e| e["kind"] == "record"), "{log:?}");
}

/// Below the free-space floor at arm, the run goes on without its streams, and its
/// state, its log and its manifest say why.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn below_the_free_space_floor_the_sine_run_goes_on_without_streams() {
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
/// seconds before every write leaves the run's station time, its outcome and its
/// wall time as they are with no wait. The writer falls behind (the run ends before
/// its record does) and nothing reaches the loop. Simulated, paced at real time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_writer_stalled_on_every_write_leaves_the_sine_loop_and_the_outcome_unchanged() {
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
        setup(&mut c).await;
        let mut config = short_config();
        // A shorter run at real time: identification, then one sweep at each level.
        config["test"]["band_hz"] = json!([300, 340]);
        // At the simulator's own rate the identification's burst is eight seconds.
        config["acquisition"]["fs_hz"] = json!(16384);
        c.ok("sine.configure", json!({ "config": config })).await;
        c.ok("sine.arm", json!({})).await;
        let t0 = Instant::now();
        c.ok("sine.start", json!({})).await;
        let end = sine_state(&mut c, "finished").await;
        let wall = t0.elapsed().as_secs_f64();
        let folder = PathBuf::from(end["record"]["folder"].as_str().unwrap());
        (end, folder, wall)
    }
    let (free, _, free_wall) = timed(Duration::ZERO).await;
    let (stalled, folder, stalled_wall) = timed(Duration::from_secs(2)).await;
    println!(
        "no wait: ended at {} s of station time, elapsed {} s, wall {free_wall:.2} s; two \
         seconds a write: ended at {} s, elapsed {} s, wall {stalled_wall:.2} s",
        free["end"]["at_s"], free["elapsed_s"], stalled["end"]["at_s"], stalled["elapsed_s"]
    );
    assert_eq!(free["end"]["outcome"], "completed", "{free}");
    assert_eq!(
        stalled["end"]["outcome"], free["end"]["outcome"],
        "{stalled}"
    );
    assert_eq!(stalled["end"]["level_db"], free["end"]["level_db"]);
    let (a, b) = (
        free["end"]["at_s"].as_f64().unwrap(),
        stalled["end"]["at_s"].as_f64().unwrap(),
    );
    assert!((a - b).abs() < 0.5, "ended at {a} s against {b} s");
    assert!(
        (free_wall - stalled_wall).abs() < 1.0,
        "wall {free_wall} s against {stalled_wall} s"
    );
    // The stalled writer was still behind when the run ended.
    assert!(
        !folder.join("end.json").is_file(),
        "the stalled writer kept up, so nothing was shown"
    );
}
