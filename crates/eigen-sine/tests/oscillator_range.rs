//! An out-of-domain frequency FAULTS the run; it does not panic the RT thread.
//!
//! `SweepOscillator::set_frequency` asserts `f in [0, fs/2)` and PANICS otherwise.
//! `SineProcessStep::retune` called it on EVERY control tick, from whatever the
//! scheduler had advanced to - so the panicking form was reachable by an ORDINARY
//! SWEEP whose band top had crossed `fs/2`, not just by an operator jog. A panic
//! there unwinds the RT thread while the AO task is still clocked, and the 9263
//! holds its last sample forever.
//!
//! Two things now stand between a run and that: the scheduler's band is narrowed
//! against `0.45 x fs` at adoption (`tests/rate_honesty.rs`), and `retune` uses
//! `try_set_frequency` and turns a refusal into a typed abort. This file tests the
//! SECOND one, by building a step whose scheduler carries a band the adoption
//! guard would have clamped - which is the only way to reach it, and exactly the
//! state a failure of the first guard would produce.
//!
//! RED-MUTATION: put `self.nco.set_frequency(f);` back in `retune`
//! (`src/service/engine.rs`). `an_out_of_domain_sweep_faults_instead_of_panicking`
//! reds. **Observed red 2026-08-08** - it did not merely fail, the test binary
//! PANICKED inside `process()` with "SweepOscillator: f_hz must be in [0, fs/2)",
//! which is the whole point: on the chassis that panic is an unwind past every
//! stop path in the service with the drive task still running.

use eigen_dsp::sine::AmplitudeServo;
use eigen_dsp::tracking::BandwidthMode;
use eigen_engine::ProcessStep;
use eigen_sine::service::engine::{
    ColaState, SchedulerBuild, SineProcessStep, StepBuild, StepHandles,
};
use eigen_sine::{
    BidirSweep, BidirSweepConfig, BlendConfig, BlendLaw, ColaWaveform, CompositeMode, DriveGuard,
    LadderConfig, LadderPlan, ProfileRow, RunMode, RungFor, SineProfile, SweepDirection, SweepRate,
    SweepTarget, TripKind,
};

/// Deliberately LOW so `fs/2` is inside the band below and a few control ticks'
/// worth of sweep can cross it.
const FS_HZ: f64 = 1_000.0;
/// Bounded so a regression that never faults ends the test rather than the
/// session. At 2000 Hz/s and a 16 ms control tick, 400 -> 500 Hz is ~4 ticks.
const MAX_TICKS: usize = 4_000;

/// One step over `band`, everything else identical between the two fixtures - so
/// the in-domain run is the SAME step with only the band changed, which is what
/// makes the fault attributable to the band.
fn step_over(band: (f64, f64)) -> (SineProcessStep, StepHandles) {
    let mut profile = SineProfile::new("flat");
    profile.push(ProfileRow::accel(band.0, 1.0));
    profile.push(ProfileRow::accel(band.1, 1.0));

    let sweep = BidirSweep::new(BidirSweepConfig {
        band,
        rate: SweepRate::LinearHzPerSec(2_000.0),
        start_hz: band.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(8),
    })
    .unwrap();

    SineProcessStep::new(StepBuild {
        fs_hz: FS_HZ,
        control_tick_frames: 16,
        out_channels: 1,
        profile,
        n_input_channels: 1,
        control_slots: vec![0],
        control_weights: vec![1.0],
        composite_mode: CompositeMode::WeightedAvg,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
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
        fault_debounce_ticks: 0,
        fault_fraction: 1e-2,
        // Long enough that the SCALAR protections stay suppressed for the whole
        // run: this test is about WHICH trip fires, and a settling artifact
        // tripping first would make it pass for the wrong reason.
        settle_ticks: u32::MAX,
        scalar_abort_debounce: 3,
        start_f_hz: band.0,
        start_level_scale: 0.1,
        run_mode: RunMode::Auto,
        // No transport lag in this fixture, so the tracking
        // reference is uncompensated and the arithmetic is unchanged.
        tracking_lag_frames: 0,
        ring_blocks: 8,
        block_frames: 64,
    })
}

/// Drive the step until it latches a trip or the budget runs out.
fn run_until_trip(step: &mut SineProcessStep, handles: &StepHandles) -> Option<TripKind> {
    let input = vec![0.0f32; 16];
    let mut drive = Vec::new();
    for _ in 0..MAX_TICKS {
        step.process(&input, 16, &mut drive);
        if handles.abort.is_tripped() {
            return handles.abort.take_record().map(|r| r.kind);
        }
    }
    None
}

/// RED-MUTATION test (module doc). The band's top is 900 Hz and `fs/2` is 500,
/// so an ordinary UP sweep walks straight out of the oscillator's domain.
#[test]
fn an_out_of_domain_sweep_faults_instead_of_panicking() {
    let (mut step, handles) = step_over((400.0, 900.0));
    match run_until_trip(&mut step, &handles) {
        Some(TripKind::OscillatorRange {
            requested_hz,
            fs_hz,
        }) => {
            assert!(
                requested_hz >= fs_hz / 2.0,
                "the trip must carry the frequency that was REFUSED and the rate it was \
                 refused against: {requested_hz} vs fs {fs_hz}"
            );
            assert!((fs_hz - FS_HZ).abs() < 1e-9, "fs {fs_hz}");
        }
        other => panic!(
            "an out-of-domain retune must abort with OscillatorRange, got {other:?}. A run \
             that simply kept going would have the servo chasing a target the excitation is \
             no longer at."
        ),
    }
}

/// The other half. The SAME step on a band that stays inside `fs/2` runs its
/// whole budget without tripping - without this, a step that faulted on tick one
/// for an unrelated reason would pass the test above.
#[test]
fn a_sweep_inside_the_domain_never_trips() {
    let (mut step, handles) = step_over((100.0, 400.0));
    let trip = run_until_trip(&mut step, &handles);
    assert!(trip.is_none(), "an in-domain sweep must not trip: {trip:?}");
}
