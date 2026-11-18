//! A law stays consistent with a drive the controller scaled for a level
//! change. When the level moves, the controller scales the drive in force by
//! the same ratio and tells the law the result; on a linear structure the
//! whole problem is then the old one times that ratio, so a law's next drives
//! must be the old run's drives times the ratio. `secant` keeps a base
//! reading, its probe and its last control step of its own; those have to be
//! carried with the level too.

use hertzforge_mimo_sine_law_math::law::{Hold, Inputs, Outputs, SineLaw};
use hertzforge_mimo_sine_law_math::matrix::{MatrixLaw, MatrixParams, MATRIX_VALUES};
use hertzforge_mimo_sine_law_math::reference_structure::ModalStructure;
use hertzforge_mimo_sine_law_math::secant::{SecantLaw, SecantParams, SECANT_VALUES};
use hertzforge_mimo_sine_law_math::setup::Setup;
use hertzforge_mimo_sine_law_math::steepest::{SteepestLaw, SteepestParams, STEEPEST_VALUES};
use hertzforge_mimo_sine_law_math::C64;

const F_HZ: f64 = 70.0;

fn structure() -> ModalStructure {
    ModalStructure {
        f_hz: vec![4.0, 4.5, 60.0, 85.0],
        zeta: vec![0.05, 0.05, 0.02, 0.03],
        drive_shape: vec![1.0, 0.3, 0.2, 1.0, 0.8, 0.4, 0.3, 0.7],
        output_shape: vec![1.0, 0.2, 0.3, 1.0, 0.6, 0.3, 0.25, 0.5],
        drives: 2,
        controls: 2,
    }
}

/// The response the loop plays through: the identified one, off by a complex
/// factor per control so a law has something to learn.
fn truth(s: &Setup) -> Vec<C64> {
    let (n, m) = (s.controls, s.drives);
    let mut h = vec![C64::new(0.0, 0.0); n * m];
    s.response_at(F_HZ, &mut h);
    let off = [C64::from_polar(1.3, 0.2), C64::from_polar(0.8, -0.15)];
    for i in 0..n {
        for j in 0..m {
            h[i * m + j] *= off[i];
        }
    }
    h
}

/// Run `law` at a dwell for `updates` on a structure answering at once
/// (`C = H D`, no lag), the level changed by `ratio` at the updates in
/// `at` (and the drive in force scaled with it, as the controller does).
/// Returns each update's drive asked and its level.
fn run(
    law: &mut dyn SineLaw,
    values: usize,
    s: &Setup,
    updates: usize,
    at: &[usize],
    ratio: f64,
) -> Vec<(Vec<C64>, f64, Vec<f64>)> {
    let (n, m) = (s.controls, s.drives);
    let h = truth(s);
    let mut level = 0.25;
    let mut d = vec![C64::new(0.0, 0.0); m];
    let mut c = vec![C64::new(0.0, 0.0); n];
    let mut out = Outputs::new(m, values);
    let mut rows = Vec::new();
    for k in 0..updates {
        if at.contains(&k) {
            level *= ratio;
            d.iter_mut().for_each(|v| *v *= ratio);
            c.iter_mut().for_each(|v| *v *= ratio);
        }
        let r = vec![C64::new(level, 0.0); n];
        let x = Inputs {
            index: k as u64,
            dt_s: if k == 0 { 0.0 } else { s.tick_s },
            f_hz: F_HZ,
            sweep_hz_s: 0.0,
            sweep_oct_min: 0.0,
            level,
            reference: &r,
            controls: &c,
            limits: &[],
            limit_bounds: &[],
            drive_in_force: &d,
            hold: if k == 0 { Hold::Engage } else { Hold::Free },
            notch_scale: 1.0,
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
        rows.push((out.drive.clone(), level, out.values.clone()));
    }
    rows
}

/// The largest relative difference between a run with level changes and the
/// same run without them, scaled, from the first change on.
fn departure(plain: &[(Vec<C64>, f64, Vec<f64>)], moved: &[(Vec<C64>, f64, Vec<f64>)]) -> f64 {
    plain
        .iter()
        .zip(moved)
        .map(|((a, la, _), (b, lb, _))| {
            let k = lb / la;
            a.iter()
                .zip(b)
                .map(|(x, y)| (x * k - y).norm() / (x * k).norm().max(1e-300))
                .fold(0.0, f64::max)
        })
        .fold(0.0, f64::max)
}

fn setup() -> Setup {
    structure().setup((30.0, 120.0), false)
}

#[test]
fn mimo_sine_secant_follows_a_drive_scaled_for_a_level_change() {
    let s = setup();
    let p = SecantParams {
        settle: Some(3),
        ..Default::default()
    };
    let plain = run(
        &mut SecantLaw::new(&s, p).unwrap(),
        SECANT_VALUES.len(),
        &s,
        200,
        &[],
        1.0,
    );
    // Updates right after a probe went out (its reading not yet taken) and
    // right after a control step (its secant pair not yet used): the level
    // moves there, once each way.
    let probe = (1..plain.len())
        .find(|&k| plain[k].2[3] == 1.0 && plain[k - 1].2[3] == 0.0)
        .expect("a probe");
    let control = (probe + 1..plain.len())
        .find(|&k| plain[k].2[3] == 0.0 && plain[k - 1].2[3] == 1.0)
        .expect("a control step");
    let at = [probe + 1, control + 1, control + 20];
    let moved = run(
        &mut SecantLaw::new(&s, p).unwrap(),
        SECANT_VALUES.len(),
        &s,
        200,
        &at,
        2.0,
    );
    // Each change doubles, so undo by the level ratio row by row.
    let dep = departure(&plain, &moved);
    println!("secant: probe at {probe}, control step at {control}; departure {dep:.2e}");
    assert!(dep < 1e-9, "{dep}");
}

#[test]
fn mimo_sine_matrix_and_steepest_descent_follow_a_scaled_drive_with_no_state() {
    let s = setup();
    for (name, mk) in [
        (
            "matrix",
            Box::new(|| {
                Box::new(MatrixLaw::new(&setup(), MatrixParams::default()).unwrap())
                    as Box<dyn SineLaw>
            }) as Box<dyn Fn() -> Box<dyn SineLaw>>,
        ),
        (
            "steepest_descent",
            Box::new(|| {
                Box::new(SteepestLaw::new(&setup(), SteepestParams::default()).unwrap())
                    as Box<dyn SineLaw>
            }),
        ),
    ] {
        let values = if name == "matrix" {
            MATRIX_VALUES.len()
        } else {
            STEEPEST_VALUES.len()
        };
        let plain = run(mk().as_mut(), values, &s, 120, &[], 1.0);
        let moved = run(mk().as_mut(), values, &s, 120, &[10, 37, 80], 2.0);
        let dep = departure(&plain, &moved);
        println!("{name}: departure {dep:.2e}");
        assert!(dep < 1e-9, "{name}: {dep}");
    }
}
