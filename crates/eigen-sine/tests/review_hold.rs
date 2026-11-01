//! The two coherence parks, and the one rule that keeps them apart.
//!
//! `AwaitingReview` is reached by a gate that PASSED and is released by
//! `proceed_after_review`. `CoherenceHold` is reached by a gate that FAILED and
//! is released by nothing - the operator rebuilds the run. They look adjacent on
//! a panel (both are "the run stopped after sysID and wants me"), which is
//! exactly why one release verb serving both would be an easy, catastrophic
//! change to make: it would put a one-click bypass of a safety refusal on the
//! screen.
//!
//! RED-MUTATION for this file: add `RunState::CoherenceHold => self.engage(),`
//! ahead of the refusal arm in `proceed_after_review`'s match
//! (`src/service/mod.rs`).
//! `the_review_verb_cannot_release_a_failed_coherence_gate` reds and the other
//! five stay green. **Observed red 2026-08-08**: the mutation engaged the run
//! past a coherence gate that had failed, which is the exact defect the two
//! parks exist to keep apart.

use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use eigen_dsp::tracking::BandwidthMode;
use eigen_hal_sim::SimConfig;
use eigen_sine::service::config::{
    CaptureConfig, ColaConfig, ControlChannel, ControlSet, SineMode, SineTestConfig,
};
use eigen_sine::{
    BidirSweepConfig, CompositeMode, CompressionPreset, FeedforwardConfig, LadderConfig,
    NotchGovernorConfig, ProfileRow, RunState, RungFor, SineProfile, SineService, SweepDirection,
    SweepRate, SweepTarget,
};

const FS: f64 = 16_384.0;
const SEED: u64 = 0x51E5_0001;

fn table_1ctrl() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(ctrl);
    t.channels.push(drv);
    t
}

/// Two axes: the coherence floor and (via `svc`) the plant's noise. The FAILING
/// gate is reached by a genuinely incoherent plant, not by an impossible floor -
/// `SineTestConfig::validate` rejects a floor outside [0, 1], and a floor of
/// exactly 1.0 would be a knife-edge that could flip on an unrelated DSP change.
fn cfg(coherence_floor: f64, review_hold: bool) -> SineTestConfig {
    let mut profile = SineProfile::new("flat");
    profile.push(ProfileRow::accel(60.0, 1.0));
    profile.push(ProfileRow::accel(500.0, 1.0));
    let band = (60.0, 500.0);
    SineTestConfig {
        profile,
        mode: SineMode::Sweep(BidirSweepConfig {
            band,
            rate: SweepRate::LogOctPerMin(8.0),
            start_hz: band.0,
            initial_direction: SweepDirection::Up,
            target: SweepTarget::Sweeps(1),
        }),
        control: ControlSet {
            channels: vec![ControlChannel {
                index: 1,
                weight: 1.0,
            }],
            mode: CompositeMode::WeightedAvg,
        },
        limit_profiles: Vec::new(),
        blend_w: 1.0,
        compression: CompressionPreset::Fast,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        feedforward: FeedforwardConfig::default(),
        notch: NotchGovernorConfig::default(),
        ladder: LadderConfig {
            sysid_level_db: -20.0,
            start_db: 0.0,
            table_db: vec![0.0],
            per_rung: RungFor::Sweeps(1),
            full_for: RungFor::Sweeps(1),
        },
        sysid_level_db: -20.0,
        coherence_floor,
        drive_vpk_ceiling: 1e4,
        drive_vpk_abort: 1e5,
        fault_debounce_ticks: 0,
        kinematic_monitor: false,
        capture: CaptureConfig::default(),
        cola: ColaConfig::default(),
        shaker: None,
        moving_mass: Default::default(),
        review_hold,
        control_tick_frames: 64,
        fs_hz: None,
        metadata: Vec::new(),
    }
}

fn svc(coherence_floor: f64, review_hold: bool) -> SineService {
    svc_with(coherence_floor, review_hold, 1e-4)
}

/// `noise_level` is what makes the FAILING branch reachable honestly: the sysID
/// burst runs at -20 dB (drive scale 0.005), so a unit-amplitude noise floor
/// swamps it and the coherence estimate collapses on its own terms.
fn svc_with(coherence_floor: f64, review_hold: bool, noise_level: f64) -> SineService {
    let sim = SimConfig::new()
        .sample_rate_hz(FS)
        .resonance(250.0, 20.0)
        .noise_level(noise_level)
        .feedthrough(0.35)
        .shared_clock(true)
        .max_output_channels(2)
        .seed(SEED);
    SineService::new(
        cfg(coherence_floor, review_hold),
        table_1ctrl(),
        Box::new(sim.build()),
        eigen_module_abi::StationClass::EigenSim,
        SEED,
        None,
    )
    .unwrap()
}

/// Arm + sysID + gate, returning the service parked wherever the gate left it.
fn to_gate(mut s: SineService) -> SineService {
    s.assess_feasibility().expect("no shaker configured");
    s.arm().expect("the Sim declares a shared clock");
    s.run_sysid(4096).expect("sysID");
    let _ = s.coherence_gate();
    s
}

// =============================================================================
// The park that PASSED
// =============================================================================

#[test]
fn a_passing_gate_parks_at_awaiting_review_and_drives_nothing() {
    let mut s = to_gate(svc(0.0, true));
    assert_eq!(s.state(), RunState::AwaitingReview);
    // The park is not cosmetic: the STEP is not built, so there is no drive scale
    // and no loop to run. A park that had already built and started the loop would
    // be a label, not a hold.
    assert_eq!(s.drive_scale(), 0.0, "the park must not have built a step");
    assert!(
        s.run_chunk().is_err(),
        "run_chunk must refuse at the park; a chunk here would drive the article \
         the operator has not yet approved"
    );
}

#[test]
fn proceed_after_review_engages_from_the_park() {
    let mut s = to_gate(svc(0.0, true));
    assert_eq!(s.state(), RunState::AwaitingReview);
    s.proceed_after_review().expect("the gate passed");
    assert_eq!(s.state(), RunState::Engaged);
    assert!(s.drive_scale() > 0.0, "engaging must build the step");
}

/// The other half. A refusal that could not have been an acceptance proves
/// nothing about the hold, so the same service with the hold OFF must engage
/// straight out of the gate.
#[test]
fn review_hold_off_engages_straight_out_of_the_gate() {
    let s = to_gate(svc(0.0, false));
    assert_eq!(s.state(), RunState::Engaged);
}

/// `run_auto` HONORS the park rather than releasing it. One line in `run_auto`
/// would auto-proceed, and it would silently make the hold a no-op for every
/// caller that used that door while the config flag still read `true`.
#[test]
fn run_auto_stops_at_the_park_instead_of_releasing_it() {
    let mut s = svc(0.0, true);
    let end = s.run_auto(4096, 50).expect("auto run");
    assert_eq!(end, RunState::AwaitingReview);
    assert_eq!(s.drive_scale(), 0.0);
}

// =============================================================================
// The park that FAILED - and is not releasable
// =============================================================================

/// THE RED-MUTATION TEST. See the module doc for the mutation and the observation.
#[test]
fn the_review_verb_cannot_release_a_failed_coherence_gate() {
    let mut s = to_gate(svc_with(0.9, true, 1.0));
    assert_eq!(
        s.state(),
        RunState::CoherenceHold,
        "a unit noise floor swamps the -20 dB sysID burst, so the gate genuinely fails"
    );
    let refusal = s
        .proceed_after_review()
        .expect_err("CoherenceHold is a gate FAILURE and has no release verb");
    let text = refusal.to_string();
    // TYPED, and it must say which park it is refusing and why - a bare
    // "bad state" would send the operator looking for the button that clears it.
    assert!(
        text.contains("CoherenceHold"),
        "the refusal must name the park: {text}"
    );
    assert!(
        text.contains("FAILED"),
        "the refusal must say the gate failed, not that it paused: {text}"
    );
    assert!(
        text.contains("reset") || text.contains("Reset"),
        "the refusal must name the only real exit: {text}"
    );
    // And nothing moved.
    assert_eq!(s.state(), RunState::CoherenceHold);
    assert_eq!(s.drive_scale(), 0.0);
}

#[test]
fn proceed_after_review_from_an_unrelated_state_is_a_typed_refusal() {
    let mut s = svc(0.0, true);
    let text = s
        .proceed_after_review()
        .expect_err("nothing to release from Idle")
        .to_string();
    assert!(text.contains("Idle"), "{text}");
    assert!(text.contains("AwaitingReview"), "{text}");
}
