//! A trip record must quote the BREACHING FRAME, not the batch end.
//!
//! The engine's raw-ring watchdog reports `AbortTrip.at_frame` - the run frame
//! its sustained breach completed on. The sine service was discarding that and
//! re-reading `step.freq_hz()` / `step.level_db()` after the batch unwound,
//! which answers a different question. The gap is a whole acquisition BLOCK:
//! `Engine::run_ticks` pushes a block to the raw ring and polls the watchdog
//! BEFORE handing that block to the process step, and on hardware a block spans
//! several control ticks.
//!
//! ## How the reference frequency is obtained without trusting the step
//!
//! The step is driven directly through [`ProcessStep::process`], so drive frame
//! `k` IS input frame `k` - no engine, no AO lookahead, no ambiguity. The drive
//! is `sin(theta_k) * scale`, and inside one control tick both the frequency and
//! the scale are constant, so three consecutive drive samples give the phase
//! increment exactly:
//!
//! ```text
//!   s[k] + s[k+2] = 2 cos(delta) * s[k+1]   =>   f = delta * fs / (2*pi)
//! ```
//!
//! That is an independent measurement of what the NCO was actually doing at a
//! frame, recovered from the waveform the step emitted rather than from any
//! accessor the fix added.

use eigen_dsp::sine::AmplitudeServo;
use eigen_dsp::sweep::SweepRate;
use eigen_dsp::tracking::BandwidthMode;
use eigen_engine::ProcessStep;
use eigen_sine::control::CompositeMode;
use eigen_sine::profile::SineProfile;
use eigen_sine::scheduler::BidirSweep;
use eigen_sine::service::config::ColaWaveform;
use eigen_sine::service::engine::{
    ColaState, LimitChannelSpec, SchedulerBuild, SineProcessStep, StepBuild,
};
use eigen_sine::{
    BidirSweepConfig, BlendConfig, BlendLaw, DriveGuard, LadderConfig, LadderPlan, RungFor,
    SweepDirection, SweepTarget,
};

/// The probe's chassis numbers: 51.2 kHz, 60-500 Hz, 60 oct/min.
const FS: f64 = 51_200.0;
const F_LO: f64 = 60.0;
const F_HI: f64 = 500.0;
/// A control tick of 10 ms, and a block that spans FOUR of them - the shape that
/// makes the watchdog's one-block lead a multi-tick lead.
const TICK_FRAMES: usize = 512;
const BLOCK_FRAMES: usize = 4 * TICK_FRAMES;

fn flat_profile(g: f64) -> SineProfile {
    let mut p = SineProfile::new("flat");
    p.push(eigen_sine::profile::ProfileRow::accel(F_LO, g));
    p.push(eigen_sine::profile::ProfileRow::accel(F_HI, g));
    p.global_alarm_db = (30.0, 30.0);
    p.global_abort_db = (60.0, 60.0);
    p
}

fn sweep_step() -> (SineProcessStep, eigen_sine::service::engine::StepHandles) {
    let servo = AmplitudeServo::new(1.0, 20.0)
        .with_gains(0.3, 2.0)
        .with_drive_ceiling(1e6)
        .with_fault_fraction(1e-3);
    let blend = BlendLaw::new(BlendConfig {
        w: 0.0,
        max_step_db_per_tick: 1.0,
    })
    .unwrap();
    let sweep = BidirSweep::new(BidirSweepConfig {
        band: (F_LO, F_HI),
        rate: SweepRate::LogOctPerMin(60.0),
        start_hz: F_LO,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(1),
    })
    .unwrap();
    let ladder = LadderPlan::new(LadderConfig {
        sysid_level_db: -20.0,
        start_db: 0.0,
        table_db: vec![0.0],
        per_rung: RungFor::Sweeps(1),
        full_for: RungFor::Seconds(60.0),
    })
    .unwrap();
    SineProcessStep::new(StepBuild {
        fs_hz: FS,
        control_tick_frames: TICK_FRAMES,
        out_channels: 1,
        profile: flat_profile(1.0),
        n_input_channels: 1,
        control_slots: vec![0],
        control_weights: vec![1.0],
        composite_mode: CompositeMode::WeightedAvg,
        tracking_bw: BandwidthMode::FixedHz(10.0),
        servo,
        blend,
        feedforward: None,
        notch: None,
        limit_channels: Vec::<LimitChannelSpec>::new(),
        scheduler: SchedulerBuild::Sweep(sweep),
        ladder,
        cola: ColaState {
            enabled: false,
            amp: 0.0,
            waveform: ColaWaveform::Sine,
        },
        drive_guard: DriveGuard::new(1e6, 1e7).unwrap(),
        kinematic_monitor: false,
        max_velocity_ips: None,
        max_displacement_in_pp: None,
        fault_debounce_ticks: 0,
        fault_fraction: 1e-2,
        settle_ticks: 4,
        scalar_abort_debounce: 3,
        start_f_hz: F_LO,
        start_level_scale: 0.3,
        run_mode: eigen_sine::RunMode::Auto,
        // No transport lag in this fixture, so the tracking
        // reference is uncompensated and the arithmetic is unchanged.
        tracking_lag_frames: 0,
        ring_blocks: 8,
        block_frames: BLOCK_FRAMES,
    })
}

/// Recover the NCO frequency the drive was generated at, from three consecutive
/// drive samples starting at `k` (0-based into the recorded drive stream).
/// Returns `None` when the middle sample is too close to a zero crossing for the
/// identity to be well conditioned.
fn nco_freq_from_drive(drive: &[f32], k: usize) -> Option<f64> {
    let (a, b, c) = (drive[k] as f64, drive[k + 1] as f64, drive[k + 2] as f64);
    if b.abs() < 1e-3 {
        return None;
    }
    let cos_d = (a + c) / (2.0 * b);
    if !(-1.0..=1.0).contains(&cos_d) {
        return None;
    }
    Some(cos_d.acos() * FS / std::f64::consts::TAU)
}

/// Median of the recoverable frequencies over a window of drive frames, so one
/// badly conditioned triple cannot decide the answer.
fn nco_freq_near(drive: &[f32], center: usize, half_width: usize) -> f64 {
    let lo = center.saturating_sub(half_width);
    let hi = (center + half_width).min(drive.len().saturating_sub(3));
    let mut v: Vec<f64> = (lo..hi)
        .filter_map(|k| nco_freq_from_drive(drive, k))
        .collect();
    assert!(
        !v.is_empty(),
        "no conditioned drive triple near frame {center}"
    );
    v.sort_by(|x, y| x.partial_cmp(y).unwrap());
    v[v.len() / 2]
}

/// THE RED TEST for quoting the breaching frame.
///
/// A raw-ring trip on a frame in the block the step has not processed yet must
/// resolve to the NCO frequency and ladder rung IN EFFECT AT THAT FRAME. The
/// value the old code recorded - the live `freq_hz()` read after the batch -
/// is a different number, and this test prints both so the size of the lie is
/// on the record.
#[test]
fn trip_context_resolves_the_breaching_frame_not_the_batch_end() {
    let (mut step, _h) = sweep_step();
    // A unity-echo response so the servo does something ordinary; the assertion
    // is about the FREQUENCY bookkeeping, not the control law.
    let mut drive_log: Vec<f32> = Vec::new();
    let mut input = vec![0.0f32; BLOCK_FRAMES];
    let mut drive_out: Vec<f32> = Vec::new();
    // 12 blocks = 48 control ticks = 0.48 s of sweep at 60 oct/min.
    for _ in 0..12 {
        step.process(&input, BLOCK_FRAMES, &mut drive_out);
        // Echo this block's drive back as next block's response (a one-block
        // plant), which is also the transport delay the tracking-lag finding is
        // about.
        input.clear();
        input.extend_from_slice(&drive_out);
        input.resize(BLOCK_FRAMES, 0.0);
        drive_log.extend_from_slice(&drive_out);
    }
    let processed = step.frames_processed();
    assert_eq!(processed, (12 * BLOCK_FRAMES) as u64);

    // Pick a breaching frame three control ticks back from the step's position -
    // i.e. inside the run, comfortably older than the block boundary, and away
    // from any tick edge so the 3-point recovery sees a constant frequency.
    let at_frame = processed - (3 * TICK_FRAMES as u64) + (TICK_FRAMES as u64 / 2);
    let ctx = step.context_at_frame(at_frame);
    // Drive index is 0-based; the frame counter is 1-based.
    let truth_hz = nco_freq_near(&drive_log, (at_frame - 1) as usize, 64);
    let batch_end_hz = step.freq_hz();

    println!("  breaching frame        : {at_frame}");
    println!("  NCO measured from drive: {truth_hz:.4} Hz");
    println!("  context_at_frame       : {:.4} Hz", ctx.freq_hz);
    println!("  batch-end freq_hz()    : {batch_end_hz:.4} Hz  (what the old record quoted)");
    println!(
        "  drive_scale {:.6}, measured {:.6}, tick {}, extrapolated {}",
        ctx.drive_scale, ctx.measured, ctx.tick, ctx.extrapolated
    );

    assert!(
        (ctx.freq_hz - truth_hz).abs() < 0.05,
        "the record must quote the breaching frame's NCO: {:.4} Hz vs the drive's own \
         {truth_hz:.4} Hz",
        ctx.freq_hz
    );
    // ...and the value the old path would have recorded is materially different.
    // If this ever stops being true the test has lost its teeth.
    assert!(
        (batch_end_hz - truth_hz).abs() > 1.0,
        "the batch-end read must be visibly wrong for this test to mean anything: \
         {batch_end_hz:.4} Hz vs {truth_hz:.4} Hz"
    );
    assert!(
        !ctx.extrapolated,
        "a frame the step has already processed is a record, not an extrapolation"
    );
}

/// A frame the step has NOT reached yet - the normal case for the raw-ring
/// watchdog, which leads the step by one block - still resolves to the last
/// committed command, and says so.
#[test]
fn a_frame_still_in_flight_is_flagged_as_extrapolated() {
    let (mut step, _h) = sweep_step();
    let input = vec![0.0f32; BLOCK_FRAMES];
    let mut drive_out: Vec<f32> = Vec::new();
    for _ in 0..4 {
        step.process(&input, BLOCK_FRAMES, &mut drive_out);
    }
    let ahead = step.frames_processed() + (BLOCK_FRAMES as u64) / 2;
    let ctx = step.context_at_frame(ahead);
    assert!(
        ctx.extrapolated,
        "a frame past the step's position must be flagged"
    );
    assert!(
        (ctx.freq_hz - step.freq_hz()).abs() < 1e-12,
        "the extrapolation is the last committed command"
    );
    // The newest snapshot is stamped with the FIRST frame it applies to, which
    // is one past the last frame processed when a block ends on a tick boundary.
    assert!(ctx.since_frame <= step.frames_processed() + 1);
}

/// The history is per control tick and MONOTONE in frame: consecutive lookups
/// walking backwards through the run return non-increasing frequencies on an
/// upward sweep, and each is stamped with the tick that committed it.
#[test]
fn the_history_walks_back_through_the_sweep() {
    let (mut step, _h) = sweep_step();
    let input = vec![0.0f32; BLOCK_FRAMES];
    let mut drive_out: Vec<f32> = Vec::new();
    for _ in 0..10 {
        step.process(&input, BLOCK_FRAMES, &mut drive_out);
    }
    let end = step.frames_processed();
    let mut prev = f64::INFINITY;
    let mut prev_tick = u64::MAX;
    let mut distinct = 0;
    for back in 0..30u64 {
        let f = end - back * TICK_FRAMES as u64;
        let ctx = step.context_at_frame(f);
        assert!(ctx.freq_hz > 0.0, "every retained frame has a frequency");
        assert!(
            ctx.freq_hz <= prev + 1e-9,
            "an upward sweep cannot read higher further back: {} then {}",
            prev,
            ctx.freq_hz
        );
        assert!(ctx.tick <= prev_tick, "tick stamps must walk back too");
        if ctx.freq_hz < prev - 1e-9 {
            distinct += 1;
        }
        prev = ctx.freq_hz;
        prev_tick = ctx.tick;
    }
    assert!(
        distinct > 20,
        "walking back a tick at a time must land on distinct ticks; got {distinct}"
    );
}
