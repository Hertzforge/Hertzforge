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
//! * **The table's drive rows are left out.** A Recorder run is capture only: the
//!   station is configured with the enabled input rows and nothing else, the engine
//!   has no output channel (its drive path does nothing with none), and the arm gate is
//!   asked for a capture-only run. The operator's table keeps its drive rows for the
//!   modules that drive; this run neither opens nor records them.
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
use hertzforge_engine::{Engine, EngineConfig, ProcessStep, StopReason};
use hertzforge_hal::Backend;

use crate::analysis::{
    BlockAssembler, ChannelDecimator, FrfPipeline, SpectrumPipeline, TimeStats,
    TimeStatsAccumulator,
};
use crate::record::RecordThread;
use crate::recording::{recorder_disk_budget_seconds, GapRecord, RecordingInfo, RecordingReader};
use crate::store::{RecordStore, RecordingSpec, WriterStore};
use crate::trigger::{TriggerEngine, TriggerEvent, DEFAULT_PRETRIGGER_S};

pub use config::{MetaField, RecState, RecorderConfig};
pub use report::{
    ChannelMeter, ChannelRow, ChannelStat, EndCause, EventLog, FrfSnapshot, Honesty, Mark,
    RecEvent, RecStatus, RecordingInfoLite, ReportData, RunFault, SessionRecording,
};

/// The process step of a capture-only run: it reads and produces nothing.
struct NoDrive;

impl ProcessStep for NoDrive {
    fn process(&mut self, _input: &[f32], _n_frames: usize, _drive_out: &mut Vec<f32>) {}
}

/// Blocks in a batch: the most the service reads between two verbs. Two, so a verb
/// waits at most one batch and one analysis pass (40 ms at 12 800 Hz on NI).
pub const BLOCKS_PER_BATCH: u64 = 2;

/// The scheduling-health jitter allowance of a capture-only run, effectively none.
/// A capture-only run drives nothing, so a late read cannot starve an output: the
/// frames wait in the station's input buffer, and a read so late that the buffer
/// overflows is an error the station reports, which ends the run in its words.
pub const CAPTURE_ONLY_JITTER_FRAC: f64 = 1e12;

/// The per-block compute budget of a capture-only run, for the same reason: work
/// that runs late delays the next read, it does not lose a frame.
pub const CAPTURE_ONLY_COMPUTE_BUDGET: std::time::Duration = std::time::Duration::from_secs(1);

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
    /// The rate the station programmed once it is up; the rate asked until then.
    /// Everything per frame is derived from it: the analysis grid, the pre-window,
    /// the holdoff, a timed length, the session clock and both sidecars.
    fs_hz: f64,
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
    /// The run's one engine, built when the station is up.
    engine: Option<Engine>,
    /// The input group delay the station states, frames: given by the caller, else
    /// the station's configured answer, else 0.
    group_delay_given: Option<f64>,
    group_delay_frames: f64,

    /// The enabled-input slot count (interleave width).
    n_input: usize,
    /// Per-enabled-input-channel analysis state.
    analysis: Vec<ChannelAnalysisState>,
    /// The live FRF lockstep path, when a pair is configured and in range.
    frf: Option<FrfState>,

    /// The trigger engine (active while Armed/Recording).
    trigger: Option<TriggerEngine>,

    /// What writes the recordings (the crate's writer unless a test supplies one),
    /// held here until the run starts and moved to the record thread for the run.
    store: Option<Box<dyn RecordStore>>,
    /// The record thread, while the run is up: the only thread that touches a
    /// recording's files.
    rec: Option<RecordThread>,
    /// The recordings handed to the record thread to finish, oldest first, with
    /// their pre-windows and the gaps the service recorded, until it reports them.
    finishing: std::collections::VecDeque<(u64, Vec<GapRecord>)>,
    /// Frames of the recording in progress the record thread took (the file's next
    /// position).
    record_queued: u64,
    /// Gaps the service recorded in the recording in progress.
    record_gaps: Vec<GapRecord>,
    /// The disk fell behind on this pass: its words.
    behind: Option<String>,
    /// Whether a recording is in progress in the store.
    recording: bool,
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

    /// Batches (engine calls) read so far.
    batches: u64,
    /// The analysis pass's last and worst time.
    pass_last: Option<std::time::Duration>,
    pass_worst: Option<std::time::Duration>,
    /// When the last batch's read ended, and the longest gap from there to the next
    /// batch's read.
    last_batch_end: Option<std::time::Instant>,
    batch_gap_max: Option<std::time::Duration>,
    /// Trigger fires since start, and the station frame of the last.
    trigger_fires: u64,
    last_fire_station_frame: Option<u64>,
    /// Recordings finished since start.
    recordings_saved: u64,
    /// How the run ended on its own, if it did.
    run_fault: Option<RunFault>,

    /// Scheduling faults this run has seen: at most one, since one ends the run.
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

        // The record table: exactly the enabled input channels, in table order and
        // with their own indexes (the frame width the bridge forwards; every
        // recording states these rows).
        let mut record_table = ChannelTable::new();
        record_table.channels.extend(input_channels.iter().cloned());

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

        // The engine interleave: in = enabled input channels, out = none. A Recorder
        // run is capture only.
        // Placeholder cadence: the real one is derived in `bring_backend_up` from what
        // the backend configured, which is the only place the block is known.
        let mut ecfg = EngineConfig::new(n_input, 0);
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
        // The capture-only health budget, by name: a late read loses nothing.
        ecfg.jitter_frac = CAPTURE_ONLY_JITTER_FRAC;
        ecfg.compute_budget = Some(CAPTURE_ONLY_COMPUTE_BUDGET);

        let (analysis, frf) = build_analysis(&cfg.acq, n_input, cfg.frf_pair);
        let fs_hz = cfg.acq.native_rate_hz;

        Ok(Self {
            cfg,
            fs_hz,
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
            engine: None,
            group_delay_given: None,
            group_delay_frames: 0.0,
            n_input,
            analysis,
            frf,
            trigger: None,
            store: Some(Box::new(WriterStore::default())),
            rec: None,
            finishing: std::collections::VecDeque::new(),
            record_queued: 0,
            record_gaps: Vec::new(),
            behind: None,
            recording: false,
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
            batches: 0,
            pass_last: None,
            pass_worst: None,
            last_batch_end: None,
            batch_gap_max: None,
            trigger_fires: 0,
            last_fire_station_frame: None,
            recordings_saved: 0,
            run_fault: None,
            scheduling_faults: 0,
        })
    }

    /// Scheduling faults this run has seen: 0, or 1 for the one that ended it.
    pub fn scheduling_faults(&self) -> u64 {
        self.scheduling_faults
    }

    /// State the station's input group delay, frames, for every recording's record
    /// (a station's own figure to 0.01 frames, where its configured answer is whole).
    pub fn with_group_delay_frames(mut self, frames: f64) -> Self {
        self.group_delay_given = Some(frames);
        self
    }

    /// The run's marks, in the order they were made.
    pub fn marks(&self) -> &[Mark] {
        &[]
    }

    /// The input group delay every recording states, frames.
    pub fn group_delay_frames(&self) -> f64 {
        self.group_delay_frames
    }

    /// Put another record store behind the seam (a test double, or a writer with
    /// other settings). Must be set before a recording begins.
    pub fn with_store(mut self, store: Box<dyn RecordStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// The seconds of frames the queue to the record thread holds.
    pub const RECORD_QUEUE_S: f64 = 2.0;

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

    /// The rate the station programmed, once it is up; the rate asked until then.
    /// A station may program another rate than asked (the 9234 programs 8533.33 Hz
    /// for 8192); the run records at the programmed rate and states both.
    pub fn actual_rate_hz(&self) -> f64 {
        self.fs_hz
    }

    /// The free-space floor a recording keeps: the larger of the floor asked (the
    /// writer's least, 1 GiB, when none) and twice a timed recording's bytes, its
    /// pre-window included.
    pub fn floor_bytes(&self) -> u64 {
        let keep = self
            .cfg
            .keep_free_bytes
            .unwrap_or(hertzforge_capture::MIN_FREE_FLOOR_BYTES);
        let pre = (self
            .cfg
            .pretrigger_s
            .unwrap_or(DEFAULT_PRETRIGGER_S)
            .clamp(0.0, crate::trigger::MAX_PRETRIGGER_S)
            * self.fs_hz)
            .round() as u64;
        let twice = self
            .timed_record_frames()
            .map_or(0, |f| 2 * (f + pre) * self.n_input as u64 * 4);
        keep.max(twice)
    }

    /// The acquisition selection at the programmed rate.
    fn acq_now(&self) -> crate::acq::AcquisitionConfig {
        crate::acq::AcquisitionConfig {
            native_rate_hz: self.fs_hz,
            ..self.cfg.acq.clone()
        }
    }

    /// A timed recording's length after its trigger, in frames, or `None` when
    /// recordings run until cut.
    pub fn timed_record_frames(&self) -> Option<u64> {
        self.cfg
            .timed_record_s
            .map(|s| (s * self.fs_hz).round().max(0.0) as u64)
    }

    /// Station seconds since start: the frames read over the programmed rate.
    pub fn session_seconds(&self) -> f64 {
        self.session_frames as f64 / self.fs_hz
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
            self.fs_hz,
            pretrigger_s,
            self.cfg.auto_rearm,
        ));
        if let Some(t) = self.trigger.as_mut() {
            t.sync_to(self.session_frames);
        }
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
        self.trigger_fires += 1;
        self.last_fire_station_frame = Some(self.session_frames);
        self.begin_recording(pretrigger, "manual")
    }

    /// Begin the run's recording from its first frame, with no pre-window: the
    /// `start` mode. Valid before the first batch is read.
    pub fn record_from_start(&mut self) -> Result<(), RecorderError> {
        if self.state != RecState::Monitoring || self.session_frames != 0 {
            return Err(RecorderError::BadState(format!(
                "record_from_start requires Monitoring before the first batch, was {:?} at \
                 frame {}",
                self.state, self.session_frames
            )));
        }
        self.trigger = None;
        self.trigger_fires += 1;
        self.last_fire_station_frame = Some(0);
        self.begin_recording(None, "start")
    }

    /// Stop the monitor loop entirely -> Stopped. Finalizes any active
    /// recording and tears the backend down.
    pub fn stop_monitor(&mut self) -> Result<(), RecorderError> {
        if self.recording {
            self.finish_recording()?;
        }
        self.trigger = None;
        self.tear_backend_down();
        self.close_record_thread();
        if self.state != RecState::Faulted {
            self.transition(RecState::Stopped);
        }
        Ok(())
    }

    /// Wait for the record thread to finish every recording handed to it, collect
    /// what it reports, and take the store back. Called once acquisition has ended.
    fn close_record_thread(&mut self) {
        if let Some(rec) = self.rec.take() {
            let (store, reported) = rec.close();
            self.collect_finished(reported);
            if let Some(store) = store {
                self.store = Some(store);
            }
        }
    }

    /// Take what the record thread has reported, without waiting.
    fn poll_record_thread(&mut self) {
        let reported = self
            .rec
            .as_ref()
            .map(|r| r.take_finished())
            .unwrap_or_default();
        self.collect_finished(reported);
    }

    fn collect_finished(&mut self, reported: Vec<crate::record::Finished>) {
        for f in reported {
            let (pre, service_gaps) = self.finishing.pop_front().unwrap_or_default();
            let session_s = self.session_seconds();
            let summary = match f.result {
                Ok(s) => s,
                Err(e) => {
                    self.log
                        .push(session_s, format!("{} could not be finished: {e}", f.name));
                    continue;
                }
            };
            self.recordings_saved += 1;
            let name = f.name;
            let mut summary = summary;
            for g in service_gaps {
                if !summary.gaps.contains(&g) {
                    summary.gaps.push(g);
                    summary.gap_free = false;
                }
            }
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
                pretrigger_frames: pre,
            });
        }
    }

    // ---------------------------------------------------------------------
    // The monitor loop pump (segment-run pattern)
    // ---------------------------------------------------------------------

    /// Read ONE batch ([`BLOCKS_PER_BATCH`] blocks) and hand it to the overload
    /// latches, the trigger, the recording and the analysis. Returns the frames read.
    /// A verb waits at most this long for the service. Any reason the station or the
    /// engine stops ends the run once, in its own words, every recording finalized,
    /// and returns `Err`.
    pub fn pump(&mut self) -> Result<u64, RecorderError> {
        if !self.state.is_live() {
            return Err(RecorderError::BadState(format!(
                "pump requires a live state, was {:?}",
                self.state
            )));
        }
        self.poll_record_thread();
        let Some(engine) = self.engine.as_mut() else {
            return Err(RecorderError::BadState("the station is not up".into()));
        };
        let mut bridge = CaptureBridge::default();
        if let Some(end) = self.last_batch_end {
            let gap = end.elapsed();
            self.batch_gap_max = Some(self.batch_gap_max.map_or(gap, |g| g.max(gap)));
        }
        // `run_ticks`, not `run`: the run is one continuous acquisition, and the
        // service owns the station's stop (`tear_backend_down`).
        let report = engine.run_ticks(
            &mut self.backend,
            &mut NoDrive as &mut dyn ProcessStep,
            &mut bridge as &mut dyn EngineCaptureSink,
            BLOCKS_PER_BATCH,
        );
        self.last_batch_end = Some(std::time::Instant::now());
        self.batches += 1;
        let frames = bridge.n_frames;
        let interleaved = std::mem::take(&mut bridge.frames);
        // The frames read before any stop are kept: they are drained first.
        self.drain_segment(&interleaved, frames)?;

        // A disk too far behind to take a batch ends the run, the gap recorded.
        if let Some(words) = self.behind.take() {
            self.end_with(EndCause::WriteBehind, words.clone());
            return Err(RecorderError::Io(words));
        }
        // A recording that cannot be written ends the run, read every pass.
        if let Some(words) = self.rec.as_ref().and_then(|r| r.fault()) {
            self.end_with(EndCause::WriteFailed, words.clone());
            return Err(RecorderError::Io(words));
        }

        let words = match report.reason {
            // The batch's budget: the normal end of a batch.
            StopReason::InputEnded if report.ticks >= BLOCKS_PER_BATCH => return Ok(frames),
            StopReason::InputEnded => "the station ended its input stream".to_string(),
            StopReason::BackendError(msg) => format!("station error: {msg}"),
            StopReason::Disconnected(msg) => format!("device link lost: {msg}"),
            StopReason::InputStalled { elapsed } => {
                format!("input stalled: no frames for {elapsed:?}")
            }
            StopReason::SchedulingFault(h) => {
                self.scheduling_faults += 1;
                format!("scheduling fault: {h:?}")
            }
            StopReason::AbortTripped(trip) => format!(
                "watchdog trip: ch {} axis {:?} value {:.4} limit {:.4}",
                trip.channel_index, trip.axis, trip.value, trip.limit
            ),
            StopReason::CaptureFatal(m) => format!("capture fatal: {m}"),
            StopReason::RingOverrun { frames } => {
                format!("raw ring overrun: {frames} frame(s) dropped with limits armed")
            }
            other => format!("the engine stopped: {other:?}"),
        };
        self.end_with(EndCause::Fault, words.clone());
        Err(RecorderError::Backend(words))
    }

    /// Read batches until at least `frames` have been read or the run ends. A test's
    /// convenience: it holds the service for the whole count.
    pub fn pump_frames(&mut self, frames: u64) -> Result<u64, RecorderError> {
        let mut read = 0u64;
        // Bounded: a station that reads nothing for long ends the run by its stall
        // deadline, and this count is the backstop.
        let mut empty = 0u32;
        while read < frames {
            let n = self.pump()?;
            read += n;
            if n == 0 {
                empty += 1;
                if empty > 10_000 {
                    break;
                }
            }
        }
        Ok(read)
    }

    /// End the run on its own, once: the cause and its words logged and held, the
    /// recording in progress finalized with what reached the disk, the station torn
    /// down.
    fn end_with(&mut self, cause: EndCause, msg: String) {
        if self.run_fault.is_some() {
            return;
        }
        self.log
            .push(self.session_seconds(), format!("FAULT: {msg}"));
        self.run_fault = Some(RunFault {
            cause,
            message: msg,
            station_frame: self.session_frames,
        });
        if self.recording {
            let _ = self.finish_recording();
        }
        self.tear_backend_down();
        self.transition(RecState::Faulted);
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

        // 2. The trigger and the recording, frame-exact: a level fire mid-batch
        //    begins a recording at its frame; a timed recording takes exactly the
        //    frames it still needs, and the rest of the batch goes on (back to the
        //    trigger when it re-arms).
        let w = self.n_input;
        let mut at = 0usize;
        while at < n {
            match self.state {
                RecState::Armed => {
                    let start_frame = self.session_frames + at as u64;
                    let (fire, used) = match self.trigger.as_mut() {
                        Some(trig) => trig.feed_until_fire(&interleaved[at * w..], n - at),
                        None => (None, n - at),
                    };
                    at += used;
                    if let Some(ev) = fire {
                        self.trigger_fires += 1;
                        self.last_fire_station_frame = Some(start_frame + used as u64);
                        self.begin_recording(Some(ev), "level")?;
                    }
                }
                RecState::Recording if self.recording => {
                    let left = (n - at) as u64;
                    let take = match self.record_target_frames {
                        Some(target) => left.min(target.saturating_sub(self.record_frames)),
                        None => left,
                    } as usize;
                    self.hand_to_record_thread(&interleaved[at * w..(at + take) * w], take);
                    at += take;
                    if let Some(t) = self.trigger.as_mut() {
                        t.skip(take as u64);
                    }
                    if self
                        .record_target_frames
                        .is_some_and(|target| self.record_frames >= target)
                    {
                        self.complete_recording()?;
                    }
                }
                _ => break,
            }
        }

        // 4. Analysis path (ALWAYS; drop-tolerant), timed.
        let pass = std::time::Instant::now();
        self.feed_analysis(interleaved, n);
        let took = pass.elapsed();
        self.pass_last = Some(took);
        self.pass_worst = Some(self.pass_worst.map_or(took, |w| w.max(took)));

        self.session_frames += n_frames;
        Ok(())
    }

    /// Hand frames of the recording in progress to the record thread, never waiting.
    /// Frames the queue cannot take are a gap, and the run ends at the pass's end
    /// (`behind`): a disk that far behind cannot be caught up from.
    fn hand_to_record_thread(&mut self, frames: &[f32], n: usize) {
        if self.behind.is_some() {
            return;
        }
        let Some(rec) = self.rec.as_ref() else {
            return;
        };
        self.record_frames += n as u64;
        if rec.push(frames, n) {
            self.record_queued += n as u64;
            return;
        }
        let gap = GapRecord {
            at_frame: self.record_queued,
            dropped_frames: n as u64,
        };
        rec.gap(gap.clone());
        self.record_gaps.push(gap);
        self.record_gap_free = false;
        let behind_s = (self.record_frames - n as u64).saturating_sub(rec.frames_on_disk()) as f64
            / self.fs_hz;
        self.behind = Some(format!(
            "the disk fell {behind_s:.1} s behind the inputs and the record thread's \
             {:.1} s queue could not take a batch; {} ends at file frame {}",
            Self::RECORD_QUEUE_S,
            self.active_run_name.clone().unwrap_or_default(),
            self.record_queued
        ));
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
    fn begin_recording(
        &mut self,
        pretrigger: Option<TriggerEvent>,
        kind: &str,
    ) -> Result<(), RecorderError> {
        self.run_counter += 1;
        let run_name = format!("rec_{:04}", self.run_counter);
        let pre_frames = pretrigger.as_ref().map_or(0, |e| e.pretrigger_frames);
        // Handed to the record thread: the folder, the sidecar, the free-space check
        // and the writer are made there, never on the thread that reads the station.
        let rec = self
            .rec
            .as_ref()
            .ok_or_else(|| RecorderError::BadState("the station is not up".into()))?;
        let pre_frames_asked = self
            .trigger
            .as_ref()
            .map_or(0, |t| t.ring_capacity_frames() as u64);
        rec.start(RecordingSpec {
            data_root: self.data_root.clone(),
            name: run_name.clone(),
            table: self.record_table.clone(),
            acq: self.acq_now(),
            requested_fs_hz: self.cfg.acq.native_rate_hz,
            floor_bytes: self.floor_bytes(),
            pretrigger,
            station: crate::recording::StationFacts {
                name: run_name.clone(),
                trigger: kind.to_string(),
                group_delay_frames: self.group_delay_frames,
                first_station_frame: self.session_frames,
                pre_frames_asked,
            },
        });
        let real_name = run_name;
        self.recording = true;
        self.record_frames = pre_frames;
        self.record_queued = pre_frames;
        self.record_gaps.clear();
        self.record_gap_free = true;
        self.record_pretrigger_frames = pre_frames;
        self.active_run_name = Some(real_name.clone());
        // Exactly the length after the trigger, plus the pre-window.
        self.record_target_frames = self.timed_record_frames().map(|t| t + pre_frames);
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
            // Re-armed: the holdoff runs in station time from the trigger, and a
            // fresh full pre-window is acquired before the next fire, so no frame is
            // in two recordings.
            if let Some(trig) = self.trigger.as_mut() {
                trig.rearm(self.rearms_remaining);
                trig.restart_pre_window();
                self.transition(RecState::Armed);
            } else {
                self.transition(RecState::Monitoring);
            }
        } else {
            // Back to monitoring, not armed: the next recording begins with no
            // pre-window unless the run is armed again.
            self.trigger = None;
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
        self.trigger = None;
        self.transition(RecState::Monitoring);
        Ok(())
    }

    /// Finalize the active recording writer, record the session summary + event.
    fn finish_recording(&mut self) -> Result<(), RecorderError> {
        if !self.recording {
            return Ok(());
        }
        self.recording = false;
        // Finished by the record thread; its summary comes back on a later pass.
        if let Some(rec) = self.rec.as_ref() {
            rec.finish();
        }
        self.finishing.push_back((
            self.record_pretrigger_frames,
            std::mem::take(&mut self.record_gaps),
        ));
        self.record_queued = 0;
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
        // The station is asked the rate configured, and the rate it programs is
        // adopted: a station that coerces the request (a delta-sigma device's ladder)
        // records at its own rate, and the run says so rather than labelling the file
        // with the rate it asked for.
        let acq =
            hertzforge_hal::AcquisitionConfig::new(self.cfg.acq.native_rate_hz, self.block_frames);
        let actual = self
            .backend
            .configure(&self.engine_table, &acq)
            .map_err(|e| RecorderError::Backend(format!("configure: {e}")))?;
        self.fs_hz = actual.actual_rate_hz;
        self.group_delay_frames = self
            .group_delay_given
            .unwrap_or_else(|| actual.group_delay_frames.map_or(0.0, f64::from));

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

        // THE ARM GATE, asked for a capture-only run (`{None, None}`): no drive
        // path, nothing to correct, so the abort-latency rule does not bind. It is
        // given the programmed rate as the run's rate, so a station that programs a
        // rate that does not exist (zero, negative, not finite) is refused by the
        // gate's rate policy before anything is derived from it.
        //
        // The factory supplies the class; it is held on the service from
        // construction so the gate reads the SAME value the caller stated.
        hertzforge_module_abi::arm_gate(
            &self.backend.caps(),
            self.station,
            hertzforge_module_abi::Resolution::capture_only(),
            Some(actual.actual_rate_hz),
            actual.actual_rate_hz,
        )
        .map_err(|reason| RecorderError::Backend(format!("arm refused: {reason}")))?;

        // Everything per frame is built at the programmed rate, now that the gate has
        // said it is one.
        let (analysis, frf) = build_analysis(&self.acq_now(), self.n_input, self.cfg.frf_pair);
        self.analysis = analysis;
        self.frf = frf;

        self.backend
            .arm()
            .map_err(|e| RecorderError::Backend(format!("arm: {e}")))?;
        // The record thread for the run, behind a queue of RECORD_QUEUE_S of frames.
        if self.rec.is_none() {
            let store = self
                .store
                .take()
                .unwrap_or_else(|| Box::new(WriterStore::default()));
            let capacity = (Self::RECORD_QUEUE_S * self.fs_hz).round().max(1.0) as u64;
            self.rec = Some(
                RecordThread::spawn(store, capacity)
                    .map_err(|e| RecorderError::Io(format!("record thread: {e}")))?,
            );
        }
        // The run's one engine: built once, never re-armed or re-started per batch.
        self.engine = Some(Engine::new(
            clone_ecfg(&self.ecfg),
            &self.engine_table,
            &self.backend,
        ));
        self.backend
            .start()
            .map_err(|e| RecorderError::Backend(format!("start: {e}")))?;
        self.backend_up = true;
        Ok(())
    }

    fn tear_backend_down(&mut self) {
        self.engine = None;
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
                &self.acq_now(),
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
            requested_fs_hz: self.cfg.acq.native_rate_hz,
            fs_hz: self.fs_hz,
            station_frames: self.session_frames,
            batches: self.batches,
            pass_ms_last: self.pass_last.map(ms),
            pass_ms_worst: self.pass_worst.map(ms),
            batch_gap_ms_max: self.batch_gap_max.map(ms),
            trigger_fires: self.trigger_fires,
            last_fire_station_frame: self.last_fire_station_frame,
            pre_frames_held: self
                .trigger
                .as_ref()
                .map_or(0, |t| t.pre_frames_held() as u64),
            pre_frames_asked: self
                .trigger
                .as_ref()
                .map_or(0, |t| t.ring_capacity_frames() as u64),
            recordings_saved: self.recordings_saved,
            frames_fed: if self.recording {
                self.record_frames
            } else {
                0
            },
            frames_on_disk: if self.recording {
                self.rec.as_ref().map_or(0, |r| r.frames_on_disk())
            } else {
                0
            },
            write_fault: self.rec.as_ref().and_then(|r| r.fault()),
            run_fault: self.run_fault.clone(),
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
        let h1_phase_deg = frf
            .pipeline
            .h1()
            .iter()
            .map(|&(re, im)| im.atan2(re).to_degrees())
            .collect();
        Some(FrfSnapshot {
            h1_mag,
            h1_phase_deg,
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

/// Build the engine table clone: the user table with (a) its drive rows left out,
/// so the station is configured with the input rows only, and (b) all abort limits
/// stripped when `watchdog_enabled == false`.
fn build_engine_table(user: &ChannelTable, watchdog_enabled: bool) -> ChannelTable {
    let mut clone = user.clone();
    clone.channels.retain(|c| c.role != ChannelRole::Drive);
    if !watchdog_enabled {
        for ch in clone.channels.iter_mut() {
            ch.abort = AbortLimits::default();
        }
    }
    clone
}

/// The analysis state for `n_input` rows at `acq`'s rate, and the FRF pair's when one
/// is configured and in range.
fn build_analysis(
    acq: &crate::acq::AcquisitionConfig,
    n_input: usize,
    pair: Option<(usize, usize)>,
) -> (Vec<ChannelAnalysisState>, Option<FrfState>) {
    let analysis = (0..n_input)
        .map(|_| ChannelAnalysisState {
            decim: ChannelDecimator::new(1, acq),
            asm: BlockAssembler::from_config(1, acq),
            spectrum: SpectrumPipeline::new(acq),
            stats: TimeStatsAccumulator::new(),
            last_stats: empty_stats(),
        })
        .collect();
    let frf = pair
        .filter(|&(x, y)| x < n_input && y < n_input && x != y)
        .map(|pair| FrfState {
            pair,
            decim: ChannelDecimator::new(2, acq),
            asm: BlockAssembler::from_config(2, acq),
            pipeline: FrfPipeline::new(acq),
        });
    (analysis, frf)
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

/// A duration in ms to the microsecond: the status carries it as a short decimal, so
/// it reads back from JSON exactly.
fn ms(d: std::time::Duration) -> f64 {
    d.as_micros() as f64 / 1e3
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
