//! The independent-drives and nearly-independent-drives laws on synthetic
//! structures with the closed loop simulated here (see `laws.rs` for the
//! harness). Every frequency response is complex, so a conjugation error is
//! seen.
//!
//! - On a decoupled structure independent drives meet the reference
//!   autospectra to rounding, and every off-diagonal drive term is zero.
//! - Where independent drives are exactly feasible the solve returns the levels
//!   that produced the reference, and the nearly independent reference has
//!   that response's coherence and phase.
//! - Where a solved level is negative it is set to zero and counted, and the
//!   rescaled reference keeps the specified autospectra and the trial
//!   response's coherence and phase.
//! - A control no drive reaches, a drive that reaches no control, more and
//!   fewer controls than drives, a rank-deficient system, a line with no
//!   reference and the lines outside the band each give a defined result.
//! - The feedback step is the hand-worked one of a two-drive, three-control
//!   line, with and without the change limit.
//! - Closed loop: the nearly independent law converges as `min_drive` does in
//!   its own tests; the independent law reaches the reference through a known
//!   gain error where it is feasible, and settles, with the count saying why,
//!   where it is not.

// Index loops over lines and channels mirror the matrix algebra they check.
#![allow(clippy::needless_range_loop)]

mod common;

use common::*;
use hertzforge_mimo_laws::{
    independent_levels, nearly_independent_reference, BuiltinMimoLaw, IndependentDrivesLaw,
    NearlyIndependentLaw, ParamVal,
};
use hertzforge_sdm::law::{DiagnosticData, MimoControlLaw, MimoLoopInputs, MimoSetup, Tolerances};
use hertzforge_sdm::linalg::{pinv, projection_achievable, Regularization};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

const LINES: usize = 64;
const BAND: (usize, usize) = (8, 56);

/// The published form's parameters that `L` declares, then `extra`.
fn published_with<L: BuiltinMimoLaw>(
    extra: &[(&'static str, ParamVal)],
) -> Vec<(&'static str, ParamVal)> {
    let keys: Vec<&str> = L::spec().params.iter().map(|p| p.key).collect();
    let mut out: Vec<(&'static str, ParamVal)> = published()
        .into_iter()
        .filter(|(k, _)| keys.contains(k) && !extra.iter().any(|(e, _)| e == k))
        .collect();
    out.extend(extra.iter().cloned());
    out
}

/// The published independent-drives law, unregularized.
fn exact() -> Vec<(&'static str, ParamVal)> {
    published_with::<IndependentDrivesLaw>(&[("regularization_constant", ParamVal::Number(0.0))])
}

/// The published independent-drives law.
fn pub_ind() -> Vec<(&'static str, ParamVal)> {
    published_with::<IndependentDrivesLaw>(&[])
}

/// The published nearly-independent law.
fn pub_near() -> Vec<(&'static str, ParamVal)> {
    published_with::<NearlyIndependentLaw>(&[])
}

/// The diagonal drive of `levels` (`lines * nd`).
fn diag_drive(levels: &[f64], lines: usize, nd: usize) -> HermitianStack {
    let mut g = HermitianStack::zeros(lines, nd);
    for line in 0..lines {
        for d in 0..nd {
            g.set(line, d, d, C64::new(levels[line * nd + d], 0.0));
        }
    }
    g
}

/// Positive levels on the band, zero outside, varying by drive and line.
fn positive_levels(seed: u64, nd: usize) -> Vec<f64> {
    let mut rng = Rng::new(seed);
    let mut g = vec![0.0; LINES * nd];
    for line in BAND.0..BAND.1 {
        for d in 0..nd {
            g[line * nd + d] = 1e-2 * (0.2 + rng.uniform());
        }
    }
    g
}

/// A run on `plant`, its reference the response of the independent drive
/// `levels`, the estimate exact.
fn feasible(seed: u64, plant: MatrixStack, levels: &[f64], eps: f64) -> Synthetic {
    let (lines, _, nd) = plant.dims();
    let reference = congruence(&plant, &diag_drive(levels, lines, nd));
    run_on(seed, plant.clone(), plant, reference, BAND, eps)
}

/// Coherence and phase of pair `(j, k)` on `line`.
fn coherence(g: &HermitianStack, line: usize, j: usize, k: usize) -> (f64, f64) {
    let c = g.get(line, j, k);
    let d = g.get(line, j, j).re * g.get(line, k, k).re;
    (c.norm_sqr() / d, c.arg())
}

/// The worst coherence and phase (degrees) difference between `a` and `b` over
/// every pair on the band's lines, pairs with coherence above `min` only.
fn coherence_phase_gap(a: &HermitianStack, b: &HermitianStack, min: f64) -> (f64, f64) {
    let n = a.n();
    let (mut coh, mut phase) = (0.0f64, 0.0f64);
    for line in BAND.0..BAND.1 {
        for j in 0..n {
            for k in (j + 1)..n {
                let (ca, pa) = coherence(a, line, j, k);
                let (cb, pb) = coherence(b, line, j, k);
                coh = coh.max((ca - cb).abs());
                if cb > min {
                    let dp = (C64::from_polar(1.0, pa) * C64::from_polar(1.0, -pb)).arg();
                    phase = phase.max(dp.abs().to_degrees());
                }
            }
        }
    }
    (coh, phase)
}

/// The worst relative difference of the autospectra of `a` from `want`'s on
/// the band's lines.
fn autospectra_gap(a: &HermitianStack, want: &HermitianStack) -> f64 {
    let mut worst: f64 = 0.0;
    for line in BAND.0..BAND.1 {
        for c in 0..a.n() {
            let (x, y) = (a.get(line, c, c).re, want.get(line, c, c).re);
            if y > 0.0 {
                worst = worst.max((x - y).abs() / y);
            } else {
                worst = worst.max(x.abs());
            }
        }
    }
    worst
}

fn per_line(out: &hertzforge_sdm::law::MimoLoopOutputs, key: &str) -> Vec<f64> {
    match out
        .diagnostics
        .iter()
        .find(|d| d.key == key)
        .map(|d| &d.data)
    {
        Some(DiagnosticData::PerLine(v)) => v.clone(),
        other => panic!("{key}: {other:?}"),
    }
}

fn scalar(out: &hertzforge_sdm::law::MimoLoopOutputs, key: &str) -> f64 {
    match out
        .diagnostics
        .iter()
        .find(|d| d.key == key)
        .map(|d| &d.data)
    {
        Some(DiagnosticData::Scalar(v)) => *v,
        other => panic!("{key}: {other:?}"),
    }
}

fn assert_diagonal(name: &str, g: &HermitianStack) {
    for line in 0..g.lines() {
        for r in 0..g.n() {
            assert_eq!(g.get(line, r, r).im, 0.0, "{name}: line {line}");
            assert!(g.get(line, r, r).re >= 0.0, "{name}: line {line} drive {r}");
            for c in (r + 1)..g.n() {
                assert_eq!(g.get(line, r, c), C64::new(0.0, 0.0), "{name}: line {line}");
            }
        }
    }
}

fn assert_out_of_band_zero(name: &str, g: &HermitianStack) {
    for line in (0..BAND.0).chain(BAND.1..g.lines()) {
        assert!(
            g.line(line).iter().all(|v| *v == C64::new(0.0, 0.0)),
            "{name}: line {line} out of band"
        );
    }
}

// -------------------------------------------------------------------------
// 1. A decoupled structure
// -------------------------------------------------------------------------

/// A diagonal complex frequency response, `|H_kk|` between 1 and 2.
fn decoupled(seed: u64, n: usize) -> MatrixStack {
    let mut rng = Rng::new(seed);
    let mut h = MatrixStack::zeros(LINES, n, n);
    for line in 0..LINES {
        for k in 0..n {
            let mag = 1.0 + rng.uniform();
            h.set(
                line,
                k,
                k,
                C64::from_polar(mag, std::f64::consts::TAU * rng.uniform()),
            );
        }
    }
    h
}

#[test]
fn on_a_decoupled_structure_independent_drives_meet_the_autospectra_to_rounding() {
    let h = decoupled(1, 4);
    // A reference with cross terms: independent drives ignore them.
    let mut s = Synthetic::with_plant(1, h, BAND, 0.0);
    s.frf = s.plant.clone();
    // Unregularized, the seed alone meets them.
    let mut law = IndependentDrivesLaw::with_params(&s.setup, &exact()).expect("law");
    let run = closed_loop(&mut law, &s, 0);
    let y = congruence(&s.plant, run.last());
    let seed_gap = autospectra_gap(&y, &s.setup.reference);
    // With the published constant the seed is short by the regularization, and
    // the loop removes it.
    let mut law = IndependentDrivesLaw::with_params(&s.setup, &pub_ind()).expect("law");
    let run = closed_loop(&mut law, &s, 30);
    let first = autospectra_gap(
        &congruence(&s.plant, &run.outputs[0].drive),
        &s.setup.reference,
    );
    let last = autospectra_gap(&congruence(&s.plant, run.last()), &s.setup.reference);
    println!(
        "decoupled 4 x 4: seed unregularized {seed_gap:.2e}, regularized seed {first:.2e}, \
         after 30 loops {last:.2e}"
    );
    assert!(seed_gap < 1e-12, "{seed_gap}");
    assert!(first > 1e-4, "the regularization did not show: {first}");
    assert!(last < 1e-12, "{last}");
    for (l, d) in run.drives().enumerate() {
        assert_diagonal(&format!("loop {l}"), d);
        assert_out_of_band_zero(&format!("loop {l}"), d);
    }
}

// -------------------------------------------------------------------------
// 2. Exactly feasible on a coupled structure
// -------------------------------------------------------------------------

#[test]
fn where_independent_drives_are_feasible_the_solve_returns_their_levels() {
    for (nc, nd, seed) in [(4usize, 4usize, 20u64), (6, 4, 21)] {
        let plant = well_conditioned(seed, nc, nd, LINES);
        let g0 = positive_levels(seed, nd);
        let s = feasible(seed, plant, &g0, 0.0);
        let lv = independent_levels(&s.frf, &s.setup.reference, BAND, 0.0, 0.0).expect("levels");
        let mut worst: f64 = 0.0;
        for (a, b) in lv.levels.iter().zip(&g0) {
            worst = worst.max((a - b).abs() / b.max(1e-300));
        }
        // The law's seed is those levels.
        let mut law = IndependentDrivesLaw::with_params(&s.setup, &exact()).expect("law");
        let seed_drive = closed_loop(&mut law, &s, 0).outputs.remove(0).drive;
        let mut law_worst: f64 = 0.0;
        for line in 0..LINES {
            for d in 0..nd {
                let (a, b) = (seed_drive.get(line, d, d).re, g0[line * nd + d]);
                law_worst = law_worst.max((a - b).abs() / b.max(1e-300));
            }
        }
        println!(
            "feasible {nc} x {nd}: levels within {worst:.2e}, the seed within {law_worst:.2e}"
        );
        assert!(worst < 1e-9, "{nc} x {nd}: {worst}");
        assert!(law_worst < 1e-9, "{nc} x {nd}: {law_worst}");
        assert!(lv.negative.iter().all(|&n| n == 0.0));
    }
}

#[test]
fn where_feasible_the_nearly_independent_reference_has_that_responses_coherence_and_phase() {
    // Not with fewer controls than drives: there many sets of levels give the
    // same autospectra, each with its own coherence, and the solve returns the
    // smallest, not the one the reference was made from.
    for (nc, nd, seed) in [(4usize, 4usize, 22u64), (6, 4, 23)] {
        let plant = well_conditioned(seed, nc, nd, LINES);
        let g0 = positive_levels(seed, nd);
        let s = feasible(seed, plant, &g0, 0.0);
        // The response of those drives, formed here on its own.
        let truth = congruence(&s.plant, &diag_drive(&g0, LINES, nd));
        let (target, lv) =
            nearly_independent_reference(&s.frf, &s.setup.reference, BAND, 0.0).expect("target");
        let (coh, phase) = coherence_phase_gap(&target, &truth, 1e-6);
        let auto = autospectra_gap(&target, &s.setup.reference);
        println!(
            "nearly independent reference {nc} x {nd}: coherence within {coh:.2e}, phase \
             within {phase:.2e} degrees, autospectra within {auto:.2e}"
        );
        assert!(coh < 1e-9, "{coh}");
        assert!(phase < 1e-6, "{phase}");
        assert!(auto < 1e-12, "{auto}");
        if nc <= nd {
            assert!(lv.negative.iter().all(|&n| n == 0.0), "{:?}", lv.negative);
        }
        assert!(worst_eigen_ratio(&target) >= -1e-9);
    }
}

// -------------------------------------------------------------------------
// 3. A negative level
// -------------------------------------------------------------------------

/// A 3 x 3 coupled complex structure with a diagonal of magnitude 1 and
/// couplings of 0.4 to 0.6, and a reference whose autospectra are those of the
/// levels (1, 1, -0.3) times 1e-2 on every line: positive autospectra that only
/// a negative third level would produce.
fn needs_a_negative_level(seed: u64) -> Synthetic {
    let (n, mut rng) = (3, Rng::new(seed));
    let mut h = MatrixStack::zeros(LINES, n, n);
    for line in 0..LINES {
        for r in 0..n {
            for c in 0..n {
                let mag = if r == c {
                    1.0
                } else {
                    0.4 + 0.2 * rng.uniform()
                };
                h.set(
                    line,
                    r,
                    c,
                    C64::from_polar(mag, std::f64::consts::TAU * rng.uniform()),
                );
            }
        }
    }
    let g = [1e-2, 1e-2, -3e-3];
    let mut reference = HermitianStack::zeros(LINES, n);
    for line in BAND.0..BAND.1 {
        for r in 0..n {
            let y: f64 = (0..n).map(|d| h.get(line, r, d).norm_sqr() * g[d]).sum();
            assert!(y > 0.0);
            reference.set(line, r, r, C64::new(y, 0.0));
        }
    }
    run_on(seed, h.clone(), h, reference, BAND, 0.0)
}

#[test]
fn a_negative_level_is_set_to_zero_counted_and_the_rescale_keeps_the_autospectra() {
    let s = needs_a_negative_level(30);
    for c in [0.0, 0.01] {
        let lv = independent_levels(&s.frf, &s.setup.reference, BAND, c, 0.0).expect("levels");
        for line in 0..LINES {
            let want = if (BAND.0..BAND.1).contains(&line) {
                1.0
            } else {
                0.0
            };
            assert_eq!(lv.negative[line], want, "c {c}, line {line}");
            if want == 1.0 {
                assert_eq!(lv.levels[line * 3 + 2], 0.0, "c {c}, line {line}");
                assert!(lv.levels[line * 3] > 0.0 && lv.levels[line * 3 + 1] > 0.0);
            }
        }
        // The trial response of the clipped levels, formed here on its own.
        let trial = congruence(&s.frf, &diag_drive(&lv.levels, LINES, 3));
        let (target, _) =
            nearly_independent_reference(&s.frf, &s.setup.reference, BAND, c).expect("target");
        let auto = autospectra_gap(&target, &s.setup.reference);
        let (coh, phase) = coherence_phase_gap(&target, &trial, 1e-6);
        // The trial itself is too large on some control, as Hunter, Cross and
        // Nelson say: what the rescale corrects.
        let trial_gap = autospectra_gap(&trial, &s.setup.reference);
        println!(
            "negative level, c {c}: trial autospectra off by {trial_gap:.3}, rescaled within \
             {auto:.2e}, coherence within {coh:.2e}, phase within {phase:.2e} degrees"
        );
        assert!(trial_gap > 0.05, "{trial_gap}");
        assert!(auto < 1e-12, "{auto}");
        assert!(coh < 1e-9, "{coh}");
        assert!(phase < 1e-6, "{phase}");
        assert!(worst_eigen_ratio(&target) >= -1e-9);
        assert_out_of_band_zero("target", &target);
    }
    // The laws report the count: the seed of each.
    let mut ind = IndependentDrivesLaw::with_params(&s.setup, &pub_ind()).expect("law");
    let mut near = NearlyIndependentLaw::with_params(&s.setup, &pub_near()).expect("law");
    for out in [
        closed_loop(&mut ind, &s, 0).outputs.remove(0),
        closed_loop(&mut near, &s, 0).outputs.remove(0),
    ] {
        let n = per_line(&out, "negative_levels");
        let total: f64 = n.iter().sum();
        assert_eq!(total, (BAND.1 - BAND.0) as f64, "{n:?}");
    }
}

#[test]
fn the_negative_floor_sets_a_negative_level_to_its_fraction_of_the_lines_largest() {
    let s = needs_a_negative_level(31);
    let lv = independent_levels(&s.frf, &s.setup.reference, BAND, 0.0, 0.01).expect("levels");
    for line in BAND.0..BAND.1 {
        let g = &lv.levels[line * 3..line * 3 + 3];
        let top = g[0].max(g[1]);
        assert!(
            (g[2] - 0.01 * top).abs() <= 1e-15 * top,
            "line {line}: {g:?}"
        );
    }
}

// -------------------------------------------------------------------------
// 4. Edges: zero rows and columns, shapes, rank, no reference
// -------------------------------------------------------------------------

#[test]
fn a_control_no_drive_reaches_and_a_drive_that_reaches_nothing_are_defined() {
    // Control 1 has an all-zero row, drive 2 an all-zero column.
    let mut plant = well_conditioned(40, 4, 4, LINES);
    for line in 0..LINES {
        for d in 0..4 {
            plant.set(line, 1, d, C64::new(0.0, 0.0));
        }
        for c in 0..4 {
            plant.set(line, c, 2, C64::new(0.0, 0.0));
        }
    }
    let s = Synthetic::with_plant(40, plant.clone(), BAND, 0.01);
    let mut s = s;
    s.frf = plant;
    let lv = independent_levels(&s.frf, &s.setup.reference, BAND, 0.01, 0.0).expect("levels");
    for line in BAND.0..BAND.1 {
        // Zero to rounding: the singular vectors carry it at 1e-16 of the line.
        let top = lv.levels[line * 4..line * 4 + 4]
            .iter()
            .copied()
            .fold(0.0f64, f64::max);
        let v = lv.levels[line * 4 + 2];
        assert!(
            v.abs() <= 1e-12 * top,
            "line {line}: a drive that reaches nothing has {v}"
        );
        assert!(lv.levels.iter().all(|v| v.is_finite()));
    }
    let (target, _) =
        nearly_independent_reference(&s.frf, &s.setup.reference, BAND, 0.01).expect("target");
    for line in BAND.0..BAND.1 {
        // The control no drive reaches keeps its autospectrum, independent.
        let want = s.setup.reference.get(line, 1, 1).re;
        assert_eq!(target.get(line, 1, 1).re, want, "line {line}");
        for c in [0, 2, 3] {
            assert_eq!(
                target.get(line, 1, c),
                C64::new(0.0, 0.0),
                "line {line} pair 1 {c}"
            );
        }
    }
    assert!(worst_eigen_ratio(&target) >= -1e-9);
    for law in [
        Box::new(IndependentDrivesLaw::with_params(&s.setup, &[]).expect("law"))
            as Box<dyn MimoControlLaw>,
        Box::new(NearlyIndependentLaw::with_params(&s.setup, &[]).expect("law")),
    ] {
        let mut law = law;
        let run = closed_loop(law.as_mut(), &s, 10);
        for (l, d) in run.drives().enumerate() {
            assert!(d.is_finite(), "loop {l}");
            assert!(worst_eigen_ratio(d) >= -1e-9, "loop {l}");
            assert_out_of_band_zero(&format!("loop {l}"), d);
        }
    }
}

#[test]
fn more_and_fewer_controls_than_drives_and_a_rank_deficient_system_are_defined() {
    // 6 x 3, 3 x 6, and 4 x 4 with two equal columns of squared magnitudes
    // (drive 3 a copy of drive 0 up to phase), so the real system is rank 3.
    let mut rank_deficient = well_conditioned(52, 4, 4, LINES);
    for line in 0..LINES {
        for c in 0..4 {
            let v = rank_deficient.get(line, c, 0);
            rank_deficient.set(line, c, 3, v * C64::from_polar(1.0, 0.7 * c as f64));
        }
    }
    let cases = [
        ("6 x 3", well_conditioned(50, 6, 3, LINES)),
        ("3 x 6", well_conditioned(51, 3, 6, LINES)),
        ("4 x 4 rank deficient", rank_deficient),
    ];
    for (name, plant) in cases {
        let nd = plant.dims().2;
        let g0 = positive_levels(7, nd);
        let s = feasible(53, plant, &g0, 0.01);
        for c in [0.0, 0.01] {
            let lv = independent_levels(&s.frf, &s.setup.reference, BAND, c, 0.0).expect("levels");
            assert!(
                lv.levels.iter().all(|v| v.is_finite() && *v >= 0.0),
                "{name}"
            );
            if name.contains("rank") {
                // Two drives the real system cannot tell apart share the level.
                for line in BAND.0..BAND.1 {
                    let (a, b) = (lv.levels[line * 4], lv.levels[line * 4 + 3]);
                    assert!(
                        (a - b).abs() <= 1e-9 * a.max(b),
                        "{name} c {c} line {line}: {a} {b}"
                    );
                }
            }
            // The solved levels reproduce the reference autospectra whatever
            // the solution chosen, wherever none was negative.
            if c == 0.0 {
                let y = congruence(&s.plant, &diag_drive(&lv.levels, LINES, nd));
                let mut worst: f64 = 0.0;
                for line in BAND.0..BAND.1 {
                    if lv.negative[line] > 0.0 {
                        continue;
                    }
                    for k in 0..s.setup.controls {
                        let (a, b) = (y.get(line, k, k).re, s.setup.reference.get(line, k, k).re);
                        worst = worst.max((a - b).abs() / b);
                    }
                }
                println!("{name}, unregularized: autospectra within {worst:.2e}");
                assert!(worst < 1e-9, "{name}: {worst}");
            }
        }
        for law in [
            Box::new(IndependentDrivesLaw::with_params(&s.setup, &[]).expect("law"))
                as Box<dyn MimoControlLaw>,
            Box::new(NearlyIndependentLaw::with_params(&s.setup, &[]).expect("law")),
        ] {
            let mut law = law;
            let run = closed_loop(law.as_mut(), &s, 10);
            for (l, d) in run.drives().enumerate() {
                assert!(d.is_finite(), "{name} loop {l}");
                assert!(worst_eigen_ratio(d) >= -1e-9, "{name} loop {l}");
                assert_out_of_band_zero(&format!("{name} loop {l}"), d);
            }
        }
    }
}

#[test]
fn a_line_with_no_reference_gets_no_drive() {
    let mut s = Synthetic::random(60, 4, 3, LINES);
    let quiet = 20;
    for r in 0..4 {
        for c in r..4 {
            s.setup.reference.set(quiet, r, c, C64::new(0.0, 0.0));
        }
    }
    let lv = independent_levels(&s.frf, &s.setup.reference, BAND, 0.01, 0.0).expect("levels");
    assert!(lv.levels[quiet * 3..quiet * 3 + 3]
        .iter()
        .all(|&v| v == 0.0));
    assert_eq!(lv.negative[quiet], 0.0);
    let (target, _) =
        nearly_independent_reference(&s.frf, &s.setup.reference, BAND, 0.01).expect("target");
    assert!(target.line(quiet).iter().all(|v| *v == C64::new(0.0, 0.0)));
    let mut law = IndependentDrivesLaw::with_params(&s.setup, &[]).expect("law");
    let run = closed_loop(&mut law, &s, 0);
    assert!(run
        .last()
        .line(quiet)
        .iter()
        .all(|v| *v == C64::new(0.0, 0.0)));
}

// -------------------------------------------------------------------------
// 5. Positive semi-definite and zero outside the band, every structure
// -------------------------------------------------------------------------

#[test]
fn every_drive_is_hermitian_psd_and_zero_out_of_band_on_the_crates_structures() {
    let mut cases: Vec<(String, Synthetic)> = Vec::new();
    for (i, (nc, nd)) in [(3, 2), (6, 4), (12, 6), (4, 4), (4, 8)]
        .into_iter()
        .enumerate()
    {
        let seed = 100 + i as u64;
        let plant = well_conditioned(seed, nc, nd, LINES);
        let reference = reachable_reference(&plant, seed, BAND);
        let frf = perturbed(&plant, seed, 0.05);
        cases.push((
            format!("{nc} x {nd}"),
            run_on(seed, plant, frf, reference, BAND, 0.01),
        ));
    }
    let ill = plant_with_singular_values(7, 6, 4, LINES, &[1.0, 0.3, 0.1, 1e-5]);
    let gain = C64::from_polar(10f64.powf(1.5 / 20.0), 10f64.to_radians());
    let ill_ref = reachable_reference(&ill, 8, BAND);
    cases.push((
        "6 x 4 ill-conditioned".into(),
        run_on(8, ill.clone(), scaled(&ill, gain), ill_ref, BAND, 0.01),
    ));
    let unreach = plant_with_singular_values(41, 24, 6, LINES, &[1.0, 0.3, 0.1, 0.03, 0.01, 3e-3]);
    let mut s = Synthetic::with_plant(41, unreach.clone(), BAND, 0.01);
    s.frf = unreach;
    cases.push(("24 x 6 unreachable".into(), s));
    cases.push(("negative level 3 x 3".into(), needs_a_negative_level(32)));
    for (name, s) in &cases {
        for law in [
            Box::new(IndependentDrivesLaw::with_params(&s.setup, &[]).expect("law"))
                as Box<dyn MimoControlLaw>,
            Box::new(NearlyIndependentLaw::with_params(&s.setup, &[]).expect("law")),
        ] {
            let mut law = law;
            let run = closed_loop(law.as_mut(), s, 30);
            for (l, d) in run.drives().enumerate() {
                let what = format!("{name} loop {l}");
                assert!(d.is_finite(), "{what}");
                for line in 0..LINES {
                    for r in 0..d.n() {
                        assert_eq!(d.get(line, r, r).im, 0.0, "{what}");
                    }
                }
                assert!(worst_eigen_ratio(d) >= -1e-9, "{what}");
                assert_out_of_band_zero(&what, d);
            }
        }
    }
}

// -------------------------------------------------------------------------
// 6. The feedback step by hand, and the closed loop
// -------------------------------------------------------------------------

/// One line, three controls, two drives:
///
/// ```text
/// H = [ 1                 0.5 i     ]      A = |H|^2 = [ 1     0.25 ]
///     [ 0.5 e^{i pi/3}    -1        ]                  [ 0.25  1    ]
///     [ e^{i pi/4}        -i        ]                  [ 1     1    ]
/// ```
///
/// `A^T A = [[2.0625, 1.5], [1.5, 2.0625]]`, so `c = 0.01 * 2.0625 = 0.020625`
/// and `N = A^T A + c I = [[2.083125, 1.5], [1.5, 2.083125]]`, `det N =
/// 2.083125^2 - 1.5^2 = 2.089409765625`.
fn hand_setup(reference: [f64; 3]) -> (MimoSetup, MatrixStack) {
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
    let mut r = HermitianStack::zeros(1, 3);
    for k in 0..3 {
        r.set(0, k, k, C64::new(reference[k], 0.0));
    }
    let setup = MimoSetup {
        controls: 3,
        drives: 2,
        lines: 1,
        df_hz: 1.0,
        band: (0, 1),
        reference: r,
        tolerances: Tolerances::none(1, 3),
    };
    (setup, h)
}

/// The law's step from levels `last` played, with the response autospectra
/// `x` measured (cross terms of the response set to something the law must
/// ignore), returning the new levels and the outputs.
fn hand_step(
    reference: [f64; 3],
    last: [f64; 2],
    x: [f64; 3],
    params: &[(&'static str, ParamVal)],
) -> (Vec<f64>, hertzforge_sdm::law::MimoLoopOutputs) {
    let (setup, h) = hand_setup(reference);
    let mut law =
        IndependentDrivesLaw::with_params(&setup, &published_with::<IndependentDrivesLaw>(params))
            .expect("law");
    let mut response = HermitianStack::zeros(1, 3);
    for k in 0..3 {
        response.set(0, k, k, C64::new(x[k], 0.0));
    }
    response.set(0, 0, 2, C64::new(0.1, -0.2));
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
        band: (0, 1),
        loop_index: l,
        last_drive: Some(&played),
        response_drive_csd: Some(&csd),
        sysid_response: None,
        sysid_drive: None,
        sysid_noise: None,
    };
    law.seed(&inputs(0)).expect("seed");
    let out = law.iterate(&inputs(1)).expect("step");
    assert_diagonal("hand", &out.drive);
    (out.drive.diagonal(0), out)
}

const DET: f64 = 2.089409765625;

#[test]
fn the_feedback_step_is_the_hand_worked_one() {
    // r = (1.5, 2.5, 3.5), x = (1.2, 2.0, 2.5): e = (0.3, 0.5, 1.0) and
    // A^T e = (0.3 + 0.125 + 1.0, 0.075 + 0.5 + 1.0) = (1.425, 1.575).
    // N^-1 A^T e = (2.083125 * 1.425 - 1.5 * 1.575, -1.5 * 1.425 + 2.083125 * 1.575) / det
    //            = (0.605953125, 1.143421875) / det.
    let step = [0.605953125 / DET, 1.143421875 / DET];
    let (g, out) = hand_step([1.5, 2.5, 3.5], [1.0, 2.0], [1.2, 2.0, 2.5], &[]);
    let want = [1.0 + step[0], 2.0 + step[1]];
    println!("hand-worked step: {g:?} against {want:?}");
    for d in 0..2 {
        assert!(
            (g[d] - want[d]).abs() < 1e-12,
            "drive {d}: {} against {}",
            g[d],
            want[d]
        );
    }
    assert_eq!(scalar(&out, "limited"), 0.0);
    assert_eq!(per_line(&out, "negative_levels"), vec![0.0]);
}

#[test]
fn with_the_change_limit_active_the_step_is_held_to_it() {
    // The same step under a 10 percent rise limit: each level rises by at most
    // 10 percent of what was played, (1.1, 2.2), both held.
    let (g, out) = hand_step(
        [1.5, 2.5, 3.5],
        [1.0, 2.0],
        [1.2, 2.0, 2.5],
        &[("limit_up_percent", ParamVal::Number(10.0))],
    );
    assert!(
        (g[0] - 1.1).abs() < 1e-12 && (g[1] - 2.2).abs() < 1e-12,
        "{g:?}"
    );
    assert_eq!(scalar(&out, "limited"), 2.0);
    // A response far above the reference: r = (0.1, 0.1, 0.1), x = (1.2, 2.0,
    // 2.5), e = (-1.1, -1.9, -2.4), A^T e = (-1.1 - 0.475 - 2.4, -0.275 - 1.9 -
    // 2.4) = (-3.975, -4.575), and N^-1 A^T e = (2.083125 * -3.975 + 1.5 *
    // 4.575, 1.5 * 3.975 - 2.083125 * 4.575) / det = (-1.417921875,
    // -3.567796875) / det = (-0.6786, -1.7076). From (1.0, 0.5) played: drive 0
    // falls to 0.3214, below the published fall limit of 67 percent, and is
    // held at 0.33; drive 1 goes negative, is set to zero and counted, and is
    // then held at 0.33 * 0.5 = 0.165.
    let (g, out) = hand_step([0.1, 0.1, 0.1], [1.0, 0.5], [1.2, 2.0, 2.5], &[]);
    let raw = [1.0 - 1.417921875 / DET, 0.5 - 3.567796875 / DET];
    assert!(
        raw[0] > 0.0 && raw[0] < 0.33 && raw[1] < 0.0,
        "{raw:?}: the hand arithmetic"
    );
    assert!((g[0] - 0.33).abs() < 1e-12, "{g:?}");
    assert!((g[1] - 0.165).abs() < 1e-12, "{g:?}");
    assert_eq!(per_line(&out, "negative_levels"), vec![1.0]);
    assert_eq!(scalar(&out, "limited"), 2.0);
    // Without the limit (99 percent, the most the parameter allows) drive 1 is
    // the negative rule's zero, at 1 percent of what was played.
    let (g, _) = hand_step(
        [0.1, 0.1, 0.1],
        [1.0, 0.5],
        [1.2, 2.0, 2.5],
        &[("limit_down_percent", ParamVal::Number(99.0))],
    );
    assert!((g[0] - raw[0]).abs() < 1e-12, "{g:?} against {raw:?}");
    assert!((g[1] - 0.005).abs() < 1e-12, "{g:?}");
}

/// The law's step on two lines with the hand-worked structure on each, the
/// same reference and response on both, from levels `last[line]` played.
fn hand_step_two_lines(last: [[f64; 2]; 2]) -> (Vec<[f64; 2]>, f64) {
    let (one, h1) = hand_setup([1.5, 2.5, 3.5]);
    let mut d = h1.data().to_vec();
    d.extend_from_slice(h1.data());
    let h = MatrixStack::from_vec(2, 3, 2, d).expect("dims");
    let mut reference = HermitianStack::zeros(2, 3);
    let mut response = HermitianStack::zeros(2, 3);
    let mut played = HermitianStack::zeros(2, 2);
    for line in 0..2 {
        for k in 0..3 {
            reference.set(line, k, k, one.reference.get(0, k, k));
            response.set(line, k, k, C64::new([1.2, 2.0, 2.5][k], 0.0));
        }
        for j in 0..2 {
            played.set(line, j, j, C64::new(last[line][j], 0.0));
        }
    }
    let setup = MimoSetup {
        lines: 2,
        band: (0, 2),
        reference,
        tolerances: Tolerances::none(2, 3),
        ..one
    };
    let mut law = IndependentDrivesLaw::with_params(&setup, &pub_ind()).expect("law");
    let csd = MatrixStack::zeros(2, 3, 2);
    let coherence = [1.0; 6];
    let inputs = |l: u64| MimoLoopInputs {
        reference: &setup.reference,
        tolerances: &setup.tolerances,
        response: &response,
        response_dof: 128.0,
        frf: &h,
        multiple_coherence: &coherence,
        df_hz: 1.0,
        band: (0, 2),
        loop_index: l,
        last_drive: Some(&played),
        response_drive_csd: Some(&csd),
        sysid_response: None,
        sysid_drive: None,
        sysid_noise: None,
    };
    law.seed(&inputs(0)).expect("seed");
    let out = law.iterate(&inputs(1)).expect("step");
    let g = (0..2)
        .map(|l| [out.drive.get(l, 0, 0).re, out.drive.get(l, 1, 1).re])
        .collect();
    (g, scalar(&out, "limited"))
}

#[test]
fn a_level_at_zero_rises_from_the_floor_below_its_drives_mean() {
    // Drive 1 played at zero on line 0 and at 1 on line 1: its in-band mean is
    // 0.5, so a rise on line 0 is measured from 1e-6 * 0.5 and may reach twice
    // that, 1e-6, though the step asks for 1.143421875 / DET = 0.547. On line 1
    // the step is inside the limit. From the floor the level doubles once a
    // loop, so 60 dB takes about 19 loops.
    let step = 1.143421875 / DET;
    let (g, limited) = hand_step_two_lines([[1.0, 0.0], [1.0, 1.0]]);
    println!("drive 1 zero on one line of two: new levels {g:?}");
    assert!((g[0][1] - 1e-6).abs() < 1e-18, "{g:?}");
    assert!((g[1][1] - (1.0 + step)).abs() < 1e-12, "{g:?}");
    assert_eq!(limited, 1.0);
}

#[test]
fn a_drive_zero_on_every_line_has_no_rise_limit() {
    // No level to measure a rise from: the step is played whole, and the core's
    // change cap governs what reaches the structure.
    let step = 1.143421875 / DET;
    let (g, limited) = hand_step_two_lines([[1.0, 0.0], [1.0, 0.0]]);
    println!("drive 1 zero on every line: new levels {g:?}");
    for line in 0..2 {
        assert!((g[line][1] - step).abs() < 1e-12, "{g:?}");
    }
    assert_eq!(limited, 0.0);
}

#[test]
fn the_default_step_is_the_whole_published_step() {
    // A = |H|^2 = [[1, 0.25], [0.25, 1], [1, 1]] has full column rank, so with
    // no regularization S+ A = I. The reference is A (1, 2) = (1.5, 2.25, 3.0);
    // from (0.8, 1.6) played, measured A (0.8, 1.6) = (1.2, 1.8, 2.4), one whole
    // step lands on (1, 2) to rounding. A half step would give (0.9, 1.8).
    let exact = [("regularization_constant", ParamVal::Number(0.0))];
    let (g, out) = hand_step([1.5, 2.25, 3.0], [0.8, 1.6], [1.2, 1.8, 2.4], &exact);
    println!("whole step from (0.8, 1.6): {g:?}");
    assert!(
        (g[0] - 1.0).abs() < 1e-12 && (g[1] - 2.0).abs() < 1e-12,
        "{g:?}"
    );
    assert_eq!(scalar(&out, "limited"), 0.0);
    let half = [
        ("regularization_constant", ParamVal::Number(0.0)),
        ("convergence", ParamVal::Number(0.5)),
    ];
    let (g, _) = hand_step([1.5, 2.25, 3.0], [0.8, 1.6], [1.2, 1.8, 2.4], &half);
    assert!(
        (g[0] - 0.9).abs() < 1e-12 && (g[1] - 1.8).abs() < 1e-12,
        "{g:?}"
    );
}

#[test]
fn the_constant_is_set_line_by_line() {
    // Line 1's structure is ten times line 0's, so A is 100 times and A^T A
    // 1e4 times as large. With the constant set per line S+ scales by 1/100
    // exactly, line 0's levels are those of line 0 alone, and line 1's are
    // line 0's over 100. One constant for the band would regularize line 0
    // with line 1's scale.
    let (one, h1) = hand_setup([1.5, 2.5, 3.5]);
    let mut d = h1.data().to_vec();
    d.extend(h1.data().iter().map(|h| *h * 10.0));
    let h = MatrixStack::from_vec(2, 3, 2, d).expect("dims");
    let mut r = HermitianStack::zeros(2, 3);
    for line in 0..2 {
        for k in 0..3 {
            r.set(line, k, k, one.reference.get(0, k, k));
        }
    }
    let alone = independent_levels(&h1, &one.reference, (0, 1), 0.01, 0.0).expect("levels");
    let both = independent_levels(&h, &r, (0, 2), 0.01, 0.0).expect("levels");
    println!("line alone {:?}, two lines {:?}", alone.levels, both.levels);
    for j in 0..2 {
        let a = alone.levels[j];
        assert!(
            (both.levels[j] - a).abs() <= 1e-12 * a.abs(),
            "{:?}",
            both.levels
        );
        assert!(
            (both.levels[2 + j] * 100.0 - a).abs() <= 1e-12 * a.abs(),
            "{:?}",
            both.levels
        );
    }
}

/// The worst `|10 log10(response / reference)|` over the controls and in-band
/// lines for the noiseless response of `drive`, against `target`'s
/// autospectra.
fn worst_db_against(s: &Synthetic, drive: &HermitianStack, target: &HermitianStack) -> f64 {
    let y = congruence(&s.plant, drive);
    let mut worst: f64 = 0.0;
    for line in BAND.0..BAND.1 {
        for c in 0..s.setup.controls {
            let (a, b) = (y.get(line, c, c).re, target.get(line, c, c).re);
            worst = worst.max((10.0 * (a / b).log10()).abs());
        }
    }
    worst
}

/// The `min_drive` tests' structure: a reachable reference and a 5 percent
/// error in the estimate the law is handed.
fn synthetic(seed: u64, nc: usize, nd: usize) -> Synthetic {
    let plant = well_conditioned(seed, nc, nd, LINES);
    let reference = reachable_reference(&plant, seed, BAND);
    let frf = perturbed(&plant, seed, 0.05);
    run_on(seed, plant, frf, reference, BAND, 0.01)
}

#[test]
fn the_nearly_independent_loop_converges_as_the_minimum_drive_loop_does() {
    // The thresholds of `min_drive.rs`: within 0.5 dB at loop 60 on a square
    // structure, within 1 dB at loop 30 with more controls than drives. The
    // target keeps the specified autospectra. With more controls than drives
    // no drive produces it (Cap, Norman and Smallwood do not take this up), so
    // there the bar is against the part of it the estimate can produce, the
    // projection the default closed loop drives toward, and the distance
    // from the target itself is printed.
    for (nc, nd, loops, bar) in [(4usize, 4usize, 60usize, 0.5), (6, 4, 30, 1.0)] {
        let s = synthetic(33, nc, nd);
        let (target, _) =
            nearly_independent_reference(&s.frf, &s.setup.reference, BAND, 0.01).expect("target");
        assert!(autospectra_gap(&target, &s.setup.reference) < 1e-12);
        let goal = if nc > nd {
            projection_achievable(&s.frf, &target, Regularization::Truncated { rcond: 1e-3 })
                .expect("projection")
        } else {
            target.clone()
        };
        // Cap, Norman and Smallwood's Eq. 5 done once from the estimate.
        let (hp, _) = pinv(&s.frf, Regularization::Truncated { rcond: 1e-3 }).expect("pinv");
        let one_shot = worst_db_against(&s, &congruence(&hp, &target), &target);
        let mut law = NearlyIndependentLaw::with_params(&s.setup, &pub_near()).expect("law");
        let run = closed_loop(&mut law, &s, loops);
        let worst = worst_db_against(&s, run.last(), &goal);
        let from_target = worst_db_against(&s, run.last(), &target);
        println!(
            "nearly independent {nc} x {nd}: {worst:.3} dB from what it can reach, \
             {from_target:.3} dB from its target at loop {loops}, {one_shot:.3} dB for Eq. 5 \
             done once"
        );
        assert!(worst < bar, "{nc} x {nd}: {worst} dB");
        // The loop ends no further from the target than the one-shot solve.
        assert!(
            from_target <= one_shot + 0.1,
            "{nc} x {nd}: {from_target} dB against {one_shot} dB one shot"
        );
        for (l, d) in run.drives().enumerate() {
            assert!(worst_eigen_ratio(d) >= -1e-9, "loop {l}");
        }
    }
}

#[test]
fn where_feasible_the_independent_loop_reaches_the_reference_through_a_gain_error() {
    // The structure is 1.5 dB less sensitive than the estimate the law is
    // handed, with a 10 degree phase error the squared magnitudes cannot see.
    let gain = C64::from_polar(10f64.powf(-1.5 / 20.0), 10f64.to_radians());
    for (nc, nd, seed) in [(4usize, 4usize, 70u64), (6, 4, 71)] {
        let estimate = well_conditioned(seed, nc, nd, LINES);
        let plant = scaled(&estimate, gain);
        let g0 = positive_levels(seed, nd);
        let reference = congruence(&plant, &diag_drive(&g0, LINES, nd));
        let s = run_on(seed, plant, estimate, reference, BAND, 0.01);
        let mut law = IndependentDrivesLaw::with_params(&s.setup, &pub_ind()).expect("law");
        let run = closed_loop(&mut law, &s, 30);
        let seed_db = worst_db(&s, &run.outputs[0].drive);
        let worst = worst_db(&s, run.last());
        println!(
            "independent {nc} x {nd} through a 1.5 dB gain error: {seed_db:.3} dB at the seed, \
             {worst:.3} dB after 30 loops"
        );
        assert!(seed_db > 1.0, "the gain error did not show: {seed_db}");
        assert!(worst < 0.5, "{nc} x {nd}: {worst} dB");
    }
}

#[test]
fn where_not_feasible_the_independent_loop_settles_and_the_count_says_why() {
    // Without measurement noise the drive stops moving. With 1 percent noise
    // the published full step passes the noise to the drive, 0.4 to 0.7 dB a
    // loop on this structure, so there the test is that the response holds:
    // the worst control stays where it settled.
    for eps in [0.0, 0.01] {
        let mut s = needs_a_negative_level(80);
        s.eps = eps;
        let mut law = IndependentDrivesLaw::with_params(&s.setup, &pub_ind()).expect("law");
        let run = closed_loop(&mut law, &s, 30);
        let changes: Vec<f64> = (28..=30).map(|l| run.scalar(l, "correction_db")).collect();
        let negative: f64 = run.per_line(30, "negative_levels").iter().sum();
        let worsts: Vec<f64> = (10..=30)
            .map(|l| worst_db(&s, &run.outputs[l].drive))
            .collect();
        let worst = worsts[worsts.len() - 1];
        let spread = worsts.iter().copied().fold(f64::MIN, f64::max)
            - worsts.iter().copied().fold(f64::MAX, f64::min);
        println!(
            "infeasible 3 x 3, noise {eps}: last changes {changes:?} dB, {negative} lines with \
             a negative level, worst control {worst:.2} dB, moving {spread:.3} dB over loops 10 to 30"
        );
        if eps == 0.0 {
            // Settled, as the law check judges it: under 0.5 dB a loop.
            assert!(changes.iter().all(|&c| c < 0.5), "{changes:?}");
        }
        // The law check's settling bar, applied to the response.
        assert!(spread < 0.5, "{spread}");
        // Not met, and the count says on which lines.
        assert!(worst > 0.5, "{worst}");
        assert!(negative >= (BAND.1 - BAND.0) as f64 / 2.0, "{negative}");
    }
}

#[test]
fn under_noise_the_two_refinements_together_move_the_infeasible_loop_more() {
    // The infeasible 3 x 3 with 1 percent noise, over loops 10 to 30: the
    // spread of the worst control and the mean largest drive change a loop,
    // with both refinements, published, each refinement alone (the
    // non-negative solve alone is the defaults), and both refinements with
    // `convergence` 0.5. With both, levels near zero switch between held at
    // zero and free from loop to loop under the noise.
    let choice = |v: &str| ParamVal::Choice(v.into());
    let settings: Vec<(&str, Vec<(&'static str, ParamVal)>)> = vec![
        (
            "both refinements (solve nonnegative, solve_weighting target)",
            vec![("solve_weighting", choice("target"))],
        ),
        ("published", pub_ind()),
        (
            "weighting only",
            published_with::<IndependentDrivesLaw>(&[("solve_weighting", choice("target"))]),
        ),
        (
            "the defaults (solve nonnegative, solve_weighting none)",
            vec![],
        ),
        (
            "both refinements, convergence 0.5",
            vec![
                ("solve_weighting", choice("target")),
                ("convergence", ParamVal::Number(0.5)),
            ],
        ),
    ];
    let mut measured = Vec::new();
    for (name, params) in &settings {
        let mut s = needs_a_negative_level(80);
        s.eps = 0.01;
        let mut law = IndependentDrivesLaw::with_params(&s.setup, params).expect("law");
        let run = closed_loop(&mut law, &s, 30);
        let worsts: Vec<f64> = (10..=30)
            .map(|l| worst_db(&s, &run.outputs[l].drive))
            .collect();
        let spread = worsts.iter().copied().fold(f64::MIN, f64::max)
            - worsts.iter().copied().fold(f64::MAX, f64::min);
        let mean = (10..=30)
            .map(|l| run.scalar(l, "correction_db"))
            .sum::<f64>()
            / 21.0;
        // Loops on which the number of levels held at zero changed.
        let held: Vec<f64> = (9..=30)
            .map(|l| run.per_line(l, "held_at_zero").iter().sum())
            .collect();
        let switches = held.windows(2).filter(|w| w[0] != w[1]).count();
        println!(
            "infeasible 3 x 3, noise 0.01, {name}: spread {spread:.3} dB, mean change \
             {mean:.3} dB a loop, held count changed on {switches} loops"
        );
        measured.push((spread, mean, switches));
    }
    // As measured (spread, mean change), each held to 5 percent.
    let want = [
        (0.627, 2.653),
        (0.295, 0.475),
        (0.175, 0.444),
        (0.266, 0.453),
        (0.409, 1.433),
    ];
    for ((name, _), (&(spread, mean, _), &(ws, wm))) in
        settings.iter().zip(measured.iter().zip(&want))
    {
        assert!((spread - ws).abs() <= 0.05 * ws, "{name}: spread {spread}");
        assert!((mean - wm).abs() <= 0.05 * wm, "{name}: mean change {mean}");
    }
    // With both refinements the count of held levels changes under the
    // noise; published and with either refinement alone it does not.
    assert!(measured[0].2 > 0, "{:?}", measured[0]);
    for m in &measured[1..4] {
        assert_eq!(m.2, 0, "{m:?}");
    }
}

/// A feasible, well-conditioned 3 x 3 structure and a reference independent
/// drives meet, with the harness's 1 percent measurement noise.
fn feasible_noisy() -> Synthetic {
    let plant = well_conditioned(81, 3, 3, LINES);
    let levels = positive_levels(81, 3);
    let mut s = feasible(81, plant, &levels, 0.0);
    s.eps = 0.01;
    s
}

#[test]
fn under_noise_the_whole_step_moves_the_drive_and_a_smaller_factor_moves_it_less() {
    let mut mean_change = Vec::new();
    for k in [1.0, 0.5] {
        let s = feasible_noisy();
        let given = published_with::<IndependentDrivesLaw>(&[("convergence", ParamVal::Number(k))]);
        let mut law = IndependentDrivesLaw::with_params(&s.setup, &given).expect("law");
        let run = closed_loop(&mut law, &s, 30);
        let changes: Vec<f64> = (10..=30).map(|l| run.scalar(l, "correction_db")).collect();
        let worsts: Vec<f64> = (10..=30)
            .map(|l| worst_db(&s, &run.outputs[l].drive))
            .collect();
        let spread = worsts.iter().copied().fold(f64::MIN, f64::max)
            - worsts.iter().copied().fold(f64::MAX, f64::min);
        let mean = changes.iter().sum::<f64>() / changes.len() as f64;
        let largest = changes.iter().copied().fold(0.0f64, f64::max);
        println!(
            "feasible 3 x 3, 1 percent noise, convergence {k}: drive change loops 28 to 30 \
             {:.3?} dB, mean {mean:.3} and largest {largest:.3} dB over loops 10 to 30; worst \
             control {:.3} dB at loop 30, spread {spread:.3} dB over loops 10 to 30",
            &changes[18..],
            worsts[20]
        );
        // The response holds while the drive moves.
        assert!(spread < 0.5, "{spread}");
        mean_change.push(mean);
    }
    assert!(
        mean_change[1] < mean_change[0],
        "a half step did not move the drive less: {mean_change:?}"
    );
}

#[test]
fn iterate_before_seed_and_a_missing_last_drive_are_refused() {
    let s = Synthetic::random(90, 3, 2, LINES);
    let mut law = IndependentDrivesLaw::with_params(&s.setup, &[]).expect("law");
    let csd = cross(&s.plant, &s.sysid_drive);
    let with = s.inputs(&s.sysid_response, &s.sysid_drive, &csd, 1, law.needs());
    let Err(err) = law.iterate(&with) else {
        panic!("not refused")
    };
    assert!(err.0.contains("before seed"), "{}", err.0);
    law.seed(&with).expect("seed");
    let without = s.inputs(&s.sysid_response, &s.sysid_drive, &csd, 1, &[]);
    let Err(err) = law.iterate(&without) else {
        panic!("not refused")
    };
    assert!(err.0.contains("last drive"), "{}", err.0);
    let mut near = NearlyIndependentLaw::with_params(&s.setup, &[]).expect("law");
    assert!(near.iterate(&without).is_err());
}

// -------------------------------------------------------------------------
// The published forms, pinned from commit 5decb4a5
// -------------------------------------------------------------------------

/// Every in-band diagonal drive entry, summed, and three entries.
fn fingerprint(g: &HermitianStack) -> [f64; 4] {
    let n = g.n();
    let mut sum = 0.0;
    for line in BAND.0..BAND.1 {
        for d in 0..n {
            sum += g.get(line, d, d).re;
        }
    }
    [
        sum,
        g.get(BAND.0, 0, 0).re,
        g.get(BAND.0 + 17, n - 1, n - 1).re,
        g.get(BAND.1 - 1, 0, n - 1).norm(),
    ]
}

/// The parameters that give the published forms of both laws.
fn published() -> Vec<(&'static str, ParamVal)> {
    vec![
        ("solve", ParamVal::Choice("set_to_zero".into())),
        ("solve_weighting", ParamVal::Choice("none".into())),
        ("exact_tolerance", ParamVal::Number(0.0)),
        ("level_set", ParamVal::Toggle(false)),
    ]
}

/// Printed by the code of commit 5decb4a5, before the refinements: law,
/// case, loop (-1: the nearly independent reference), fingerprint.
const PINNED: &[(&str, &str, i64, [f64; 4])] = &[
    (
        "independent",
        "6x4",
        0,
        [
            2.0945695421006287,
            0.041342309989361455,
            0.006013191538783034,
            0.0,
        ],
    ),
    (
        "independent",
        "6x4",
        1,
        [
            2.1849678661855485,
            0.04388736773277341,
            0.007345471440304391,
            0.0,
        ],
    ),
    (
        "independent",
        "6x4",
        5,
        [
            2.2148427746406822,
            0.04192090629739884,
            0.0073281619742067644,
            0.0,
        ],
    ),
    (
        "nearly",
        "6x4",
        0,
        [
            1.9322983150469617,
            0.033235280844885716,
            0.006185165039210962,
            0.00011997452673139224,
        ],
    ),
    (
        "nearly",
        "6x4",
        1,
        [
            1.9223056739576576,
            0.032742593189651,
            0.006175692357445786,
            0.0001327171307765957,
        ],
    ),
    (
        "nearly",
        "6x4",
        5,
        [
            1.9241840857513948,
            0.03224403401320896,
            0.006176854697786981,
            9.747507053483406e-5,
        ],
    ),
    (
        "reference",
        "6x4",
        -1,
        [
            4.621882610475126,
            0.022769799544710808,
            0.011441352684717491,
            0.006204606252918832,
        ],
    ),
    (
        "independent",
        "3x3neg",
        0,
        [0.9488509544269965, 0.009885579242546237, 0.0, 0.0],
    ),
    (
        "independent",
        "3x3neg",
        1,
        [0.9582595437205905, 0.00992379081647185, 0.0, 0.0],
    ),
    (
        "independent",
        "3x3neg",
        5,
        [0.9589175994861803, 0.010002551749624748, 0.0, 0.0],
    ),
    (
        "nearly",
        "3x3neg",
        0,
        [
            1.0088949296183778,
            0.017043616614229973,
            0.0010762569061763168,
            0.0016780782019599956,
        ],
    ),
    (
        "nearly",
        "3x3neg",
        1,
        [
            1.0087050387190384,
            0.0169827165131694,
            0.0010609962649488795,
            0.001686483886404145,
        ],
    ),
    (
        "nearly",
        "3x3neg",
        5,
        [
            1.0074097441409633,
            0.016933652765063423,
            0.0010703087997476293,
            0.001641058366777867,
        ],
    ),
    (
        "reference",
        "3x3neg",
        -1,
        [
            1.2239948611322422,
            0.011241510796354895,
            0.000751269086267139,
            0.0008853875796315238,
        ],
    ),
    (
        "independent",
        "3x5",
        0,
        [
            2.557759480419605,
            0.009525102605957458,
            0.012263342076599543,
            0.0,
        ],
    ),
    (
        "independent",
        "3x5",
        1,
        [
            2.6466803668908017,
            0.007648419908588037,
            0.012691021409553601,
            0.0,
        ],
    ),
    (
        "independent",
        "3x5",
        5,
        [
            2.680073509550447,
            0.009857968214684428,
            0.01239856194942411,
            0.0,
        ],
    ),
    (
        "nearly",
        "3x5",
        0,
        [
            1.4507113856965712,
            0.0068425278885957015,
            0.006363882804270293,
            0.0008411176878307101,
        ],
    ),
    (
        "nearly",
        "3x5",
        1,
        [
            1.4477047354857542,
            0.006805952453617438,
            0.006472889281696079,
            0.00080166201809892,
        ],
    ),
    (
        "nearly",
        "3x5",
        5,
        [
            1.4466928211531038,
            0.007158610138405746,
            0.0065537991101481275,
            0.000780719831685278,
        ],
    ),
    (
        "reference",
        "3x5",
        -1,
        [
            3.4916745095839135,
            0.02554845159581733,
            0.030598396468249623,
            0.003786896655621966,
        ],
    ),
];

#[test]
fn with_the_published_parameters_both_laws_give_what_they_gave_before_the_refinements() {
    let cases: Vec<(&str, Synthetic)> = vec![
        ("6x4", synthetic(33, 6, 4)),
        ("3x3neg", {
            let mut s = needs_a_negative_level(80);
            s.eps = 0.01;
            s
        }),
        ("3x5", synthetic(34, 3, 5)),
    ];
    let mut worst: f64 = 0.0;
    for (name, s) in &cases {
        let mut got: Vec<(&str, i64, [f64; 4])> = Vec::new();
        let mut law = IndependentDrivesLaw::with_params(&s.setup, &pub_ind()).expect("law");
        let run = closed_loop(&mut law, s, 5);
        for l in [0usize, 1, 5] {
            got.push(("independent", l as i64, fingerprint(&run.outputs[l].drive)));
        }
        let mut near = NearlyIndependentLaw::with_params(&s.setup, &pub_near()).expect("law");
        let run = closed_loop(&mut near, s, 5);
        for l in [0usize, 1, 5] {
            got.push(("nearly", l as i64, fingerprint(&run.outputs[l].drive)));
        }
        let (t, _) =
            nearly_independent_reference(&s.frf, &s.setup.reference, BAND, 0.01).expect("target");
        got.push(("reference", -1, fingerprint(&t)));
        for (law, l, f) in got {
            let (_, _, _, want) = PINNED
                .iter()
                .find(|(a, c, k, _)| *a == law && c == name && *k == l)
                .expect("pinned");
            for j in 0..4 {
                let scale = want[0].abs();
                let gap = (f[j] - want[j]).abs() / scale;
                worst = worst.max(gap);
                assert!(
                    gap <= 1e-12,
                    "{law} {name} loop {l} [{j}]: {} against {}",
                    f[j],
                    want[j]
                );
            }
        }
    }
    println!("published forms against the pinned numbers: worst {worst:.2e} relative");
}

/// Printed by the round 5 code with `exact_tolerance` 0 and the level set
/// off: N. Hunter's weighting and non-negative solve, with the regularization
/// scaled drive by drive under the weighting (Marquardt's scaling). Case,
/// loop, fingerprint. A regression pin only; round 3's numbers, from the
/// one constant per line, no longer hold.
const PINNED_ROUND_5: &[(&str, usize, [f64; 4])] = &[
    (
        "6x4",
        0,
        [
            1.9389481014062129,
            0.033308937221682225,
            0.0047043457567341644,
            0.00016816519813205814,
        ],
    ),
    (
        "6x4",
        1,
        [
            1.9338310490232353,
            0.032773517311516225,
            0.004703733506733967,
            0.00017258735602371992,
        ],
    ),
    (
        "6x4",
        5,
        [
            1.9342146370994375,
            0.032339625486439746,
            0.004723060266112141,
            0.00019683686304795744,
        ],
    ),
    (
        "3x3neg",
        0,
        [
            0.9804207988903297,
            0.01763367912648192,
            0.0015940826150752014,
            0.0024503322253968305,
        ],
    ),
    (
        "3x3neg",
        1,
        [
            0.979065946950929,
            0.01757210501905116,
            0.0015737152747466261,
            0.002456970796820915,
        ],
    ),
    (
        "3x3neg",
        5,
        [
            0.9799695759121739,
            0.01752669229019966,
            0.001586667957780767,
            0.0024008496615724735,
        ],
    ),
    (
        "3x5",
        0,
        [
            1.497432420556191,
            0.005904660659743874,
            0.005964645488281702,
            0.0008437909554581137,
        ],
    ),
    (
        "3x5",
        1,
        [
            1.4925421083769808,
            0.0059546772087860364,
            0.0060430197625793245,
            0.0008037050972265801,
        ],
    ),
    (
        "3x5",
        5,
        [
            1.4926367891857921,
            0.0061915616544864485,
            0.0061068225848305,
            0.0007812305703297185,
        ],
    ),
];

#[test]
fn with_marquardts_scaling_and_the_level_set_and_exactness_test_off_nearly_gives_round_5s_numbers()
{
    let cases: Vec<(&str, Synthetic)> = vec![
        ("6x4", synthetic(33, 6, 4)),
        ("3x3neg", {
            let mut s = needs_a_negative_level(80);
            s.eps = 0.01;
            s
        }),
        ("3x5", synthetic(34, 3, 5)),
    ];
    let mut worst: f64 = 0.0;
    for (name, s) in &cases {
        let p = [
            ("exact_tolerance", ParamVal::Number(0.0)),
            ("level_set", ParamVal::Toggle(false)),
        ];
        let mut near = NearlyIndependentLaw::with_params(&s.setup, &p).expect("law");
        let run = closed_loop(&mut near, s, 5);
        for &(case, l, want) in PINNED_ROUND_5.iter().filter(|(c, _, _)| c == name) {
            let f = fingerprint(&run.outputs[l].drive);
            for j in 0..4 {
                let gap = (f[j] - want[j]).abs() / want[0].abs();
                worst = worst.max(gap);
                assert!(
                    gap <= 1e-12,
                    "{case} loop {l} [{j}]: {} against {}",
                    f[j],
                    want[j]
                );
            }
        }
    }
    println!("level set and exactness test off against round 5: worst {worst:.2e} relative");
}

// -------------------------------------------------------------------------
// The regularization under the weighting (Marquardt's scaling)
// -------------------------------------------------------------------------

#[test]
fn with_the_target_weighting_on_a_decoupled_structure_the_seed_is_short_by_the_constant_and_the_loop_closes_it(
) {
    // The twin, with the weighting by the target (off by default), of the
    // decoupled test above: with the weighting each drive is regularized by
    // the same relative amount, so the seed is short by c / (1 + c) of every
    // target and each loop cuts the gap by the same factor again.
    let h = decoupled(1, 4);
    let mut s = Synthetic::with_plant(1, h, BAND, 0.0);
    s.frf = s.plant.clone();
    let target = [("solve_weighting", ParamVal::Choice("target".into()))];
    let mut law = IndependentDrivesLaw::with_params(&s.setup, &target).expect("law");
    let run = closed_loop(&mut law, &s, 30);
    let gaps: Vec<f64> = run
        .drives()
        .map(|d| autospectra_gap(&congruence(&s.plant, d), &s.setup.reference))
        .collect();
    println!(
        "decoupled 4 x 4, weighting by the target: seed {:.3e}, loop 5 {:.3e}, loop 10 {:.3e}, loop 30 {:.3e}",
        gaps[0], gaps[5], gaps[10], gaps[30]
    );
    let c: f64 = 0.01;
    assert!((gaps[0] - c / (1.0 + c)).abs() < 1e-9, "{}", gaps[0]);
    assert!(
        gaps[0] > 1e-4,
        "the regularization did not show: {}",
        gaps[0]
    );
    assert!(gaps[10] < 1e-9, "{}", gaps[10]);
    assert!(gaps[30] < 1e-12, "{}", gaps[30]);
    for (l, d) in run.drives().enumerate() {
        assert_diagonal(&format!("loop {l}"), d);
        assert_out_of_band_zero(&format!("loop {l}"), d);
    }
}

#[test]
fn with_targets_a_thousand_apart_both_drives_converge_at_the_same_rate() {
    let h = decoupled(5, 2);
    let mut reference = HermitianStack::zeros(LINES, 2);
    for line in BAND.0..BAND.1 {
        reference.set(line, 0, 0, C64::new(1e-3, 0.0));
        reference.set(line, 1, 1, C64::new(1.0, 0.0));
    }
    let s = run_on(5, h.clone(), h, reference, BAND, 0.0);
    // With the weighting by the target, off by default.
    let target = [("solve_weighting", ParamVal::Choice("target".into()))];
    let mut law = IndependentDrivesLaw::with_params(&s.setup, &target).expect("law");
    let run = closed_loop(&mut law, &s, 6);
    for (l, d) in run.drives().enumerate() {
        let y = congruence(&s.plant, d);
        let mut short = [0.0f64; 2];
        for line in BAND.0..BAND.1 {
            for k in 0..2 {
                let t = s.setup.reference.get(line, k, k).re;
                short[k] = short[k].max(1.0 - y.get(line, k, k).re / t);
            }
        }
        println!(
            "loop {l}: each control short by {:.3e} and {:.3e}",
            short[0], short[1]
        );
        // Through loop 3: by loop 4 the gap is near rounding.
        if l < 4 {
            let want = (0.01f64 / 1.01).powi(l as i32 + 1);
            for k in 0..2 {
                assert!(
                    (short[k] - want).abs() <= 1e-6 * want,
                    "loop {l}, control {k}: {} against {want}",
                    short[k]
                );
            }
        }
    }
}

#[test]
fn with_the_target_weighting_the_independent_loop_settles_low_where_the_targets_cannot_all_be_met()
{
    // Why the weighting is off by default in `independent` (its Choice 21).
    // Tall structures, the estimate exact, no noise: the targets are the
    // autospectra of uncorrelated drives a decade apart in level, each moved
    // by a random factor within 3 dB, so no uncorrelated drives meet them and
    // the targets differ between controls by more than a decade. The level
    // error of a control is 10 log10 of its response's power over its
    // target's, summed over the band; the mean over the controls after 30
    // loops is negative with the weighting on and near zero with it off.
    for (nc, nd, seed) in [(6usize, 3usize, 90u64), (12, 6, 94)] {
        let h = well_conditioned(seed, nc, nd, LINES);
        let mut rng = Rng::new(seed);
        let mut g = HermitianStack::zeros(LINES, nd);
        for line in BAND.0..BAND.1 {
            for d in 0..nd {
                let level = 1e-2 * 10f64.powf(2.0 * d as f64 / (nd - 1) as f64);
                g.set(line, d, d, C64::new(level, 0.0));
            }
        }
        let feasible = congruence(&h, &g);
        let mut reference = HermitianStack::zeros(LINES, nc);
        let mut apart: f64 = 0.0;
        for line in BAND.0..BAND.1 {
            let (mut lo, mut hi) = (f64::MAX, 0.0f64);
            for k in 0..nc {
                let off = 10f64.powf(3.0 * (2.0 * rng.uniform() - 1.0) / 10.0);
                let t = feasible.get(line, k, k).re * off;
                lo = lo.min(t);
                hi = hi.max(t);
                reference.set(line, k, k, C64::new(t, 0.0));
            }
            apart = apart.max(hi / lo);
        }
        assert!(apart >= 10.0, "{apart}");
        let s = run_on(seed, h.clone(), h, reference, BAND, 0.0);
        let mut level = [0.0f64; 2];
        for (j, w) in ["target", "none"].into_iter().enumerate() {
            let p = [("solve_weighting", ParamVal::Choice(w.into()))];
            let mut law = IndependentDrivesLaw::with_params(&s.setup, &p).expect("law");
            let run = closed_loop(&mut law, &s, 30);
            let y = congruence(&s.plant, run.last());
            for k in 0..nc {
                let (mut a, mut b) = (0.0, 0.0);
                for line in BAND.0..BAND.1 {
                    a += y.get(line, k, k).re;
                    b += s.setup.reference.get(line, k, k).re;
                }
                level[j] += 10.0 * (a / b).log10() / nc as f64;
            }
        }
        println!(
            "{nc} x {nd}, targets up to {apart:.0} times apart: mean level error {:.3} dB with \
             the weighting by the target, {:.3} dB without",
            level[0], level[1]
        );
        assert!(level[0] < -0.5, "{nc} x {nd}: {} dB", level[0]);
        assert!(level[1].abs() < 0.25, "{nc} x {nd}: {} dB", level[1]);
    }
}
