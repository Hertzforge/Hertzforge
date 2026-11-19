//! Under the controller's notch every built-in law aims for the reference the
//! controller judges the controls against, `reference x notch_scale`, so it
//! never pushes against the notch and its drive follows the notch out as the
//! controller releases it.

use hertzforge_mimo_sine_law_math::law::{Hold, Inputs, Outputs, SineLaw};
use hertzforge_mimo_sine_law_math::matrix::{MatrixLaw, MatrixParams, MATRIX_VALUES};
use hertzforge_mimo_sine_law_math::paired::{PairedLaw, PairedParams, PAIRED_VALUES};
use hertzforge_mimo_sine_law_math::ratio::{RatioLaw, RatioParams, RATIO_VALUES};
use hertzforge_mimo_sine_law_math::reference_structure::ModalStructure;
use hertzforge_mimo_sine_law_math::secant::{SecantLaw, SecantParams, SECANT_VALUES};
use hertzforge_mimo_sine_law_math::setup::Setup;
use hertzforge_mimo_sine_law_math::steepest::{SteepestLaw, SteepestParams, STEEPEST_VALUES};
use hertzforge_mimo_sine_law_math::C64;

const F_HZ: f64 = 70.0;

/// The notch's depth the controller holds the run at.
const DEPTH: f64 = 0.5;

fn setup() -> Setup {
    ModalStructure {
        f_hz: vec![4.0, 4.5, 60.0, 85.0],
        zeta: vec![0.05, 0.05, 0.02, 0.03],
        drive_shape: vec![1.0, 0.1, 0.1, 1.0, 0.8, 0.1, 0.1, 0.7],
        output_shape: vec![1.0, 0.1, 0.1, 1.0, 0.6, 0.1, 0.1, 0.5],
        drives: 2,
        controls: 2,
    }
    .setup((30.0, 120.0), false)
}

/// Run `law` at a dwell on a structure that answers at once (`C = H D`, `H`
/// the identification), the notch in at `DEPTH` from the first update; the
/// controls at the end, against the reference.
fn end_db(law: &mut dyn SineLaw, values: usize, updates: usize) -> Vec<f64> {
    let s = setup();
    let (n, m) = (s.controls, s.drives);
    let mut h = vec![C64::new(0.0, 0.0); n * m];
    s.response_at(F_HZ, &mut h);
    let r = vec![C64::new(1.0, 0.0); n];
    let mut d = vec![C64::new(0.0, 0.0); m];
    let mut c = vec![C64::new(0.0, 0.0); n];
    let mut out = Outputs::new(m, values);
    for k in 0..updates {
        let x = Inputs {
            index: k as u64,
            dt_s: if k == 0 { 0.0 } else { s.tick_s },
            f_hz: F_HZ,
            sweep_hz_s: 0.0,
            sweep_oct_min: 0.0,
            level: 1.0,
            reference: &r,
            controls: &c,
            limits: &[],
            limit_bounds: &[],
            drive_in_force: &d,
            hold: Hold::Notch,
            notch_scale: DEPTH,
            settle_open: false,
        };
        if k == 0 {
            law.engage(&x, &mut out).unwrap();
        } else {
            law.tick(&x, &mut out).unwrap();
        }
        d.copy_from_slice(&out.drive);
        for (i, ci) in c.iter_mut().enumerate() {
            *ci = (0..m).fold(C64::new(0.0, 0.0), |a, j| a + h[i * m + j] * d[j]);
        }
    }
    c.iter().map(|v| 20.0 * v.norm().log10()).collect()
}

#[test]
fn mimo_sine_every_builtin_law_aims_for_the_notched_reference() {
    let s = setup();
    let want = 20.0 * DEPTH.log10();
    let laws: Vec<(&str, Box<dyn SineLaw>, usize)> = vec![
        (
            "matrix",
            Box::new(MatrixLaw::new(&s, MatrixParams::default()).unwrap()),
            MATRIX_VALUES.len(),
        ),
        (
            "paired",
            Box::new(PairedLaw::new(&s, PairedParams::diagonal(2, 2)).unwrap()),
            PAIRED_VALUES.len(),
        ),
        (
            "steepest_descent",
            Box::new(SteepestLaw::new(&s, SteepestParams::default()).unwrap()),
            STEEPEST_VALUES.len(),
        ),
        (
            "secant",
            Box::new(SecantLaw::new(&s, SecantParams::default()).unwrap()),
            SECANT_VALUES.len(),
        ),
        (
            "ratio",
            Box::new(RatioLaw::new(&s, RatioParams::default()).unwrap()),
            RATIO_VALUES.len(),
        ),
    ];
    for (name, mut law, values) in laws {
        let end = end_db(law.as_mut(), values, 3000);
        println!("{name}: controls at {end:.3?} dB, the notched reference at {want:.3} dB");
        for e in end {
            assert!((e - want).abs() < 0.25, "{name}: {e} against {want}");
        }
    }
}
