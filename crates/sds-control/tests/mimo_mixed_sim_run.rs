//! MIMO Mixed Mode end to end on the simulator's linear 4 by 4 sine structure,
//! through the wire: configure, arm, identify, review, proceed with a floor, two
//! tones and a band, the level by hand to full and back, hold and resume, a part
//! switched off, and a stop, ending in a complete run folder; and a run in the
//! automatic schedule to completion at full level.
//!
//! The runs are SIMULATED; nothing here is evidence about hardware.

mod support;

use sds_control::server::Service;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use support::mmix::{self, controller, opts};
use support::{serve, Client};

async fn client() -> Client {
    let mut o = opts(Duration::from_secs(600));
    o.record.stream_buffer_s = 600.0;
    let addr = serve(Service::with_options(o)).await;
    Client::connect(addr).await
}

fn part<'a>(s: &'a Value, id: &str) -> &'a Value {
    s["parts"]
        .as_array()
        .and_then(|p| p.iter().find(|p| p["id"] == id))
        .unwrap_or_else(|| panic!("no part {id} in {}", s["parts"]))
}

fn json_at(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_mixed_runs_by_hand_through_the_wire_to_a_complete_folder() {
    let mut c = client().await;
    controller(&mut c).await;
    let cfg = mmix::manual(mmix::floor_tones_band(), -6.0);
    let dry = c
        .ok(
            "mimo_mixed.configure",
            json!({"config": cfg, "dry_run": true}),
        )
        .await;
    assert_eq!(dry["errors"], json!([]), "{dry}");
    let r = c.ok("mimo_mixed.configure", json!({"config": cfg})).await;
    assert_eq!(r["state"]["state"], "configured", "{r}");

    let s = c.ok("mimo_mixed.arm", json!({})).await;
    assert_eq!(s["state"], "armed", "{s}");
    c.ok("mimo_mixed.start", json!({})).await;
    let s = mmix::phase(&mut c, "awaiting_review").await;
    let id = &s["identification"];
    assert_eq!(id["passed"], true, "{id}");
    assert_eq!(id["band_coherence"].as_array().unwrap().len(), 4, "{id}");

    // Nothing but proceed moves the run on from the review.
    mmix::refused_as(&mut c, "mimo_mixed.hold", json!({}), "wrong_state").await;
    c.ok("mimo_mixed.proceed", json!({})).await;
    let s = mmix::phase(&mut c, "engaged").await;
    assert_eq!(s["mode"], "manual", "{s}");
    for p in ["t1", "t2", "b1"] {
        assert_eq!(part(&s, p)["enabled"], true, "{}", s["parts"]);
    }

    // The level by hand: to full, down 3 dB, back to full.
    let r = c.ok("mimo_mixed.level", json!({"to_db": 0.0})).await;
    assert_eq!(r["level_db"], 0.0, "{r}");
    mmix::until(&mut c, "full level", |s| s["level_db"] == 0.0).await;
    let r = c.ok("mimo_mixed.level", json!({"delta_db": -3.0})).await;
    assert_eq!(r["level_db"], -3.0, "{r}");
    mmix::until(&mut c, "-3 dB", |s| s["level_db"] == -3.0).await;
    c.ok("mimo_mixed.level", json!({"to_db": 0.0})).await;
    let s = mmix::until(&mut c, "full level again", |s| s["level_db"] == 0.0).await;
    assert!(
        s["floor"]["loops"].as_u64().unwrap_or(0) > 0,
        "{}",
        s["floor"]
    );

    // Hold and resume.
    c.ok("mimo_mixed.hold", json!({})).await;
    mmix::until(&mut c, "held", |s| s["held"] == true).await;
    c.ok("mimo_mixed.resume", json!({})).await;
    mmix::until(&mut c, "resumed", |s| s["held"] == false).await;

    // A tone off; the floor cannot be switched off.
    let r = c
        .ok("mimo_mixed.part", json!({"id": "t2", "on": false}))
        .await;
    assert_eq!(r, json!({"id": "t2", "on": false}));
    let s = mmix::until(&mut c, "t2 off", |s| part(s, "t2")["on"] == false).await;
    assert_eq!(part(&s, "t2")["enabled"], false, "{}", s["parts"]);
    assert_eq!(part(&s, "t1")["on"], true, "{}", s["parts"]);
    let e = mmix::refused_as(
        &mut c,
        "mimo_mixed.part",
        json!({"id": "nope", "on": false}),
        "part_refused",
    )
    .await;
    assert_eq!(e["detail"]["why"], "unknown_part", "{e}");

    c.ok("mimo_mixed.stop", json!({})).await;
    let end = mmix::state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_stop", "{}", end["end"]);
    let folder = mmix::folder(&end);
    let run = mmix::complete(&folder).await;
    assert_eq!(run["test"], "mimo_mixed", "{run}");

    // The record: setup at arm, the table, the laws and their calls, the results.
    let setup = json_at(&folder.join("setup.json"));
    assert_eq!(setup["config"]["schedule"]["mode"], "manual");
    assert_eq!(
        setup["resolved"]["law"]["floor"]["law"]["id"],
        "pinv_closed"
    );
    assert_eq!(setup["resolved"]["law"]["tones"]["law"]["id"], "matrix");
    assert!(folder.join("table.json").is_file());
    let law = json_at(&folder.join("law/law.json"));
    assert_eq!(
        law["calls"]["format"], "hertzforge.mimo_mixed.law_ticks.v1",
        "{law}"
    );
    for r in [
        "reference",
        "identification_frf",
        "identification_coherence",
        "control_spectra",
        "drive_spectra",
        "tone_levels",
        "band_levels",
        "drive_levels",
        "control_trace",
    ] {
        assert!(
            folder.join(format!("results/{r}.json")).is_file(),
            "no result {r} in {}",
            folder.display()
        );
    }
    let ended = json_at(&folder.join("end.json"));
    assert_eq!(ended["end"]["outcome"], "operator_stop", "{ended}");
    assert_eq!(ended["streams"]["inputs"]["drops"], 0, "{ended}");
    let log = std::fs::read_to_string(folder.join("log.jsonl")).unwrap();
    for word in ["level", "hold", "resume", "t2"] {
        assert!(log.contains(word), "the log never says {word}:\n{log}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_mixed_runs_its_schedule_to_completion_at_full_level() {
    let mut c = client().await;
    controller(&mut c).await;
    let cfg = mmix::full_level_s(mmix::floor_tones_band(), 2.0);
    c.ok("mimo_mixed.configure", json!({"config": cfg})).await;
    c.ok("mimo_mixed.start", json!({})).await;
    mmix::phase(&mut c, "awaiting_review").await;
    c.ok("mimo_mixed.proceed", json!({})).await;
    let s = mmix::phase(&mut c, "full_level").await;
    assert_eq!(s["mode"], "auto", "{s}");
    // In the automatic schedule the level is the schedule's.
    mmix::refused_as(
        &mut c,
        "mimo_mixed.level",
        json!({"to_db": -3.0}),
        "manual_mode_required",
    )
    .await;
    let end = mmix::state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "completed", "{}", end["end"]);
    let run = mmix::complete(&mmix::folder(&end)).await;
    assert_eq!(run["complete"], true);
}
