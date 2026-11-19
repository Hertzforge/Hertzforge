//! The operator's hold stops the level clock as well as the test's: a rung
//! that lasts so many seconds does not count them while the run is held.
//!
//! Red first (session log 2026-11-08, Sine core): the hold held the frequency
//! scheduler and not the ladder, so a one-second rung advanced during a
//! two-second hold.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_hal_sim::SimConfig;
use hertzforge_sine::service::config::{
    CaptureConfig, ColaConfig, ControlChannel, ControlSet, SineMode, SineTestConfig,
};
use hertzforge_sine::{
    CompositeMode, CompressionPreset, DwellFor, DwellPlan, FeedforwardConfig, LadderConfig,
    NotchGovernorConfig, ProfileRow, RunState, RungFor, SineProfile, SineService,
};

const FS: f64 = 16_384.0;

fn service() -> SineService {
    let mut table = ChannelTable::new();
    let mut c = Channel::new(1);
    c.role = ChannelRole::Control;
    c.units = Units::AccelG;
    table.channels.push(c);
    let mut d = Channel::new(2);
    d.role = ChannelRole::Drive;
    d.units = Units::VoltageV;
    table.channels.push(d);
    let mut profile = SineProfile::new("flat");
    profile.push(ProfileRow::accel(60.0, 1.0));
    profile.push(ProfileRow::accel(500.0, 1.0));
    let cfg = SineTestConfig {
        profile,
        mode: SineMode::Dwell(DwellPlan {
            freq_hz: 180.0,
            for_: DwellFor::Seconds(600.0),
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
        compression: CompressionPreset::Fast,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        feedforward: FeedforwardConfig::default(),
        notch: NotchGovernorConfig::default(),
        ladder: LadderConfig {
            sysid_level_db: -20.0,
            start_db: -6.0,
            table_db: vec![-6.0, 0.0],
            per_rung: RungFor::Seconds(1.0),
            full_for: RungFor::Seconds(600.0),
        },
        sysid_level_db: -20.0,
        coherence_floor: 0.0,
        drive_vpk_ceiling: 1e4,
        drive_vpk_abort: 1e5,
        fault_debounce_ticks: 0,
        kinematic_monitor: false,
        capture: CaptureConfig::default(),
        cola: ColaConfig::default(),
        shaker: None,
        moving_mass: Default::default(),
        review_hold: false,
        control_tick_frames: 164,
        fs_hz: None,
        metadata: Vec::new(),
        law_guards: None,
        control_tick_s: None,
    };
    let sim = SimConfig::new()
        .sample_rate_hz(FS)
        .resonance(250.0, 20.0)
        .noise_level(1e-4)
        .feedthrough(0.35)
        .shared_clock(true)
        .max_output_channels(2)
        .seed(0x51E5_0004);
    SineService::new(
        cfg,
        table,
        Box::new(sim.build()),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        0x51E5_0004,
        None,
    )
    .unwrap()
}

fn pump(s: &mut SineService, seconds: f64) {
    // A Sim batch is two one-frame blocks.
    let mut left = (seconds * FS / 2.0).ceil() as u32;
    while left > 0 {
        let n = left.min(4096);
        assert!(s.run_batches(n).unwrap());
        left -= n;
    }
}

#[test]
fn a_held_run_does_not_count_its_rung_seconds() {
    let mut s = service();
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_sysid(4096).unwrap();
    assert!(s.coherence_gate().unwrap());
    assert_eq!(s.state(), RunState::Engaged);

    pump(&mut s, 0.3);
    s.hold().unwrap();
    pump(&mut s, 2.0);
    let held = s.live_status();
    assert!(held.held);
    assert_eq!(
        held.rung_index, 0,
        "the one-second rung advanced while held"
    );
    assert!(
        held.at_level_s < 0.35,
        "the rung clock ran while held: {} s",
        held.at_level_s
    );

    s.resume().unwrap();
    pump(&mut s, 1.0);
    let resumed = s.live_status();
    assert_eq!(resumed.rung_index, 1, "the rung finishes once resumed");
}
