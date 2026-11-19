//! What each piece of `matrix` is for, shown by taking it away. Each test
//! failed first with its piece removed (see the session record).

mod common;
use common::*;
use hertzforge_mimo_sine_law_math::law::worst_error_db;
use hertzforge_mimo_sine_law_math::linalg::{mat_vec, reg_pinv};
use hertzforge_mimo_sine_law_math::matrix::{MatrixLaw, MatrixParams, MATRIX_VALUES};
use hertzforge_mimo_sine_law_math::readings::opposition_ratio;
use hertzforge_mimo_sine_law_math::reference_structure::{
    run_closed_loop, Dynamics, ModalStructure, Played, Row, Schedule,
};
use hertzforge_mimo_sine_law_math::speed::GainRule;
use hertzforge_mimo_sine_law_math::{Setup, C64};

fn case(name: &str) -> (ModalStructure, Setup) {
    let data = load("matrix_parity.json");
    let c = &data[name];
    let st = structure(c);
    let setup = st.setup(band(c), true);
    (st, setup)
}

fn run(
    st: &ModalStructure,
    setup: &Setup,
    p: MatrixParams,
    s: Schedule,
    played: Played,
) -> Result<Vec<Row>, String> {
    let mut law = MatrixLaw::new(setup, p)?;
    run_closed_loop(
        &mut law,
        st,
        setup,
        s,
        played,
        Dynamics::Envelope,
        MATRIX_VALUES.len(),
        100_000,
    )
}

/// Worst control error, dB, over the updates after the first second.
fn worst_after(rows: &[Row], skip: usize) -> f64 {
    let ones = vec![C64::new(1.0, 0.0); rows[0].controls.len()];
    rows.iter()
        .skip(skip)
        .map(|r| worst_error_db(&r.controls, &ones))
        .fold(0.0, f64::max)
}

#[test]
fn mimo_sine_fixed_published_gain_loses_the_loop_where_the_rule_holds_it() {
    let (st, setup) = case("lin4x4");
    let dwell = Schedule::Dwell { f: 69.0, secs: 3.0 };
    let rule = run(&st, &setup, MatrixParams::default(), dwell, Played::AsAsked).unwrap();
    let fixed = MatrixParams {
        gain_rule: GainRule::Fixed { g: 0.5 },
        ..Default::default()
    };
    let pub_ = run(&st, &setup, fixed, dwell, Played::AsAsked);
    let e_rule = worst_after(&rule, 150);
    let e_fixed = pub_
        .as_ref()
        .map(|r| worst_after(r, 150))
        .unwrap_or(f64::INFINITY);
    eprintln!("dwell at 69 Hz: rule {e_rule:.3} dB, fixed 0.5 {e_fixed:.1} dB");
    assert!(e_rule < 0.1, "the rule holds the loop: {e_rule}");
    assert!(e_fixed > 12.0, "the published gain loses it: {e_fixed}");
}

#[test]
fn mimo_sine_carry_lowers_the_swept_error_and_weight_zero_is_the_correction_alone() {
    let (st, setup) = case("lin4x4");
    let sweep = Schedule::Sweep {
        f0: 60.0,
        f1: 85.0,
        oct_min: 4.0,
    };
    let carried = run(&st, &setup, MatrixParams::default(), sweep, Played::AsAsked).unwrap();
    let alone = run(
        &st,
        &setup,
        MatrixParams {
            carry: 0.0,
            ..Default::default()
        },
        sweep,
        Played::AsAsked,
    )
    .unwrap();
    let (ec, ea) = (worst_after(&carried, 100), worst_after(&alone, 100));
    eprintln!("sweep 60-85 Hz at 4 oct/min: carry 0.67 {ec:.2} dB, carry 0 {ea:.2} dB");
    assert!(
        ec < 0.5 * ea,
        "the carry lowers the swept error: {ec} against {ea}"
    );
    // Weight 0, reference of ones: each drive is the drive in force plus g Z(f) (R - C).
    let mut law = MatrixLaw::new(
        &setup,
        MatrixParams {
            carry: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    let model = law.model().clone();
    let (n, m) = (st.controls, st.drives);
    for k in 200..203 {
        let (prev, row) = (&alone[k - 1], &alone[k]);
        let mut h = vec![C64::new(0.0, 0.0); n * m];
        model.at(row.f, &mut h);
        let (z, _) = reg_pinv(&h, n, m, 1e-3).unwrap();
        let e: Vec<C64> = row
            .controls
            .iter()
            .map(|c| C64::new(1.0, 0.0) - c)
            .collect();
        let mut step = vec![C64::new(0.0, 0.0); m];
        mat_vec(&z, m, n, &e, &mut step);
        let g = row.values[0];
        let want: Vec<C64> = prev
            .drive
            .iter()
            .zip(&step)
            .map(|(d, s)| d + s * g)
            .collect();
        assert!(rel_err(&row.drive, &want) < 1e-12, "update {k}");
    }
    let _ = &mut law;
}

#[test]
fn mimo_sine_rebase_keeps_a_ceilinged_drive_from_winding_up() {
    let (st, setup) = case("lin4x4");
    let dwell = Schedule::Dwell { f: 69.0, secs: 4.0 };
    let free = run(&st, &setup, MatrixParams::default(), dwell, Played::AsAsked).unwrap();
    let need = free
        .last()
        .unwrap()
        .drive
        .iter()
        .map(|v| v.norm())
        .fold(0.0, f64::max);
    let ceiling = Played::Ceiling(0.5 * need);
    let with = run(&st, &setup, MatrixParams::default(), dwell, ceiling).unwrap();
    let without = run(
        &st,
        &setup,
        MatrixParams {
            rebase: false,
            ..Default::default()
        },
        dwell,
        ceiling,
    )
    .unwrap();
    let top = |r: &Row| r.asked.iter().map(|v| v.norm()).fold(0.0, f64::max) / (0.5 * need);
    let (a, b) = (top(with.last().unwrap()), top(without.last().unwrap()));
    eprintln!("asked over ceiling after 4 s: rebased {a:.2}, not rebased {b:.1}");
    assert!(a < 1.5, "rebased: the ask stays near what is played: {a}");
    assert!(b > 5.0, "not rebased: the ask winds up: {b}");
}

#[test]
fn mimo_sine_reset_keeps_spare_drives_from_growing_on_a_three_by_six() {
    // The whole band of the structure's elastic modes, so the row space of
    // the response turns as far as a full test's would.
    let (st, _) = case("lin3x6");
    let setup = st.setup((40.0, 400.0), false);
    let sweep = Schedule::Sweep {
        f0: 40.0,
        f1: 400.0,
        oct_min: 4.0,
    };
    let opp = |rows: &[Row]| {
        let (n, m) = (st.controls, st.drives);
        let mut h = vec![C64::new(0.0, 0.0); n * m];
        rows.iter()
            .skip(1)
            .map(|r| {
                st.frf(r.f, &mut h);
                20.0 * opposition_ratio(&h, n, m, &r.drive).unwrap().log10()
            })
            .fold(0.0, f64::max)
    };
    let with = run(&st, &setup, MatrixParams::default(), sweep, Played::AsAsked).unwrap();
    let without = run(
        &st,
        &setup,
        MatrixParams {
            reset_unseen: false,
            ..Default::default()
        },
        sweep,
        Played::AsAsked,
    )
    .unwrap();
    let (a, b) = (opp(&with), opp(&without));
    eprintln!("largest opposition ratio over the sweep: reset {a:.2} dB, no reset {b:.2} dB");
    assert!(a < 0.5, "reset: {a}");
    assert!(b > 3.0, "no reset: {b}");
}
