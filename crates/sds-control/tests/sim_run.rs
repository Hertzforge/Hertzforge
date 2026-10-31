//! Simulated runs end to end through the controller's random service and Sim plant.
//!
//! Every assertion here is about a SIMULATED run: the plant is `eigen-hal-sim`, and
//! nothing in this file is evidence about hardware.

use sds_control::codec::{self, Arrays};
use sds_control::profiles;
use sds_control::protocol;
use sds_control::sim::{Outcome, RunRequest, SimRun};
use serde_json::Value;
use std::time::{Duration, Instant};

fn request(profile: &str, level_db: f64, speed: f64, duration_s: f64) -> RunRequest {
    RunRequest {
        profile: profiles::find(profile).expect("profile exists"),
        level_db,
        speed,
        duration_s,
    }
}

/// Every `random.display` data object has the contract's keys, a spectrum with the
/// required roles on `link: frequency`, and a reading with the required rows.
fn check_display(data: &Value, arrays: &Arrays) {
    for key in [
        "run_id",
        "source",
        "state",
        "elapsed_s",
        "level_db",
        "views",
    ] {
        assert!(data.get(key).is_some(), "display data lacks {key}: {data}");
    }
    assert_eq!(
        data["source"], "SIM",
        "a simulated run is always marked SIM"
    );
    let state = data["state"].as_str().unwrap();
    assert!(state == "running" || state == "stopped", "{state}");
    let views = data["views"].as_array().unwrap();
    assert_eq!(views.len(), 2);

    let spectrum = &views[0];
    assert_eq!(spectrum["view"], "spectrum");
    assert_eq!(spectrum["link"], "frequency");
    assert_eq!(spectrum["x"]["scale"], "log");
    let series = spectrum["series"].as_array().unwrap();
    let roles: Vec<&str> = series.iter().map(|s| s["role"].as_str().unwrap()).collect();
    assert!(roles.contains(&"reference"));
    assert_eq!(
        roles.iter().filter(|r| **r == "tolerance").count(),
        2,
        "{roles:?}"
    );
    assert_eq!(
        roles.iter().filter(|r| **r == "abort").count(),
        2,
        "{roles:?}"
    );
    // Every series' x and y are attached arrays of one common length.
    let ids: std::collections::HashMap<&str, usize> =
        arrays.iter().map(|(id, a)| (id, a.data.len())).collect();
    let mut n = None;
    for s in series {
        for axis in ["x", "y"] {
            let id = codec::as_array_ref(&s[axis]).expect("series data is an array reference");
            let len = ids[id];
            assert!(len > 100, "{} has {len} lines", s["name"]);
            assert_eq!(*n.get_or_insert(len), len, "series lengths differ");
        }
    }

    let reading = &views[1];
    assert_eq!(reading["view"], "reading");
    let labels: Vec<&str> = reading["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["label"].as_str().unwrap())
        .collect();
    for want in [
        "Control rms",
        "Reference rms",
        "Drive rms",
        "Level",
        "Elapsed",
    ] {
        assert!(labels.contains(&want), "reading lacks {want}: {labels:?}");
    }
}

fn series_by_role<'a>(data: &'a Value, role: &str) -> Vec<&'a Value> {
    data["views"][0]["series"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["role"] == role)
        .collect()
}

#[test]
fn an_unpaced_run_completes_and_streams_well_formed_display_data() {
    let run = SimRun::start("t-complete".into(), request("ramp_plateau", 0.0, 0.0, 4.0))
        .expect("the run arms");
    let wall = Instant::now();
    let mut samples = Vec::new();
    while !run.is_finished() {
        let mut arrays = Arrays::new();
        let data = run.display_data(&mut arrays);
        check_display(&data, &arrays);
        samples.push(data);
        assert!(
            wall.elapsed() < Duration::from_secs(300),
            "run did not finish"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(run.wait(Duration::from_secs(5)));
    let wall_s = wall.elapsed().as_secs_f64();
    let sim_s = run.elapsed_s();
    println!(
        "unpaced SIM run: {sim_s:.2} simulated s in {wall_s:.2} wall s = {:.1}x real time \
         ({} build)",
        sim_s / wall_s,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );

    let mut arrays = Arrays::new();
    let last = run.display_data(&mut arrays);
    check_display(&last, &arrays);
    assert_eq!(last["state"], "stopped");
    assert_eq!(
        last["outcome"],
        Outcome::Completed.as_str(),
        "{}",
        last["reason"]
    );
    assert!(last["reason"].as_str().unwrap().contains("test complete"));
    assert!(
        (last["level_db"].as_f64().unwrap()).abs() < 1e-9,
        "ended at the requested level"
    );
    // Soft start (-21 dB), ladder (4 x 2 s), then 4 s at level: well over 10 s.
    assert!(sim_s > 10.0, "simulated {sim_s} s");

    // The final spectrum carries the measured control and the drive, and the control
    // sits on the reference: the in-band mean is within 1.5 dB.
    assert_eq!(series_by_role(&last, "measured").len(), 1);
    assert_eq!(series_by_role(&last, "drive").len(), 1);
    let decoded = codec::decode(
        &codec::encode(&protocol::event("random.display", 0, last.clone()), &arrays).unwrap(),
    )
    .unwrap();
    let y = |role: &str| {
        decoded
            .array(&series_by_role(&decoded.message["data"], role)[0]["y"])
            .unwrap()
            .to_f64()
    };
    let (ctrl, reference) = (y("measured"), y("reference"));
    let mean_db = 10.0 * (ctrl.iter().sum::<f64>() / reference.iter().sum::<f64>()).log10();
    println!("final SIM control vs reference, band mean: {mean_db:+.2} dB");
    assert!(
        mean_db.abs() < 1.5,
        "control is {mean_db:+.2} dB off the reference"
    );

    // Elapsed time never runs backwards.
    let elapsed: Vec<f64> = samples
        .iter()
        .map(|d| d["elapsed_s"].as_f64().unwrap())
        .collect();
    assert!(elapsed.windows(2).all(|w| w[1] >= w[0]), "{elapsed:?}");
}

#[test]
fn a_paced_run_tracks_wall_time_and_stops_on_request_with_a_reason() {
    let run = SimRun::start(
        "t-paced".into(),
        request("narrowband_peak", -3.0, 1.0, 60.0),
    )
    .expect("the run arms");
    let wall = Instant::now();
    std::thread::sleep(Duration::from_millis(3000));
    let sim_s = run.elapsed_s();
    let wall_s = wall.elapsed().as_secs_f64();
    println!("paced SIM run: {sim_s:.2} simulated s in {wall_s:.2} wall s");
    // Paced to real time: never ahead of the wall clock by more than one engine
    // block, and not far behind it either.
    assert!(
        sim_s <= wall_s + 0.05,
        "paced run ran ahead: {sim_s} s in {wall_s} s"
    );
    assert!(
        sim_s > 0.5 * wall_s,
        "paced run fell behind: {sim_s} s in {wall_s} s"
    );
    assert!(!run.is_finished());

    run.request_stop();
    assert!(run.wait(Duration::from_secs(10)), "stop took too long");
    let mut arrays = Arrays::new();
    let data = run.display_data(&mut arrays);
    check_display(&data, &arrays);
    assert_eq!(data["state"], "stopped");
    assert_eq!(data["outcome"], Outcome::OperatorStop.as_str());
    assert!(data["reason"].as_str().unwrap().contains("operator"));
}

#[test]
fn an_abort_ends_the_run_as_stopped_and_says_why() {
    // A drive ceiling far below what the profile needs holds the control under the
    // abort-low band, so the envelope protection must trip.
    let mut req = request("ramp_plateau", 0.0, 0.0, 30.0);
    req.profile.drive_ceiling_vrms = 0.2;
    let run = SimRun::start("t-abort".into(), req).expect("the run arms");
    assert!(run.wait(Duration::from_secs(300)), "run did not end");
    let mut arrays = Arrays::new();
    let data = run.display_data(&mut arrays);
    check_display(&data, &arrays);
    println!("abort reason: {}", data["reason"]);
    assert_eq!(data["state"], "stopped");
    assert_eq!(
        data["outcome"],
        Outcome::Aborted.as_str(),
        "{}",
        data["reason"]
    );
    assert!(
        data["reason"].as_str().unwrap().contains("abort"),
        "{}",
        data["reason"]
    );
    let rows = data["views"][1]["rows"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|r| r["label"] == "Stop reason" && r["status"] == "fail"),
        "the reading shows the abort as a failure: {rows:?}"
    );
}

#[test]
fn every_demo_profile_runs_to_completion_without_a_trip() {
    for p in profiles::all() {
        let run = SimRun::start(format!("t-{}", p.id), request(p.id, 0.0, 0.0, 3.0))
            .unwrap_or_else(|e| panic!("{} did not arm: {e}", p.id));
        assert!(
            run.wait(Duration::from_secs(300)),
            "{} did not finish",
            p.id
        );
        let mut arrays = Arrays::new();
        let data = run.display_data(&mut arrays);
        check_display(&data, &arrays);
        let rows = data["views"][1]["rows"].as_array().unwrap();
        let row = |label: &str| rows.iter().find(|r| r["label"] == label).unwrap()["value"].clone();
        println!(
            "{}: {} | control {} g, reference {} g, drive {} V",
            p.id,
            data["reason"],
            row("Control rms"),
            row("Reference rms"),
            row("Drive rms")
        );
        assert_eq!(
            data["outcome"],
            Outcome::Completed.as_str(),
            "{}: {}",
            p.id,
            data["reason"]
        );
        let (c, r) = (
            row("Control rms").as_f64().unwrap(),
            row("Reference rms").as_f64().unwrap(),
        );
        assert!(
            (20.0 * (c / r).log10()).abs() < 1.0,
            "{}: control {c} g vs reference {r} g",
            p.id
        );
    }
}

#[test]
fn bad_knobs_are_refused_by_name() {
    let e = SimRun::start("t-bad".into(), request("ramp_plateau", 12.0, 1.0, 10.0))
        .err()
        .unwrap();
    assert!(e.contains("level_db"), "{e}");
    let e = SimRun::start("t-bad".into(), request("ramp_plateau", 0.0, -1.0, 10.0))
        .err()
        .unwrap();
    assert!(e.contains("speed"), "{e}");
}
