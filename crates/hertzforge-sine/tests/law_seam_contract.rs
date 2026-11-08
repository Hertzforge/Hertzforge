//! The control-law seam as a contract: the service stores a law factory and a
//! clock without calling either, and refuses a configuration that asks for the
//! core's law guards or a tick in seconds, until the seam is built.
//!
//! The refusal matters for the same reason SRS's did: a configuration that asks
//! for the guards (the per-tick cap, the rise cap, the call budget, the miss
//! limit) must not quietly run the servo without them.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_hal_sim::SimConfig;
use hertzforge_sine::service::config::{
    CaptureConfig, ColaConfig, ControlChannel, ControlSet, SineMode, SineTestConfig,
};
use hertzforge_sine::{
    BidirSweepConfig, CompositeMode, CompressionPreset, FeedforwardConfig, LadderConfig,
    LawDescriptor, LawFault, LawGuards, MonotonicClock, NotchGovernorConfig, ProfileRow, RunState,
    RungFor, ServiceError, SineControlLaw, SineLawFactory, SineLawSetup, SineProfile, SineService,
    SweepDirection, SweepRate, SweepTarget,
};
use std::sync::Arc;

const FS: f64 = 16_384.0;
const SYSID_SEED: u64 = 0x51E5_0001;

fn table() -> ChannelTable {
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
        blend_w: 0.5,
        compression: CompressionPreset::Slow,
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
        review_hold: false,
        control_tick_frames: 48,
        fs_hz: None,
        metadata: Vec::new(),
        law_guards: None,
        control_tick_s: None,
    }
}

fn plant() -> Box<dyn hertzforge_hal::Backend> {
    Box::new(
        SimConfig::new()
            .sample_rate_hz(FS)
            .resonance(250.0, 20.0)
            .noise_level(1e-4)
            .feedthrough(0.35)
            .shared_clock(true)
            .max_output_channels(1)
            .seed(SYSID_SEED)
            .build(),
    )
}

fn build(cfg: SineTestConfig) -> Result<SineService, ServiceError> {
    SineService::new(
        cfg,
        table(),
        plant(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        SYSID_SEED,
        Some(std::env::temp_dir().join(format!("hertzforge-sine-seam-{}", std::process::id()))),
    )
}

/// A factory that refuses to build: enough to prove the seam's traits are
/// object safe and that the service stores one.
struct NeverBuilds;

impl SineLawFactory for NeverBuilds {
    fn build(&self, _setup: &SineLawSetup<'_>) -> Result<Box<dyn SineControlLaw + Send>, LawFault> {
        Err(LawFault::Failed("never".into()))
    }

    fn describe(&self) -> LawDescriptor {
        LawDescriptor {
            id: "never".into(),
            name: "never".into(),
            version: "0".into(),
            sha256: None,
            params: Vec::new(),
            diagnostics: Vec::new(),
            live: Vec::new(),
        }
    }
}

fn refused_as_not_built(r: Result<SineService, ServiceError>) {
    match r {
        Err(ServiceError::Config(e)) => assert!(e.to_string().contains("not built"), "{e}"),
        Err(e) => panic!("refused for the wrong reason: {e}"),
        Ok(_) => panic!("a configuration asking for the law seam was accepted"),
    }
}

#[test]
fn the_law_guards_and_a_tick_in_seconds_are_refused_until_the_seam_is_built() {
    let mut c = cfg();
    c.law_guards = Some(LawGuards::default());
    refused_as_not_built(build(c));

    let mut c = cfg();
    c.control_tick_s = Some(0.01);
    refused_as_not_built(build(c));
}

#[test]
fn guards_out_of_range_are_refused_by_the_config_check_first() {
    let mut c = cfg();
    c.law_guards = Some(LawGuards {
        max_step_db: 0.5,
        ..LawGuards::default()
    });
    let e = c
        .validate()
        .expect_err("a cap tighter than the shipped law's clamp");
    assert!(e.to_string().contains("max_step_db"), "{e}");

    let mut c = cfg();
    c.control_tick_s = Some(0.001);
    let e = c
        .validate()
        .expect_err("a tick under the built-in law's shortest");
    assert!(e.to_string().contains("control_tick_s"), "{e}");
}

#[test]
fn without_them_the_service_builds_as_before_and_stores_a_factory_and_a_clock() {
    let s = build(cfg())
        .expect("the run without the seam builds")
        .with_law_factory(Arc::new(NeverBuilds))
        .with_law_clock(Arc::new(MonotonicClock::new()));
    assert!(s.has_law_factory());
    assert_eq!(s.state(), RunState::Idle);
}

#[test]
fn a_settings_record_without_the_new_fields_reads_and_writes_unchanged() {
    // An existing record has neither field; both are left out when unset, so a
    // record written before the seam and one written after are the same text.
    let c = cfg();
    let text = serde_json::to_string(&c).expect("serializes");
    assert!(!text.contains("law_guards") && !text.contains("control_tick_s"));
    let back: SineTestConfig = serde_json::from_str(&text).expect("reads back");
    assert_eq!(back, c);
    assert_eq!(serde_json::to_string(&back).unwrap(), text);

    let mut with = cfg();
    with.law_guards = Some(LawGuards::default());
    with.control_tick_s = Some(0.01);
    let back: SineTestConfig =
        serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
    assert_eq!(back, with);
}
