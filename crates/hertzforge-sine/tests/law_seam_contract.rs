//! The control-law seam as a contract: the service builds its law from the
//! factory it is given after identification, accepts the core's law guards
//! and a tick in seconds (converted at the adopted rate), and ends a run whose
//! law cannot be built before anything is driven at test level.

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

#[test]
fn the_law_guards_and_a_tick_in_seconds_are_accepted_and_the_tick_converted() {
    let mut c = cfg();
    c.law_guards = Some(LawGuards::default());
    c.control_tick_s = Some(0.01);
    c.control_tick_frames = 1;
    let mut s = build(c).expect("the guards and a tick in seconds build");
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    // 10 ms at 16384 Hz is 164 frames, whatever the frame count said.
    let note = s
        .log()
        .events()
        .iter()
        .find_map(|e| match &e.kind {
            hertzforge_sine::EventKind::Note { text } if text.starts_with("control tick") => {
                Some(text.clone())
            }
            _ => None,
        })
        .expect("the conversion is logged");
    assert!(note.contains("164 frames"), "{note}");
    s.run_sysid(4096).unwrap();
    assert!(s.coherence_gate().unwrap());
    s.run_batches(8192).unwrap();
    let t = s.live_status().t_s * FS;
    assert_eq!(t.round() as u64 % 164, 0, "ticks of 164 frames: {t}");
}

#[test]
fn a_law_that_cannot_be_built_ends_the_run_before_it_drives() {
    let mut s = build(cfg())
        .expect("builds")
        .with_law_factory(Arc::new(NeverBuilds))
        .with_law_clock(Arc::new(MonotonicClock::new()));
    assert!(s.has_law_factory());
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_sysid(4096).unwrap();
    assert!(s.coherence_gate().unwrap(), "the gate itself passed");
    assert_eq!(s.state(), RunState::Aborted);
    match &s.trip().expect("a trip").kind {
        hertzforge_sine::TripKind::LawFault { fault, consecutive } => {
            assert_eq!((fault.reason(), *consecutive), ("error", 1));
        }
        other => panic!("{other:?}"),
    }
    assert!(s.drive_is_zero(), "nothing was driven");
    assert_eq!(s.live_status().t_s, 0.0, "the loop never ran");
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
