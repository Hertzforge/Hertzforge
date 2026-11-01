//! Poll/pull scheduler: the engine's tick loop, driven by the device sample counter.
//!
//! This is NOT a DAW audio callback (none of the backends deliver one). Each tick:
//!
//! 1. **Pull** the next input block from the `dyn Backend` (`read_input`) into a scratch
//!    buffer, and read `poll_status()` for the device counter + epoch.
//! 2. **Timebase**: feed the counter/epoch/frames-delivered to the [`Timebase`], which
//!    surfaces resets and gaps. On a reset or gap the framed-DSP state and the
//!    watchdog integrators reset (no transform spans a boundary).
//! 3. **Raw ring**: push every raw frame into the RAW SPSC ring.
//! 4. **Watchdog**: poll the [`AbortWatchdog`] on the raw ring. A trip stops the run.
//! 5. **Process**: hand the block to the [`ProcessStep`], timed by [`SchedulingHealth`].
//!    A health fault triggers the mode-appropriate response (ramp-down for HostClosesLoop).
//! 6. **Capture**: write the raw block to the [`CaptureSink`] with its timebase meta.
//! 7. **Drive out**: enqueue the produced drive into [`OutputDelivery`], pull the next
//!    frame honoring the lookahead/underrun contract, and `write_drive` it back.
//!
//! The scheduler owns the CONSUMER side of the raw ring only through the watchdog; it
//! also keeps a private copy of the just-read block for processing + capture, so the
//! ring is purely the watchdog's raw tap (the watchdog gets its own SPSC channel of raw
//! samples).

use crate::capture::{BlockMeta, CaptureError, CaptureSink};
use crate::health::{HealthBudget, HealthStatus, SchedulingHealth};
use crate::output::{DeliveryOutcome, OutputDelivery, UnderrunAbort};
use crate::ring::{self, Producer};
use crate::timebase::{Timebase, TimebaseEvent};
use crate::watchdog::{AbortTrip, AbortWatchdog};
use eigen_channel_table::ChannelTable;
use eigen_hal::{Backend, ConfiguredActual, HalError, InputRead, LoopOwner};
use std::time::{Duration, Instant};

/// The per-tick processing step: takes a raw input block (interleaved f32) and produces
/// a drive block (interleaved f32) of `drive_len` per frame. This is the seam a control
/// module (SMART/Random/Sine) plugs into; the engine only sees this trait.
pub trait ProcessStep: Send {
    /// Process one input block of `n_frames`. `input` is `n_frames * in_channels`.
    /// Push `n_frames * out_channels` drive samples into `drive_out` (cleared for you).
    /// Called on the RT thread; must not allocate in steady state.
    fn process(&mut self, input: &[f32], n_frames: usize, drive_out: &mut Vec<f32>);

    /// Called when the timebase crosses an epoch boundary or a marked gap: reset any
    /// overlap-add / framed state so no transform spans the discontinuity.
    fn on_discontinuity(&mut self, _event: TimebaseEvent) {}
}

/// How many blocks' worth of drive the engine will stage for a backend that keeps
/// refusing before calling the output stuck. See [`Engine::deliver_drive`] step 5.
const MAX_STAGED_BLOCKS: usize = 8;

/// THE RING-OVERRUN RULE, as a pure decision.
///
/// A dropped raw frame is a frame the abort watchdog never examined. On a run with
/// limits ARMED it could have been the frame that carried the breach, so the run is
/// not "degraded", it is UNMONITORED, and it fails. On a run with no limits armed
/// nothing could have been hidden, so the count is a scheduling-health signal
/// ([`RunReport::ring_overruns`]) and not a stop.
///
/// Extracted as a free function on purpose. In today's SINGLE-THREADED engine a
/// producer-side overrun cannot actually happen - `Engine::run`'s push loop drains
/// the watchdog and retries on a full ring, `drain_each` empties the ring completely,
/// so the retry always succeeds, and the ring is sized at >= 8 blocks besides. A rule
/// whose only test drives an unreachable path is a rule with no test, so the DECISION
/// is testable on its own while the plumbing that feeds it is tested through the
/// engine. It becomes reachable the moment the watchdog moves to its own thread (the
/// intended design), where the producer can no longer drain on the consumer's behalf.
pub fn ring_overrun_stop(overruns: usize, limits_armed: bool) -> Option<StopReason> {
    if overruns > 0 && limits_armed {
        Some(StopReason::RingOverrun { frames: overruns })
    } else {
        None
    }
}

/// A trivial pass-through-to-zero process step: emits `out_channels` zeros per frame.
/// Useful as the "no control module loaded" default and as a test double.
pub struct SilentDrive {
    pub out_channels: usize,
}

impl ProcessStep for SilentDrive {
    fn process(&mut self, _input: &[f32], n_frames: usize, drive_out: &mut Vec<f32>) {
        drive_out.clear();
        drive_out.resize(n_frames * self.out_channels, 0.0);
    }
}

/// Why the scheduler stopped a run.
#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    /// The backend's input script/stream ended cleanly.
    InputEnded,
    /// The abort watchdog tripped on a limit breach.
    AbortTripped(AbortTrip),
    /// A drive underrun fired the underrun policy / hard underrun.
    DriveUnderrun(UnderrunAbort),
    /// Scheduling health degraded past budget in a HostClosesLoop run (ramp + abort).
    SchedulingFault(HealthStatus),
    /// A fatal capture error.
    CaptureFatal(String),
    /// A backend I/O error on read/write.
    BackendError(String),
    /// The device link was LOST mid-run (`HalError::Disconnected`): USB
    /// re-enumeration (a delegated controller's IEPE circuit check does this), a hot-unplugged DAQ.
    /// Distinct from `BackendError` because the OPERATOR response differs - the
    /// hardware may still be moving with no host link to command it, so the run is
    /// treated as aborted (best-effort `emergency_stop`, never a clean end), and the
    /// physical E-stop is the remaining authority. Distinct from `InputEnded` so a
    /// vanished device can never masquerade as a completed test.
    Disconnected(String),
    /// The input stream went SILENT: `read_input` kept answering `NoDataYet` for
    /// longer than the stall deadline.
    ///
    /// Distinct from `InputEnded` for the same reason `Disconnected` is: a stalled
    /// acquisition must never read as a completed test. Before this variant, a
    /// permanently stalled stream exited through `max_ticks` as a CLEAN
    /// `InputEnded` - the run "finished" with the shaker still live and the host
    /// blind to it. `emergency_stop` is called before this is returned.
    InputStalled {
        /// How long the input had been silent when the deadman fired.
        elapsed: Duration,
    },
    /// The RAW ring overran during a run with limits ARMED.
    ///
    /// A dropped raw frame on a limits-armed run is a frame the abort watchdog
    /// never examined, and it could have been the frame carrying the breach. That
    /// is not a degraded run to be reported afterwards, it is a FAILED run:
    /// `emergency_stop` is called and this is the stop reason. Distinct from
    /// `BackendError` because the operator response differs (the ring is too
    /// shallow or the watchdog is too slow for this block size, both host-side).
    RingOverrun {
        /// Raw frames the producer had to drop.
        frames: usize,
    },
}

/// Outcome of running the loop.
#[derive(Debug, Clone, PartialEq)]
pub struct RunReport {
    pub reason: StopReason,
    pub ticks: u64,
    pub frames_in: u64,
    pub frames_captured: u64,
    pub estops: u32,
    pub resets: u32,
    pub gaps: u32,
    pub health_faults: u64,
    /// RAW frames the ring producer had to drop because the ring was full.
    ///
    /// This counter existed on `ring::Producer` from the start and NOTHING read
    /// it: a run could drop raw frames the abort watchdog therefore never
    /// examined, and report a clean stop. On a limits-armed run any non-zero
    /// value is now a FAILED run ([`StopReason::RingOverrun`]); on a run with no
    /// limits armed it is reported here and is a scheduling-health signal.
    pub ring_overruns: usize,
    /// Breaches the watchdog observed during the ARM BLANKING window and did not act
    /// on. Non-zero is a WARNING: the run was over a limit while the window was open.
    pub blanked_trips: u32,
}

/// A live snapshot of engine health, for the run-status surface (the overrun counter
/// needs a LIVE home, not only an end-of-run report). Cheap to take; take it as often
/// as the UI refreshes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineHealth {
    /// Ticks executed so far.
    pub ticks: u64,
    /// Input frames pulled so far.
    pub frames_in: u64,
    /// RAW ring drops so far. **Any non-zero value on a limits-armed run means
    /// the run is about to fail** - it is surfaced live so an operator sees it
    /// happen rather than reading about it afterwards.
    pub ring_overruns: usize,
    /// Whether this run has any channel limits armed.
    pub limits_armed: bool,
    /// Whether the arm-blanking window is currently suppressing trips.
    pub blanking: bool,
    /// Would-have-tripped breaches observed during blanking.
    pub blanked_trips: u32,
    /// Scheduling-health faults so far.
    pub health_faults: u64,
    /// The WORST per-tick compute time observed so far.
    ///
    /// [`SchedulingHealth`] has always tracked this and nothing outside the engine
    /// could read it, so "how close is this module to its deadline?" was answerable
    /// only after a fault - i.e. only once the answer was "past it". A module gate
    /// that has to state a compute margin on real hardware needs the number BEFORE
    /// the margin is gone. Compare against [`Self::compute_budget`].
    pub worst_compute: Duration,
    /// The per-tick COMPUTE budget this run is armed on: the tick period on a
    /// hardware path, or the explicit relaxation a deterministic harness set. The
    /// denominator for [`Self::worst_compute`], surfaced beside it so a caller
    /// cannot pair the measurement with a budget it re-derived and got wrong.
    pub compute_budget: Duration,
    /// Drive frames queued host-side in the output delivery buffer.
    pub output_queued: usize,
    /// Drive frames STAGED for a backend that has not accepted them yet.
    pub output_staged: usize,
    /// Watchdog polls so far. Compare against `ticks`: a poll count below the
    /// tick count means some path skipped the safety check.
    pub watchdog_polls: u64,
}

/// Configuration for one run.
pub struct EngineConfig {
    pub in_channels: usize,
    pub out_channels: usize,
    /// Frames per tick (the block size pulled per `read_input`).
    pub frames_per_tick: usize,
    /// Raw ring capacity in frames (the watchdog tap depth).
    pub ring_frames: usize,
    /// Integrator high-pass corner for the watchdog vel/disp paths.
    pub watchdog_hp_hz: f64,
    /// Sustained-breach debounce for the watchdog (1 = spike-sensitive).
    pub watchdog_sustain: u32,
    /// Allowed period-jitter fraction for scheduling health.
    pub jitter_frac: f64,
    /// Per-tick COMPUTE budget for the processing step. `None` = the tick period
    /// (`frames_per_tick / sample_rate`), which is the correct HARDWARE
    /// semantics: processing must finish before the next block's data is due.
    ///
    /// It exists because the tick period is the WRONG budget for a deterministic
    /// EIGEN-SIM harness, and that mismatch was manufacturing safety events out of
    /// nothing. Such a harness runs the engine flat out between control-DSP
    /// passes; at the Sim's one-frame block the period is ~122 us, so a single OS
    /// hiccup inside a `process` call that genuinely takes nanoseconds trips
    /// `ComputeOverrun` and aborts the run. Every module then grew a retry loop to
    /// paper over it, and those retries later had to be unwound. Recorded as a
    /// deferred refinement; retired here.
    ///
    /// A HARDWARE path leaves this `None`. A Sim harness sets it EXPLICITLY to
    /// [`crate::block::TEST_RELAX_COMPUTE_BUDGET`], by name.
    pub compute_budget: Option<Duration>,
    /// How long `read_input` may answer `NoDataYet` before the run is declared STALLED.
    /// `None` = derive it as [`STALL_DEADLINE_BLOCKS`] x the block period at the
    /// configured rate.
    ///
    /// The liveness measure is TIME, not tick count: a tick that produced no
    /// frames says nothing about how long the stream has been silent, and it was
    /// exactly that conflation that let a permanently stalled stream run out
    /// `max_ticks` and report a clean `InputEnded`.
    pub stall_deadline: Option<Duration>,
    /// ARM BLANKING window. Private with a checked setter so a run whose blanking never
    /// ends CANNOT be constructed - see [`EngineConfig::set_blanking`].
    blanking: Duration,
}

/// The stall deadline, in BLOCKS, when [`EngineConfig::stall_deadline`] is `None`.
/// Four blocks: long enough that a single late driver read is not a stall, short
/// enough that a genuinely silent stream is caught within a few block periods.
pub const STALL_DEADLINE_BLOCKS: u32 = 4;

/// Hard cap on the arm-blanking window.
///
/// Blanking suppresses the abort watchdog. A window that never ends is a DISABLED
/// watchdog wearing a settling-time costume, so the cap is not advisory: values
/// beyond it are refused at configuration time, and there is no code path that
/// keeps the window open past it.
pub const MAX_BLANKING: Duration = Duration::from_secs(30);

/// Placeholder IEPE settling allowance for a HARDWARE station, for a caller that
/// has no measured figure. NOT a measured value: IEPE excitation turn-on plus the
/// watchdog's HP-integrator re-arm is the transient being covered, and the real
/// number is per-accelerometer. The backend/caller SETS the window; this is only
/// the documented starting point.
pub const IEPE_SETTLE_PLACEHOLDER: Duration = Duration::from_secs(2);

/// Refusals from an [`EngineConfig`] setter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineConfigError {
    /// The requested blanking window exceeds [`MAX_BLANKING`].
    BlankingTooLong { requested: Duration, max: Duration },
}

impl std::fmt::Display for EngineConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineConfigError::BlankingTooLong { requested, max } => write!(
                f,
                "arm blanking of {requested:?} exceeds the {max:?} cap; blanking suppresses \
                 the abort watchdog, so an unbounded window is a disabled watchdog"
            ),
        }
    }
}

impl std::error::Error for EngineConfigError {}

impl EngineConfig {
    pub fn new(in_channels: usize, out_channels: usize) -> Self {
        EngineConfig {
            in_channels,
            out_channels,
            frames_per_tick: 64,
            ring_frames: crate::block::ring_frames_for(64),
            watchdog_hp_hz: 5.0,
            watchdog_sustain: 1,
            jitter_frac: 0.5,
            compute_budget: None,
            stall_deadline: None,
            // EIGEN-SIM default: ZERO. There is no IEPE excitation to settle and
            // no armature, so there is nothing to blank and every tick is armed.
            // A hardware caller sets a window explicitly (see
            // `IEPE_SETTLE_PLACEHOLDER` and `set_blanking`).
            blanking: Duration::ZERO,
        }
    }

    /// The configured arm-blanking window.
    pub fn blanking(&self) -> Duration {
        self.blanking
    }

    /// Set the ARM BLANKING window, refusing anything past [`MAX_BLANKING`].
    ///
    /// This is the ONLY way to set it, and the field is private, so a run whose
    /// blanking window never ends cannot be expressed - not by a struct literal,
    /// not by a clamp that silently succeeds, not by `Duration::MAX`. The refusal
    /// is the point: blanking suppresses the abort watchdog.
    pub fn set_blanking(&mut self, window: Duration) -> Result<(), EngineConfigError> {
        if window > MAX_BLANKING {
            return Err(EngineConfigError::BlankingTooLong {
                requested: window,
                max: MAX_BLANKING,
            });
        }
        self.blanking = window;
        Ok(())
    }

    /// Build a config whose block cadence comes from what the backend ACTUALLY
    /// configured.
    ///
    /// This is the seam every caller should build through. The caller states the
    /// block it wants in [`eigen_hal::AcquisitionConfig::block_frames`], the
    /// backend answers in [`ConfiguredActual::block_frames`], and this derives
    /// `frames_per_tick` from that answer plus `ring_frames` from the block. The
    /// scheduling-health budget then falls out of the real block cadence in
    /// [`Engine::new`], because it is computed from `frames_per_tick` and the
    /// configured rate.
    ///
    /// The alternative - a hard-coded `frames_per_tick` - is how a run ends up
    /// polling at a cadence the hardware never agreed to deliver.
    pub fn with_block(in_channels: usize, out_channels: usize, actual: &ConfiguredActual) -> Self {
        let frames_per_tick = crate::block::frames_per_tick_from(actual);
        let mut cfg = EngineConfig::new(in_channels, out_channels);
        cfg.frames_per_tick = frames_per_tick;
        cfg.ring_frames = crate::block::ring_frames_for(frames_per_tick);
        cfg
    }
}

/// The engine. Owns the raw-ring producer, the watchdog, timebase, output delivery,
/// scheduling-health monitor, and the capture sink. `run` executes the poll/pull loop
/// to completion (single-threaded here so it is deterministically testable; the wider
/// system pins this on an RT thread with the watchdog on its own isolated thread).
pub struct Engine {
    cfg: EngineConfig,
    raw_prod: Producer,
    watchdog: AbortWatchdog,
    timebase: Timebase,
    output: OutputDelivery,
    health: SchedulingHealth,
    loop_owner: LoopOwner,
    /// The contracted output lookahead, in frames (from `caps.output_buffer`).
    lookahead_frames: usize,
    /// Have we pushed the device-side zero prefill yet? See [`Engine::prefill_output`].
    prefilled: bool,
    /// Cap on `staged_out`, in frames: a backend refusing beyond this is stuck.
    max_staged_frames: usize,
    /// Running totals for the stall fault's own diagnosis: a stuck-output
    /// message that cannot say HOW stuck sends the next reader guessing.
    total_staged_frames: u64,
    total_accepted_frames: u64,
    delivery_ticks: u64,
    /// Consecutive ticks in which the backend accepted ZERO drive frames.
    /// The stuck-output test (see `deliver_drive` step 5): progress, not depth.
    zero_accept_ticks: u32,
    /// `Some((requested, raised_to))` when `Engine::new` had to raise a too-shallow
    /// `ring_frames` to the block-derived floor. Surfaced through
    /// [`Engine::ring_auto_raised`] and in the health snapshot's warning.
    ring_auto_raised: Option<(usize, usize)>,
    /// The resolved input-stall deadline: `cfg.stall_deadline`, or
    /// `STALL_DEADLINE_BLOCKS` x the block period at the configured rate.
    stall_deadline: Duration,
    /// The resolved arm-blanking window, already capped.
    blanking: Duration,
    /// Live counters for [`Engine::health`], carried across `run()` calls the way
    /// the rest of the engine's state is (the sine service re-runs one Engine per
    /// chunk, so per-run locals are not a usable live surface).
    live_ticks: u64,
    live_frames_in: u64,
    // scratch buffers, allocated once at construction (zero alloc in the loop)
    in_scratch: Vec<f32>,
    frame_scratch: Vec<f32>,
    drive_scratch: Vec<f32>,
    /// Drive frames pulled from [`OutputDelivery`] that the backend has not accepted
    /// yet (interleaved). Re-offered, in order, ahead of anything newer.
    staged_out: Vec<f32>,
    /// Preallocated zero block for the device prefill.
    zero_block: Vec<f32>,
}

impl Engine {
    /// Build the engine from the channel table and the backend's advertised caps. The
    /// watchdog frame layout, the output contract, and the health budget are all derived
    /// from those pinned sources.
    pub fn new(mut cfg: EngineConfig, table: &ChannelTable, backend: &dyn Backend) -> Self {
        let caps = backend.caps();
        // RING SIZING. The raw ring must hold at least
        // `RING_BLOCKS` blocks, so the watchdog can fall that many blocks behind
        // the acquisition before a raw frame is dropped. A short ring is
        // AUTO-RAISED, not refused: a ring one block too shallow silently drops
        // frames the abort watchdog needed to see, and refusing to construct the
        // engine would take the live safety monitor offline to punish a
        // configuration mistake. The raise is recorded so the caller can see it.
        let required_ring = crate::block::ring_frames_for(cfg.frames_per_tick);
        let ring_auto_raised = if cfg.ring_frames < required_ring {
            let was = cfg.ring_frames;
            cfg.ring_frames = required_ring;
            Some((was, required_ring))
        } else {
            None
        };
        let (raw_prod, raw_cons) = ring::channel(cfg.ring_frames, cfg.in_channels);

        let watchdog = AbortWatchdog::new(
            table,
            raw_cons,
            caps.sample_rate_hz,
            cfg.watchdog_hp_hz,
            cfg.watchdog_sustain,
        );

        let contract = caps
            .output_buffer
            .unwrap_or(eigen_hal::OutputBufferContract {
                min_lookahead_frames: 4,
                underrun_policy: eigen_hal::UnderrunPolicy::RampDownThenAbort,
            });
        let mut output = OutputDelivery::new(contract, cfg.out_channels);
        // Prime the lookahead with benign zero drive so the first ticks are covered.
        output.prime_with_zeros(contract.min_lookahead_frames as usize);

        let mut budget = HealthBudget::from_rate(
            caps.sample_rate_hz,
            cfg.frames_per_tick as u32,
            cfg.jitter_frac,
        );
        // An explicit compute budget overrides the tick-period default. See the
        // field's doc for why a deterministic harness needs one.
        if let Some(b) = cfg.compute_budget {
            budget = budget.with_compute_budget(b);
        }

        let in_scratch = vec![0.0; cfg.frames_per_tick * cfg.in_channels];
        let frame_scratch = vec![0.0; cfg.in_channels];
        let drive_scratch = Vec::with_capacity(cfg.frames_per_tick * cfg.out_channels);
        // Staging headroom: enough that a transient refusal (an NI write-space query
        // that failed for one tick) is absorbed, small enough that a persistently
        // stuck output is caught within a handful of blocks rather than in a heap dump.
        let max_staged_frames =
            (cfg.frames_per_tick * MAX_STAGED_BLOCKS).max(contract.min_lookahead_frames as usize);
        let staged_out =
            Vec::with_capacity((max_staged_frames + cfg.frames_per_tick) * cfg.out_channels);
        let zero_block = vec![0.0; contract.min_lookahead_frames as usize * cfg.out_channels];
        // The input-stall deadline. Derived from the REAL block cadence
        // and the configured rate, so it tracks the block-size policy instead of
        // being a fixed number that is generous at one rate and absurd at another.
        // `Duration::MAX` when the rate is unusable is deliberate: a rate of zero
        // means the period is meaningless, and an aggressive deadman derived from
        // nonsense would abort healthy runs.
        let stall_deadline = cfg.stall_deadline.unwrap_or_else(|| {
            if caps.sample_rate_hz > 0.0 && caps.sample_rate_hz.is_finite() {
                Duration::from_secs_f64(
                    STALL_DEADLINE_BLOCKS as f64 * cfg.frames_per_tick as f64 / caps.sample_rate_hz,
                )
            } else {
                Duration::MAX
            }
        });

        Engine {
            timebase: Timebase::new(caps.sample_rate_hz),
            health: SchedulingHealth::new(budget),
            loop_owner: caps.loop_owner,
            lookahead_frames: contract.min_lookahead_frames as usize,
            prefilled: false,
            max_staged_frames,
            total_staged_frames: 0,
            total_accepted_frames: 0,
            delivery_ticks: 0,
            zero_accept_ticks: 0,
            ring_auto_raised,
            stall_deadline,
            blanking: cfg.blanking.min(MAX_BLANKING),
            live_ticks: 0,
            live_frames_in: 0,
            raw_prod,
            watchdog,
            output,
            cfg,
            in_scratch,
            frame_scratch,
            drive_scratch,
            staged_out,
            zero_block,
        }
    }

    /// The watchdog's per-limit-type latency budget, surfaced for the arm() gate.
    pub fn latency_budget(&self) -> crate::watchdog::LatencyBudget {
        self.watchdog.latency_budget()
    }

    /// Frames per tick this engine actually runs (after the config seam).
    pub fn frames_per_tick(&self) -> usize {
        self.cfg.frames_per_tick
    }

    /// Raw-ring capacity in frames, AFTER any auto-raise.
    pub fn ring_frames(&self) -> usize {
        self.cfg.ring_frames
    }

    /// `Some((requested, raised_to))` if the configured ring depth was below the
    /// block-derived floor and had to be raised. `None` when the caller's depth
    /// already satisfied it.
    pub fn ring_auto_raised(&self) -> Option<(usize, usize)> {
        self.ring_auto_raised
    }

    /// Run the poll/pull loop until a stop condition. `max_ticks` bounds the loop so a
    /// test cannot hang; pass a large value for a real run.
    pub fn run(
        &mut self,
        backend: &mut dyn Backend,
        process: &mut dyn ProcessStep,
        sink: &mut dyn CaptureSink,
        max_ticks: u64,
    ) -> RunReport {
        let report = self.run_ticks(backend, process, sink, max_ticks);
        // The RUN epilogue: a finished run leaves no drive on the hardware.
        // Deliberately NOT part of `run_ticks` - a segmented caller (the
        // recorder's monitor pump) runs MANY tick batches over ONE continuously
        // running backend, and stopping the tasks at every segment boundary
        // kills the AO between segments (the silent per-segment re-start then
        // fails and output_space stays 0 - the daemon's 2026-07-30 stall) and
        // puts a dropped-sample seam in what must be gap-free acquisition.
        let _ = backend.stop();
        report
    }

    /// [`Engine::run`] WITHOUT the backend-stop epilogue: one bounded batch of
    /// ticks over a backend the CALLER keeps running across calls. The safety
    /// paths inside the loop (watchdog trip -> `emergency_stop`, stall deadman,
    /// ring overrun) are identical - only the end-of-run "drive off" convention
    /// moves to the caller, whose run is the whole monitor session rather than
    /// one segment.
    pub fn run_ticks(
        &mut self,
        backend: &mut dyn Backend,
        process: &mut dyn ProcessStep,
        sink: &mut dyn CaptureSink,
        max_ticks: u64,
    ) -> RunReport {
        let frame_len = self.cfg.in_channels;
        let mut ticks = 0u64;
        let mut frames_in = 0u64;
        let mut frames_captured = 0u64;
        let mut resets = 0u32;
        let mut gaps = 0u32;
        // Liveness + blanking are both WALL-CLOCK measures, taken from the moment
        // the loop starts rather than from a tick count (liveness: a tick that
        // produced no frames says nothing about how long the stream has been
        // silent; blanking: a settling transient is a duration, not a block count).
        let run_start = Instant::now();
        let mut last_frames_instant = run_start;

        let reason = loop {
            if ticks >= max_ticks {
                break StopReason::InputEnded;
            }
            ticks += 1;
            self.live_ticks += 1;

            // --- 0. ARM BLANKING. The watchdog OBSERVES throughout - its
            // integrators must keep running or the transient never settles - but
            // does not act until the window closes. The window is capped at
            // construction (`MAX_BLANKING`) and is derived from a Duration that
            // only ever counts DOWN, so it always ends.
            self.watchdog
                .set_blanked(!self.blanking.is_zero() && run_start.elapsed() < self.blanking);

            // --- 1. PULL input from the backend. ---
            // `read_input` speaks FRAMES, not samples (`InputRead::Frames(n)` names the
            // unit in the type). This loop previously bound the old `usize` return to
            // `n_samples` and divided it by `frame_len` to "get" frames, which is only
            // correct at `in_channels == 1` (frames == samples) and silently discarded
            // (N-1)/N of every block at N channels: the raw ring, the process step, and the
            // capture sink all saw the first 1/N of the buffer, self-consistently labelled as
            // 1/N of the frames. Nothing looked broken because the deception was coherent.
            // Frames are the unit on the wire here; samples are derived from them.
            //
            // The old return also OVERLOADED `Ok(0)` as end-of-stream, so a continuous acquisition
            // with no frames ready THIS poll (an NI DAQmx timeout, a delegated controller's
            // all-stale telemetry block) read as a spurious clean stop. `NoDataYet` now keeps
            // polling; only `Ended` stops.
            //
            // A `NoDataYet` tick used to `continue` outright, skipping the ring push,
            // THE WATCHDOG POLL and the drive top-up - and a permanently stalled stream
            // then ran `max_ticks` out and reported a CLEAN `InputEnded`, i.e. a test
            // that never acquired a sample reported as a test that finished. Item 7
            // fixes all three: the watchdog is still polled (it must see an
            // empty-but-alive stream), the drive is still topped up (the AO must not
            // starve because the AI hiccuped), and a stream silent for longer than
            // `stall_deadline` is a FAULTED stop, never a clean end.
            let n_frames = match backend.read_input(&mut self.in_scratch) {
                Ok(InputRead::Ended) => break StopReason::InputEnded,
                // Frames(0) is a backend contract wart (the contract says use NoDataYet);
                // treat it as NoDataYet rather than re-overloading it as an end-of-stream.
                Ok(InputRead::NoDataYet) | Ok(InputRead::Frames(0)) => {
                    // The watchdog still runs. It drains an empty ring and finds
                    // nothing, which is the correct answer for a live-but-idle
                    // stream - and it keeps the poll count equal to the tick count,
                    // so "was the watchdog checked every block" stays checkable.
                    if let Some(trip) = self.watchdog.poll(backend) {
                        break StopReason::AbortTripped(trip);
                    }
                    // The drive keeps flowing: any frames the backend refused on an
                    // earlier tick are re-offered. Nothing NEW is pulled from the
                    // queue (no block was processed, so no new drive exists, and
                    // draining the lookahead here would spend exactly the reserve
                    // that a stalled input is the reason to hold).
                    if self.loop_owner == LoopOwner::HostClosesLoop {
                        if let Some(stop) = self.deliver_drive(backend, 0) {
                            break stop;
                        }
                    }
                    // THE DEADMAN. Time-based, not tick-based.
                    let silent = last_frames_instant.elapsed();
                    if silent > self.stall_deadline {
                        let _ = backend.emergency_stop();
                        break StopReason::InputStalled { elapsed: silent };
                    }
                    // The wait this tick spent blocked in the driver's read
                    // timeout is INPUT-stream time, not thread-scheduling
                    // time; without this the next processing tick reads the
                    // gap as PeriodJitter and aborts the run (the NI read
                    // timeout is 2.0 blocks against a 1.5-block allowance).
                    self.health.note_idle_wait();
                    continue;
                }
                Ok(InputRead::Frames(n)) => n,
                // The link is GONE: the device may still be moving with no host channel
                // to command it. Best-effort emergency_stop (it will usually fail against
                // a vanished transport - that is fine, the call must still be attempted in
                // case the transport is half-alive), then stop with the DISTINCT reason so
                // the operator sees "link lost", never "test completed".
                // TODO: the reconnect seam - re-drive `open()` and resume where
                // the operation is safe to resume - is deliberately NOT implemented here.
                Err(HalError::Disconnected(detail)) => {
                    let _ = backend.emergency_stop();
                    break StopReason::Disconnected(detail);
                }
                Err(e) => break StopReason::BackendError(e.to_string()),
            };
            // A backend claiming more frames than the scratch holds would panic the slicing
            // below on the RT thread. Treat it as a backend fault rather than an abort.
            let n_samples = n_frames * frame_len;
            if n_samples > self.in_scratch.len() {
                break StopReason::BackendError(format!(
                    "read_input claimed {n_frames} frames x {frame_len} ch = {n_samples} samples, \
                     exceeding the {}-sample input scratch",
                    self.in_scratch.len()
                ));
            }
            frames_in += n_frames as u64;
            self.live_frames_in += n_frames as u64;
            // The stream delivered: the deadman's clock restarts here and nowhere
            // else. (A tick that produced no frames must NOT refresh it - that is
            // the conflation that made the old `continue` look alive forever.)
            last_frames_instant = Instant::now();
            let status = backend.poll_status();

            // --- 2. TIMEBASE: detect reset / gap / wrap. ---
            let start_frame = self.timebase.global_frame() as u64;
            let events = self.timebase.update(&status, n_frames as u64);
            let mut gap_before = false;
            for ev in &events {
                match ev {
                    TimebaseEvent::Reset { .. } => {
                        resets += 1;
                        self.watchdog.reset_integrators();
                        process.on_discontinuity(*ev);
                    }
                    TimebaseEvent::Gap { .. } => {
                        gaps += 1;
                        gap_before = true;
                        self.watchdog.reset_integrators();
                        process.on_discontinuity(*ev);
                    }
                    TimebaseEvent::Wrap => {}
                }
            }

            // --- 3. RAW RING: publish every raw frame for the watchdog tap. ---
            for fi in 0..n_frames {
                let base = fi * frame_len;
                self.frame_scratch
                    .copy_from_slice(&self.in_scratch[base..base + frame_len]);
                // If the raw ring is full the watchdog is behind; drain it first so a
                // slow watchdog cannot mask a breach by dropping raw frames.
                if !self.raw_prod.push(&self.frame_scratch) {
                    self.watchdog.poll(backend);
                    let _ = self.raw_prod.push(&self.frame_scratch);
                }
            }

            // --- 4. WATCHDOG: raw-ring limit check. A trip stops the run. ---
            if let Some(trip) = self.watchdog.poll(backend) {
                break StopReason::AbortTripped(trip);
            }

            // --- 4b. RING OVERRUN on a LIMITS-ARMED run is a FAILED run.
            if let Some(stop) =
                ring_overrun_stop(self.raw_prod.overruns(), self.watchdog.limits_armed())
            {
                let _ = backend.emergency_stop();
                break stop;
            }

            // --- 5. PROCESS the block, timed for scheduling health. ---
            self.health.tick_start();
            process.process(
                &self.in_scratch[..n_samples],
                n_frames,
                &mut self.drive_scratch,
            );
            let health = self.health.tick_end();
            if health.is_fault() {
                // HostClosesLoop: a scheduling fault means the drive cannot be trusted
                // on time; ramp down and abort. Delegate: alarm only (the device owns
                // the loop). Here we only run the drive path for HostClosesLoop.
                if self.loop_owner == LoopOwner::HostClosesLoop {
                    let _ = backend.emergency_stop();
                    break StopReason::SchedulingFault(health);
                }
            }

            // --- 6. CAPTURE the raw block. ---
            let meta = BlockMeta {
                epoch: status.epoch,
                start_frame,
                n_frames: n_frames as u64,
                gap_before,
            };
            match sink.write_block(&self.in_scratch[..n_samples], meta) {
                Ok(()) => frames_captured += n_frames as u64,
                Err(CaptureError::WouldBlock) => { /* backpressure: mark gap, continue */ }
                Err(CaptureError::Fatal(m)) => break StopReason::CaptureFatal(m),
            }

            // --- 7. DRIVE OUT (HostClosesLoop only). ---
            if self.loop_owner == LoopOwner::HostClosesLoop {
                if let Some(stop) = self.deliver_drive(backend, n_frames) {
                    break stop;
                }
            }
        };

        let _ = sink.finalize();

        RunReport {
            reason,
            ticks,
            frames_in,
            frames_captured,
            estops: self.watchdog.estop_calls(),
            resets,
            gaps,
            health_faults: self.health.fault_count(),
            ring_overruns: self.raw_prod.overruns(),
            blanked_trips: self.watchdog.blanked_trips(),
        }
    }

    /// A LIVE snapshot of engine health.
    ///
    /// `RunReport` only exists once the run is over; the overrun counter needs a
    /// home an operator surface can poll WHILE the run is happening. Cheap: a few
    /// atomic loads and field reads, no allocation.
    pub fn health(&self) -> EngineHealth {
        EngineHealth {
            ticks: self.live_ticks,
            frames_in: self.live_frames_in,
            ring_overruns: self.raw_prod.overruns(),
            limits_armed: self.watchdog.limits_armed(),
            blanking: self.watchdog.is_blanked(),
            blanked_trips: self.watchdog.blanked_trips(),
            health_faults: self.health.fault_count(),
            worst_compute: self.health.worst_compute(),
            compute_budget: self.health.budget().compute_budget,
            output_queued: self.output.queued(),
            output_staged: self
                .staged_out
                .len()
                .checked_div(self.cfg.out_channels)
                .unwrap_or(0),
            watchdog_polls: self.watchdog.polls(),
        }
    }

    /// The resolved input-stall deadline this run enforces.
    pub fn stall_deadline(&self) -> Duration {
        self.stall_deadline
    }

    /// The resolved arm-blanking window this run applies, post-cap.
    pub fn blanking(&self) -> Duration {
        self.blanking
    }

    /// The first breach the watchdog observed during the blanking window, if any.
    /// Non-`None` is a WARNING for the operator, not a trip.
    pub fn first_blanked_trip(&self) -> Option<AbortTrip> {
        self.watchdog.first_blanked_trip()
    }

    /// Push benign ZERO drive into the backend's output buffer up to the contracted
    /// `min_lookahead_frames`, so the device starts with its lookahead already in
    /// hand instead of racing the first processed block.
    ///
    /// Call it between `arm()` and `start()`. It is idempotent, and `run()` invokes
    /// it lazily on the first delivering tick for callers that do not (so an existing
    /// arm -> start -> run sequence still gets the prefill, just one tick later).
    ///
    /// The zeros come from OUTSIDE [`OutputDelivery`], not from its primed queue:
    /// the queue's zeros ARE the host-side lookahead, and draining them to fill the
    /// device would trip the low-water policy on tick one.
    /// Declare the output ALREADY primed, so this engine will not write its
    /// zero prefill.
    ///
    /// The prefill exists to satisfy a non-regenerating output's start
    /// precondition: such a device refuses to start on an empty buffer. That is
    /// a per-RUN concern. A caller that scores one continuous acquisition in
    /// segments - and therefore builds a fresh `Engine` per segment while the
    /// device tasks keep running - must say so, or every segment boundary
    /// injects `lookahead_frames` of SILENCE ahead of that segment's real drive.
    ///
    /// That was a real bug: measured on a DMM at the AO node as a 50 ms quiet window
    /// recurring at exactly the 1.44 s segment period, with the output buffer
    /// FULL at every boundary (it was full of these zeros), no underrun, and a
    /// clean synthesizer. Nothing downstream could see it because the silence
    /// was real, queued drive.
    pub fn mark_output_prefilled(&mut self) {
        self.prefilled = true;
    }

    /// Adopt a previous engine's undelivered drive in place of this one's zero
    /// priming. See [`OutputDelivery::adopt_queue`].
    pub fn adopt_output_queue(&mut self, frames: Vec<Vec<f32>>) {
        self.output.adopt_queue(frames);
    }

    /// Take this engine's undelivered drive, to hand to its successor.
    pub fn take_output_queue(&mut self) -> Vec<Vec<f32>> {
        self.output.take_queue()
    }

    pub fn prefill_output(&mut self, backend: &mut dyn Backend) -> Result<usize, StopReason> {
        if self.prefilled {
            return Ok(0);
        }
        self.prefilled = true;
        if self.loop_owner != LoopOwner::HostClosesLoop {
            return Ok(0); // the device generates its own drive
        }
        let want = self.lookahead_frames;
        if want == 0 || self.cfg.out_channels == 0 {
            return Ok(0);
        }
        let out_ch = self.cfg.out_channels;
        self.zero_block.clear();
        self.zero_block.resize(want * out_ch, 0.0);
        let mut sent = 0usize;
        // A backend with no space semantics takes everything in one call; one with a
        // real buffer may take it in pieces. Bounded so a refusing backend cannot spin.
        for _ in 0..want.max(1) {
            if sent >= want {
                break;
            }
            let slice = &self.zero_block[sent * out_ch..want * out_ch];
            match backend.write_drive(slice) {
                Ok(0) => break, // no room / not accepting: not fatal at prefill time
                Ok(n) => sent += n.min(want - sent),
                Err(HalError::Disconnected(d)) => {
                    let _ = backend.emergency_stop();
                    return Err(StopReason::Disconnected(d));
                }
                Err(e) => return Err(StopReason::BackendError(e.to_string())),
            }
        }
        Ok(sent)
    }

    /// Enqueue the produced drive, then TOP UP the backend's output buffer with as
    /// many queued frames as it has room for, retaining whatever it refused.
    /// Returns `Some(StopReason)` if an underrun (or a stuck output) aborted the run.
    ///
    /// ## The policy
    ///
    /// 1. This tick's produced frames go into [`OutputDelivery`], which owns the
    ///    lookahead contract and the underrun policy.
    /// 2. The engine PULLS `n_frames` frames from it - the rate-matched cadence: the
    ///    loop consumed `n_frames` input frames this tick, so the device consumed
    ///    about `n_frames` output frames. This is the same demand the old lockstep
    ///    delivery had, which is why the underrun policy fires at exactly the same
    ///    point it always did. Pulling is where `DeliveryOutcome::Underrun` surfaces.
    /// 3. The pulled frames are STAGED, behind any frames a previous tick staged and
    ///    the backend refused. Drive order is never reordered or dropped.
    /// 4. `backend.output_space()` caps how many staged frames are OFFERED this tick:
    ///    `None` means the legacy lockstep contract (no independent output buffer, so
    ///    everything offered must be accepted and a short accept is a fault);
    ///    `Some(space)` is real headroom on a DMA-clocked AO buffer, and a short
    ///    accept is honest backpressure - the tail stays staged for the next tick.
    /// 5. Staging is BOUNDED. A backend that keeps refusing is a stuck output, not a
    ///    slow one; unbounded staging would grow memory while the drive silently fell
    ///    further and further behind the loop it is supposed to be closing.
    fn deliver_drive(&mut self, backend: &mut dyn Backend, n_frames: usize) -> Option<StopReason> {
        let out_ch = self.cfg.out_channels;
        if out_ch == 0 {
            return None;
        }
        if !self.prefilled {
            if let Err(stop) = self.prefill_output(backend) {
                return Some(stop);
            }
        }
        // 1. Enqueue this tick's drive frames (refused once we are ramping down).
        for fi in 0..n_frames {
            let base = fi * out_ch;
            if base + out_ch <= self.drive_scratch.len() {
                self.output
                    .enqueue(&self.drive_scratch[base..base + out_ch]);
            }
        }

        // 2/3. Pull the tick's cadence into the staging buffer.
        for _ in 0..n_frames {
            match self.output.next_frame() {
                DeliveryOutcome::Frame(f) | DeliveryOutcome::PolicyFrame(f) => {
                    self.staged_out.extend_from_slice(&f);
                }
                DeliveryOutcome::Underrun => {
                    // An actual underrun is an abort, never a run-dry.
                    let _ = backend.emergency_stop();
                    let ab = self
                        .output
                        .abort_pending()
                        .unwrap_or(UnderrunAbort::HardUnderrun);
                    return Some(StopReason::DriveUnderrun(ab));
                }
            }
        }

        // 4. Offer what the device has room for.
        let staged_frames = self.staged_out.len() / out_ch;
        self.delivery_ticks = self.delivery_ticks.saturating_add(1);
        self.total_staged_frames = self.total_staged_frames.saturating_add(n_frames as u64);
        if staged_frames == 0 {
            return None;
        }
        let space = backend.output_space();
        let offer = match space {
            Some(s) => staged_frames.min(s),
            None => staged_frames, // lockstep: everything offered must be accepted
        };
        if offer == 0 {
            // Zero SPACE is zero progress. Counting it only inside the write
            // path missed the very case the stuck-output test exists for: a
            // backend reporting no room never reaches a `write_drive` call at
            // all, so the counter sat at zero while the queue grew.
            self.zero_accept_ticks = self.zero_accept_ticks.saturating_add(1);
        }
        if offer > 0 {
            let accepted = match backend.write_drive(&self.staged_out[..offer * out_ch]) {
                Ok(n) => n,
                Err(HalError::Disconnected(detail)) => {
                    // Link lost with drive possibly still live device-side:
                    // best-effort stop, distinct reason (see the read path).
                    let _ = backend.emergency_stop();
                    return Some(StopReason::Disconnected(detail));
                }
                Err(e) => return Some(StopReason::BackendError(e.to_string())),
            };
            if accepted > offer {
                let _ = backend.emergency_stop();
                return Some(StopReason::BackendError(format!(
                    "write_drive claimed {accepted} of {offer} offered drive frames"
                )));
            }
            if space.is_none() && accepted < offer {
                // No space semantics means no backpressure semantics: a lockstep
                // backend that refuses a frame has a broken output stream, and
                // dropping drive on the floor is a discontinuity at the shaker.
                let _ = backend.emergency_stop();
                return Some(StopReason::BackendError(format!(
                    "write_drive accepted {accepted} of {offer} offered drive frames on a \
                     backend reporting no output_space(); the lockstep contract has no \
                     backpressure to fall back on"
                )));
            }
            self.staged_out.drain(..accepted * out_ch);
            self.total_accepted_frames = self.total_accepted_frames.saturating_add(accepted as u64);
            if accepted > 0 {
                self.zero_accept_ticks = 0;
            } else {
                self.zero_accept_ticks = self.zero_accept_ticks.saturating_add(1);
            }
        }

        // 5. A stuck output is a fault - but STUCK means NO PROGRESS, not deep.
        //
        // This was a depth bound (`staged > frames_per_tick * 8`), which made
        // it a function of how LONG a segment runs rather than of whether the
        // output is moving. A pretest segment at nPts 32768 runs 512 ticks
        // against 128 at 8192, so the same healthy backpressure that never
        // reached the bound in a short segment sailed past it in a long one:
        // the acquisition was cut off after 9 ticks and returned 2304 frames
        // against a 131072-frame drive, silently, which is what destroyed the
        // FRF at large block sizes (root cause found 2026-08-03).
        //
        // Same lesson as the stall watchdog, one layer down: a threshold
        // that a legitimate configuration can cross is a false alarm waiting
        // for a big enough test. A backend that has accepted NOTHING for this
        // many consecutive ticks is genuinely stuck no matter how deep the
        // queue is; one that is still accepting is merely busy. The absolute
        // ceiling remains as a memory backstop, an order of magnitude above
        // any real backpressure.
        // DEPTH ALONE IS NOT A FAULT; depth WITHOUT PROGRESS is. A deep queue
        // on a backend that is still accepting is a long segment doing its
        // job. A queue that is not draining at all is stuck no matter how
        // shallow. Requiring both is what lets a 512-tick pretest run while
        // still catching a dead output within a couple of ticks of it dying.
        let deep = self.staged_out.len() / out_ch > self.max_staged_frames;
        // One tick, not two: a never-accepting output must fault at exactly
        // the depth it always did (the delivery tests pin that timing against
        // a short input script), while a queue that accepted anything on its
        // most recent tick is draining and gets to stay deep.
        let no_progress = self.zero_accept_ticks >= 1;
        let memory_cap = self.max_staged_frames.saturating_mul(16);
        if (deep && no_progress)
            || self.zero_accept_ticks >= MAX_STAGED_BLOCKS as u32
            || self.staged_out.len() / out_ch > memory_cap
        {
            let _ = backend.emergency_stop();
            // The message carries the PRODUCER/CONSUMER accounting, because the
            // two ways to reach this point need opposite fixes and the depth
            // alone cannot tell them apart: an output that DIED (accepted ~0 of
            // everything staged) versus one that is merely SLOWER than the tick
            // loop feeds it (accepted most of it, steadily falling behind). The
            // second is a rate mismatch upstream, and reading "stalled" for it
            // sends the next reader hunting a dead device that is in fact alive.
            let staged = self.total_staged_frames.max(1);
            let pct = (self.total_accepted_frames as f64 / staged as f64) * 100.0;
            return Some(StopReason::BackendError(format!(
                "drive output stalled: {} frames staged, nothing accepted for {} \
                 consecutive ticks (memory cap {}); over {} ticks the loop staged {} \
                 frames and the device accepted {} ({pct:.1}%), so the output is \
                 {}",
                self.staged_out.len() / out_ch,
                self.zero_accept_ticks,
                memory_cap,
                self.delivery_ticks,
                self.total_staged_frames,
                self.total_accepted_frames,
                if pct < 5.0 {
                    "not draining at all"
                } else {
                    "draining but SLOWER than the loop feeds it - a rate mismatch, \
                     not a dead device"
                }
            )));
        }
        None
    }
}
