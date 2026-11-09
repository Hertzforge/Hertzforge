//! The Recorder end to end through the wire on the control service's simulated station,
//! the real core in the service's run, paced to real time, its inputs carrying the
//! stated source (schema/PROTOCOL.md, "The Recorder on the Simulator"). Judged on
//! station frames and the files the run writes, never on when a frame arrived.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::recorder::sim_signal::{source_value, SOURCE};
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::{options, serve, Client};

const FS: f64 = 12_800.0;
const LONG: Duration = Duration::from_secs(40);

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

fn row(index: u32, role: &str, range: f64, accel_limit: Option<f64>) -> Value {
    let units = if role == "drive" { "V" } else { "g" };
    json!({
        "index": index, "enabled": true, "description": format!("Row {index}"),
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": 1000,
        "range_max": range, "polarity": "positive", "control_group": null,
        "abort": {"accel_g": accel_limit, "velocity_ips": null, "displacement_mil": null}
    })
}

/// Four recorded rows at range 5 g and the table's drive row, as the bench's.
fn four_rows() -> Vec<Value> {
    vec![
        row(1, "control", 5.0, None),
        row(2, "monitor", 5.0, None),
        row(3, "monitor", 5.0, None),
        row(4, "monitor", 5.0, None),
        row(5, "drive", 10.0, None),
    ]
}

struct Rig {
    svc: Arc<Service>,
    c: Client,
    addr: std::net::SocketAddr,
}

async fn rig(rows: Vec<Value>, timeout: Duration) -> Rig {
    let svc = Service::with_options(options(
        no_driver(),
        1.0,
        timeout,
        Duration::from_millis(300),
    ));
    svc.start_background();
    let addr = serve(svc.clone()).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok("channels.set", json!({"revision": rev, "channels": rows}))
        .await;
    Rig { svc, c, addr }
}

/// Wait for a `recorder.state` event satisfying `pred`.
async fn state_where(
    c: &mut Client,
    what: &str,
    wait: Duration,
    pred: impl Fn(&Value) -> bool,
) -> Value {
    let i = c
        .wait_for(what, wait, |e| {
            e.topic() == "recorder.state" && pred(e.data())
        })
        .await;
    c.events[i].data().clone()
}

/// Poll `recorder.state` until `pred` holds.
async fn poll_state(c: &mut Client, wait: Duration, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + wait;
    loop {
        let s = c.ok("recorder.state", json!({})).await;
        if pred(&s) {
            return s;
        }
        assert!(Instant::now() < deadline, "state never came: {s}");
        c.pump(Duration::from_millis(50)).await;
    }
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// A recording's samples: `(rows, interleaved values)`.
fn samples(dir: &Path) -> (usize, Vec<f32>) {
    let rec = read_json(&dir.join("recorder.json"));
    let rows = rec["rows"].as_array().unwrap().len();
    let raw = std::fs::read(dir.join("telemetry.f32")).unwrap();
    let v = raw
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    (rows, v)
}

/// The folder a run names in its log ("records in <path>").
async fn records_folder(c: &mut Client) -> PathBuf {
    let log = c.ok("recorder.log", json!({})).await;
    let line = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|e| {
            e["text"]
                .as_str()?
                .strip_prefix("records in ")
                .map(str::to_owned)
        })
        .expect("the log names the run folder");
    PathBuf::from(line)
}

/// A least-squares fit of `a sin(2 pi f t) + b cos(2 pi f t) + c` at a known `f`: the
/// amplitude.
fn fit_amplitude(x: &[f64], f: f64, fs: f64, first: u64) -> f64 {
    let (mut ss, mut cc, mut sc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let mean = x.iter().sum::<f64>() / x.len() as f64;
    for (i, v) in x.iter().enumerate() {
        let t = (first + i as u64) as f64 / fs;
        let (s, c) = (2.0 * std::f64::consts::PI * f * t).sin_cos();
        ss += s * s;
        cc += c * c;
        sc += s * c;
        ys += (v - mean) * s;
        yc += (v - mean) * c;
    }
    let det = ss * cc - sc * sc;
    let a = (ys * cc - yc * sc) / det;
    let b = (yc * ss - ys * sc) / det;
    (a * a + b * b).sqrt()
}

/// sim-1: `start` mode, 12 800 Hz, a 10 s recording. One recording of exactly 128 000
/// frames from station frame 0; each row carries its part of the source at its
/// amplitude and frequency, in table order; row 1's markers exactly 25 600 frames
/// apart; `t_s` counts station frames.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recorder_sim_records_from_the_first_frame_in_start_mode() {
    let mut r = rig(four_rows(), Duration::from_secs(30)).await;
    let cfg = json!({
        "title": "Start mode", "acquisition": {"fs_hz": FS},
        "trigger": {"mode": "start", "record_s": 10},
        "capture": {"run_name": "sim1"},
    });
    let configured = r.c.ok("recorder.configure", json!({"config": cfg})).await;
    assert_eq!(configured["derived"]["outputs"], "none");
    assert_eq!(configured["warnings"][0]["reason"], "drive_channel_present");
    let started = r.c.ok("recorder.start", json!({})).await;
    assert_eq!(started["state"], "running");
    assert_eq!(started["phase"], "recording");
    assert_eq!(started["grid"]["fs_hz"], 12800);
    assert_eq!(started["grid"]["group_delay_frames"], 0);
    // The timed recording ends and the run returns to monitoring.
    let s = poll_state(&mut r.c, LONG, |s| {
        s["phase"] == "monitoring" && s["recordings"] == 1
    })
    .await;
    assert!(s["station_frames"].as_u64().unwrap() >= 128_000);
    assert_eq!(
        s["t_s"].as_f64().unwrap(),
        (s["station_frames"].as_f64().unwrap() / FS * 1000.0).round() / 1000.0
    );
    let stopped = r.c.ok("recorder.stop", json!({})).await;
    assert!(stopped["state"] == "stopping" || stopped["state"] == "finished");
    let end = state_where(&mut r.c, "finished", LONG, |s| s["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_stop");
    assert_eq!(end["end"]["recordings"], 1);

    let folder = records_folder(&mut r.c).await;
    let list = r.c.ok("recorder.recordings", json!({})).await;
    assert_eq!(list["next"], 1);
    let rec = &list["recordings"][0];
    assert_eq!(rec["frames"], 128_000, "{rec}");
    assert_eq!(rec["first_station_frame"], 0);
    assert_eq!(rec["trigger"], "start");
    assert_eq!(rec["complete"], true);
    assert_eq!(rec["gap_free"], true);
    let dir = folder.join(rec["folder"].as_str().unwrap());
    let (rows, v) = samples(&dir);
    assert_eq!(rows, 4);
    assert_eq!(v.len(), 128_000 * 4);
    // Rows 2 to 4: each its tone at its amplitude, within 0.1% (the noise is 2e-4).
    for (col, part) in SOURCE.iter().enumerate().skip(1) {
        let sds_control::recorder::sim_signal::Part::Tone(f, a) = *part else {
            unreachable!()
        };
        let x: Vec<f64> = (0..128_000).map(|i| f64::from(v[i * rows + col])).collect();
        let amp = fit_amplitude(&x, f, FS, 0);
        assert!((amp - a).abs() / a < 1e-3, "row {}: {amp} for {a}", col + 1);
        // And each sample is the source at its station frame, to the noise.
        let worst = (0..128_000)
            .map(|i| (x[i] - source_value(i as u64, col, FS)).abs())
            .fold(0.0f64, f64::max);
        assert!(
            worst < 2e-3,
            "row {} departs from the source by {worst}",
            col + 1
        );
    }
    // Row 1: a marker rising through 0.25 every 25 600 frames.
    let crossings: Vec<usize> = (1..128_000)
        .filter(|&i| v[(i - 1) * rows] < 0.25 && v[i * rows] >= 0.25)
        .collect();
    assert_eq!(crossings.len(), 5, "{crossings:?}");
    for w in crossings.windows(2) {
        assert_eq!(w[1] - w[0], 25_600, "{crossings:?}");
    }
    assert!(
        crossings[0] >= 6400 + 21 && crossings[0] <= 6400 + 23,
        "{crossings:?}"
    );
}

/// sim-2: `manual`, a 1 s pre-window: arm, 3 s, record, about 4 s, cut; then a record
/// 0.3 s after a second arm. The first recording's pre-window is exactly 12 800 frames
/// and joins the live frames with no gap; its frames are the cut's last frame less the
/// record's, plus the pre-window; the second's short pre-window is stated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recorder_sim_manual_recordings_carry_their_pre_windows() {
    let mut r = rig(four_rows(), Duration::from_secs(30)).await;
    let cfg = json!({
        "acquisition": {"fs_hz": FS}, "trigger": {"mode": "manual", "pre_s": 1.0},
        "capture": {"run_name": "sim2"},
    });
    r.c.ok("recorder.configure", json!({"config": cfg})).await;
    r.c.ok("recorder.start", json!({})).await;
    let armed = r.c.ok("recorder.arm", json!({})).await;
    assert_eq!(armed["pre_frames"], 12_800);
    r.c.pump(Duration::from_secs(3)).await;
    let rec = r.c.ok("recorder.record", json!({})).await;
    assert_eq!(rec["name"], "rec_0001");
    assert_eq!(rec["pre_frames"], 12_800);
    let record_frame = rec["station_frame"].as_u64().unwrap();
    r.c.pump(Duration::from_secs(4)).await;
    let cut = r.c.ok("recorder.cut", json!({})).await;
    assert_eq!(cut["name"], "rec_0001");
    let last = cut["last_station_frame"].as_u64().unwrap();
    // A cut leaves the run monitoring, not armed: a second arm, then a record soon.
    let s = r.c.ok("recorder.state", json!({})).await;
    assert_eq!(s["phase"], "monitoring");
    r.c.ok("recorder.arm", json!({})).await;
    r.c.pump(Duration::from_millis(300)).await;
    let rec2 = r.c.ok("recorder.record", json!({})).await;
    r.c.pump(Duration::from_millis(500)).await;
    r.c.ok("recorder.cut", json!({})).await;
    r.c.ok("recorder.stop", json!({})).await;
    state_where(&mut r.c, "finished", LONG, |s| s["state"] == "finished").await;

    let folder = records_folder(&mut r.c).await;
    let list = r.c.ok("recorder.recordings", json!({})).await;
    let one = &list["recordings"][0];
    assert_eq!(one["pre_frames"], 12_800);
    assert_eq!(one["first_station_frame"], record_frame - 12_800);
    assert_eq!(
        one["frames"].as_u64().unwrap(),
        last + 1 - record_frame + 12_800,
        "the cut's last frame (inclusive) less the record's, plus the pre-window: {one}"
    );
    // The file is the source at its station frames, across the join.
    let (rows, v) = samples(&folder.join(one["folder"].as_str().unwrap()));
    let first = one["first_station_frame"].as_u64().unwrap();
    let worst = (0..one["frames"].as_u64().unwrap() as usize)
        .map(|i| (f64::from(v[i * rows + 2]) - source_value(first + i as u64, 2, FS)).abs())
        .fold(0.0f64, f64::max);
    assert!(worst < 2e-3, "row 3 departs from the source by {worst}");
    let two = &list["recordings"][1];
    let short = two["pre_frames"].as_u64().unwrap();
    assert_eq!(two["pre_frames_asked"], 12_800);
    assert!(
        (3840 - 512..=3840 + 1024).contains(&short),
        "0.3 s of pre-window, give or take a batch: {short}"
    );
    assert_eq!(rec2["pre_frames"].as_u64().unwrap(), short);
}

/// sim-3: `level` on row 1 at 0.25 rising, a 0.5 s pre-window, 1 s recordings: three
/// recordings at three markers, each 19 200 frames, each trigger the first sample at or
/// over 0.25; then a 3 s holdoff: triggers 51 200 frames apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recorder_sim_level_trigger_records_at_the_markers() {
    let mut r = rig(four_rows(), Duration::from_secs(30)).await;
    let cfg = json!({
        "acquisition": {"fs_hz": FS},
        "trigger": {"mode": "level", "row": 1, "level": 0.25, "pre_s": 0.5, "record_s": 1.0,
                    "count": 3},
        "capture": {"run_name": "sim3"},
    });
    r.c.ok("recorder.configure", json!({"config": cfg})).await;
    r.c.ok("recorder.start", json!({})).await;
    // Armed after the first marker, so a full pre-window fills before the next.
    r.c.pump(Duration::from_secs(1)).await;
    r.c.ok("recorder.arm", json!({})).await;
    let s = poll_state(&mut r.c, LONG, |s| {
        s["recordings"] == 3 && s["phase"] == "monitoring"
    })
    .await;
    assert_eq!(s["trigger"]["fires"], 3);
    r.c.ok("recorder.stop", json!({})).await;
    state_where(&mut r.c, "finished", LONG, |s| s["state"] == "finished").await;
    let folder = records_folder(&mut r.c).await;
    let list = r.c.ok("recorder.recordings", json!({})).await;
    let mut triggers = Vec::new();
    for rec in list["recordings"].as_array().unwrap() {
        assert_eq!(rec["frames"], 19_200, "{rec}");
        assert_eq!(rec["pre_frames"], 6400);
        let t = rec["trigger_station_frame"].as_u64().unwrap();
        let (rows, v) = samples(&folder.join(rec["folder"].as_str().unwrap()));
        assert!(v[6400 * rows] >= 0.25 && v[6399 * rows] < 0.25, "{rec}");
        let _ = rows;
        triggers.push(t);
    }
    for w in triggers.windows(2) {
        assert_eq!(w[1] - w[0], 25_600, "{triggers:?}");
    }

    // With a 3 s holdoff the marker 2 s after a trigger is skipped.
    let cfg = json!({
        "acquisition": {"fs_hz": FS},
        "trigger": {"mode": "level", "row": 1, "level": 0.25, "pre_s": 0.5, "record_s": 1.0,
                    "count": 2, "holdoff_s": 3.0},
        "capture": {"run_name": "sim3b"},
    });
    r.c.ok("recorder.configure", json!({"config": cfg})).await;
    r.c.ok("recorder.start", json!({})).await;
    // Armed after the first marker, so a full pre-window fills before the next.
    r.c.pump(Duration::from_secs(1)).await;
    r.c.ok("recorder.arm", json!({})).await;
    poll_state(&mut r.c, LONG, |s| s["recordings"] == 2).await;
    r.c.ok("recorder.stop", json!({})).await;
    state_where(&mut r.c, "finished", LONG, |s| {
        s["state"] == "finished" && s["run_id"] == "recorder-0002"
    })
    .await;
    let list = r.c.ok("recorder.recordings", json!({})).await;
    let t: Vec<u64> = list["recordings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["trigger_station_frame"].as_u64().unwrap())
        .collect();
    assert_eq!(t[1] - t[0], 51_200, "{t:?}");
}

/// sim-4 (the floor): a floor above the free space refuses the start
/// `not_enough_space`, both numbers stated, no folder made.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recorder_sim_refuses_a_start_under_the_floor() {
    let mut r = rig(four_rows(), Duration::from_secs(30)).await;
    let cfg = json!({
        "acquisition": {"fs_hz": FS}, "trigger": {"mode": "start"},
        "capture": {"run_name": "sim4", "keep_free_gb": 1.0e9},
    });
    r.c.ok("recorder.configure", json!({"config": cfg})).await;
    let e = r.c.refused("recorder.start", json!({})).await;
    assert_eq!(e["reason"], "not_enough_space", "{e}");
    assert!(e["detail"]["free_bytes"].as_u64().is_some());
    assert!(
        e["detail"]["floor_bytes"].as_u64().unwrap() > e["detail"]["free_bytes"].as_u64().unwrap()
    );
    assert_eq!(
        r.c.ok("recorder.state", json!({})).await["state"],
        "configured"
    );
    let log = r.c.ok("recorder.log", json!({})).await;
    assert!(
        !log.to_string().contains("records in"),
        "no folder is made by a refusal"
    );
}

/// sim-5: the controlling connection closes mid-recording; the run goes on through the
/// grace, then ends `safe_stop` with the recording finalized. Separately, a shutdown
/// ends a run the same way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recorder_sim_a_lost_controller_and_a_shutdown_end_safely() {
    let mut r = rig(four_rows(), Duration::from_secs(2)).await;
    let cfg = json!({
        "acquisition": {"fs_hz": FS}, "trigger": {"mode": "start"},
        "capture": {"run_name": "sim5"},
    });
    r.c.ok("recorder.configure", json!({"config": cfg})).await;
    r.c.ok("recorder.start", json!({})).await;
    let mut observer = Client::connect(r.addr).await;
    r.c.pump(Duration::from_millis(500)).await;
    drop(r.c);
    let end = poll_state(&mut observer, LONG, |s| s["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "safe_stop", "{end}");
    assert!(end["end"]["message"]
        .as_str()
        .unwrap()
        .starts_with("Nobody held control"));
    let list = observer.ok("recorder.recordings", json!({})).await;
    assert_eq!(list["recordings"][0]["complete"], true);
    assert!(list["recordings"][0]["frames"].as_u64().unwrap() > 12_800);

    // A shutdown.
    let mut r = rig(four_rows(), Duration::from_secs(30)).await;
    r.c.ok("recorder.configure", json!({"config": cfg})).await;
    r.c.ok("recorder.start", json!({})).await;
    r.c.pump(Duration::from_millis(500)).await;
    let said = r
        .svc
        .shutdown()
        .await
        .expect("the shutdown names the run it ended");
    assert!(said.contains("safe_stop"), "{said}");
    let s = r.c.ok("recorder.state", json!({})).await;
    assert_eq!(s["end"]["outcome"], "safe_stop");
    assert!(s["end"]["message"].as_str().unwrap().contains("shut down"));
    assert_eq!(
        r.c.ok("recorder.recordings", json!({})).await["recordings"][0]["complete"],
        true
    );
}

/// sim-6: an observer aborts (any connection may); an observer's record is refused
/// `not_controller`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recorder_sim_an_observer_may_abort_but_not_record() {
    let mut r = rig(four_rows(), Duration::from_secs(30)).await;
    let cfg = json!({
        "acquisition": {"fs_hz": FS}, "trigger": {"mode": "manual"},
        "capture": {"run_name": "sim6"},
    });
    r.c.ok("recorder.configure", json!({"config": cfg})).await;
    r.c.ok("recorder.start", json!({})).await;
    r.c.ok("recorder.record", json!({})).await;
    let mut observer = Client::connect(r.addr).await;
    let e = observer.refused("recorder.record", json!({})).await;
    assert_eq!(e["reason"], "not_controller");
    let e = observer.refused("recorder.cut", json!({})).await;
    assert_eq!(e["reason"], "not_controller");
    r.c.pump(Duration::from_millis(500)).await;
    observer.ok("recorder.abort", json!({})).await;
    let end = poll_state(&mut observer, Duration::from_secs(10), |s| {
        s["state"] == "finished"
    })
    .await;
    assert_eq!(end["end"]["outcome"], "operator_abort");
    let list = observer.ok("recorder.recordings", json!({})).await;
    assert_eq!(
        list["recordings"][0]["complete"], true,
        "finalized, nothing discarded"
    );
}

/// sim-7: row 1's accel limit 0.4 g and its range 0.5 g (the indicator at 0.45 g): a
/// mark at each marker, the recording running its full length; row 1's overload latched
/// and no other row's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recorder_sim_limits_are_marks_and_the_recording_runs_on() {
    let rows = vec![
        row(1, "control", 0.5, Some(0.4)),
        row(2, "monitor", 5.0, Some(0.4)),
        row(3, "monitor", 5.0, None),
        row(4, "monitor", 5.0, None),
    ];
    let mut r = rig(rows, Duration::from_secs(30)).await;
    let cfg = json!({
        "acquisition": {"fs_hz": FS}, "trigger": {"mode": "start", "record_s": 5},
        "capture": {"run_name": "sim7"},
    });
    r.c.ok("recorder.configure", json!({"config": cfg})).await;
    r.c.ok("recorder.start", json!({})).await;
    poll_state(&mut r.c, LONG, |s| s["recordings"] == 1).await;
    r.c.ok("recorder.stop", json!({})).await;
    state_where(&mut r.c, "finished", LONG, |s| s["state"] == "finished").await;
    let list = r.c.ok("recorder.recordings", json!({})).await;
    assert_eq!(
        list["recordings"][0]["frames"], 64_000,
        "the recording ran its length"
    );
    let marks = r.c.ok("recorder.marks", json!({})).await;
    let limits: Vec<&Value> = marks["marks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["kind"] == "limit")
        .collect();
    assert_eq!(
        limits.len(),
        3,
        "a mark at each of the three markers: {marks}"
    );
    for m in &limits {
        assert_eq!(m["row"], 1);
        assert_eq!(m["recording"], "rec_0001");
    }
    for w in limits.windows(2) {
        assert_eq!(
            w[1]["station_frame"].as_u64().unwrap() - w[0]["station_frame"].as_u64().unwrap(),
            25_600
        );
    }
    let overloads: Vec<&Value> = marks["marks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["kind"] == "overload")
        .collect();
    assert_eq!(overloads.len(), 1);
    assert_eq!(overloads[0]["row"], 1);
}

/// sim-9: a Recorder run and an Explorer run refuse each other `run_active`, each
/// naming the other; and during a shutdown `recorder.start`, `arm` and `record` are
/// refused `shutting_down`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recorder_sim_runs_exclude_each_other_and_shutdown_refuses_starts() {
    let mut rows = four_rows();
    rows[4] = row(5, "drive", 10.0, None);
    let mut r = rig(rows, Duration::from_secs(30)).await;
    let explorer = json!({"acquisition": {"fs_hz": FS}, "band_hz": [20, 2000],
                          "start_hz": 250, "max_drive_vpk": 0.5});
    r.c.ok("explorer.configure", json!({"config": explorer}))
        .await;
    let rec = json!({"acquisition": {"fs_hz": FS}, "trigger": {"mode": "manual"},
                     "capture": {"run_name": "sim9"}});
    r.c.ok("recorder.configure", json!({"config": rec})).await;
    r.c.ok("recorder.start", json!({})).await;
    let e = r.c.refused("explorer.start", json!({})).await;
    assert_eq!(e["reason"], "run_active");
    assert!(e["message"].as_str().unwrap().contains("Recorder"), "{e}");
    r.c.ok("recorder.stop", json!({})).await;
    poll_state(&mut r.c, LONG, |s| s["state"] == "finished").await;
    r.c.ok("explorer.start", json!({})).await;
    let e = r.c.refused("recorder.reset", json!({})).await;
    assert_eq!(e["reason"], "run_active");
    r.c.ok("explorer.abort", json!({})).await;

    let svc = r.svc.clone();
    let down = tokio::spawn(async move { svc.shutdown().await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    for m in ["recorder.start", "recorder.arm", "recorder.record"] {
        let e = r.c.refused(m, json!({})).await;
        assert_eq!(e["reason"], "shutting_down", "{m}: {e}");
    }
    let _ = down.await;
}
