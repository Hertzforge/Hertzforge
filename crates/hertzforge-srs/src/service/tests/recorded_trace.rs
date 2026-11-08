//! The recorded trace of the run without `control`: what the SRS seam must not
//! move.
//!
//! On the service fixtures (a), (b), (c), (e), (g) and (i) of `tests.rs`, it
//! records every fire the plant was handed (its length, its peak and a hash of
//! its bits, the identification burst included), the component table as
//! armed, and after each pulse the component amplitudes (f64 bits), the
//! measured SRS (bits), the components the limit cap held back and the state;
//! then how the run ended and its trip. Object keys are written sorted, so the
//! trace reads the same whether `serde_json` keeps insertion order (the
//! workspace build) or not (the package alone).
//!
//! On the platform it was recorded on (macOS on aarch64) the trace must match
//! byte for byte. Elsewhere a last bit may round differently in the maths
//! library, so numbers are held to relative 1e-9 and the drive hash is not
//! compared.
//!
//! Re-record (only when a change to the run is intended and said so):
//! `HERTZFORGE_RECORD_SRS_TRACE=1 cargo test -p hertzforge-srs recorded_trace`.

use super::{
    cfg_a, cfg_b, cfg_c, cfg_e, mock_cfg, plant, siso_table, table_with_monitor, MockPlant,
    PlantSpy,
};
use crate::service::config::SrsTestConfig;
use crate::service::plant::{FirePath, FiredResponses, HalPulsePlant, PlantError, PulsePlant};
use crate::service::report::RunState;
use crate::service::SrsShockService;
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal_sim::SimBackend;
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

const TRACE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/srs_open_loop_trace.txt"
);

/// The platform the trace was recorded on.
const RECORDING_PLATFORM: bool = cfg!(all(target_os = "macos", target_arch = "aarch64"));

/// One fire the plant was handed.
#[derive(Debug, Clone)]
struct Fired {
    len: usize,
    ring_out: usize,
    peak: f64,
    fnv: u64,
}

/// A plant that records every drive it is handed, then plays it on the plant
/// it wraps.
struct Tap {
    inner: Box<dyn PulsePlant + Send>,
    fired: Arc<Mutex<Vec<Fired>>>,
}

impl PulsePlant for Tap {
    fn fs_hz(&self) -> f64 {
        self.inner.fs_hz()
    }
    fn ensure_open(&mut self) -> Result<(), String> {
        self.inner.ensure_open()
    }
    fn fire(
        &mut self,
        drive_g: &[f64],
        ring_out_samples: usize,
        cancel: Option<&AtomicBool>,
    ) -> Result<FiredResponses, PlantError> {
        self.fired.lock().unwrap().push(Fired {
            len: drive_g.len(),
            ring_out: ring_out_samples,
            peak: drive_g.iter().fold(0.0f64, |m, v| m.max(v.abs())),
            fnv: fnv(drive_g),
        });
        self.inner.fire(drive_g, ring_out_samples, cancel)
    }
    fn stop(&mut self) -> Result<(), String> {
        self.inner.stop()
    }
    fn emergency_stop(&mut self) -> Result<(), String> {
        self.inner.emergency_stop()
    }
}

/// FNV-1a over the samples' bits.
fn fnv(x: &[f64]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in x {
        for b in v.to_bits().to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn bits(v: f64) -> Value {
    Value::String(format!("0x{:016x}", v.to_bits()))
}

fn bits_all(x: &[f64]) -> Value {
    Value::Array(x.iter().map(|&v| bits(v)).collect())
}

/// `v` as JSON text with every object's keys sorted.
fn canon(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| format!("{}:{}", serde_json::to_string(k).unwrap(), canon(&m[*k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canon).collect::<Vec<_>>().join(",")),
        other => serde_json::to_string(other).unwrap(),
    }
}

fn service(
    cfg: SrsTestConfig,
    table: ChannelTable,
    inner: Box<dyn PulsePlant + Send>,
) -> (SrsShockService, Arc<Mutex<Vec<Fired>>>) {
    let fired = Arc::new(Mutex::new(Vec::new()));
    let tap = Tap {
        inner,
        fired: Arc::clone(&fired),
    };
    let s = SrsShockService::new(
        cfg,
        table,
        FirePath::Host(Box::new(tap)),
        Some(std::env::temp_dir().join(format!("hertzforge-srs-trace-{}", std::process::id()))),
    )
    .unwrap();
    (s, fired)
}

fn sim(
    cfg: SrsTestConfig,
    table: ChannelTable,
    backend: SimBackend,
) -> (SrsShockService, Arc<Mutex<Vec<Fired>>>) {
    let host = HalPulsePlant::new(
        backend,
        table.clone(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap();
    service(cfg, table, Box::new(host))
}

fn mock(cfg: SrsTestConfig) -> (SrsShockService, Arc<Mutex<Vec<Fired>>>) {
    let spy = Arc::new(PlantSpy::default());
    service(
        cfg,
        siso_table(),
        Box::new(MockPlant::new(spy, 20_000.0, 20_000.0)),
    )
}

/// Collects one fixture's lines.
struct Recorder {
    fixture: &'static str,
    lines: Vec<String>,
}

impl Recorder {
    fn new(fixture: &'static str) -> Self {
        Recorder {
            fixture,
            lines: Vec::new(),
        }
    }

    fn push(&mut self, mut v: Value) {
        v["fixture"] = Value::String(self.fixture.into());
        self.lines.push(canon(&v));
    }

    fn armed(&mut self, s: &SrsShockService) {
        let amps = s
            .drive_components()
            .map(|e| e.amplitudes())
            .unwrap_or_default();
        self.push(
            json!({"event": "armed", "state": format!("{:?}", s.state()), "amps": bits_all(&amps)}),
        );
    }

    fn pulse(&mut self, s: &SrsShockService) {
        let amps = s
            .drive_components()
            .map(|e| e.amplitudes())
            .unwrap_or_default();
        self.push(json!({
            "event": "pulse",
            "pulse": s.store().len(),
            "state": format!("{:?}", s.state()),
            "level_db": s.current_level_db(),
            "amps": bits_all(&amps),
            "srs": bits_all(s.last_measured_srs().unwrap_or(&[])),
            "limited": s.status().last_limit_status.limited_components,
        }));
    }

    fn end(&mut self, s: &SrsShockService, fired: &Mutex<Vec<Fired>>) {
        for (i, f) in fired.lock().unwrap().iter().enumerate() {
            self.push(json!({
                "event": "fire",
                "index": i,
                "len": f.len,
                "ring_out": f.ring_out,
                "peak": bits(f.peak),
                "drive_fnv": format!("{:016x}", f.fnv),
            }));
        }
        let trip = s
            .trip()
            .map(|t| serde_json::to_value(t).unwrap())
            .unwrap_or(Value::Null);
        self.push(json!({
            "event": "end",
            "state": format!("{:?}", s.state()),
            "pulses": s.store().len(),
            "rows": s.report_data().pulses.len(),
            "trip": trip,
        }));
    }
}

/// `run_auto`'s sequence, one pulse at a time, recording after each.
fn step_auto(s: &mut SrsShockService, rec: &mut Recorder, max_pulses: u64) {
    s.assess_feasibility().unwrap();
    if s.state() == RunState::FeasibilityBlocked {
        return;
    }
    s.arm().unwrap();
    if s.state() == RunState::SysId {
        s.run_sysid().unwrap();
    }
    rec.armed(s);
    let mut fired = 0u64;
    while matches!(s.state(), RunState::Armed | RunState::Firing) && fired < max_pulses {
        let more = s.fire_pulse().unwrap();
        rec.pulse(s);
        if !more {
            break;
        }
        fired += 1;
    }
}

/// The same fixture through `run_auto` itself must fire the same drives and
/// end on the same table as the stepped run: the stepper is `run_auto`.
fn same_as_run_auto(
    stepped: &SrsShockService,
    stepped_fired: &Mutex<Vec<Fired>>,
    mut whole: SrsShockService,
    whole_fired: &Mutex<Vec<Fired>>,
) {
    let end = whole.run_auto(200).unwrap();
    assert_eq!(end, stepped.state());
    let a: Vec<(usize, u64)> = stepped_fired
        .lock()
        .unwrap()
        .iter()
        .map(|f| (f.len, f.fnv))
        .collect();
    let b: Vec<(usize, u64)> = whole_fired
        .lock()
        .unwrap()
        .iter()
        .map(|f| (f.len, f.fnv))
        .collect();
    assert_eq!(a, b, "run_auto fired other drives than its steps");
    assert_eq!(
        stepped.drive_components().map(|e| e.amplitudes()),
        whole.drive_components().map(|e| e.amplitudes())
    );
}

fn record() -> Vec<String> {
    let mut out = Vec::new();

    // (a) full AUTO run on a unity plant.
    let mut rec = Recorder::new("a");
    let (mut s, f) = sim(cfg_a(), siso_table(), plant(vec![], 1.0));
    step_auto(&mut s, &mut rec, 200);
    rec.end(&s, &f);
    let (w, wf) = sim(cfg_a(), siso_table(), plant(vec![], 1.0));
    same_as_run_auto(&s, &f, w, &wf);
    out.extend(rec.lines);

    // (b) a limit channel with a resonant user-supplied FRF.
    let mut rec = Recorder::new("b");
    let (mut s, f) = sim(
        cfg_b(),
        table_with_monitor("fixture_resp"),
        plant(vec![], 1.0),
    );
    step_auto(&mut s, &mut rec, 200);
    rec.end(&s, &f);
    let (w, wf) = sim(
        cfg_b(),
        table_with_monitor("fixture_resp"),
        plant(vec![], 1.0),
    );
    same_as_run_auto(&s, &f, w, &wf);
    out.extend(rec.lines);

    // (c) the envelope abort on a gross over-level.
    let mut rec = Recorder::new("c");
    let (mut s, f) = sim(cfg_c(), siso_table(), plant(vec![], 20.0));
    step_auto(&mut s, &mut rec, 200);
    rec.end(&s, &f);
    let (w, wf) = sim(cfg_c(), siso_table(), plant(vec![], 20.0));
    same_as_run_auto(&s, &f, w, &wf);
    out.extend(rec.lines);

    // (e) the ladder on a mild resonance.
    let mut rec = Recorder::new("e");
    let (mut s, f) = sim(cfg_e(), siso_table(), plant(vec![(500.0, 4.0)], 1.0));
    step_auto(&mut s, &mut rec, 200);
    rec.end(&s, &f);
    let (w, wf) = sim(cfg_e(), siso_table(), plant(vec![(500.0, 4.0)], 1.0));
    same_as_run_auto(&s, &f, w, &wf);
    out.extend(rec.lines);

    // (g) the review hold, then one pulse.
    let mut rec = Recorder::new("g");
    let (mut s, f) = mock(mock_cfg());
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_sysid().unwrap();
    assert_eq!(s.state(), RunState::AwaitingReview);
    s.proceed_after_review().unwrap();
    rec.armed(&s);
    s.fire_pulse().unwrap();
    rec.pulse(&s);
    rec.end(&s, &f);
    out.extend(rec.lines);

    // (i) a pulse then a stop; an abort before any pulse.
    let mut rec = Recorder::new("i");
    let mut cfg = mock_cfg();
    cfg.review_after_sysid = false;
    cfg.sysid = None;
    let (mut s, f) = mock(cfg.clone());
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    rec.armed(&s);
    s.fire_pulse().unwrap();
    rec.pulse(&s);
    s.stop().unwrap();
    rec.end(&s, &f);
    let (mut s, f) = mock(cfg);
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.abort_now("operator abort");
    rec.end(&s, &f);
    out.extend(rec.lines);

    out
}

/// Numbers (and bit strings) within relative `tol`; everything else equal.
fn close(a: &Value, b: &Value, tol: f64) -> bool {
    fn num(v: &Value) -> Option<f64> {
        match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) if s.len() == 18 && s.starts_with("0x") => {
                u64::from_str_radix(&s[2..], 16).ok().map(f64::from_bits)
            }
            _ => None,
        }
    }
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter().all(|(k, va)| {
                    k == "drive_fnv" || y.get(k).is_some_and(|vb| close(va, vb, tol))
                })
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| close(p, q, tol))
        }
        _ => match (num(a), num(b)) {
            (Some(p), Some(q)) => p == q || (p - q).abs() <= tol * p.abs().max(q.abs()),
            _ => a == b,
        },
    }
}

#[test]
fn the_run_without_control_matches_its_recorded_trace() {
    let lines = record();
    if std::env::var_os("HERTZFORGE_RECORD_SRS_TRACE").is_some() {
        std::fs::create_dir_all(std::path::Path::new(TRACE_PATH).parent().unwrap()).unwrap();
        std::fs::write(TRACE_PATH, lines.join("\n") + "\n").unwrap();
        return;
    }
    let want = std::fs::read_to_string(TRACE_PATH).expect("the recorded trace");
    let want: Vec<&str> = want.lines().collect();
    assert_eq!(
        lines.len(),
        want.len(),
        "the trace has {} lines, the recording {}",
        lines.len(),
        want.len()
    );
    for (i, (got, exp)) in lines.iter().zip(&want).enumerate() {
        if RECORDING_PLATFORM {
            assert_eq!(got, exp, "trace line {} differs", i + 1);
        } else {
            let g: Value = serde_json::from_str(got).unwrap();
            let e: Value = serde_json::from_str(exp).unwrap();
            assert!(
                close(&g, &e, 1e-9),
                "trace line {} differs:\n{got}\n{exp}",
                i + 1
            );
        }
    }
}
