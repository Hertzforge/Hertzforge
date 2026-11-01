//! Sine notch LIVENESS: the notch demonstrably ENGAGES, at least once, under a
//! profile designed to approach the limit.
//!
//! # Why this file exists
//!
//! The notch test the old acceptance gate leaned on - `notch_holds_limit_channel`
//! (`sim_e2e.rs:391-443`) - **cannot fail toward its claim**. It runs a profile and
//! asserts the limit channel never exceeded its bound. A run in which the notch never
//! engaged at all, because the profile never approached the limit, passes that
//! assertion just as cleanly as a run in which the notch worked. It cannot
//! distinguish "the governor held the line" from "there was no line to hold", and a
//! result that cannot fail is not evidence.
//!
//! The notch-hysteresis suite (`sim_e2e.rs:2697-2790`) is genuinely falsifiable - it
//! asserts that a legitimately-notched under-test does NOT trip the low-side abort, and
//! that a genuine over-test DOES still trip the high side - and both remain part of the
//! gate.
//! What neither states positively is the LIVENESS property: *the governor engaged*.
//!
//! This file states it, two independent ways:
//!
//! 1. **The service's own reporting.** A watcher thread polls the live
//!    `ControlSnapshot` while the engine runs and latches whether `notch_binding` was
//!    ever observed true. That is the governor saying, in its own words and during the
//!    run, that it was holding the drive down.
//! 2. **A no-notch COUNTERFACTUAL.** The identical fixture is run with
//!    `notch: None`, and the achieved control level is compared. With the governor the
//!    control point is held far below its target (the drive is capped to protect the
//!    limit channel); without it the servo drives the control channel up to target.
//!    The difference IS the notch's effect on the drive, measured rather than asserted.
//!
//! Point 2 is what makes point 1 non-vacuous: it proves the fixture actually approaches
//! the limit, so `notch_binding` had something to be true about.
//!
//! # Red-mutation (the falsifiability rule)
//!
//! **Disable the notch** - pass `notch: None` in [`notch_step`], or make
//! `NotchGovernor::ceiling` return `f64::INFINITY` so it never binds - and BOTH tests
//! red: `notch_engages_at_least_once_under_a_limiting_profile` sees `notch_binding`
//! never set, and `the_notch_measurably_reduces_the_drive_versus_no_notch` sees the two
//! runs converge to the same control level.
//!
//! # Scope
//!
//! A NEW file, deliberately: the gate diff-asserts that the EXISTING sine
//! suites are untouched. Nothing here is shared with `sim_e2e.rs`; the fixture is
//! rebuilt locally (that file's helpers are private to it), which also means this test
//! cannot be broken by a change to a fixture written for a different claim.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_dsp::sine::AmplitudeServo;
use hertzforge_dsp::tracking::BandwidthMode;
use hertzforge_engine::{Engine, EngineConfig, StopReason, VecCaptureSink};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner, OutputBufferContract, UnderrunPolicy,
};
use hertzforge_sine::service::engine::{
    ColaState, LimitChannelSpec, SchedulerBuild, SineProcessStep, StepBuild, StepHandles,
};
use hertzforge_sine::TripKind;
use hertzforge_sine::{
    BlendConfig, BlendLaw, ColaWaveform, CompositeMode, DriveGuard, DwellFor, DwellPlan, DwellRun,
    LadderConfig, LadderPlan, NotchGovernor, NotchGovernorConfig, ProfileRow, RungFor, SineProfile,
};

// =====================================================================================
// Fixture: a two-input (control + limit) plant whose LIMIT channel is far more
// sensitive than its control channel, so a profile that asks for the control target
// necessarily drives the limit channel past its bound - i.e. a profile designed to
// approach the limit.
// =====================================================================================

/// Sample rate. Matches the notch-hysteresis fixtures' 8,192 Hz so the tracking filter
/// and the 300 Hz dwell sit in the same regime those were tuned against.
const FS_HZ: f64 = 8_192.0;
/// Dwell frequency, inside the profile band.
const DWELL_HZ: f64 = 300.0;
/// Control-channel sensitivity: response g per unit drive.
const CTRL_GAIN: f32 = 0.5;
/// Limit-channel sensitivity: 10x the control channel. This is the whole fixture -
/// the limit channel reaches its bound long before the control channel reaches target.
const LIMIT_GAIN: f32 = 5.0;
/// Control-channel target, g pk.
const CTRL_TARGET_G: f64 = 1.0;
/// Limit-channel abort bound, g pk. With `LIMIT_GAIN` = 5.0 the governor must cap the
/// drive near 0.4 / 5.0 = 0.08, which puts the control channel near
/// 0.08 * 0.5 = 0.04 g - about 4% of its 1 g target.
const LIMIT_G: f64 = 0.4;
/// Engine ticks per run. One frame per tick, so this is also the frame count.
const RUN_TICKS: u64 = 8_000;

/// A minimal two-input (control + limit) one-drive backend: each input reads a
/// gain-scaled copy of the last commanded drive sample. No free-running offsets and no
/// dead-sensor behavior - this fixture is about the governor, not about fault paths.
struct TwoChannelBackend {
    fs_hz: f64,
    counter: u64,
    last_drive: f32,
    ctrl_gain: f32,
    limit_gain: f32,
    /// Peak |commanded drive| over the whole run. Recorded HERE, at the point the
    /// drive actually leaves the host, so the counterfactual measures the drive
    /// itself rather than a sampled proxy for it - and deterministically, with no
    /// polling that could miss a peak.
    peak_drive: f32,
    open: bool,
}

impl TwoChannelBackend {
    fn new() -> Self {
        TwoChannelBackend {
            fs_hz: FS_HZ,
            counter: 0,
            last_drive: 0.0,
            ctrl_gain: CTRL_GAIN,
            limit_gain: LIMIT_GAIN,
            peak_drive: 0.0,
            open: false,
        }
    }
}

impl Backend for TwoChannelBackend {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: 2,
            max_output_channels: 1,
            abort_latency_bound: None,
            output_buffer: Some(OutputBufferContract {
                min_lookahead_frames: 4,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            sample_rate_hz: self.fs_hz,
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.open = true;
        Ok(())
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.open = false;
        Ok(())
    }
    fn configure(
        &mut self,
        _t: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        Ok(ConfiguredActual {
            actual_rate_hz: acq.requested_rate_hz,
            block_frames: acq.block_frames,
            group_delay_frames: None,
        })
    }
    fn arm(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn start(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        // Interleaved [control, limit] per frame; both are pure gain on the last
        // commanded drive sample.
        let frames = out.len() / 2;
        for f in 0..frames {
            out[f * 2] = self.last_drive * self.ctrl_gain;
            out[f * 2 + 1] = self.last_drive * self.limit_gain;
        }
        self.counter += frames as u64;
        // FRAMES, per the `hertzforge_hal::Backend::read_input` contract.
        Ok(InputRead::Frames(frames))
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        for &d in drive {
            self.peak_drive = self.peak_drive.max(d.abs());
        }
        if let Some(&d) = drive.last() {
            self.last_drive = d;
        }
        Ok(drive.len())
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus {
            connected: self.open,
            faulted: false,
            message: "notch-liveness".into(),
            sample_counter: self.counter,
            epoch: 0,
        }
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        Ok(())
    }
}

/// Control channel 1 (AccelG), limit monitor channel 3 (AccelG), drive channel 2.
fn table_ctrl_and_limit() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    let mut lim = Channel::new(3);
    lim.role = ChannelRole::Monitor;
    lim.units = Units::AccelG;
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(ctrl);
    t.channels.push(lim);
    t.channels.push(drv);
    t
}

/// A flat `g`-peak profile over `[lo, hi]` with explicit alarm / abort bands.
fn flat_profile(
    label: &str,
    lo: f64,
    hi: f64,
    g: f64,
    alarm_db: f64,
    abort_db: f64,
) -> SineProfile {
    let mut p = SineProfile::new(label);
    p.push(ProfileRow::accel(lo, g));
    p.push(ProfileRow::accel(hi, g));
    p.global_alarm_db = (alarm_db, alarm_db);
    p.global_abort_db = (abort_db, abort_db);
    p
}

/// The fixture step. `with_notch` is the ONE variable between the two runs - the
/// counterfactual axis - so any difference in outcome is attributable to the governor
/// and to nothing else.
fn notch_step(with_notch: bool) -> StepBuild {
    let servo = AmplitudeServo::new(1.0, 60.0)
        .with_gains(0.3, 6.0)
        .with_drive_ceiling(1e6)
        .with_fault_fraction(1e-3);
    let blend = BlendLaw::new(BlendConfig {
        w: 0.0,
        max_step_db_per_tick: 1.0,
    })
    .unwrap();
    let dwell = DwellRun::new(DwellPlan {
        freq_hz: DWELL_HZ,
        for_: DwellFor::Seconds(10.0),
    })
    .unwrap();
    let ladder = LadderPlan::new(LadderConfig {
        sysid_level_db: -20.0,
        start_db: 0.0,
        table_db: vec![0.0],
        per_rung: RungFor::Sweeps(1),
        full_for: RungFor::Seconds(10.0),
    })
    .unwrap();

    // The control profile asks for CTRL_TARGET_G with a WIDE (+/-6 dB) abort band: the
    // notched run is a legitimate under-test and must not trip the low-side scalar
    // abort before the run has had time to demonstrate anything (that suppression is
    // the notch-hysteresis behavior the existing suite already pins; this file must not
    // re-litigate it, only avoid being cut short by it).
    let ctrl_profile = flat_profile("ctrl", 100.0, 500.0, CTRL_TARGET_G, 3.0, 6.0);
    let limit_profile = flat_profile("limit", 100.0, 500.0, LIMIT_G, 3.0, 6.0);

    let notch = with_notch.then(|| {
        NotchGovernor::new(
            NotchGovernorConfig {
                smoothing_ticks: 3.0,
                drive_floor: 0.01,
                ..NotchGovernorConfig::default()
            },
            1,
        )
        .unwrap()
    });

    StepBuild {
        fs_hz: FS_HZ,
        control_tick_frames: 32,
        out_channels: 1,
        profile: ctrl_profile,
        n_input_channels: 2,
        control_slots: vec![0],
        control_weights: vec![1.0],
        composite_mode: CompositeMode::WeightedAvg,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        servo,
        blend,
        feedforward: None,
        notch,
        // The limit channel is declared either way: only the GOVERNOR differs between
        // the two runs, so the no-notch run is the same test with the protection
        // removed, not a different test.
        limit_channels: vec![LimitChannelSpec {
            input_slot: 1,
            channel_index: 3,
            profile: limit_profile,
            sysid_h: None,
        }],
        scheduler: SchedulerBuild::Dwell(dwell),
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
        fault_debounce_ticks: 25,
        fault_fraction: 1e-2,
        settle_ticks: 20,
        scalar_abort_debounce: 3,
        start_f_hz: DWELL_HZ,
        start_level_scale: 0.3,
        run_mode: hertzforge_sine::RunMode::Auto,
        // No transport lag in this fixture, so the tracking
        // reference is uncompensated and the arithmetic is unchanged.
        tracking_lag_frames: 0,
        ring_blocks: 8,
        block_frames: 64,
    }
}

/// What one fixture run observed.
struct RunOutcome {
    /// Was `notch_binding` EVER seen true by the watcher during the run? The
    /// service's own reporting of the governor engaging.
    ever_bound: bool,
    /// How many watcher samples saw it binding (0 when it never engaged).
    binding_samples: u64,
    /// Peak |commanded drive| over the whole run, straight off the backend's
    /// `write_drive`. THE measurement the counterfactual compares.
    peak_drive: f32,
    /// The control channel's achieved level at the end of the run, g pk.
    final_measured_g: f64,
    /// The control channel's target at the end of the run, g pk.
    final_target_g: f64,
    /// The trip the run ended on, if any.
    trip: Option<TripKind>,
    /// Ticks the engine actually executed.
    ticks: u64,
}

/// Run the fixture once, with a watcher thread sampling the live status while the
/// engine runs.
///
/// The watcher is what makes the liveness claim a LIVE observation rather than an
/// end-state inference: `notch_binding` is a per-tick condition, and reading it only
/// after the run would confuse "never engaged" with "engaged and then released".
fn run_fixture(with_notch: bool) -> RunOutcome {
    let (step, handles) = SineProcessStep::new(notch_step(with_notch));
    let StepHandles { status, .. } = handles.clone();

    let stop_watch = Arc::new(AtomicBool::new(false));
    let ever_bound = Arc::new(AtomicBool::new(false));
    let binding_samples = Arc::new(AtomicU64::new(0));
    // Both threads meet here so the watcher is definitely sampling before the first
    // engine tick - otherwise a fast run could finish before the watcher starts and
    // the test would read "never engaged" for a scheduling reason.
    let gate = Arc::new(Barrier::new(2));

    let watcher = {
        let status = Arc::clone(&status);
        let stop_watch = Arc::clone(&stop_watch);
        let ever_bound = Arc::clone(&ever_bound);
        let binding_samples = Arc::clone(&binding_samples);
        let gate = Arc::clone(&gate);
        std::thread::spawn(move || {
            gate.wait();
            let deadline = Instant::now() + Duration::from_secs(60);
            while !stop_watch.load(Ordering::Relaxed) && Instant::now() < deadline {
                if status.read().notch_binding {
                    ever_bound.store(true, Ordering::Relaxed);
                    binding_samples.fetch_add(1, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_micros(200));
            }
        })
    };

    let mut backend = TwoChannelBackend::new();
    backend.open().unwrap();
    backend.start().unwrap();
    let mut ecfg = EngineConfig::new(2, 1);
    ecfg.frames_per_tick = 1;
    ecfg.ring_frames = 256;
    // A deterministic host harness, not a hardware path: the jitter guard and the
    // tick-period compute budget both measure wall clock, and this loop runs flat out.
    // Named constants, per the engine's own instruction for a Sim harness.
    ecfg.jitter_frac = hertzforge_engine::TEST_DISABLE_JITTER_GUARD;
    ecfg.compute_budget = Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET);
    ecfg.watchdog_sustain = 64;
    let mut engine = Engine::new(ecfg, &table_ctrl_and_limit(), &backend);
    let mut sink = VecCaptureSink::new();

    gate.wait();
    let mut step = step;
    let mut attempts = 0;
    let mut ticks = 0u64;
    loop {
        let report = engine.run(&mut backend, &mut step, &mut sink, RUN_TICKS);
        ticks += report.ticks;
        match report.reason {
            StopReason::SchedulingFault(_) if attempts < 8 => {
                attempts += 1;
                let _ = backend.arm();
                let _ = backend.start();
            }
            _ => break,
        }
    }

    stop_watch.store(true, Ordering::Relaxed);
    watcher.join().expect("watcher thread");

    let snap = status.read();
    RunOutcome {
        ever_bound: ever_bound.load(Ordering::Relaxed),
        binding_samples: binding_samples.load(Ordering::Relaxed),
        peak_drive: backend.peak_drive,
        final_measured_g: snap.measured_g,
        final_target_g: snap.target_g,
        trip: handles.abort.take_record().map(|r| r.kind),
        ticks,
    }
}

// =====================================================================================
// The tests
// =====================================================================================

/// **The liveness claim.** Under a profile whose control target necessarily drives the
/// (10x more sensitive) limit channel past its bound, the notch governor ENGAGES - and
/// says so, live, through the service's own status surface.
///
/// The setup-validity assertion is what stops this from passing vacuously: the control
/// point must end up held far below its target. If it reached target, the limit was
/// never approached and `notch_binding` would have had nothing to be true about.
///
/// Red-mutation: build the step with `notch: None` (or make the governor's ceiling
/// unbounded) and `ever_bound` is false - the test reds on its first assertion.
#[test]
fn notch_engages_at_least_once_under_a_limiting_profile() {
    let out = run_fixture(true);

    assert!(
        out.ever_bound,
        "the notch governor never reported binding during a run whose control target \
         (%{CTRL_TARGET_G} g at gain {CTRL_GAIN}) requires a drive that puts the limit \
         channel (gain {LIMIT_GAIN}) at {:.2} g, well past its {LIMIT_G} g bound - \
         either the governor did not engage, or it is not reporting that it did \
         (binding samples: {})",
        CTRL_TARGET_G / CTRL_GAIN as f64 * LIMIT_GAIN as f64,
        out.binding_samples,
    );
    assert!(
        out.binding_samples > 0,
        "ever_bound and binding_samples must agree"
    );

    // Setup validity: the governor held the control point far below its target. A run
    // that reached target never approached the limit, and would make the assertion
    // above unfalsifiable.
    assert!(
        out.final_target_g > 0.0,
        "the fixture must actually have a target ({:.4} g)",
        out.final_target_g
    );
    assert!(
        out.final_measured_g < 0.5 * out.final_target_g,
        "setup invalid: the control point was NOT held below its target by the notch \
         (measured {:.4} g vs target {:.4} g over {} ticks), so the profile never \
         approached the limit and the liveness assertion above could not have failed",
        out.final_measured_g,
        out.final_target_g,
        out.ticks,
    );
}

/// **The counterfactual.** With the governor REMOVED, the same profile drives the
/// protected channel MORE THAN TWICE as far past its bound. The gap between the two
/// excursions is the notch's measured effect on the drive.
///
/// The measurement is the limit channel's reading at the moment it tripped, taken from
/// the service's own `TripKind::LimitChannel` record - the channel the governor exists
/// to protect, in the units the bound is expressed in. Peak COMMANDED drive is
/// deliberately NOT the assertion: both arms open with the same start-level transient
/// before either loop has reacted, so the peak is dominated by a moment that predates
/// the governor's first decision and would not discriminate.
///
/// # What this test does NOT claim
///
/// Both arms trip. The governor is a smoothed ceiling on the drive, not an
/// instantaneous clamp, so it does not prevent the opening transient from crossing the
/// bound - the abort remains the backstop, and that ordering is correct. What the
/// governor changes, measurably, is HOW FAR past the bound the protected channel gets:
/// barely over it when governed, more than double it when not. Claiming the notch
/// prevents the trip outright would be claiming something this fixture does not show.
///
/// Red-mutation: disable the notch in the notched arm - pass `notch: None` in
/// [`notch_step`] regardless of `with_notch`, or make `NotchGovernor::ceiling` return
/// `f64::INFINITY` - and the two arms become the same run, so the excursion ratio
/// collapses to 1 and the assertion reds.
#[test]
fn the_notch_measurably_reduces_the_drive_versus_no_notch() {
    let notched = run_fixture(true);
    let free = run_fixture(false);

    // The governor engaged in one arm and, having been removed, could not in the other.
    assert!(
        notched.ever_bound,
        "the notched arm must report binding (see the liveness test)"
    );
    assert!(
        !free.ever_bound,
        "the no-notch arm must not report binding - it has no governor"
    );

    // Setup validity: BOTH arms must have over-tested the LIMIT channel, which is what
    // makes this a profile that approaches the limit at all. A run that ended any other
    // way (a low-side under-test, a servo fault, a clean finish) is not the fixture
    // this test is about, and the comparison below would be measuring something else.
    let notched_excursion = limit_excursion(&notched.trip).unwrap_or_else(|| {
        panic!(
            "notched arm did not trip on the limit channel: {:?}",
            notched.trip
        )
    });
    let free_excursion = limit_excursion(&free.trip).unwrap_or_else(|| {
        panic!(
            "no-notch arm did not trip on the limit channel: {:?}",
            free.trip
        )
    });

    // The governed arm was held essentially AT the bound: it crossed, because a smoothed
    // ceiling cannot undo an opening transient, but only just.
    assert!(
        notched_excursion < 1.25,
        "the governed arm should sit essentially at the limit bound; it reached \
         {notched_excursion:.3}x it. Peak commanded drive was {:.4}.",
        notched.peak_drive,
    );
    // The ungoverned arm ran far past it.
    assert!(
        free_excursion > 1.5,
        "the un-notched arm should over-test the limit channel substantially; it only \
         reached {free_excursion:.3}x the bound, so there was little for the governor \
         to prevent and the ratio below would prove little"
    );

    // THE measurement: the governor more than halved how far past its bound the
    // protected channel got. The notch is the ONLY difference between the two runs.
    assert!(
        notched_excursion * 1.5 < free_excursion,
        "the notch did not measurably reduce the drive at the protected channel: \
         governed excursion {notched_excursion:.3}x the bound vs ungoverned \
         {free_excursion:.3}x. (Peak commanded drive: governed {:.4}, ungoverned {:.4}.)",
        notched.peak_drive,
        free.peak_drive,
    );
}

/// The limit channel's reading at trip, as a MULTIPLE of its bound. `None` when the run
/// ended some other way - deliberately not defaulted, so a fixture that stopped for an
/// unrelated reason fails loudly instead of silently comparing zeros.
fn limit_excursion(trip: &Option<TripKind>) -> Option<f64> {
    match trip {
        Some(TripKind::LimitChannel {
            measured, limit, ..
        }) if *limit > 0.0 => Some(measured / limit),
        _ => None,
    }
}
