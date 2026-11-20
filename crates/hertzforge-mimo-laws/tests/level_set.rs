//! N. Hunter's level set in `nearly_independent`, the law's open form, his
//! law as his note runs it, and the first drive on exact lines the closed
//! loop cannot hold as diagonal.

#![allow(clippy::needless_range_loop)]

mod common;

use common::*;
use hertzforge_law_host::{Param, ParamValue};
use hertzforge_mimo_laws::{BuiltinMimoLaw, NearlyIndependentLaw, ParamVal};
use hertzforge_sdm::law::{DiagnosticData, MimoControlLaw, MimoLoopOutputs};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

fn open() -> (&'static str, ParamVal) {
    ("loop", ParamVal::Choice("open".into()))
}

fn level_set(on: bool) -> (&'static str, ParamVal) {
    ("level_set", ParamVal::Toggle(on))
}

fn per_line(out: &MimoLoopOutputs, key: &str) -> Vec<f64> {
    match &out
        .diagnostics
        .iter()
        .find(|d| d.key == key)
        .unwrap_or_else(|| panic!("no {key}"))
        .data
    {
        DiagnosticData::PerLine(v) => v.clone(),
        other => panic!("{key}: {other:?}"),
    }
}

fn scalar(out: &MimoLoopOutputs, key: &str) -> f64 {
    match &out
        .diagnostics
        .iter()
        .find(|d| d.key == key)
        .unwrap_or_else(|| panic!("no {key}"))
        .data
    {
        DiagnosticData::Scalar(v) => *v,
        other => panic!("{key}: {other:?}"),
    }
}

/// A random run with the estimate 5 percent off the structure.
fn run_for(seed: u64, nc: usize, nd: usize) -> Synthetic {
    let mut s = Synthetic::random(seed, nc, nd, 64);
    s.frf = perturbed(&s.plant, seed, 0.05);
    s
}

/// Seed `law` on `s` and return the outputs.
fn seed(law: &mut dyn MimoControlLaw, s: &Synthetic) -> MimoLoopOutputs {
    let needs = law.needs().to_vec();
    let csd = cross(&s.plant, &s.sysid_drive);
    law.seed(&s.inputs(&s.sysid_response, &s.sysid_drive, &csd, 0, &needs))
        .expect("seed")
}

#[test]
fn the_open_form_returns_the_same_drive_whatever_the_response() {
    for (nc, nd, seed_no) in [(4usize, 4usize, 70u64), (6, 4, 71), (3, 5, 72)] {
        let s = run_for(seed_no, nc, nd);
        let mut law = NearlyIndependentLaw::with_params(&s.setup, &[open()]).expect("law");
        let first = seed(&mut law, &s);
        let needs = law.needs().to_vec();
        let csd = cross(&s.plant, &first.drive);
        // Two different responses: the structure's response to the drive, and
        // ten times a different loop's.
        let r1 = s.respond(&first.drive, 1);
        let mut r2 = s.respond(&first.drive, 7);
        r2.diagonal_congruence(&vec![10.0; s.setup.lines * nc])
            .expect("dims");
        let second = law
            .iterate(&s.inputs(&r1, &first.drive, &csd, 1, &needs))
            .expect("loop");
        let third = law
            .iterate(&s.inputs(&r2, &second.drive, &csd, 2, &needs))
            .expect("loop");
        assert_eq!(second.drive, first.drive, "{nc} x {nd}");
        assert_eq!(third.drive, first.drive, "{nc} x {nd}");
        assert_eq!(scalar(&third, "correction_db"), 0.0);
        // The open form's drive is the closed loop's first drive, except on
        // an exact line the closed loop cannot hold as diagonal: there the
        // open form plays the diagonal drive and the closed loop its own.
        let mut closed = NearlyIndependentLaw::with_params(&s.setup, &[]).expect("law");
        let closed_first = seed(&mut closed, &s);
        let exact = per_line(&closed_first, "exact");
        let not_held = scalar(&closed_first, "exact_not_held");
        if not_held == 0.0 {
            assert_eq!(closed_first.drive, first.drive, "{nc} x {nd}");
        }
        let mut differ = 0;
        for line in 0..s.setup.lines {
            if closed_first.drive.line(line) != first.drive.line(line) {
                assert!(exact[line] > 0.0, "{nc} x {nd} line {line}");
                differ += 1;
            }
        }
        assert_eq!(differ as f64, not_held, "{nc} x {nd}");
        // The closed loop does read the response.
        let moved = closed
            .iterate(&s.inputs(&r2, &first.drive, &csd, 1, &needs))
            .expect("loop");
        assert_ne!(moved.drive, first.drive);
        for out in [&first, &second, &third] {
            assert!(worst_eigen_ratio(&out.drive) >= -1e-9, "{nc} x {nd}");
        }
    }
}

#[test]
fn in_the_open_form_the_drive_follows_the_estimate() {
    let mut s = run_for(73, 6, 4);
    let mut law = NearlyIndependentLaw::with_params(&s.setup, &[open()]).expect("law");
    let first = seed(&mut law, &s);
    // A new estimate: the drive is the one formed for it, as a fresh law
    // forms it at its seed.
    s.frf = perturbed(&s.plant, 74, 0.05);
    let needs = law.needs().to_vec();
    let csd = cross(&s.plant, &first.drive);
    let r = s.respond(&first.drive, 1);
    let next = law
        .iterate(&s.inputs(&r, &first.drive, &csd, 1, &needs))
        .expect("loop");
    let mut fresh = NearlyIndependentLaw::with_params(&s.setup, &[open()]).expect("law");
    let want = seed(&mut fresh, &s);
    assert_eq!(next.drive, want.drive);
    assert_ne!(next.drive, first.drive);
    let change = scalar(&next, "correction_db");
    println!("open form, new estimate: largest drive change {change:.3} dB");
    assert!(change > 0.0);
    assert_eq!(
        per_line(&next, "level_set_factor"),
        per_line(&want, "level_set_factor")
    );
}

#[test]
fn the_first_drive_with_the_level_set_is_the_factor_times_the_one_without() {
    for (nc, nd, seed_no) in [(4usize, 4usize, 75u64), (6, 4, 76), (3, 5, 77)] {
        let s = run_for(seed_no, nc, nd);
        let mut on = NearlyIndependentLaw::with_params(&s.setup, &[]).expect("law");
        let mut off =
            NearlyIndependentLaw::with_params(&s.setup, &[level_set(false)]).expect("law");
        let a = seed(&mut on, &s);
        let b = seed(&mut off, &s);
        let k = per_line(&a, "level_set_factor");
        let exact = per_line(&a, "exact");
        let (lo, hi) = s.setup.band;
        let mut moved = 0;
        for line in 0..s.setup.lines {
            if !(lo..hi).contains(&line) || exact[line] > 0.0 {
                assert_eq!(k[line], 1.0);
            }
            if (k[line] - 1.0).abs() > 1e-6 {
                moved += 1;
            }
            for (x, y) in a.drive.line(line).iter().zip(b.drive.line(line)) {
                assert!(
                    (x - y * k[line]).norm() <= 1e-15 * (1.0 + y.norm()),
                    "{nc} x {nd} line {line}"
                );
            }
            // The total predicted power is the total specified.
            if (lo..hi).contains(&line) && exact[line] == 0.0 {
                let y = congruence(&s.frf, &a.drive);
                let got: f64 = y.diagonal(line).iter().sum();
                let want: f64 = s.setup.reference.diagonal(line).iter().sum();
                assert!(
                    (got - want).abs() <= 1e-12 * want,
                    "{nc} x {nd} line {line}"
                );
            }
        }
        let (kmin, kmax) = k[lo..hi]
            .iter()
            .fold((f64::MAX, 0.0f64), |(a, b), v| (a.min(*v), b.max(*v)));
        println!("{nc} x {nd}: level-set factor {kmin:.4} to {kmax:.4}, {moved} lines away from 1");
        assert_eq!(scalar(&a, "level_set_guarded"), 0.0);
    }
}

#[test]
fn with_the_level_set_the_closed_loop_settles_where_it_does_without() {
    // More controls than drives (on a square structure the factor is 1 to
    // rounding: every reference is reachable and the predicted power is the
    // specified), the estimate exact: the loop's later loops are the shared
    // loop's, so after the different first drive it reaches the same drive.
    let structure = well_conditioned(78, 6, 4, 64);
    let reference = reachable_reference(&structure, 78, (8, 56));
    let s = run_on(78, structure.clone(), structure, reference, (8, 56), 0.0);
    let mut on = NearlyIndependentLaw::with_params(&s.setup, &[]).expect("law");
    let mut off = NearlyIndependentLaw::with_params(&s.setup, &[level_set(false)]).expect("law");
    let a = closed_loop(&mut on, &s, 40);
    let b = closed_loop(&mut off, &s, 40);
    let start = (8..56)
        .map(|l| line_rel(&a.outputs[0].drive, &b.outputs[0].drive, l))
        .fold(0.0f64, f64::max);
    let end = (8..56)
        .map(|l| line_rel(a.last(), b.last(), l))
        .fold(0.0f64, f64::max);
    println!("level set on against off: first drives {start:.2e} apart, after 40 loops {end:.2e}");
    assert!(start > 1e-3, "{start:e}");
    assert!(end <= 1e-9, "{end:e}");
    for d in a.drives().chain(b.drives()) {
        assert!(worst_eigen_ratio(d) >= -1e-9);
    }
}

#[test]
fn a_line_with_nothing_to_set_the_level_from_keeps_factor_one_and_is_counted() {
    // An in-band line whose reference is zero: no target to set a level to.
    let mut s = run_for(79, 3, 3);
    let line = s.setup.band.0 + 5;
    for r in 0..3 {
        for c in r..3 {
            s.setup.reference.set(line, r, c, C64::new(0.0, 0.0));
        }
    }
    let mut law = NearlyIndependentLaw::with_params(&s.setup, &[]).expect("law");
    let out = seed(&mut law, &s);
    assert_eq!(scalar(&out, "level_set_guarded"), 1.0);
    assert_eq!(per_line(&out, "level_set_factor")[line], 1.0);
    assert!(out.drive.line(line).iter().all(|v| v.is_finite()));
}

#[test]
fn drives_are_positive_semi_definite_in_both_forms_on_every_shape() {
    for (nc, nd, seed_no) in [(4usize, 4usize, 80u64), (6, 4, 81), (3, 5, 82)] {
        let s = run_for(seed_no, nc, nd);
        for form in ["closed", "open"] {
            let p = [("loop", ParamVal::Choice(form.into()))];
            let mut law = NearlyIndependentLaw::with_params(&s.setup, &p).expect("law");
            let run = closed_loop(&mut law, &s, 10);
            let worst = run.drives().map(worst_eigen_ratio).fold(f64::MAX, f64::min);
            println!("{nc} x {nd} {form}: worst eigenvalue ratio {worst:.2e}");
            assert!(worst >= -1e-9, "{nc} x {nd} {form}");
        }
    }
}

#[test]
fn the_law_file_agrees_in_the_open_form_with_the_level_set() {
    for (nc, nd, seed_no) in [(3usize, 2usize, 83u64), (6, 4, 84), (3, 5, 85)] {
        let s = run_for(seed_no, nc, nd);
        let mut native = NearlyIndependentLaw::with_params(&s.setup, &[open()]).expect("law");
        let mut file = wasm_law(
            "hertzforge_law_mimo_nearly_independent",
            &s.setup,
            &[Param::new("loop", ParamValue::Choice("open".into()))],
        );
        let p = parity(&mut native, &mut file, &s, 5);
        println!("{nc} x {nd} open: {p:?}");
        assert!(p.drive_scale > 0.0);
        assert!(p.drive <= 1e-12 && p.diagnostics <= 1e-12, "{p:?}");
    }
    // A 1 percent change of the regularization constant is seen in the open
    // form.
    let s = run_for(86, 6, 4);
    let mut native = NearlyIndependentLaw::with_params(
        &s.setup,
        &[open(), ("regularization_constant", ParamVal::Number(0.01))],
    )
    .expect("law");
    let mut file = wasm_law(
        "hertzforge_law_mimo_nearly_independent",
        &s.setup,
        &[
            Param::new("loop", ParamValue::Choice("open".into())),
            Param::new("regularization_constant", ParamValue::Number(0.0101)),
        ],
    );
    let p = parity(&mut native, &mut file, &s, 3);
    println!("open, regularization_constant 0.01 against 0.0101: {p:?}");
    assert!(p.drive > 1e-9, "{p:?}");
}

/// The spec declares the two new parameters with their defaults.
#[test]
fn the_level_set_and_the_loop_are_declared() {
    let spec = NearlyIndependentLaw::spec();
    let get = |k: &str| {
        format!(
            "{:?}",
            spec.params
                .iter()
                .find(|p| p.key == k)
                .unwrap_or_else(|| panic!("{k}"))
                .kind
        )
    };
    assert_eq!(get("level_set"), "Toggle(true)");
    assert!(
        get("loop").contains("default: \"closed\""),
        "{}",
        get("loop")
    );
    let _ = (HermitianStack::zeros(1, 1), MatrixStack::zeros(1, 1, 1));
}

/// A 4 x 3 structure, exact estimate, whose third singular value is 1e-4 of
/// 2 on every fourth line (below the loop's 1e-3 truncation) and 0.5
/// elsewhere, and a reference that independent drives meet on every line.
/// Also returns the truncated in-band lines.
fn tall_truncated() -> (Synthetic, usize) {
    let lines = 64;
    let fine = plant_with_singular_values(41, 4, 3, lines, &[2.0, 1.0, 0.5]);
    let low = plant_with_singular_values(42, 4, 3, lines, &[2.0, 1.0, 2e-4]);
    let mut h = fine.clone();
    let mut truncated = 0;
    let band = Synthetic::random(41, 4, 3, lines).setup.band;
    for line in 0..lines {
        if line % 4 == 0 {
            h.line_mut(line).copy_from_slice(low.line(line));
            if (band.0..band.1).contains(&line) {
                truncated += 1;
            }
        }
    }
    let mut rng = Rng::new(43);
    let mut g = HermitianStack::zeros(lines, 3);
    for line in band.0..band.1 {
        for d in 0..3 {
            g.set(line, d, d, C64::new(1e-2 * (0.2 + rng.uniform()), 0.0));
        }
    }
    let reference = congruence(&h, &g);
    (run_on(41, h.clone(), h, reference, band, 0.0), truncated)
}

#[test]
fn at_the_defaults_the_first_loop_keeps_the_first_drive_on_exact_lines() {
    let mut cases: Vec<(String, Synthetic, Option<usize>)> = Vec::new();
    for (nc, nd, seed) in [(3usize, 5usize, 51u64), (2, 4, 52)] {
        let mut s = Synthetic::random(seed, nc, nd, 64);
        s.eps = 0.0;
        cases.push((format!("{nc} x {nd}"), s, None));
    }
    let (s, truncated) = tall_truncated();
    cases.push(("4 x 3, truncated".into(), s, Some(truncated)));
    for (name, s, truncated) in &cases {
        let mut law = NearlyIndependentLaw::with_params(&s.setup, &[]).expect("law");
        let run = closed_loop(&mut law, s, 1);
        let exact: f64 = run.per_line(0, "exact").iter().sum();
        let not_held = run.scalar(0, "exact_not_held");
        let change = run.scalar(1, "correction_db");
        let mut worst: f64 = 0.0;
        for line in s.setup.band.0..s.setup.band.1 {
            worst = worst.max(line_rel(&run.outputs[0].drive, &run.outputs[1].drive, line));
        }
        println!(
            "{name}: {exact} exact lines, {not_held} not held, loop 0 to 1: {change:.2e} dB, \
             {worst:.2e} relative"
        );
        assert!(exact > 0.0, "{name}");
        // Every exact line of a wide structure is not held; on the tall one,
        // exactly the truncated lines.
        assert_eq!(not_held, truncated.map_or(exact, |t| t as f64), "{name}");
        assert!(change <= 1e-9, "{name}: {change:e} dB");
        assert!(worst <= 1e-12, "{name}: {worst:e}");
        // Where it is held, the first drive is the diagonal drive exactly.
        let exact_lines = run.per_line(0, "exact");
        let nd = s.setup.drives;
        let mut diagonal = 0;
        for line in s.setup.band.0..s.setup.band.1 {
            let off = (0..nd)
                .flat_map(|r| (0..nd).filter(move |c| *c != r).map(move |c| (r, c)))
                .any(|(r, c)| run.outputs[0].drive.get(line, r, c) != C64::new(0.0, 0.0));
            if exact_lines[line] > 0.0 && !off {
                diagonal += 1;
            }
        }
        assert_eq!(diagonal as f64, exact - not_held, "{name}");
        // The open form plays the diagonal drive on every exact line.
        let mut law = NearlyIndependentLaw::with_params(&s.setup, &[open()]).expect("law");
        let out = seed(&mut law, s);
        assert_eq!(scalar(&out, "exact_not_held"), 0.0, "{name}");
        for line in s.setup.band.0..s.setup.band.1 {
            if exact_lines[line] > 0.0 {
                for r in 0..nd {
                    for c in 0..nd {
                        if r != c {
                            assert_eq!(out.drive.get(line, r, c), C64::new(0.0, 0.0));
                        }
                    }
                }
            }
        }
    }
}
