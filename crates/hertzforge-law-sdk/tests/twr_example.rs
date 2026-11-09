//! The TWR template (`template/twr`), its law compiled natively and run
//! through the SDK's trait, so the code a law author copies builds and does
//! what its documentation says.

#[path = "../template/twr/src/law.rs"]
mod law;

use hertzforge_law_sdk::twr::*;
use hertzforge_law_sdk::{Param, ParamKind, ParamValue, Params, TwrSingleLaw};
use law::Correction;

const N: usize = 64;

fn setup(gain: f64) -> TwrSetup {
    let bins = N / 2 + 1;
    let reference: Vec<f64> = (0..N)
        .map(|i| {
            if (16..48).contains(&i) {
                (std::f64::consts::TAU * 2.0 * (i - 16) as f64 / 32.0).sin()
            } else {
                0.0
            }
        })
        .collect();
    let c = |re: f64| Complex { re, im: 0.0 };
    TwrSetup {
        fs_hz: 1024.0,
        record_len: N as u32,
        band_lo_hz: 1.0,
        band_hi_hz: 500.0,
        reference,
        section_start: 16,
        section_len: 32,
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

fn seed_inputs() -> PlayInputs {
    PlayInputs {
        play_index: 0,
        kind: PlayKind::Iteration,
        response: Vec::new(),
        drive: Vec::new(),
        judged: None,
        limited: false,
    }
}

fn after(play_index: u64, response: Vec<f64>, drive: Vec<f64>, limited: bool) -> PlayInputs {
    PlayInputs {
        play_index,
        kind: PlayKind::Iteration,
        response,
        drive,
        judged: Some(Verdict {
            gated: false,
            rms_error_pct: 50.0,
            peak_error_pct: 50.0,
            spectral_error_db: 6.0,
        }),
        limited,
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
    assert_eq!(m.diagnostics.len(), 1);
}

#[test]
fn the_seed_is_the_inverse_times_the_reference_and_the_model_error_scales_it() {
    let s = setup(2.0);
    let ref_peak = peak(&s.reference);
    let mut law = Correction::new(s.clone(), params(0.5, true, 0.0)).unwrap();
    let d = law.seed(&seed_inputs()).unwrap().drive;
    assert!((peak(&d) - ref_peak / 2.0).abs() < 1e-9, "{}", peak(&d));
    let mut cold = Correction::new(s, params(0.5, true, -6.0)).unwrap();
    let h = cold.seed(&seed_inputs()).unwrap().drive;
    assert!((peak(&h) / peak(&d) - 10f64.powf(-6.0 / 20.0)).abs() < 1e-9);
}

/// The bench's numbers: an inverse 4 dB low at `beta` 0.5 leaves 0.685 of the
/// error after each correction (`1 - 0.5 x 0.631`); off, every play stays at
/// the first drive (0.499 of the target at -6 dB).
#[test]
fn it_corrects_by_beta_and_holds_when_correct_is_off() {
    let s = setup(1.0);
    let reference = s.reference.clone();
    let shortfall = |d: &[f64]| {
        let e: f64 = d.iter().zip(&reference).map(|(a, b)| (a - b).powi(2)).sum();
        let r: f64 = reference.iter().map(|v| v * v).sum();
        (e / r).sqrt()
    };
    let mut law = Correction::new(s.clone(), params(0.5, true, -4.0)).unwrap();
    let mut drive = law.seed(&seed_inputs()).unwrap().drive;
    let k = 10f64.powf(-4.0 / 20.0);
    assert!((shortfall(&drive) - (1.0 - k)).abs() < 1e-9);
    for p in 1..=5 {
        let before = shortfall(&drive);
        drive = law
            .iterate(&after(p, drive.clone(), drive.clone(), false))
            .unwrap()
            .drive;
        let factor = shortfall(&drive) / before;
        assert!(
            (factor - (1.0 - 0.5 * k)).abs() < 1e-9,
            "play {p}: {factor}"
        );
        assert!((factor - 0.685).abs() < 1e-3);
    }

    let mut off = Correction::new(s, params(0.5, false, -6.0)).unwrap();
    let first = off.seed(&seed_inputs()).unwrap().drive;
    let mut drive = first.clone();
    for p in 1..=3 {
        drive = off
            .iterate(&after(p, drive.clone(), drive.clone(), false))
            .unwrap()
            .drive;
    }
    assert_eq!(drive, first, "holds");
    assert!((peak(&drive) / peak(&reference) - 0.501).abs() < 1e-3);
}

/// A clipped play is corrected from what played: the controller held the drive
/// to half, the plant measured it, and the next drive starts from that half.
#[test]
fn a_clipped_play_is_corrected_from_the_drive_that_played() {
    let s = setup(1.0);
    let mut law = Correction::new(s, params(0.5, true, 0.0)).unwrap();
    let seed = law.seed(&seed_inputs()).unwrap().drive;
    let half: Vec<f64> = seed.iter().map(|v| 0.5 * v).collect();
    let next = law
        .iterate(&after(1, half.clone(), half.clone(), true))
        .unwrap()
        .drive;
    // From the half: 0.5 R + 0.5 (R - 0.5 R) = 0.75 R.
    assert!(
        (peak(&next) / peak(&seed) - 0.75).abs() < 1e-9,
        "{}",
        peak(&next)
    );
}
