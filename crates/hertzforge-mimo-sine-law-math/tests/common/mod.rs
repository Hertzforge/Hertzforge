//! Shared test helpers: reading the testbed's exported vectors.
#![allow(dead_code)]

use hertzforge_mimo_sine_law_math::reference_structure::ModalStructure;
use hertzforge_mimo_sine_law_math::setup::DelayEstimate;
use hertzforge_mimo_sine_law_math::speed::GainRule;
use hertzforge_mimo_sine_law_math::C64;
use serde_json::Value;

pub fn load(name: &str) -> Value {
    let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json")
}

pub fn f64s(v: &Value) -> Vec<f64> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|x| x.as_f64().expect("number"))
        .collect()
}

pub fn flat(v: &Value) -> Vec<f64> {
    v.as_array().expect("array").iter().flat_map(f64s).collect()
}

pub fn cplx(v: &Value) -> Vec<C64> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|p| C64::new(p[0].as_f64().expect("re"), p[1].as_f64().expect("im")))
        .collect()
}

pub fn structure(case: &Value) -> ModalStructure {
    ModalStructure {
        f_hz: f64s(&case["f_hz"]),
        zeta: f64s(&case["zeta"]),
        drive_shape: flat(&case["drive_shape"]),
        output_shape: flat(&case["output_shape"]),
        drives: case["m"].as_u64().expect("m") as usize,
        controls: case["n"].as_u64().expect("n") as usize,
    }
}

pub fn band(case: &Value) -> (f64, f64) {
    let b = f64s(&case["band"]);
    (b[0], b[1])
}

/// Largest |a - b| over the largest |b|.
pub fn rel_err(a: &[C64], b: &[C64]) -> f64 {
    let scale = b.iter().map(|v| v.norm()).fold(1e-300, f64::max);
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).norm())
        .fold(0.0, f64::max)
        / scale
}

/// The research testbed's gain rule, named: the loop-speed rule reading the
/// structure delay by the largest element (the form the parity vectors hold).
pub fn testbed_rule() -> GainRule {
    GainRule::LoopSpeed {
        g_max: 0.5,
        beta: 0.5,
        delay: DelayEstimate::Largest,
    }
}
