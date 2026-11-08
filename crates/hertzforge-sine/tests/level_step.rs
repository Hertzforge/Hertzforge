//! A manual level step at the built-in law's defaults lands within a second.
//!
//! Red first (session log 2026-11-08, Sine integration): at the defaults then
//! in force (prediction weight 0.5, slow compression) a -1 dB step had moved
//! the control point -0.84 dB 1 s after it. The prediction weight plays no
//! part in a level step (the prediction changes with frequency, not with
//! level); the servo's compression sets how fast the rest is taken.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_hal_sim::SimConfig;
use hertzforge_sine::law::servo_blend::{ServoBlendFactory, ServoBlendParams};
use hertzforge_sine::law::LawGuards;
use hertzforge_sine::service::config::{
    CaptureConfig, ColaConfig, ControlChannel, ControlSet, SineMode, SineTestConfig,
};
use hertzforge_sine::{
    CompositeMode, DwellFor, DwellPlan, FeedforwardConfig, LadderConfig, NotchGovernorConfig,
    ProfileRow, RunMode, RunState, RungFor, SineProfile, SineService,
};
use std::sync::Arc;

const FS: f64 = 16_384.0;

/// A dwell at full level on a wire (the drive read back at 1 g a volt), the
/// built-in law at its defaults, as the control service runs it: the law's
/// blend and compression written into the configuration too.
fn service() -> SineService {
    let law = ServoBlendParams::default();
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
    profile.push(ProfileRow::accel(20.0, 0.2));
    profile.push(ProfileRow::accel(500.0, 0.2));
    let cfg = SineTestConfig {
        profile,
        mode: SineMode::Dwell(DwellPlan {
            freq_hz: 250.0,
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
        blend_w: law.blend_w,
        compression: law.compression,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        feedforward: FeedforwardConfig::default(),
        notch: NotchGovernorConfig::default(),
        ladder: LadderConfig {
            sysid_level_db: -12.0,
            start_db: 0.0,
            table_db: vec![-12.0, -6.0, 0.0],
            per_rung: RungFor::Seconds(600.0),
            full_for: RungFor::Seconds(600.0),
        },
        sysid_level_db: -12.0,
        coherence_floor: 0.5,
        drive_vpk_ceiling: 0.5,
        drive_vpk_abort: 0.8,
        fault_debounce_ticks: 50,
        kinematic_monitor: false,
        capture: CaptureConfig::default(),
        cola: ColaConfig::default(),
        shaker: None,
        moving_mass: Default::default(),
        review_hold: false,
        control_tick_frames: 164,
        fs_hz: None,
        metadata: Vec::new(),
        law_guards: Some(LawGuards::default()),
        control_tick_s: Some(0.010),
    };
    let sim = SimConfig::new()
        .sample_rate_hz(FS)
        .resonances(Vec::new())
        .feedthrough(1.0)
        .noise_level(1e-4)
        .shared_clock(true)
        .max_output_channels(2)
        .seed(0x51E5_0006);
    SineService::new(
        cfg,
        table,
        Box::new(sim.build()),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        0x51E5_0006,
        None,
    )
    .unwrap()
    .with_law_factory(Arc::new(ServoBlendFactory { params: law }))
}

/// Pump `seconds` of station time and return the achieved level (dB against
/// the full-level profile) every 50 ms, with its station time.
fn pump(s: &mut SineService, seconds: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    // A Sim batch is two one-frame blocks.
    let per = (0.05 * FS / 2.0).round() as u32;
    for _ in 0..(seconds / 0.05).round() as u32 {
        assert!(s.run_batches(per).unwrap());
        let st = s.live_status();
        out.push((st.t_s, st.achieved_level_db.expect("engaged")));
    }
    out
}

#[test]
fn a_minus_one_db_step_at_the_defaults_is_within_a_tenth_of_a_db_inside_a_second() {
    let mut s = service();
    s.set_run_mode(RunMode::Manual).unwrap();
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_sysid(0).unwrap();
    assert!(s.coherence_gate().unwrap());
    assert_eq!(s.state(), RunState::Engaged);

    let settled = pump(&mut s, 6.0);
    let before: f64 = settled[settled.len() - 20..]
        .iter()
        .map(|(_, l)| l)
        .sum::<f64>()
        / 20.0;
    assert!(before.abs() < 0.1, "full level not reached: {before:.3} dB");
    let t0 = s.live_status().t_s;
    s.step_level(-1.0).unwrap();
    let after = pump(&mut s, 2.0);
    for (t, l) in after.iter().filter(|(t, _)| *t - t0 >= 1.0) {
        let moved = l - before;
        assert!(
            (moved + 1.0).abs() <= 0.10,
            "{:.2} s after the step the level had moved {moved:+.3} dB, not -1.00 +-0.10",
            t - t0
        );
    }
}
