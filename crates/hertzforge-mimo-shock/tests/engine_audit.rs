//! The engine changes an adversarial audit asked for, each against a stated
//! number fixed before the run.
//!
//! - A1 A failing level is never left: a run whose controls cannot pass ends
//!   `not_converged` with every pulse at the first level.
//! - A2 The default ladder is -12, -9, -6, -3, -1.5 and 0 dB.
//! - A3 A law's own predicted over-test (its value `predicted_over_test_db`) of
//!   10 dB is refused against the controls' abort line; three in a row end
//!   the run `law_fault` (`predicted_peak`). (The trend halving is tested in
//!   the ladder's own tests.)
//! - A4 Every control's report carries its signed over-test.
//! - A5 An SRS pulse upside down (`shock-linear-4x4`, every drive of the seed
//!   negated) fails every control on polarity; an SRS pulse with an echo 0.15 s
//!   after it at half its size fails on effective duration (over 1.2 times the
//!   target's). Changed after the run, with its reason: at 0.15 s the echo
//!   overlaps the 0.3 s wavelet composite and the first-drive bound refused the
//!   seed (its predicted peak over twice the target's); the echo is moved to
//!   0.6 s, after the composite and its tail end. The number judged is unchanged.
//! - A6 Measurement noise of 30 % of each record's rms after identification,
//!   `shock-linear-4x4`, classical, up to 12 pulses a level and 12 at full
//!   level: (a) a law whose answer grows 3 % a pulse is stopped by the
//!   divergence watch by the tenth pulse at the level it ends at; (b) the
//!   default `inverse` is not stopped by it. Both with the controls' abort
//!   line moved out of the way (20 dB), so the watch is what acts: on the
//!   first run of (a) the noise lifted a peak over the classical abort line
//!   (2.28 dB) and the measured abort ended the run first.

mod support;

use hertzforge_hal_sim::plants::ShockPlant;
use hertzforge_mimo_shock::builtin::InverseFactory;
use hertzforge_mimo_shock::{EndOutcome, HoldTiming, LadderPlan, Timing};
use std::sync::Arc;
use support::*;

#[test]
fn mimo_shock_engine_a1_a_failing_level_is_never_left() {
    let mut cfg = config(4, 0, 4, LEN_CLASSICAL);
    cfg.timing = Timing::Hold(HoldTiming {
        delays_s: vec![0.005; 4],
        window_s: 0.0005,
    });
    let (mut e, _) = ready(
        ShockPlant::Linear4x4,
        cfg,
        classical(&[20.0; 4]),
        Arc::new(InverseFactory::default()),
    );
    let end = e.run_to_end(None).unwrap();
    assert_eq!(end.outcome, EndOutcome::NotConverged, "{end:?}");
    assert!(e.pulses().iter().all(|p| p.level_db == -12.0 && !p.passed));
    assert_eq!(e.pulses().len(), 3);
}

#[test]
fn mimo_shock_engine_a2_the_default_ladder_has_a_half_step_before_full_level() {
    assert_eq!(
        LadderPlan::default().levels_db,
        vec![-12.0, -9.0, -6.0, -3.0, -1.5, 0.0]
    );
}

#[test]
fn mimo_shock_engine_a3_a_law_predicting_its_own_over_test_is_held_to_the_abort_line() {
    let (mut e, _) = ready(
        ShockPlant::Linear4x4,
        config(4, 0, 4, LEN_CLASSICAL),
        classical(&[20.0; 4]),
        Arc::new(Twisted::new(Twist::Predict(10.0))),
    );
    let end = e.run_to_end(None).unwrap();
    println!("{}", trace(&e));
    assert_eq!(
        (end.outcome, end.reason.as_str()),
        (EndOutcome::LawFault, "predicted_peak")
    );
}

#[test]
fn mimo_shock_engine_a4_every_control_reports_its_signed_over_test() {
    let (mut e, _) = ready(
        ShockPlant::Linear4x4,
        config(4, 0, 4, LEN_CLASSICAL),
        classical(&[20.0; 4]),
        Arc::new(Twisted::new(Twist::SeedTimes(0.8))),
    );
    let r = e.fire_next(None).unwrap();
    // 0.8 times the seed: about -1.9 dB at every control, signed.
    for c in &r.controls {
        assert!(c.over_test_db < -1.0 && c.over_test_db > -3.0, "{c:?}");
    }
    assert!(r.over_test_db < -1.0);
}

#[test]
fn mimo_shock_engine_a5_an_upside_down_srs_pulse_fails_on_polarity() {
    let (mut e, _) = ready(
        ShockPlant::Linear4x4,
        config(4, 0, 4, LEN_SRS),
        srs(&[20.0; 4]),
        Arc::new(Twisted::new(Twist::SeedTimes(-1.0))),
    );
    let r = e.fire_next(None).unwrap();
    println!("{}", trace(&e));
    for c in &r.controls {
        assert!(!c.polarity_ok && !c.verdict.passed, "{c:?}");
        // The SRS alone would have passed it.
        assert!(c.verdict.error_db < 3.0, "{c:?}");
    }
}

#[test]
fn mimo_shock_engine_a5_an_srs_pulse_too_long_fails_on_effective_duration() {
    let (mut e, _) = ready(
        ShockPlant::Linear4x4,
        config(4, 0, 4, LEN_SRS),
        srs(&[20.0; 4]),
        Arc::new(Twisted::new(Twist::SeedEcho(0.6, 0.5))),
    );
    let r = e.fire_next(None).unwrap();
    for c in &r.controls {
        println!(
            "{:?} {} {}",
            c.duration_ratio, c.verdict.error_db, c.energy_db
        );
        assert!(
            c.duration_ratio.unwrap() > 1.2 && !c.duration_ok && !c.verdict.passed,
            "{c:?}"
        );
    }
}

fn a6(
    law: Arc<dyn hertzforge_mimo_shock::MimoShockLawFactory>,
) -> hertzforge_mimo_shock::MimoShockEngine {
    let mut cfg = config(4, 0, 4, LEN_CLASSICAL);
    cfg.guards.control_abort_db = Some(20.0);
    cfg.ladder = LadderPlan {
        max_pulses_per_level: 12,
        full_level_pulses: 12,
        ..LadderPlan::default()
    };
    let shot = NoisyShot::new(ShockPlant::Linear4x4, 4, 0, 4, 0.3);
    let mut e = ready_on(Box::new(shot), cfg, classical(&[20.0; 4]), law);
    let end = e.run_to_end(None).unwrap();
    println!("{}", trace(&e));
    println!("{end:?}");
    e
}

#[test]
fn mimo_shock_engine_a6_a_slow_divergence_is_stopped_by_the_tenth_pulse_at_its_level() {
    let e = a6(Arc::new(Twisted::new(Twist::Grow(1.03))));
    let end = e.end().unwrap();
    assert_eq!(
        (end.outcome, end.reason.as_str()),
        (EndOutcome::SafeStop, "divergence")
    );
    let last = e.pulses().last().unwrap().level_db;
    let at = e.pulses().iter().filter(|p| p.level_db == last).count();
    assert!(at <= 10, "{at} pulses at {last} dB");
}

#[test]
fn mimo_shock_engine_a6_a_legitimate_noisy_run_is_not_stopped() {
    let e = a6(Arc::new(InverseFactory::default()));
    let end = e.end().unwrap();
    assert_ne!(end.reason, "divergence", "{end:?}");
}
