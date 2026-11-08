//! TWR through the protocol, on a real loopback WebSocket, with the simulated TWR
//! engine: the reference import and preview, configure and its checks, the run from
//! identification through the review hold to the end, the manual verbs and their
//! refusals, the coherence hold and its only release, the controller rules, the run
//! exclusion against the other modules both ways, the shutdown, and plays held while
//! nobody holds control, then the orphan safe stop.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use support::schema::Schema;
use support::{options, serve, sim_config, Client};

const LONG: Duration = Duration::from_secs(30);

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

fn row(index: u32, role: &str, description: &str) -> Value {
    let units = if role == "drive" { "V" } else { "g" };
    json!({
        "index": index, "enabled": true, "description": description,
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": if units == "V" { 1000 } else { 100 },
        "range_max": if units == "V" { 10 } else { 100 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

fn table() -> Value {
    json!([row(1, "control", "Control"), row(2, "drive", "Shaker")])
}

/// A 200 Hz triangle, 2048 samples at 12800 Hz, starting at rest: a time column.
fn reference_csv() -> String {
    let mut s = String::from("time_s,accel_g\n");
    for i in 0..2048 {
        let k = ((i + 16) % 64) as f64;
        let v = 0.25 * (1.0 - (k - 32.0).abs() / 16.0);
        s.push_str(&format!("{},{v}\n", i as f64 / 12_800.0));
    }
    s
}

fn config(sha: &Value, mode: &str, review: bool) -> Value {
    json!({
        "title": "Protocol TWR",
        "reference": {"sha256": sha},
        "identification": {"duration_s": 0.5, "review": review},
        "schedule": {"mode": mode, "levels_db": [-12, -6, 0], "max_iterations": 4},
        "run": {"schedule": "loop", "count": 2},
        "limits": {"drive_vpk_ceiling": 0.5}
    })
}

async fn controller(c: &mut Client) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": table()}),
    )
    .await;
}

/// Import the triangle as pasted text; its SHA-256.
async fn import(c: &mut Client) -> Value {
    let text: String = reference_csv();
    c.ok("twr.reference.import", json!({"text": text})).await["reference"]["sha256"].clone()
}

async fn wait_twr(c: &mut Client, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let i = c
        .wait_for(what, LONG, |e| e.topic() == "twr.state" && pred(e.data()))
        .await;
    c.events[i].data().clone()
}

fn service(speed: f64, timeout: Duration) -> std::sync::Arc<Service> {
    Service::with_options(options(no_driver(), speed, timeout, Duration::from_secs(5)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_auto_run_goes_from_the_import_through_review_to_completed() {
    let schema = Schema::load();
    let svc = service(0.0, Duration::from_secs(30));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;

    let none = obs.ok("twr.state", json!({})).await;
    schema.assert_valid(&none, Some("twrState"));
    assert_eq!(none["state"], "unconfigured");

    // The import is open to every connection: by path (a time column) and pasted.
    let dir = std::env::temp_dir().join(format!("sds-twr-protocol-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("r1.csv");
    std::fs::write(&file, reference_csv()).unwrap();
    let by_path = obs
        .ok(
            "twr.reference.import",
            json!({"path": file.to_str().unwrap()}),
        )
        .await;
    schema.assert_valid(&by_path, Some("twrReferenceImportResult"));
    assert_eq!(by_path["reference"]["format"], "csv");
    assert_eq!(by_path["reference"]["fs_hz"], 12800);
    assert_eq!(by_path["reference"]["samples"], 2048);
    assert_eq!(by_path["reference"]["name"], "r1");
    let sha = import(&mut obs).await;
    assert_eq!(
        sha, by_path["reference"]["sha256"],
        "the same samples pasted and read from a file are one record"
    );
    let e = obs
        .refused("twr.reference.import", json!({"path": "r1.csv"}))
        .await;
    assert_eq!(e["reason"], "bad_params");
    let e = obs
        .refused("twr.reference.import", json!({"text": "a,b\nc,d\n"}))
        .await;
    assert_eq!(e["reason"], "reference_unreadable");

    // The preview needs no control and repeats the configuration's preparation.
    let preview = obs
        .ok(
            "twr.reference.preview",
            json!({"reference": {"sha256": sha}}),
        )
        .await;
    schema.assert_valid(&preview, Some("twrReferencePreviewResult"));
    assert_eq!(preview["derived"]["grid"]["record_len"], 4096);
    let titles: Vec<&str> = preview["views"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Section", "Spectrum", "Velocity", "Displacement"]);
    let e = obs
        .refused(
            "twr.reference.preview",
            json!({"reference": {"sha256": "0".repeat(64)}}),
        )
        .await;
    assert_eq!(e["reason"], "config_invalid");
    assert_eq!(e["detail"]["errors"][0]["reason"], "reference_unknown");

    // A dry run needs no control; configuring does.
    let dry = obs
        .ok(
            "twr.configure",
            json!({"config": config(&sha, "auto", true), "dry_run": true}),
        )
        .await;
    schema.assert_valid(&dry, Some("twrDryRunResult"));
    assert!(dry["derived"].is_object() && dry["errors"] == json!([]));
    assert_eq!(
        obs.refused(
            "twr.configure",
            json!({"config": config(&sha, "auto", true)})
        )
        .await["reason"],
        "not_controller"
    );
    let set = c
        .ok(
            "twr.configure",
            json!({"config": config(&sha, "auto", true)}),
        )
        .await;
    schema.assert_valid(&set, Some("twrConfigureResult"));
    assert_eq!(set["state"]["state"], "configured");
    assert_eq!(set["state"]["law"]["id"], "twr_correct");
    assert_eq!(set["derived"]["plays_target"], 2);
    let cfg = obs.ok("twr.config", json!({})).await;
    schema.assert_valid(&cfg, Some("twrConfigResult"));
    assert_eq!(cfg["run_id"], set["state"]["run_id"]);

    // A subscription before the run: data once identification ends, once a play.
    obs.ok(
        "twr.subscribe",
        json!({"plots": [{"id": "rep", "quantity": "response"},
                         {"id": "frf", "quantity": "frf"},
                         {"id": "plays", "quantity": "plays"}]}),
    )
    .await;

    // Start identifies, then holds for review; only the controller proceeds.
    assert_eq!(
        obs.refused("twr.start", json!({})).await["reason"],
        "not_controller"
    );
    let started = c.ok("twr.start", json!({})).await;
    schema.assert_valid(&started, Some("twrState"));
    let review = wait_twr(&mut c, "review", |d| d["state"] == "review").await;
    assert_eq!(review["phase"], "awaiting_review");
    assert_eq!(review["identification"]["passed"], true);
    assert_eq!(review["grid"]["actual"], true);
    assert_eq!(
        obs.refused("twr.proceed", json!({})).await["reason"],
        "not_controller"
    );
    assert_eq!(
        c.refused("twr.iterate", json!({})).await["reason"],
        "wrong_mode"
    );
    c.ok("twr.proceed", json!({})).await;
    let end = wait_twr(&mut c, "finished", |d| d["state"] == "finished").await;
    schema.assert_valid(&end, Some("twrState"));
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    // Three levels, each gating on its third correction play, then two playback plays.
    assert_eq!(end["plays"]["done"], 11, "{end}");
    assert_eq!(end["plays"]["playback"], 2);
    assert_eq!(end["end"]["play"], 10);
    assert!(end["t_s"].as_f64().unwrap() > 0.0);

    // Every play has a row, in order; the station's clock counts on.
    let plays = obs.ok("twr.plays", json!({})).await;
    schema.assert_valid(&plays, Some("twrPlaysResult"));
    let rows = plays["plays"].as_array().unwrap();
    assert_eq!(plays["next"], 11);
    let kinds: Vec<&str> = rows.iter().map(|r| r["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds[..9], ["iteration"; 9]);
    assert_eq!(kinds[9..], ["playback"; 2]);
    let levels: Vec<f64> = rows
        .iter()
        .map(|r| r["level_db"].as_f64().unwrap())
        .collect();
    assert_eq!(
        levels,
        [-12.0, -12.0, -12.0, -6.0, -6.0, -6.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    );
    let starts: Vec<u64> = rows
        .iter()
        .map(|r| r["start_frame"].as_u64().unwrap())
        .collect();
    assert!(starts.windows(2).all(|w| w[1] > w[0]), "{starts:?}");
    let later = obs.ok("twr.plays", json!({"since": 9})).await;
    assert_eq!(later["plays"].as_array().unwrap().len(), 2);

    // A play's records: full resolution, decimated, and gone once older than eight.
    let one = obs
        .ok("twr.record", json!({"index": 10, "points": 0}))
        .await;
    schema.assert_valid(&one, Some("twrRecordResult"));
    let titles: Vec<&str> = one["views"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Replication", "Error", "Drive"]);
    let id = Value::from(obs.last_id);
    let decoded = obs
        .decoded_response(&id)
        .expect("twr.record is a binary frame");
    let y = decoded
        .array(&decoded.message["result"]["views"][0]["series"][0]["y"])
        .unwrap();
    assert_eq!(y.to_f64().len(), 4096, "0 points sends every sample");
    let e = obs.refused("twr.record", json!({"index": 1})).await;
    assert_eq!(e["code"], "not_found");
    let e = obs.refused("twr.record", json!({"index": 11})).await;
    assert_eq!(e["code"], "not_found");

    // The events: a state on each change; live frames to the screen without a
    // subscription of its own and the data to the one with; the log.
    obs.pump(Duration::from_millis(300)).await;
    c.pump(Duration::from_millis(100)).await;
    for ev in obs.of_topic("twr.state") {
        schema.assert_valid(ev.data(), Some("twrState"));
    }
    let states = obs.of_topic("twr.state");
    let judged: std::collections::BTreeSet<u64> = states
        .iter()
        .filter_map(|e| e.data()["last_play"]["index"].as_u64())
        .collect();
    assert_eq!(judged.len(), 11, "a state event follows every judged play");
    assert!(
        obs.of_topic("twr.live").is_empty(),
        "a subscribed screen gets twr.data, not twr.live"
    );
    let live = c.of_topic("twr.live");
    assert!(!live.is_empty());
    for ev in &live {
        assert!(ev.decoded.is_some(), "twr.live is a binary frame");
        schema.assert_valid(ev.data(), Some("twrLive"));
    }
    assert_eq!(live.last().unwrap().data()["state"], "finished");
    let data = obs.of_topic("twr.data");
    assert!(
        data.len() >= 2,
        "once identification ended and once after plays"
    );
    for ev in &data {
        schema.assert_valid(ev.data(), Some("twrData"));
    }
    let last = data.last().unwrap().data();
    assert_eq!(last["plots"][2]["views"][0]["view"], "table");
    assert_eq!(last["plots"][1]["views"].as_array().unwrap().len(), 2);
    let log = obs.ok("twr.log", json!({})).await;
    assert!(log["next"].as_u64().unwrap() > 3);

    // A finished run arms again only after a reset; clear forgets it.
    assert_eq!(
        c.refused("twr.arm", json!({})).await["reason"],
        "run_finished"
    );
    let reset = c.ok("twr.reset", json!({})).await;
    assert_eq!(reset["state"], "configured");
    assert_ne!(reset["run_id"], set["state"]["run_id"]);
    let cleared = c.ok("twr.clear", json!({})).await;
    assert_eq!(cleared["state"], "unconfigured");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_configuration_with_errors_is_refused_with_its_items() {
    let schema = Schema::load();
    let svc = service(0.0, Duration::from_secs(30));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let sha = import(&mut c).await;
    let mut bad = config(&sha, "auto", true);
    bad["limits"]["drive_vpk_ceiling"] = json!(12);
    bad["schedule"]["levels_db"] = json!([-6, -12, 0]);
    bad["control"] = json!({"max_peak_rise_db": 5});
    bad["pads"] = json!({"pre": 600_000, "post": 600_000});
    let e = c.refused("twr.configure", json!({"config": bad})).await;
    assert_eq!(e["reason"], "config_invalid");
    schema.assert_valid(&e["detail"], Some("report"));
    let fields: Vec<(&str, &str)> = e["detail"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["reason"].as_str().unwrap(), i["field"].as_str().unwrap()))
        .collect();
    for want in [
        ("levels_not_ascending", "schedule.levels_db[1]"),
        ("out_of_range", "limits.drive_vpk_ceiling"),
        ("out_of_range", "control.max_peak_rise_db"),
        ("record_too_long", "section.len"),
    ] {
        assert!(fields.contains(&want), "{want:?} not in {fields:?}");
    }
    // An unknown reference, a reference in another unit, and a changed file.
    let mut unknown = config(&sha, "auto", true);
    unknown["reference"]["sha256"] = json!("a".repeat(64));
    let e = c.refused("twr.configure", json!({"config": unknown})).await;
    assert_eq!(e["detail"]["errors"][0]["reason"], "reference_unknown");
    let ips = c
        .ok(
            "twr.reference.import",
            json!({"text": reference_csv(), "units": "in/s"}),
        )
        .await["reference"]["sha256"]
        .clone();
    assert_ne!(ips, sha, "the unit is part of the record");
    let e = c
        .refused(
            "twr.configure",
            json!({"config": config(&ips, "auto", true)}),
        )
        .await;
    assert_eq!(e["detail"]["errors"][0]["reason"], "units_mismatch", "{e}");
    let dir = std::env::temp_dir().join(format!("sds-twr-changed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("r1.csv");
    std::fs::write(&file, reference_csv()).unwrap();
    let path = file.to_str().unwrap().to_owned();
    let from_file =
        c.ok("twr.reference.import", json!({"path": path})).await["reference"]["sha256"].clone();
    let mut named = config(&from_file, "auto", true);
    named["reference"]["path"] = json!(path);
    c.ok(
        "twr.configure",
        json!({"config": named.clone(), "dry_run": true}),
    )
    .await;
    std::fs::write(&file, reference_csv().replace("0.25\n", "0.5\n")).unwrap();
    let e = c.refused("twr.configure", json!({"config": named})).await;
    assert_eq!(
        e["detail"]["errors"][0]["reason"], "reference_changed",
        "{e}"
    );
    let _ = std::fs::remove_dir_all(dir);
    // A wrong shape is refused naming the field.
    let mut shape = config(&sha, "auto", true);
    shape["run"] = json!({"schedule": "sometimes"});
    let e = c.refused("twr.configure", json!({"config": shape})).await;
    assert_eq!(e["reason"], "bad_params");
    assert_eq!(e["detail"]["param"], "config.run.schedule");
    let mut no_ceiling = config(&sha, "auto", true);
    no_ceiling["limits"] = json!({});
    let e = c
        .refused("twr.configure", json!({"config": no_ceiling}))
        .await;
    assert_eq!(e["detail"]["param"], "config.limits.drive_vpk_ceiling");
    // A Random law is not a TWR law.
    let mut law = config(&sha, "auto", true);
    law["control"] = json!({"law": {"id": "pinv_closed"}});
    let e = c.refused("twr.configure", json!({"config": law})).await;
    assert_eq!(e["detail"]["errors"][0]["reason"], "law_wrong_world", "{e}");
    // Nothing was configured.
    assert_eq!(c.ok("twr.state", json!({})).await["state"], "unconfigured");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_verbs_and_their_refusals_then_the_stop_mid_play() {
    let schema = Schema::load();
    // Played at the simulator's pace: a play is 0.4 s, so a stop lands inside one.
    let svc = service(1.0, Duration::from_secs(30));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    let sha = import(&mut c).await;
    let mut cfg = config(&sha, "manual", false);
    cfg["identification"]["duration_s"] = json!(0.4);
    c.ok("twr.configure", json!({"config": cfg})).await;
    c.ok("twr.start", json!({})).await;
    let waiting = wait_twr(&mut c, "waiting", |d| {
        d["state"] == "running" && d["phase"] == "waiting"
    })
    .await;
    assert_eq!(waiting["level_db"], -12);
    assert_eq!(waiting["rung"], 0);

    // The observer may not play or move the level.
    for m in ["twr.iterate", "twr.play"] {
        assert_eq!(
            obs.refused(m, json!({})).await["reason"],
            "not_controller",
            "{m}"
        );
    }
    assert_eq!(
        obs.refused("twr.level", json!({"delta_db": 1})).await["reason"],
        "not_controller"
    );

    // Playback before the level gates is refused by name.
    let e = c.refused("twr.play", json!({})).await;
    assert_eq!(e["reason"], "not_converged", "{e}");

    // One correction play; a second one while it plays is refused.
    c.ok("twr.iterate", json!({})).await;
    let e = c.refused("twr.iterate", json!({})).await;
    assert_eq!(e["reason"], "wrong_state", "{e}");
    let one = wait_twr(&mut c, "one play", |d| {
        d["plays"]["done"] == 1 && d["phase"] == "waiting"
    })
    .await;
    assert_eq!(one["last_play"]["iteration"], 1);
    assert_eq!(one["last_play"]["gated"], false);

    // Level moves: one step at a time, a rung, never past the table, never past the
    // ceiling.
    assert_eq!(
        c.ok("twr.level", json!({"delta_db": 1})).await["level_db"],
        -11
    );
    for bad in [
        json!({"delta_db": 2}),
        json!({"delta_db": 1, "rung": 1}),
        json!({}),
    ] {
        assert_eq!(c.refused("twr.level", bad).await["reason"], "bad_params");
    }
    assert_eq!(
        c.refused("twr.level", json!({"rung": 9})).await["reason"],
        "not_a_rung"
    );
    assert_eq!(c.ok("twr.level", json!({"rung": 2})).await["level_db"], 0);
    assert_eq!(
        c.refused("twr.level", json!({"delta_db": 1})).await["reason"],
        "not_a_rung"
    );

    // Three correction plays gate the level; then playback; freeze answers at once.
    for n in 2..=4 {
        c.ok("twr.iterate", json!({})).await;
        wait_twr(&mut c, "a play", |d| {
            d["plays"]["done"] == n && d["phase"] == "waiting"
        })
        .await;
    }
    let frozen = c.ok("twr.freeze", json!({"frozen": true})).await;
    assert_eq!(frozen, json!({"frozen": true}));
    let f = wait_twr(&mut c, "frozen", |d| d["frozen"] == true).await;
    schema.assert_valid(&f, Some("twrState"));
    c.ok("twr.play", json!({})).await;
    wait_twr(&mut c, "a playback play", |d| d["phase"] == "playing").await;
    assert_eq!(
        c.refused("twr.iterate", json!({})).await["reason"],
        "wrong_state"
    );

    // An observer stops inside the play: it ends at its next block.
    obs.wait_for("frames into the playback play", LONG, |e| {
        e.topic() == "twr.live"
            && e.data()["play"]["kind"] == "playback"
            && e.data()["play"]["frames_done"].as_u64().unwrap_or(0) > 1000
    })
    .await;
    let stopped = obs.ok("twr.stop", json!({})).await;
    assert!(
        ["stopping", "finished"].contains(&stopped["state"].as_str().unwrap()),
        "{stopped}"
    );
    let end = wait_twr(&mut c, "finished", |d| d["state"] == "finished").await;
    schema.assert_valid(&end, Some("twrState"));
    assert_eq!(end["end"]["outcome"], "operator_stop", "{end}");
    assert_eq!(end["end"]["play"], 4);
    let done = end["end"]["frames_done"].as_u64().unwrap();
    assert!(done > 1000 && done < 5120, "{done}");
    let plays = c.ok("twr.plays", json!({})).await;
    assert_eq!(plays["next"], 4, "the cancelled play has no row");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_observer_aborts_inside_a_play() {
    let svc = service(1.0, Duration::from_secs(30));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    let sha = import(&mut c).await;
    let mut cfg = config(&sha, "auto", false);
    cfg["identification"]["duration_s"] = json!(0.4);
    c.ok("twr.configure", json!({"config": cfg})).await;
    c.ok("twr.start", json!({})).await;
    obs.wait_for("frames into a play", LONG, |e| {
        e.topic() == "twr.live" && e.data()["play"]["frames_done"].as_u64().unwrap_or(0) > 1000
    })
    .await;
    let asked = Instant::now();
    obs.ok("twr.abort", json!({})).await;
    let end = wait_twr(&mut obs, "finished", |d| d["state"] == "finished").await;
    assert!(asked.elapsed() < Duration::from_secs(2));
    assert_eq!(end["end"]["outcome"], "operator_abort", "{end}");
    assert_eq!(end["phase"], "aborted");
    assert!(end["end"]["frames_done"].as_u64().is_some(), "{end}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_coherence_hold_holds_until_a_reset_releases_it() {
    let schema = Schema::load();
    let svc = service(0.0, Duration::from_secs(30));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let sha = import(&mut c).await;
    let mut cfg = config(&sha, "manual", true);
    cfg["identification"]["drive_vrms"] = json!(0.00005);
    let set = c.ok("twr.configure", json!({"config": cfg})).await;
    c.ok("twr.start", json!({})).await;
    let held = wait_twr(&mut c, "held", |d| d["state"] == "held").await;
    schema.assert_valid(&held, Some("twrState"));
    assert_eq!(held["phase"], "coherence_hold");
    assert_eq!(held["identification"]["passed"], false);
    for m in ["twr.proceed", "twr.iterate", "twr.play"] {
        let e = c.refused(m, json!({})).await;
        assert_eq!(e["reason"], "coherence_hold", "{m}: {e}");
    }
    // Nothing plays and the hold is live: another module may not start.
    assert_eq!(
        c.refused(
            "random.configure",
            json!({"config": sim_config("auto", json!([{"db": 0}]), 1.0)})
        )
        .await["reason"],
        "run_active"
    );
    let reset = c.ok("twr.reset", json!({})).await;
    assert_eq!(reset["state"], "configured");
    assert_ne!(reset["run_id"], set["state"]["run_id"]);
    assert_eq!(c.ok("twr.plays", json!({})).await["next"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_twr_run_and_every_other_module_exclude_each_other() {
    let svc = service(0.0, Duration::from_secs(30));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let sha = import(&mut c).await;
    // A TWR run held for review is live: Random, shock, SRS and Sine are refused.
    c.ok(
        "twr.configure",
        json!({"config": config(&sha, "manual", true)}),
    )
    .await;
    c.ok("twr.start", json!({})).await;
    wait_twr(&mut c, "review", |d| d["state"] == "review").await;
    let e = c
        .refused(
            "random.configure",
            json!({"config": sim_config("auto", json!([{"db": 0}]), 1.0)}),
        )
        .await;
    assert_eq!(e["reason"], "run_active", "{e}");
    assert!(e["message"].as_str().unwrap().contains("TWR run"), "{e}");
    let shock = json!({
        "pulse": {"shape": "half_sine", "amplitude_g": 20, "duration_ms": 11},
        "acquisition": {"fs_hz": 20480},
        "schedule": {"mode": "manual", "levels_db": [0], "pulses": [1]},
        "limits": {"drive_vpk_ceiling": 0.5, "drive_vpk_abort": 0.8}
    });
    assert_eq!(
        c.refused("shock.configure", json!({"config": shock.clone()}))
            .await["reason"],
        "run_active"
    );
    assert_eq!(
        c.refused("sim.start", json!({"profile": "ramp_plateau"}))
            .await["reason"],
        "run_active"
    );
    // A second configure is refused while the run is live; stop it.
    assert_eq!(
        c.refused(
            "twr.configure",
            json!({"config": config(&sha, "auto", true)})
        )
        .await["reason"],
        "run_active"
    );
    c.ok("twr.stop", json!({})).await;
    wait_twr(&mut c, "finished", |d| d["state"] == "finished").await;

    // And the other way: a live shock run refuses a TWR configure and arm.
    c.ok("shock.configure", json!({"config": shock})).await;
    c.ok("shock.arm", json!({})).await;
    assert_eq!(
        c.refused(
            "twr.configure",
            json!({"config": config(&sha, "auto", true)})
        )
        .await["reason"],
        "run_active"
    );
    assert_eq!(
        c.refused("twr.reset", json!({})).await["reason"],
        "run_active"
    );
    c.ok("shock.abort", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_refuses_the_drive_verbs_and_safe_stops_the_run() {
    let svc = service(0.0, Duration::from_secs(30));
    let addr = serve(svc.clone()).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let sha = import(&mut c).await;
    c.ok(
        "twr.configure",
        json!({"config": config(&sha, "manual", false)}),
    )
    .await;
    c.ok("twr.start", json!({})).await;
    wait_twr(&mut c, "waiting", |d| d["phase"] == "waiting").await;
    let said = svc.shutdown().await;
    assert!(said.is_some_and(|s| s.contains("safe_stop")));
    for (m, p) in [
        ("twr.arm", json!({})),
        ("twr.start", json!({})),
        ("twr.proceed", json!({})),
        ("twr.iterate", json!({})),
        ("twr.play", json!({})),
        ("twr.level", json!({"delta_db": 1})),
    ] {
        assert_eq!(c.refused(m, p).await["reason"], "shutting_down", "{m}");
    }
    let s = c.ok("twr.state", json!({})).await;
    assert_eq!(s["end"]["outcome"], "safe_stop");
    assert!(s["end"]["message"].as_str().unwrap().contains("shut down"));
    // Stop, abort and reading still answer.
    c.ok("twr.stop", json!({})).await;
    c.ok("twr.plays", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plays_hold_when_the_controller_goes_resume_on_a_reclaim_and_safe_stop_after() {
    let schema = Schema::load();
    let timeout = Duration::from_millis(2500);
    let svc = service(1.0, timeout);
    let addr = serve(svc).await;
    let mut obs = Client::connect(addr).await;

    let mut c = Client::connect(addr).await;
    let lease = c.ok("session.claim", json!({"name": "Shell"})).await["lease"].clone();
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": table()}),
    )
    .await;
    let sha = import(&mut c).await;
    let mut cfg = config(&sha, "auto", false);
    cfg["identification"]["duration_s"] = json!(0.4);
    c.ok("twr.configure", json!({"config": cfg.clone()})).await;
    c.ok("twr.start", json!({})).await;
    obs.wait_for("a play", LONG, |e| {
        e.topic() == "twr.state" && e.data()["plays"]["done"] == 1
    })
    .await;
    drop(c);
    let dropped_at = Instant::now();
    // The play in flight finishes; none starts after it.
    obs.pump(Duration::from_millis(700)).await;
    let held = obs.ok("twr.plays", json!({})).await["next"]
        .as_u64()
        .unwrap();
    obs.pump(Duration::from_millis(500)).await;
    assert_eq!(
        obs.ok("twr.plays", json!({})).await["next"]
            .as_u64()
            .unwrap(),
        held,
        "no play while nobody holds control"
    );
    assert!(held <= 2, "at most the play in flight finished: {held}");
    let s = obs.ok("twr.state", json!({})).await;
    schema.assert_valid(&s, Some("twrState"));
    assert_eq!(s["state"], "running", "{s}");
    assert_eq!(s["phase"], "waiting", "{s}");
    assert!(dropped_at.elapsed() < timeout);

    // A reclaim within the grace resumes the run.
    let mut c2 = Client::connect(addr).await;
    let back = c2
        .ok("session.claim", json!({"name": "Shell", "lease": lease}))
        .await;
    assert_eq!(back["resumed"], true, "{back}");
    let end = wait_twr(&mut c2, "finished", |d| d["state"] == "finished").await;
    assert_eq!(end["end"]["outcome"], "completed", "{end}");

    // Without a reclaim the orphan clock safe-stops it, mid-play or between.
    c2.ok("twr.configure", json!({"config": cfg})).await;
    c2.ok("twr.start", json!({})).await;
    obs.wait_for("a play of the second run", LONG, |e| {
        e.topic() == "twr.state"
            && e.data()["run_id"] != end["run_id"]
            && e.data()["plays"]["done"] == 1
    })
    .await;
    drop(c2);
    let i = obs
        .wait_for("the safe stop", LONG, |e| {
            e.topic() == "twr.state"
                && e.data()["run_id"] != end["run_id"]
                && e.data()["state"] == "finished"
        })
        .await;
    let fin = obs.events[i].data().clone();
    assert_eq!(fin["end"]["outcome"], "safe_stop", "{fin}");
    assert!(fin["end"]["message"]
        .as_str()
        .unwrap()
        .contains("Nobody held control"));
    assert!(fin["plays"]["done"].as_u64().unwrap() <= 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_level_moves_from_review_in_manual_and_from_a_held_gate_in_auto() {
    let svc = service(0.0, Duration::from_secs(30));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let sha = import(&mut c).await;

    // Manual: the level is null until proceed; a move from review steps from the
    // table's first level.
    c.ok(
        "twr.configure",
        json!({"config": config(&sha, "manual", true)}),
    )
    .await;
    c.ok("twr.start", json!({})).await;
    let review = wait_twr(&mut c, "review", |d| d["state"] == "review").await;
    assert_eq!(review["level_db"], Value::Null, "{review}");
    assert_eq!(
        c.ok("twr.level", json!({"delta_db": 1})).await["level_db"],
        -11
    );
    c.ok("twr.proceed", json!({})).await;
    let w = wait_twr(&mut c, "waiting", |d| d["phase"] == "waiting").await;
    assert_eq!(w["level_db"], -11);
    assert_eq!(w["rung"], Value::Null, "off the table's levels");
    // Playback before the gate names the gate.
    let e = c.refused("twr.play", json!({})).await;
    assert_eq!(e["reason"], "not_converged");
    assert_eq!(e["detail"]["gate_pct"], 10);
    c.ok("twr.stop", json!({})).await;
    wait_twr(&mut c, "finished", |d| d["state"] == "finished").await;

    // Auto under the hold policy: a level that spends its plays without a gate
    // holds; only then does a level move, and the run goes on from it.
    let mut cfg = config(&sha, "auto", false);
    cfg["schedule"]["max_iterations"] = json!(1);
    cfg["schedule"]["on_gate_fail"] = json!("hold");
    let second = c.ok("twr.configure", json!({"config": cfg})).await["state"]["run_id"].clone();
    assert_eq!(
        c.refused("twr.level", json!({"delta_db": 1})).await["reason"],
        "wrong_mode",
        "in auto a level moves only from the held gate"
    );
    c.ok("twr.start", json!({})).await;
    let holding = wait_twr(&mut c, "holding", |d| {
        d["run_id"] == second && d["phase"] == "holding"
    })
    .await;
    assert_eq!(holding["plays"]["at_level"], 1, "{holding}");
    assert_eq!(
        c.refused("twr.iterate", json!({})).await["reason"],
        "wrong_mode"
    );
    assert_eq!(c.ok("twr.level", json!({"rung": 1})).await["level_db"], -6);
    let moved = wait_twr(&mut c, "a play at -6 dB", |d| {
        d["last_play"]["level_db"] == -6
    })
    .await;
    assert_eq!(moved["mode"], "auto");
    c.ok("twr.abort", json!({})).await;
    let end = wait_twr(&mut c, "finished", |d| {
        d["run_id"] == second && d["state"] == "finished"
    })
    .await;
    assert_eq!(end["end"]["outcome"], "operator_abort");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_coherence_hold_refusal_says_the_coherence_against_its_floor() {
    let svc = service(0.0, Duration::from_secs(30));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let sha = import(&mut c).await;
    let mut cfg = config(&sha, "manual", true);
    cfg["identification"]["drive_vrms"] = json!(0.00005);
    c.ok("twr.configure", json!({"config": cfg})).await;
    c.ok("twr.start", json!({})).await;
    wait_twr(&mut c, "held", |d| d["state"] == "held").await;
    let e = c.refused("twr.proceed", json!({})).await;
    assert_eq!(e["reason"], "coherence_hold");
    assert_eq!(e["detail"]["min_mean_coherence"], 0.8, "{e}");
    assert_eq!(e["detail"]["coherence_mean"], 0.5, "{e}");
    c.ok("twr.reset", json!({})).await;
}
