//! The MIMO Sine template (`template/sine-mimo`), its law compiled natively
//! and run through the SDK's trait, so the code a law author copies builds and
//! does what its documentation says. The same source is built as a law file
//! (`law-fixture-sine-mimo-template`) and loaded by the law host's tests.

#[path = "../template/sine-mimo/src/law.rs"]
mod law;

use hertzforge_law_sdk::sine_mimo::*;
use hertzforge_law_sdk::{Param, ParamValue, Params, SineMimoLaw};
use law::PairRatio;

fn setup(controls: usize, drives: usize) -> Setup {
    let grid = vec![50.0, 100.0, 150.0];
    let lines = grid.len();
    let mut response = MatrixStack::zeros(lines, controls, drives);
    for k in 0..lines {
        for i in 0..controls.min(drives) {
            response.set(k, i, i, C64::new(0.0, 2.0 + k as f64));
        }
    }
    Setup {
        controls,
        drives,
        limits: 0,
        fs_hz: 10240.0,
        tick_s: 0.01,
        band_lo_hz: 50.0,
        band_hi_hz: 150.0,
        test_type: TestType::Dwell,
        shape: math::Shape::of(controls, drives),
        drive_ceiling_v: vec![1.0; drives],
        identification: Identification {
            frequency_hz: grid,
            response,
            limit_response: MatrixStack::zeros(lines, 0, drives),
            coherence: vec![1.0; controls * lines],
            limit_coherence: Vec::new(),
            trusted: vec![true; lines],
        },
        identification_level_db: -20.0,
        reference: vec![C64::new(1.0, 0.0); controls * lines],
        phase_specified: true,
        limit_bounds: Vec::new(),
        bands: Bands {
            alarm_low_db: vec![3.0; controls * lines],
            alarm_high_db: vec![3.0; controls * lines],
            abort_low_db: vec![6.0; controls * lines],
            abort_high_db: vec![6.0; controls * lines],
        },
        tracking: TrackingRule::Proportional {
            pct: 10.0,
            max_hz: 20.0,
        },
        loop_delay: Default::default(),
    }
}

fn params(gain: f64, max_step_db: f64) -> Params {
    Params(vec![
        Param {
            key: "gain".into(),
            value: ParamValue::Number(gain),
        },
        Param {
            key: "max_step_db".into(),
            value: ParamValue::Number(max_step_db),
        },
    ])
}

fn inputs(controls: Vec<C64>, drive_in_force: Vec<C64>, hold: Hold) -> Inputs {
    Inputs {
        index: 1,
        dt_s: 0.01,
        frequency_hz: 100.0,
        sweep_hz_per_s: 0.0,
        sweep_oct_per_min: 0.0,
        level: 1.0,
        reference: vec![C64::new(1.0, 0.0); controls.len()],
        controls,
        limits: Vec::new(),
        limit_bounds: Vec::new(),
        drive_in_force,
        hold,
        notch_scale: 1.0,
        settling: false,
    }
}

#[test]
fn mimo_sine_the_template_states_what_it_accepts() {
    let m = PairRatio::describe();
    assert_eq!(m.law.name, "pair-ratio");
    assert_eq!(manifest_shapes(&m), ALL_SHAPES);
    assert_eq!(
        manifest_test_types(&m),
        [TestType::Dwell, TestType::Stepped]
    );
    assert!(m.amplitude_only);
    let keys: Vec<&str> = m.law.params.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(keys, ["gain", "max_step_db"]);
    assert_eq!(m.law.diagnostics[0].key, "error_db");
}

#[test]
fn mimo_sine_the_template_starts_on_its_own_paths_and_follows_the_ratio() {
    // Three controls, two drives: control 2 has no drive.
    let mut law = PairRatio::new(setup(3, 2), params(0.5, 6.0)).unwrap();
    let zero = vec![C64::new(0.0, 0.0); 2];
    let first = law
        .engage(&inputs(vec![C64::new(0.0, 0.0); 3], zero, Hold::Engage))
        .unwrap();
    // R / H at 100 Hz, the middle line: 1 / (3 j) = -j / 3.
    for d in &first.drive {
        assert!((d - C64::new(0.0, -1.0 / 3.0)).norm() < 1e-12, "{d}");
    }
    let half = C64::new(0.5, 0.0);
    let held = law
        .tick(&inputs(vec![half; 3], first.drive.clone(), Hold::Engage))
        .unwrap();
    assert_eq!(held.drive, first.drive, "the settle window holds the drive");
    let next = law
        .tick(&inputs(vec![half; 3], first.drive.clone(), Hold::Free))
        .unwrap();
    // Half the wanted amplitude, gain 0.5: up by 3 dB, the phase kept.
    let up = 10f64.powf(0.5 * 20.0 * 2f64.log10() / 20.0);
    for (d, f) in next.drive.iter().zip(&first.drive) {
        assert!((d / f - up).norm() < 1e-12);
    }
    assert!((next.values[0].1 - 20.0 * 2f64.log10()).abs() < 1e-12);
    // The step is capped.
    let mut law = PairRatio::new(setup(2, 2), params(1.0, 1.0)).unwrap();
    let tiny = vec![C64::new(1e-3, 0.0); 2];
    let capped = law
        .tick(&inputs(tiny, vec![C64::new(0.1, 0.0); 2], Hold::Free))
        .unwrap();
    assert!((capped.drive[0].re - 0.1 * 10f64.powf(1.0 / 20.0)).abs() < 1e-12);
}
