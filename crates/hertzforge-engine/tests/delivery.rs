//! AO top-up delivery contract, hardware-free.
//!
//! The engine no longer writes one drive frame per input frame in lockstep. It
//! stages the tick's frames and tops up the backend's output buffer with as many
//! as `output_space()` allows, RETAINING what the backend refused for the next
//! tick. These tests pin the three properties that matter:
//!
//! 1. a refused frame is retained and re-offered IN ORDER, never dropped;
//! 2. a backend with no space semantics still gets the exact legacy contract;
//! 3. an output that never accepts is a FAULT, not an unbounded queue.

#[path = "../src/test_backend.rs"]
mod test_backend;

use hertzforge_channel_table::{AbortLimits, Channel, ChannelRole, ChannelTable};
use hertzforge_engine::{Engine, EngineConfig, StopReason, VecCaptureSink};
use hertzforge_hal::Backend;
use test_backend::{MockBackend, ScriptedBlock};

fn table_1ch() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut c = Channel::new(1);
    c.role = ChannelRole::Control;
    c.abort = AbortLimits::default(); // no limits: delivery is what is under test
    t.channels.push(c);
    t
}

fn cfg(frames_per_tick: usize) -> EngineConfig {
    let mut c = EngineConfig::new(1, 1);
    c.frames_per_tick = frames_per_tick;
    c.ring_frames = 1024;
    c
}

fn script(blocks: usize, frames: usize) -> Vec<ScriptedBlock> {
    (1..=blocks)
        .map(|i| MockBackend::const_block(&[1.0], frames, (i * frames) as u64, 0))
        .collect()
}

/// Emits a strictly increasing counter as drive, so the frames the backend
/// captured can be checked for ORDER, gaps and duplicates - the three ways a
/// retain-and-re-offer policy can go wrong.
struct CountingStep {
    next: f32,
}

impl hertzforge_engine::ProcessStep for CountingStep {
    fn process(&mut self, _input: &[f32], n_frames: usize, out: &mut Vec<f32>) {
        out.clear();
        for _ in 0..n_frames {
            self.next += 1.0;
            out.push(self.next);
        }
    }
}

/// The retained frames must come back out in order, with nothing dropped and
/// nothing repeated. The mock's output buffer drains at HALF the block rate, so
/// it is guaranteed to refuse part of most blocks.
///
/// RED-MUTATION: drop the unaccepted tail instead of retaining it (replace the
/// `self.staged_out.drain(..accepted * out_ch)` with a full `clear()`) and this
/// test reds on the gap check - the captured drive skips the refused frames.
#[test]
fn refused_drive_frames_are_retained_and_re_offered_in_order() {
    let table = table_1ch();
    let mut backend = MockBackend::new(1)
        .with_rate(1000.0)
        .with_output_buffer(12, 4) // 12-frame AO buffer draining 4 frames per block
        .with_script(script(8, 8));
    backend.open().unwrap();
    backend.arm().unwrap();
    backend.start().unwrap();

    let mut engine = Engine::new(cfg(8), &table, &backend);
    let mut proc = CountingStep { next: 0.0 };
    let mut sink = VecCaptureSink::new();
    let report = engine.run(&mut backend, &mut proc, &mut sink, 100);

    assert_eq!(
        report.reason,
        StopReason::InputEnded,
        "backpressure is not a fault; the run ends on the script, not on the AO buffer"
    );

    // The prefill zeros come first, then the counter stream. Strip the zeros.
    let captured = &backend.captured_drive;
    let first_nonzero = captured
        .iter()
        .position(|&v| v != 0.0)
        .expect("some real drive was delivered");
    let stream = &captured[first_nonzero..];
    assert!(
        !stream.is_empty(),
        "the top-up must deliver real drive, not only the prefill"
    );
    // Exactly 1.0, 2.0, 3.0, ... - in order, no gaps, no repeats.
    for (i, &v) in stream.iter().enumerate() {
        assert_eq!(
            v,
            (i + 1) as f32,
            "drive frame {i} out of order / dropped / repeated: got {v}"
        );
    }
    // And backpressure genuinely happened: the device could not take everything
    // the loop produced, so fewer frames landed than were generated. Without this
    // the test could pass vacuously against a backend that never refused.
    assert!(
        (stream.len() as u64) < report.frames_in,
        "setup invalid: the AO buffer never applied backpressure ({} delivered of {} produced)",
        stream.len(),
        report.frames_in
    );
}

/// A backend with no `output_space()` gets the legacy lockstep contract: every
/// offered frame must be accepted. The visible change is the CALL SHAPE - one
/// `write_drive` per tick carrying the whole block, not one per frame.
#[test]
fn lockstep_backend_keeps_its_contract_with_one_write_per_tick() {
    let table = table_1ch();
    let mut backend = MockBackend::new(1)
        .with_rate(1000.0)
        .with_script(script(5, 8)); // no output buffer: output_space() is None
    backend.open().unwrap();
    backend.arm().unwrap();
    backend.start().unwrap();

    let mut engine = Engine::new(cfg(8), &table, &backend);
    let mut proc = CountingStep { next: 0.0 };
    let mut sink = VecCaptureSink::new();
    let report = engine.run(&mut backend, &mut proc, &mut sink, 100);

    assert_eq!(report.reason, StopReason::InputEnded);
    assert_eq!(report.frames_in, 40);
    // 5 delivering ticks + 1 prefill call. The retired lockstep made 8 calls per
    // tick (one per frame); the top-up makes one per tick with the block.
    assert_eq!(
        backend.write_calls, 6,
        "one prefill write + one top-up write per delivering tick"
    );
    // A lockstep backend accepts everything OFFERED, and the engine offers one
    // frame per input frame. The 4 frames still sitting in the queue when the
    // input ended ARE the contracted lookahead - the queue is primed with 4 zero
    // frames at construction, so the counter stream runs exactly that far behind.
    // (Under the retired per-frame lockstep the arithmetic was identical; only the
    // number of `write_drive` calls changed.)
    let delivered = backend.captured_drive.iter().filter(|&&v| v != 0.0).count();
    assert_eq!(
        delivered,
        40 - 4,
        "frames_in minus the contracted lookahead"
    );
}

/// An output that stops accepting is a stuck device, not a slow one. Staging is
/// bounded, and blowing the bound is a faulted stop with `emergency_stop` called -
/// never an ever-deeper queue with the drive falling silently further behind.
///
/// RED-MUTATION: remove the `max_staged_frames` check at the end of
/// `deliver_drive` and this test reds - the run ends `InputEnded` with the drive
/// silently queued in host memory.
#[test]
fn an_output_that_never_accepts_is_a_faulted_stop() {
    let table = table_1ch();
    let mut backend = MockBackend::new(1)
        .with_rate(1000.0)
        .with_output_buffer(4, 0) // 4 frames of room, never drains
        .with_script(script(60, 8));
    backend.open().unwrap();
    backend.arm().unwrap();
    backend.start().unwrap();

    let mut engine = Engine::new(cfg(8), &table, &backend);
    let mut proc = CountingStep { next: 0.0 };
    let mut sink = VecCaptureSink::new();
    let report = engine.run(&mut backend, &mut proc, &mut sink, 200);

    match &report.reason {
        StopReason::BackendError(m) => assert!(
            m.contains("drive output stalled"),
            "expected the stuck-output message, got: {m}"
        ),
        other => panic!("expected a stalled-output BackendError, got {other:?}"),
    }
    assert!(
        backend.estop_count() >= 1,
        "a stuck output aborts through emergency_stop"
    );
}

// ---------------------------------------------------------------------------
// The AO-starvation negative control, in the two NI shapes
// ---------------------------------------------------------------------------
//
// `an_output_that_never_accepts_is_a_faulted_stop` (above) is the generic
// coverage and it stands: a `with_output_buffer(4, 0)` mock accepts 4 frames
// once, then reports `Some(0)` forever, and the run ends
// `BackendError("drive output stalled")` with `emergency_stop` called.
//
// What it does NOT cover is the two shapes `hertzforge-hal-ni` can actually produce,
// and in both of them the engine's behaviour differs from that test's path:
//
//   1. `output_space()` answers `Some(0)` from the FIRST tick. `NiBackend::
//      output_space` does exactly this whenever `query_write_space()` fails
//      (backend.rs: "reporting no space"). Then `offer == 0` and the engine's
//      `if offer > 0` guard means `write_drive` is never called AT ALL - so
//      every liar-check and short-accept branch is skipped and only the
//      staging-depth cap is left standing.
//   2. `write_drive` answers `Ok(0)` while `output_space()` still reports room.
//      `NiBackend::write_drive` produces this on a DAQmx timeout, which it
//      treats as honest backpressure rather than a fault - so a device whose AO
//      has stopped consuming refuses forever and returns no error at any point.
//
// Both must fault VISIBLY. The alternative on real hardware is not a hang: the
// AO buffer holds whatever was last written, and a REGENERATING output would
// replay it into a shaker indefinitely while every plot stayed alive. That is
// why `NiBackend` sets `DO_NOT_ALLOW_REGEN` and refuses to open if the readback
// disagrees; these tests are the engine-side half of the same guarantee.

/// Shape 1: `output_space()` is `Some(0)` from tick zero, so `write_drive` is
/// never even called. The starvation is caught anyway.
///
/// RED-MUTATION: remove the `max_staged_frames` check at the end of
/// `deliver_drive` and this reds - and note it reds HARDER than that generic test,
/// because here `write_drive` is never called, so nothing else in the delivery
/// path could have noticed.
#[test]
fn an_output_that_reports_no_space_from_the_first_tick_still_faults() {
    let table = table_1ch();
    let mut backend = MockBackend::new(1)
        .with_rate(1000.0)
        .with_output_buffer(4, 0)
        .with_output_full() // Some(0) before a single frame is offered
        .with_script(script(60, 8));
    backend.open().unwrap();
    backend.arm().unwrap();
    backend.start().unwrap();
    assert_eq!(
        backend.output_space(),
        Some(0),
        "the premise: no space, from the start"
    );

    let mut engine = Engine::new(cfg(8), &table, &backend);
    let mut proc = CountingStep { next: 0.0 };
    let mut sink = VecCaptureSink::new();
    let report = engine.run(&mut backend, &mut proc, &mut sink, 200);

    match &report.reason {
        StopReason::BackendError(m) => assert!(
            m.contains("drive output stalled"),
            "expected the stuck-output message, got: {m}"
        ),
        other => panic!("expected a stalled-output BackendError, got {other:?}"),
    }
    assert!(
        backend.estop_count() >= 1,
        "a starved output aborts through emergency_stop"
    );
    // The run must NOT have reached the end of the 60-block script: a stop that
    // arrives only when the input runs out is not a starvation detector.
    assert!(
        !backend.script_done(),
        "the fault must arrive while data is still flowing, not at end-of-script"
    );
    // ANTI-VACUITY: nothing was delivered, so nothing was replayed either. The
    // engine did not quietly hand the device the same frames again.
    assert!(
        backend.captured_drive.is_empty(),
        "an output with no space must receive nothing, not a repeat"
    );
}

/// Shape 2: `write_drive` answers `Ok(0)` forever while `output_space()` reports
/// room - the DAQmx-timeout signature, where every call succeeds and nothing
/// moves.
///
/// RED-MUTATION: remove the `max_staged_frames` check and this reds with the run
/// ending `InputEnded` - i.e. a CLEAN stop after a run in which the shaker was
/// commanded nothing at all. That is the failure this control exists to make
/// impossible: not a crash, a clean report over a dead drive.
#[test]
fn an_output_that_accepts_zero_while_claiming_space_is_a_faulted_stop() {
    let table = table_1ch();
    let mut backend = MockBackend::new(1)
        .with_rate(1000.0)
        .with_output_buffer(64, 8) // plenty of room, and it drains
        .refusing_all_writes() // ...but every write takes nothing
        .with_script(script(60, 8));
    backend.open().unwrap();
    backend.arm().unwrap();
    backend.start().unwrap();
    assert!(
        backend.output_space().unwrap() > 0,
        "the premise: the device CLAIMS room"
    );

    let mut engine = Engine::new(cfg(8), &table, &backend);
    let mut proc = CountingStep { next: 0.0 };
    let mut sink = VecCaptureSink::new();
    let report = engine.run(&mut backend, &mut proc, &mut sink, 200);

    match &report.reason {
        StopReason::BackendError(m) => assert!(
            m.contains("drive output stalled"),
            "expected the stuck-output message, got: {m}"
        ),
        other => panic!("an Ok(0)-forever write must fault, not run to completion: {other:?}"),
    }
    assert!(backend.estop_count() >= 1);
    assert!(
        !backend.script_done(),
        "the fault must arrive while data is still flowing"
    );
    // The engine DID try: unlike shape 1 it called `write_drive` every tick, and
    // each call returned Ok. A test that could not tell "tried and was refused"
    // from "never tried" would pass on a backend that was never asked.
    assert!(
        backend.write_calls > 1,
        "the engine must have offered repeatedly, got {} call(s)",
        backend.write_calls
    );
    assert!(
        backend.captured_drive.is_empty(),
        "nothing was accepted, so nothing may have been captured"
    );
}

/// The device-side prefill: before the first block is processed, the backend has
/// the contracted lookahead of benign ZERO drive already in hand.
///
/// RED-MUTATION: make `prefill_output` a no-op and this test reds - the backend
/// sees nothing until the first processed block arrives.
#[test]
fn prefill_puts_the_contracted_lookahead_in_the_device_before_start() {
    let table = table_1ch();
    let mut backend = MockBackend::new(1)
        .with_rate(1000.0)
        .with_output_buffer(64, 0)
        .with_script(script(2, 8));
    backend.open().unwrap();
    backend.arm().unwrap();

    let mut engine = Engine::new(cfg(8), &table, &backend);
    let sent = engine
        .prefill_output(&mut backend)
        .expect("prefill must not fault");
    // The mock's contract says min_lookahead_frames = 4.
    assert_eq!(sent, 4, "the whole contracted lookahead was prefilled");
    assert_eq!(backend.output_filled(), 4);
    assert!(
        backend.captured_drive.iter().all(|&v| v == 0.0),
        "the prefill is benign ZERO drive, never a guess at the first block"
    );
    // Idempotent: a caller that also lets `run()` prefill does not double up.
    assert_eq!(engine.prefill_output(&mut backend).unwrap(), 0);
    assert_eq!(backend.output_filled(), 4);
}
