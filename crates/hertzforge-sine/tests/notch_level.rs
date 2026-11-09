//! A limit channel's notch holds it at its LIMIT, the level its profile
//! states, and the limit-channel abort stays at that profile's abort edge
//! above it.
//!
//! Red first (session log 2026-11-08, Sine core): the notch held the channel
//! at the abort edge itself, the same number the abort trips on, so a
//! binding notch ended the run on the limit-channel abort as soon as the
//! response read a hair over (1e-5 of 0.755 g, 1.5 s into a dwell on a
//! resonance).

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_hal_sim::SimConfig;
use hertzforge_sine::service::config::{
    CaptureConfig, ColaConfig, ControlChannel, ControlSet, LimitProfile, SineMode, SineTestConfig,
};
use hertzforge_sine::service::engine::TickObservation;
use hertzforge_sine::{
    CompositeMode, CompressionPreset, DwellFor, DwellPlan, FeedforwardConfig, LadderConfig,
    NotchGovernorConfig, ProfileRow, RunState, RungFor, SineProfile, SineService,
};
use std::sync::{Arc, Mutex};

const FS: f64 = 16_384.0;
const LIMIT_G: f64 = 0.6;

fn flat(g: f64, alarm: f64, abort: f64) -> SineProfile {
    let mut p = SineProfile::new("flat");
    p.push(ProfileRow::accel(60.0, g));
    p.push(ProfileRow::accel(500.0, g));
    p.global_alarm_db = (alarm, alarm);
    p.global_abort_db = (abort, abort);
    p
}

#[test]
fn the_notch_holds_a_limit_channel_at_its_limit_not_at_its_abort_edge() {
    let mut table = ChannelTable::new();
    let mut c = Channel::new(1);
    c.role = ChannelRole::Control;
    c.units = Units::AccelG;
    table.channels.push(c);
    let mut d = Channel::new(2);
    d.role = ChannelRole::Drive;
    d.units = Units::VoltageV;
    table.channels.push(d);
    let mut l = Channel::new(3);
    l.role = ChannelRole::Monitor;
    l.units = Units::AccelG;
    table.channels.push(l);
    let cfg = SineTestConfig {
        // The control asks for 1 g at a dwell on the resonance, where the
        // limit channel (the Sim gives it the same response) may have 0.6 g.
        profile: flat(1.0, 3.0, 20.0),
        mode: SineMode::Dwell(DwellPlan {
            freq_hz: 250.0,
            for_: DwellFor::Seconds(4.0),
        }),
        control: ControlSet {
            channels: vec![ControlChannel {
                index: 1,
                weight: 1.0,
            }],
            mode: CompositeMode::WeightedAvg,
        },
        limit_profiles: vec![LimitProfile {
            channel_index: 3,
            profile: flat(LIMIT_G, 1.0, 2.0),
            shared_name: None,
        }],
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
            start_db: 0.0,
            table_db: vec![0.0],
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
        .seed(0x51E5_0001);
    let seen: Arc<Mutex<Vec<TickObservation>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let mut s = SineService::new(
        cfg,
        table,
        Box::new(sim.build()),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        0x51E5_0001,
        None,
    )
    .unwrap()
    .with_tick_observer(Box::new(move |o: &TickObservation| {
        sink.lock().unwrap().push(*o)
    }));
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_sysid(4096).unwrap();
    assert!(s.coherence_gate().unwrap());
    while matches!(s.state(), RunState::Engaged | RunState::FullLevel) {
        if !s.run_chunk().unwrap() {
            break;
        }
    }
    assert!(
        s.trip().is_none(),
        "the run ended on {:?}",
        s.trip().map(|t| t.kind.clone())
    );
    assert_eq!(s.state(), RunState::Stopped, "the dwell completed");
    let ticks = seen.lock().unwrap();
    let held = ticks.iter().filter(|t| t.notch_binding).count();
    assert!(
        held > ticks.len() / 2,
        "the notch held: {held} of {}",
        ticks.len()
    );
    // After the first second the limit channel (here the measured response)
    // sits at its limit: within half a dB, and never near its abort edge.
    let worst = ticks
        .iter()
        .filter(|t| t.tick > 100)
        .map(|t| 20.0 * (t.measured / LIMIT_G).log10())
        .fold(0.0f64, |m, db| m.max(db.abs()));
    assert!(
        worst < 0.5,
        "the limit channel was {worst:.2} dB from its limit"
    );
}
