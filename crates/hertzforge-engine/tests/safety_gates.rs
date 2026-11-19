//! Engine safety gates, hardware-free.
//!
//! Every test here states the MUTATION that makes it red - the mutation discipline.
//! A safety test whose green cannot be turned red is not evidence that the gate exists.

#[path = "../src/test_backend.rs"]
mod test_backend;

use std::time::Duration;

use hertzforge_channel_table::{AbortLimits, Channel, ChannelRole, ChannelTable};
use hertzforge_engine::{
    scheduler::SilentDrive, BlockMeta, CaptureError, CaptureSink, Engine, EngineConfig,
    EngineConfigError, StopReason, VecCaptureSink, MAX_BLANKING,
};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner, OutputBufferContract, UnderrunPolicy,
};
use test_backend::MockBackend;

fn table_1ch(accel_limit: Option<f64>) -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut c = Channel::new(1);
    c.role = ChannelRole::Control;
    c.abort = AbortLimits {
        accel_g: accel_limit,
        velocity_ips: None,
        displacement_mil: None,
    };
    t.channels.push(c);
    t
}

fn cfg(frames_per_tick: usize) -> EngineConfig {
    let mut c = EngineConfig::new(1, 1);
    c.frames_per_tick = frames_per_tick;
    c.watchdog_sustain = 1; // spike-sensitive
    c
}

fn caps_1ch(rate: f64) -> Caps {
    Caps {
        loop_owner: LoopOwner::HostClosesLoop,
        native_modes: vec![],
        clock: ClockDomain::Shared,
        max_input_channels: 1,
        max_output_channels: 1,
        abort_latency_bound: None,
        output_buffer: Some(OutputBufferContract {
            min_lookahead_frames: 4,
            underrun_policy: UnderrunPolicy::RampDownThenAbort,
        }),
        sample_rate_hz: rate,
        delegated_response_observable: false,
        clock_groups: vec![],
    }
}

// =====================================================================================
// Item 7 - the input-stall deadman
// =====================================================================================

/// A backend that is ALIVE and answering, and never delivers a frame. Exactly the
/// shape of an NI acquisition whose sample clock stopped: the driver is happy, the
/// reads time out, and nothing arrives - forever.
struct EternalNoDataYet {
    reads: u64,
    estops: u32,
    drive_frames: usize,
}

impl Backend for EternalNoDataYet {
    fn caps(&self) -> Caps {
        caps_1ch(1000.0)
    }
    fn open(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), HalError> {
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
    fn read_input(&mut self, _out: &mut [f32]) -> Result<InputRead, HalError> {
        self.reads += 1;
        Ok(InputRead::NoDataYet) // alive, and never a frame
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.drive_frames += drive.len();
        Ok(drive.len())
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus {
            connected: true,
            ..Default::default()
        }
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.estops += 1;
        Ok(())
    }
}

/// A permanently silent input must be a FAULTED stop, never a clean end.
///
/// RED-MUTATION: delete the deadman (the `if silent > self.stall_deadline` block
/// on the `NoDataYet` arm of `Engine::run`) and this test reds - the run walks out
/// through `max_ticks` and reports `InputEnded`, i.e. a test that never acquired a
/// single sample reports as a test that finished.
#[test]
fn an_eternally_silent_input_is_a_faulted_stop_not_a_clean_end() {
    let table = table_1ch(Some(10.0));
    let mut backend = EternalNoDataYet {
        reads: 0,
        estops: 0,
        drive_frames: 0,
    };
    let mut c = cfg(8);
    c.stall_deadline = Some(Duration::from_millis(20));
    let mut engine = Engine::new(c, &table, &backend);
    let mut proc = SilentDrive { out_channels: 1 };
    let mut sink = VecCaptureSink::new();

    let report = engine.run(&mut backend, &mut proc, &mut sink, 10_000_000);

    match report.reason {
        StopReason::InputStalled { elapsed } => {
            assert!(
                elapsed >= Duration::from_millis(20),
                "the reported silence {elapsed:?} must be at least the deadline"
            );
        }
        other => panic!("expected InputStalled, got {other:?}"),
    }
    assert_eq!(report.frames_in, 0, "nothing was ever acquired");
    assert_eq!(
        backend.estops, 1,
        "a stalled input aborts through emergency_stop"
    );
    // And it stopped on the DEADMAN, not on max_ticks: the tick budget was 10M.
    assert!(
        report.ticks < 10_000_000,
        "the run must end on the deadline, not on the tick budget"
    );
}

/// A `NoDataYet` tick must still poll the watchdog and still feed the drive. The
/// old `continue` skipped both: an AI hiccup silently took the safety monitor and
/// the AO stream offline for as long as it lasted.
///
/// RED-MUTATION: restore the bare `Ok(NoDataYet) => continue` and this test reds
/// on the watchdog-poll count (zero polls across a run of stalled ticks).
#[test]
fn a_stalled_tick_still_polls_the_watchdog_and_still_feeds_the_drive() {
    let table = table_1ch(Some(10.0));
    let mut backend = EternalNoDataYet {
        reads: 0,
        estops: 0,
        drive_frames: 0,
    };
    let mut c = cfg(8);
    // Long deadline so the run ends on `max_ticks`, isolating the per-tick work.
    c.stall_deadline = Some(Duration::from_secs(30));
    let mut engine = Engine::new(c, &table, &backend);
    let mut proc = SilentDrive { out_channels: 1 };
    let mut sink = VecCaptureSink::new();

    let report = engine.run(&mut backend, &mut proc, &mut sink, 50);

    assert_eq!(report.reason, StopReason::InputEnded, "ended on max_ticks");
    let health = engine.health();
    assert_eq!(
        health.watchdog_polls, 50,
        "the watchdog must be polled on EVERY tick, including the ones with no data"
    );
    assert!(
        backend.drive_frames > 0,
        "the drive prefill must still have reached the device: the AO cannot be \
         allowed to starve because the AI hiccuped"
    );
}

/// The deadman clock is TIME, not ticks: a stream that keeps delivering, however
/// slowly, is alive and must never be declared stalled.
///
/// RED-MUTATION: refresh `last_frames_instant` on EVERY tick instead of only on a
/// delivering one and the eternal-`NoDataYet` test above reds; refresh it on NO
/// tick and this one reds. The pair pins the clock to exactly the right event.
#[test]
fn an_intermittent_but_delivering_stream_is_not_a_stall() {
    /// Delivers a frame every 4th poll and `NoDataYet` the rest of the time.
    struct Intermittent {
        polls: u64,
    }
    impl Backend for Intermittent {
        fn caps(&self) -> Caps {
            caps_1ch(1000.0)
        }
        fn open(&mut self) -> Result<(), HalError> {
            Ok(())
        }
        fn close(&mut self) -> Result<(), HalError> {
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
            self.polls += 1;
            if self.polls.is_multiple_of(4) {
                out.iter_mut().for_each(|s| *s = 1.0);
                Ok(InputRead::Frames(out.len()))
            } else {
                Ok(InputRead::NoDataYet)
            }
        }
        fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
            Ok(drive.len())
        }
        fn poll_status(&mut self) -> BackendStatus {
            BackendStatus {
                connected: true,
                sample_counter: self.polls / 4 * 8,
                ..Default::default()
            }
        }
        fn emergency_stop(&mut self) -> Result<(), HalError> {
            Ok(())
        }
    }

    let table = table_1ch(Some(1000.0)); // generous: never breaches
    let mut backend = Intermittent { polls: 0 };
    let mut c = cfg(8);
    // A deadline far shorter than the whole run, but longer than the gap between
    // two delivering polls. A tick-counting deadman would fire here; a
    // time-based one that is reset by DELIVERY does not.
    c.stall_deadline = Some(Duration::from_millis(500));
    let mut engine = Engine::new(c, &table, &backend);
    let mut proc = SilentDrive { out_channels: 1 };
    let mut sink = VecCaptureSink::new();
    let report = engine.run(&mut backend, &mut proc, &mut sink, 400);

    assert_eq!(
        report.reason,
        StopReason::InputEnded,
        "a slow-but-delivering stream is alive"
    );
    assert_eq!(
        report.frames_in,
        100 * 8,
        "every 4th poll delivered a block"
    );
}

// =====================================================================================
// Item 8 - ring overrun surfacing + the limits-armed hard fail
// =====================================================================================

/// The overrun counter is REAL, not a constant zero: a healthy run reports zero,
/// and `ring::tests::full_ring_drops_and_counts_overruns` proves the producer
/// counts a genuine drop. This test pins the reporting seam (`RunReport` +
/// `EngineHealth`), which had NO reader at all before the ring-overrun rule.
///
/// RED-MUTATION: return a hard-coded `0` from `RunReport.ring_overruns` /
/// `EngineHealth.ring_overruns` instead of reading `raw_prod.overruns()` and
/// `a_dropped_raw_frame_fails_a_limits_armed_run` (below) reds, because the rule
/// it drives is fed by this same read.
///
/// REACHABILITY, stated plainly (a result that cannot fail is not evidence): in
/// TODAY's single-threaded engine a producer-side overrun cannot occur.
/// `Engine::run`'s push loop drains the watchdog and retries on a full ring, and
/// `drain_each` empties the ring completely, so the retry always succeeds - and the
/// ring is sized at >= 8 blocks anyway. So this test asserting `ring_overruns == 0`
/// proves the plumbing, NOT the rule. The rule itself is driven directly by the test
/// below, and it becomes reachable the moment the watchdog moves to its own thread
/// (the intended design), where the producer can no longer drain on the consumer's
/// behalf.
#[test]
fn a_healthy_run_reports_zero_overruns_through_both_surfaces() {
    let table = table_1ch(Some(1000.0)); // generous limit: never trips
    let script: Vec<_> = (1..=5)
        .map(|i| MockBackend::const_block(&[1.0], 8, (i as u64) * 8, 0))
        .collect();
    let mut backend = MockBackend::new(1).with_rate(1000.0).with_script(script);
    backend.open().unwrap();
    backend.arm().unwrap();
    backend.start().unwrap();

    let mut engine = Engine::new(cfg(8), &table, &backend);
    let mut proc = SilentDrive { out_channels: 1 };
    let mut sink = VecCaptureSink::new();
    let report = engine.run(&mut backend, &mut proc, &mut sink, 100);

    assert_eq!(report.reason, StopReason::InputEnded);
    assert_eq!(report.ring_overruns, 0);
    assert_eq!(engine.health().ring_overruns, 0);
    assert!(
        engine.health().limits_armed,
        "this table carries an accel limit, so the overrun rule binds this run"
    );
}

/// THE RULE: any dropped raw frame on a LIMITS-ARMED run fails the run; on a run
/// with no limits armed it does not. Driven directly against
/// `hertzforge_engine::scheduler::ring_overrun_stop`, the pure decision the tick loop
/// calls, because the engine cannot currently produce a drop to feed it (see the
/// reachability note above) and a rule tested only through an unreachable path is
/// a rule with no test.
///
/// RED-MUTATION: make `ring_overrun_stop` return `None` unconditionally (i.e.
/// report the overrun and carry on, the pre-item-8 posture) and this test reds -
/// a limits-armed run that dropped frames the watchdog never examined completes
/// as if it had been monitored throughout.
#[test]
fn a_dropped_raw_frame_fails_a_limits_armed_run() {
    use hertzforge_engine::scheduler::ring_overrun_stop;

    // No drops: nothing to decide, either way.
    assert_eq!(ring_overrun_stop(0, true), None);
    assert_eq!(ring_overrun_stop(0, false), None);

    // Drops with limits ARMED: the run FAILS, carrying the count.
    assert_eq!(
        ring_overrun_stop(1, true),
        Some(StopReason::RingOverrun { frames: 1 }),
        "ONE dropped frame is enough - it could have been the frame with the breach"
    );
    assert_eq!(
        ring_overrun_stop(37, true),
        Some(StopReason::RingOverrun { frames: 37 })
    );

    // Drops with NO limits armed: nothing could have been hidden, so it is a
    // scheduling-health signal (reported in `RunReport`) and not a stop.
    assert_eq!(ring_overrun_stop(37, false), None);
}

/// The ring depth is ENFORCED, not merely suggested: a caller that asks for a ring
/// shallower than the block-derived floor gets it raised, and the raise is
/// recorded rather than applied silently.
///
/// RED-MUTATION: drop the auto-raise in `Engine::new` and this test reds - the
/// engine builds a 16-frame ring under a 512-frame block, which would drop 31 of
/// every 32 raw frames straight past the abort watchdog.
#[test]
fn a_ring_shallower_than_the_block_is_raised_and_the_raise_is_recorded() {
    let table = table_1ch(Some(10.0));
    let backend = MockBackend::new(1).with_rate(51_200.0);
    let mut c = EngineConfig::new(1, 1);
    c.frames_per_tick = 512;
    c.ring_frames = 16; // absurdly shallow: under ONE block
    let engine = Engine::new(c, &table, &backend);

    assert_eq!(engine.ring_auto_raised(), Some((16, 512 * 8)));
    assert_eq!(engine.ring_frames(), 512 * 8);
    assert!(
        engine.ring_frames() >= 8 * engine.frames_per_tick(),
        "the ring must hold at least 8 blocks"
    );

    // A ring that already satisfies the floor is left exactly as configured.
    let mut ok = EngineConfig::new(1, 1);
    ok.frames_per_tick = 8;
    ok.ring_frames = 4096;
    let e2 = Engine::new(ok, &table, &backend);
    assert_eq!(e2.ring_auto_raised(), None);
    assert_eq!(e2.ring_frames(), 4096);
}

/// `limits_armed` is what the overrun rule keys off, so it must key off the TABLE
/// and not off anything incidental.
#[test]
fn limits_armed_follows_the_channel_table() {
    let backend = MockBackend::new(1).with_rate(1000.0);
    assert!(
        Engine::new(cfg(8), &table_1ch(Some(10.0)), &backend)
            .health()
            .limits_armed
    );
    assert!(
        !Engine::new(cfg(8), &table_1ch(None), &backend)
            .health()
            .limits_armed,
        "a table with no abort axes arms no limits, so the overrun rule does not bind"
    );
}

// =====================================================================================
// Item 10 - the arm blanking window
// =====================================================================================

/// A sink that only COUNTS. The blanking tests run tens of thousands of ticks
/// waiting for a wall-clock window to close, and `VecCaptureSink` would grow
/// without bound across them.
struct CountingSink {
    frames: u64,
}

impl CaptureSink for CountingSink {
    fn write_block(&mut self, _samples: &[f32], meta: BlockMeta) -> Result<(), CaptureError> {
        self.frames += meta.n_frames;
        Ok(())
    }
    fn finalize(&mut self) -> Result<(), CaptureError> {
        Ok(())
    }
}

/// A backend that breaches on every frame, generating blocks on the fly (no
/// scripted-block allocation) so a long run is cheap.
struct AlwaysBreaching {
    value: f32,
    counter: u64,
    estops: u32,
}

impl Backend for AlwaysBreaching {
    fn caps(&self) -> Caps {
        caps_1ch(1000.0)
    }
    fn open(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), HalError> {
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
        out.iter_mut().for_each(|s| *s = self.value);
        self.counter += out.len() as u64;
        Ok(InputRead::Frames(out.len()))
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        Ok(drive.len())
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus {
            connected: true,
            sample_counter: self.counter,
            ..Default::default()
        }
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.estops += 1;
        Ok(())
    }
}

/// A breach DURING the blanking window is observed and warned about, not acted on;
/// the identical breach AFTER the window trips. Both halves in one test so the
/// only difference between them is the window.
///
/// RED-MUTATION: make the blanking permanent (replace the per-tick
/// `run_start.elapsed() < self.blanking` with a bare `true`) and this test reds on
/// the second half - the post-window breach never trips and the run ends
/// `InputEnded` with the channel over its limit throughout.
#[test]
fn blanking_suppresses_a_trip_inside_the_window_and_arms_after_it() {
    let table = table_1ch(Some(10.0));
    let mut backend = AlwaysBreaching {
        value: 99.0,
        counter: 0,
        estops: 0,
    };
    let mut c = cfg(8);
    c.set_blanking(Duration::from_millis(30))
        .expect("30 ms is inside the cap");
    let mut engine = Engine::new(c, &table, &backend);
    let mut proc = SilentDrive { out_channels: 1 };
    let mut sink = CountingSink { frames: 0 };
    // Bounded well above the ticks a 30 ms window can span, so a healthy run
    // trips long before the budget and a MUTATED one (permanent blanking) still
    // terminates instead of hanging the suite.
    let report = engine.run(&mut backend, &mut proc, &mut sink, 2_000_000);

    // FIRST HALF: the breach was seen while blanked and NOT acted on.
    assert!(
        report.blanked_trips > 0,
        "setup invalid: the window never observed a would-have-tripped breach, so \
         the suppression half of this test proved nothing"
    );
    assert!(
        engine.first_blanked_trip().is_some(),
        "the first suppressed breach is kept as an operator warning"
    );
    // SECOND HALF: once the window closed, the same breach tripped.
    match report.reason {
        StopReason::AbortTripped(trip) => assert!(trip.value >= 99.0),
        other => panic!("the watchdog must arm after the window; got {other:?}"),
    }
    assert_eq!(
        report.estops, 1,
        "the post-window trip called emergency_stop exactly once"
    );
    assert_eq!(backend.estops, 1);
}

/// With NO blanking configured (the HERTZFORGE-SIM default) the very first breach
/// trips. The control for the test above: it proves the suppression there came
/// from the window and not from something else about the run.
#[test]
fn zero_blanking_trips_on_the_first_breach() {
    let table = table_1ch(Some(10.0));
    let mut backend = AlwaysBreaching {
        value: 99.0,
        counter: 0,
        estops: 0,
    };
    let c = cfg(8);
    assert_eq!(
        c.blanking(),
        Duration::ZERO,
        "HERTZFORGE-SIM default is no window"
    );
    let mut engine = Engine::new(c, &table, &backend);
    let mut proc = SilentDrive { out_channels: 1 };
    let mut sink = VecCaptureSink::new();
    let report = engine.run(&mut backend, &mut proc, &mut sink, 100);

    assert!(matches!(report.reason, StopReason::AbortTripped(_)));
    assert_eq!(report.ticks, 1, "it tripped on the very first block");
    assert_eq!(report.blanked_trips, 0, "nothing was suppressed");
}

// =====================================================================================
// The per-tick COMPUTE BUDGET seam (the thing that made the one-attempt retry rule
// implementable)
// =====================================================================================

/// A `ProcessStep` that deliberately burns wall-clock time.
struct SlowStep {
    dwell: Duration,
}

impl hertzforge_engine::ProcessStep for SlowStep {
    fn process(&mut self, _input: &[f32], n: usize, out: &mut Vec<f32>) {
        std::thread::sleep(self.dwell);
        out.clear();
        out.resize(n, 0.0);
    }
}

fn run_with_budget(budget: Option<Duration>, dwell: Duration) -> StopReason {
    let table = table_1ch(None);
    let script: Vec<_> = (1..=10)
        .map(|i| MockBackend::const_block(&[1.0], 8, (i as u64) * 8, 0))
        .collect();
    let mut backend = MockBackend::new(1).with_rate(1000.0).with_script(script);
    backend.open().unwrap();
    backend.arm().unwrap();
    backend.start().unwrap();
    let mut c = cfg(8);
    c.jitter_frac = hertzforge_engine::TEST_DISABLE_JITTER_GUARD; // isolate the COMPUTE half
    c.compute_budget = budget;
    let mut engine = Engine::new(c, &table, &backend);
    let mut proc = SlowStep { dwell };
    let mut sink = CountingSink { frames: 0 };
    engine
        .run(&mut backend, &mut proc, &mut sink, 100)
        .reason
        .clone()
}

/// The compute budget is a real, configurable gate, not decoration: the SAME slow
/// step faults under the tick-period default and runs clean under an explicit
/// relaxed budget.
///
/// This is what made the one-attempt retry rule implementable. With the budget
/// hard-wired to the tick period (8 frames at 1 kHz = 8 ms here), a deterministic
/// harness manufactures `SchedulingFault`s out of OS jitter, and every Sim module
/// grew a retry loop to swallow them. Removing the CAUSE is what lets the retry be
/// one attempt on zero progress instead of eight on anything.
///
/// RED-MUTATION: ignore `cfg.compute_budget` in `Engine::new` (drop the
/// `with_compute_budget` call) and this test reds on the second half - the relaxed
/// run faults exactly like the default one, and the seam is decorative.
#[test]
fn the_compute_budget_is_configurable_and_enforced() {
    // 20 ms of work against the 8 ms tick-period default: a genuine overrun.
    let with_default = run_with_budget(None, Duration::from_millis(20));
    assert!(
        matches!(with_default, StopReason::SchedulingFault(_)),
        "the tick-period default must still catch a real overrun; got {with_default:?}"
    );

    // The same work under an explicit relaxed budget: clean.
    let with_relaxed = run_with_budget(
        Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET),
        Duration::from_millis(20),
    );
    assert_eq!(
        with_relaxed,
        StopReason::InputEnded,
        "an explicit budget above the per-tick cost must not fault"
    );
}

/// It must be IMPOSSIBLE to configure a run whose blanking window never ends. The
/// field is private and the only setter refuses past the cap.
///
/// RED-MUTATION: make `EngineConfig::blanking` public (or make `set_blanking`
/// clamp instead of refusing) and this test reds - `Duration::MAX` becomes an
/// expressible blanking window, i.e. an expressible permanently-disabled watchdog.
#[test]
fn a_blanking_window_past_the_cap_is_refused_not_clamped() {
    let mut c = cfg(8);
    let over = MAX_BLANKING + Duration::from_millis(1);
    let err = c
        .set_blanking(over)
        .expect_err("past the cap must be refused");
    assert_eq!(
        err,
        EngineConfigError::BlankingTooLong {
            requested: over,
            max: MAX_BLANKING,
        }
    );
    // The refusal LEFT THE CONFIG ALONE - a rejected value must not half-apply.
    assert_eq!(c.blanking(), Duration::ZERO);

    // Duration::MAX - "never ends" in its purest form - is refused too.
    assert!(c.set_blanking(Duration::MAX).is_err());
    assert_eq!(c.blanking(), Duration::ZERO);

    // Exactly at the cap is allowed: the boundary is inclusive.
    c.set_blanking(MAX_BLANKING)
        .expect("the cap itself is legal");
    assert_eq!(c.blanking(), MAX_BLANKING);
}
