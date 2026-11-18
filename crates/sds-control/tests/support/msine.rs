//! MIMO Sine over the wire: the simulator's sine structures as channel tables,
//! configurations for them, and waits on the module's events.
#![allow(dead_code)]

use super::{options, Client};
use hertzforge_station::StationBuildError;
use sds_control::server::ServiceOptions;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::time::Duration;

pub const LONG: Duration = Duration::from_secs(600);

pub fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

/// Unpaced simulator options with `timeout` as the orphan clock.
pub fn opts(timeout: Duration) -> ServiceOptions {
    options(no_driver(), 0.0, timeout, Duration::from_secs(5))
}

pub fn row(index: u32, role: &str) -> Value {
    let units = if role == "drive" { "V" } else { "g" };
    json!({
        "index": index, "enabled": true, "description": format!("{role} {index}"),
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": 1000,
        "range_max": if role == "drive" { 10 } else { 1000 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

/// The table of a structure of `nc` controls, `nm` other responses
/// (monitors, for limit channels) and `nd` drives: channel `k` is index `k`.
pub fn rows(nc: u32, nm: u32, nd: u32) -> Vec<Value> {
    (1..=nc)
        .map(|i| row(i, "control"))
        .chain((nc + 1..=nc + nm).map(|i| row(i, "monitor")))
        .chain((nc + nm + 1..=nc + nm + nd).map(|i| row(i, "drive")))
        .collect()
}

/// Claim control, select the simulator and set the table.
pub async fn controller(c: &mut Client, nc: u32, nm: u32, nd: u32) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": rows(nc, nm, nd)}),
    )
    .await;
}

/// A flat profile at `g` over 5 to 2000 Hz.
pub fn flat(g: f64, alarm_db: f64, abort_db: f64) -> Value {
    json!({"label": "flat", "rows": [{"freq_hz": 5, "quantity": "accel_g", "value": g},
                                     {"freq_hz": 2000, "quantity": "accel_g", "value": g}],
           "alarm_db": [alarm_db, alarm_db], "abort_db": [abort_db, abort_db]})
}

/// Channels of a run on a structure of `nc` controls, `nm` monitors and `nd`
/// drives, every control at 1 g in phase, every drive with ceiling 2 V and
/// abort 4 V.
pub fn channels(nc: u32, nm: u32, nd: u32) -> Value {
    json!({
        "controls": (1..=nc).map(|i| json!({"channel": i, "profile": flat(1.0, 1.5, 6.0),
                                            "phase": [{"freq_hz": 10, "phase_deg": 0}]}))
            .collect::<Vec<_>>(),
        "drives": (nc + nm + 1..=nc + nm + nd)
            .map(|i| json!({"channel": i, "ceiling_vpk": 2, "abort_vpk": 4}))
            .collect::<Vec<_>>(),
    })
}

/// A configuration of `test` on `structure` with `law`: a quick
/// identification (1 Hz lines, 8 records), no review hold.
pub fn config(structure: &str, nc: u32, nm: u32, nd: u32, test: Value, law: &str) -> Value {
    json!({
        "title": "Four-Shaker Test",
        "channels": channels(nc, nm, nd),
        "test": test,
        "identification": {"resolution_hz": 1, "frames": 8, "review": false},
        "control": {"law": {"id": law}},
        "simulator": {"structure": structure},
        "operator": "A. Operator", "article": "Plate", "notes": "",
    })
}

pub fn sweep(lo: f64, hi: f64, oct_per_min: f64) -> Value {
    json!({"kind": "sweep", "band_hz": [lo, hi], "rate": {"law": "log", "oct_per_min": oct_per_min},
           "end": {"sweeps": 1}})
}

pub fn dwell(f: f64, s: f64) -> Value {
    json!({"kind": "dwell", "freq_hz": f, "end": {"seconds": s}})
}

pub fn stepped(freqs: &[f64], s: f64) -> Value {
    json!({"kind": "stepped", "frequencies_hz": freqs, "dwell": {"seconds": s}})
}

/// Wait for a `mimo_sine.state` event in `state`.
pub async fn state(c: &mut Client, state: &str) -> Value {
    let i = c
        .wait_for(&format!("mimo_sine.state {state}"), LONG, |e| {
            e.topic() == "mimo_sine.state" && e.data()["state"] == state
        })
        .await;
    c.events[i].data().clone()
}

/// Wait for a `mimo_sine.state` event in `phase`.
pub async fn phase(c: &mut Client, phase: &str) -> Value {
    let i = c
        .wait_for(&format!("mimo_sine.state phase {phase}"), LONG, |e| {
            e.topic() == "mimo_sine.state" && e.data()["phase"] == phase
        })
        .await;
    c.events[i].data().clone()
}

/// Configure and start; wait for the run to finish; its final state.
pub async fn run_to_end(c: &mut Client, cfg: Value) -> Value {
    c.ok("mimo_sine.configure", json!({"config": cfg})).await;
    c.ok("mimo_sine.start", json!({})).await;
    state(c, "finished").await
}

/// The run folder of a finished state.
pub fn folder(state: &Value) -> std::path::PathBuf {
    std::path::PathBuf::from(state["record"]["folder"].as_str().expect("a run folder"))
}

/// Wait until the run's record is complete on disk.
pub async fn complete(folder: &std::path::Path) -> Value {
    for _ in 0..400 {
        if let Ok(t) = std::fs::read_to_string(folder.join("run.json")) {
            let v: Value = serde_json::from_str(&t).unwrap();
            if v["complete"] == true {
                return v;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the record at {} never completed", folder.display());
}
