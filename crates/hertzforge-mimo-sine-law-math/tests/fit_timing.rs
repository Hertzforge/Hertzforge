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
fn mimo_sine_fit_splits_a_long_chain_and_keeps_its_true_modes() {
    let st = structure(&load("matrix_parity.json")["lin4x4"]);
    let band = (40.0, 400.0);
    let setup = st.setup(band, true);
    // Unsplit, the scatter chains 15 candidates over 330 to 400 Hz into one
    // group over the size bound, and the group is left out whole.
    let whole = FitLimits {
        split: false,
        ..FitLimits::default()
    };
    let (mf0, rep0) = ModalForm::fit_bounded(&setup.identification, band, whole).unwrap();
    assert_eq!(rep0.candidates_left_out, 15);
    assert!(rep0
        .groups
        .iter()
        .any(|g| g.outcome == GroupOutcome::TooLarge));
    let in_chain = (330.0, 400.0);
    assert_eq!(true_modes_found(&st, in_chain, &mf0, 0.1), (0, 3));
    // Split: nothing left out, and the three true modes in the chain are found
    // within a tenth of a bandwidth.
    let (mf, rep) =
        ModalForm::fit_bounded(&setup.identification, band, FitLimits::default()).unwrap();
    assert_eq!(rep.candidates_left_out, 0);
    assert!(rep.groups.iter().all(|g| g.candidates <= 8));
    assert_eq!(true_modes_found(&st, in_chain, &mf, 0.1), (3, 3));
}

#[test]
fn mimo_sine_fit_work_bound_leaves_a_group_out_and_the_model_still_builds() {
    let st = structure(&load("matrix_parity.json")["lin4x4"]);
    // The eight-candidate group over 30 to 120 Hz needs about 1.8e9 units;
    // with 1e8 it stops and is left out, its window recorded.
    let setup = st.setup((30.0, 120.0), true);
    let tight = FitLimits {
        max_group_work: 100_000_000,
        split: false,
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
    assert_eq!(rep.left_out_windows.len(), 1);
    assert_eq!(mf.modes(), 1);
    let model = Model::build_with(
        &setup.identification,
        (30.0, 120.0),
        Default::default(),
        tight,
    )
    .unwrap();
    assert!(model.left_out_at(80.0) && !model.left_out_at(31.0));
    // Splitting, the group that runs out of work is halved and its halves
    // fitted in its place: fewer candidates are left out.
    let split = FitLimits {
        split: true,
        ..tight
    };
    let (mf2, rep2) = ModalForm::fit_bounded(&setup.identification, (30.0, 120.0), split).unwrap();
    eprintln!(
        "split on work bound: kept {}, left out {}",
        mf2.modes(),
        rep2.candidates_left_out
    );
    assert!(rep2.candidates_left_out < 8 && mf2.modes() > 1);
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
        split: false,
        subtract_fitted: false,
    };
    time_fit(&st, (40.0, 400.0), none);
}

/// True modes of `st` within `band` matched by a fitted pole within `tol`
/// half-power bandwidths in frequency and a factor 1.5 in damping.
fn true_modes_found(
    st: &ModalStructure,
    band: (f64, f64),
    mf: &ModalForm,
    tol: f64,
) -> (usize, usize) {
    let mut found = 0;
    let mut total = 0;
    for r in 0..st.f_hz.len() {
        let (f, z) = (st.f_hz[r], st.zeta[r]);
        if f < band.0 || f > band.1 {
            continue;
        }
        total += 1;
        let bw = 2.0 * z * f;
        if (0..mf.modes())
            .any(|j| (mf.f_hz[j] - f).abs() < tol * bw && (mf.zeta[j] / z).ln().abs() < 1.5f64.ln())
        {
            found += 1;
        }
    }
    (found, total)
}

#[test]
#[ignore = "diagnosis: split with and without subtracting the groups already fitted"]
fn mimo_sine_fit_split_diagnosis() {
    let st = structure(&load("matrix_parity.json")["lin4x4"]);
    let setup = st.setup((40.0, 400.0), true);
    let before = FitLimits {
        split: false,
        ..FitLimits::default()
    };
    let m0 = Model::build_with(
        &setup.identification,
        (40.0, 400.0),
        Default::default(),
        before,
    )
    .unwrap();
    eprintln!(
        "no split (left out): model error {:.4}",
        model_error(&st, &setup, &m0)
    );
    let id = Model::build_with(
        &setup.identification,
        (40.0, 400.0),
        hertzforge_mimo_sine_law_math::modal::ModelChoice::Identification,
        before,
    )
    .unwrap();
    eprintln!(
        "identification as measured: model error {:.4}",
        model_error(&st, &setup, &id)
    );
    for subtract in [false, true] {
        let lim = FitLimits {
            subtract_fitted: subtract,
            ..FitLimits::default()
        };
        let t = Instant::now();
        let (mf, rep) = ModalForm::fit_bounded(&setup.identification, (40.0, 400.0), lim).unwrap();
        let (found, total) = true_modes_found(&st, (40.0, 400.0), &mf, 0.1);
        eprintln!(
            "subtract {subtract}: {:.2} s, {} modes kept, {found} of {total} true within 0.1 bandwidth, left out {}, groups {:?}",
            t.elapsed().as_secs_f64(),
            mf.modes(),
            rep.candidates_left_out,
            rep.groups.iter().map(|g| (g.candidates, g.lines)).collect::<Vec<_>>()
        );
        let model = Model::build_with(
            &setup.identification,
            (40.0, 400.0),
            Default::default(),
            lim,
        )
        .unwrap();
        eprintln!(
            "  model error against the truth: {:.4}",
            model_error(&st, &setup, &model)
        );
        eprintln!(
            "  fitted {:?}",
            mf.f_hz
                .iter()
                .zip(&mf.zeta)
                .map(|(f, z)| (f.round(), (z * 1e4).round() / 1e4))
                .collect::<Vec<_>>()
        );
    }
}

/// RMS over the grid of each line's |model - truth| / |truth| (Frobenius).
fn model_error(
    st: &ModalStructure,
    setup: &hertzforge_mimo_sine_law_math::Setup,
    model: &Model,
) -> f64 {
    use hertzforge_mimo_sine_law_math::C64;
    let (n, m) = (st.controls, st.drives);
    let mut h = vec![C64::new(0.0, 0.0); n * m];
    let mut t = vec![C64::new(0.0, 0.0); n * m];
    let g = &setup.identification.grid_hz;
    let mut acc = 0.0;
    for &f in g {
        model.at(f, &mut h);
        st.frf(f, &mut t);
        let d: f64 = h.iter().zip(&t).map(|(a, b)| (a - b).norm_sqr()).sum();
        let r: f64 = t.iter().map(|b| b.norm_sqr()).sum();
        acc += d / r;
    }
    (acc / g.len() as f64).sqrt()
}

#[test]
#[ignore = "diagnosis: the parity case's group work"]
fn mimo_sine_fit_parity_case_work() {
    let st = structure(&load("matrix_parity.json")["lin4x4"]);
    let setup = st.setup((30.0, 120.0), true);
    let (_, rep) =
        ModalForm::fit_bounded(&setup.identification, (30.0, 120.0), FitLimits::default()).unwrap();
    for g in &rep.groups {
        eprintln!(
            "group {} candidates, {} lines, {} evaluations, work {:.3e}",
            g.candidates, g.lines, g.evaluations, g.work as f64
        );
    }
}
