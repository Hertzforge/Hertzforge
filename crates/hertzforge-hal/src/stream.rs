//! BLOCK-WISE drive/response streaming over a [`Backend`].
//!
//! The direct-backend players - TWR's `BackendReferencePlayer`, SRS's
//! `HalPulsePlant`, classical shock's `BackendPulseFirer` - each play a finished
//! drive RECORD into a backend and capture the response, without going through
//! `hertzforge_engine::Engine`. All three did it ONE SAMPLE PER CALL, and all three
//! carried a comment blaming the `SimBackend` per-block drive latch. That latch is
//! gone (the Sim buffers the full per-frame drive stream), and
//! one-sample-per-call is not a shape any real driver wants: an NI-DAQmx write of
//! one frame per call is a syscall per sample.
//!
//! This module is the ONE implementation of the block-wise contract, so the three
//! players agree on it rather than each growing their own:
//!
//! * **Write honors the ACCEPTED COUNT.** `write_drive` returns how many frames it
//!   took, which may be fewer than offered. The remainder is RETAINED and
//!   re-offered - never dropped, never re-ordered. Dropping a drive frame is a
//!   discontinuity at the shaker.
//! * **Read POLLS on `NoDataYet`.** A continuous acquisition legitimately has "no
//!   frames ready this poll" as a steady-state answer; the old code treated it as
//!   an instant fatal. It is now a bounded retry against a deadline, and only
//!   exceeding the deadline is fatal.
//! * **`Ended` mid-play stays FATAL.** The stream promised a record's worth of
//!   response and stopped early; there is no honest way to continue.
//! * **Only DELIVERED frames are handed to the caller.** The frame buffer is never
//!   passed on unless `read_input` actually wrote into it. That is the
//!   stale-buffer-as-measurement bug class, and this is the structural fix for it.
//!
//! Reads and writes INTERLEAVE, one block at a time, so a bounded device buffer
//! cannot deadlock the play: whatever the device accepted is read back (freeing
//! room) before the remainder is re-offered.

use crate::{Backend, HalError, InputRead};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Default deadline for one `read_input` that keeps answering `NoDataYet`.
pub const DEFAULT_READ_DEADLINE: Duration = Duration::from_secs(2);

/// Default pause between `NoDataYet` polls. Small enough to stay responsive on a
/// block-cadence device, large enough not to spin a core.
pub const DEFAULT_READ_POLL_INTERVAL: Duration = Duration::from_micros(200);

/// How a block-wise play failed.
#[derive(Debug)]
pub enum StreamError {
    /// The backend refused a read or a write.
    Hal(HalError),
    /// `read_input` reported `Ended` before the record was played out. FATAL by
    /// design: a stream that promised a record's worth of response and stopped
    /// early cannot be continued honestly.
    EndedMidPlay {
        frames_done: usize,
        frames_expected: usize,
    },
    /// `read_input` stayed `NoDataYet` past the deadline.
    ReadStalled {
        frames_done: usize,
        waited: Duration,
    },
    /// `write_drive` accepted NOTHING while nothing was in flight to read back and
    /// free room. The output is stuck, not merely slow.
    OutputStuck { frames_done: usize },
    /// `read_input` claimed MORE frames than the buffer it was handed can hold.
    /// A contract violation; trusting it would read past the written region and
    /// hand the caller uninitialized or stale samples as measurement.
    OverClaimedFrames { claimed: usize, capacity: usize },
    /// The caller's `cancel` flag was observed TRUE at a block boundary, so no
    /// further drive was offered and the play returned early.
    ///
    /// **The frames the device already ACCEPTED KEEP PLAYING.** This is a
    /// hardware bound, not a policy choice: mid-record preemption of a committed
    /// DAQmx output is REFUSED by the cDAQ-9178 (`-200289`), so the committed
    /// queue - `frames_committed - frames_done` frames still ahead of the read
    /// cursor - is what bounds how long drive remains on the wire after the flag
    /// flips. THE CALLER DECIDES what happens to that tail: `emergency_stop()`
    /// on the backend to taper and kill the task (shock's ABORT), or simply let
    /// the queue drain and then `stop()` (shock's STOP). This variant reports the
    /// arithmetic; it does not perform either.
    Cancelled {
        /// Response frames delivered to `on_input` before the cancel was seen.
        frames_done: usize,
        /// Frames the DEVICE accepted - the ones that keep playing.
        frames_committed: usize,
        /// Frames in the record that was being played.
        frames_expected: usize,
    },
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamError::Hal(e) => write!(f, "{e}"),
            StreamError::EndedMidPlay {
                frames_done,
                frames_expected,
            } => write!(
                f,
                "input stream ENDED mid-play after {frames_done} of {frames_expected} frames"
            ),
            StreamError::ReadStalled {
                frames_done,
                waited,
            } => write!(
                f,
                "input stalled after {frames_done} frames: no data for {waited:?}"
            ),
            StreamError::OutputStuck { frames_done } => write!(
                f,
                "drive output accepted nothing after {frames_done} frames with none in flight"
            ),
            StreamError::OverClaimedFrames { claimed, capacity } => write!(
                f,
                "read_input claimed {claimed} frames into a {capacity}-frame buffer"
            ),
            StreamError::Cancelled {
                frames_done,
                frames_committed,
                frames_expected,
            } => write!(
                f,
                "play CANCELLED after {frames_done} of {frames_expected} frames; \
                 {} committed frame(s) still queued at the device",
                frames_committed.saturating_sub(*frames_done)
            ),
        }
    }
}

impl std::error::Error for StreamError {}

impl From<HalError> for StreamError {
    fn from(e: HalError) -> Self {
        StreamError::Hal(e)
    }
}

/// Knobs for [`play_block_wise`].
#[derive(Debug, Clone, Copy)]
pub struct BlockPlay {
    /// Frames per write/read block. Comes from the configure seam
    /// (`ConfiguredActual::block_frames`), never invented at the call site.
    pub block_frames: usize,
    /// How long one read may keep answering `NoDataYet` before it is a stall.
    pub read_deadline: Duration,
    /// Pause between `NoDataYet` polls (`ZERO` spins).
    pub poll_interval: Duration,
}

impl BlockPlay {
    /// A block play at `block_frames` with the default read-stall policy.
    pub fn new(block_frames: usize) -> Self {
        BlockPlay {
            block_frames: block_frames.max(1),
            read_deadline: DEFAULT_READ_DEADLINE,
            poll_interval: DEFAULT_READ_POLL_INTERVAL,
        }
    }

    pub fn with_read_deadline(mut self, d: Duration) -> Self {
        self.read_deadline = d;
        self
    }

    pub fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }
}

/// Play `drive` (interleaved, `n_drive` per frame) into `backend` BLOCK-WISE and
/// hand every delivered input block to `on_input`, which receives
/// `(interleaved_frames, n_frames)` with `n_frames * n_inputs` samples.
///
/// `scratch` is the caller's input buffer; it must hold at least
/// `play.block_frames * n_inputs` samples and is reused for every block, so a
/// steady-state play allocates nothing.
///
/// `committed_frames` is how many frames from the HEAD of `drive` the device has
/// ALREADY accepted before this call - the per-shot cold-task shape, where the
/// caller commits the whole record and only then calls `start()` so the backend's
/// own start-time silence top-up lands BEHIND the record instead of ahead of the
/// leading edge. It seeds the internal written-counter, which is the one thing a
/// caller could not previously say: this function pairs its reads with its OWN
/// writes, so handed the full record it re-sent the committed prefix, and handed
/// only the remainder it read back `committed` frames too few and truncated the
/// response window by exactly the part that was prefilled. `0` is the plain case
/// (nothing pre-committed) and is byte-identical to the behavior before the
/// parameter existed. It also makes [`StreamError::Cancelled`]'s
/// `frames_committed` whole: the pre-start commit is part of what keeps driving
/// after the flag flips, and understating it is the dangerous direction.
///
/// `cancel`, when `Some`, is read ONCE per block-loop iteration, BEFORE the next
/// block of drive is offered. Observed `true` stops the play and returns
/// [`StreamError::Cancelled`] - read that variant's docs before deciding what to
/// do with the drive the device has already committed. `None` is byte-identical
/// to the pre-cancellation behavior: the flag is never read and no branch is
/// taken beyond one `Option` test per block.
///
/// Returns the number of response FRAMES delivered, which equals
/// `drive.len() / n_drive` on success.
// 9 arguments, two over clippy's 7. The alternative is folding `cancel` into
// `BlockPlay`, which is `Copy` and lives at the call sites as a value - a
// borrowed `&AtomicBool` inside it would put a lifetime on the knobs struct and
// force every existing caller to name it. The knobs describe the PLAY POLICY
// (block size, deadlines); the cancel flag is a live channel from the caller's
// other thread and `committed_frames` is per-PLAY state, true of this record and
// no other. Keeping them apart is the honest split, so the lint is allowed here
// rather than the seam bent around it.
#[allow(clippy::too_many_arguments)]
pub fn play_block_wise(
    backend: &mut dyn Backend,
    drive: &[f32],
    n_drive: usize,
    n_inputs: usize,
    scratch: &mut [f32],
    play: &BlockPlay,
    committed_frames: usize,
    cancel: Option<&AtomicBool>,
    mut on_input: impl FnMut(&[f32], usize),
) -> Result<usize, StreamError> {
    let n_drive = n_drive.max(1);
    let n_inputs = n_inputs.max(1);
    let block = play
        .block_frames
        .max(1)
        .min(scratch.len() / n_inputs)
        .max(1);
    let total = drive.len() / n_drive;
    // Frames the device ACCEPTED - SEEDED with what the caller already committed
    // before `start()`. Clamped to the record because a caller cannot have
    // committed more of this record than exists.
    let mut written = committed_frames.min(total);
    let mut read = 0usize; // frames read back
    let mut stuck_since: Option<std::time::Instant> = None;

    while read < total {
        // --- CANCEL: one check per block, BEFORE any more drive is offered. ---
        // Checking here and nowhere else is what makes the cancel latency a
        // BLOCK, and the block is the only granularity this hardware honors
        // (mid-record preemption is refused, `-200289`). What is already
        // committed keeps playing; see `StreamError::Cancelled`.
        if let Some(flag) = cancel {
            if flag.load(Ordering::Relaxed) {
                return Err(StreamError::Cancelled {
                    frames_done: read,
                    frames_committed: written,
                    frames_expected: total,
                });
            }
        }

        // --- TOP UP: refill the device GREEDILY, then read one block. ---
        // One-block-per-iteration was measured underrunning on the chassis
        // (bench session, 2026-08-08): iterations are READ-paced (~one block of
        // wall time each), so a single-block offer makes supply exactly equal
        // consumption with ZERO recovery margin - any transient deficit
        // persists until it accumulates past the buffer, and an over-buffer
        // record underran for ~2049 frames at the committed-head boundary
        // (measured as a -2049-frame shift of the record's tail on the xcorr
        // row, r degraded 1.00 -> 0.97). A greedy writer refills to the
        // device's space every iteration, restoring the margin the pre-start
        // commit established.
        if written < total {
            let mut accepted_any = 0usize;
            loop {
                let want = (total - written).min(block);
                if want == 0 {
                    break;
                }
                let accepted =
                    backend.write_drive(&drive[written * n_drive..(written + want) * n_drive])?;
                // A backend claiming more than it was offered is a contract
                // violation; trusting it would walk `written` past the record.
                written += accepted.min(want);
                accepted_any += accepted.min(want);
                if accepted < want {
                    break; // device full (or refusing) - stop offering
                }
            }
            let accepted = accepted_any;
            if accepted == 0 && written == read {
                // Nothing accepted and nothing of OURS in flight. On a
                // driver-paced device this is a normal TRANSIENT, not a stuck
                // output: the buffer can be full of frames this function never
                // wrote (the backend's own start prefill, an arm-time flush)
                // and space opens only as the device plays them out. Declaring
                // OutputStuck on the first zero-accept was measured doing
                // exactly that on the real cDAQ-9178 (gate-6 sweep,
                // 2026-07-30): TWR and SRS both died ~5 ms into a play while
                // the AO was busy draining prefill silence. The stuck test is
                // therefore TIME-bounded, symmetric with the read-stall
                // deadline: give the device `read_deadline` of wall clock to
                // open room, polling at `poll_interval`, and only a window
                // with NO acceptance is a stuck output.
                let since = *stuck_since.get_or_insert_with(std::time::Instant::now);
                if since.elapsed() >= play.read_deadline {
                    return Err(StreamError::OutputStuck { frames_done: read });
                }
                if !play.poll_interval.is_zero() {
                    std::thread::sleep(play.poll_interval);
                }
                continue;
            }
            if accepted > 0 {
                stuck_since = None;
            }
        }

        // --- READ BACK: only what the device has actually been given. ---
        let want_read = (written - read).min(block);
        if want_read == 0 {
            continue; // the top-up above guarantees progress or an error
        }
        let got = poll_read(
            backend,
            &mut scratch[..want_read * n_inputs],
            n_inputs,
            play,
            read,
            total,
        )?;
        // ONLY delivered frames reach the caller. `read_input` wrote exactly `got`
        // frames into `scratch`; everything past that is last block's data, and
        // handing it on is the stale-buffer-as-measurement bug.
        on_input(&scratch[..got * n_inputs], got);
        read += got;
    }
    Ok(read)
}

/// One read with bounded `NoDataYet` polling. `Ended` is fatal.
fn poll_read(
    backend: &mut dyn Backend,
    out: &mut [f32],
    n_inputs: usize,
    play: &BlockPlay,
    frames_done: usize,
    frames_expected: usize,
) -> Result<usize, StreamError> {
    let capacity = out.len() / n_inputs;
    let start = Instant::now();
    loop {
        match backend.read_input(out)? {
            InputRead::Frames(n) if n > 0 => {
                // A backend claiming more frames than the buffer holds would have
                // the caller read past the written region - stale or uninitialized
                // samples handed on as measurement. Same guard the engine's read
                // path carries, for the same reason.
                if n > capacity {
                    return Err(StreamError::OverClaimedFrames {
                        claimed: n,
                        capacity,
                    });
                }
                return Ok(n);
            }
            // `Frames(0)` is a contract wart; the contract says use `NoDataYet`.
            InputRead::Frames(_) | InputRead::NoDataYet => {
                let waited = start.elapsed();
                if waited > play.read_deadline {
                    return Err(StreamError::ReadStalled {
                        frames_done,
                        waited,
                    });
                }
                if play.poll_interval.is_zero() {
                    std::hint::spin_loop();
                } else {
                    std::thread::sleep(play.poll_interval);
                }
            }
            InputRead::Ended => {
                return Err(StreamError::EndedMidPlay {
                    frames_done,
                    frames_expected,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AcquisitionConfig, BackendStatus, Caps, ClockDomain, ConfiguredActual, LoopOwner};
    use hertzforge_channel_table::ChannelTable;

    /// A scriptable one-in/one-out backend: `accept_per_write` caps what
    /// `write_drive` takes, `no_data_before_frames` makes the first N reads answer
    /// `NoDataYet`, `end_after` makes it report `Ended` mid-play.
    struct Scripted {
        buffered: Vec<f32>,
        /// Cap on SAMPLES accepted per `write_drive` call.
        accept_per_write: usize,
        no_data_countdown: usize,
        end_after_reads: Option<usize>,
        reads: usize,
        /// Channel count on BOTH sides (this double is symmetric).
        n_ch: usize,
    }

    impl Scripted {
        fn new(accept_per_write: usize) -> Self {
            Scripted {
                buffered: Vec::new(),
                accept_per_write,
                no_data_countdown: 0,
                end_after_reads: None,
                reads: 0,
                n_ch: 1,
            }
        }

        fn with_channels(mut self, n: usize) -> Self {
            self.n_ch = n;
            self
        }
    }

    impl Backend for Scripted {
        fn caps(&self) -> Caps {
            Caps {
                loop_owner: LoopOwner::HostClosesLoop,
                native_modes: vec![],
                clock: ClockDomain::Shared,
                max_input_channels: 1,
                max_output_channels: 1,
                abort_latency_bound: None,
                output_buffer: None,
                sample_rate_hz: 1000.0,
                delegated_response_observable: false,
                clock_groups: vec![],
            }
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
            self.reads += 1;
            if let Some(limit) = self.end_after_reads {
                if self.reads > limit {
                    return Ok(InputRead::Ended);
                }
            }
            if self.no_data_countdown > 0 {
                self.no_data_countdown -= 1;
                return Ok(InputRead::NoDataYet);
            }
            // FRAMES, not samples: `n_ch` interleaved samples make one frame, and
            // `InputRead::Frames` names the unit.
            let n_ch = self.n_ch.max(1);
            let frames = (out.len() / n_ch).min(self.buffered.len() / n_ch);
            if frames == 0 {
                return Ok(InputRead::NoDataYet);
            }
            // Response = the buffered drive, doubled (a gain-2 loopback).
            for (i, v) in self.buffered.drain(..frames * n_ch).enumerate() {
                out[i] = v * 2.0;
            }
            Ok(InputRead::Frames(frames))
        }
        fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
            let n_ch = self.n_ch.max(1);
            // Accept WHOLE frames only, capped by `accept_per_write` samples.
            let frames = (drive.len() / n_ch).min(self.accept_per_write / n_ch);
            self.buffered.extend_from_slice(&drive[..frames * n_ch]);
            Ok(frames)
        }
        fn poll_status(&mut self) -> BackendStatus {
            BackendStatus::default()
        }
        fn emergency_stop(&mut self) -> Result<(), HalError> {
            Ok(())
        }
    }

    fn drive_ramp(n: usize) -> Vec<f32> {
        (1..=n).map(|i| i as f32).collect()
    }

    #[test]
    fn a_whole_record_streams_in_order_at_block_size() {
        let mut b = Scripted::new(usize::MAX);
        let drive = drive_ramp(1000);
        let mut scratch = vec![0.0f32; 64];
        let mut got = Vec::new();
        let n = play_block_wise(
            &mut b,
            &drive,
            1,
            1,
            &mut scratch,
            &BlockPlay::new(64),
            0,
            None,
            |blk, frames| {
                assert_eq!(blk.len(), frames);
                got.extend_from_slice(blk);
            },
        )
        .unwrap();
        assert_eq!(n, 1000);
        let expect: Vec<f32> = drive.iter().map(|d| d * 2.0).collect();
        assert_eq!(got, expect, "response aligned sample-for-sample with drive");
    }

    /// RED-MUTATION: ignore `write_drive`'s return and advance by the OFFERED
    /// count instead of the accepted one, and this test reds - the refused tail is
    /// skipped and the response is short and misaligned.
    #[test]
    fn a_partial_write_retains_the_remainder() {
        // Accepts only 5 samples per call against a 64-frame block.
        let mut b = Scripted::new(5);
        let drive = drive_ramp(101); // not a multiple of anything convenient
        let mut scratch = vec![0.0f32; 64];
        let mut got = Vec::new();
        let n = play_block_wise(
            &mut b,
            &drive,
            1,
            1,
            &mut scratch,
            &BlockPlay::new(64),
            0,
            None,
            |blk, _| got.extend_from_slice(blk),
        )
        .unwrap();
        assert_eq!(n, 101);
        let expect: Vec<f32> = drive.iter().map(|d| d * 2.0).collect();
        assert_eq!(got, expect, "nothing dropped, nothing re-ordered");
    }

    /// RED-MUTATION: make `NoDataYet` fatal again (the pre-item-5 behavior) and
    /// this test reds - a stream that was merely not ready yet kills the play.
    #[test]
    fn no_data_yet_is_polled_not_fatal() {
        let mut b = Scripted::new(usize::MAX);
        b.no_data_countdown = 5; // five empty polls before anything arrives
        let drive = drive_ramp(32);
        let mut scratch = vec![0.0f32; 32];
        let mut frames = 0;
        let n = play_block_wise(
            &mut b,
            &drive,
            1,
            1,
            &mut scratch,
            &BlockPlay::new(32).with_poll_interval(Duration::ZERO),
            0,
            None,
            |_, f| frames += f,
        )
        .unwrap();
        assert_eq!(n, 32);
        assert_eq!(frames, 32);
        assert!(b.reads > 5, "the empty polls really happened");
    }

    /// RED-MUTATION: drop the deadline check in `poll_read` and this test hangs
    /// instead of failing - an unbounded poll is not a policy.
    #[test]
    fn an_endless_no_data_yet_is_a_bounded_stall_error() {
        let mut b = Scripted::new(usize::MAX);
        b.no_data_countdown = usize::MAX;
        let drive = drive_ramp(8);
        let mut scratch = vec![0.0f32; 8];
        let err = play_block_wise(
            &mut b,
            &drive,
            1,
            1,
            &mut scratch,
            &BlockPlay::new(8)
                .with_read_deadline(Duration::from_millis(20))
                .with_poll_interval(Duration::from_millis(1)),
            0,
            None,
            |_, _| {},
        )
        .expect_err("an endless NoDataYet must fail, not hang");
        assert!(matches!(err, StreamError::ReadStalled { .. }), "{err:?}");
    }

    /// `Ended` mid-play stays FATAL: the stream promised a record's worth of
    /// response and stopped early.
    #[test]
    fn ended_mid_play_is_fatal() {
        let mut b = Scripted::new(usize::MAX);
        b.end_after_reads = Some(1);
        let drive = drive_ramp(64);
        let mut scratch = vec![0.0f32; 8];
        let err = play_block_wise(
            &mut b,
            &drive,
            1,
            1,
            &mut scratch,
            &BlockPlay::new(8),
            0,
            None,
            |_, _| {},
        )
        .expect_err("Ended mid-play is fatal");
        match err {
            StreamError::EndedMidPlay {
                frames_done,
                frames_expected,
            } => {
                assert_eq!(frames_expected, 64);
                assert!(frames_done < 64);
            }
            other => panic!("expected EndedMidPlay, got {other:?}"),
        }
    }

    /// An output that accepts nothing at all, with nothing in flight, is stuck -
    /// bounded error, not an infinite loop.
    #[test]
    fn an_output_that_accepts_nothing_is_a_bounded_error() {
        let mut b = Scripted::new(0);
        let drive = drive_ramp(16);
        let mut scratch = vec![0.0f32; 16];
        let err = play_block_wise(
            &mut b,
            &drive,
            1,
            1,
            &mut scratch,
            &BlockPlay::new(16),
            0,
            None,
            |_, _| {},
        )
        .expect_err("a stuck output must fail");
        assert!(matches!(err, StreamError::OutputStuck { .. }), "{err:?}");
    }

    /// Multi-channel: the frame arithmetic must use FRAMES on both sides.
    #[test]
    fn multichannel_frames_are_counted_as_frames() {
        let mut b = Scripted::new(usize::MAX).with_channels(2);
        // 2 drive channels, 2 input channels, 10 frames.
        let drive: Vec<f32> = (0..20).map(|i| i as f32).collect();
        let mut scratch = vec![0.0f32; 4 * 2];
        let mut frames = 0usize;
        let n = play_block_wise(
            &mut b,
            &drive,
            2,
            2,
            &mut scratch,
            &BlockPlay::new(4),
            0,
            None,
            |blk, f| {
                assert_eq!(blk.len(), f * 2, "block is n_frames x n_inputs samples");
                frames += f;
            },
        )
        .unwrap();
        assert_eq!(n, 10, "20 samples of 2 channels is 10 FRAMES");
        assert_eq!(frames, 10);
    }

    /// A cancel flag raised MID-PLAY stops the play within one block.
    ///
    /// RED-MUTATION, run 2026-08-08: deleting the `if let Some(flag) = cancel`
    /// block at the top of `play_block_wise`'s loop reds this test on the
    /// `expect_err` - the play runs to completion and returns `Ok(256)`
    /// ("a raised cancel flag must end the play: 256").
    ///
    /// The `blocks_delivered < TOTAL_BLOCKS` assert below is a SECOND, weaker
    /// mutation's catcher, and it is what makes this a LATENCY test rather than
    /// a "the variant exists" test: a check that is present but placed where it
    /// runs only once, or only after the whole record has been offered, still
    /// returns `Cancelled` and still satisfies every other assert here. It
    /// cannot satisfy that one.
    ///
    /// The flag is raised from inside `on_input`, i.e. by the block that just
    /// came back - the same shape as a host thread flipping it asynchronously,
    /// minus the race, so the assertion is exact instead of statistical.
    #[test]
    fn a_cancel_flag_stops_the_play_within_one_block() {
        const TOTAL: usize = 256;
        const BLOCK: usize = 32;
        const TOTAL_BLOCKS: usize = TOTAL / BLOCK; // 8

        let mut b = Scripted::new(usize::MAX);
        let drive = drive_ramp(TOTAL);
        let mut scratch = vec![0.0f32; BLOCK];
        let cancel = AtomicBool::new(false);
        let mut blocks_delivered = 0usize;
        let err = play_block_wise(
            &mut b,
            &drive,
            1,
            1,
            &mut scratch,
            &BlockPlay::new(BLOCK),
            0,
            Some(&cancel),
            |_, _| {
                blocks_delivered += 1;
                // Raise it once the SECOND block is in hand, so the play has
                // demonstrably been running when the cancel arrives.
                if blocks_delivered == 2 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        )
        .expect_err("a raised cancel flag must end the play");

        match err {
            StreamError::Cancelled {
                frames_done,
                frames_committed,
                frames_expected,
            } => {
                assert_eq!(frames_expected, TOTAL);
                assert_eq!(frames_done, 2 * BLOCK, "cancelled at the next boundary");
                assert!(
                    frames_committed >= frames_done,
                    "the committed queue is never behind the read cursor: \
                     committed={frames_committed} done={frames_done}"
                );
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
        assert!(
            blocks_delivered < TOTAL_BLOCKS,
            "the play must stop EARLY: delivered {blocks_delivered} of {TOTAL_BLOCKS} blocks"
        );
        assert_eq!(blocks_delivered, 2, "within ONE block of the flag");
    }

    /// `cancel: None` is the pre-cancellation behavior, byte for byte: the whole
    /// record plays and the response is aligned sample-for-sample. This is the
    /// control for the production call sites that pass `None` (TWR's two; sine
    /// and the two shock sites now pass a live flag).
    #[test]
    fn no_cancel_input_plays_the_whole_record() {
        let mut b = Scripted::new(usize::MAX);
        let drive = drive_ramp(256);
        let mut scratch = vec![0.0f32; 32];
        let mut got = Vec::new();
        let n = play_block_wise(
            &mut b,
            &drive,
            1,
            1,
            &mut scratch,
            &BlockPlay::new(32),
            0,
            None,
            |blk, _| got.extend_from_slice(blk),
        )
        .unwrap();
        assert_eq!(n, 256);
        let expect: Vec<f32> = drive.iter().map(|d| d * 2.0).collect();
        assert_eq!(got, expect);
    }

    /// A flag that is ALREADY true when the play starts cancels before any drive
    /// is offered: `frames_committed == 0`, so nothing is queued at the device
    /// and there is no tail to decide about.
    #[test]
    fn a_pre_raised_cancel_flag_offers_no_drive_at_all() {
        let mut b = Scripted::new(usize::MAX);
        let drive = drive_ramp(64);
        let mut scratch = vec![0.0f32; 16];
        let cancel = AtomicBool::new(true);
        let err = play_block_wise(
            &mut b,
            &drive,
            1,
            1,
            &mut scratch,
            &BlockPlay::new(16),
            0,
            Some(&cancel),
            |_, _| panic!("no block may be delivered"),
        )
        .expect_err("a pre-raised flag must not play");
        match err {
            StreamError::Cancelled {
                frames_done,
                frames_committed,
                ..
            } => {
                assert_eq!(frames_done, 0);
                assert_eq!(frames_committed, 0, "nothing committed, nothing to drain");
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
        assert!(b.buffered.is_empty(), "the backend saw no drive");
    }

    /// `committed_frames` is the per-shot cold-task seam: drive the CALLER already
    /// put at the device before `start()` is neither re-sent nor lost from the read
    /// window.
    ///
    /// RED-MUTATION: pass `0` here (the earlier shape, which is all a caller
    /// could say) and the head is re-sent - `got` grows the first `HEAD` samples
    /// twice and the equality reds. Hand the play only `drive[HEAD..]` instead and
    /// `n` comes back `HEAD` frames short: the two failures this seam closes, and the
    /// reason two module crates each grew a shim to work around them.
    #[test]
    fn a_pre_committed_head_is_neither_re_sent_nor_lost_from_the_read_window() {
        const TOTAL: usize = 200;
        const HEAD: usize = 48;
        let mut b = Scripted::new(usize::MAX);
        let drive = drive_ramp(TOTAL);

        // The caller's own pre-start commit, exactly as a cold-task fire does it.
        let committed = b.write_drive(&drive[..HEAD]).expect("the head commits");
        assert_eq!(committed, HEAD);

        let mut scratch = vec![0.0f32; 32];
        let mut got = Vec::new();
        let n = play_block_wise(
            &mut b,
            &drive,
            1,
            1,
            &mut scratch,
            &BlockPlay::new(32),
            committed,
            None,
            |blk, _| got.extend_from_slice(blk),
        )
        .unwrap();

        assert_eq!(
            n, TOTAL,
            "the response window is the WHOLE record; a play that reads back only \
             what it wrote truncates it by the committed head"
        );
        let expect: Vec<f32> = drive.iter().map(|d| d * 2.0).collect();
        assert_eq!(
            got, expect,
            "each sample reaches the device exactly once, in order, across the \
             pre-commit seam"
        );
    }

    /// A cancel with a pre-committed head reports the WHOLE committed queue - the
    /// frames that keep driving after the flag flips include the ones the caller
    /// put there before `start()`, and understating them is the dangerous
    /// direction (see [`StreamError::Cancelled`]).
    #[test]
    fn a_cancel_counts_the_pre_committed_head_in_the_committed_queue() {
        const TOTAL: usize = 128;
        const HEAD: usize = 64;
        let mut b = Scripted::new(usize::MAX);
        let drive = drive_ramp(TOTAL);
        let committed = b.write_drive(&drive[..HEAD]).expect("the head commits");
        let mut scratch = vec![0.0f32; 16];
        let cancel = AtomicBool::new(true); // raised before the play begins

        let err = play_block_wise(
            &mut b,
            &drive,
            1,
            1,
            &mut scratch,
            &BlockPlay::new(16),
            committed,
            Some(&cancel),
            |_, _| panic!("no block may be delivered"),
        )
        .expect_err("a pre-raised flag must not play");
        match err {
            StreamError::Cancelled {
                frames_done,
                frames_committed,
                frames_expected,
            } => {
                assert_eq!(frames_done, 0);
                assert_eq!(frames_expected, TOTAL);
                assert_eq!(
                    frames_committed, HEAD,
                    "the committed queue is the caller's pre-start commit, not zero"
                );
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }
}
