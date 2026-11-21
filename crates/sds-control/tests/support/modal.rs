//! The Modal Test's test rig on the simulator: the channel rows, the setups, the
//! service, and the readers of a run folder.

use super::{options, serve, Client};
use hertzforge_modal_test::structure::{accelerance, Dof, MODES};
use hertzforge_station::StationBuildError;
use sds_control::server::{Service, ServiceOptions};
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Duration, Instant};

pub fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

/// A measured row: `units`, a range, a DOF and its part in an FRF.
pub fn row(index: u32, units: &str, range: f64, node: u32, part: &str) -> Value {
    json!({
        "index": index, "enabled": true, "description": format!("{part} {index}"),
        "role": "monitor", "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": 10, "range_max": range,
        "polarity": "positive", "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null},
        "node": node, "direction": "Z+", "analysis_role": part,
    })
}

pub fn drive_row(index: u32) -> Value {
    json!({
        "index": index, "enabled": true, "description": format!("shaker {index}"),
        "role": "drive", "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": "V", "sensitivity_mv_per_eu": 1000, "range_max": 10, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

/// Row 1 the hammer at node 1, rows 2 and 3 responses at nodes 4 and 7; with
/// `shakers`, row 4 a second force at node 8 and rows 5 and 6 the shakers' drives.
pub fn rows(shakers: bool) -> Vec<Value> {
    let mut r = vec![
        row(1, "lbf", 100.0, 1, "reference"),
        row(2, "g", 50.0, 4, "response"),
        row(3, "g", 50.0, 7, "response"),
    ];
    if shakers {
        r.push(row(4, "lbf", 100.0, 8, "reference"));
        r.push(drive_row(5));
        r.push(drive_row(6));
    }
    r
}

pub async fn rig(shakers: bool, speed: f64) -> Client {
    rig_with(shakers, base_options(speed)).await
}

/// The service's options for a test on the simulator at `speed`.
pub fn base_options(speed: f64) -> ServiceOptions {
    options(
        no_driver(),
        speed,
        Duration::from_secs(60),
        Duration::from_secs(5),
    )
}

/// A service with `o`, the controller claimed, the simulator selected, and the rows.
pub async fn rig_with(shakers: bool, o: ServiceOptions) -> Client {
    let svc = Service::with_options(o);
    svc.start_background();
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": rows(shakers)}),
    )
    .await;
    c
}

pub fn impact_config() -> Value {
    json!({
        "title": "Roving Hammer", "operator": "A. Operator", "article": "Beam, SN 0007",
        "mode": "impact", "fs_hz": 1024, "block": 1024, "band": {"hi_hz": 250},
        "averages": 3,
        "roving": {"channels": [1], "points": {"start": 1, "count": 3}},
        "impact": {"holdoff_blocks": 0},
    })
}

pub fn shaker_config(source: &str) -> Value {
    json!({
        "title": "Two Shakers", "mode": "shaker", "fs_hz": 1024, "block": 1024,
        "band": {"hi_hz": 250}, "averages": 12,
        "shaker": {"drives": [{"row": 5, "level_vrms": 0.05}, {"row": 6, "level_vrms": 0.05}],
                   "source": source, "burst_pct": 40, "ramp_s": 0.5, "ceiling_v": 1.0},
    })
}

pub fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

pub fn lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

pub fn f64s(path: &Path) -> Vec<f64> {
    std::fs::read(path)
        .unwrap()
        .chunks(8)
        .map(|b| f64::from_le_bytes(b.try_into().unwrap()))
        .collect()
}

pub fn f32s(path: &Path) -> Vec<f32> {
    std::fs::read(path)
        .unwrap()
        .chunks(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}

pub async fn state(c: &mut Client) -> Value {
    c.ok("modal_test.state", json!({})).await
}

pub async fn until(
    c: &mut Client,
    wait: Duration,
    what: &str,
    f: impl Fn(&Value) -> bool,
) -> Value {
    let t0 = Instant::now();
    loop {
        let s = state(c).await;
        if f(&s) {
            return s;
        }
        assert!(t0.elapsed() < wait, "waiting for {what}: {s}");
        c.pump(Duration::from_millis(20)).await;
    }
}

pub async fn record_done(c: &mut Client, folder: &Path) {
    let t0 = Instant::now();
    while !(folder.join("end.json").is_file() && read(&folder.join("run.json"))["complete"] == true)
    {
        assert!(t0.elapsed() < Duration::from_secs(30), "{folder:?}");
        c.pump(Duration::from_millis(20)).await;
    }
}

/// One record's FRF: `[resp][ref][line]` of (re, im).
pub type Frf = Vec<Vec<Vec<(f64, f64)>>>;

/// The measured FRF of every record of `modal_frf`, with the abscissa's start and step.
pub fn frf_records(folder: &Path) -> (Vec<Value>, Vec<Frf>, f64, f64) {
    let side = read(&folder.join("results/modal_frf.json"));
    let shape: Vec<usize> = side["arrays"][0]["shape"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as usize)
        .collect();
    let (nr, nf, nl) = (shape[0], shape[1], shape[2]);
    let v = f64s(&folder.join("results/modal_frf.bin"));
    let per = nr * nf * nl * 2;
    let recs = lines(&folder.join("results/modal_frf.jsonl"));
    assert_eq!(v.len(), per * recs.len());
    let out = v
        .chunks(per)
        .map(|c| {
            (0..nr)
                .map(|i| {
                    (0..nf)
                        .map(|j| {
                            (0..nl)
                                .map(|l| {
                                    let k = ((i * nf + j) * nl + l) * 2;
                                    (c[k], c[k + 1])
                                })
                                .collect()
                        })
                        .collect()
                })
                .collect()
        })
        .collect();
    let start = side["abscissa"]["start"].as_f64().unwrap();
    let step = side["abscissa"]["step"].as_f64().unwrap();
    (recs, out, start, step)
}

/// The largest relative error of `h` against the structure's FRF from `b` to `a` at
/// the resonances.
pub fn worst_at_resonances(h: &[(f64, f64)], a: &Dof, b: &Dof, start: f64, step: f64) -> f64 {
    MODES
        .iter()
        .map(|(f, _)| {
            let l = ((f - start) / step).round() as usize;
            let want = accelerance(a, b, start + l as f64 * step);
            (h[l].0 - want.re).hypot(h[l].1 - want.im) / want.norm()
        })
        .fold(0.0, f64::max)
}
