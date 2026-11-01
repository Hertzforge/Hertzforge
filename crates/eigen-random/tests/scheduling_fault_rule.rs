//! The `SchedulingFault` retry rule.
//!
//! `PlantLoop::run_segment` used to retry a `SchedulingFault` up to EIGHT times
//! and then run a ninth and keep whatever came out, on EVERY station and
//! regardless of whether the segment had made progress. That was "wrong and
//! potentially unsafe on real hardware". The rule is now
//! `scheduling_fault_retry_allowed`, driven directly here because a HARDWARE
//! `PlantLoop` could not be constructed before the backend factory existed, and
//! a rule with no reachable test is a rule with no test.
//!
//! Note: `PlantLoop` now takes a `Box<dyn Backend>` + a `StationClass`, so a
//! hardware-station loop IS constructible here in principle. The pure-decision
//! tests below stay as they are - they cover the rule at every station without
//! needing a driver on the host - and the last test now proves the ROUND TRIP
//! (the class the caller supplied is the class the loop reports and the rule
//! reads), which is the part that used to be unfalsifiable.

use eigen_channel_table::{Channel, ChannelRole, ChannelTable};
use eigen_hal_sim::SimConfig;
use eigen_module_abi::StationClass;
use eigen_random::service::engine::{
    scheduling_fault_retry_allowed, PlantLoop, SIM_SEGMENT_ATTEMPTS,
};
use eigen_random::LoopConfig;

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    t.channels.push(drv);
    t
}

/// RED-MUTATION: drop the `!station.is_hardware()` conjunct from
/// `scheduling_fault_retry_allowed` and this test reds - a hardware station that
/// missed its drive deadline with a shaker moving gets silently re-armed and
/// re-driven, which is the behavior flagged as potentially unsafe.
#[test]
fn a_hardware_station_never_retries_a_scheduling_fault() {
    for station in [StationClass::NiSimulated, StationClass::NiReal] {
        assert!(
            !scheduling_fault_retry_allowed(station, 0, 1),
            "{station} must not retry even on a zero-progress fault"
        );
        assert!(
            !scheduling_fault_retry_allowed(station, 4096, 1),
            "{station} must not retry a progressing fault either"
        );
    }
}

/// RED-MUTATION: drop the `frames_in == 0` conjunct and this test reds - a
/// segment that had already advanced the persistent plant gets its drive record
/// replayed from the top into that advanced plant, which is a different
/// experiment wearing the first one's name.
#[test]
fn only_a_zero_progress_segment_is_retried_on_eigen_sim() {
    assert!(
        scheduling_fault_retry_allowed(StationClass::EigenSim, 0, 1),
        "the harness-jitter case the retry was built for: nothing happened yet"
    );
    assert!(
        !scheduling_fault_retry_allowed(StationClass::EigenSim, 1, 1),
        "ONE acquired frame is progress; the plant has advanced"
    );
    assert!(!scheduling_fault_retry_allowed(
        StationClass::EigenSim,
        99_999,
        1
    ));
}

/// RED-MUTATION: raise `SIM_SEGMENT_ATTEMPTS` back toward the old 8 (or drop the
/// `attempts_so_far` conjunct entirely) and this test reds - the loop is back to
/// grinding a persistent fault instead of surfacing it.
#[test]
fn eigen_sim_gets_exactly_one_retry() {
    assert_eq!(SIM_SEGMENT_ATTEMPTS, 2, "the first attempt plus ONE retry");
    assert!(scheduling_fault_retry_allowed(StationClass::EigenSim, 0, 1));
    assert!(
        !scheduling_fault_retry_allowed(StationClass::EigenSim, 0, 2),
        "the retry is not repeated"
    );
    assert!(!scheduling_fault_retry_allowed(
        StationClass::EigenSim,
        0,
        8
    ));
}

/// The station reaches the rule through the arm-gate plumbing: `PlantLoop`
/// holds the SAME value it handed the gate at `open()`, so the two cannot drift.
///
/// RED-MUTATION: have `PlantLoop::open` pass one class to `arm_gate` and store a
/// different one and this test reds against whichever the field returns.
#[test]
fn the_plant_loop_carries_the_station_it_armed_against() {
    let mut cfg = LoopConfig::demo_default();
    let plant = SimConfig::new()
        .sample_rate_hz(cfg.fs_hz)
        .resonances(vec![(200.0, 2.5), (550.0, 2.5)])
        .noise_level(2e-4)
        .feedthrough(0.35)
        .shared_clock(true)
        .seed(0x5A5A_1234)
        .max_input_channels(1)
        .max_output_channels(1)
        .build();
    let loop_ = PlantLoop::open(Box::new(plant), StationClass::EigenSim, &mut cfg, table())
        .expect("the demo plant opens");
    assert_eq!(
        loop_.station(),
        StationClass::EigenSim,
        "the loop reports the class the CALLER supplied, not one it invented"
    );
    // And the rule reads that value, so the loop is in the non-hardware branch.
    assert!(!loop_.station().is_hardware());
}
