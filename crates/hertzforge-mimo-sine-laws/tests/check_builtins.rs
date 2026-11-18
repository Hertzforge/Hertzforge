//! The law check run on the five built-in laws, natively: each runs every
//! case its manifest accepts on the simulator's sine structures, and each
//! verdict is the one recorded here. A built-in law's failures are findings,
//! named and kept, never hidden by easing a requirement:
//!
//! - `matrix`: `level_ladder` (1.16 dB at the first judged update after the
//!   step to -12 dB on the mildly softening structure, against 1.0 dB; under
//!   1.0 dB 0.07 s later) and `limit_channel` (the article 0.66 dB over its
//!   bound against 0.5 dB, and a control 3.46 dB over its reference within a
//!   second of a notch release, against 1.0 dB, with the backstop notch
//!   going in and out several times between 213 and 272 Hz). Reported, not
//!   scored: `moved_resonance_hardening`, 2.28 dB.
//! - `paired`: `measurement_noise` and `stepped` (it opens about 5.4 dB from
//!   the reference and is still there when the engine's settle window
//!   closes) and `level_ladder` (4.8 dB on the -20 dB rung).
//! - `steepest_descent` and `secant`: `level_ladder`, a control abort a
//!   second after the step from -20 to -12 dB: the drive does not follow the
//!   scheduled level.
//! - `ratio`: none.

use hertzforge_mimo_sine::laws::builtin;
use hertzforge_mimo_sine_laws::check::{check_law, sim_station, CaseResult, MimoSineCheckCase};
use std::time::Instant;

pub fn print(id: &str, results: &[CaseResult]) {
    for r in results {
        let v = &r.verdict;
        let (ident, build, run) = r
            .measure
            .as_ref()
            .map_or((0.0, 0.0, 0.0), |m| (m.ident_s, m.build_s, m.run_s));
        println!(
            "{id:17} {:27} {:5} {:18} {} [ident {ident:.2} s, build {build:.2} s, run {run:.2} s]",
            v.case.name(),
            if v.passed { "pass" } else { "FAIL" },
            v.failed.as_deref().unwrap_or(""),
            v.detail
        );
    }
}

fn run(id: &str) -> Vec<CaseResult> {
    let t = Instant::now();
    let r = check_law(builtin(id).expect("a built-in"), sim_station);
    print(id, &r);
    println!("{id}: the check took {:.1} s", t.elapsed().as_secs_f64());
    r
}

/// The run took `id` through every case its manifest accepts, and the cases
/// in `failed` failed by the requirement named; every other case passed.
fn expect(id: &str, failed: &[(&str, &str)]) {
    let r = run(id);
    let got: Vec<(&str, Option<&str>)> = r
        .iter()
        .map(|c| (c.verdict.case.name(), c.verdict.failed.as_deref()))
        .collect();
    let want: Vec<(&str, Option<&str>)> = r
        .iter()
        .map(|c| {
            let name = c.verdict.case.name();
            (
                name,
                failed
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, rule)| *rule),
            )
        })
        .collect();
    assert_eq!(got, want, "{id}");
    for (n, _) in failed {
        assert!(got.iter().any(|(g, _)| g == n), "{id} did not run {n}");
    }
    let accepted = builtin(id).unwrap().manifest().test_types;
    assert_eq!(
        r.len(),
        MimoSineCheckCase::ALL
            .iter()
            .filter(|c| accepted.contains(&c.test_type()))
            .count()
    );
}

#[test]
fn mimo_sine_check_matrix_natively() {
    expect(
        "matrix",
        &[
            ("level_ladder", "each_rung_within"),
            ("limit_channel", "limit_held"),
            ("moved_resonance_hardening", "sweep_within"),
        ],
    );
}

#[test]
fn mimo_sine_check_paired_natively() {
    expect(
        "paired",
        &[
            ("measurement_noise", "within"),
            ("level_ladder", "each_rung_within"),
            ("stepped", "within"),
        ],
    );
}

#[test]
fn mimo_sine_check_ratio_natively() {
    expect("ratio", &[]);
}

#[test]
fn mimo_sine_check_steepest_descent_natively() {
    expect("steepest_descent", &[("level_ladder", "no_abort")]);
}

#[test]
fn mimo_sine_check_secant_natively() {
    expect("secant", &[("level_ladder", "no_abort")]);
}
