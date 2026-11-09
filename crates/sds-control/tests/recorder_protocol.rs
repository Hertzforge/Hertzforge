//! The Recorder's wire rules (schema/PROTOCOL.md, "Recorder"): the state machine and
//! every verb's refusal by name (`wrong_state` with `state` and `phase`, `wrong_mode`
//! with `mode`, `not_configured`, `run_finished`, `bad_params` with `param`); observers
//! may read, dry-run, stop and abort and are refused `not_controller` otherwise; the
//! log reaches every connection and names the run folder's absolute path; every state
//! and list answer validates against the schema.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use support::schema::Schema;
use support::{options, serve, Client};

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

fn row(index: u32, role: &str) -> Value {
    let units = if role == "drive" { "V" } else { "g" };
    json!({
        "index": index, "enabled": true, "description": format!("Row {index}"),
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": 1000, "range_max": 5,
        "polarity": "positive", "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

async fn rig() -> (Client, Client) {
    let svc = Service::with_options(options(
        no_driver(),
        1.0,
        Duration::from_secs(30),
        Duration::from_millis(300),
    ));
    svc.start_background();
    let addr = serve(svc.clone()).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": [row(1, "control"), row(2, "monitor"), row(3, "drive")]}),
    )
    .await;
    let observer = Client::connect(addr).await;
    (c, observer)
}

fn reason(e: &Value) -> &str {
    e["reason"].as_str().unwrap_or("")
}

async fn until(c: &mut Client, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let s = c.ok("recorder.state", json!({})).await;
        if pred(&s) {
            return s;
        }
        assert!(Instant::now() < deadline, "never: {s}");
        c.pump(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recorder_verbs_are_refused_by_name_in_every_state_and_mode() {
    let schema = Schema::load();
    let (mut c, mut obs) = rig().await;
    let e = c.refused("recorder.start", json!({})).await;
    assert_eq!(reason(&e), "not_configured");
    schema.assert_valid(
        &c.ok("recorder.state", json!({})).await,
        Some("recorderState"),
    );

    // An observer dry-runs and reads; it may not configure.
    let cfg = json!({"acquisition": {"fs_hz": 12800}, "trigger": {"mode": "manual", "pre_s": 0.5},
                     "capture": {"run_name": "protocol"}});
    let dry = obs
        .ok(
            "recorder.configure",
            json!({"config": cfg, "dry_run": true}),
        )
        .await;
    schema.assert_valid(&dry, Some("recorderDryRunResult"));
    let e = obs
        .refused("recorder.configure", json!({"config": cfg}))
        .await;
    assert_eq!(reason(&e), "not_controller");

    let r = c.ok("recorder.configure", json!({"config": cfg})).await;
    schema.assert_valid(&r, Some("recorderConfigureResult"));
    schema.assert_valid(
        &c.ok("recorder.config", json!({})).await,
        Some("recorderConfigResult"),
    );
    for verb in [
        "recorder.arm",
        "recorder.record",
        "recorder.cut",
        "recorder.disarm",
    ] {
        let e = c.refused(verb, json!({})).await;
        assert_eq!(reason(&e), "wrong_state", "{verb}: {e}");
        assert_eq!(e["detail"]["state"], "configured");
        assert_eq!(e["detail"]["phase"], "idle");
    }
    let e = c.refused("recorder.start", json!({"now": true})).await;
    assert_eq!(reason(&e), "bad_params");
    assert_eq!(e["detail"]["param"], "now");

    let started = c.ok("recorder.start", json!({})).await;
    schema.assert_valid(&started, Some("recorderState"));
    let e = c.refused("recorder.start", json!({})).await;
    assert_eq!(reason(&e), "wrong_state");
    let e = obs.refused("recorder.arm", json!({})).await;
    assert_eq!(reason(&e), "not_controller");
    let e = c.refused("recorder.cut", json!({})).await;
    assert_eq!(
        (reason(&e), e["detail"]["phase"].as_str()),
        ("wrong_state", Some("monitoring"))
    );
    let e = c.refused("recorder.disarm", json!({})).await;
    assert_eq!(reason(&e), "wrong_state");
    let armed = c.ok("recorder.arm", json!({})).await;
    schema.assert_valid(&armed, Some("recorderArmResult"));
    assert_eq!(armed["pre_frames"], 6400);
    assert_eq!(c.ok("recorder.state", json!({})).await["phase"], "armed");
    let e = c.refused("recorder.arm", json!({})).await;
    assert_eq!(reason(&e), "wrong_state");
    c.ok("recorder.disarm", json!({})).await;
    assert_eq!(
        c.ok("recorder.state", json!({})).await["phase"],
        "monitoring"
    );
    let rec = c.ok("recorder.record", json!({})).await;
    schema.assert_valid(&rec, Some("recorderRecordResult"));
    let e = c.refused("recorder.record", json!({})).await;
    assert_eq!(
        (reason(&e), e["detail"]["phase"].as_str()),
        ("wrong_state", Some("recording"))
    );
    c.pump(Duration::from_millis(300)).await;
    let cut = c.ok("recorder.cut", json!({})).await;
    schema.assert_valid(&cut, Some("recorderCutResult"));
    let e = c.refused("recorder.reset", json!({})).await;
    assert_eq!(reason(&e), "run_active");

    // Stop is open to every connection.
    obs.ok("recorder.stop", json!({})).await;
    let end = until(&mut c, |s| s["state"] == "finished").await;
    schema.assert_valid(&end, Some("recorderState"));
    assert_eq!(end["end"]["outcome"], "operator_stop");
    assert!(
        end["station_frames"].as_u64().unwrap() > 0,
        "the finished state keeps its frames"
    );
    let list = obs.ok("recorder.recordings", json!({})).await;
    schema.assert_valid(&list, Some("recorderRecordingsResult"));
    let marks = obs.ok("recorder.marks", json!({"since": 0})).await;
    schema.assert_valid(&marks, Some("recorderMarksResult"));
    let e = c.refused("recorder.start", json!({})).await;
    assert_eq!(reason(&e), "run_finished");
    let reset = c.ok("recorder.reset", json!({})).await;
    assert_eq!(reset["state"], "configured");
    let cleared = c.ok("recorder.clear", json!({})).await;
    assert_eq!(cleared["state"], "unconfigured");

    // Mode rules: start mode takes no arm or record; level mode no record or cut.
    let start = json!({"trigger": {"mode": "start"}, "capture": {"run_name": "protocol2"}});
    c.ok("recorder.configure", json!({"config": start})).await;
    c.ok("recorder.start", json!({})).await;
    for verb in ["recorder.arm", "recorder.record", "recorder.disarm"] {
        let e = c.refused(verb, json!({})).await;
        assert_eq!(reason(&e), "wrong_mode", "{verb}: {e}");
        assert_eq!(e["detail"]["mode"], "start");
    }
    c.ok("recorder.abort", json!({})).await;
    let end = until(&mut c, |s| s["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_abort");
    let level = json!({"trigger": {"mode": "level", "row": 1, "level": 0.25, "record_s": 1},
                       "capture": {"run_name": "protocol3"}});
    c.ok("recorder.configure", json!({"config": level})).await;
    c.ok("recorder.start", json!({})).await;
    for verb in ["recorder.record", "recorder.cut"] {
        let e = c.refused(verb, json!({})).await;
        assert_eq!(reason(&e), "wrong_mode", "{verb}: {e}");
        assert_eq!(e["detail"]["mode"], "level");
    }
    c.ok("recorder.stop", json!({})).await;
    until(&mut c, |s| s["state"] == "finished").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recorder_log_reaches_every_connection_and_names_the_folder() {
    let schema = Schema::load();
    let (mut c, mut obs) = rig().await;
    let cfg = json!({"trigger": {"mode": "start"}, "capture": {"run_name": "logged"}});
    c.ok("recorder.configure", json!({"config": cfg})).await;
    c.ok("recorder.start", json!({})).await;
    let i = obs
        .wait_for(
            "a recorder.log naming the folder",
            Duration::from_secs(10),
            |e| {
                e.topic() == "recorder.log"
                    && e.data()["entries"].as_array().is_some_and(|l| {
                        l.iter().any(|x| {
                            x["text"]
                                .as_str()
                                .is_some_and(|t| t.starts_with("records in "))
                        })
                    })
            },
        )
        .await;
    let entries = obs.events[i].data()["entries"].clone();
    let line = entries
        .as_array()
        .unwrap()
        .iter()
        .find_map(|x| {
            x["text"]
                .as_str()?
                .strip_prefix("records in ")
                .map(str::to_owned)
        })
        .unwrap();
    assert!(std::path::Path::new(&line).is_absolute(), "{line}");
    for e in entries.as_array().unwrap() {
        schema.assert_valid(e, Some("logEntry"));
    }
    // The first recording's line follows the folder's, once the recording opens: read the log
    // until it is there (one read could come between the two).
    let mut log = obs.ok("recorder.log", json!({})).await;
    for _ in 0..100 {
        let began = log["entries"].as_array().is_some_and(|l| {
            l.iter().any(|e| {
                e["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("rec_0001 began at station frame 0"))
            })
        });
        if began {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        log = obs.ok("recorder.log", json!({})).await;
    }
    let texts: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["text"].as_str())
        .collect();
    assert!(texts
        .iter()
        .any(|t| t.starts_with("the station programmed 12800 Hz (asked 12800 Hz)")));
    assert!(texts
        .iter()
        .any(|t| t.starts_with("outputs: none (drive rows 3 not opened)")));
    assert!(texts
        .iter()
        .any(|t| t.starts_with("rec_0001 began at station frame 0")));
    c.ok("recorder.stop", json!({})).await;
    until(&mut c, |s| s["state"] == "finished").await;
}
