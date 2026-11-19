//! Each built-in law on synthetic plants with the closed loop simulated here:
//! the response is `H G H^H` through the true plant, measured with a small
//! perturbation, and the law is handed an estimate of the plant with an error
//! of its own. 30 loops after the seed.
//!
//! - Laws 1 and 3 drive the response within 0.5 dB of the reference on every
//!   control on every in-band line (3 x 2, 6 x 4, 12 x 6 and an
//!   ill-conditioned 6 x 4).
//! - Law 2 settles at the pseudo-inverse solution, and on a 6 x 4 plant at
//!   `P G_ref P^H`.
//! - Law 3 keeps exactly `m` directions and lowers the drive against the
//!   unconstrained inverse.
//! - Law 4's achieved coherence and phase match the identification's.
//! - Law 1's per-loop change never exceeds its cap.
//! - Every law's drive is positive semi-definite on every line, every loop.

// Index loops over lines and channels mirror the matrix algebra they check.
#![allow(clippy::needless_range_loop)]

mod common;

use common::*;
use hertzforge_mimo_laws::{
    buzz_reference, BuiltinMimo, BuiltinMimoLaw, BuzzMatchedLaw, OpenLoopLaw, ParamVal,
    PseudoInverseLaw, ShapeConstrainedLaw,
};
use hertzforge_sdm::law::MimoControlLaw;
use hertzforge_sdm::linalg::{self, Regularization};
use hertzforge_sdm::{HermitianStack, C64};

const LINES: usize = 64;
const BAND: (usize, usize) = (8, 56);
const LOOPS: usize = 30;

/// Every drive of `run` positive semi-definite to rounding.
fn assert_psd(name: &str, run: &Run) {
    for (l, d) in run.drives().enumerate() {
        let w = worst_eigen_ratio(d);
        assert!(w >= -1e-9, "{name}: loop {l} has eigenvalue ratio {w}");
    }
}

/// The well-conditioned plants of the brief, each with a reachable reference
/// and a 5 percent error in the estimate the law is handed.
fn plants() -> Vec<(String, Synthetic)> {
    let mut v = Vec::new();
    for (i, (nc, nd)) in [(3, 2), (6, 4), (12, 6)].into_iter().enumerate() {
        let seed = 100 + i as u64;
        let plant = well_conditioned(seed, nc, nd, LINES);
        let reference = reachable_reference(&plant, seed, BAND);
        let frf = perturbed(&plant, seed, 0.05);
        v.push((
            format!("{nc} x {nd}"),
            run_on(seed, plant, frf, reference, BAND, 0.01),
        ));
    }
    v
}

/// A 6 x 4 plant with singular values 1, 0.3, 0.1 and 1e-5 (condition
/// 1e5), a reference reachable in its three strong directions, and the
/// estimate off by a 1.5 dB, 10 degree gain.
fn ill_conditioned() -> Synthetic {
    let plant = plant_with_singular_values(7, 6, 4, LINES, &[1.0, 0.3, 0.1, 1e-5]);
    let svd = linalg::svd(&plant).expect("svd");
    // Drive only along the three strong right singular vectors.
    let mut x = HermitianStack::zeros(LINES, 4);
    let mut rng = Rng::new(77);
    for line in BAND.0..BAND.1 {
        let g3 = random_psd(&mut rng, 3, 1e-2);
        let v = svd.v.line(line);
        for r in 0..4 {
            for c in r..4 {
                let mut s = C64::new(0.0, 0.0);
                for a in 0..3 {
                    for b in 0..3 {
                        s += v[r * 4 + a] * g3[a * 3 + b] * v[c * 4 + b].conj();
                    }
                }
                x.set(line, r, c, s);
            }
        }
    }
    let reference = congruence(&plant, &x);
    let gain = C64::from_polar(10f64.powf(1.5 / 20.0), 10f64.to_radians());
    let frf = scaled(&plant, gain);
    run_on(8, plant, frf, reference, BAND, 0.01)
}

fn loops_to(run: &Run, s: &Synthetic, db: f64) -> Option<usize> {
    run.drives().position(|d| worst_db(s, d) <= db)
}

// -------------------------------------------------------------------------
// Laws 1 and 3 converge
// -------------------------------------------------------------------------

#[test]
fn the_closed_loop_laws_reach_the_reference_within_half_a_db() {
    let mut cases = plants();
    cases.push(("6 x 4 ill-conditioned".into(), ill_conditioned()));
    for (name, s) in &cases {
        for law in [BuiltinMimo::PseudoInverse, BuiltinMimo::ShapeConstrained] {
            // On the ill-conditioned plant the reference lives in three
            // directions, and the shape law's default ratio keeps two of
            // them; it is told to keep three.
            let params = if law == BuiltinMimo::ShapeConstrained && name.contains("ill") {
                vec![
                    ("selection".to_string(), ParamVal::Choice("count".into())),
                    ("shapes".to_string(), ParamVal::Integer(3)),
                ]
            } else {
                Vec::new()
            };
            let given: Vec<(&str, ParamVal)> = params
                .iter()
                .map(|(k, v)| (k.as_str(), v.clone()))
                .collect();
            let resolved = law.spec().resolve(&given).expect("valid");
            let mut l = law.build(&s.setup, &resolved).expect("builds");
            let run = closed_loop(l.as_mut(), s, LOOPS);
            let seed_db = worst_db(s, &run.outputs[0].drive);
            let last_db = worst_db(s, run.last());
            println!(
                "{:?} on {name}: seed {seed_db:.2} dB, after {LOOPS} loops {last_db:.4} dB, \
                 within 0.5 dB from loop {:?}, within 0.1 dB from loop {:?}",
                law,
                loops_to(&run, s, 0.5),
                loops_to(&run, s, 0.1),
            );
            assert!(
                last_db < 0.5,
                "{law:?} on {name}: {last_db} dB after {LOOPS} loops"
            );
            assert!(
                seed_db > last_db,
                "{law:?} on {name}: the loop did nothing ({seed_db} dB at the seed)"
            );
            assert_psd(&format!("{law:?} on {name}"), &run);
        }
    }
}

#[test]
fn the_ill_conditioned_plant_keeps_only_its_strong_directions() {
    let s = ill_conditioned();
    let mut law = PseudoInverseLaw::with_params(&s.setup, &[]).expect("builds");
    let run = closed_loop(&mut law, &s, 5);
    let kept = run.per_line(0, "kept");
    let cond = run.per_line(0, "condition");
    for line in BAND.0..BAND.1 {
        assert_eq!(kept[line], 3.0, "line {line}");
        assert!(
            cond[line] > 1e4 && cond[line] < 1e6,
            "line {line}: {}",
            cond[line]
        );
    }
    assert_eq!(kept[0], 0.0, "out of band reports nothing");
    // The weak direction is never driven: the drive's trace stays at the
    // level the three strong directions need, nowhere near 1e10 times it.
    let tr = |d: &HermitianStack| (BAND.0..BAND.1).map(|l| d.trace(l)).sum::<f64>();
    let ratio = tr(run.last()) / tr(&run.outputs[0].drive);
    assert!(ratio < 10.0, "the drive grew {ratio} times");
}

// -------------------------------------------------------------------------
// Law 1's cap
// -------------------------------------------------------------------------

#[test]
fn the_default_laws_per_loop_change_never_exceeds_its_cap() {
    // The estimate 10 dB off either way. Too strong: the seed drive is 10 dB
    // short and the loop climbs against the rise cap (at e = 1; the plant is a
    // tenth as sensitive as the law believes, so the loop is slow but stable).
    // Too weak: the drive is 10 dB too high and the loop falls against the fall
    // cap, the factor's diagonals clamped on the way. The plant is then ten
    // times as sensitive as the law believes, so the loop is stable only for
    // e * 10 < 2 (at e = 1 it reaches the reference and then oscillates, as any
    // linear correction would); e = 0.15 is stable and still forces the clamp,
    // which needs e * (10 - 1) > 1. (A deficit never clamps a diagonal on the
    // way up: with e at most 1 the linearized step is at most half the
    // diagonal, inside the +3.5 dB clamp.)
    let seed = 300;
    let plant = well_conditioned(seed, 6, 4, LINES);
    let reference = reachable_reference(&plant, seed, BAND);
    for (gain_db, rising, e) in [(10.0, true, 1.0), (-10.0, false, 0.15)] {
        let frf = scaled(&plant, C64::new(10f64.powf(gain_db / 20.0), 0.0));
        let s = run_on(seed, plant.clone(), frf, reference.clone(), BAND, 0.01);
        for (up, down) in [(3.5, 6.0), (1.0, 2.0)] {
            let mut law = PseudoInverseLaw::with_params(
                &s.setup,
                &[
                    ("convergence", ParamVal::Number(e)),
                    ("cap_up_db", ParamVal::Number(up)),
                    ("cap_down_db", ParamVal::Number(down)),
                ],
            )
            .expect("builds");
            let run = closed_loop(&mut law, &s, LOOPS);
            let (mut worst_up, mut worst_down, mut clamped) = (0.0f64, 0.0f64, 0.0);
            for l in 1..run.outputs.len() {
                let (a, b) = (&run.outputs[l - 1].drive, &run.outputs[l].drive);
                for line in BAND.0..BAND.1 {
                    let (da, db) = (a.diagonal(line), b.diagonal(line));
                    let top = da.iter().copied().fold(0.0f64, f64::max);
                    for (x, y) in da.iter().zip(&db) {
                        let change = 10.0 * (y / x.max(1e-6 * top)).log10();
                        worst_up = worst_up.max(change);
                        worst_down = worst_down.min(change);
                    }
                }
                clamped += run.scalar(l, "clamped");
                assert!(run.scalar(l, "correction_db") <= up.max(down) + 1e-9);
            }
            let last = worst_db(&s, run.last());
            println!(
                "estimate {gain_db:+} dB, cap +{up} / -{down} dB: largest rise {worst_up:.4} dB, \
                 largest fall {worst_down:.4} dB, {clamped} factor diagonals clamped, final \
                 error {last:.3} dB"
            );
            assert!(
                worst_up <= up + 1e-9,
                "rose {worst_up} dB against a cap of {up}"
            );
            assert!(
                -worst_down <= down + 1e-9,
                "fell {worst_down} dB against {down}"
            );
            if rising {
                assert!(
                    worst_up > 0.9 * up,
                    "the rise cap never bound: {worst_up} dB"
                );
            } else {
                assert!(
                    -worst_down > 0.9 * down,
                    "the fall cap never bound: {worst_down}"
                );
                assert!(clamped > 0.0, "no factor diagonal was ever clamped");
            }
            assert!(last < 0.5, "{last} dB after {LOOPS} loops");
        }
    }
}

// -------------------------------------------------------------------------
// Law 2
// -------------------------------------------------------------------------

#[test]
fn the_open_loop_law_settles_at_the_pseudo_inverse_and_the_projection() {
    // 6 x 4, a random reference the plant cannot reach, the FRF exact and
    // held: the drive is H+ G_ref H+^H every loop, and the response the
    // projection P G_ref P^H.
    let seed = 400;
    let plant = well_conditioned(seed, 6, 4, LINES);
    let mut s = Synthetic::with_plant(seed, plant.clone(), BAND, 0.01);
    s.frf = plant.clone();
    let mut law = OpenLoopLaw::with_params(&s.setup, &[]).expect("builds");
    let run = closed_loop(&mut law, &s, LOOPS);
    assert_psd("open loop", &run);

    let reg = Regularization::Truncated { rcond: 1e-3 };
    let (hp, _) = linalg::pinv(&plant, reg).expect("pinv");
    let want = linalg::congruence(&hp, &s.setup.reference).expect("congruence");
    let projection = linalg::projection_achievable(&plant, &s.setup.reference, reg).expect("P");
    let mut worst_drive: f64 = 0.0;
    let mut worst_resp: f64 = 0.0;
    let mut unreachable: f64 = 0.0;
    for (l, d) in run.drives().enumerate() {
        assert_eq!(
            d, &run.outputs[0].drive,
            "loop {l}: the open-loop drive moved"
        );
        assert_eq!(run.scalar(l, "correction_db"), 0.0);
    }
    let y = congruence(&plant, run.last());
    for line in BAND.0..BAND.1 {
        worst_drive = worst_drive.max(line_rel(run.last(), &want, line));
        worst_resp = worst_resp.max(line_rel(&y, &projection, line));
        unreachable = unreachable.max(line_rel(&projection, &s.setup.reference, line));
    }
    println!(
        "open loop 6 x 4: drive against H+ G H+^H {worst_drive:.2e}, response against \
         P G P^H {worst_resp:.2e}, reference against its projection {unreachable:.2}"
    );
    assert!(worst_drive < 1e-12, "{worst_drive}");
    assert!(worst_resp < 1e-9, "{worst_resp}");
    assert!(
        unreachable > 0.1,
        "the reference was reachable after all: {unreachable}"
    );
}

#[test]
fn the_open_loop_law_shows_the_plants_error_and_the_closed_loop_removes_it() {
    // The estimate is off by 3 dB: open loop the response stays 3 dB off;
    // closed loop (law 1) it converges.
    let seed = 410;
    let plant = well_conditioned(seed, 3, 2, LINES);
    let reference = reachable_reference(&plant, seed, BAND);
    let frf = scaled(&plant, C64::new(10f64.powf(3.0 / 20.0), 0.0));
    let s = run_on(seed, plant, frf, reference, BAND, 0.01);
    let mut open = OpenLoopLaw::with_params(&s.setup, &[]).expect("builds");
    let open_run = closed_loop(&mut open, &s, LOOPS);
    let mut closed = PseudoInverseLaw::with_params(&s.setup, &[]).expect("builds");
    let closed_run = closed_loop(&mut closed, &s, LOOPS);
    let (o, c) = (
        worst_db(&s, open_run.last()),
        worst_db(&s, closed_run.last()),
    );
    println!("3 dB plant error: open loop {o:.3} dB, closed loop {c:.4} dB after {LOOPS} loops");
    assert!((o - 3.0).abs() < 1e-6, "{o}");
    assert!(c < 0.5, "{c}");
}

// -------------------------------------------------------------------------
// Law 3
// -------------------------------------------------------------------------

#[test]
fn the_shape_constrained_law_keeps_exactly_m_directions_and_lowers_the_drive() {
    let seed = 500;
    let plant = plant_with_singular_values(seed, 6, 4, LINES, &[3.0, 1.5, 0.4, 0.1]);
    let s = Synthetic::with_plant(seed, plant.clone(), BAND, 0.0);
    for m in [1i64, 2, 3] {
        let mut shape = ShapeConstrainedLaw::with_params(
            &s.setup,
            &[
                ("selection", ParamVal::Choice("count".into())),
                ("shapes", ParamVal::Integer(m)),
            ],
        )
        .expect("builds");
        let run = closed_loop(&mut shape, &s, 5);
        assert_psd("shape", &run);
        let mut open = OpenLoopLaw::with_params(
            &s.setup,
            &[("regularization", ParamVal::Choice("none".into()))],
        )
        .expect("builds");
        let free = closed_loop(&mut open, &s, 0);
        let kept = run.per_line(5, "kept");
        for d in run.drives() {
            let e = linalg::eigh(d).expect("eigh");
            for line in BAND.0..BAND.1 {
                let v = &e.values[line * 4..(line + 1) * 4];
                let max = v[3];
                let rank = v.iter().filter(|x| **x > 1e-9 * max).count();
                assert_eq!(rank, m as usize, "line {line}: eigenvalues {v:?}");
            }
        }
        // Against the unconstrained pseudo-inverse drive, seed for seed.
        let seed_c = &run.outputs[0].drive;
        let (mut trace_c, mut trace_f) = (0.0, 0.0);
        for line in BAND.0..BAND.1 {
            assert_eq!(kept[line], m as f64);
            let (tc, tf) = (seed_c.trace(line), free.last().trace(line));
            assert!(tc < tf, "line {line}: constrained {tc} against free {tf}");
            trace_c += tc;
            trace_f += tf;
        }
        println!(
            "shape-constrained, m = {m}: drive trace {:.2} dB below the unconstrained inverse",
            10.0 * (trace_f / trace_c).log10()
        );
    }
}

#[test]
fn the_shape_constrained_law_converges_inside_its_constraint() {
    // A reference in the plant's two leading directions, the estimate off by a
    // 2 dB, 20 degree gain: with m = 2 the closed loop reaches it.
    let seed = 510;
    let plant = plant_with_singular_values(seed, 6, 4, LINES, &[3.0, 1.5, 0.4, 0.1]);
    let svd = linalg::svd(&plant).expect("svd");
    let mut rng = Rng::new(511);
    let mut reference = HermitianStack::zeros(LINES, 6);
    for line in BAND.0..BAND.1 {
        let x = random_psd(&mut rng, 2, 1e-2);
        let u = svd.u.line(line);
        for r in 0..6 {
            for c in r..6 {
                let mut v = C64::new(0.0, 0.0);
                for a in 0..2 {
                    for b in 0..2 {
                        v += u[r * 4 + a] * x[a * 2 + b] * u[c * 4 + b].conj();
                    }
                }
                reference.set(line, r, c, v);
            }
        }
    }
    let gain = C64::from_polar(10f64.powf(2.0 / 20.0), 20f64.to_radians());
    let s = run_on(
        seed,
        plant.clone(),
        scaled(&plant, gain),
        reference,
        BAND,
        0.01,
    );
    let mut law = ShapeConstrainedLaw::with_params(
        &s.setup,
        &[
            ("selection", ParamVal::Choice("count".into())),
            ("shapes", ParamVal::Integer(2)),
        ],
    )
    .expect("builds");
    let run = closed_loop(&mut law, &s, LOOPS);
    let (first, last) = (
        worst_db(&s, &run.outputs[0].drive),
        worst_db(&s, run.last()),
    );
    println!(
        "shape-constrained m = 2 inside its constraint: seed {first:.2} dB, last {last:.4} dB"
    );
    assert!(last < 0.5, "{last}");
    assert_psd("shape m = 2", &run);
}

// -------------------------------------------------------------------------
// Law 4
// -------------------------------------------------------------------------

/// Coherence and phase of pair `(j, k)` on `line`.
fn coherence(g: &HermitianStack, line: usize, j: usize, k: usize) -> (f64, f64) {
    let c = g.get(line, j, k);
    let d = g.get(line, j, j).re * g.get(line, k, k).re;
    (c.norm_sqr() / d, c.arg())
}

#[test]
fn the_buzz_law_matches_the_identification_coherence_and_phase() {
    for (nc, seed) in [(3usize, 600u64), (6, 601)] {
        let plant = well_conditioned(seed, nc, nc, LINES);
        // A reference whose autospectra are all the law keeps of it.
        let mut s = Synthetic::with_plant(seed, plant.clone(), BAND, 0.01);
        s.frf = perturbed(&plant, seed, 0.05);
        let mut law =
            BuzzMatchedLaw::with_params(&s.setup, &[("coherence_floor", ParamVal::Number(0.0))])
                .expect("builds");
        let run = closed_loop(&mut law, &s, LOOPS);
        assert_psd("buzz", &run);
        let y = congruence(&plant, run.last());
        let (mut worst_coh, mut worst_phase, mut worst_auto) = (0.0f64, 0.0f64, 0.0f64);
        for line in BAND.0..BAND.1 {
            for j in 0..nc {
                let a = y.get(line, j, j).re / s.setup.reference.get(line, j, j).re;
                worst_auto = worst_auto.max((10.0 * a.log10()).abs());
                for k in (j + 1)..nc {
                    let (ca, pa) = coherence(&y, line, j, k);
                    let (cb, pb) = coherence(&s.sysid_response, line, j, k);
                    worst_coh = worst_coh.max((ca - cb).abs());
                    let dp = (C64::from_polar(1.0, pa) * C64::from_polar(1.0, -pb)).arg();
                    if cb > 0.05 {
                        worst_phase = worst_phase.max(dp.abs().to_degrees());
                    }
                }
            }
        }
        println!(
            "buzz {nc} x {nc}: coherence within {worst_coh:.4}, phase within \
             {worst_phase:.3} degrees, autospectra within {worst_auto:.4} dB"
        );
        assert!(worst_coh < 0.02, "{worst_coh}");
        assert!(worst_phase < 2.0, "{worst_phase}");
        assert!(worst_auto < 0.5, "{worst_auto}");
    }
}

#[test]
fn the_buzz_reference_keeps_autospectra_leaves_weak_pairs_independent_and_is_psd() {
    let seed = 610;
    let plant = well_conditioned(seed, 6, 4, LINES);
    let s = Synthetic::with_plant(seed, plant, BAND, 0.0);
    for floor in [0.0, 0.3, 0.9] {
        let g = buzz_reference(&s.setup.reference, &s.sysid_response, BAND, floor).expect("builds");
        assert!(worst_eigen_ratio(&g) >= -1e-9, "floor {floor}");
        let mut independent = 0;
        for line in BAND.0..BAND.1 {
            for j in 0..6 {
                let (a, b) = (g.get(line, j, j).re, s.setup.reference.get(line, j, j).re);
                assert!(
                    (a / b - 1.0).abs() < 1e-9,
                    "floor {floor} line {line} control {j}"
                );
                for k in (j + 1)..6 {
                    if coherence(&s.sysid_response, line, j, k).0 < floor {
                        independent += 1;
                        // Repair may give back a little, never much.
                        assert!(coherence(&g, line, j, k).0 < floor.max(0.5));
                    }
                }
            }
        }
        for j in 0..6 {
            assert_eq!(g.get(0, j, j).re, 0.0, "out of band is zero");
        }
        println!("buzz reference, floor {floor}: {independent} pairs left independent");
        if floor >= 0.9 {
            assert!(independent > 0);
        }
    }
}

// -------------------------------------------------------------------------
// Every law, every plant: positive semi-definite every loop
// -------------------------------------------------------------------------

#[test]
fn every_laws_drive_is_positive_semi_definite_every_loop() {
    let mut cases = plants();
    cases.push(("6 x 4 ill-conditioned".into(), ill_conditioned()));
    for (name, s) in &cases {
        for law in BuiltinMimo::ALL {
            let mut l = law.build(&s.setup, &[]).expect("builds");
            let run = closed_loop(l.as_mut(), s, LOOPS);
            assert_psd(&format!("{law:?} on {name}"), &run);
            for (i, d) in run.drives().enumerate() {
                for line in (0..BAND.0).chain(BAND.1..LINES) {
                    assert!(
                        d.line(line).iter().all(|v| *v == C64::new(0.0, 0.0)),
                        "{law:?} on {name}: loop {i} line {line} out of band"
                    );
                }
            }
        }
    }
}

#[test]
fn each_law_declares_what_it_reports() {
    let s = Synthetic::random(1, 3, 2, LINES);
    for law in BuiltinMimo::ALL {
        let spec = law.spec();
        let mut l = law.build(&s.setup, &[]).expect("builds");
        assert_eq!(l.needs(), spec.needs.as_slice());
        assert_eq!(l.shapes(), spec.shapes.as_slice());
        let run = closed_loop(l.as_mut(), &s, 2);
        for out in &run.outputs {
            let keys: Vec<&str> = out.diagnostics.iter().map(|d| d.key.as_str()).collect();
            let declared: Vec<&str> = spec.diagnostics.iter().map(|d| d.key).collect();
            assert_eq!(keys, declared, "{law:?}");
        }
        assert_eq!(BuiltinMimo::from_id(law.id()), Some(law));
    }
}

// -------------------------------------------------------------------------
// The FRF updated during the run
// -------------------------------------------------------------------------

#[test]
fn the_closed_loop_carries_its_correction_across_an_frf_update() {
    // The estimate changes at loop 10 (the controller's FRF update): the law
    // forms its inverse and its coordinates again and carries the factor over
    // from the drive in force, so the drive does not jump and the loop goes on
    // converging. On a 6 x 4 plant the reduced coordinates themselves rotate.
    for (nc, nd, seed) in [(4usize, 4usize, 700u64), (6, 4, 701)] {
        let plant = well_conditioned(seed, nc, nd, LINES);
        let reference = reachable_reference(&plant, seed, BAND);
        let first = perturbed(&plant, seed, 0.05);
        let second = perturbed(&plant, seed + 1, 0.05);
        let s = run_on(seed, plant, first, reference, BAND, 0.01);
        let mut s2 = Synthetic::with_plant(seed, s.plant.clone(), BAND, 0.01);
        s2.frf = second;
        s2.setup.reference = s.setup.reference.clone();
        let mut law = PseudoInverseLaw::with_params(&s.setup, &[]).expect("builds");
        let mut response = s.sysid_response.clone();
        let mut last = s.sysid_drive.clone();
        let mut outputs = Vec::new();
        for l in 0..=LOOPS as u64 {
            let run = if l < 10 { &s } else { &s2 };
            let csd = cross(&run.plant, &last);
            let inputs = run.inputs(&response, &last, &csd, l, &[]);
            let out = if l == 0 {
                law.seed(&inputs)
            } else {
                law.iterate(&inputs)
            }
            .expect("runs");
            last = out.drive.clone();
            response = run.respond(&last, l + 1);
            outputs.push(out);
        }
        let run = Run { outputs };
        assert_psd("FRF update", &run);
        let before = worst_db(&s, &run.outputs[9].drive);
        let jump = run.scalar(10, "correction_db");
        let last_db = worst_db(&s, run.last());
        // What starting over would have done: the new estimate's seed drive,
        // against the drive in force.
        let mut fresh = PseudoInverseLaw::with_params(&s2.setup, &[]).expect("builds");
        let csd = cross(&s2.plant, &s2.sysid_drive);
        let restart = fresh
            .seed(&s2.inputs(&s2.sysid_response, &s2.sysid_drive, &csd, 0, &[]))
            .expect("seeds")
            .drive;
        let mut restart_jump: f64 = 0.0;
        for line in BAND.0..BAND.1 {
            let (a, b) = (run.outputs[9].drive.diagonal(line), restart.diagonal(line));
            for (x, y) in a.iter().zip(&b) {
                restart_jump = restart_jump.max((10.0 * (y / x).log10()).abs());
            }
        }
        println!(
            "{nc} x {nd}, FRF updated at loop 10: {before:.3} dB before, a {jump:.3} dB drive \
             change at the update (starting over: {restart_jump:.3} dB), {last_db:.3} dB after \
             {LOOPS} loops"
        );
        assert!(
            jump < 0.5 * restart_jump,
            "the update did not carry the correction: {jump} dB against {restart_jump} dB"
        );
        assert!(last_db < 0.5, "{last_db} dB after the update");
    }
}
