//! The other four built-in laws end to end through their adapters: each at a
//! dwell, and `ratio` and `paired` at a stepped sine, on a structure each can
//! hold.
//!
//! Requirements: every run completes (no abort, so every judged update is
//! inside the 6 dB abort band), every law's declared values arrive in the
//! records through its adapter, and at a dwell the worst error after the
//! settle window is inside the 1.5 dB alarm band. Measured on this branch:
//! dwell worst `paired` 1.19 dB (at 130 Hz), `steepest_descent` 0.30,
//! `secant` 0.25, `ratio` 0.27; stepped worst `paired` 5.63, `ratio` 1.84.

mod common;

use common::*;
use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_mimo_sine::records::Outcome;
use hertzforge_mimo_sine::*;
use hertzforge_sine::scheduler::{DwellFor, StepSineConfig, StepSpacing};

fn case(s: SineStructure, id: &str, test: TestPlan, max_s: f64) -> (Option<Outcome>, f64, usize) {
    let r = run(engaged(s, setup(s, test, id), law(id), NOISE), max_s);
    let v: Vec<f64> = r
        .ticks
        .iter()
        .filter(|t| !t.settling && t.miss.is_none() && t.time_s > 0.0)
        .flat_map(|t| t.errors_db().to_vec())
        .collect();
    let worst = v.iter().fold(0.0f64, |a, e| a.max(e.abs()));
    // The law's values arrived through the adapter.
    let with_values = r.ticks.iter().filter(|t| !t.values[0].is_nan()).count();
    println!(
        "{id} on {} {:?}: {:?}, worst {worst:.2} dB, {} updates with values",
        s.name(),
        r.service.setup().test_type(),
        r.outcome,
        with_values
    );
    (r.outcome, worst, with_values)
}

fn check((o, worst, values): (Option<Outcome>, f64, usize), dwell: bool) {
    assert_eq!(o, Some(Outcome::Completed));
    assert!(values > 100, "{values}");
    assert!(worst < if dwell { 1.5 } else { 6.0 }, "{worst}");
}

fn stepped() -> TestPlan {
    TestPlan::Stepped(StepPlan::Grid(StepSineConfig {
        band: (60.0, 120.0),
        steps_per_octave: 6.0,
        spacing: StepSpacing::Log,
        dwell: Some(DwellFor::Seconds(3.0)),
    }))
}

#[test]
fn mimo_sine_paired_at_a_dwell_and_a_stepped_sine() {
    for (s, t) in [
        (SineStructure::Linear4x4, dwell(130.0, 8.0)),
        (SineStructure::Linear4x4, stepped()),
    ] {
        let dwell = matches!(t, TestPlan::Dwell(_));
        check(case(s, "paired", t, 60.0), dwell);
    }
}

#[test]
fn mimo_sine_steepest_descent_at_a_dwell() {
    check(
        case(
            SineStructure::Linear4x4,
            "steepest_descent",
            dwell(96.0, 8.0),
            12.0,
        ),
        true,
    );
}

#[test]
fn mimo_sine_secant_at_a_dwell() {
    check(
        case(SineStructure::Linear4x4, "secant", dwell(96.0, 15.0), 20.0),
        true,
    );
}

#[test]
fn mimo_sine_ratio_at_a_dwell_and_a_stepped_sine() {
    for t in [dwell(96.0, 8.0), stepped()] {
        let dwell = matches!(t, TestPlan::Dwell(_));
        check(case(SineStructure::Linear4x4, "ratio", t, 60.0), dwell);
    }
}
