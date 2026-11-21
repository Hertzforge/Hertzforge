//! N. Hunter's refinements of the two independent-drive laws: the weighting
//! by the target, the non-negative solve, the exactness test, and the level
//! set's worked example (its law tests are in `level_set.rs`).
//!
//! The worked example is the one of his note "The Independent Drive Law,
//! Step by Step": `H = [[1.0, 0.4], [0.5, 1.0], [0.8, -0.6]]` and three
//! target sets, with the regularization constant at zero. The note prints
//! four significant figures (three for the trial response, its scale factors
//! and its rescale), so each value here is held to half a unit in its last
//! printed figure, except one rescale entry whose printed value does not
//! follow from the note's own figures (see case 2).

#![allow(clippy::needless_range_loop)]

mod common;

use common::*;
use hertzforge_mimo_laws::{
    independent_levels_with, nearly_independent_reference_with, nonnegative_least_squares,
    squared_magnitudes, BuiltinMimoLaw, IndependentDrivesLaw, LevelSolve, NearlyIndependentLaw,
    ParamVal, SolveOptions, NNLS_ITERATIONS_PER_UNKNOWN,
};
use hertzforge_sdm::law::{MimoControlLaw, MimoLoopInputs, MimoLoopOutputs, MimoSetup, Tolerances};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

const ONE: (usize, usize) = (0, 1);

/// The note's structure, one line, real.
fn note_frf() -> MatrixStack {
    let h = [1.0, 0.4, 0.5, 1.0, 0.8, -0.6];
    MatrixStack::from_vec(1, 3, 2, h.iter().map(|v| C64::new(*v, 0.0)).collect()).expect("dims")
}

fn targets(t: [f64; 3]) -> HermitianStack {
    let mut r = HermitianStack::zeros(1, 3);
    for k in 0..3 {
        r.set(0, k, k, C64::new(t[k], 0.0));
    }
    r
}

const CASE1: [f64; 3] = [1.08, 0.75, 0.82];
const CASE2: [f64; 3] = [1.0, 1.0, 1.0];
const CASE3: [f64; 3] = [1.0, 0.1, 1.0];

fn opts(weighted: bool, solve: LevelSolve, c: f64) -> SolveOptions {
    SolveOptions {
        weighted,
        solve,
        regularization_constant: c,
        negative_floor: 0.0,
    }
}

/// `got` within half a unit of the last of `decimals` figures of `want`.
fn printed(what: &str, got: f64, want: f64, decimals: i32) {
    let tol = 0.5 * 10f64.powi(-decimals);
    assert!(
        (got - want).abs() <= tol,
        "{what}: {got} against the note's {want} (to {tol})"
    );
}

fn setup_for(t: [f64; 3]) -> MimoSetup {
    MimoSetup {
        controls: 3,
        drives: 2,
        lines: 1,
        df_hz: 1.0,
        band: ONE,
        reference: targets(t),
        tolerances: Tolerances::none(1, 3),
    }
}

/// A law's seed on the note's structure.
fn seed_on(law: &mut dyn MimoControlLaw, setup: &MimoSetup, h: &MatrixStack) -> MimoLoopOutputs {
    let response = HermitianStack::zeros(1, 3);
    let played = HermitianStack::zeros(1, 2);
    let csd = MatrixStack::zeros(1, 3, 2);
    let coherence = [1.0; 3];
    let inputs = MimoLoopInputs {
        reference: &setup.reference,
        tolerances: &setup.tolerances,
        response: &response,
        response_dof: 128.0,
        frf: h,
        multiple_coherence: &coherence,
        df_hz: 1.0,
        band: ONE,
        loop_index: 0,
        last_drive: Some(&played),
        response_drive_csd: Some(&csd),
        sysid_response: None,
        sysid_drive: None,
        sysid_noise: None,
    };
    law.seed(&inputs).expect("seed")
}

fn per_line(out: &MimoLoopOutputs, key: &str) -> Vec<f64> {
    match &out
        .diagnostics
        .iter()
        .find(|d| d.key == key)
        .unwrap_or_else(|| panic!("no {key}"))
        .data
    {
        hertzforge_sdm::law::DiagnosticData::PerLine(v) => v.clone(),
        other => panic!("{key}: {other:?}"),
    }
}

fn unregularized() -> Vec<(&'static str, ParamVal)> {
    vec![("regularization_constant", ParamVal::Number(0.0))]
}

// -------------------------------------------------------------------------
// 1. The note's worked example
// -------------------------------------------------------------------------

#[test]
fn the_weighted_matrix_of_case_3_is_the_notes() {
    // Row 2 (target 0.1) multiplied by 10: [[1.0, 0.16], [2.5, 10.0], [0.64, 0.36]].
    let a = squared_magnitudes(&note_frf(), &targets(CASE3), ONE, true).expect("a");
    let want = [1.0, 0.16, 2.5, 10.0, 0.64, 0.36];
    for (g, w) in a.line(0).iter().zip(want) {
        assert!((g.re - w).abs() < 1e-12, "{} against {w}", g.re);
    }
    // Unweighted it is |H|^2: [[1.00, 0.16], [0.25, 1.00], [0.64, 0.36]].
    let a = squared_magnitudes(&note_frf(), &targets(CASE3), ONE, false).expect("a");
    let want = [1.0, 0.16, 0.25, 1.0, 0.64, 0.36];
    for (g, w) in a.line(0).iter().zip(want) {
        assert!((g.re - w).abs() < 1e-12, "{} against {w}", g.re);
    }
}

#[test]
fn the_plain_levels_and_the_sign_test_of_cases_2_and_3_are_the_notes() {
    let h = note_frf();
    // Case 2: s_ls = (0.9376, 0.7960), both positive.
    let lv = independent_levels_with(
        &h,
        &targets(CASE2),
        ONE,
        &opts(true, LevelSolve::SetToZero, 0.0),
    )
    .expect("levels");
    printed("case 2 s1", lv.levels[0], 0.9376, 4);
    printed("case 2 s2", lv.levels[1], 0.7960, 4);
    assert_eq!(lv.negative, vec![0.0]);
    // Case 3: s_ls = (1.2046, -0.2003), the sign test fails; set to zero
    // keeps 1.2046.
    let lv = independent_levels_with(
        &h,
        &targets(CASE3),
        ONE,
        &opts(true, LevelSolve::SetToZero, 0.0),
    )
    .expect("levels");
    printed("case 3 s1 kept", lv.levels[0], 1.2046, 4);
    assert_eq!(lv.levels[1], 0.0);
    assert_eq!(lv.negative, vec![1.0]);
}

#[test]
fn the_non_negative_levels_of_case_3_are_the_notes_and_differ_from_set_to_zero() {
    let h = note_frf();
    // The note: s = (0.5405, 0); the paper's rule (1.2046, 0).
    let nn = independent_levels_with(
        &h,
        &targets(CASE3),
        ONE,
        &opts(true, LevelSolve::NonNegative, 0.0),
    )
    .expect("levels");
    println!("case 3 non-negative {:?}", nn.levels);
    printed("case 3 nnls s1", nn.levels[0], 0.5405, 4);
    assert_eq!(nn.levels[1], 0.0);
    assert_eq!(nn.negative, vec![1.0]);
    assert_eq!(nn.held_at_zero, vec![1.0]);
    assert_eq!(nn.fallbacks, 0);
    let clip = independent_levels_with(
        &h,
        &targets(CASE3),
        ONE,
        &opts(true, LevelSolve::SetToZero, 0.0),
    )
    .expect("levels");
    assert!(
        (nn.levels[0] - clip.levels[0]).abs() > 0.6,
        "{nn:?} {clip:?}"
    );
    assert_eq!(clip.held_at_zero, vec![0.0]);
}

#[test]
fn weighting_changes_the_levels_of_case_3() {
    // Unweighted the target of 0.1 counts a tenth as much: other levels.
    let h = note_frf();
    let w = independent_levels_with(
        &h,
        &targets(CASE3),
        ONE,
        &opts(true, LevelSolve::NonNegative, 0.0),
    )
    .expect("levels");
    let u = independent_levels_with(
        &h,
        &targets(CASE3),
        ONE,
        &opts(false, LevelSolve::NonNegative, 0.0),
    )
    .expect("levels");
    println!("case 3 weighted {:?}, unweighted {:?}", w.levels, u.levels);
    assert!((w.levels[0] - u.levels[0]).abs() > 1e-3, "{w:?} {u:?}");
}

#[test]
fn case_1_is_exact_and_its_drive_is_diagonal() {
    // s = (1.0, 0.5) meets (1.08, 0.75, 0.82) exactly: X = diag(1.0, 0.5).
    let h = note_frf();
    let r = targets(CASE1);
    for c in [0.0, 0.01] {
        let (t, lv) = nearly_independent_reference_with(
            &h,
            &r,
            ONE,
            &opts(true, LevelSolve::NonNegative, c),
            1e-10,
        )
        .expect("reference");
        assert_eq!(lv.exact, vec![1.0], "c {c}");
        assert!(
            (lv.levels[0] - 1.0).abs() < 1e-12 && (lv.levels[1] - 0.5).abs() < 1e-12,
            "c {c}: {:?}",
            lv.levels
        );
        // The reference on an exact line is the response of those levels.
        let y = congruence(&h, &{
            let mut g = HermitianStack::zeros(1, 2);
            g.set(0, 0, 0, C64::new(1.0, 0.0));
            g.set(0, 1, 1, C64::new(0.5, 0.0));
            g
        });
        assert!(line_rel(&t, &y, 0) < 1e-12, "c {c}");
    }
    // The law starts from the diagonal drive, even with the default constant.
    let setup = setup_for(CASE1);
    let mut law = NearlyIndependentLaw::with_params(&setup, &[]).expect("law");
    let out = seed_on(&mut law, &setup, &h);
    let d = &out.drive;
    println!(
        "case 1 seed drive {:?}",
        [d.get(0, 0, 0), d.get(0, 0, 1), d.get(0, 1, 1)]
    );
    assert!((d.get(0, 0, 0).re - 1.0).abs() < 1e-12);
    assert!((d.get(0, 1, 1).re - 0.5).abs() < 1e-12);
    assert!(d.get(0, 0, 1).norm() < 1e-12);
    assert_eq!(per_line(&out, "exact"), vec![1.0]);
    // No level set on an exact line, and the drive is diagonal exactly.
    assert_eq!(per_line(&out, "level_set_factor"), vec![1.0]);
    assert_eq!(d.get(0, 0, 1), C64::new(0.0, 0.0));
    // With the test off and the default constant, the regularized levels are
    // used: not (1.0, 0.5).
    let (_, lv) = nearly_independent_reference_with(
        &h,
        &r,
        ONE,
        &opts(true, LevelSolve::NonNegative, 0.01),
        0.0,
    )
    .expect("reference");
    assert_eq!(lv.exact, vec![0.0]);
    assert!((lv.levels[0] - 1.0).abs() > 1e-4, "{:?}", lv.levels);
}

#[test]
fn case_2_follows_the_note_through_the_level_set() {
    let h = note_frf();
    let r = targets(CASE2);
    let (t, lv) = nearly_independent_reference_with(
        &h,
        &r,
        ONE,
        &opts(true, LevelSolve::NonNegative, 0.0),
        1e-10,
    )
    .expect("reference");
    // Not exact: three targets, two drives.
    assert_eq!(lv.exact, vec![0.0]);
    // Trial response S0 = H diag(s) H^T.
    let s = [lv.levels[0], lv.levels[1]];
    let hv = [[1.0, 0.4], [0.5, 1.0], [0.8, -0.6]];
    let s0 = |m: usize, n: usize| hv[m][0] * s[0] * hv[n][0] + hv[m][1] * s[1] * hv[n][1];
    let want_s0 = [
        [1.065, 0.787, 0.559],
        [0.787, 1.030, -0.103],
        [0.559, -0.103, 0.887],
    ];
    for m in 0..3 {
        for n in 0..3 {
            printed(&format!("S0 {m}{n}"), s0(m, n), want_s0[m][n], 3);
        }
    }
    // Scale factors c_m = sqrt(t_m / S0_mm) = (0.969, 0.985, 1.062).
    let want_c = [0.969, 0.985, 1.062];
    for m in 0..3 {
        printed(&format!("c {m}"), (1.0 / s0(m, m)).sqrt(), want_c[m], 3);
    }
    // The rescaled response, S_hat_mn = c_m S0_mn c_n, to rounding.
    for m in 0..3 {
        for n in 0..3 {
            let v = t.get(0, m, n);
            let want = s0(m, n) / (s0(m, m) * s0(n, n)).sqrt();
            assert!(v.im.abs() < 1e-15);
            assert!(
                (v.re - want).abs() < 1e-12,
                "S_hat {m}{n}: {} against {want}",
                v.re
            );
        }
    }
    // Against the note's printed S_hat. Its 0.752 for entry (1, 2) is not
    // what its own figures give: 0.787 x 0.969 x 0.985 = 0.7512, and the
    // full-precision value is 0.75148, which rounds to 0.751. That entry is
    // held to 0.001; every other entry to half a unit in the third decimal.
    let want_t = [
        [1.0, 0.752, 0.575],
        [0.752, 1.0, -0.107],
        [0.575, -0.107, 1.0],
    ];
    for m in 0..3 {
        for n in 0..3 {
            let v = t.get(0, m, n).re;
            if (m, n) == (0, 1) || (m, n) == (1, 0) {
                assert!((v - 0.752).abs() <= 1e-3, "S_hat {m}{n}: {v}");
                println!("S_hat {m}{n} = {v:.5} (the note prints 0.752)");
            } else {
                printed(&format!("S_hat {m}{n}"), v, want_t[m][n], 3);
            }
        }
    }
    // The drive before the level set, X_raw = H+ S_hat H+^T, the law's seed
    // with the level set off (both singular values kept, so the loop's
    // truncation does not act).
    let setup = setup_for(CASE2);
    let mut p = unregularized();
    p.push(("level_set", ParamVal::Toggle(false)));
    let mut law = NearlyIndependentLaw::with_params(&setup, &p).expect("law");
    let out = seed_on(&mut law, &setup, &h);
    let x = &out.drive;
    printed("X 11", x.get(0, 0, 0).re, 0.9581, 4);
    printed("X 12", x.get(0, 0, 1).re, -0.0564, 4);
    printed("X 22", x.get(0, 1, 1).re, 0.8124, 4);
    // Its predicted responses (1.0429, 0.9954, 0.9598).
    let y = congruence(&h, x);
    for (m, w) in [1.0429, 0.9954, 0.9598].into_iter().enumerate() {
        printed(&format!("predicted {m}"), y.get(0, m, m).re, w, 4);
    }
    assert_eq!(per_line(&out, "exact"), vec![0.0]);
    assert_eq!(per_line(&out, "level_set_factor"), vec![1.0]);
    let raw = out.drive.clone();
    // The level set (the default): k = 3 / 2.9981 = 1.0006, the drive
    // (0.9587, -0.0565, 0.8129) and its predicted responses (1.0435, 0.9960,
    // 0.9604).
    let mut law = NearlyIndependentLaw::with_params(&setup, &unregularized()).expect("law");
    let out = seed_on(&mut law, &setup, &h);
    let k = per_line(&out, "level_set_factor")[0];
    println!("case 2 level-set factor {k}");
    printed("k", k, 1.0006, 4);
    let x = &out.drive;
    printed("kX 11", x.get(0, 0, 0).re, 0.9587, 4);
    printed("kX 12", x.get(0, 0, 1).re, -0.0565, 4);
    printed("kX 22", x.get(0, 1, 1).re, 0.8129, 4);
    let y = congruence(&h, x);
    for (m, w) in [1.0435, 0.9960, 0.9604].into_iter().enumerate() {
        printed(&format!("level-set predicted {m}"), y.get(0, m, m).re, w, 4);
    }
    let total: f64 = (0..3).map(|m| y.get(0, m, m).re).sum();
    assert!((total - 3.0).abs() < 1e-12, "{total}");
    for (a, b) in x.line(0).iter().zip(raw.line(0)) {
        assert!((a - b * k).norm() <= 1e-15, "{a} against {k} x {b}");
    }
}

#[test]
fn case_3_starts_from_the_notes_drive() {
    // From the non-negative levels (0.5405, 0): S0 and S_hat are rank one and
    // X_raw = [[1.1788, -0.2427], [-0.2427, 0.0500]].
    let h = note_frf();
    let setup = setup_for(CASE3);
    let mut p = unregularized();
    p.push(("level_set", ParamVal::Toggle(false)));
    let mut law = NearlyIndependentLaw::with_params(&setup, &p).expect("law");
    let out = seed_on(&mut law, &setup, &h);
    let x = &out.drive;
    printed("X 11", x.get(0, 0, 0).re, 1.1788, 4);
    printed("X 12", x.get(0, 0, 1).re, -0.2427, 4);
    printed("X 22", x.get(0, 1, 1).re, 0.0500, 4);
    assert_eq!(per_line(&out, "held_at_zero"), vec![1.0]);
    assert_eq!(per_line(&out, "exact"), vec![0.0]);
}

// -------------------------------------------------------------------------
// 2. The non-negative solve
// -------------------------------------------------------------------------

/// A random squared-magnitude system with the Tikhonov rows, and a right
/// side, for `nc` controls and `nd` drives.
fn random_system(rng: &mut Rng, nc: usize, nd: usize, c_rel: f64) -> (Vec<f64>, Vec<f64>, usize) {
    let mut a = vec![0.0; nc * nd];
    for v in a.iter_mut() {
        *v = rng.cnormal().norm_sqr();
    }
    let normal_max = (0..nd)
        .map(|d| (0..nc).map(|k| a[k * nd + d].powi(2)).sum::<f64>())
        .fold(0.0f64, f64::max);
    let rc = (c_rel * normal_max).sqrt();
    let rows = nc + nd;
    let mut m = vec![0.0; rows * nd];
    m[..nc * nd].copy_from_slice(&a);
    for d in 0..nd {
        m[(nc + d) * nd + d] = rc;
    }
    let mut y = vec![0.0; rows];
    for k in 0..nc {
        y[k] = 0.2 + rng.uniform();
    }
    (m, y, rows)
}

#[test]
fn the_non_negative_solve_meets_its_optimality_conditions_on_every_shape() {
    let mut rng = Rng::new(404);
    let mut worst_free: f64 = 0.0;
    let mut most = 0usize;
    let mut held_cases = 0;
    for (nc, nd) in [(4usize, 4usize), (6, 4), (3, 5), (12, 6), (2, 6)] {
        for case in 0..200 {
            let c_rel = if case % 4 == 0 { 0.0 } else { 0.01 };
            let (m, y, rows) = random_system(&mut rng, nc, nd, c_rel);
            let Some(sol) = nonnegative_least_squares(&m, &y, rows, nd) else {
                panic!("{nc} x {nd} case {case}: no solution within the bound");
            };
            most = most.max(sol.iterations);
            assert!(sol.iterations <= NNLS_ITERATIONS_PER_UNKNOWN * nd);
            let x = &sol.x;
            assert!(x.iter().all(|v| *v >= 0.0), "{x:?}");
            // The pull w = M^T (y - M x), minus half the gradient.
            let mut w = vec![0.0; nd];
            for r in 0..rows {
                let mut res = y[r];
                for c in 0..nd {
                    res -= m[r * nd + c] * x[c];
                }
                for c in 0..nd {
                    w[c] += m[r * nd + c] * res;
                }
            }
            let scale = m.iter().map(|v| v * v).sum::<f64>().sqrt()
                * y.iter().map(|v| v * v).sum::<f64>().sqrt();
            for c in 0..nd {
                if x[c] == 0.0 {
                    // A level at zero: no pull upward.
                    assert!(
                        w[c] <= 1e-12 * scale,
                        "{nc} x {nd} case {case}: held {c} pulls {}",
                        w[c]
                    );
                } else {
                    // A free level: zero gradient.
                    worst_free = worst_free.max(w[c].abs() / scale);
                    assert!(
                        w[c].abs() <= 1e-10 * scale,
                        "{nc} x {nd} case {case}: free {c} {}",
                        w[c]
                    );
                }
            }
            if x.contains(&0.0) {
                held_cases += 1;
            }
        }
    }
    println!(
        "non-negative solve, 1000 random systems: {held_cases} with a level held at zero, \
         free gradient at most {worst_free:.2e} relative, at most {most} subproblems"
    );
    assert!(held_cases > 100, "too few constrained cases: {held_cases}");
}

#[test]
fn where_nothing_is_negative_the_non_negative_solve_is_the_plain_solve() {
    // Directly: y = M x0 with x0 > 0 and full column rank.
    let mut rng = Rng::new(405);
    for (nc, nd) in [(4usize, 4usize), (6, 4), (8, 3)] {
        let (m, _, rows) = random_system(&mut rng, nc, nd, 0.0);
        let x0: Vec<f64> = (0..nd).map(|_| 0.1 + rng.uniform()).collect();
        let y: Vec<f64> = (0..rows)
            .map(|r| (0..nd).map(|c| m[r * nd + c] * x0[c]).sum())
            .collect();
        let sol = nonnegative_least_squares(&m, &y, rows, nd).expect("solution");
        for c in 0..nd {
            assert!(
                (sol.x[c] - x0[c]).abs() <= 1e-10 * x0[c],
                "{:?} {x0:?}",
                sol.x
            );
        }
    }
    // Through the laws' solve: a reachable reference has no negative level,
    // and the two solves give the same levels, bit for bit.
    for (nc, nd, seed) in [(4usize, 4usize, 60u64), (6, 4, 61), (3, 5, 62)] {
        let structure = well_conditioned(seed, nc, nd, 64);
        let mut g = HermitianStack::zeros(64, nd);
        let mut rng = Rng::new(seed);
        for line in 8..56 {
            for d in 0..nd {
                g.set(line, d, d, C64::new(0.2 + rng.uniform(), 0.0));
            }
        }
        let r = congruence(&structure, &g);
        for weighted in [true, false] {
            let a = independent_levels_with(
                &structure,
                &r,
                (8, 56),
                &opts(weighted, LevelSolve::NonNegative, 0.01),
            )
            .expect("levels");
            let b = independent_levels_with(
                &structure,
                &r,
                (8, 56),
                &opts(weighted, LevelSolve::SetToZero, 0.01),
            )
            .expect("levels");
            assert!(
                a.negative.iter().all(|n| *n == 0.0),
                "{nc} x {nd}: {:?}",
                a.negative
            );
            assert_eq!(a.levels, b.levels, "{nc} x {nd} weighted {weighted}");
        }
    }
}

// -------------------------------------------------------------------------
// 3. The feedback step
// -------------------------------------------------------------------------

/// One step of `independent` on the B1 hand-worked structure.
fn hand_step(
    reference: [f64; 3],
    last: [f64; 2],
    x: [f64; 3],
    params: &[(&'static str, ParamVal)],
) -> (Vec<f64>, MimoLoopOutputs) {
    let (pi, i) = (std::f64::consts::PI, C64::new(0.0, 1.0));
    let h = MatrixStack::from_vec(
        1,
        3,
        2,
        vec![
            C64::new(1.0, 0.0),
            i * 0.5,
            C64::from_polar(0.5, pi / 3.0),
            C64::new(-1.0, 0.0),
            C64::from_polar(1.0, pi / 4.0),
            -i,
        ],
    )
    .expect("dims");
    let setup = setup_for(reference);
    let mut law = IndependentDrivesLaw::with_params(&setup, params).expect("law");
    let mut response = HermitianStack::zeros(1, 3);
    for k in 0..3 {
        response.set(0, k, k, C64::new(x[k], 0.0));
    }
    let mut played = HermitianStack::zeros(1, 2);
    played.set(0, 0, 0, C64::new(last[0], 0.0));
    played.set(0, 1, 1, C64::new(last[1], 0.0));
    let csd = MatrixStack::zeros(1, 3, 2);
    let coherence = [1.0; 3];
    let inputs = |l: u64| MimoLoopInputs {
        reference: &setup.reference,
        tolerances: &setup.tolerances,
        response: &response,
        response_dof: 128.0,
        frf: &h,
        multiple_coherence: &coherence,
        df_hz: 1.0,
        band: ONE,
        loop_index: l,
        last_drive: Some(&played),
        response_drive_csd: Some(&csd),
        sysid_response: None,
        sysid_drive: None,
        sysid_noise: None,
    };
    law.seed(&inputs(0)).expect("seed");
    let out = law.iterate(&inputs(1)).expect("step");
    (out.drive.diagonal(0), out)
}

fn choice(v: &str) -> ParamVal {
    ParamVal::Choice(v.into())
}

#[test]
fn where_no_level_is_held_at_zero_the_non_negative_step_is_the_published_step() {
    for weighting in ["none", "target"] {
        let nn = [
            ("solve", choice("nonnegative")),
            ("solve_weighting", choice(weighting)),
        ];
        let pubd = [
            ("solve", choice("set_to_zero")),
            ("solve_weighting", choice(weighting)),
        ];
        let (a, out) = hand_step([1.5, 2.5, 3.5], [1.0, 2.0], [1.2, 2.0, 2.5], &nn);
        let (b, _) = hand_step([1.5, 2.5, 3.5], [1.0, 2.0], [1.2, 2.0, 2.5], &pubd);
        println!("{weighting}: non-negative step {a:?}, published {b:?}");
        assert_eq!(a, b, "{weighting}");
        assert_eq!(per_line(&out, "held_at_zero"), vec![0.0]);
    }
    // Unweighted, that is the B1 hand-worked step.
    let det = 2.089409765625;
    let (a, _) = hand_step(
        [1.5, 2.5, 3.5],
        [1.0, 2.0],
        [1.2, 2.0, 2.5],
        &[("solve_weighting", choice("none"))],
    );
    assert!((a[0] - (1.0 + 0.605953125 / det)).abs() < 1e-12, "{a:?}");
    assert!((a[1] - (2.0 + 1.143421875 / det)).abs() < 1e-12, "{a:?}");
}

#[test]
fn where_the_step_goes_negative_the_non_negative_step_holds_a_level_at_zero() {
    // The B1 case that drives level 2 negative, with the fall limit wide.
    let wide = ("limit_down_percent", ParamVal::Number(99.0));
    let none = ("solve_weighting", choice("none"));
    let (a, out) = hand_step(
        [0.1, 0.1, 0.1],
        [1.0, 0.5],
        [1.2, 2.0, 2.5],
        &[wide.clone(), none.clone()],
    );
    let (b, _) = hand_step(
        [0.1, 0.1, 0.1],
        [1.0, 0.5],
        [1.2, 2.0, 2.5],
        &[wide, none, ("solve", choice("set_to_zero"))],
    );
    println!("non-negative step {a:?}, set to zero {b:?}");
    assert_eq!(per_line(&out, "negative_levels"), vec![1.0]);
    // The plain step makes level 2 negative; the set-to-zero rule keeps
    // level 1 at 0.32. Over levels of zero or more the minimizer of the same
    // objective holds both at zero (the response is far above the target on
    // all three controls), and the fall limit then floors each at 1 percent of
    // its last level.
    assert_eq!(per_line(&out, "held_at_zero"), vec![2.0]);
    assert!((a[0] - 0.01).abs() < 1e-12, "{a:?}");
    assert!((a[1] - 0.005).abs() < 1e-12, "{a:?}");
    assert!((b[0] - 0.32137683171215525).abs() < 1e-12, "{b:?}");
}

// -------------------------------------------------------------------------
// 5. Each refinement off changes a result; defaults are the refinements
// -------------------------------------------------------------------------

#[test]
fn the_defaults_are_the_refinements() {
    for spec in [IndependentDrivesLaw::spec(), NearlyIndependentLaw::spec()] {
        let get = |k: &str| {
            spec.params
                .iter()
                .find(|p| p.key == k)
                .map(|p| p.kind.clone())
        };
        assert!(
            format!("{:?}", get("solve").expect("solve")).contains("default: \"nonnegative\""),
            "{}",
            spec.name
        );
        // The weighting is off by default in `independent` (its Choice 21).
        let weighting = if spec.name == "independent drives" {
            "none"
        } else {
            "target"
        };
        assert!(
            format!("{:?}", get("solve_weighting").expect("weighting"))
                .contains(&format!("default: \"{weighting}\"")),
            "{}",
            spec.name
        );
        assert_eq!(spec.author, "Hertzforge; refinements by N. Hunter");
    }
    let near = NearlyIndependentLaw::spec();
    let tol = near
        .params
        .iter()
        .find(|p| p.key == "exact_tolerance")
        .expect("tolerance");
    assert!(
        format!("{:?}", tol.kind).contains("default: 1e-10"),
        "{:?}",
        tol.kind
    );
}

#[test]
fn each_refinement_turned_off_by_its_parameter_changes_the_seed() {
    let h = note_frf();
    let diag = |out: &MimoLoopOutputs| out.drive.diagonal(0);
    // Weighting and the non-negative solve, on case 3, both laws.
    // `independent` with both refinements on: the weighting is off by default.
    let s3 = setup_for(CASE3);
    let both = || {
        let mut p = unregularized();
        p.push(("solve_weighting", choice("target")));
        p
    };
    let base = IndependentDrivesLaw::with_params(&s3, &both())
        .map(|mut l| seed_on(&mut l, &s3, &h))
        .expect("law");
    for (key, off) in [("solve_weighting", "none"), ("solve", "set_to_zero")] {
        let mut p: Vec<(&'static str, ParamVal)> =
            both().into_iter().filter(|(k, _)| *k != key).collect();
        p.push((key, choice(off)));
        let mut law = IndependentDrivesLaw::with_params(&s3, &p).expect("law");
        let other = seed_on(&mut law, &s3, &h);
        println!(
            "independent case 3: {key} {off} {:?}, both refinements {:?}",
            diag(&other),
            diag(&base)
        );
        assert!((diag(&other)[0] - diag(&base)[0]).abs() > 0.1, "{key}");
    }
    // For nearly_independent case 3 cannot show either: with one level left
    // the trial response is rank one and the rescale removes its level. A
    // random 3 x 3 structure where many lines need a negative level does.
    let s = Synthetic::random(80, 3, 3, 64);
    let seed_drive = |p: &[(&'static str, ParamVal)]| {
        let mut law = NearlyIndependentLaw::with_params(&s.setup, p).expect("law");
        closed_loop(&mut law, &s, 0).last().clone()
    };
    let base = seed_drive(&[]);
    for (key, off) in [("solve_weighting", "none"), ("solve", "set_to_zero")] {
        let other = seed_drive(&[(key, choice(off))]);
        let worst = (s.setup.band.0..s.setup.band.1)
            .map(|l| line_rel(&other, &base, l))
            .fold(0.0f64, f64::max);
        println!("nearly random 3 x 3: {key} {off} moves the seed drive by {worst:.3} at most");
        assert!(worst > 1e-3, "nearly {key}");
    }
    // The exactness test, on case 1 at the default constant.
    let s1 = setup_for(CASE1);
    let mut on = NearlyIndependentLaw::with_params(&s1, &[]).expect("law");
    let mut off =
        NearlyIndependentLaw::with_params(&s1, &[("exact_tolerance", ParamVal::Number(0.0))])
            .expect("law");
    let a = seed_on(&mut on, &s1, &h);
    let b = seed_on(&mut off, &s1, &h);
    println!(
        "nearly case 1: exactness on {:?}, off {:?}",
        diag(&a),
        diag(&b)
    );
    assert!(
        b.drive.get(0, 0, 1).norm() > 1e-4 || (diag(&b)[0] - 1.0).abs() > 1e-4,
        "{:?}",
        b.drive
    );
    assert_eq!(per_line(&b, "exact"), vec![0.0]);
}

#[test]
fn at_the_defaults_the_laws_hold_a_negative_level_at_zero_on_a_structure_that_needs_one() {
    // A 3 x 3 structure where a third of the drive would have to be negative.
    let s = Synthetic::random(80, 3, 3, 64);
    let mut ind = IndependentDrivesLaw::with_params(&s.setup, &[]).expect("law");
    let run = closed_loop(&mut ind, &s, 3);
    let held: f64 = run.per_line(0, "held_at_zero").iter().sum();
    let fallbacks = run.scalar(0, "solve_fallbacks");
    let mut near = NearlyIndependentLaw::with_params(&s.setup, &[]).expect("law");
    let nrun = closed_loop(&mut near, &s, 3);
    let nheld: f64 = nrun.per_line(0, "held_at_zero").iter().sum();
    println!("defaults on a random 3 x 3: independent held {held} levels at the seed, nearly {nheld}, fallbacks {fallbacks}");
    for d in run.drives().chain(nrun.drives()) {
        assert!(worst_eigen_ratio(d) >= -1e-9);
    }
    assert_eq!(fallbacks, 0.0);
}

/// Run `native` and `file` side by side as `parity` does, and give the worst
/// drive difference and the worst difference of each diagnostic by key.
fn parity_by_key(
    native: &mut dyn MimoControlLaw,
    file: &mut dyn MimoControlLaw,
    s: &Synthetic,
    loops: u64,
) -> (f64, Vec<(String, f64)>) {
    let needs = native.needs().to_vec();
    let mut response = s.sysid_response.clone();
    let mut last = s.sysid_drive.clone();
    let mut csd = cross(&s.plant, &last);
    let mut drive: f64 = 0.0;
    let mut keys: Vec<(String, f64)> = Vec::new();
    for l in 0..=loops {
        let inputs = s.inputs(&response, &last, &csd, l, &needs);
        let (a, b) = if l == 0 {
            (native.seed(&inputs), file.seed(&inputs))
        } else {
            (native.iterate(&inputs), file.iterate(&inputs))
        };
        let (a, b) = (a.expect("native"), b.expect("law file"));
        for line in 0..s.setup.lines {
            drive = drive.max(line_rel(&a.drive, &b.drive, line));
        }
        assert_eq!(a.diagnostics.len(), b.diagnostics.len());
        for da in &a.diagnostics {
            let db = b
                .diagnostics
                .iter()
                .find(|d| d.key == da.key)
                .expect("same keys");
            let one = MimoLoopOutputs {
                drive: a.drive.clone(),
                diagnostics: vec![da.clone()],
            };
            let two = MimoLoopOutputs {
                drive: b.drive.clone(),
                diagnostics: vec![db.clone()],
            };
            let r = diag_rel(&one, &two);
            match keys.iter_mut().find(|(k, _)| *k == da.key) {
                Some((_, w)) => *w = w.max(r),
                None => keys.push((da.key.to_string(), r)),
            }
        }
        last = a.drive;
        response = s.respond(&last, l + 1);
        csd = cross(&s.plant, &last);
    }
    (drive, keys)
}

#[test]
fn the_law_files_take_the_same_non_negative_steps_as_the_native_laws() {
    // At the defaults, on the 3 x 3 structure where many lines need a
    // negative level (so the non-negative solve runs), 5 percent off in the
    // estimate so the loop has work to do.
    use hertzforge_law_host::{Param, ParamValue};
    let mut s = Synthetic::random(80, 3, 3, 64);
    s.frf = perturbed(&s.plant, 80, 0.05);
    let published = [("solve", "set_to_zero"), ("solve_weighting", "none")];
    let host: Vec<Param> = published
        .iter()
        .map(|(k, v)| Param::new(*k, ParamValue::Choice((*v).into())))
        .collect();
    let native: Vec<(&'static str, ParamVal)> =
        published.iter().map(|(k, v)| (*k, choice(v))).collect();
    let mut host_near = host.clone();
    host_near.push(Param::new("exact_tolerance", ParamValue::Number(0.0)));
    host_near.push(Param::new("level_set", ParamValue::Toggle(false)));
    let mut native_near = native.clone();
    native_near.push(("exact_tolerance", ParamVal::Number(0.0)));
    native_near.push(("level_set", ParamVal::Toggle(false)));
    for (form, ind_p, ind_h, near_p, near_h) in [
        ("defaults", vec![], vec![], vec![], vec![]),
        (
            "published",
            native.clone(),
            host.clone(),
            native_near,
            host_near,
        ),
    ] {
        let mut ind = IndependentDrivesLaw::with_params(&s.setup, &ind_p).expect("law");
        let mut ind_file = wasm_law("hertzforge_law_mimo_independent", &s.setup, &ind_h);
        let (d, keys) = parity_by_key(&mut ind, &mut ind_file, &s, 10);
        println!("independent, {form}: drive {d:.2e}, {keys:?}");
        assert!(d <= 1e-12, "{form}");
        for (k, w) in &keys {
            assert!(*w <= 1e-12, "independent {form} {k}: {w:e}");
        }
        let mut near = NearlyIndependentLaw::with_params(&s.setup, &near_p).expect("law");
        let mut near_file = wasm_law("hertzforge_law_mimo_nearly_independent", &s.setup, &near_h);
        let (d, keys) = parity_by_key(&mut near, &mut near_file, &s, 10);
        println!("nearly_independent, {form}: drive {d:.2e}, {keys:?}");
        assert!(d <= 1e-12, "{form}");
        for (k, w) in &keys {
            // `correction_db` is the shared loop's largest drive change from
            // one loop to the next, in decibels (`largest_change_db`, with a
            // floor at 1e-6 of the line's largest). A drive PSD just above
            // that floor carries the drive's last-place differences (about
            // 1e-14 from the seed on, in both forms) as a relative
            // difference some hundred times larger, so the change in
            // decibels differs by some 1e-11. Every diagnostic this law
            // computes itself is held to 1e-12.
            let bar = if k == "correction_db" { 1e-10 } else { 1e-12 };
            assert!(*w <= bar, "nearly_independent {form} {k}: {w:e}");
        }
    }
    let mut ind = IndependentDrivesLaw::with_params(&s.setup, &[]).expect("law");
    let run = closed_loop(&mut ind, &s, 10);
    let held: f64 = (0..=10)
        .map(|l| run.per_line(l, "held_at_zero").iter().sum::<f64>())
        .sum();
    println!("independent, defaults: levels held at zero over the run {held}");
    assert!(held > 0.0);
}

#[test]
fn a_non_finite_input_to_the_non_negative_solve_is_refused() {
    let m = [1.0, 0.5, 0.2, 1.0];
    let y = [1.0, -1.0];
    assert!(nonnegative_least_squares(&m, &y, 2, 2).is_some());
    for (m, y) in [
        ([f64::NAN, 0.5, 0.2, 1.0], y),
        ([1.0, f64::INFINITY, 0.2, 1.0], y),
        (m, [1.0, f64::NEG_INFINITY]),
        (m, [f64::NAN, -1.0]),
        // Finite, but the norms overflow: every pull would fall below an
        // infinite tolerance and zero would be returned as the minimizer.
        ([1e200, 5e199, 2e199, 1e200], y),
    ] {
        assert_eq!(nonnegative_least_squares(&m, &y, 2, 2), None, "{m:?} {y:?}");
    }
}

#[test]
fn a_step_that_is_not_finite_is_refused() {
    // Found by a search over matrices whose norm is just inside the range of
    // a double: the norms are finite, but the Householder products of a
    // subproblem overflow, its solution is not a number, and the step toward
    // it is not finite. Without the guard this returns levels taken along
    // that step, `[0, 5e-159]`, as if they were the minimizer.
    let m = [
        6.735287263891597e153,
        4.106285861639476e153,
        -8.417925246499588e153,
        -5.385200721496558e153,
    ];
    let y = [-1.3386371416063563e-5, -5.2695632398583054e-5];
    assert_eq!(nonnegative_least_squares(&m, &y, 2, 2), None);
}
