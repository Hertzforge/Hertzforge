//! The control tick allocates nothing in its steady state, with the law
//! behind the seam: neither the built-in law nor the core's calls into a law
//! under its guards reach the allocator.
//!
//! The step is driven directly through `ProcessStep::process` on a loop-back
//! plant (each block's input is the previous block's drive times a gain), and
//! every allocation made on this thread while the counter is on is counted.
//!
//! Falsified once (session log 2026-11-08, Sine core): a `Vec` built inside
//! the law's tick counts one allocation per tick.

use hertzforge_dsp::sine::AmplitudeServo;
use hertzforge_dsp::sweep::SweepRate;
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_engine::ProcessStep;
use hertzforge_sine::control::CompositeMode;
use hertzforge_sine::law::servo_blend::{ServoBlendLaw, ServoBlendParams};
use hertzforge_sine::law::{LawClock, LawGuards};
use hertzforge_sine::profile::SineProfile;
use hertzforge_sine::scheduler::BidirSweep;
use hertzforge_sine::service::config::ColaWaveform;
use hertzforge_sine::service::engine::{
    ColaState, LimitChannelSpec, SchedulerBuild, SineProcessStep, StepBuild,
};
use hertzforge_sine::{
    BidirSweepConfig, BlendConfig, BlendLaw, DriveGuard, LadderConfig, LadderPlan, RungFor,
    SweepDirection, SweepTarget,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.with(|c| c.get()) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.with(|c| c.get()) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const FS: f64 = 16_384.0;
const TICK: usize = 164;
const BLOCK: usize = 256;

/// A clock that moves a fixed 10 microseconds a reading: a call is always on
/// time, and reading it allocates nothing.
struct StepClock(std::sync::atomic::AtomicU64);

impl LawClock for StepClock {
    fn now_s(&self) -> f64 {
        self.0.fetch_add(10, Ordering::Relaxed) as f64 * 1e-6
    }
}

fn step() -> SineProcessStep {
    let servo = AmplitudeServo::new(1.0, 20.0)
        .with_gains(0.3, 2.0)
        .with_drive_ceiling(1e6)
        .with_fault_fraction(1e-3);
    let blend = BlendLaw::new(BlendConfig {
        w: 0.5,
        max_step_db_per_tick: 1.0,
    })
    .unwrap();
    let mut profile = SineProfile::new("flat");
    profile.push(hertzforge_sine::profile::ProfileRow::accel(60.0, 1.0));
    profile.push(hertzforge_sine::profile::ProfileRow::accel(500.0, 1.0));
    profile.global_abort_db = (40.0, 40.0);
    let (step, _handles) = SineProcessStep::new(StepBuild {
        fs_hz: FS,
        control_tick_frames: TICK,
        out_channels: 1,
        profile,
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
        scheduler: SchedulerBuild::Sweep(
            BidirSweep::new(BidirSweepConfig {
                band: (60.0, 500.0),
                rate: SweepRate::LogOctPerMin(2.0),
                start_hz: 60.0,
                initial_direction: SweepDirection::Up,
                target: SweepTarget::Sweeps(4),
            })
            .unwrap(),
        ),
        ladder: LadderPlan::new(LadderConfig {
            sysid_level_db: -20.0,
            start_db: 0.0,
            table_db: vec![0.0],
            per_rung: RungFor::Sweeps(1),
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
        settle_ticks: 4,
        scalar_abort_debounce: 3,
        start_f_hz: 60.0,
        start_level_scale: 0.3,
        run_mode: hertzforge_sine::RunMode::Auto,
        tracking_lag_frames: 0,
        ring_blocks: 8,
        block_frames: BLOCK,
    });
    step
}

/// Run `blocks` blocks of a loop-back plant (gain 2), counting allocations
/// over the last `count` of them.
fn run(step: &mut SineProcessStep, blocks: usize, count: usize) -> usize {
    let mut input = vec![0.0f32; BLOCK];
    let mut drive: Vec<f32> = Vec::with_capacity(BLOCK * 2);
    ALLOCS.store(0, Ordering::Relaxed);
    for b in 0..blocks {
        if b == blocks - count {
            COUNTING.with(|c| c.set(true));
        }
        step.process(&input, BLOCK, &mut drive);
        for (i, d) in drive.iter().take(BLOCK).enumerate() {
            input[i] = 2.0 * d;
        }
    }
    COUNTING.with(|c| c.set(false));
    ALLOCS.load(Ordering::Relaxed)
}

#[test]
fn the_control_tick_allocates_nothing_with_or_without_the_guards() {
    // The built-in law, as the step builds it, with no guards: about 7500
    // ticks counted after 1600 of warm-up (past the swept trace's halving at
    // 8192 points, which must not reach the allocator either).
    let mut s = step();
    let n = run(&mut s, 6_000, 4_800);
    assert_eq!(n, 0, "{n} allocations on the control thread, no guards");

    // The same law installed through the seam under the core's guards, timed
    // on a clock and caught per call.
    let mut s = step();
    let params = ServoBlendParams {
        blend_w: 0.5,
        compression: hertzforge_sine::CompressionPreset::Slow,
        kp: 0.3,
    };
    let law = ServoBlendLaw::new(&params).unwrap();
    s.install_law(
        Box::new(law),
        &params.descriptor(),
        Some(LawGuards::default()),
        Arc::new(StepClock(std::sync::atomic::AtomicU64::new(0))),
    );
    s.engage_law(0.0).unwrap();
    let n = run(&mut s, 6_000, 4_800);
    assert_eq!(
        n, 0,
        "{n} allocations on the control thread, under the guards"
    );
}
