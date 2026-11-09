//! The SRS shock template (`template/shock-srs`), its law compiled natively and
//! run through the SDK's trait, so the code a law author copies builds and does
//! what its documentation says.

#[path = "../template/shock-srs/src/law.rs"]
mod law;

use hertzforge_law_sdk::srs::*;
use hertzforge_law_sdk::{Param, ParamKind, ParamValue, Params, ShockSrsLaw};
use law::PlainRatio;

fn bands() -> Vec<f64> {
    (0..14).map(|i| 100.0 * 2f64.powf(i as f64 / 3.0)).collect()
}

fn setup(gain: Option<f64>) -> SrsSetup {
    let b = bands();
    SrsSetup {
        component_hz: b.clone(),
        target: vec![10.0; b.len()],
        q: 10.0,
        component_gain: gain.map(|g| vec![g; b.len()]),
        band_hz: b,
    }
}

fn params(max_db: f64, correct: bool, seed_error_db: f64) -> Params {
    Params(vec![
        Param {
            key: "max_db_per_pulse".into(),
            value: ParamValue::Number(max_db),
        },
        Param {
            key: "correct".into(),
            value: ParamValue::Toggle(correct),
        },
        Param {
            key: "seed_error_db".into(),
            value: ParamValue::Number(seed_error_db),
        },
    ])
}

fn inputs(measured_db: f64) -> SrsInputs {
    let n = bands().len();
    SrsInputs {
        pulse_index: 1,
        target: vec![10.0; n],
        measured: vec![10.0 * 10f64.powf(measured_db / 20.0); n],
        commanded_scale: 1.0,
    }
}

#[test]
fn the_template_declares_its_change_limit_correct_and_seed_error() {
    let m = PlainRatio::describe();
    let keys: Vec<&str> = m.params.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(keys, ["max_db_per_pulse", "correct", "seed_error_db"]);
    assert!(matches!(m.params[1].kind, ParamKind::Toggle(true)));
    assert_eq!(m.diagnostics[0].key, "worst_band_error_db");
}

#[test]
fn the_seed_inverts_the_identified_gain_and_the_seed_error_scales_it() {
    let seed0 = SrsInputs {
        pulse_index: 0,
        measured: vec![0.0; bands().len()],
        ..inputs(0.0)
    };
    let mut l = PlainRatio::new(setup(Some(2.0)), params(3.0, true, 0.0)).unwrap();
    assert!(l.seed(&seed0).unwrap().gains.iter().all(|&g| g == 0.5));
    let mut l = PlainRatio::new(setup(Some(2.0)), params(3.0, true, -4.0)).unwrap();
    let want = 0.5 * 10f64.powf(-4.0 / 20.0);
    assert!(l
        .seed(&seed0)
        .unwrap()
        .gains
        .iter()
        .all(|&g| (g - want).abs() < 1e-15));
    let mut blind = PlainRatio::new(setup(None), params(3.0, true, 0.0)).unwrap();
    assert!(blind.seed(&seed0).unwrap().gains.iter().all(|&g| g == 1.0));
}

#[test]
fn each_pulse_follows_the_ratio_within_its_limit_unless_correction_is_off() {
    let mut l = PlainRatio::new(setup(Some(1.0)), params(3.0, true, 0.0)).unwrap();
    let out = l.iterate(&inputs(-1.0)).unwrap();
    assert!(out
        .gains
        .iter()
        .all(|&g| (g - 10f64.powf(1.0 / 20.0)).abs() < 1e-12));
    assert!((out.values[0].value + 1.0).abs() < 1e-9);
    let out = l.iterate(&inputs(-10.0)).unwrap();
    assert!(
        out.gains
            .iter()
            .all(|&g| (g - 10f64.powf(3.0 / 20.0)).abs() < 1e-12),
        "held to 3 dB"
    );
    let mut hold = PlainRatio::new(setup(Some(1.0)), params(3.0, false, -4.0)).unwrap();
    assert!(hold
        .iterate(&inputs(-4.0))
        .unwrap()
        .gains
        .iter()
        .all(|&g| g == 1.0));
}
