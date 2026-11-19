//! A commanded retune re-opens the settle window (chassis sitting,
//! 2026-08-08): GoTo 80 Hz at full level tripped `ScalarAbort { low }` at
//! tick 930 - measured 0.0634 g against a 0.1416 g bound - because the
//! tracking filters re-acquire at the new center and the tracked amplitude
//! transiently collapses. The window must (a) hold the scalar abort through a
//! commanded move exactly as it does through a rung change, and (b) still be
//! a WINDOW: a loop that is genuinely dead after the window expires must trip.
//!
//! OBSERVED RED 2026-08-08 against the pre-fix arms (settle re-open lines
//! removed from Jog/GoTo): the retune half trips ScalarAbort low during the
//! re-acquisition transient - the chassis failure, reproduced.

use hertzforge_dsp::sine::AmplitudeServo;
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_engine::ProcessStep;
use hertzforge_sine::service::engine::{
    ColaState, Command, SchedulerBuild, SineProcessStep, StepBuild, StepHandles,
};
use hertzforge_sine::{
    BidirSweep, BidirSweepConfig, BlendConfig, BlendLaw, ColaWaveform, CompositeMode, DriveGuard,
    LadderConfig, LadderPlan, ProfileRow, RunMode, RungFor, SineProfile, SweepDirection, SweepRate,
    SweepTarget, TripKind,
};

const FS_HZ: f64 = 8_192.0;
const TICK_FRAMES: usize = 16;
/// Fixed 5 Hz tracking bandwidth: re-acquisition ~1/5 s = ~102 control ticks,
/// far past the 3-tick scalar debounce - the transient is REAL at this scale,
/// as it was on the chassis.
const SETTLE_TICKS: u32 = 400;

fn step() -> (SineProcessStep, StepHandles) {
    let mut profile = SineProfile::new("flat");
    profile.push(ProfileRow::accel(60.0, 1.0));
    profile.push(ProfileRow::accel(500.0, 1.0));
    let sweep = BidirSweep::new(BidirSweepConfig {
        band: (60.0, 500.0),
        rate: SweepRate::LogOctPerMin(0.001), // effectively parked; GoTo moves it
        start_hz: 100.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(1000),
    })
    .unwrap();
    SineProcessStep::new(StepBuild {
        fs_hz: FS_HZ,
        control_tick_frames: TICK_FRAMES,
        out_channels: 1,
        profile,
        n_input_channels: 1,
        control_slots: vec![0],
        control_weights: vec![1.0],
        composite_mode: CompositeMode::WeightedAvg,
        tracking_bw: BandwidthMode::FixedHz(5.0),
        servo: AmplitudeServo::new(1.0, 20.0).with_gains(0.3, 2.0),
        blend: BlendLaw::new(BlendConfig {
            w: 0.0,
            max_step_db_per_tick: 1.0,
        })
        .unwrap(),
        feedforward: None,
        notch: None,
        limit_channels: Vec::new(),
        scheduler: SchedulerBuild::Sweep(sweep),
        ladder: LadderPlan::new(LadderConfig {
            sysid_level_db: -20.0,
            start_db: 0.0,
            table_db: vec![0.0],
            per_rung: RungFor::Seconds(600.0),
            full_for: RungFor::Seconds(600.0),
        })
        .unwrap(),
        cola: ColaState {
            enabled: false,
            amp: 0.0,
            waveform: ColaWaveform::Sine,
        },
        drive_guard: DriveGuard::new(1e6, 1e7).unwrap(),
        kinematic_monitor: false,
        max_velocity_ips: None,
        max_displacement_in_pp: None,
        fault_debounce_ticks: 10_000, // keep this test about the SCALAR abort
        fault_fraction: 1e-2,
        settle_ticks: SETTLE_TICKS,
        scalar_abort_debounce: 3,
        start_f_hz: 100.0,
        start_level_scale: 1.0,
        run_mode: RunMode::Auto,
        // No transport lag in this fixture, so the tracking
        // reference is uncompensated and the arithmetic is unchanged.
        tracking_lag_frames: 0,
        ring_blocks: 8,
        block_frames: 64,
    })
}

/// Unity plant with a one-tick transport delay: this tick's input is last
/// tick's drive. The loop is HEALTHY - any low-side trip is the filter
/// transient, not the plant.
fn tick_feedback(step: &mut SineProcessStep, prev_drive: &mut Vec<f32>, ticks: usize) {
    let mut drive = Vec::new();
    for _ in 0..ticks {
        let input: Vec<f32> = if prev_drive.len() == TICK_FRAMES {
            prev_drive.clone()
        } else {
            vec![0.0; TICK_FRAMES]
        };
        step.process(&input, TICK_FRAMES, &mut drive);
        *prev_drive = drive.clone();
    }
}

#[test]
fn a_commanded_retune_does_not_trip_the_scalar_abort_and_a_dead_loop_still_does() {
    let (mut step, handles) = step();
    let mut prev = Vec::new();

    // Warm up well past the initial settle window on a healthy loop.
    tick_feedback(&mut step, &mut prev, SETTLE_TICKS as usize + 300);
    assert!(
        !handles.abort.is_tripped(),
        "healthy warm-up must not trip: {:?}",
        handles.abort.take_record()
    );

    // The chassis shape: a large commanded move at level. The filters
    // re-acquire (~102 ticks at 5 Hz) while the scalar debounce is 3.
    handles.mailbox.post(Command::GoTo(450.0));
    tick_feedback(&mut step, &mut prev, SETTLE_TICKS as usize + 300);
    assert!(
        !handles.abort.is_tripped(),
        "the re-acquisition transient after a commanded retune must ride the \
         settle window, not trip the low-side scalar abort (2026-08-08 chassis sitting): {:?}",
        handles.abort.take_record()
    );

    // The window is a WINDOW: kill the plant and the re-armed protections must
    // catch it. (Feed zeros long enough to exhaust a full settle window plus
    // the debounce.)
    let zeros = vec![0.0f32; TICK_FRAMES];
    let mut drive = Vec::new();
    let mut tripped = None;
    for _ in 0..(SETTLE_TICKS as usize + 200) {
        step.process(&zeros, TICK_FRAMES, &mut drive);
        if handles.abort.is_tripped() {
            tripped = handles.abort.take_record().map(|r| r.kind);
            break;
        }
    }
    match tripped {
        Some(TripKind::ScalarAbort { ref side, .. }) if side == "low" => {}
        other => panic!(
            "a genuinely dead loop after the window must trip ScalarAbort low - the \
             re-open must not become a disable; got {other:?}"
        ),
    }
}
