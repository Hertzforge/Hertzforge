//! The Sim-era watchdog-constant audit, for the sine module.
//!
//! `WATCHDOG_ACCEL_HEADROOM` is the one constant in the audit set that
//! MULTIPLIES an armed safety ceiling: `arm_accel_watchdog` writes
//!
//! ```text
//! ch.abort.accel_g = max_over_band(profile.abort_high) x 10^(max_rung_dB/20) x headroom
//! ```
//!
//! into the engine's channel table, so a larger headroom is a LOOSER backstop.
//! Every other constant in the audit (the sustains) can only make the watchdog
//! act later; this one can make it not act at all.
//!
//! A new file rather than an addition to `sim_e2e.rs`: that suite's own ceiling
//! test (`watchdog_arms_accel_axis_from_profile_abort_ceiling`) hard-codes
//! `let headroom = 4.0;` as a literal, which means it pins the SIM value and
//! would go green again after a wrong-but-consistent edit to both sides. These
//! assertions are about the RELATIONSHIP between the two stations, which no
//! single-literal edit can satisfy.

use eigen_module_abi::StationClass;
use eigen_sine::service::watchdog_accel_headroom;

/// EIGEN-SIM behaviour is UNCHANGED by this audit. Stated first, because "do not
/// silently change Sim" is the constraint the whole audit runs under: the Sim
/// suites are the control-law truth and a shifted ceiling there would move
/// results nobody asked to move.
#[test]
fn the_eigen_sim_headroom_is_exactly_what_it_always_was() {
    assert_eq!(
        watchdog_accel_headroom(StationClass::EigenSim),
        4.0,
        "the Sim-era value must not move; sim_e2e's ceiling test pins it too"
    );
}

/// THE audit verdict: a hardware station arms a TIGHTER coarse backstop.
///
/// The Sim value's justification (clear a bounded RSTD-probe ring-up or a
/// rung-entry overshoot without a false raw-ring trip) is just as true on
/// hardware, so this cannot be 1.0. What changes is the cost asymmetry: on the
/// Sim plant a false trip costs a re-run, so headroom is nearly free; on
/// hardware a LATE trip costs an armature.
///
/// RED-MUTATION: make `watchdog_accel_headroom` ignore its argument (return
/// `WATCHDOG_ACCEL_HEADROOM` for every station) and the whole file reds. Note
/// that `sim_e2e.rs`'s existing ceiling test stays GREEN under that mutation,
/// which is why this file has to exist.
#[test]
fn a_hardware_station_arms_a_tighter_coarse_backstop() {
    let sim = watchdog_accel_headroom(StationClass::EigenSim);
    for station in [StationClass::NiSimulated, StationClass::NiReal] {
        let hw = watchdog_accel_headroom(station);
        assert!(
            hw < sim,
            "{station}: headroom {hw} must be tighter than the Sim's {sim} - it MULTIPLIES \
             the ceiling that gets armed"
        );
        // ...and not tightened into uselessness: a headroom at or below 1 would
        // arm the raw-ring backstop AT the profile's own abort band, where a
        // bounded transient the fine per-frequency abort is meant to judge would
        // trip the coarse one first.
        assert!(
            hw > 1.0,
            "{station}: headroom {hw} at or below 1.0 makes the coarse backstop \
             fire before the fine abort can judge a bounded transient"
        );
        // The exact step is one octave (12 dB -> 6 dB). Asserted as a RATIO so a
        // literal edit on one side alone cannot satisfy it.
        assert!(
            (sim / hw - 2.0).abs() < 1e-12,
            "{station}: expected exactly half the Sim headroom, got {hw} vs {sim}"
        );
    }
}

/// Both hardware classes get the SAME headroom, and that is deliberate.
///
/// NI-SIM's signal is not a measurement, so no ceiling there protects anything
/// physical. Giving it the looser Sim value anyway would mean the arm path
/// proven before hardware arrives is not the arm path that runs after it - and
/// the whole point of the driver-simulated rig is that it exercises the real one.
#[test]
fn ni_sim_is_armed_the_same_way_ni_real_is() {
    assert_eq!(
        watchdog_accel_headroom(StationClass::NiSimulated),
        watchdog_accel_headroom(StationClass::NiReal)
    );
}
