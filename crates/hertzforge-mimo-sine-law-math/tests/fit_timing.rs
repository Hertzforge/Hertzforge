//! The modal fit's time and its bounds. The two timings are ignored by
//! default (run with `--ignored`, optimized build); the bound test runs always.

mod common;
use common::*;
use hertzforge_mimo_sine_law_math::modal::{FitLimits, GroupOutcome, ModalForm, Model};
use hertzforge_mimo_sine_law_math::reference_structure::ModalStructure;
use std::time::Instant;

/// A seeded 16 controls by 8 drives structure with `modes` modes log-spaced
/// over 6 to 1900 Hz, damping 1 to 3 percent, shapes uniform in -1 to 1.
fn big(modes: usize) -> ModalStructure {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut u = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x >> 11) as f64 / (1u64 << 53) as f64
    };
    let (n, m) = (16, 8);
    let f_hz: Vec<f64> = (0..modes)
        .map(|r| 6.0 * (1900.0f64 / 6.0).powf((r as f64 + 0.3 + 0.4 * u()) / modes as f64))
        .collect();
    let zeta = (0..modes).map(|_| 0.01 + 0.02 * u()).collect();
    let drive_shape = (0..modes * m).map(|_| 2.0 * u() - 1.0).collect();
    let output_shape = (0..modes * n).map(|_| 2.0 * u() - 1.0).collect();
    ModalStructure {
        f_hz,
        zeta,
        drive_shape,
        output_shape,
        drives: m,
        controls: n,
    }
}

fn time_fit(st: &ModalStructure, band: (f64, f64), limits: FitLimits) {
    let setup = st.setup(band, true);
    let t = Instant::now();
    let (mf, rep) = ModalForm::fit_bounded(&setup.identification, band, limits).unwrap();
    let secs = t.elapsed().as_secs_f64();
    eprintln!(
        "{} x {} over {:?} Hz, {} lines: {secs:.2} s, {} modes kept, {} candidates left out, work {:.2e}",
        st.controls,
        st.drives,
        band,
        setup.identification.grid_hz.len(),
        mf.modes(),
        rep.candidates_left_out,
        rep.total_work as f64
    );
    for g in &rep.groups {
        eprintln!(
            "  group {:2} candidates, {:5} lines, {:6} evaluations, work {:.2e}, {:?}",
            g.candidates, g.lines, g.evaluations, g.work as f64, g.outcome
        );
    }
}

#[test]
fn mimo_sine_fit_bound_leaves_a_group_out_and_the_model_still_builds() {
    use hertzforge_mimo_sine_law_math::matrix::{MatrixLaw, MatrixParams};
    let st = structure(&load("matrix_parity.json")["lin4x4"]);
    // Over 40 to 400 Hz the identification's scatter merges 15 candidates into
    // one group, over the size bound: it is left out, the rest is fitted.
    let setup = st.setup((40.0, 400.0), true);
    let law = MatrixLaw::new(&setup, MatrixParams::default()).unwrap();
    let fit = &law.model().fit;
    assert_eq!(fit.candidates_left_out, 15);
    assert!(fit
        .groups
        .iter()
        .any(|g| g.outcome == GroupOutcome::TooLarge));
    assert_eq!(law.model().modes.modes(), 5);
    // A work bound hit while fitting: the eight-candidate group over 30 to
    // 120 Hz needs about 1.8e9 units; with 1e8 it stops and is left out.
    let setup = st.setup((30.0, 120.0), true);
    let tight = FitLimits {
        max_group_work: 100_000_000,
        ..FitLimits::default()
    };
    let (mf, rep) = ModalForm::fit_bounded(&setup.identification, (30.0, 120.0), tight).unwrap();
    let out: Vec<_> = rep
        .groups
        .iter()
        .filter(|g| g.outcome == GroupOutcome::OutOfWork)
        .collect();
    assert_eq!(out.len(), 1);
    assert!(out[0].work <= 100_000_000);
    assert_eq!(rep.candidates_left_out, 8);
    assert_eq!(mf.modes(), 1);
    let model = Model::from_modes(&setup.identification, mf, 8).unwrap();
    assert_eq!(model.modes.modes(), 1);
}

#[test]
#[ignore = "timing: run with --ignored in an optimized build"]
fn mimo_sine_fit_time_four_by_four_scatter_40_to_400_hz() {
    let st = structure(&load("matrix_parity.json")["lin4x4"]);
    time_fit(&st, (40.0, 400.0), FitLimits::default());
}

#[test]
#[ignore = "timing: run with --ignored in an optimized build"]
fn mimo_sine_fit_time_sixteen_by_eight_5_to_2000_hz() {
    // 200 modes over the band, and a cluster of eight close modes near
    // 1500 Hz so one group of the largest size fits over a wide window.
    let mut st = big(200);
    for r in 0..8 {
        st.f_hz.push(1500.0 * (1.0 + 0.012 * r as f64));
        st.zeta.push(0.02);
        for j in 0..st.drives {
            st.drive_shape
                .push(if (r + j) % 3 == 0 { 0.8 } else { -0.3 });
        }
        for i in 0..st.controls {
            st.output_shape
                .push(if (r * 7 + i) % 5 == 0 { 0.9 } else { 0.2 });
        }
    }
    time_fit(&st, (5.0, 2000.0), FitLimits::default());
    // The work bounds alone, with no cap on a group's size: the worst the
    // counts allow.
    let open = FitLimits {
        max_group: usize::MAX,
        ..FitLimits::default()
    };
    time_fit(&st, (5.0, 2000.0), open);
}

#[test]
#[ignore = "diagnosis: the fit with no bounds (minutes)"]
fn mimo_sine_fit_time_four_by_four_unbounded() {
    let st = structure(&load("matrix_parity.json")["lin4x4"]);
    let none = FitLimits {
        max_group: usize::MAX,
        max_iterations: 40,
        max_evaluations: usize::MAX,
        max_group_work: u64::MAX,
        max_total_work: u64::MAX,
    };
    time_fit(&st, (40.0, 400.0), none);
}
