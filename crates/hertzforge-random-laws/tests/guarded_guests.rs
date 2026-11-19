//! Misbehaving law files, run as Random control laws through the catalog and
//! [`hertzforge_random_laws::WasmControlLaw`], end to end: the Random service on
//! the Sim plant, the law imported from a file, chosen by id with its
//! parameters, built by the run's law factory.
//!
//! The fixture (`crates/hertzforge-law-host/guests/misbehave`) seeds and
//! follows level changes normally, so each run engages, then misbehaves on
//! every loop in the way its `mode` names. For each: the drive the core played
//! never exceeded its ceiling and never held a non-finite value, the drive in
//! force was held through every miss, and the run ended in the core's
//! controlled stop with the law fault the misbehavior is: a trap is
//! `panicked`, a law that never returns is `over_budget`, one that grows past
//! its memory cap is `over_memory`, a NaN drive `non_finite`, a short drive
//! `wrong_length`, an undeclared diagnostic `bad_diagnostics`, and the law's own
//! error `error`.
//!
//! The spinning law is stopped by the host at the run's own call budget: the
//! same number the core measures the call against.

mod common;

use common::*;
use hertzforge_law_host::{HostConfig, LawHost, Param, ParamValue};
use hertzforge_random::service::RandomService;
use hertzforge_random::{LawFaultReason, RunState, TripKind};
use hertzforge_random_laws::{LawCatalog, LawChoice};

/// Engage a manual run on the misbehaving fixture in `mode` and tick it until
/// it ends.
fn guarded(mode: &str) -> (RandomService, Vec<Tick>, Scratch) {
    let scratch = Scratch::new(&format!("guarded-{mode}"));
    let host = LawHost::new(HostConfig::default()).expect("the host starts");
    let mut cat = LawCatalog::with_host(host, scratch.path()).expect("the catalog opens");
    let entry = cat
        .import(&law_file(MISBEHAVE))
        .expect("the fixture imports");
    let choice = LawChoice::new(
        entry.id,
        vec![Param::new("mode", ParamValue::Choice(mode.into()))],
    );
    let mut cfg = manual_cfg();
    let factory = cat.resolve(&choice, &mut cfg).expect("the choice resolves");
    let mut svc = service(cfg, factory, scratch.path(), false);
    engage(&mut svc);
    let ticks = run(&mut svc, 40);
    (svc, ticks, scratch)
}

/// The run ended in the law-fault controlled stop: `Stopped` (not `Aborted`), a
/// `LawFault` trip with `reason` after three misses in a row, the stop went the
/// ramped way, and the drive in force was held through every miss.
fn assert_law_fault_stop(svc: &RandomService, ticks: &[Tick], reason: LawFaultReason) {
    println!(
        "ended {:?} after {} ticks, trip {:?}, stats {:?}",
        svc.state(),
        ticks.len(),
        svc.trip(),
        svc.law_guard_stats()
    );
    assert_eq!(
        svc.state(),
        RunState::Stopped,
        "a law fault takes the controlled stop"
    );
    let trip = svc.trip().expect("the law fault is recorded");
    assert_eq!(
        trip.kind,
        TripKind::LawFault {
            reason,
            consecutive: 3
        }
    );
    assert!(
        svc.stop_requested_db().is_some(),
        "the stop took the shutdown ramp"
    );
    // Two settle loops, three misses, then the ramp-down.
    assert!(
        ticks.len() <= 10,
        "ended within 10 loops, took {}",
        ticks.len()
    );
    let stats = svc.law_guard_stats();
    assert_eq!(
        stats.misses, 3,
        "three misses, then the law is not called again"
    );
    let first = &ticks[0].drive;
    assert!(first.iter().any(|&p| p > 0.0), "the run was driving");
    for (i, t) in ticks.iter().take(5).enumerate() {
        assert_eq!(
            &t.drive,
            first,
            "tick {}: the drive in force changed",
            i + 1
        );
    }
}

fn log_text(svc: &RandomService) -> String {
    format!("{:?}", svc.log().events())
}

#[test]
fn a_trapping_law_file_is_stopped_under_control() {
    let (svc, ticks, _s) = guarded("trap");
    assert_law_fault_stop(&svc, &ticks, LawFaultReason::Panicked);
    assert!(
        log_text(&svc).contains("deliberate trap from the misbehaving fixture"),
        "the trap's message reaches the run log"
    );
}

#[test]
fn a_law_file_that_never_returns_is_stopped_at_the_runs_own_call_budget() {
    let (svc, ticks, _s) = guarded("spin");
    assert_law_fault_stop(&svc, &ticks, LawFaultReason::OverBudget);
    // The host's deadline is the core's budget: a quarter of the 1.5 s loop.
    let budget_ms = svc.config().law_guards.budget_fraction * 1.5e3;
    assert_eq!(budget_ms, 375.0);
    assert!(
        log_text(&svc).contains("stopped at its 375.0 ms budget"),
        "the host stopped the call at the core's budget: {}",
        log_text(&svc)
    );
}

#[test]
fn a_law_file_that_grows_past_its_memory_cap_is_stopped_under_control() {
    let (svc, ticks, _s) = guarded("grow");
    assert_law_fault_stop(&svc, &ticks, LawFaultReason::OverMemory);
}

#[test]
fn a_law_file_returning_a_nan_drive_is_held_and_stopped() {
    let (svc, ticks, _s) = guarded("nan");
    assert_law_fault_stop(&svc, &ticks, LawFaultReason::NonFinite);
}

#[test]
fn a_law_file_returning_a_short_drive_is_held_and_stopped() {
    let (svc, ticks, _s) = guarded("short");
    assert_law_fault_stop(&svc, &ticks, LawFaultReason::WrongLength);
}

#[test]
fn a_law_file_reporting_an_undeclared_diagnostic_is_held_and_stopped() {
    let (svc, ticks, _s) = guarded("undeclared");
    assert_law_fault_stop(&svc, &ticks, LawFaultReason::BadDiagnostics);
}

#[test]
fn a_law_files_own_error_is_held_and_stopped() {
    let (svc, ticks, _s) = guarded("law_error");
    assert_law_fault_stop(&svc, &ticks, LawFaultReason::Error);
    assert!(log_text(&svc).contains("deliberate error from the misbehaving fixture"));
}

#[test]
fn a_well_behaved_law_file_runs_on() {
    // The same fixture in `ok` mode: no misses, the run is still going after
    // the loops the misbehaving runs stopped within.
    let (svc, ticks, _s) = guarded_ok();
    assert_eq!(svc.law_guard_stats().misses, 0);
    assert!(svc.trip().is_none(), "no trip: {:?}", svc.trip());
    assert_eq!(ticks.len(), 12);
    assert!(matches!(
        svc.state(),
        RunState::Engaged | RunState::FullLevel
    ));
}

fn guarded_ok() -> (RandomService, Vec<Tick>, Scratch) {
    let scratch = Scratch::new("guarded-ok");
    let host = LawHost::new(HostConfig::default()).expect("the host starts");
    let mut cat = LawCatalog::with_host(host, scratch.path()).expect("the catalog opens");
    let entry = cat
        .import(&law_file(MISBEHAVE))
        .expect("the fixture imports");
    let mut cfg = manual_cfg();
    let factory = cat
        .resolve(&LawChoice::new(entry.id, Vec::new()), &mut cfg)
        .expect("the choice resolves");
    let mut svc = service(cfg, factory, scratch.path(), false);
    engage(&mut svc);
    let ticks = run(&mut svc, 12);
    (svc, ticks, scratch)
}
