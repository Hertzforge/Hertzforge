//! The ladder starts where the run engages: at the rung of its start level.
//!
//! Red first (session log 2026-11-08, Sine integration): `LadderPlan::new`
//! started at `table_db[0]` and never read `start_db`, while the service seeded
//! the engage drive at `start_db`. A run asked to start at full level engaged
//! with the drive scaled for full level and the ladder commanding the first
//! rung, so the loop pulled the level down to it.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_hal_sim::SimConfig;
use hertzforge_sine::service::config::{
    CaptureConfig, ColaConfig, ControlChannel, ControlSet, SineMode, SineTestConfig,
};
use hertzforge_sine::{
    CompositeMode, CompressionPreset, DwellFor, DwellPlan, FeedforwardConfig, LadderConfig,
    LadderPlan, NotchGovernorConfig, ProfileRow, RunMode, RunState, RungFor, SineProfile,
    SineService,
};

const FS: f64 = 16_384.0;

fn ladder(start_db: f64, table_db: Vec<f64>) -> LadderConfig {
    LadderConfig {
        sysid_level_db: -20.0,
        start_db,
        table_db,
        per_rung: RungFor::Sweeps(1),
        full_for: RungFor::Sweeps(1),
    }
}

#[test]
fn the_ladder_starts_at_the_rung_of_its_start_level() {
    let table = vec![-12.0, -6.0, 0.0];
    let l = LadderPlan::new(ladder(0.0, table.clone())).unwrap();
    assert_eq!(
        (l.rung_index(), l.level_db()),
        (2, 0.0),
        "start at full level"
    );

    let mut l = LadderPlan::new(ladder(-6.0, table.clone())).unwrap();
    assert_eq!(
        (l.rung_index(), l.level_db()),
        (1, -6.0),
        "start at the middle rung"
    );
    l.note_sweep_completed();
    assert_eq!(
        (l.rung_index(), l.level_db()),
        (2, 0.0),
        "the next rung is the one above the start"
    );

    let l = LadderPlan::new(ladder(-12.0, table)).unwrap();
    assert_eq!(
        (l.rung_index(), l.level_db()),
        (0, -12.0),
        "the first rung as before"
    );
}

#[test]
fn a_start_between_rungs_is_held_where_asked_on_the_nearest_rung() {
    // The seed is scaled for the start level, so the ladder commands it too.
    let mut l = LadderPlan::new(ladder(-10.0, vec![-12.0, -9.0, -6.0, 0.0])).unwrap();
    assert_eq!(l.rung_index(), 1, "the nearest rung is -9 dB");
    assert_eq!(l.level_db(), -10.0, "the level is the start level");
    l.note_sweep_completed();
    assert_eq!((l.rung_index(), l.level_db()), (2, -6.0));
}

#[test]
fn a_start_the_table_cannot_reach_is_refused() {
    for start in [3.0, -20.0, f64::NAN, f64::INFINITY] {
        assert!(
            LadderPlan::new(ladder(start, vec![-12.0, -6.0, 0.0])).is_err(),
            "a start at {start} dB was taken"
        );
    }
}

fn service(start_db: f64) -> SineService {
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
            start_db,
            table_db: vec![-12.0, -6.0, 0.0],
            per_rung: RungFor::Seconds(600.0),
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
        .seed(0x51E5_0005);
    SineService::new(
        cfg,
        table,
        Box::new(sim.build()),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        0x51E5_0005,
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
fn a_run_started_at_full_level_holds_full_level() {
    let mut s = service(0.0);
    s.set_run_mode(RunMode::Manual).unwrap();
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_sysid(4096).unwrap();
    assert!(s.coherence_gate().unwrap());
    assert_eq!(s.state(), RunState::Engaged);

    pump(&mut s, 3.0);
    let st = s.live_status();
    assert_eq!(st.level_db, 0.0, "the scheduled level is the start level");
    assert_eq!(st.rung_index, 2, "the ladder is on the start's rung");
    assert!(
        st.at_last_level,
        "a start at full level is at the last level"
    );
    let achieved = st.achieved_level_db.expect("a level once engaged");
    assert!(
        achieved.abs() < 0.5,
        "the response settled at {achieved:.2} dB, not at the start level"
    );
}
