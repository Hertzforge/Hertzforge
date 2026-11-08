//! The classical shock template (`template/shock-classical`), its law compiled
//! natively and run through the SDK's trait, so the code a law author copies
//! builds and does what its documentation says.

#[path = "../template/shock-classical/src/law.rs"]
mod law;

use hertzforge_law_sdk::shock::*;
use hertzforge_law_sdk::{Param, ParamKind, ParamValue, Params, ShockClassicalLaw};
use law::Correction;

const N: usize = 64;

fn setup(gain: f64) -> ClassicalSetup {
    let bins = N / 2 + 1;
    let reference: Vec<f64> = (0..N)
        .map(|i| {
            if (16..32).contains(&i) {
                (std::f64::consts::PI * (i - 16) as f64 / 16.0).sin()
            } else {
                0.0
            }
        })
        .collect();
    let c = |re: f64| Complex { re, im: 0.0 };
    ClassicalSetup {
        fs_hz: 1024.0,
        record_len: N as u32,
        band_lo_hz: 1.0,
        band_hi_hz: 500.0,
        reference,
        main_start: 16,
        main_len: 16,
        plant: Plant {
            frf: vec![c(gain); bins],
            coherence: vec![1.0; bins],
            inverse: vec![c(1.0 / gain); bins],
            trusted: vec![true; bins],
            epsilon: 0.0,
        },
        drive_ceiling: 10.0,
    }
}

fn params(beta: f64, correct: bool, model_error_db: f64) -> Params {
    Params(vec![
        Param {
            key: "beta".into(),
            value: ParamValue::Number(beta),
        },
        Param {
            key: "correct".into(),
            value: ParamValue::Toggle(correct),
        },
        Param {
            key: "model_error_db".into(),
            value: ParamValue::Number(model_error_db),
        },
    ])
}

fn seed_inputs() -> PulseInputs {
    PulseInputs {
        pulse_index: 0,
        response: Vec::new(),
        drive: Vec::new(),
        judged: None,
        limited: false,
    }
}

fn peak(x: &[f64]) -> f64 {
    x.iter().fold(0.0f64, |m, v| m.max(v.abs()))
}

#[test]
fn the_template_declares_beta_correct_and_model_error() {
    let m = Correction::describe();
    let keys: Vec<&str> = m.params.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(keys, ["beta", "correct", "model_error_db"]);
    assert!(matches!(m.params[1].kind, ParamKind::Toggle(true)));
}

#[test]
fn the_seed_is_the_inverse_times_the_reference_and_the_model_error_scales_it() {
    let s = setup(2.0);
    let ref_peak = peak(&s.reference);
    let mut law = Correction::new(s.clone(), params(0.5, true, 0.0)).unwrap();
    let d = law.seed(&seed_inputs()).unwrap().drive;
    assert!((peak(&d) - ref_peak / 2.0).abs() < 1e-9, "{}", peak(&d));
    let mut hot = Correction::new(s, params(0.5, true, 4.0)).unwrap();
    let h = hot.seed(&seed_inputs()).unwrap().drive;
    assert!((peak(&h) / peak(&d) - 10f64.powf(4.0 / 20.0)).abs() < 1e-9);
}

#[test]
fn it_corrects_by_beta_and_holds_when_correct_is_off() {
    // The law believes the plant 6 dB hotter than it is: the seed plays half.
    let s = setup(1.0);
    let reference = s.reference.clone();
    for correct in [true, false] {
        let mut law = Correction::new(s.clone(), params(0.5, correct, -6.0)).unwrap();
        let mut drive = law.seed(&seed_inputs()).unwrap().drive;
        let first = peak(&drive);
        for _ in 0..3 {
            let inputs = PulseInputs {
                pulse_index: 1,
                response: drive.clone(),
                drive: drive.clone(),
                judged: None,
                limited: false,
            };
            drive = law.iterate(&inputs).unwrap().drive;
        }
        let err = peak(
            &drive
                .iter()
                .zip(&reference)
                .map(|(d, r)| d - r)
                .collect::<Vec<_>>(),
        );
        if correct {
            // An inverse 6 dB low leaves 1 - 0.5 x 0.5 = 0.75 of the error
            // after each pulse.
            let expect = (1.0 - first) * 0.75f64.powi(3);
            assert!(
                (err - expect).abs() < 0.01,
                "corrects: {err} against {expect}"
            );
        } else {
            assert!((peak(&drive) - first).abs() < 1e-12, "holds");
        }
    }
}
