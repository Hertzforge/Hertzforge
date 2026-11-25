//! MIMO Mixed Mode over the wire: the simulator's linear 4 by 4 sine structure as
//! a channel table, configurations for it built from the law check's own (so the
//! wire carries what the controller is known to run), and waits on the module's
//! events.
#![allow(dead_code)]

use super::{msine, Client};
use hertzforge_mimo_mixed_laws::{mimo_mixed_check_config, MimoMixedCheckCase};
use hertzforge_reference::HermitianStack;
use serde_json::{json, Map, Value};
use std::time::Duration;

#[allow(unused_imports)]
pub use msine::{no_driver, opts, paced, LONG};

/// The structure every run here plays on: the standard one of the 4 by 4 table,
/// which the station takes when its `sine_structure` is null.
pub const STRUCTURE: &str = "sine-linear-4x4";
/// The grid: 16384 Hz in frames of 4096 points (4 Hz lines).
pub const FS_HZ: f64 = 16_384.0;
pub const N_PTS: u64 = 4096;

/// Claim control, select the simulator and set the 4 by 4 table (controls 1 to
/// 4, drives 5 to 8).
pub async fn controller(c: &mut Client) {
    msine::controller(c, 4, 0, 4).await;
}

/// The inline reference: `level` g^2/Hz on every control from 20 to 1800 Hz,
/// coherence 0.5 between controls (the law check's reference).
pub fn reference(n: usize, level: f64) -> Value {
    let freqs = [20.0, 1800.0];
    let per = HermitianStack::packed_len(n);
    let mut re = vec![0.0; freqs.len() * per];
    for k in 0..freqs.len() {
        for r in 0..n {
            for c in r..n {
                let g = if r == c { 1.0 } else { 0.5 };
                re[k * per + HermitianStack::packed_index(n, r, c)] = g * level;
            }
        }
    }
    let im = vec![0.0; re.len()];
    json!({"kind": "inline", "freqs_hz": freqs, "n": n, "re": re, "im": im})
}

/// The controller's configuration of `case` as the wire carries it: the
/// reference inline, the grid in `estimation`, the record's words.
pub fn from_case(case: MimoMixedCheckCase) -> Value {
    let cfg = mimo_mixed_check_config(case);
    let n = cfg.controls.len();
    let mut v = serde_json::to_value(&cfg).expect("the configuration serializes");
    let o = v.as_object_mut().unwrap();
    o.remove("reference");
    // Random's capture rules hold an infinite period, which JSON cannot carry: the
    // wire leaves them out and takes their defaults.
    o.remove("capture");
    // The schedule's mode as the wire names it.
    let mode = o["schedule"]["mode"]
        .as_str()
        .unwrap_or("Auto")
        .to_lowercase();
    o["schedule"]["mode"] = json!(mode);
    let est = o
        .entry("estimation")
        .or_insert_with(|| Value::Object(Map::new()));
    est["fs_hz"] = json!(FS_HZ);
    est["n_pts"] = json!(N_PTS);
    o.insert("reference".into(), reference(n, 1e-3));
    o.insert("title".into(), json!("Four-Shaker Mixed Test"));
    o.insert("operator".into(), json!("A. Operator"));
    o.insert("article".into(), json!("Plate"));
    o.insert("notes".into(), json!(""));
    v
}

/// A floor, two fixed tones (150 and 700 Hz, 1 g) and a 40 Hz band at 300 Hz.
pub fn floor_tones_band() -> Value {
    let mut v = from_case(MimoMixedCheckCase::FixedTonesOnFloor);
    let band = from_case(MimoMixedCheckCase::NarrowbandFixed)["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "b1")
        .cloned()
        .expect("the check's band");
    v["parts"].as_array_mut().unwrap().push(band);
    v
}

/// `cfg` in manual mode: the level moves only by hand, from `start_db`.
pub fn manual(mut cfg: Value, start_db: f64) -> Value {
    cfg["schedule"]["mode"] = json!("manual");
    cfg["schedule"]["start_db"] = json!(start_db);
    cfg
}

/// `cfg` with a full level of `s` seconds.
pub fn full_level_s(mut cfg: Value, s: f64) -> Value {
    cfg["full_level_duration_s"] = json!(s);
    cfg
}

/// Wait for a `mimo_mixed.state` event in `state`.
pub async fn state(c: &mut Client, state: &str) -> Value {
    let i = c
        .wait_for(&format!("mimo_mixed.state {state}"), LONG, |e| {
            e.topic() == "mimo_mixed.state" && e.data()["state"] == state
        })
        .await;
    c.events[i].data().clone()
}

/// Wait for a `mimo_mixed.state` event in `phase`.
pub async fn phase(c: &mut Client, phase: &str) -> Value {
    let i = c
        .wait_for(&format!("mimo_mixed.state phase {phase}"), LONG, |e| {
            e.topic() == "mimo_mixed.state" && e.data()["phase"] == phase
        })
        .await;
    c.events[i].data().clone()
}

/// Wait until the state's `pred` holds, polling `mimo_mixed.state`.
pub async fn until(c: &mut Client, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = std::time::Instant::now() + LONG;
    loop {
        let s = c.ok("mimo_mixed.state", json!({})).await;
        if pred(&s) {
            return s;
        }
        if s["state"] == "finished" {
            panic!("the run finished before {what}: {}", s["end"]);
        }
        if std::time::Instant::now() > deadline {
            panic!("no {what} within {LONG:?}: {s}");
        }
        c.pump(Duration::from_millis(100)).await;
    }
}

/// Call `method`, see it refused by the name `reason`; the error.
pub async fn refused_as(c: &mut Client, method: &str, params: Value, reason: &str) -> Value {
    let e = c.refused(method, params).await;
    assert_eq!(e["reason"], reason, "{method}: {e}");
    e
}

/// The run folder of a state.
pub fn folder(state: &Value) -> std::path::PathBuf {
    msine::folder(state)
}

#[allow(unused_imports)]
pub use msine::complete;
