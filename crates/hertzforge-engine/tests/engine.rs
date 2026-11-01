//! End-to-end engine safety tests, all HARDWARE-FREE. Drives the poll/pull
//! [`Engine`] against the scripted in-crate [`MockBackend`] and asserts the four
//! fault-injection behaviors plus the happy path.
//!
//! The mock is the crate's own `src/test_backend.rs`, included directly here (an
//! integration test is a separate crate and cannot reach a `#[cfg(test)]` module), so
//! there is exactly one mock definition shared with the in-crate unit tests.

#[path = "../src/test_backend.rs"]
mod test_backend;

use hertzforge_channel_table::{AbortLimits, Channel, ChannelRole, ChannelTable};
use hertzforge_engine::{
    scheduler::SilentDrive, Engine, EngineConfig, StopReason, TimebaseEvent, VecCaptureSink,
};
use hertzforge_hal::Backend;
use test_backend::{MockBackend, ScriptedBlock};

/// A 1-channel table with an acceleration abort limit on the control channel.
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

fn engine_cfg_1ch() -> EngineConfig {
    let mut cfg = EngineConfig::new(1, 1);
    cfg.frames_per_tick = 8;
    cfg.ring_frames = 256;
    cfg.watchdog_sustain = 1; // spike-sensitive
    cfg
}

fn open_arm_start(b: &mut MockBackend) {
    b.open().unwrap();
    b.arm().unwrap();
    b.start().unwrap();
}

// ------------------------------------------------------------------------------------
// (a) over-limit spike -> watchdog trips + emergency_stop called
// ------------------------------------------------------------------------------------
#[test]
fn over_limit_spike_trips_watchdog_and_estops() {
    let table = table_1ch(Some(10.0));
    let script = vec![
        // clean block, counter 8
        MockBackend::const_block(&[2.0], 8, 8, 0),
        // spike block: one frame at 99 g (>10) among 8, counter 16
        {
            let mut b = MockBackend::const_block(&[2.0], 8, 16, 0);
            b.samples[3] = 99.0; // inject the over-limit spike mid-block
            b
        },
        // would-be more input, but the run should already have aborted
        MockBackend::const_block(&[2.0], 8, 24, 0),
    ];
    let mut backend = MockBackend::new(1).with_rate(1000.0).with_script(script);
    open_arm_start(&mut backend);

    let mut engine = Engine::new(engine_cfg_1ch(), &table, &backend);
    let mut proc = SilentDrive { out_channels: 1 };
    let mut sink = VecCaptureSink::new();

    let report = engine.run(&mut backend, &mut proc, &mut sink, 100);

    match report.reason {
        StopReason::AbortTripped(trip) => {
            assert_eq!(trip.channel_index, 1);
            assert!(trip.value >= 99.0);
        }
        other => panic!("expected AbortTripped, got {other:?}"),
    }
    assert_eq!(report.estops, 1, "emergency_stop called exactly once");
    assert_eq!(backend.estop_count(), 1);
    assert!(!backend.is_started(), "run stopped");
}

// ------------------------------------------------------------------------------------
// (b) counter reset -> epoch bump detected
// ------------------------------------------------------------------------------------
#[test]
fn counter_reset_detected() {
    // Capture the discontinuity events the process step is told about.
    use std::sync::{Arc, Mutex};
    struct RecordingProc {
        events: Arc<Mutex<Vec<TimebaseEvent>>>,
    }
    impl hertzforge_engine::ProcessStep for RecordingProc {
        fn process(&mut self, _in: &[f32], n: usize, out: &mut Vec<f32>) {
            out.clear();
            out.resize(n, 0.0);
        }
        fn on_discontinuity(&mut self, ev: TimebaseEvent) {
            self.events.lock().unwrap().push(ev);
        }
    }

    let table = table_1ch(None); // no limits: we only care about timebase here
    let script = vec![
        MockBackend::const_block(&[1.0], 8, 5000, 0),
        MockBackend::const_block(&[1.0], 8, 5008, 0),
        // device re-enumerated: counter resets to 8, epoch bumps to 1
        MockBackend::const_block(&[1.0], 8, 8, 1),
    ];
    let mut backend = MockBackend::new(1).with_rate(1000.0).with_script(script);
    open_arm_start(&mut backend);

    let mut engine = Engine::new(engine_cfg_1ch(), &table, &backend);
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut proc = RecordingProc {
        events: events.clone(),
    };
    let mut sink = VecCaptureSink::new();

    let report = engine.run(&mut backend, &mut proc, &mut sink, 100);
    assert_eq!(report.reason, StopReason::InputEnded);
    assert_eq!(report.resets, 1, "one epoch bump detected");
    assert!(events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, TimebaseEvent::Reset { new_epoch: 1 })));
    // Capture recorded the epoch bump on the block metadata.
    assert!(sink.blocks.iter().any(|b| b.epoch == 1));
}

// ------------------------------------------------------------------------------------
// (c) counter gap -> dropped samples detected
// ------------------------------------------------------------------------------------
#[test]
fn counter_gap_detected() {
    let table = table_1ch(None);
    let script = vec![
        MockBackend::const_block(&[1.0], 8, 8, 0),
        // delivered 8 frames but the counter jumped to 100 (was 8): 84 dropped
        MockBackend::const_block(&[1.0], 8, 100, 0),
    ];
    let mut backend = MockBackend::new(1).with_rate(1000.0).with_script(script);
    open_arm_start(&mut backend);

    let mut engine = Engine::new(engine_cfg_1ch(), &table, &backend);
    let mut proc = SilentDrive { out_channels: 1 };
    let mut sink = VecCaptureSink::new();

    let report = engine.run(&mut backend, &mut proc, &mut sink, 100);
    assert_eq!(report.gaps, 1, "one dropped-sample gap detected");
    // The block following the gap is marked gap_before in the capture index.
    assert!(sink.blocks.iter().any(|b| b.gap_before));
}

// ------------------------------------------------------------------------------------
// (d) drive underrun -> UnderrunPolicy fires (ramp-down + abort)
// ------------------------------------------------------------------------------------
#[test]
fn drive_underrun_ramps_and_aborts() {
    // A process step that produces NO drive samples starves the output queue; once the
    // primed lookahead drains, the RampDownThenAbort policy fires.
    struct NoDrive;
    impl hertzforge_engine::ProcessStep for NoDrive {
        fn process(&mut self, _in: &[f32], _n: usize, out: &mut Vec<f32>) {
            out.clear(); // emit nothing: the DAC will run out of queued drive
        }
    }

    let table = table_1ch(None);
    // Long clean input so the loop keeps ticking while the drive queue drains.
    let script: Vec<ScriptedBlock> = (1..=20)
        .map(|i| MockBackend::const_block(&[1.0], 8, (i as u64) * 8, 0))
        .collect();
    let mut backend = MockBackend::new(1).with_rate(1000.0).with_script(script);
    open_arm_start(&mut backend);

    // out_channels 1, default contract min_lookahead 4 (from mock caps).
    let mut engine = Engine::new(engine_cfg_1ch(), &table, &backend);
    let mut proc = NoDrive;
    let mut sink = VecCaptureSink::new();

    let report = engine.run(&mut backend, &mut proc, &mut sink, 100);
    match report.reason {
        StopReason::DriveUnderrun(_) => {}
        other => panic!("expected DriveUnderrun, got {other:?}"),
    }
    // The underrun is an abort: emergency_stop was called and the shaker was ramped
    // (the ramp frames were written before the abort; see output::tests for the shape).
    assert!(
        backend.estop_count() >= 1,
        "underrun aborts via emergency_stop"
    );
    assert!(
        !backend.captured_drive.is_empty(),
        "ramp frames were delivered before the abort (no zero-cliff)"
    );
}

// ------------------------------------------------------------------------------------
// happy path: frames flow backend -> ring -> processing -> CaptureSink, drive flows back
// ------------------------------------------------------------------------------------
#[test]
fn run_stops_the_backend_but_run_ticks_leaves_it_running() {
    // The epilogue split (2026-07-30): `run` = one-shot module run, drive off at
    // the end; `run_ticks` = one segment of a CONTINUOUS session (the recorder's
    // monitor pump), where a per-segment backend stop kills the AO between
    // segments and puts a dropped-sample seam in gap-free acquisition. If the
    // epilogue creeps back into `run_ticks`, the second assertion reds.
    struct Silent;
    impl hertzforge_engine::ProcessStep for Silent {
        fn process(&mut self, _input: &[f32], n: usize, out: &mut Vec<f32>) {
            out.clear();
            out.resize(n, 0.0);
        }
    }
    let table = table_1ch(None);
    let script: Vec<ScriptedBlock> = (1..=2)
        .map(|i| MockBackend::const_block(&[0.5], 8, (i as u64) * 8, 0))
        .collect();

    let mut backend = MockBackend::new(1)
        .with_rate(1000.0)
        .with_script(script.clone());
    open_arm_start(&mut backend);
    let mut engine = Engine::new(engine_cfg_1ch(), &table, &backend);
    let report = engine.run_ticks(&mut backend, &mut Silent, &mut VecCaptureSink::new(), 2);
    assert_eq!(report.frames_in, 16);
    assert!(
        backend.is_started(),
        "run_ticks must leave the backend RUNNING for the next segment"
    );

    let mut backend = MockBackend::new(1).with_rate(1000.0).with_script(script);
    open_arm_start(&mut backend);
    let mut engine = Engine::new(engine_cfg_1ch(), &table, &backend);
    let _ = engine.run(&mut backend, &mut Silent, &mut VecCaptureSink::new(), 2);
    assert!(
        !backend.is_started(),
        "run's epilogue must still stop the backend (drive off at run end)"
    );
}

#[test]
fn happy_path_frames_flow_and_drive_returns() {
    let table = table_1ch(Some(1000.0)); // generous limit, never tripped
    let script: Vec<ScriptedBlock> = (1..=5)
        .map(|i| MockBackend::const_block(&[3.0], 8, (i as u64) * 8, 0))
        .collect();
    let mut backend = MockBackend::new(1).with_rate(1000.0).with_script(script);
    open_arm_start(&mut backend);

    // A process step that echoes input to drive so we can see drive flow back.
    struct Echo;
    impl hertzforge_engine::ProcessStep for Echo {
        fn process(&mut self, input: &[f32], n: usize, out: &mut Vec<f32>) {
            out.clear();
            out.extend_from_slice(&input[..n]); // 1:1 channels
        }
    }

    let mut engine = Engine::new(engine_cfg_1ch(), &table, &backend);
    let mut proc = Echo;
    let mut sink = VecCaptureSink::new();

    let report = engine.run(&mut backend, &mut proc, &mut sink, 100);

    assert_eq!(report.reason, StopReason::InputEnded);
    assert_eq!(report.estops, 0, "no abort on the happy path");
    assert_eq!(report.frames_in, 40, "5 blocks x 8 frames flowed in");
    assert_eq!(report.frames_captured, 40, "all frames reached the sink");
    assert!(sink.finalized, "sink was finalized");
    assert_eq!(sink.total_frames(), 40);
    // The interleaved capture holds the raw samples end to end.
    assert_eq!(sink.samples.len(), 40);
    assert!(sink.samples.iter().all(|&s| s == 3.0));
    // Drive flowed back to the backend (through the lookahead buffer).
    assert!(
        !backend.captured_drive.is_empty(),
        "drive frames were written back to the backend"
    );
}

// ------------------------------------------------------------------------------------
// MULTI-CHANNEL regression. Every test above this line is 1-channel, which is why the
// frames-vs-samples defect survived: at in_channels == 1 frames and samples are the same
// number, so the scheduler's `n_samples / frame_len` conversion was correct by accident.
// ------------------------------------------------------------------------------------

/// A 2-channel table: CH1 Control (carries the abort limit), CH2 Monitor.
fn table_2ch(accel_limit: Option<f64>) -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut c1 = Channel::new(1);
    c1.role = ChannelRole::Control;
    c1.abort = AbortLimits {
        accel_g: accel_limit,
        velocity_ips: None,
        displacement_mil: None,
    };
    t.channels.push(c1);
    let mut c2 = Channel::new(2);
    c2.role = ChannelRole::Monitor;
    c2.abort = AbortLimits {
        accel_g: accel_limit,
        velocity_ips: None,
        displacement_mil: None,
    };
    t.channels.push(c2);
    t
}

fn engine_cfg_2ch() -> EngineConfig {
    let mut cfg = EngineConfig::new(2, 1); // 2 in, 1 out
    cfg.frames_per_tick = 8;
    cfg.ring_frames = 256;
    cfg.watchdog_sustain = 1; // spike-sensitive
    cfg
}

/// The safety half of the regression: a breach in the TAIL of a multi-channel block must
/// still reach the watchdog.
///
/// The scheduler used to divide `read_input`'s FRAME count by `in_channels` to "get"
/// frames, so at 2 channels it published only the first 4 of every 8 frames to the raw
/// ring. A spike in frame 7 was never examined and never tripped. The block below is
/// clean for its first half and breaches only in the last frame, so it fails on the old
/// code (run ends `InputEnded`, no trip) and trips on the fixed code.
#[test]
fn multichannel_tail_breach_still_reaches_the_watchdog() {
    let table = table_2ch(Some(10.0));
    let spike_block = {
        let mut b = MockBackend::const_block(&[2.0, 2.0], 8, 8, 0);
        // Frame 7, channel 1 -> interleaved index 7*2 + 0 = 14. The final frame of the
        // block: invisible to a loop that stops at frame 4.
        b.samples[14] = 99.0;
        b
    };
    let mut backend = MockBackend::new(2)
        .with_rate(1000.0)
        .with_drive_channels(1) // 2 in, 1 out: write_drive accepts 1-wide frames
        .with_script(vec![spike_block]);
    open_arm_start(&mut backend);

    let mut engine = Engine::new(engine_cfg_2ch(), &table, &backend);
    let mut proc = SilentDrive { out_channels: 1 };
    let mut sink = VecCaptureSink::new();

    let report = engine.run(&mut backend, &mut proc, &mut sink, 100);

    assert!(
        matches!(report.reason, StopReason::AbortTripped(_)),
        "a 99 g breach in the LAST frame of a 2-channel block must trip the watchdog; \
         got {:?}. If this is InputEnded, the raw-ring loop is truncating the block again.",
        report.reason
    );
    assert_eq!(report.estops, 1, "the trip must have called emergency_stop");
}

/// The accounting half: frame and sample counts must both be right at N > 1 channels.
#[test]
fn multichannel_frame_and_sample_accounting() {
    let table = table_2ch(Some(1000.0)); // generous: never trips
    let script: Vec<ScriptedBlock> = (1..=5)
        .map(|i| MockBackend::const_block(&[3.0, 4.0], 8, (i as u64) * 8, 0))
        .collect();
    let mut backend = MockBackend::new(2)
        .with_rate(1000.0)
        .with_drive_channels(1) // 2 in, 1 out: write_drive accepts 1-wide frames
        .with_script(script);
    open_arm_start(&mut backend);

    let mut engine = Engine::new(engine_cfg_2ch(), &table, &backend);
    let mut proc = SilentDrive { out_channels: 1 };
    let mut sink = VecCaptureSink::new();

    let report = engine.run(&mut backend, &mut proc, &mut sink, 100);

    assert_eq!(report.reason, StopReason::InputEnded);
    // 5 blocks x 8 frames. The old code reported 20 here (40/2): frames divided by channels.
    assert_eq!(report.frames_in, 40, "5 blocks x 8 frames flowed in");
    assert_eq!(report.frames_captured, 40, "every frame reached the sink");
    assert_eq!(sink.total_frames(), 40);
    // The sink holds FRAMES x CHANNELS samples. The old code sliced `in_scratch[..n_samples]`
    // with n_samples holding a frame count, so it captured 40 samples instead of 80 and
    // silently dropped half the acquisition.
    assert_eq!(
        sink.samples.len(),
        80,
        "40 frames x 2 channels = 80 interleaved samples"
    );
    // Both channels' values must be present and interleaved, not just CH1's.
    assert!(
        sink.samples.chunks(2).all(|f| f[0] == 3.0 && f[1] == 4.0),
        "every captured frame must carry both channels in order"
    );
}
