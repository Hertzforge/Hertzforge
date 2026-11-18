//! `matrix` carries nothing where the model is not modal (inside the window
//! of a group the fit left out), and switching the carry puts no step in the
//! drive.

mod common;
use common::*;
use hertzforge_mimo_sine_law_math::matrix::{MatrixLaw, MatrixParams, MATRIX_VALUES};
use hertzforge_mimo_sine_law_math::modal::{FitLimits, Model};
use hertzforge_mimo_sine_law_math::reference_structure::{
    run_closed_loop, Dynamics, Played, Schedule,
};

#[test]
fn mimo_sine_carry_is_off_inside_a_left_out_window_with_no_step_in_the_drive() {
    let st = structure(&load("matrix_parity.json")["lin4x4"]);
    let band = (30.0, 120.0);
    let setup = st.setup(band, true);
    let tight = FitLimits {
        max_group_work: 100_000_000,
        split: false,
        ..FitLimits::default()
    };
    let model = Model::build_with(&setup.identification, band, Default::default(), tight).unwrap();
    let (lo, hi) = model.fit.left_out_windows[0];
    let mut law = MatrixLaw::with_model(&setup, MatrixParams::default(), model).unwrap();
    let sweep = Schedule::Sweep {
        f0: 31.0,
        f1: 50.0,
        oct_min: 1.0,
    };
    let rows = run_closed_loop(
        &mut law,
        &st,
        &setup,
        sweep,
        Played::AsAsked,
        Dynamics::Envelope,
        MATRIX_VALUES.len(),
        100_000,
    )
    .unwrap();
    let share = 3; // carried_share
    let inside = |f: f64| f >= lo - 1.0 && f <= hi + 1.0;
    let deep = |f: f64| f > lo + 1.0 && f < hi - 1.0;
    assert!(
        rows.iter()
            .filter(|r| deep(r.f))
            .all(|r| r.values[share] == 0.0),
        "carry off inside"
    );
    assert!(
        rows.iter()
            .skip(40)
            .filter(|r| !inside(r.f))
            .all(|r| r.values[share] > 0.0),
        "carry on outside"
    );
    // The switch: the drive's change at the update the carry turns off is no
    // larger than the changes just before it.
    let step = |k: usize| {
        let d: f64 = rows[k]
            .drive
            .iter()
            .zip(&rows[k - 1].drive)
            .map(|(a, b)| (a - b).norm_sqr())
            .sum();
        let n: f64 = rows[k].drive.iter().map(|a| a.norm_sqr()).sum();
        (d / n).sqrt()
    };
    let s = (41..rows.len())
        .find(|&k| rows[k].values[share] == 0.0 && rows[k - 1].values[share] > 0.0)
        .unwrap();
    let before = (s - 20..s).map(step).fold(0.0, f64::max);
    eprintln!(
        "carry off at {:.2} Hz: step {:.2e} against {:.2e} before",
        rows[s].f,
        step(s),
        before
    );
    assert!(step(s) <= 1.5 * before, "{} {}", step(s), before);
}
