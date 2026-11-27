//! Shared test helpers: reading the testbed's exported vectors.
#![allow(dead_code)]

use hertzforge_mimo_sine_law_math::reference_structure::ModalStructure;
use hertzforge_mimo_sine_law_math::setup::{DelayEstimate, Identification};
use hertzforge_mimo_sine_law_math::speed::GainRule;
use hertzforge_mimo_sine_law_math::{MatrixStack, C64};
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

/// Little-endian float64 bytes, base64 (standard alphabet, padded).
pub fn b64_f64s(s: &str) -> Vec<f64> {
    let val = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("not base64"),
        }
    };
    let mut bytes = Vec::new();
    for chunk in s.as_bytes().chunks(4) {
        let pad = chunk.iter().filter(|&&c| c == b'=').count();
        let mut acc = 0u32;
        for &c in chunk {
            acc = (acc << 6) | if c == b'=' { 0 } else { val(c) };
        }
        let three = [(acc >> 16) as u8, (acc >> 8) as u8, acc as u8];
        bytes.extend_from_slice(&three[..3 - pad]);
    }
    bytes
        .chunks(8)
        .map(|b| f64::from_le_bytes(b.try_into().expect("8 bytes")))
        .collect()
}

/// The identification a modal-fit vector holds (its band's lines).
pub fn identification(c: &Value) -> Identification {
    let g = f64s(&c["grid_hz"]);
    let (n, m) = (
        c["n"].as_u64().expect("n") as usize,
        c["m"].as_u64().expect("m") as usize,
    );
    let re = b64_f64s(c["h_re_b64"].as_str().expect("h_re"));
    let im = b64_f64s(c["h_im_b64"].as_str().expect("h_im"));
    assert_eq!(re.len(), g.len() * n * m);
    let h: Vec<C64> = re.iter().zip(&im).map(|(a, b)| C64::new(*a, *b)).collect();
    Identification {
        response: MatrixStack::from_vec(g.len(), n, m, h).expect("shape"),
        limit_response: MatrixStack::zeros(g.len(), 0, m),
        coherence: vec![1.0; n * g.len()],
        limit_coherence: Vec::new(),
        trusted: vec![true; g.len()],
        grid_hz: g,
    }
}

/// A complex array written as `{"re": base64, "im": base64}`.
pub fn b64_cplx(v: &Value) -> Vec<C64> {
    let re = b64_f64s(v["re"].as_str().expect("re"));
    let im = b64_f64s(v["im"].as_str().expect("im"));
    re.into_iter()
        .zip(im)
        .map(|(a, b)| C64::new(a, b))
        .collect()
}
