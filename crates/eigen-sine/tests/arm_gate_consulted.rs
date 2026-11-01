//! Sine's arm path CONSULTS the shared arm gate (`eigen_module_abi::arm_gate`).
//!
//! One of six sibling tests (a `tests/arm_gate_consulted.rs` in each module
//! crate) that together answer the predicate "every arm consults the
//! resolver". Per-crate rather than one cross-module file, because the
//! falsifying mutation is per-module: bypass the helper in ONE module's arm
//! path and exactly that module's test reds, which is what makes the suite tell
//! you WHERE the gate went missing.
//!
//! RED-MUTATION for this file: delete the `eigen_module_abi::arm_gate(...)` call
//! from `SineService::arm_inner` (in `src/service/mod.rs`) and the refusal test
//! below reds - the service would run its per-tick servo against a plant whose
//! clock domain is not proven Shared.
//!
//! ## Why the refusal lever here is the CLOCK, not a device-loop backend
//!
//! Sine arms `{HostSynthesized, HostPerTick}`, so the gate's clock precondition
//! is the refusal that reaches this path. The mock is a Sim plant built with
//! `shared_clock(false)` (the DEFAULT for `SimConfig`, which the closed-loop
//! fixtures all opt out of): an unproven clock routes through the same
//! `arm_gate` call and yields the same typed `UnsupportedReason` as the sibling
//! tests' device-loop mocks.
//!
//! This file used to say the mock had to go in one layer up because the
//! service built its own `SimBackend`. It does not any more - the service takes
//! a finished `Box<dyn Backend>`, so the plant under test is constructed here.

use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use eigen_dsp::tracking::BandwidthMode;
use eigen_hal_sim::SimConfig;
use eigen_sine::service::config::{
    CaptureConfig, ColaConfig, ControlChannel, ControlSet, SineMode, SineTestConfig,
};
use eigen_sine::{
    BidirSweepConfig, CompositeMode, CompressionPreset, FeedforwardConfig, LadderConfig,
    NotchGovernorConfig, ProfileRow, RungFor, SineProfile, SineService, SweepDirection, SweepRate,
    SweepTarget,
};

const FS: f64 = 16_384.0;

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

/// The minimal valid sweep config. Deliberately the same shape the `sim_e2e`
/// fixtures use, minus everything the arm gate does not read: this test is about
/// WHICH gate refuses, not about control quality.
fn cfg() -> SineTestConfig {
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
        coherence_floor: 0.5,
        drive_vpk_ceiling: 1e4,
        drive_vpk_abort: 1e5,
        fault_debounce_ticks: 0,
        kinematic_monitor: false,
        capture: CaptureConfig::default(),
        cola: ColaConfig::default(),
        shaker: None,
        moving_mass: Default::default(),
        // Irrelevant to this file (nothing here gets past `arm`), and OFF so it
        // stays that way if the gate ever grows a post-arm step.
        review_hold: false,
        control_tick_frames: 48,
        fs_hz: None,
        metadata: Vec::new(),
    }
}

/// `shared_clock` left at its default FALSE: the plant does not claim a proven
/// shared clock, and Sine's per-tick correction requires one.
fn plant(shared_clock: bool) -> Box<dyn eigen_hal::Backend> {
    Box::new(
        SimConfig::new()
            .sample_rate_hz(FS)
            .resonance(250.0, 20.0)
            .noise_level(1e-4)
            .feedthrough(0.35)
            .shared_clock(shared_clock)
            .max_output_channels(2)
            .seed(SYSID_SEED)
            .build(),
    )
}

/// The sysID PRNG seed. Was `SimConfig::seed` before `SimConfig` was taken out of
/// the constructor; kept at the same value so the burst is bit-identical.
const SYSID_SEED: u64 = 0x51E5_0001;

/// The earlier `SineService::new(cfg, table, sim_cfg, data_root)` shape.
fn svc(plant: Box<dyn eigen_hal::Backend>) -> SineService {
    SineService::new(
        cfg(),
        table_1ctrl(),
        plant,
        eigen_module_abi::StationClass::EigenSim,
        SYSID_SEED,
        None,
    )
    .unwrap()
}

#[test]
fn sine_arm_refuses_an_unproven_clock_with_the_gates_typed_reason() {
    let mut svc = svc(plant(false));
    svc.assess_feasibility().expect("no shaker configured");
    let text = svc
        .arm()
        .expect_err("Sine corrects per tick; that needs a proven shared clock")
        .to_string();

    // The gate's `ClockNotShared` text, reaching the service error verbatim
    // (surfacing it as a UI panel is later work; today it must at least survive).
    assert!(
        text.contains("per-tick host correction"),
        "the refusal must be the gate's, not a generic backend error: {text}"
    );
    assert!(
        text.contains("Shared"),
        "the refusal must name the precondition: {text}"
    );
    assert!(
        text.contains("drift"),
        "the refusal must say WHY it matters: {text}"
    );
    assert!(text.contains("arm refused"), "{text}");
}

/// The other half. A refusal that could not have been an acceptance proves
/// nothing about the gate, so the same service on a plant that DOES declare a
/// shared clock must arm.
#[test]
fn sine_arm_succeeds_on_a_plant_that_declares_a_shared_clock() {
    let mut svc = svc(plant(true));
    svc.assess_feasibility().expect("no shaker configured");
    svc.arm().expect("a proven shared clock satisfies the gate");
}
