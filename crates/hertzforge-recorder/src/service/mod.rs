//! `RecorderService` - the Data Recorder run orchestration.
//!
//! The service owns a backend (whatever the station factory built; the seam is generic
//! so a real backend drops in later), runs the engine MONITOR-ONLY (SilentDrive feeds
//! zeros into a synthetic drive channel), streams every input frame into the analysis
//! pipeline always and into the recording writer + trigger engine while
//! armed/recording, and assembles the live [`RecStatus`] + serde [`ReportData`]
//! surfaces.
//!
//! # The engine table clone
//!
//! [`Engine`] runs against a CLONE of the user channel table with two edits:
//!
//! * **One synthetic Drive channel is injected** (`"SILENT (recorder)"`).
//!   a host-loop backend's `write_drive` errors when the table maps ZERO Drive rows, and
//!   the engine drive path runs on every `HostClosesLoop` tick, so a table with
//!   no Drive row would fault on tick 1. The injected drive is fed zeros by
//!   [`SilentDrive`] (harmless; the recorder drives nothing).
//! * **When `watchdog_enabled == false` (the default) all abort
//!   limits are STRIPPED from the clone.** The engine watchdog skips channels
//!   with no abort axes, so it monitors nothing. The per-channel OVERLOAD
//!   latches still run on the REAL table inside the service. When
//!   `watchdog_enabled == true`, `AbortWatchdog::poll` may call
//!   `emergency_stop` on the recorder's OWN backend only (the recorder owns no
//!   other device); a trip faults the run.
//!
//! # The segment-run monitor loop
//!
//! The engine is run per SEGMENT ([`Engine::new`] + [`Engine::run`] with a
//! bounded `max_ticks_per_segment`), exactly like hertzforge-random's `PlantLoop`
//! and the sine service, WITH the [`StopReason::SchedulingFault`] RETRY (a
//! wall-clock artifact of the deterministic harness): re-arm / re-start /
//! re-run, bounded attempts. Each segment's captured frames flow through the
//! [`CaptureBridge`] (an [`EngineCaptureSink`]) and the service then drains
//! them into analysis (always) and the record/trigger paths (when
//! armed/recording). The bridge's `finalize()` is an IDEMPOTENT NO-OP - a
//! segment boundary calls it, but the record writer is closed only at service
//! stop. The RECORD path has priority; analysis is drop-tolerant.
//!
//! Timed recordings count FRAMES (`timed_record_s * native_rate`), because the
//! Sim plant free-runs with no wall clock.

pub mod config;
pub mod report;

use std::path::PathBuf;

use hertzforge_channel_table::{AbortLimits, Channel, ChannelRole, ChannelTable, Units};
use hertzforge_engine::capture::{BlockMeta, CaptureError, CaptureSink as EngineCaptureSink};
use hertzforge_engine::{Engine, EngineConfig, ProcessStep, SilentDrive, StopReason};
use hertzforge_hal::Backend;

use crate::analysis::{
    BlockAssembler, ChannelDecimator, FrfPipeline, SpectrumPipeline, TimeStats,
    TimeStatsAccumulator,
};
use crate::recording::{
    recorder_disk_budget_seconds, RecordingInfo, RecordingReader, RecordingSummary, RecordingWriter,
};
use crate::trigger::{TriggerEngine, TriggerEvent, DEFAULT_PRETRIGGER_S};

pub use config::{MetaField, RecState, RecorderConfig};
pub use report::{
    ChannelMeter, ChannelRow, ChannelStat, EventLog, FrfSnapshot, Honesty, RecEvent, RecStatus,
    RecordingInfoLite, ReportData, SessionRecording,
};

/// The synthetic-drive channel label injected into the engine table clone.
pub const SILENT_DRIVE_LABEL: &str = "SILENT (recorder)";

/// Default segment length in frames (~0.25 s of the 65,536 Hz native rate,
/// rounded to a power of two): short enough that a scheduling-fault blip only
/// costs one segment, long enough that per-segment overhead is negligible.
pub const DEFAULT_SEGMENT_FRAMES: u64 = 16_384;

/// Frames per acquisition block the recorder REQUESTS at configure. The engine's
/// `frames_per_tick` then comes from what the backend answers, not from this
/// constant.
///
/// It used to be a hard-coded 1 with a comment citing "the Sim latches ONE
/// drive value per block". That constraint was removed (the Sim now
/// buffers the full per-frame drive stream), so the comment was false. The
/// recorder keeps the HERTZFORGE-SIM natural block for the reason named by
/// `SIM_NATURAL_BLOCK_FRAMES` - its segment budget is counted in SAMPLES and
/// handed to the engine as `max_ticks`. A hardware station overrides it via
/// `RecorderService::with_block_frames`, and the recorder's drive is silent zeros
/// either way, so the block only ever affects the INPUT cadence here.
const DEFAULT_BLOCK_FRAMES: usize = hertzforge_engine::SIM_NATURAL_BLOCK_FRAMES;

/// Bounded retry count for a spurious wall-clock [`StopReason::SchedulingFault`]
/// (the deterministic harness timing artifact).
const MAX_SEGMENT_ATTEMPTS: u32 = 8;

/// The HERTZFORGE-SIM abort-watchdog debounce, in RAW FRAMES (~61 us at the Sim
/// plant's 65 536 Hz). Unchanged from the value this module has always used.
const SIM_WATCHDOG_SUSTAIN_FRAMES: u32 = 4;

/// The abort-watchdog debounce on a HARDWARE station, in SECONDS.
///
/// The same ~60 us [`SIM_WATCHDOG_SUSTAIN_FRAMES`] represents at the Sim rate,
/// in the unit that survives a rate change. A monitor should not false-trip on a
/// single stray sample, and it also must not sit over a limit for a rate-
/// dependent length of time before acting - see
/// `hertzforge_engine::watchdog_sustain_frames`. At any rate below ~16 kHz this
/// collapses to 1 frame, which is the correct direction: fewer samples per unit
/// time means a shorter debounce, not a longer one.
const WATCHDOG_SUSTAIN_SECONDS: f64 = 0.00006;

/// Errors from a recorder service operation.
#[derive(Debug, Clone, PartialEq)]
pub enum RecorderError {
    /// The config was invalid.
    Config(String),
    /// The operation is not allowed in the current state.
    BadState(String),
    /// The backend refused an operation, or the engine reported a
    /// BackendError / watchdog trip.
    Backend(String),
    /// A capture / IO error.
    Io(String),
}

impl std::fmt::Display for RecorderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecorderError::Config(s) => write!(f, "config error: {s}"),
            RecorderError::BadState(s) => write!(f, "bad state: {s}"),
            RecorderError::Backend(s) => write!(f, "backend error: {s}"),
            RecorderError::Io(s) => write!(f, "io error: {s}"),
        }
    }
}

impl std::error::Error for RecorderError {}

/// Per-channel overload proxy latch: latches when the sample
/// magnitude reaches `overload_fraction * range_max` on the REAL table.
#[derive(Debug, Clone, PartialEq)]
struct OverloadLatch {
    /// The enabled-input slot this latch watches.
    slot: usize,
    /// The overload trip level in EU (`fraction * range_max`).
    trip_level: f64,
    /// Whether it has latched.
    latched: bool,
    /// Whether the latch event has been logged (log once).
    logged: bool,
}

/// The live analysis state for one enabled input channel.
struct ChannelAnalysisState {
    decim: ChannelDecimator,
    asm: BlockAssembler,
    spectrum: SpectrumPipeline,
    stats: TimeStatsAccumulator,
    /// Whole-session windowed stats (RMS/peak for the meters + report).
    last_stats: TimeStats,
}

/// The FRF lockstep path state (two decimators + one 2-channel assembler),
/// carried across segments so the block boundaries are continuous.
struct FrfState {
    /// The pair in enabled-input slots.
    pair: (usize, usize),
    /// A two-channel decimator (slot 0 = x, slot 1 = y).
    decim: ChannelDecimator,
    /// The lockstep 2-channel block assembler.
    asm: BlockAssembler,
    /// The FRF accumulator.
    pipeline: FrfPipeline,
}

// REMOVED 2026-07-16: `FrameToSampleAdapter`.
//
// It wrapped the backend and multiplied `read_input`'s FRAME count by the input-channel
// count, so that the engine's (incorrect) `return / in_channels` division would cancel it
// back out and recover the true frame count. Its own doc said it "drops away the moment
// the engine's contract is unified." That has now happened: `hertzforge-engine`'s scheduler
// takes `read_input`'s return as FRAMES, which is what `hertzforge_hal::Backend::read_input`
// always specified and what every backend always returned. With the division gone, the
// compensating multiply here would over-count by `in_channels` squared, so the adapter is
// deleted rather than kept.
//
// The recorder was the only module carrying this antidote, which is why it was the only
// one that survived multi-channel input; every other module stalled on tick 1 or silently
// dropped (N-1)/N of the block. See `hertzforge-engine/tests/engine.rs`
// `multichannel_tail_breach_still_reaches_the_watchdog` for the regression that pins it.

/// The capture-sink BRIDGE: an [`EngineCaptureSink`] the engine writes each
/// tick's raw block through. It BUFFERS the segment's interleaved input frames;
/// the service drains the buffer into the analysis / record / trigger paths
/// after each segment returns. This avoids a re-entrant borrow (the engine
/// borrows the sink while also borrowing the backend), keeps the RECORD path
/// prioritized, and makes the analysis path drop-countable. `finalize()`
/// defaults to a no-op: a segment boundary must not close the
/// record sink.
#[derive(Default)]
struct CaptureBridge {
    /// Interleaved input frames captured this segment (`n_channels` per frame).
    frames: Vec<f32>,
    /// Frame count captured this segment.
    n_frames: u64,
}

impl EngineCaptureSink for CaptureBridge {
    fn write_block(&mut self, samples: &[f32], meta: BlockMeta) -> Result<(), CaptureError> {
        self.frames.extend_from_slice(samples);
        self.n_frames += meta.n_frames;
        Ok(())
    }
    // finalize() -> default no-op (idempotent segment boundary).
}

/// The host-side Recorder run service. Owns the backend + the analysis pipeline
/// + the recording lifecycle + the event log.
///
/// Generic over the backend `B`, defaulting to the station factory's product
/// `Box<dyn Backend>`. Nothing in the loop assumes a Sim plant.
///
/// The default type parameter was `SimBackend` and a `RecorderService<SimBackend>`
/// constructor built one from a `SimConfig`, which pinned the Tauri layer's four
/// type annotations to a concrete Sim. Both are gone - `with_backend` is now the
/// only constructor and it takes the backend AND the station class the arm gate
/// will be given.
pub struct RecorderService<B: Backend = Box<dyn Backend>> {
    cfg: RecorderConfig,
    /// Driver-programmed rate, retained from the last `configure` (None until
    /// the backend has been up once). See [`RecorderService::actual_rate_hz`].
    actual_rate_hz: Option<f64>,
    /// The REAL user channel table (overload latches + report run on this).
    user_table: ChannelTable,
    /// The engine table clone: synthetic drive injected, abort limits stripped
    /// when the watchdog is off.
    engine_table: ChannelTable,
    /// The recording table: the enabled INPUT channels only, matching the
    /// interleaved frame width the bridge forwards.
    record_table: ChannelTable,
    state: RecState,
    data_root: PathBuf,

    /// The owned backend. Used directly: the engine's
    /// frame accounting is correct for multi-channel input as of the 2026-07-16 scheduler
    /// fix, so the former `FrameToSampleAdapter` wrapper is gone.
    backend: B,
    /// The station class the arm gate is given. Supplied
    /// with the backend at construction; never inferred from `B`.
    station: hertzforge_module_abi::StationClass,
    /// What the watchdog policy decided at construction, and why. Reported on
    /// every status poll so the pane can render the persistent disarmed banner
    /// without re-deriving the rule (a second copy of a safety rule is a second
    /// rule).
    watchdog: crate::service::config::WatchdogArming,
    /// The engine config (interleave width + tick + watchdog knobs). Its
    /// `frames_per_tick` / `ring_frames` are re-derived from `ConfiguredActual`
    /// in `bring_backend_up`.
    ecfg: EngineConfig,
    /// Frames per acquisition block to REQUEST at configure. See
    /// [`DEFAULT_BLOCK_FRAMES`] and `with_block_frames`.
    block_frames: usize,
    /// Whether the backend has been brought up (open/configure/arm/start).
    backend_up: bool,

    /// The enabled-input slot count (interleave width).
    n_input: usize,
    /// Per-enabled-input-channel analysis state.
    analysis: Vec<ChannelAnalysisState>,
    /// The live FRF lockstep path, when a pair is configured and in range.
    frf: Option<FrfState>,

    /// The trigger engine (active while Armed/Recording).
    trigger: Option<TriggerEngine>,

    /// The active recording writer.
    writer: Option<RecordingWriter>,
    /// Frames written to the active recording.
    record_frames: u64,
    /// Target frames for a timed recording (`None` = manual stop).
    record_target_frames: Option<u64>,
    /// Whether the active recording is gap-free so far.
    record_gap_free: bool,
    /// Pre-trigger frames prepended to the active recording (for the report).
    record_pretrigger_frames: u64,

    /// Per-channel overload latches on the REAL table.
    overloads: Vec<OverloadLatch>,

    /// Frames the engine loop has processed this session (the session
    /// clock is frame-derived, not wall-clock).
    session_frames: u64,
    /// The event log.
    log: EventLog,
    /// Recordings produced this session (for the report).
    session_recordings: Vec<SessionRecording>,
    /// Auto-rearm recordings still owed after the current one.
    rearms_remaining: u32,
    /// A monotonically incrementing run counter for run-directory naming.
    run_counter: u32,
    /// The name of the active recording's run directory (for status).
    active_run_name: Option<String>,

    /// How many [`StopReason::SchedulingFault`] retries this session has taken.
    ///
    /// It is OBSERVABILITY ONLY - nothing reads it to make a decision, and the retry
    /// behavior is byte-for-byte what it was. It exists because the soak gate's
    /// predicate is ZERO SchedulingFault over 30+ minutes, and `run_segment` swallows
    /// the retry by design (a spurious wall-clock overrun on a deterministic harness is
    /// not a real fault). Swallowed and uncounted, the predicate was unmeasurable: a
    /// soak could retry a hundred segments and report a clean run. Counted, the
    /// operator sees the number and the gate means something.
    scheduling_faults: u64,
}

impl<B: Backend> RecorderService<B> {
    /// Build a service against a backend `B` - the station factory's
    /// `Box<dyn Backend>`, or a scripted test double. The backend must deliver
    /// exactly the enabled INPUT channels per frame via `read_input` and accept
    /// the synthetic drive via `write_drive`. It must already be sized to the
    /// table's enabled-input count; sizing is the factory's job, not the
    /// service's (a service that resizes its own backend is a service that can
    /// only ever have a synthetic one).
    ///
    /// `station` is the class the arm gate is given. It travels with the backend
    /// because the class is a property of what the backend is bound to, not of
    /// its type.
    ///
    /// This ABSORBED the old `RecorderService::<SimBackend>::new(cfg, table,
    /// sim_cfg, data_root)`, which built a Sim from a `SimConfig` and sized it
    /// here.
    pub fn with_backend(
        cfg: RecorderConfig,
        table: ChannelTable,
        backend: B,
        station: hertzforge_module_abi::StationClass,
        data_root: PathBuf,
    ) -> Result<Self, RecorderError> {
        cfg.validate().map_err(RecorderError::Config)?;
        table
            .validate()
            .map_err(|e| RecorderError::Config(format!("channel table: {e}")))?;

        let data_root = cfg.data_root.clone().unwrap_or(data_root);

        // The enabled INPUT channels (non-drive), in table order - the
        // interleave width the backend delivers and the bridge forwards.
        let input_channels: Vec<Channel> = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .cloned()
            .collect();
        let n_input = input_channels.len();
        if n_input == 0 {
            return Err(RecorderError::Config(
                "no enabled input channels to record".into(),
            ));
        }

        // The record table: exactly the enabled input channels (frame width
        // matches the bridge output). Re-index 1.. so it validates cleanly.
        let mut record_table = ChannelTable::new();
        for (i, ch) in input_channels.iter().enumerate() {
            let mut c = ch.clone();
            c.index = (i + 1) as u32;
            record_table.channels.push(c);
        }

        // The engine table clone: user table + one synthetic drive; abort
        // limits stripped when the watchdog is off.
        //
        // WHAT ARMS is `resolve_watchdog`, not `cfg.watchdog_enabled`: on a hardware
        // station a table that carries abort limits arms by default, because otherwise
        // those limits render in the grid, export into the report, and enforce nothing.
        // `cfg.watchdog_enabled` is the request; this is the answer, and it states its
        // own basis.
        let watchdog = crate::service::config::resolve_watchdog(
            &cfg,
            station,
            crate::service::config::table_has_abort_limits(&table),
        );
        let engine_table = build_engine_table(&table, watchdog.armed);

        // Overload latches on the REAL table (enabled non-drive channels).
        let overloads: Vec<OverloadLatch> = input_channels
            .iter()
            .enumerate()
            .map(|(slot, ch)| OverloadLatch {
                slot,
                trip_level: cfg.overload_fraction * ch.range_max,
                latched: false,
                logged: false,
            })
            .collect();

        // The engine interleave: in = enabled input channels, out = drive
        // channels in the clone (always >= 1 after injection).
        let n_drive = engine_table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Drive)
            .count()
            .max(1);
        // Placeholder cadence: the real one is derived in `open_backend` from what
        // the backend configured, which is the only place the block is known.
        let mut ecfg = EngineConfig::new(n_input, n_drive);
        ecfg.frames_per_tick = DEFAULT_BLOCK_FRAMES;
        // A deliberately deep watchdog tap for a monitor: the recorder's whole job
        // is not to miss frames. `bring_backend_up` only ever RAISES this to the
        // block-derived floor.
        ecfg.ring_frames = 4096;
        // A passive monitor should not false-trip on transients; give the
        // watchdog a small sustain. (Moot when the watchdog is off - the clone
        // carries no limits.) HERTZFORGE-SIM keeps this frame count; a hardware
        // station replaces it in `bring_backend_up`, where the ACTUAL rate is
        // known - see the constants note there.
        ecfg.watchdog_sustain = SIM_WATCHDOG_SUSTAIN_FRAMES;
        // Deterministic single-threaded harness: it runs the engine flat out
        // between segments, so wall-clock tick spacing carries no information.
        ecfg.jitter_frac = hertzforge_engine::TEST_DISABLE_JITTER_GUARD;
        // The tick-period compute budget is meaningless for this harness; relax it
        // explicitly instead of retrying the spurious ComputeOverrun it produces.
        ecfg.compute_budget = Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET);

        let analysis = (0..n_input)
            .map(|_| ChannelAnalysisState {
                decim: ChannelDecimator::new(1, &cfg.acq),
                asm: BlockAssembler::from_config(1, &cfg.acq),
                spectrum: SpectrumPipeline::new(&cfg.acq),
                stats: TimeStatsAccumulator::new(),
                last_stats: empty_stats(),
            })
            .collect();

        let frf = cfg
            .frf_pair
            .filter(|&(x, y)| x < n_input && y < n_input && x != y)
            .map(|pair| FrfState {
                pair,
                decim: ChannelDecimator::new(2, &cfg.acq),
                asm: BlockAssembler::from_config(2, &cfg.acq),
                pipeline: FrfPipeline::new(&cfg.acq),
            });

        Ok(Self {
            cfg,
            actual_rate_hz: None,
            user_table: table,
            engine_table,
            record_table,
            state: RecState::Idle,
            data_root,
            backend,
            station,
            watchdog,
            ecfg,
            block_frames: DEFAULT_BLOCK_FRAMES,
            backend_up: false,
            n_input,
            analysis,
            frf,
            trigger: None,
            writer: None,
            record_frames: 0,
            record_target_frames: None,
            record_gap_free: true,
            record_pretrigger_frames: 0,
            overloads,
            session_frames: 0,
            log: EventLog::default(),
            session_recordings: Vec::new(),
            rearms_remaining: 0,
            run_counter: 0,
            active_run_name: None,
            scheduling_faults: 0,
        })
    }

    /// How many [`StopReason::SchedulingFault`]s this service has seen, retried or
    /// not. OBSERVABILITY ONLY - see the field docs. The soak gate's predicate is
    /// that this reads `0` after >= 30 minutes.
    pub fn scheduling_faults(&self) -> u64 {
        self.scheduling_faults
    }

    /// Request a specific acquisition block (frames per `read_input`) at
    /// configure. A hardware station passes `hertzforge_engine::default_block_frames(fs)`;
    /// the HERTZFORGE-SIM plant path leaves the default (see [`DEFAULT_BLOCK_FRAMES`]).
    /// Must be set BEFORE the backend is brought up.
    pub fn with_block_frames(mut self, frames: usize) -> Self {
        self.block_frames = frames.max(1);
        self
    }

    /// The current run state.
    pub fn state(&self) -> RecState {
        self.state
    }

    /// The monotonic session seconds (frame-derived).
    /// The configured native acquisition rate in Hz. The daemon's pump uses it
    /// to rate-lock delivery pacing (frames delivered are worth `n/rate` seconds
    /// of wall clock, and delivering faster than that on an unpaced backend
    /// would spin a thread hot).
    pub fn native_rate_hz(&self) -> f64 {
        self.cfg.acq.native_rate_hz
    }

    /// The rate the driver ACTUALLY programmed, once the backend is up; the
    /// configured rate until then. The distinction is the HAL rate contract: the
    /// 9234 coerces a 65 536 Hz request to 51 200 Hz, and every wall-clock
    /// budget derived from the REQUEST (pump pacing, the soak's segment budget)
    /// is ~28% wrong on real hardware.
    pub fn actual_rate_hz(&self) -> f64 {
        self.actual_rate_hz.unwrap_or(self.cfg.acq.native_rate_hz)
    }

    pub fn session_seconds(&self) -> f64 {
        self.session_frames as f64 / self.cfg.acq.native_rate_hz
    }

    /// The data root run directories land under.
    pub fn data_root(&self) -> &std::path::Path {
        &self.data_root
    }

    // ---------------------------------------------------------------------
    // State transitions
    // ---------------------------------------------------------------------

    /// Idle -> Configured. Re-validates the config.
    pub fn configure(&mut self) -> Result<(), RecorderError> {
        self.cfg.validate().map_err(RecorderError::Config)?;
        self.transition(RecState::Configured);
        Ok(())
    }

    /// Configured/Stopped -> Monitoring. Brings the backend up and starts the
    /// live engine loop bookkeeping (the loop advances via
    /// [`RecorderService::pump`]). No disk writes in Monitoring.
    pub fn start_monitor(&mut self) -> Result<(), RecorderError> {
        if !matches!(self.state, RecState::Configured | RecState::Stopped) {
            return Err(RecorderError::BadState(format!(
                "start_monitor requires Configured/Stopped, was {:?}",
                self.state
            )));
        }
        self.bring_backend_up()?;
        self.transition(RecState::Monitoring);
        Ok(())
    }

    /// Monitoring -> Armed. Activates the trigger engine + pre-trigger ring.
    pub fn arm(&mut self) -> Result<(), RecorderError> {
        if self.state != RecState::Monitoring {
            return Err(RecorderError::BadState(format!(
                "arm requires Monitoring, was {:?}",
                self.state
            )));
        }
        let pretrigger_s = self.cfg.pretrigger_s.unwrap_or(DEFAULT_PRETRIGGER_S);
        self.trigger = Some(TriggerEngine::new(
            self.cfg.trigger_mode.clone(),
            self.n_input,
            self.cfg.acq.native_rate_hz,
            pretrigger_s,
            self.cfg.auto_rearm,
        ));
        self.rearms_remaining = self.cfg.auto_rearm;
        self.transition(RecState::Armed);
        Ok(())
    }

    /// Start a recording NOW (manual fire). Valid from Monitoring or Armed. If
    /// armed with a filled pre-trigger ring, the pre-window is prepended.
    pub fn record_now(&mut self) -> Result<(), RecorderError> {
        if !matches!(self.state, RecState::Monitoring | RecState::Armed) {
            return Err(RecorderError::BadState(format!(
                "record_now requires Monitoring/Armed, was {:?}",
                self.state
            )));
        }
        let pretrigger = self
            .trigger
            .as_mut()
            .map(|t| t.manual_fire())
            .filter(|ev| ev.pretrigger_frames > 0);
        self.begin_recording(pretrigger)
    }

    /// Stop the monitor loop entirely -> Stopped. Finalizes any active
    /// recording and tears the backend down.
    pub fn stop_monitor(&mut self) -> Result<(), RecorderError> {
        if self.writer.is_some() {
            self.finish_recording()?;
        }
        self.trigger = None;
        self.tear_backend_down();
        self.transition(RecState::Stopped);
        Ok(())
    }

    // ---------------------------------------------------------------------
    // The monitor loop pump (segment-run pattern)
    // ---------------------------------------------------------------------

    /// Run ONE monitor segment ([`DEFAULT_SEGMENT_FRAMES`]) with the
    /// SchedulingFault retry, then drain the captured frames into the
    /// analysis / record / trigger paths. Returns the frames processed. A
    /// `BackendError` or watchdog trip latches the service Faulted (message
    /// preserved) and returns `Err`. Each segment is bounded so no call hangs.
    pub fn pump(&mut self) -> Result<u64, RecorderError> {
        self.pump_frames(DEFAULT_SEGMENT_FRAMES)
    }

    /// [`RecorderService::pump`] with an explicit per-segment frame budget
    /// (exposed for fast tests).
    pub fn pump_frames(&mut self, segment_frames: u64) -> Result<u64, RecorderError> {
        if !self.state.is_live() {
            return Err(RecorderError::BadState(format!(
                "pump requires a live state, was {:?}",
                self.state
            )));
        }
        let (frames, reason) = self.run_segment(segment_frames)?;
        match reason {
            StopReason::BackendError(msg) => {
                self.fault(format!("backend error: {msg}"));
                return Err(RecorderError::Backend(msg));
            }
            StopReason::Disconnected(msg) => {
                // The device link vanished mid-segment (USB re-enumeration /
                // hot-unplug). Latch Faulted like a backend error - a recorder with
                // no device is not recording - but keep the distinct wording so the
                // operator sees "link lost", not a generic backend fault.
                self.fault(format!("device link lost: {msg}"));
                return Err(RecorderError::Backend(format!("device link lost: {msg}")));
            }
            StopReason::AbortTripped(trip) => {
                let msg = format!(
                    "watchdog trip: ch {} axis {:?} value {:.4} limit {:.4}",
                    trip.channel_index, trip.axis, trip.value, trip.limit
                );
                self.fault(msg.clone());
                return Err(RecorderError::Backend(msg));
            }
            StopReason::CaptureFatal(m) => {
                self.fault(format!("capture fatal: {m}"));
                return Err(RecorderError::Io(m));
            }
            // The acquisition went SILENT. A recorder whose
            // input stalled is not recording, and the blanket `_ => {}` below would
            // have let it pump empty segments forever while reporting healthy.
            StopReason::InputStalled { elapsed } => {
                let msg = format!("input stalled: no frames for {elapsed:?}");
                self.fault(msg.clone());
                return Err(RecorderError::Backend(msg));
            }
            // Raw frames the watchdog never examined, on a
            // run with limits armed. A monitor that dropped frames did not monitor.
            StopReason::RingOverrun { frames } => {
                let msg = format!("raw ring overrun: {frames} frame(s) dropped with limits armed");
                self.fault(msg.clone());
                return Err(RecorderError::Backend(msg));
            }
            _ => {}
        }
        Ok(frames)
    }

    /// Latch the service Faulted, finalizing any active recording and tearing
    /// the backend down, with the fault message logged.
    fn fault(&mut self, msg: String) {
        self.log
            .push(self.session_seconds(), format!("FAULT: {msg}"));
        if self.writer.is_some() {
            let _ = self.finish_recording();
        }
        self.tear_backend_down();
        self.transition(RecState::Faulted);
    }

    /// Run the engine for one segment with the SchedulingFault retry, returning
    /// the captured frames + the accepted stop reason, then drain the frames.
    fn run_segment(&mut self, segment_frames: u64) -> Result<(u64, StopReason), RecorderError> {
        let mut bridge = CaptureBridge::default();
        let mut reason;
        let mut attempt = 0u32;
        loop {
            // Re-arm/start so the plant is Running with a clean drive path; the
            // backend state persists so the plant/noise stream is continuous.
            let _ = self.backend.arm();
            let _ = self.backend.start();
            let mut engine = Engine::new(clone_ecfg(&self.ecfg), &self.engine_table, &self.backend);
            let mut step = SilentDrive {
                out_channels: self.ecfg.out_channels,
            };
            bridge.frames.clear();
            bridge.n_frames = 0;
            // The segment budget is FRAMES; the engine's run budget is TICKS.
            // They were numerically equal for the HERTZFORGE-SIM plant's natural
            // block of 1 frame, and that hidden equivalence became a defect the
            // day a hardware station set a real block (2026-07-30): 16384
            // "ticks" x 1024-frame blocks is 5.5 MINUTES of driver-paced
            // acquisition per pump call, with the service mutex held the whole
            // way - the daemon looked wedged, StopMonitor starved behind the
            // lock, and the segment's capture bridge accumulated ~330 MB.
            let fpt = self.ecfg.frames_per_tick.max(1) as u64;
            let ticks = segment_frames.div_ceil(fpt);
            // `run_ticks`, NOT `run`: the monitor session is ONE continuous
            // acquisition; `run`'s backend-stop epilogue at every segment
            // boundary stopped the NI tasks, the silent re-start below failed,
            // and the next segment stalled against a dead AO (2026-07-30).
            // The service owns the real stop (`tear_backend_down`).
            let report = engine.run_ticks(
                &mut self.backend,
                &mut step as &mut dyn ProcessStep,
                &mut bridge as &mut dyn EngineCaptureSink,
                ticks,
            );
            reason = report.reason;
            if matches!(reason, StopReason::SchedulingFault(_)) {
                // COUNTED even when retried and even on the final attempt: the
                // soak gate asks how many happened, not how many were fatal.
                // See the field docs on `scheduling_faults`.
                self.scheduling_faults += 1;
            }
            if matches!(reason, StopReason::SchedulingFault(_))
                && attempt + 1 < MAX_SEGMENT_ATTEMPTS
            {
                attempt += 1;
                continue; // spurious wall-clock overrun; retry a clean segment
            }
            break;
        }

        let frames = bridge.n_frames;
        let interleaved = std::mem::take(&mut bridge.frames);
        self.drain_segment(&interleaved, frames)?;
        Ok((frames, reason))
    }

    /// Drain a segment's interleaved frames into: overload latches (real
    /// table), trigger engine (while armed), recording writer (while
    /// recording; RECORD path has priority), and analysis (always).
    fn drain_segment(&mut self, interleaved: &[f32], n_frames: u64) -> Result<(), RecorderError> {
        if n_frames == 0 || self.n_input == 0 {
            return Ok(());
        }
        let n = n_frames as usize;

        // 1. Overload latches on the real table.
        self.update_overloads(interleaved, n);

        // 2. Trigger engine (while Armed). A fire starts a recording with the
        //    pre-window prepended, then this segment's frames from the trigger
        //    index onward are written as the gap-free live continuation. The
        //    trigger borrow is scoped and dropped BEFORE begin_recording so the
        //    &mut self call is legal.
        if self.state == RecState::Armed {
            let fire: Option<(TriggerEvent, usize)> = self.trigger.as_mut().and_then(|trig| {
                trig.feed(interleaved, n).map(|ev| {
                    // feed advances the full block, so the trigger's global
                    // counter is now the segment END; the segment START is
                    // `end - n`, and the trigger's local index is
                    // `trigger_global - (end - n)`.
                    let end = trig.global_frame();
                    let seg_start_global = end.saturating_sub(n as u64);
                    let local = ev.trigger_frame_global.saturating_sub(seg_start_global) as usize;
                    (ev, local)
                })
            });
            if let Some((ev, local)) = fire {
                self.begin_recording(Some(ev))?;
                if local < n {
                    if let Some(w) = self.writer.as_mut() {
                        let tail = &interleaved[local * self.n_input..];
                        let tail_frames = n - local;
                        w.push_frames(tail, tail_frames);
                        self.record_frames = w.frames_fed();
                        self.record_gap_free = w.gaps().is_empty();
                    }
                    if let Some(target) = self.record_target_frames {
                        if self.record_frames >= target {
                            self.complete_recording()?;
                        }
                    }
                }
            }
        } else if self.state == RecState::Recording {
            // 3. RECORD path (priority): write every frame.
            if let Some(w) = self.writer.as_mut() {
                w.push_frames(interleaved, n);
                self.record_frames = w.frames_fed();
                self.record_gap_free = w.gaps().is_empty();
            }
            if let Some(target) = self.record_target_frames {
                if self.record_frames >= target {
                    self.complete_recording()?;
                }
            }
        }

        // 4. Analysis path (ALWAYS; drop-tolerant).
        self.feed_analysis(interleaved, n);

        self.session_frames += n_frames;
        Ok(())
    }

    /// Feed the analysis pipelines (per-channel spectra + stats + optional
    /// FRF). Drop-tolerant: analysis never back-pressures the record
    /// path (both drive from the same in-memory buffer, so there is nothing to
    /// drop here; the real threaded assembler's drop counter is a documented
    /// seam).
    fn feed_analysis(&mut self, interleaved: &[f32], n_frames: usize) {
        let n_ch = self.n_input;
        for slot in 0..n_ch {
            let samples: Vec<f64> = (0..n_frames)
                .map(|f| f64::from(interleaved[f * n_ch + slot]))
                .collect();
            let a = &mut self.analysis[slot];
            a.stats.push_slice(&samples);
            let dec = a.decim.process(0, &samples);
            a.asm.push(0, &dec);
            while let Some(blocks) = a.asm.next_block() {
                a.spectrum.push_block(&blocks[0]);
            }
            a.last_stats = a.stats.finish();
        }
        // FRF pair through the dedicated lockstep 2-channel path.
        if let Some(frf) = self.frf.as_mut() {
            let (xs, ys) = frf.pair;
            let x: Vec<f64> = (0..n_frames)
                .map(|f| f64::from(interleaved[f * n_ch + xs]))
                .collect();
            let y: Vec<f64> = (0..n_frames)
                .map(|f| f64::from(interleaved[f * n_ch + ys]))
                .collect();
            let xd = frf.decim.process(0, &x);
            let yd = frf.decim.process(1, &y);
            frf.asm.push(0, &xd);
            frf.asm.push(1, &yd);
            while let Some(blocks) = frf.asm.next_block() {
                frf.pipeline.push_block(&blocks[0], &blocks[1]);
            }
        }
    }

    /// Update the per-channel overload latches from a segment's frames.
    fn update_overloads(&mut self, interleaved: &[f32], n_frames: usize) {
        let n_ch = self.n_input;
        let session_s = self.session_seconds();
        let mut newly: Vec<usize> = Vec::new();
        for latch in self.overloads.iter_mut() {
            if latch.latched {
                continue;
            }
            for f in 0..n_frames {
                let v = f64::from(interleaved[f * n_ch + latch.slot]).abs();
                if v >= latch.trip_level {
                    latch.latched = true;
                    if !latch.logged {
                        latch.logged = true;
                        newly.push(latch.slot);
                    }
                    break;
                }
            }
        }
        for slot in newly {
            self.log.push(
                session_s,
                format!("overload latched on channel slot {slot}"),
            );
        }
    }

    // ---------------------------------------------------------------------
    // Recording lifecycle
    // ---------------------------------------------------------------------

    /// Begin a recording (from a trigger fire or record_now). Prepends the
    /// pre-window when given. Transitions to Recording.
    fn begin_recording(&mut self, pretrigger: Option<TriggerEvent>) -> Result<(), RecorderError> {
        self.run_counter += 1;
        let run_name = format!("rec_{:04}", self.run_counter);
        let pre_frames = pretrigger.as_ref().map_or(0, |e| e.pretrigger_frames);
        let writer = RecordingWriter::start(
            &self.data_root,
            &run_name,
            &self.cfg.acq,
            self.record_table.clone(),
            pretrigger,
        )
        .map_err(|e| RecorderError::Io(e.to_string()))?;
        // The writer's run dir name may differ from the requested name after
        // sanitize/de-collision; track the real one.
        let real_name = writer
            .run_dir()
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or(run_name);
        self.record_frames = writer.frames_fed();
        self.record_gap_free = writer.gaps().is_empty();
        self.record_pretrigger_frames = pre_frames;
        self.writer = Some(writer);
        self.active_run_name = Some(real_name.clone());
        self.record_target_frames = self.cfg.timed_record_frames();
        let session_s = self.session_seconds();
        self.log.push(
            session_s,
            format!(
                "record start: {real_name} (pretrigger {pre_frames} frames{})",
                self.record_target_frames
                    .map(|t| format!(", timed {t} frames"))
                    .unwrap_or_default()
            ),
        );
        self.transition(RecState::Recording);
        Ok(())
    }

    /// Complete a timed / trigger-tail recording: finalize, then either
    /// auto-rearm (back to Armed) or return to Monitoring.
    fn complete_recording(&mut self) -> Result<(), RecorderError> {
        self.finish_recording()?;
        if self.rearms_remaining > 0 {
            self.rearms_remaining -= 1;
            // The trigger engine's own holdoff (>= ring depth) prevents the
            // next pre-window from overlapping this run's tail. Re-arm the
            // detector and keep the trigger active.
            if let Some(trig) = self.trigger.as_mut() {
                trig.rearm(self.rearms_remaining);
                self.transition(RecState::Armed);
            } else {
                self.transition(RecState::Monitoring);
            }
        } else {
            self.transition(RecState::Monitoring);
        }
        Ok(())
    }

    /// Manually stop the active recording (operator stop) -> Monitoring.
    pub fn stop_recording(&mut self) -> Result<(), RecorderError> {
        if self.state != RecState::Recording {
            return Err(RecorderError::BadState(format!(
                "stop_recording requires Recording, was {:?}",
                self.state
            )));
        }
        self.finish_recording()?;
        self.transition(RecState::Monitoring);
        Ok(())
    }

    /// Finalize the active recording writer, record the session summary + event.
    fn finish_recording(&mut self) -> Result<(), RecorderError> {
        let Some(writer) = self.writer.take() else {
            return Ok(());
        };
        let summary: RecordingSummary = writer
            .finish()
            .map_err(|e| RecorderError::Io(e.to_string()))?;
        let name = summary
            .run_dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let session_s = self.session_seconds();
        self.log.push(
            session_s,
            format!(
                "record stop: {name} ({} frames, gap_free={})",
                summary.frames_written, summary.gap_free
            ),
        );
        if !summary.gaps.is_empty() {
            self.log.push(
                session_s,
                format!("gaps in {name}: {} drop record(s)", summary.gaps.len()),
            );
        }
        self.session_recordings.push(SessionRecording {
            name,
            frames: summary.frames_written,
            gap_free: summary.gap_free,
            gaps: summary.gaps,
            pretrigger_frames: self.record_pretrigger_frames,
        });
        self.record_frames = 0;
        self.record_target_frames = None;
        self.record_gap_free = true;
        self.record_pretrigger_frames = 0;
        self.active_run_name = None;
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Backend lifecycle
    // ---------------------------------------------------------------------

    fn bring_backend_up(&mut self) -> Result<(), RecorderError> {
        if self.backend_up {
            return Ok(());
        }
        self.backend
            .open()
            .map_err(|e| RecorderError::Backend(format!("open: {e}")))?;
        let acq = hertzforge_hal::AcquisitionConfig::new(
            self.backend.caps().sample_rate_hz,
            self.block_frames,
        );
        let actual = self
            .backend
            .configure(&self.engine_table, &acq)
            .map_err(|e| RecorderError::Backend(format!("configure: {e}")))?;
        self.actual_rate_hz = Some(actual.actual_rate_hz);

        // The engine's block cadence, ring depth and scheduling-health budget all
        // come from what the backend ACTUALLY configured, not from the request.
        let derived =
            EngineConfig::with_block(self.ecfg.in_channels, self.ecfg.out_channels, &actual);
        self.ecfg.frames_per_tick = derived.frames_per_tick;
        // The derivation is a FLOOR, never a ceiling: a recorder that deliberately
        // asked for a deeper watchdog tap keeps it. Shrinking a configured ring to
        // a computed minimum would quietly reduce how far the watchdog may fall
        // behind before raw frames start being dropped.
        self.ecfg.ring_frames = self.ecfg.ring_frames.max(derived.ring_frames);
        // Constants note: `watchdog_sustain` counts RAW FRAMES, so the module's 4
        // is a time only against the HERTZFORGE-SIM plant's rate. This is the ONLY place
        // the ACTUAL rate is known, and it matters more here than in any other
        // module: this watchdog ARMS BY DEFAULT on a hardware station carrying
        // abort limits, so its debounce is a live safety number rather than a
        // monitor's tuning preference.
        if self.station.is_hardware() {
            self.ecfg.watchdog_sustain = hertzforge_engine::watchdog_sustain_frames(
                WATCHDOG_SUSTAIN_SECONDS,
                actual.actual_rate_hz,
            );
        }

        // THE ARM GATE. The Recorder requests `{HostSynthesized, None}`: the
        // host produces drive samples and NOTHING corrects them.
        //
        // Note the deliberate divergence from the `hertzforge-module-abi` resolution table,
        // whose row 14 lists "Recorder / any -> {None, None}, CaptureOnly falls out".
        // That describes a recorder with no drive path. THIS recorder has one: the
        // service injects a synthetic drive channel into the engine table because the
        // engine's drive path runs on every `HostClosesLoop` tick and a host-loop
        // backend's `write_drive` errors on a table with zero Drive rows, so
        // `SilentDrive` feeds it zeros. Requesting `{None, None}` would let this
        // service ARM against a device-loop backend and then fault on tick 1 when
        // `write_drive` came back `Unsupported` - which is exactly the failure the gate
        // exists to move forward to arm time. The request must name the drive path the
        // module actually uses, not the one it wishes it had.
        //
        // Consequence, and it is the conservative one: on a hardware station
        // the abort-latency rule now binds the Recorder too. Its drive is
        // zeros and energizes nothing, but a safety gate that reasons about
        // what the samples CONTAIN rather than whether a drive path exists is
        // a gate that can be argued around later.
        //
        // The factory supplies the class; it is held on the service from
        // construction so the gate reads the SAME value the caller stated.
        hertzforge_module_abi::arm_gate(
            &self.backend.caps(),
            self.station,
            hertzforge_module_abi::Resolution::host_open_loop(),
            None,
            actual.actual_rate_hz,
        )
        .map_err(|reason| RecorderError::Backend(format!("arm refused: {reason}")))?;

        self.backend
            .arm()
            .map_err(|e| RecorderError::Backend(format!("arm: {e}")))?;
        // An NI-class AO REFUSES to start on an empty buffer (-200462, found by
        // the first cross-host wire probe, 2026-07-25). The recorder's drive is
        // ALWAYS silence (SilentDrive feeds the injected synthetic row), so the
        // prefill is a lookahead of zeros written between arm and start. Sim
        // accepts-and-ignores this; a backend with no output contract skips it.
        let caps = self.backend.caps();
        if let Some(contract) = caps.output_buffer {
            let n_out = caps.max_output_channels as usize;
            if n_out > 0 && contract.min_lookahead_frames > 0 {
                let zeros = vec![0.0f32; contract.min_lookahead_frames as usize * n_out];
                let mut offset = 0usize;
                while offset < zeros.len() {
                    match self.backend.write_drive(&zeros[offset..]) {
                        Ok(0) | Err(_) => break, // no space / not supported: start decides
                        Ok(frames) => offset += frames * n_out,
                    }
                }
            }
        }
        self.backend
            .start()
            .map_err(|e| RecorderError::Backend(format!("start: {e}")))?;
        self.backend_up = true;
        Ok(())
    }

    fn tear_backend_down(&mut self) {
        if self.backend_up {
            let _ = self.backend.stop();
            let _ = self.backend.close();
            self.backend_up = false;
        }
    }

    // ---------------------------------------------------------------------
    // Live surfaces
    // ---------------------------------------------------------------------

    /// The one-poll live status.
    pub fn status(&self, free_bytes: u64) -> RecStatus {
        let overload_latches = self.overloads.iter().map(|l| (l.slot, l.latched)).collect();
        let (n_averages, averages_complete) = self
            .analysis
            .first()
            .map(|a| (a.spectrum.n_averages(), a.spectrum.complete()))
            .unwrap_or((0, false));
        let coherence_valid = self
            .frf
            .as_ref()
            .is_some_and(|f| f.pipeline.coherence_valid());
        RecStatus {
            state: self.state,
            monitoring_seconds: self.session_seconds(),
            recording_run: self.active_run_name.clone(),
            recording_frames: self.record_frames,
            gap_free_so_far: self.record_gap_free,
            disk_budget_seconds: recorder_disk_budget_seconds(
                &self.cfg.acq,
                self.n_input,
                free_bytes,
            ),
            overload_latches,
            trigger_armed: self.state == RecState::Armed,
            rearms_remaining: self.trigger.as_ref().map_or(0, |t| t.rearms_remaining()),
            n_averages,
            averages_complete,
            coherence_valid,
            watchdog_armed: self.watchdog.armed,
            limits_disarmed: self.watchdog.disarmed_with_limits,
            watchdog_basis: self.watchdog.basis.clone(),
        }
    }

    /// What the watchdog policy decided at construction, and why.
    pub fn watchdog(&self) -> &crate::service::config::WatchdogArming {
        &self.watchdog
    }

    /// The per-channel PSD spectra (enabled-input slot order).
    pub fn spectra(&mut self) -> Vec<(usize, Vec<f64>)> {
        self.analysis
            .iter()
            .enumerate()
            .map(|(slot, a)| (slot, a.spectrum.psd()))
            .collect()
    }

    /// The live FRF snapshot for the configured pair, if any.
    pub fn frf(&mut self) -> Option<FrfSnapshot> {
        let frf = self.frf.as_ref()?;
        let h1_mag = frf
            .pipeline
            .h1()
            .iter()
            .map(|&(re, im)| (re * re + im * im).sqrt())
            .collect();
        Some(FrfSnapshot {
            h1_mag,
            coherence: frf.pipeline.coherence(),
            valid: frf.pipeline.coherence_valid(),
            df_hz: frf.pipeline.df_hz(),
            n_averages: frf.pipeline.n_averages(),
        })
    }

    /// Per-channel live meters (RMS + peak over the accumulated window).
    pub fn meters(&self) -> Vec<ChannelMeter> {
        self.analysis
            .iter()
            .enumerate()
            .map(|(slot, a)| ChannelMeter {
                ch: slot,
                rms: a.last_stats.rms,
                peak: a.last_stats.peak,
            })
            .collect()
    }

    /// List the recordings under `data_root` (or the service's data root when
    /// `None`) as lightweight rows.
    pub fn recordings_list(&self, data_root: Option<&std::path::Path>) -> Vec<RecordingInfoLite> {
        let root = data_root.unwrap_or(&self.data_root);
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(root) else {
            return out;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            if let Ok(reader) = RecordingReader::open(&dir) {
                let info: &RecordingInfo = reader.info();
                out.push(RecordingInfoLite {
                    name: dir
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    frames: info.frames,
                    duration_s: info.duration_s,
                    n_channels: info.n_channels,
                    status: info.status.clone(),
                    gap_free: info.gaps.is_empty() && info.clean,
                });
            }
        }
        out
    }

    /// Assemble the serde report data (settings echo, channel snapshot, recordings,
    /// event log, metadata, honesty notes incl. the concurrent-recording line).
    pub fn report_data(&self) -> ReportData {
        let channels = self
            .user_table
            .channels
            .iter()
            .map(|c| ChannelRow {
                index: c.index,
                role: format!("{:?}", c.role),
                units: units_name(&c.units),
                range_max: c.range_max,
            })
            .collect();
        let channel_stats = self
            .analysis
            .iter()
            .enumerate()
            .map(|(slot, a)| report::ChannelStat::from_stats(slot, &a.last_stats))
            .collect();
        ReportData {
            settings: self.cfg.clone(),
            channels,
            recordings: self.session_recordings.clone(),
            event_log: self.log.clone(),
            metadata: self.cfg.metadata.clone(),
            channel_stats,
            honesty: Honesty::default(),
        }
    }

    // ---------------------------------------------------------------------
    // Internal
    // ---------------------------------------------------------------------

    fn transition(&mut self, to: RecState) {
        if self.state == to {
            return;
        }
        let session_s = self.session_seconds();
        self.log
            .push(session_s, format!("state {:?} -> {:?}", self.state, to));
        self.state = to;
    }
}

/// Build the engine table clone: the user table with (a) ONE
/// synthetic Drive channel injected (fed zeros by SilentDrive), and (b) all
/// abort limits stripped when `watchdog_enabled == false`.
fn build_engine_table(user: &ChannelTable, watchdog_enabled: bool) -> ChannelTable {
    let mut clone = user.clone();
    if !watchdog_enabled {
        for ch in clone.channels.iter_mut() {
            ch.abort = AbortLimits::default();
        }
    }
    let next_index = clone.channels.iter().map(|c| c.index).max().unwrap_or(0) + 1;
    let mut drive = Channel::new(next_index);
    drive.role = ChannelRole::Drive;
    drive.enabled = true;
    drive.save = false;
    drive.description = SILENT_DRIVE_LABEL.to_string();
    clone.channels.push(drive);
    clone
}

/// Clone an [`EngineConfig`] (it is not `Clone`).
fn clone_ecfg(c: &EngineConfig) -> EngineConfig {
    let mut e = EngineConfig::new(c.in_channels, c.out_channels);
    e.frames_per_tick = c.frames_per_tick;
    e.ring_frames = c.ring_frames;
    e.watchdog_hp_hz = c.watchdog_hp_hz;
    e.watchdog_sustain = c.watchdog_sustain;
    e.jitter_frac = c.jitter_frac;
    e.compute_budget = c.compute_budget;
    e.stall_deadline = c.stall_deadline;
    e
}

fn empty_stats() -> TimeStats {
    TimeStats {
        mean: 0.0,
        rms: 0.0,
        peak: 0.0,
        pk_pk: 0.0,
        crest: 0.0,
        skew: 0.0,
        kurtosis: 0.0,
    }
}

fn units_name(u: &Units) -> String {
    match u {
        Units::AccelG => "g".to_string(),
        Units::VelocityIps => "in/s".to_string(),
        Units::DisplacementMil => "mil".to_string(),
        Units::ForceLbf => "lbf".to_string(),
        Units::VoltageV => "V".to_string(),
        Units::Custom(s) => s.clone(),
    }
}
