//! The core's guards on an SRS law and on the drive, each against a law or a
//! setting made to break it: a law's output that is not usable is a miss and
//! the next pulse refires the table in force; misses in a row end the run
//! `law_fault`; the drive in volts is held to its ceiling and refused over its
//! abort; the predicted kinematics, the coherence hold, the identification's
//! volts cap, the time between pulses, the iteration limits and the level
//! move refusal.

mod common;

use common::*;
use hertzforge_shock_common::law_guard::LawFault;
use hertzforge_shock_common::ShockRunMode;
use hertzforge_srs::service::report::{
    ConvergenceLimit, EventKind, HeldReason, RunState, TripKind,
};
use hertzforge_srs::{ServiceError, SrsShockService};
use std::sync::Arc;

/// One level, 0 dB, ten pulses: a table in force refires the same drive.
fn one_level() -> hertzforge_srs::SrsTestConfig {
    let mut cfg = closed_cfg(1.0);
    cfg.schedule.table_db = vec![0.0];
    cfg.schedule.start_db = 0.0;
    cfg.schedule.pulses_per_level = vec![10];
    cfg
}

/// Run `mode` under `cfg` on a 1 g/V wire: how it ended, and what the plant
/// was handed.
fn run_adversary_with(
    cfg: hertzforge_srs::SrsTestConfig,
    mode: Mode,
) -> (SrsShockService, Vec<Vec<f64>>) {
    let p = Linear::new(1.0);
    let fired = p.fired.clone();
    let mut s = service(cfg, p).with_law_factory(Arc::new(Adversary(mode)));
    run(&mut s);
    let f = fired.lock().unwrap().clone();
    (s, f)
}

/// Run `mode` on one level.
fn run_adversary(mode: Mode) -> (SrsShockService, Vec<Vec<f64>>) {
    run_adversary_with(one_level(), mode)
}

/// Three misses in a row end the run `law_fault` with `reason`, after three
/// pulses that each played the seed's table (two refires).
fn assert_three_misses(s: &SrsShockService, fired: &[Vec<f64>], reason: &str) {
    assert_eq!(s.state(), RunState::Aborted, "trip: {:?}", s.trip());
    match &s.trip().expect("a trip").kind {
        TripKind::LawFault { fault, consecutive } => {
            assert_eq!(fault.reason(), reason, "{fault}");
            assert_eq!(*consecutive, 3);
        }
        k => panic!("wrong trip: {k:?}"),
    }
    assert_eq!(fired.len(), 4, "the identification and three pulses");
    assert_eq!(
        fired[2], fired[1],
        "the second pulse refires the table in force"
    );
    assert_eq!(fired[3], fired[1], "and so does the third");
    let rows = s.report_data().pulses;
    assert!(rows[0].law.as_ref().unwrap().used, "the seed was used");
    for r in &rows[1..] {
        let call = r.law.as_ref().unwrap();
        assert!(!call.used);
        assert_eq!(call.miss.as_ref().unwrap().reason(), reason);
    }
}

#[test]
fn a_nan_gain_is_refused_and_three_in_a_row_end_the_run() {
    let (s, f) = run_adversary(Mode::Nan);
    assert_three_misses(&s, &f, "non_finite");
}

#[test]
fn a_zero_gain_is_refused() {
    let (s, f) = run_adversary(Mode::Zero);
    assert_three_misses(&s, &f, "not_positive");
}

#[test]
fn a_negative_gain_is_refused() {
    let (s, f) = run_adversary(Mode::Negative);
    assert_three_misses(&s, &f, "not_positive");
}

#[test]
fn a_short_gains_list_is_refused() {
    let (s, f) = run_adversary(Mode::Short);
    assert_three_misses(&s, &f, "wrong_length");
}

#[test]
fn a_gain_rising_7_db_in_a_pulse_is_refused() {
    let (s, f) = run_adversary(Mode::Rise(7.0));
    assert_three_misses(&s, &f, "gain_rise");
}

#[test]
fn a_slow_law_is_over_budget_on_the_test_clock() {
    let clock = Arc::new(TestClock::default());
    let p = Linear::new(1.0);
    let fired = p.fired.clone();
    let mut s = service(one_level(), p)
        .with_law_factory(Arc::new(Adversary(Mode::Slow(Arc::clone(&clock)))))
        .with_law_clock(clock);
    run(&mut s);
    let f = fired.lock().unwrap().clone();
    assert_three_misses(&s, &f, "over_budget");
    match &s.trip().unwrap().kind {
        TripKind::LawFault {
            fault: LawFault::OverBudget { took_s, budget_s },
            ..
        } => {
            assert!((took_s - 0.3).abs() < 1e-9);
            assert!(
                (budget_s - 0.25).abs() < 1e-12,
                "a quarter of the 1 s spacing"
            );
        }
        k => panic!("{k:?}"),
    }
}

#[test]
fn a_panicking_law_is_contained() {
    let (s, f) = run_adversary(Mode::Panic);
    assert_three_misses(&s, &f, "panicked");
}

#[test]
fn a_seed_that_is_not_usable_fires_nothing() {
    let (s, f) = run_adversary(Mode::SeedNan);
    assert_eq!(s.state(), RunState::Aborted);
    match &s.trip().unwrap().kind {
        TripKind::LawFault { fault, consecutive } => {
            assert_eq!(fault.reason(), "non_finite");
            assert_eq!(*consecutive, 1);
        }
        k => panic!("{k:?}"),
    }
    assert_eq!(f.len(), 1, "only the identification fired");
}

#[test]
fn a_law_that_never_corrects_ends_not_converged_at_the_first_level() {
    let (s, f) = run_adversary_with(closed_cfg(1.0), Mode::NeverCorrects);
    assert_eq!(s.state(), RunState::NotConverged, "trip: {:?}", s.trip());
    let trip = s.trip().unwrap();
    assert_eq!(
        trip.kind,
        TripKind::NotConverged {
            limit: ConvergenceLimit::PerLevel,
            pulses: 5
        }
    );
    assert_eq!(trip.at_level_db, -6.0);
    assert_eq!(f.len(), 6, "five pulses at -6 dB, none above it");
    assert!(s
        .report_data()
        .pulses
        .iter()
        .all(|r| r.passed == Some(false)));
}

#[test]
fn the_runs_pulse_cap_ends_it_not_converged() {
    let mut cfg = closed_cfg(1.0);
    cfg.control.as_mut().unwrap().guards.max_pulses = 3;
    let p = Linear::new(1.0);
    let fired = p.fired.clone();
    let mut s = service(cfg, p);
    run(&mut s);
    assert_eq!(s.state(), RunState::NotConverged);
    assert_eq!(
        s.trip().unwrap().kind,
        TripKind::NotConverged {
            limit: ConvergenceLimit::Run,
            pulses: 3
        }
    );
    assert_eq!(fired.lock().unwrap().len(), 4);
}

/// The full-level drive the 1 g target needs on a 1 g/V wire.
fn full_level_vpk() -> f64 {
    let mut s = service(closed_cfg(1.0), Linear::new(1.0));
    identify(&mut s);
    s.proceed_after_review().unwrap();
    s.status().prediction.unwrap().drive_vpk
}

#[test]
fn a_predicted_drive_over_the_abort_ends_the_run_before_any_pulse() {
    let v0 = full_level_vpk();
    let mut cfg = closed_cfg(1.0);
    let c = cfg.control.as_mut().unwrap();
    c.drive_vpk_ceiling = 0.5 * v0;
    c.drive_vpk_abort = 0.9 * v0;
    let p = Linear::new(1.0);
    let fired = p.fired.clone();
    let mut s = service(cfg, p);
    identify(&mut s);
    s.proceed_after_review().unwrap();
    assert_eq!(s.state(), RunState::Aborted);
    match s.trip().unwrap().kind {
        TripKind::DriveVpk {
            vpk,
            abort_vpk,
            predicted,
        } => {
            assert!(predicted);
            assert!((vpk - v0).abs() < 1e-12);
            assert_eq!(abort_vpk, 0.9 * v0);
        }
        ref k => panic!("{k:?}"),
    }
    assert_eq!(fired.lock().unwrap().len(), 1, "only the identification");
    assert_eq!(s.trip().unwrap().at_pulse, 0);
}

#[test]
fn a_drive_that_grows_past_the_abort_is_refused_before_it_plays() {
    // +5 dB a pulse (under the 6 dB gain cap): the third pulse's drive would
    // pass an abort set just above the second's.
    let v0 = full_level_vpk();
    let mut cfg = one_level();
    cfg.envelope.alarm_db = (60.0, 60.0);
    cfg.envelope.abort_db = (60.0, 60.0);
    let c = cfg.control.as_mut().unwrap();
    c.drive_vpk_ceiling = v0 * 10f64.powf(5.2 / 20.0);
    c.drive_vpk_abort = c_abort(v0);
    let p = Linear::new(1.0);
    let fired = p.fired.clone();
    let mut s = service(cfg, p).with_law_factory(Arc::new(Adversary(Mode::Rise(5.0))));
    run(&mut s);
    assert_eq!(s.state(), RunState::Aborted, "trip: {:?}", s.trip());
    match s.trip().unwrap().kind {
        TripKind::DriveVpk { predicted, vpk, .. } => {
            assert!(!predicted);
            assert!(vpk > v0 * 10f64.powf(9.0 / 20.0));
        }
        ref k => panic!("{k:?}"),
    }
    let f = fired.lock().unwrap();
    assert_eq!(
        f.len(),
        3,
        "the identification and two pulses; the third never played"
    );
    assert!(peak(&f[2]) < c_abort(v0));
}

fn c_abort(v0: f64) -> f64 {
    v0 * 10f64.powf(5.5 / 20.0)
}

#[test]
fn a_drive_over_the_ceiling_is_scaled_to_it_and_logged() {
    let v0 = full_level_vpk();
    let mut cfg = closed_cfg(1.0);
    cfg.envelope.alarm_db = (60.0, 60.0);
    cfg.envelope.abort_db = (60.0, 60.0);
    let c = cfg.control.as_mut().unwrap();
    c.drive_vpk_ceiling = 0.8 * v0;
    c.drive_vpk_abort = 2.0 * v0;
    let p = Linear::new(1.0);
    let fired = p.fired.clone();
    let mut s = service(cfg, p);
    run(&mut s);
    let rows = s.report_data().pulses;
    let full: Vec<_> = rows.iter().filter(|r| r.level_db == 0.0).collect();
    assert!(!full.is_empty());
    for r in &full {
        assert!(r.limit_scale < 1.0, "{r:?}");
        assert!((r.drive_vpk.unwrap() - 0.8 * v0).abs() < 1e-9 * v0);
    }
    assert!(fired
        .lock()
        .unwrap()
        .iter()
        .all(|d| peak(d) <= 0.8 * v0 * (1.0 + 1e-12)));
    assert!(s
        .log()
        .events()
        .iter()
        .any(|e| matches!(&e.kind, EventKind::DriveScaled { reason, .. } if reason.contains("drive ceiling"))));
}

#[test]
fn the_predicted_kinematics_refuse_the_first_full_level_pulse() {
    let x0 = {
        let mut s = service(closed_cfg(1.0), Linear::new(1.0));
        identify(&mut s);
        s.proceed_after_review().unwrap();
        s.status().prediction.unwrap().displacement_pk_pk_in
    };
    let mut cfg = closed_cfg(1.0);
    cfg.kinematic_abort.disp_pp_in = Some(0.9 * x0);
    let p = Linear::new(1.0);
    let fired = p.fired.clone();
    let mut s = service(cfg, p);
    run(&mut s);
    assert_eq!(s.state(), RunState::Aborted, "trip: {:?}", s.trip());
    let trip = s.trip().unwrap();
    match &trip.kind {
        TripKind::Kinematic {
            axis,
            value,
            predicted,
            ..
        } => {
            assert!(*predicted);
            assert_eq!(axis, "displacement");
            assert!((value / x0 - 1.0).abs() < 0.05, "{value} against {x0}");
        }
        k => panic!("{k:?}"),
    }
    assert_eq!(trip.at_level_db, 0.0);
    assert_eq!(
        fired.lock().unwrap().len(),
        3,
        "two pulses at -6 dB, none at 0 dB"
    );
}

#[test]
fn low_identification_coherence_holds_the_run() {
    let p = Linear::noisy(1.0, 0.5);
    let fired = p.fired.clone();
    let mut s = service(closed_cfg(1.0), p);
    assert_eq!(identify(&mut s), RunState::Held);
    match s.status().held {
        Some(HeldReason::LowCoherence { mean, floor }) => {
            assert!(mean < floor);
            assert_eq!(floor, 0.8);
        }
        h => panic!("{h:?}"),
    }
    assert!(!s.status().identification.unwrap().passed);
    assert!(s.proceed_after_review().is_err());
    assert!(s.fire_pulse().is_err());
    assert_eq!(fired.lock().unwrap().len(), 1);
}

#[test]
fn the_identification_burst_is_held_at_its_volts_cap() {
    // A target big enough that the burst at -20 dB is over 0.02 V.
    let mut cfg = closed_cfg(4.0);
    cfg.control.as_mut().unwrap().sysid_max_vpk = Some(0.02);
    let p = Linear::new(1.0);
    let fired = p.fired.clone();
    let mut s = service(cfg, p);
    identify(&mut s);
    let burst = peak(&fired.lock().unwrap()[0]);
    assert!((burst - 0.02).abs() < 1e-12, "{burst}");
    let id = s.status().identification.unwrap();
    assert!((id.drive_vpk - 0.02).abs() < 1e-12);
    assert_eq!(id.max_vpk, 0.02);
}

#[test]
fn pulses_start_no_closer_than_the_time_between_pulses() {
    let mut s = service(closed_cfg(1.0), Linear::new(1.0));
    run(&mut s);
    let starts: Vec<f64> = s
        .report_data()
        .pulses
        .iter()
        .map(|r| r.at_s.unwrap())
        .collect();
    assert_eq!(starts.len(), 4);
    for w in starts.windows(2) {
        assert!(w[1] - w[0] >= 1.0 - 1e-12, "{starts:?}");
    }
}

#[test]
fn a_level_move_past_the_abort_is_refused() {
    let v0 = full_level_vpk();
    let mut cfg = closed_cfg(1.0);
    cfg.schedule.mode = ShockRunMode::Manual;
    cfg.schedule.start_db = 0.0;
    let c = cfg.control.as_mut().unwrap();
    c.drive_vpk_ceiling = 1.05 * v0;
    c.drive_vpk_abort = 1.08 * v0;
    let mut s = service(cfg, Linear::new(1.0));
    identify(&mut s);
    s.proceed_after_review().unwrap();
    assert_eq!(s.state(), RunState::Armed);
    let want = s.predicted_drive_vpk_at(1.0).unwrap();
    match s.step_up() {
        Err(ServiceError::DriveCeiling {
            level_db,
            vpk,
            abort_vpk,
        }) => {
            assert_eq!(level_db, 1.0);
            assert_eq!(vpk, want);
            assert_eq!(abort_vpk, 1.08 * v0);
        }
        r => panic!("{r:?}"),
    }
    assert_eq!(s.current_level_db(), 0.0, "the level did not move");
    assert_eq!(s.step_down().unwrap(), -1.0, "a move down is not refused");
}
