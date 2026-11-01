//! `SineService` - the Path-B closed-loop sine run orchestration + the level
//! ladder state machine.
//!
//! The service owns the [`SineTestConfig`], the channel table, the station backend,
//! the persistent [`engine::SineProcessStep`], the [`report::EventLog`], and the
//! capture sink. It runs the state machine (`Idle -> Feasibility -> SysId ->
//! CoherenceGate -> Engaged -> FullLevel -> Stopped`, plus `Aborted` from any
//! running state, `CoherenceHold` and `FeasibilityBlocked` parks) and assembles
//! the serde-serializable [`report::ReportData`] + the live [`report::SineStatus`].
//!
//! # The loop shape, from the service's side
//!
//! At `arm`, the service builds the persistent [`engine::SineProcessStep`] and
//! keeps its shared handles. To run, it calls `Engine::run(..., ticks_per_chunk)`
//! in chunks, RE-ARMING the backend per chunk (an HERTZFORGE-SIM plant's state persists),
//! then between chunks: reads the status snapshot, checks the abort latch (ending
//! the run with the trip), checks the stop flag, checks whether the scheduler /
//! ladder completed (advancing Engaged -> FullLevel -> Stopped), and services the
//! capture clocks. Operator commands go through the step's lock-free mailbox, so
//! STOP is honored within one chunk. The engine runs at `frames_per_tick = 1`
//! (the retired Sim per-block drive latch, mirrored from hertzforge-demo).

pub mod config;
pub mod engine;
pub mod report;
pub mod ring;

use crate::control::{BlendConfig, BlendLaw, DriveGuard, FeedforwardTable, NotchGovernor};
use crate::feasibility::assess_sine;
use crate::profile::SineProfile;
use crate::scheduler::{BidirSweep, DwellRun, LadderPlan, RstdPlan, StepSine};
use config::{
    assess_tracking_lag as hertzforge_sine_assess_tracking_lag, CaptureRule, ConfigError, SineMode,
    SineTestConfig, TrackingLagVerdict,
};
use engine::{
    ColaState, Command, ControlSnapshot, LimitChannelSpec, SchedulerBuild, SineProcessStep,
    StepBuild, StepHandles,
};
pub use engine::{RunMode, RAMP_DB_PER_S};
use hertzforge_capture::{
    default_data_root, save_snapshot, CaptureConfig, CaptureSink, Mark, Snapshot,
};
use hertzforge_channel_table::{ChannelRole, ChannelTable, Units};
use hertzforge_dsp::sine::AmplitudeServo;
use hertzforge_hal::Backend;
use hertzforge_shaker_db::{FeasibilityReport, RatingValue, Verdict};
use report::{
    ChannelRow, ChannelTrace, CompositeModeRecord, EventKind, EventLog, FeasibilityDigest,
    FrfDisplay, Honesty, LevelHistoryEntry, RunSettings, RunState, SineStatus, TracePoint,
    TripRecord,
};
use std::path::PathBuf;
use std::sync::atomic::Ordering;

/// Safety-factor headroom on the coarse raw-ring accel watchdog ceiling,
/// applied on top of the profile's worst-case abort-high (itself already the
/// abort band above target) scaled to the highest ladder rung. 4x (~+12 dB)
/// clears a bounded RAW transient - a resonance ring-up during the RSTD low-level
/// search probe, a controlled overshoot at a rung entry - without a false raw-ring
/// trip, while staying far below a genuine runaway (which sits near Q x target,
/// i.e. 20-50x the target for the Q=20-50 fixtures). The fine per-frequency,
/// per-rung abort remains the demodulated scalar-abort's job (engine.rs); this
/// constant only sizes the fast coarse backstop.
///
/// **This is the HERTZFORGE-SIM value.** See [`watchdog_accel_headroom`]
/// for why a hardware station gets a tighter one, and note what the number DOES:
/// it MULTIPLIES the ceiling that gets armed, so raising it loosens the backstop.
const WATCHDOG_ACCEL_HEADROOM: f64 = 4.0;

/// The coarse raw-ring headroom on a HARDWARE station.
///
/// 2x (~+6 dB) rather than the Sim's 4x (~+12 dB). The Sim value's own
/// justification - clearing a bounded RSTD-probe ring-up or a rung-entry
/// overshoot - is just as true on hardware, so this cannot go to 1.0; what
/// changes is the COST of being wrong in each direction. On the Sim plant a
/// false raw-ring trip costs a re-run, so headroom is nearly free. On hardware a
/// LATE trip costs an armature, and the same headroom is a 4x-over-abort signal
/// the fast backstop deliberately ignores. Halving it keeps the transient
/// allowance (a bounded overshoot is rarely more than a few dB) while still
/// sitting an order of magnitude below the Q x target runaway the constant's
/// own note cites at 20-50x.
///
/// `[H]` - a judgment about the right hardware margin, not a measurement. The
/// arrival-day loopback + first real sweep are what turn it into `[C]`; until
/// then the fine per-frequency demodulated scalar abort (engine.rs) remains
/// the primary limit and this is only the fast coarse backstop.
const WATCHDOG_ACCEL_HEADROOM_HARDWARE: f64 = 2.0;

/// The HERTZFORGE-SIM abort-watchdog debounce, in RAW FRAMES (~0.24 ms at 65 536 Hz).
/// Unchanged from the value this module has always used.
const SIM_WATCHDOG_SUSTAIN_FRAMES: u32 = 16;

/// The abort-watchdog debounce on a HARDWARE station, in SECONDS - the same
/// quarter-millisecond, in the unit that survives a rate change. See
/// `hertzforge_engine::watchdog_sustain_frames` for why the frame count could not.
const WATCHDOG_SUSTAIN_SECONDS: f64 = 0.00025;

/// The coarse raw-ring headroom for this station.
///
/// Stated as a function, with the station in hand, for the same reason
/// `default_jitter_frac` and `default_compute_budget` are: the Sim-era value is a
/// harness tuning choice, and carrying it onto hardware unexamined is the class
/// of mistake this project keeps paying for. Unlike those two, this one
/// MULTIPLIES an armed safety ceiling, so getting it wrong does not produce a
/// spurious fault - it produces a backstop that does not fire.
///
/// `pub` so a test can assert the RATIO between the two stations rather than
/// re-typing either literal (the existing HERTZFORGE-SIM ceiling test hard-codes 4.0,
/// which is exactly how a constant and its test drift together).
pub fn watchdog_accel_headroom(station: hertzforge_module_abi::StationClass) -> f64 {
    if station.is_hardware() {
        WATCHDOG_ACCEL_HEADROOM_HARDWARE
    } else {
        WATCHDOG_ACCEL_HEADROOM
    }
}

/// The engine chunk length in test SECONDS: how much test time one `run_chunk`
/// call covers. The caller's contract, unchanged across the batch conversion.
const CHUNK_SECONDS: f64 = 0.25;
/// The minimum chunk length in frames (a floor for very low sample rates).
const MIN_CHUNK_FRAMES: f64 = 256.0;

/// Engine TICKS per bounded `run_ticks` batch.
///
/// One tick is one acquisition BLOCK, not one frame - the unit conflation that
/// made the old control-tick bound vacuous on hardware. Two blocks is ~40 ms at a
/// 1024-frame block and 51.2 kHz, which is the worst-case latency from an
/// operator intent (or a Stop) to the drive acting on it: `run_ticks` has no
/// cooperative stop, so the batch length IS that latency. One tick would halve
/// it and double the per-batch overhead the between-batch work has to fit
/// inside; two is Explorer's proven value.
const TICKS_PER_BATCH: u64 = 2;

/// How much acquisition, IN TIME, the raw display/time-stream ring holds before
/// it drops the oldest block.
///
/// Stated in seconds, not blocks (a display drop-cap denominated
/// in "segments" once discarded 84% of frames when a segment shrank). A quarter
/// second is runway for the drain; a consumer further behind than that is not
/// going to catch up by being given more memory, and `blocks_dropped()` says so.
const RING_SECONDS: f64 = 0.25;

/// How long the two-phase stop waits for the device to play out what it was
/// already given, before stopping the tasks anyway.
///
/// A device that has not drained in two seconds is a device that has stopped
/// generating; waiting longer does not fix it, and the run has to end. Stated in
/// TIME because that is what the wait IS - the queue depth it covers changes with
/// the block and the rate.
const DRAIN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
/// Poll interval for the drain. Short enough that the wait is not itself a
/// noticeable part of a stop, long enough not to spin.
const DRAIN_POLL: std::time::Duration = std::time::Duration::from_millis(1);

/// A panic payload's message, for the pump's `catch_unwind` arm.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// The resonance sharpness the sysID FRF resolution targets. The config does
/// not (yet) carry an expected Q, so the sysID sizes its FFT to resolve a peak at
/// least this sharp (`df <= (f_lo / Q) / 4`), which covers the Q<=50 fixtures.
/// Making this configurable is a clean future extension; today it is the default.
const D8_SYSID_Q_TARGET: f64 = 50.0;

/// Errors from a service operation.
#[derive(Debug, Clone, PartialEq)]
pub enum ServiceError {
    /// The config was invalid.
    Config(ConfigError),
    /// The operation is not allowed in the current state.
    BadState(String),
    /// The plant/backend refused an operation.
    Plant(String),
    /// A capture/snapshot IO error.
    Io(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::Config(e) => write!(f, "{e}"),
            ServiceError::BadState(s) => write!(f, "bad state: {s}"),
            ServiceError::Plant(s) => write!(f, "plant error: {s}"),
            ServiceError::Io(s) => write!(f, "io error: {s}"),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<ConfigError> for ServiceError {
    fn from(e: ConfigError) -> Self {
        ServiceError::Config(e)
    }
}

/// The mode payload for a periodic-save snapshot.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SineSnapshotPayload {
    pub freq_hz: f64,
    pub level_db: f64,
    pub target_g: f64,
    pub measured_g: f64,
    pub per_channel_tracked: Vec<f64>,
    pub per_channel_broadband: Vec<f64>,
    pub thd: f64,
    pub band_coherence: f64,
    pub notch_binding: bool,
}

/// The host-side Sine run service (Path B). Owns the config + the Sim backend +
/// the persistent process step + capture + the event log.
/// One CONTROL channel's swept curves, resampled onto the sweep raster.
///
/// `channel_index` is the REAL channel-table index, not a slot: the display ids
/// for per-channel traces are the table indices themselves (the Random precedent),
/// so a packer that carried a slot would label the wrong channel the moment an
/// operator disabled a row.
#[derive(Debug, Clone, PartialEq)]
pub struct SineDisplayChannel {
    /// The channel-table index this rides as its trace id.
    pub channel_index: u32,
    /// Tracked amplitude at the excitation frequency, g pk, on the raster.
    pub tracked: Vec<f32>,
    /// Broadband RMS, g, on the raster.
    pub broadband: Vec<f32>,
}

/// Everything a sine display packer needs, on ONE grid, and nothing else.
///
/// See [`SineService::display_snapshot`] for why this exists beside
/// [`report::ReportData`] and why the vectors arrive already rastered.
#[derive(Debug, Clone, PartialEq)]
pub struct SineDisplaySnapshot {
    /// First bin frequency, Hz - the run band's LOW edge.
    pub f0_hz: f64,
    /// Bin spacing, Hz.
    pub df_hz: f64,
    /// Completed one-direction sweep passes; the legacy frame header's `seq`.
    pub sweeps_done: u32,
    /// The control profile's target, g pk.
    pub profile: Vec<f32>,
    /// Alarm band upper edge, level-scaled.
    pub alarm_hi: Vec<f32>,
    /// Alarm band lower edge, level-scaled.
    pub alarm_lo: Vec<f32>,
    /// Abort band upper edge, level-scaled.
    pub abort_hi: Vec<f32>,
    /// Abort band lower edge, level-scaled.
    pub abort_lo: Vec<f32>,
    /// The sysID `|H|` drive reference. EMPTY until a sysID has run - a packer
    /// publishes nothing rather than a flat zero curve, because a zero curve on the
    /// drive id reads as "the drive is dead" and not as "there is no estimate yet".
    pub drive: Vec<f32>,
    /// Per control channel, on their real table indices.
    pub channels: Vec<SineDisplayChannel>,
    /// The mean of the per-channel tracked curves - a DISPLAY PROXY for the servo's
    /// weighted composite, which is computed per tick against live amplitudes and
    /// is not a resampled envelope.
    pub composite: Vec<f32>,
    /// Total harmonic distortion as a FRACTION; `None` when not measured.
    pub thd: Option<f64>,
    /// The COLA reference amplitude, volts pk; `None` when COLA is off.
    pub cola: Option<f64>,
}

pub struct SineService {
    cfg: SineTestConfig,
    table: ChannelTable,
    /// The rate the loop runs at, read ONCE from `backend.caps()` at construction.
    ///
    /// This used to be `sim_cfg.sample_rate_hz`, i.e. what the service had
    /// ASKED the Sim for. Caps is the backend's own answer, which on the Sim is
    /// the same number and on hardware is the one that is true.
    sample_rate_hz: f64,
    /// PRNG seed for the sysID noise burst (`run_sysid`). Was `sim_cfg.seed`,
    /// which tied a CONTROL-side determinism knob to the plant's noise seed for
    /// no reason other than both living in the same struct.
    sysid_seed: u64,
    state: RunState,
    log: EventLog,

    /// The band the run sweeps / searches over.
    band: (f64, f64),
    /// Input-frame offsets of the control channels, in control-set order.
    control_slots: Vec<usize>,
    /// Resolved limit-channel specs (input slot + profile).
    limit_specs: Vec<(usize, u32, crate::profile::SineProfile)>,

    // --- sysID artifacts ---------------------------------------------------------
    frf: Option<FrfDisplay>,
    band_coherence: f64,
    feedforward: Option<FeedforwardTable>,

    // --- the persistent step + its handles --------------------------------------
    step: Option<SineProcessStep>,
    handles: Option<StepHandles>,
    /// The station backend (persists across chunks). Held from construction; the
    /// arm path opens/configures/arms/starts it in place. `None` only after an arm
    /// that failed partway - there is no re-arm path (the UI rebuilds the service),
    /// and a second attempt says so rather than silently building a second plant.
    backend: Option<Box<dyn Backend>>,
    /// The persistent engine. Built ONCE per run and re-run per chunk so its
    /// `OutputDelivery` (the drive lookahead) PERSISTS across chunk boundaries.
    /// Previously the engine was rebuilt every chunk, which re-primed
    /// `OutputDelivery` with `min_lookahead` zero frames and discarded the same
    /// number of computed frames at each chunk end - a ~4-frame zero-drive notch +
    /// amplitude discontinuity at every 0.25 s boundary. `Engine::new` only borrows
    /// the table + backend transiently (the returned Engine owns its state), so it
    /// persists cleanly without touching hertzforge-engine.
    engine: Option<hertzforge_engine::Engine>,

    // --- run bookkeeping --------------------------------------------------------
    level_history: Vec<LevelHistoryEntry>,
    last_snapshot: ControlSnapshot,
    tick_count: u64,

    // --- capture ----------------------------------------------------------------
    capture: Option<CaptureSink>,
    data_root: PathBuf,

    // --- feasibility + trip -----------------------------------------------------
    feasibility: Option<FeasibilityDigest>,
    trip: Option<TripRecord>,
    /// WHY the run ended, as [`SineService::finish`] recorded it - the same
    /// sentence that goes into the event log and the daemon journal.
    ///
    /// It exists because [`SineService::trip`] does NOT cover every abort. A
    /// control-tick protection latches a [`TripRecord`]; the SIX engine-level
    /// terminal arms in `run_one_batch` (scheduling fault, input stall, device
    /// link lost, drive underrun, ring overrun, backend/capture fatal) and the
    /// pump-panic arm carry a REASON STRING and no record at all - and those are
    /// the classes a chassis produces and a Sim does not. A host that could only
    /// read `trip()` would report the most hardware-specific aborts as
    /// unexplained, which is exactly how an early chassis abort was first reported
    /// with nothing but the word `aborted` attached to it.
    end_reason: Option<String>,
    override_ack: Option<String>,

    // --- station-derived engine knobs -------------------------------------------
    /// Which station class this service arms against. Today the Sim service is
    /// always `HertzforgeSim`; the backend factory supplies the real class, and the
    /// two knobs below key off it.
    station: hertzforge_module_abi::StationClass,
    /// Frames per acquisition block to REQUEST at configure. The engine's
    /// `frames_per_tick` comes from what the backend ANSWERS, never from here.
    block_frames: usize,
    /// What the backend actually configured, captured at arm. The engine's block
    /// cadence, ring depth and health budget are all derived from this.
    configured: Option<hertzforge_hal::ConfiguredActual>,
    /// Who advances the level ladder. Set BEFORE the step exists (a MANUAL run is
    /// chosen at Start; the step is not built until the coherence gate releases),
    /// carried into [`StepBuild::run_mode`], and also postable live.
    run_mode: RunMode,
    /// Allowed period-jitter fraction handed to the engine's scheduling health.
    /// See [`SineService::default_jitter_frac`] for why the two stations differ.
    jitter_frac: f64,
    /// Per-tick compute budget handed to the engine's scheduling health. See
    /// [`SineService::default_compute_budget`].
    compute_budget: Option<std::time::Duration>,
    /// Reusable holder for the ring drain, so the between-batch tap does not
    /// allocate a fresh `Vec<Vec<f32>>` on every pass.
    ring_drain: Vec<Vec<f32>>,
    /// Run test-time of the last periodic snapshot; `None` = none taken yet.
    /// The CLOCK the `periodic_save_s` interval is measured on.
    last_periodic_save_s: Option<f64>,
    /// Events already surfaced into the log, so `surface_events` is INCREMENTAL.
    /// See that method for what the linear scans were costing.
    surfaced_rstd_search: bool,
    surfaced_rstd_setpoint: bool,
    /// Set by `park_after_sysid`: the AO/AI tasks are STOPPED and the engage path
    /// must bring them back up.
    device_parked: bool,
    /// Read once per block inside the sysID burst; `true` cancels it. Shared so a
    /// Stop can be raised from another thread WITHOUT the service mutex.
    sysid_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The drive->response pipeline lag the sysID measured, in frames.
    sysid_lag_frames: usize,
}

impl SineService {
    /// Build a service for `cfg` against `table` on `backend`, validating the
    /// config + the channel table + the control set + the COLA width invariant
    /// up front. `data_root` is the LOCAL capture root (resolved via
    /// [`default_data_root`] if `None`).
    ///
    /// ## `SimConfig` left this signature (an API break)
    ///
    /// The old shape took a `SimConfig` and built its own `SimBackend` at arm,
    /// which made a hardware Sine run impossible from outside this crate. It now
    /// takes a finished `Box<dyn Backend>` and the `StationClass` it is bound to.
    /// Two values that used to be read off `SimConfig` moved:
    ///
    /// * `sample_rate_hz` comes from `backend.caps()` - the backend's own answer,
    ///   not the request the caller made of it.
    /// * `max_output_channels` (the COLA-width gate below) likewise. This is the
    ///   API break the module x backend matrix flagged: the gate now refuses on
    ///   what the STATION can actually drive, so an NI set with one AO module
    ///   greys out COLA for the same reason a one-output Sim did.
    ///
    /// `sysid_seed` was `SimConfig::seed`; it is a control-side determinism knob
    /// that had no business riding on the plant's noise seed.
    pub fn new(
        cfg: SineTestConfig,
        table: ChannelTable,
        backend: Box<dyn Backend>,
        station: hertzforge_module_abi::StationClass,
        sysid_seed: u64,
        data_root: Option<PathBuf>,
    ) -> Result<Self, ServiceError> {
        let caps = backend.caps();
        cfg.validate()?;
        table
            .validate()
            .map_err(|e| ServiceError::Config(ConfigError::BadTable(e.to_string())))?;

        // Resolve the control-channel slots (offset among enabled input channels).
        let control_slots =
            Self::resolve_slots(&table, &cfg.control.indices()).map_err(ServiceError::Config)?;

        // Resolve the limit-channel specs.
        let mut limit_specs = Vec::new();
        for lp in &cfg.limit_profiles {
            lp.profile
                .validate()
                .map_err(|e| ServiceError::Config(ConfigError::BadProfile(e.to_string())))?;
            let slot =
                Self::resolve_slots(&table, &[lp.channel_index]).map_err(ServiceError::Config)?[0];
            limit_specs.push((slot, lp.channel_index, lp.profile.clone()));
        }

        // The COLA width invariant: the table's Drive-role count must
        // equal the out-channel count the config implies. When COLA is on, the
        // service injects a synthetic labeled "COLA (aux)" Drive row and enforces
        // the count; greying / refusal when the backend cannot supply 2 outputs.
        let mut table = table;
        Self::enforce_cola_width(&mut table, &cfg, caps.max_output_channels)
            .map_err(ServiceError::Config)?;

        let band = cfg.mode.band();

        // The control profile MUST cover the sweep/dwell band. Out of band,
        // `profile.{abort_bounds,alarm_bounds,target_accel_g_pk}(f)` all return
        // None, which silently disarms the amplitude alarm/abort AND collapses the
        // servo target to ~1e-12 (near-zero excitation). Reject at construction
        // time so this can never be a silent runtime hole. Checked at BOTH band
        // edges (the interior is covered by continuity of the profile grammar).
        Self::validate_profile_covers_band(&cfg.profile, band).map_err(ServiceError::Config)?;

        // THE CONTROL-TICK BOUND USED TO LIVE HERE, and it compared the wrong units
        // against a rate the device had not answered yet.
        //
        // The old bound was `control_tick_frames <= (caps.sample_rate_hz * 0.25)`,
        // read as "at least one control tick per chunk". Two things were wrong
        // with it. `chunk_ticks` is handed to `Engine::run` as `max_ticks`, and an
        // engine TICK is one BLOCK, not one frame - so on hardware the bound
        // compared a frame count against a block count and was vacuous by three
        // orders of magnitude. And it read `caps.sample_rate_hz`, the PRE-CONFIGURE
        // rate: a device that coerces (the 9234 rides a 13.1072 MHz/(256n) ladder)
        // is bounded here against a rate it never runs at.
        //
        // The bound now lives at `adopt_configured_rate`, post-adoption, in the
        // units the batch pump actually uses:
        // `control_tick_frames <= TICKS_PER_BATCH * actual.block_frames`.

        // Arm the raw-ring accel watchdog. Translate the control profile's
        // abort-HIGH accel ceiling into the control channels' `AbortLimits.accel_g`
        // so `AbortWatchdog::new` actually monitors the accel axis on the raw
        // input ring (a sub-frame raw peak the demodulated scalar path cannot see
        // at control-tick cadence). We use a CONSERVATIVE CONSTANT ceiling = the
        // MAX abort-high accel over the profile band (a raw peak above the
        // worst-case profile+abort is unambiguously a fault). The FINE
        // per-frequency abort remains the scalar-abort's job (engine.rs); this
        // is the coarse, fast, raw-peak backstop that fires within one raw frame
        // vs. the scalar path's control-tick cadence.
        Self::arm_accel_watchdog(&mut table, &cfg, band, station);

        let data_root = data_root.unwrap_or_else(default_data_root);
        // The factory supplies the class. Everything station-derived below
        // keys off this one value, so there is exactly one thing to change.
        //
        // `cfg.fs_hz` is the operator's REQUEST and it wins here, because this
        // field is what `arm_inner` hands to `AcquisitionConfig::new` - i.e. it is
        // the number the run ASKS the device for. It is not, and must never be
        // read as, what the device answered: `adopt_configured_rate` overwrites
        // this same field with `ConfiguredActual::actual_rate_hz` at the arm, and
        // every Hz-denominated derivation after that point reads the answer.
        // Absent (the pre-2026-08-09 shape, and every stored config) = the
        // station's own rate, unchanged.
        let sample_rate_hz = match cfg.fs_hz {
            Some(fs) if fs.is_finite() && fs > 0.0 => fs,
            _ => caps.sample_rate_hz,
        };

        Ok(SineService {
            cfg,
            table,
            sample_rate_hz,
            sysid_seed,
            state: RunState::Idle,
            log: EventLog::default(),
            band,
            control_slots,
            limit_specs,
            frf: None,
            band_coherence: 0.0,
            feedforward: None,
            step: None,
            handles: None,
            backend: Some(backend),
            engine: None,
            level_history: Vec::new(),
            last_snapshot: ControlSnapshot::default(),
            tick_count: 0,
            capture: None,
            data_root,
            feasibility: None,
            trip: None,
            end_reason: None,
            override_ack: None,
            station,
            run_mode: RunMode::Auto,
            block_frames: Self::default_block_frames(station, sample_rate_hz),
            configured: None,
            jitter_frac: Self::default_jitter_frac(station),
            compute_budget: Self::default_compute_budget(station),
            ring_drain: Vec::new(),
            last_periodic_save_s: None,
            surfaced_rstd_search: false,
            surfaced_rstd_setpoint: false,
            device_parked: false,
            sysid_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            sysid_lag_frames: 0,
        })
    }

    /// The acquisition block this station should REQUEST at configure.
    ///
    /// HERTZFORGE-SIM keeps [`hertzforge_engine::SIM_NATURAL_BLOCK_FRAMES`]: this service
    /// counts a chunk in FRAMES and hands that count to the engine as `max_ticks`,
    /// so the two units must coincide. (It is NOT the retired SimBackend drive
    /// latch, which has been removed; the Sim streams every drive frame at any
    /// block size now.) A HARDWARE station takes the rate-derived block, which is
    /// what the driver actually wants to hand over per read.
    fn default_block_frames(station: hertzforge_module_abi::StationClass, fs_hz: f64) -> usize {
        if station.is_hardware() {
            hertzforge_engine::default_block_frames(fs_hz)
        } else {
            hertzforge_engine::SIM_NATURAL_BLOCK_FRAMES
        }
    }

    /// The scheduling-health period-jitter allowance for this station.
    ///
    /// HERTZFORGE-SIM gets [`hertzforge_engine::TEST_DISABLE_JITTER_GUARD`] EXPLICITLY, by
    /// name: the Sim run loop is a deterministic single-threaded harness that
    /// executes the engine flat out and then STOPS between chunks for the control
    /// DSP, so the wall-clock interval between ticks bears no relation to the
    /// sample cadence and the guard would flag every chunk boundary. That was the
    /// reason for the bare `1e12` literal this replaces; the value is unchanged,
    /// but it is now named, documented, and confined to the station where it is
    /// true.
    ///
    /// A HARDWARE station gets the engine's own block-derived budget. There the
    /// tick cadence IS the sample cadence, and period jitter is precisely the
    /// symptom the guard exists to catch (a thermal throttle or an MMCSS budget
    /// overrun AFTER `arm()` already passed its latency gate). Carrying the
    /// harness value onto hardware would disable that check silently.
    fn default_jitter_frac(station: hertzforge_module_abi::StationClass) -> f64 {
        if station.is_hardware() {
            hertzforge_engine::EngineConfig::new(1, 1).jitter_frac
        } else {
            hertzforge_engine::TEST_DISABLE_JITTER_GUARD
        }
    }

    /// The per-tick COMPUTE budget for this station.
    ///
    /// HERTZFORGE-SIM gets [`hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET`] EXPLICITLY, by
    /// name. The engine's default budget is the TICK PERIOD (~61 us at this
    /// service's rate at one frame per tick), which is right on hardware and
    /// meaningless for a deterministic harness: an OS hiccup inside a `process`
    /// call that genuinely takes microseconds trips `ComputeOverrun` and aborts the
    /// chunk. That spurious fault is what the module retry loops existed for;
    /// relaxing the budget removes the CAUSE, which is what lets the retry rule
    /// above be strict.
    ///
    /// A HARDWARE station gets `None` - the tick period IS the deadline there, and
    /// a compute overrun means the drive really did arrive late.
    fn default_compute_budget(
        station: hertzforge_module_abi::StationClass,
    ) -> Option<std::time::Duration> {
        if station.is_hardware() {
            None
        } else {
            Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET)
        }
    }

    /// Override the acquisition block this service requests. Must be called
    /// before `arm()`.
    pub fn with_block_frames(mut self, frames: usize) -> Self {
        self.block_frames = frames.max(1);
        self
    }

    /// Override the scheduling-health period-jitter allowance. Must be called
    /// before `arm()`. A hardware path should not need this.
    pub fn with_jitter_frac(mut self, frac: f64) -> Self {
        self.jitter_frac = frac;
        self
    }

    /// Which station class this service arms against.
    pub fn station(&self) -> hertzforge_module_abi::StationClass {
        self.station
    }

    /// The input interleave width: enabled non-Drive rows in the configured table.
    ///
    /// This is EXACTLY what a configured backend's channel map presents per frame,
    /// which is what `SimBackend::n_input_channels()` used to be asked for. Reading
    /// it off the table instead removes two problems: it is not a `SimBackend`
    /// inherent method (the service holds a `Box<dyn Backend>` now), and it does not
    /// depend on WHEN the backend was configured - the backend is held from
    /// construction now, and an unconfigured one answers 0.
    ///
    /// Call sites still gate on `self.configured.is_some()` so the pre-arm fallbacks
    /// they had are preserved exactly.
    fn n_input_slots(&self) -> usize {
        self.table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .count()
    }

    /// Offset of each requested channel index among the ENABLED input
    /// (control/monitor/reference) channels - the interleave slot the plant
    /// presents (Drive channels are not in the input frame). Errors if a
    /// requested index is not an enabled input channel.
    fn resolve_slots(table: &ChannelTable, indices: &[u32]) -> Result<Vec<usize>, ConfigError> {
        let mut result = Vec::with_capacity(indices.len());
        for &idx in indices {
            let mut slot = 0usize;
            let mut found = None;
            for ch in table.channels.iter().filter(|c| c.enabled) {
                if ch.role == ChannelRole::Drive {
                    continue; // drives are outputs, not in the input frame
                }
                if ch.index == idx {
                    found = Some(slot);
                    break;
                }
                slot += 1;
            }
            match found {
                Some(s) => result.push(s),
                None => {
                    return Err(ConfigError::BadControlSet(format!(
                        "channel {idx} is not an enabled input (control/monitor/reference) channel"
                    )))
                }
            }
        }
        Ok(result)
    }

    /// Enforce the COLA width invariant. When COLA is enabled the
    /// backend must be able to drive 2 output channels and the table must carry 2
    /// Drive-role rows; the service injects a synthetic "COLA (aux)" Drive row
    /// when the table has exactly 1. When COLA is off, exactly 1 Drive row is
    /// required (a lone implicit drive is fine).
    fn enforce_cola_width(
        table: &mut ChannelTable,
        cfg: &SineTestConfig,
        max_output_channels: u32,
    ) -> Result<(), ConfigError> {
        let want = cfg.out_channels();
        let drive_rows = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Drive)
            .count();
        if cfg.cola.enabled {
            if max_output_channels < 2 {
                return Err(ConfigError::BadCola(format!(
                    "COLA needs 2 output channels but the backend advertises \
                     max_output_channels = {max_output_channels} (grey/refuse COLA)"
                )));
            }
            match drive_rows {
                0 | 1 => {
                    // Inject the synthetic COLA aux Drive row so the table's
                    // Drive count matches out_channels == 2. If there were 0, we
                    // also add a control drive row first.
                    if drive_rows == 0 {
                        let idx = Self::next_index(table);
                        let mut d = hertzforge_channel_table::Channel::new(idx);
                        d.role = ChannelRole::Drive;
                        d.units = Units::VoltageV;
                        d.description = "Drive".into();
                        table.channels.push(d);
                    }
                    let idx = Self::next_index(table);
                    let mut cola = hertzforge_channel_table::Channel::new(idx);
                    cola.role = ChannelRole::Drive;
                    cola.units = Units::VoltageV;
                    cola.description = "COLA (aux)".into();
                    table.channels.push(cola);
                }
                2 => { /* already the right width */ }
                n => {
                    return Err(ConfigError::BadCola(format!(
                        "COLA needs exactly 2 Drive rows (control + COLA aux); table has {n}"
                    )));
                }
            }
        } else {
            match drive_rows {
                0 => {
                    // A HostClosesLoop plant needs a drive; inject one control
                    // drive (a Sim plant would synthesize one, but the table
                    // must declare it so the width invariant holds).
                    let idx = Self::next_index(table);
                    let mut d = hertzforge_channel_table::Channel::new(idx);
                    d.role = ChannelRole::Drive;
                    d.units = Units::VoltageV;
                    d.description = "Drive".into();
                    table.channels.push(d);
                }
                1 => { /* the right width */ }
                n => {
                    return Err(ConfigError::BadCola(format!(
                        "COLA disabled needs exactly 1 Drive row; table has {n} (enable COLA or \
                         remove the extra Drive row)"
                    )));
                }
            }
        }
        // Post-condition: the enabled Drive-row count equals `want`.
        let drive_rows = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Drive)
            .count();
        if drive_rows != want {
            return Err(ConfigError::BadCola(format!(
                "internal: Drive-row count {drive_rows} != out_channels {want} after enforcement"
            )));
        }
        Ok(())
    }

    fn next_index(table: &ChannelTable) -> u32 {
        table.channels.iter().map(|c| c.index).max().unwrap_or(0) + 1
    }

    /// Reject when the control profile does not cover the run's sweep/dwell
    /// band. Out of band, `target_accel_g_pk(f)` is `None`, which would collapse
    /// the servo target and disarm the amplitude protections; a config that sweeps
    /// wider than its profile is unsafe, not merely degraded. Checked at both band
    /// edges (a small epsilon inside the edge so a boundary-equal frequency that
    /// the segment lookup treats as out-of-range is still caught).
    fn validate_profile_covers_band(
        profile: &SineProfile,
        band: (f64, f64),
    ) -> Result<(), ConfigError> {
        let (lo, hi) = band;
        for &edge in &[lo, hi] {
            if profile.target_accel_g_pk(edge).is_none() {
                return Err(ConfigError::BadProfile(format!(
                    "control profile does not cover the run band edge {edge:.4} Hz \
                     (band {lo:.4}-{hi:.4} Hz); the profile must span the full \
                     sweep/dwell band or the amplitude alarm/abort disarm and the \
                     servo target collapses out of band. Fix it on the SCHEDULE tab \
                     (Run Band), or add a profile row that reaches {edge:.4} Hz."
                )));
            }
        }
        Ok(())
    }

    /// The conservative constant accel-abort ceiling for the raw-ring
    /// watchdog: the MAX abort-high accel (g pk) over the profile band, sampled on
    /// a dense log grid across the run band and at the profile row breakpoints so
    /// no interior peak is missed. `None` if the profile is empty out of band.
    fn max_abort_high_accel(profile: &SineProfile, band: (f64, f64)) -> Option<f64> {
        let (lo, hi) = band;
        let (lo, hi) = if hi > lo {
            (lo, hi)
        } else {
            (lo * 0.999, lo * 1.001)
        };
        let mut probes: Vec<f64> = Vec::new();
        // Dense log grid over the band.
        let steps = 256usize;
        let (llo, lhi) = (lo.max(1e-9).ln(), hi.max(1e-9).ln());
        for k in 0..=steps {
            let t = k as f64 / steps as f64;
            probes.push((llo + t * (lhi - llo)).exp());
        }
        // The profile row breakpoints inside the band (abort-high can peak there).
        for row in &profile.rows {
            if row.freq_hz >= lo && row.freq_hz <= hi {
                probes.push(row.freq_hz);
            }
        }
        let mut worst: Option<f64> = None;
        for f in probes {
            if let Some((_, ab_hi)) = profile.abort_bounds(f) {
                worst = Some(worst.map_or(ab_hi, |w: f64| w.max(ab_hi)));
            }
        }
        worst
    }

    /// Write the conservative constant accel-abort ceiling into every control
    /// channel's `AbortLimits.accel_g` (plus any per-channel accel abort the limit
    /// config carries), so `AbortWatchdog::new` monitors the accel axis. Only
    /// SETS a value; never lowers a stricter limit an operator already set.
    ///
    /// The ceiling is deliberately COARSE, not the fine per-frequency abort (that
    /// stays the scalar-abort's job at control-tick cadence): it is the profile's
    /// worst-case abort-high accel over the band, scaled to the MAXIMUM ladder
    /// rung (the watchdog is a constant, so it must clear the highest level the
    /// run reaches), times a [`WATCHDOG_ACCEL_HEADROOM`] safety factor. The
    /// headroom lets a bounded RAW transient - a resonance ring-up during the
    /// RSTD low-level search probe, or a controlled overshoot at a rung entry -
    /// pass without a false raw-ring trip, while a genuine runaway (which sits
    /// near Q x target, i.e. many multiples of the abort band) still trips fast on
    /// the raw ring. This is the fast coarse backstop; the fine per-frequency,
    /// per-rung abort is enforced on the demodulated scalar path (engine.rs).
    /// The coarse raw-ring headroom for this station.
    ///
    /// Stated as a function, with the station in hand, for the same reason
    /// `default_jitter_frac` and `default_compute_budget` are: the Sim-era value
    /// is a harness tuning choice and carrying it onto hardware unexamined is
    /// the class of mistake this project keeps paying for. Unlike those two, this
    /// one MULTIPLIES an armed safety ceiling, so getting it wrong is not a
    /// spurious fault - it is a backstop that does not fire.
    fn arm_accel_watchdog(
        table: &mut ChannelTable,
        cfg: &SineTestConfig,
        band: (f64, f64),
        station: hertzforge_module_abi::StationClass,
    ) {
        // The maximum linear rung scale the ladder can reach (start + every table
        // rung; the watchdog constant must clear the highest level the run hits).
        let max_rung_db = cfg
            .ladder
            .table_db
            .iter()
            .copied()
            .chain(std::iter::once(cfg.ladder.start_db))
            .fold(cfg.ladder.start_db, f64::max);
        let max_rung_scale = 10f64.powf(max_rung_db / 20.0);
        let scale = max_rung_scale * watchdog_accel_headroom(station);

        let control_indices = cfg.control.indices();
        let ctrl_ceiling = Self::max_abort_high_accel(&cfg.profile, band).map(|c| c * scale);
        for ch in table.channels.iter_mut() {
            if ch.role == ChannelRole::Drive {
                continue;
            }
            // Control channels get the (rung-scaled, headroomed) control-profile
            // ceiling.
            if control_indices.contains(&ch.index) {
                if let Some(c) = ctrl_ceiling {
                    ch.abort.accel_g = Some(match ch.abort.accel_g {
                        Some(existing) => existing.min(c),
                        None => c,
                    });
                }
            }
            // Limit channels get their OWN profile's worst-case abort-high, same
            // rung-scale + headroom.
            for lp in &cfg.limit_profiles {
                if lp.channel_index == ch.index {
                    if let Some(c) =
                        Self::max_abort_high_accel(&lp.profile, band).map(|c| c * scale)
                    {
                        ch.abort.accel_g = Some(match ch.abort.accel_g {
                            Some(existing) => existing.min(c),
                            None => c,
                        });
                    }
                }
            }
        }
    }

    // --- introspection ----------------------------------------------------------

    pub fn state(&self) -> RunState {
        self.state
    }
    /// The config this service was built from. Exposed so a caller holding ONLY a
    /// configured service can rebuild an equivalent one without re-asking the UI to
    /// reassemble the config - the operator's Reset escape hatch (`SineState::reset`)
    /// must work even when the UI's assembled config is momentarily un-buildable,
    /// which is exactly the state that stranded the 2026-07-30 real-chassis Random
    /// run. Sine carries the same `CoherenceHold` park, so it strands the same way.
    pub fn config(&self) -> &SineTestConfig {
        &self.cfg
    }
    pub fn log(&self) -> &EventLog {
        &self.log
    }
    pub fn band_coherence(&self) -> f64 {
        self.band_coherence
    }
    pub fn trip(&self) -> Option<&TripRecord> {
        self.trip.as_ref()
    }

    /// WHY the run ended, verbatim from the terminal path that ended it.
    ///
    /// `None` until a run has ended. This is the accessor a HOST reads to put a
    /// reason on the wire beside the terminal state, and it is deliberately the
    /// same string the event log and the journal carry so the two can be grepped
    /// against each other.
    ///
    /// Read [`SineService::trip`] when the structured fields are wanted; read this
    /// when the question is "why did it end", because it is the only answer that
    /// covers the engine-level terminal arms as well as the control-tick trips.
    pub fn end_reason(&self) -> Option<&str> {
        self.end_reason.as_deref()
    }

    /// The sysID FRF display record (`|H|`, coherence, trust mask on the sysID
    /// grid); `None` until `run_sysid` has completed.
    ///
    /// Exposed for the DAEMON's gate publish: the sysID pair rides reserved trace
    /// ids on PASS *and* on FAIL, and a held gate is exactly when an operator most
    /// needs to see the curve that held it. `report_data()` carries the same record
    /// but builds the whole report to get it, which is the wrong cost on a path that
    /// runs at the gate and (for the local packer) once per display window.
    pub fn frf(&self) -> Option<&FrfDisplay> {
        self.frf.as_ref()
    }

    /// The step's shared [`engine::StatusCell`]; `None` until the step is built
    /// (i.e. until the run engages).
    ///
    /// The Explorer precedent, with one difference worth stating: the Explorer's
    /// cell carries its whole `ExplorerStatus` including the run state, so its
    /// daemon host reads state and numbers from one place. Sine's cell is a
    /// CONTROL snapshot - the numbers a control tick produces - and carries no
    /// state, because the state machine lives on the service and the control tick
    /// has no business writing it. A host therefore pairs this cell with its own
    /// lock-free phase mirror; see `hertzforge-backendd`'s `SineHost`.
    pub fn status_cell(&self) -> Option<std::sync::Arc<engine::StatusCell>> {
        self.handles.as_ref().map(|h| h.status.clone())
    }

    /// **THE DISPLAY SNAPSHOT**: every sine overlay, already resampled onto the
    /// band-spanning [`crate::sweep_raster::SweepRaster`], and nothing else.
    ///
    /// # Why this exists beside `report_data`
    ///
    /// A display packer needs the swept traces, the profile and band overlays, the
    /// sysID drive reference and three scalars. [`SineService::report_data`] builds
    /// all of that AND the event log, the level history, the feasibility digest,
    /// the channel-table digest and the honesty section - a whole report - which is
    /// the right cost once at the end of a run and the wrong cost at a 25 Hz
    /// display cadence. This is the same products with none of the rest.
    ///
    /// # Why it returns RASTERED vectors rather than the raw traces
    ///
    /// Two packers publish these overlays on the same reserved trace ids to the
    /// same decoder: the local Tauri host and the daemon's `SineHost`. The same id
    /// must mean the same shape on both paths, so the grid and the resampling are
    /// done HERE, once, rather than by two callers that have to keep agreeing. See
    /// the [`crate::sweep_raster`] module docs.
    ///
    /// # Cost, stated
    ///
    /// One raster (256 points) plus five profile evaluations per bin, plus one
    /// sort-and-interpolate per control channel over a trace bounded to
    /// `~2 * TRACE_CAP` points. It allocates. It is meant for the caller's
    /// BETWEEN-BATCH pass under a display-cadence gate, never from inside a control
    /// tick, and a caller that publishes on every batch instead of on a timer will
    /// feel it.
    pub fn display_snapshot(&self) -> SineDisplaySnapshot {
        let raster = crate::sweep_raster::SweepRaster::over_band(self.band);
        let s = &self.last_snapshot;
        let profile = &self.cfg.profile;

        // The profile target, in g pk, at every bin. `None` outside the authored
        // table is published as ZERO rather than as an invented value - an overlay
        // that guessed past its own table would be drawn with the same weight as
        // the rows the operator typed.
        let target = raster.map(|f| profile.target_accel_g_pk(f).unwrap_or(0.0));

        // The alarm / abort envelopes are the target scaled by the band in dB AND
        // by the current LEVEL: a run at -12 dB is judged against a -12 dB band,
        // and an envelope drawn at full level while the ladder is at its first rung
        // would show the operator a run that looks wildly under target.
        let level_scale = 10f64.powf(s.level_db / 20.0);
        let (alarm_m, alarm_p) = profile.global_alarm_db;
        let (abort_m, abort_p) = profile.global_abort_db;
        let env = |db: f64| -> Vec<f32> {
            raster.map(|f| {
                profile
                    .target_accel_g_pk(f)
                    .map(|t| t * level_scale * 10f64.powf(db / 20.0))
                    .unwrap_or(0.0)
            })
        };

        // The DRIVE reference: the sysID |H| on the raster. A proxy for the
        // commanded drive SHAPE - the real per-tick drive is one number at one
        // frequency and is not a curve - and empty until a sysID has run, which is
        // honest: before the gate there is no plant estimate to draw.
        let drive = match self.frf.as_ref() {
            Some(f) if !f.freqs.is_empty() => raster.resample_curve(&f.freqs, &f.h_mag),
            _ => Vec::new(),
        };

        // Per CONTROL channel, on its REAL table index (the Random precedent), plus
        // the composite. The composite here is the MEAN of the per-channel tracked
        // curves and is a DISPLAY proxy: the servo's own composite is weighted and
        // mode-dependent (min / max / weighted-average) and is computed per tick
        // against the live amplitudes, not against a resampled envelope. It is
        // labelled a proxy in the id census for that reason.
        let mut channels: Vec<SineDisplayChannel> = Vec::new();
        let mut sum = vec![0f32; raster.len()];
        let mut count = vec![0u32; raster.len()];
        if let Some(h) = self.handles.as_ref() {
            let tracked = h.trace.tracked.lock().ok();
            let broadband = h.trace.broadband.lock().ok();
            if let (Some(tracked), Some(broadband)) = (tracked, broadband) {
                for (cc, &slot) in self
                    .cfg
                    .control
                    .channels
                    .iter()
                    .zip(self.control_slots.iter())
                {
                    let to_points = |v: Option<&Vec<(f64, f64)>>| -> Vec<TracePoint> {
                        v.map(|v| {
                            v.iter()
                                .map(|&(f, a)| TracePoint {
                                    freq_hz: f,
                                    value: a,
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                    };
                    let t = raster.resample_trace(&to_points(tracked.get(slot)));
                    let b = raster.resample_trace(&to_points(broadband.get(slot)));
                    for (k, &v) in t.iter().enumerate() {
                        if v > 0.0 {
                            sum[k] += v;
                            count[k] += 1;
                        }
                    }
                    channels.push(SineDisplayChannel {
                        channel_index: cc.index,
                        tracked: t,
                        broadband: b,
                    });
                }
            }
        }
        let composite: Vec<f32> = sum
            .iter()
            .zip(count.iter())
            .map(|(&s, &c)| if c > 0 { s / c as f32 } else { 0.0 })
            .collect();

        SineDisplaySnapshot {
            f0_hz: raster.f0_hz,
            df_hz: raster.df_hz,
            sweeps_done: s.sweeps_done,
            profile: target,
            alarm_hi: env(alarm_p),
            alarm_lo: env(-alarm_m),
            abort_hi: env(abort_p),
            abort_lo: env(-abort_m),
            drive,
            channels,
            composite,
            // THD is a live SCALAR, not a swept log, so it rides the whole raster as
            // a flat marker. Carried as a fraction; a wire that wants percent
            // converts at the one place that knows both units.
            thd: s.thd.is_finite().then_some(s.thd),
            cola: self.cfg.cola.enabled.then_some(self.cfg.cola.volts_pk),
        }
    }

    /// Is test PROGRESS currently held (the operator's Hold, not a stop)?
    ///
    /// A hold is not a run state: the output CONTINUES at the held frequency and
    /// level and only the sweep / dwell / rung clocks stop. The wire has a
    /// dedicated `hold` field for exactly this reason, and this is what fills it.
    /// `false` before the step exists - nothing is progressing, so nothing is held.
    pub fn is_held(&self) -> bool {
        let Some(step) = self.step.as_ref() else {
            return false;
        };
        step.is_held()
    }
    pub fn feasibility(&self) -> Option<&FeasibilityDigest> {
        self.feasibility.as_ref()
    }
    /// The (possibly COLA-augmented) channel table the run uses.
    pub fn table(&self) -> &ChannelTable {
        &self.table
    }

    /// The rate the loop is actually running at.
    ///
    /// Pre-arm this is what the backend's caps advertised; from `arm_inner` onward
    /// it is what `configure()` answered (the rate adoption). Exposed because a
    /// gate row that states a frequency, a block period or a settle time in TIME
    /// has to divide by the SAME number the loop synthesizes against - a caller
    /// that re-derived it from its own request would be asserting against the rate
    /// the device coerced away from.
    pub fn sample_rate_hz(&self) -> f64 {
        self.sample_rate_hz
    }

    /// What the backend answered at `configure()`; `None` before arm.
    pub fn configured(&self) -> Option<hertzforge_hal::ConfiguredActual> {
        self.configured
    }

    /// A LIVE engine-health snapshot (worst tick compute against its budget,
    /// scheduling faults, raw-ring drops); `None` before the first engine batch.
    ///
    /// The chassis gate's compute-margin and zero-fault rows read this. It is the
    /// one pair of numbers no HERTZFORGE-SIM run can produce honestly: the Sim path
    /// arms `TEST_RELAX_COMPUTE_BUDGET`, so the fraction is meaningless there and
    /// only means something on a station whose budget IS the tick period.
    pub fn engine_health(&self) -> Option<hertzforge_engine::EngineHealth> {
        self.engine.as_ref().map(|e| e.health())
    }

    /// The drive scale the step last committed (0.0 when not engaged).
    pub fn drive_scale(&self) -> f64 {
        self.step.as_ref().map(|s| s.drive_scale()).unwrap_or(0.0)
    }

    /// The COLA aux ramp factor (0.0 when not engaged). Exposed beside
    /// [`SineService::drive_scale`] because "the drive is at zero" is a statement
    /// about BOTH outputs at `out_channels = 2`, and a caller given only the first
    /// one cannot check it.
    pub fn cola_scale(&self) -> f64 {
        self.step.as_ref().map(|s| s.cola_scale()).unwrap_or(0.0)
    }

    /// The raw acquisition ring's depth in BLOCKS, sized in TIME.
    ///
    /// A threshold denominated in a producer-defined unit is a latent fault - the
    /// same "16 blocks" is 0.25 s at a 1024-frame block on the chassis and 1 ms at
    /// the Sim's one-frame block. [`RING_SECONDS`] is the statement; the block
    /// count is derived from it and floored so a very large block still gets a
    /// couple of slots.
    fn ring_blocks(&self) -> usize {
        let block = self
            .configured
            .map(|a| a.block_frames)
            .unwrap_or(self.block_frames)
            .max(1);
        let frames = (self.sample_rate_hz * RING_SECONDS).max(1.0);
        ((frames / block as f64).ceil() as usize).clamp(4, 4096)
    }

    /// Blocks the raw ring dropped because the drain fell behind, this run.
    ///
    /// PUBLISHED rather than absorbed: a display that has silently lost blocks
    /// looks exactly like one that has not.
    pub fn blocks_dropped(&self) -> u64 {
        self.handles.as_ref().map(|h| h.ring.dropped()).unwrap_or(0)
    }

    /// Drain the raw acquisition ring, handing every block to `on_block` as
    /// `(interleaved samples, stride)` - the TIME-DOMAIN TAP.
    ///
    /// Call BETWEEN tick batches, never from inside one.
    ///
    /// # Why a callback and not a return value
    ///
    /// Those `Vec<f32>`s are the ring's RECYCLED buffers, handed back to the
    /// producer at the end of this call so the RT tick never reaches the
    /// allocator. A caller that took ownership would starve the pool and put an
    /// allocation on the drive clock. Borrowing them for the length of the call
    /// gives the caller the same data with none of that.
    pub fn service_analysis_tapped<F>(&mut self, mut on_block: F)
    where
        F: FnMut(&[f32], usize),
    {
        let Some(h) = self.handles.as_ref() else {
            return;
        };
        let ring = h.ring.clone();
        let stride = ring.stride();
        let mut drained = std::mem::take(&mut self.ring_drain);
        drained.clear();
        ring.drain_into(&mut drained);
        for block in &drained {
            on_block(block, stride);
        }
        ring.recycle(&mut drained);
        self.ring_drain = drained;
    }

    /// NEGATIVE CONTROL: shift the sysID feedforward table by `db`
    /// before the step is built, so the prediction is knowingly WRONG by that
    /// much and the level row can only pass if the feedback path closes it.
    ///
    /// Valid between `run_sysid` and `coherence_gate` (the gate builds the step
    /// from this table); refuses anywhere else rather than silently doing
    /// nothing. See [`crate::control::FeedforwardTable::offset_db`] for why a
    /// flat unity plant needs this to make its accuracy row falsifiable.
    pub fn inject_feedforward_error_db(&mut self, db: f64) -> Result<(), ServiceError> {
        if !matches!(self.state, RunState::CoherenceGate) {
            return Err(ServiceError::BadState(format!(
                "inject_feedforward_error_db from {:?}; valid only between run_sysid and \
                 coherence_gate (the gate builds the step from the table)",
                self.state
            )));
        }
        match self.feedforward.as_mut() {
            Some(ff) => {
                ff.offset_db(db);
                self.log.push(EventKind::Note {
                    text: format!(
                        "NEGATIVE CONTROL: sysID feedforward offset by {db:+.2} dB. This is a \
                         gate instrument, not a run-time control; the servo must close it."
                    ),
                });
                Ok(())
            }
            None => Err(ServiceError::BadState(
                "no feedforward table (sysID produced no trusted bins)".into(),
            )),
        }
    }

    fn transition(&mut self, to: RunState) {
        let from = self.state;
        if from != to {
            self.state = to;
            self.log.push(EventKind::StateChange { from, to });
        }
    }

    // --- feasibility + arming ----------------------------------------------------

    /// Run the pre-test feasibility assessment. Transitions
    /// `Idle -> Feasibility`; if the shaker force/kinematics are Exceeded, parks
    /// at `FeasibilityBlocked` (arming hard-blocked until `arm_with_override`).
    /// No shaker configured => a permissive NotEvaluated report + a note.
    pub fn assess_feasibility(&mut self) -> Result<FeasibilityReport, ServiceError> {
        if !matches!(self.state, RunState::Idle) {
            return Err(ServiceError::BadState(format!(
                "assess_feasibility from {:?}",
                self.state
            )));
        }
        self.transition(RunState::Feasibility);

        let report = match &self.cfg.shaker {
            None => {
                self.log.push(EventKind::Note {
                    text: "No shaker configured; feasibility NOT evaluated. Arming allowed \
                           without a shaker-limit block."
                        .into(),
                });
                FeasibilityReport {
                    items: Vec::new(),
                    overall: Verdict::NotEvaluated,
                    assumptions: vec!["No shaker record configured.".into()],
                }
            }
            Some(shaker) => assess_sine(shaker, &self.cfg.moving_mass, &self.cfg.profile),
        };

        self.log.push(EventKind::Feasibility {
            overall: report.overall.to_string(),
            exceeded: report.exceeded(),
            has_advisory: report.has_advisory(),
        });
        let shaker_name = self
            .cfg
            .shaker
            .as_ref()
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "(no shaker configured)".into());
        self.feasibility = Some(FeasibilityDigest {
            report: report.clone(),
            shaker_name,
            override_ack: None,
        });

        if report.exceeded() {
            self.transition(RunState::FeasibilityBlocked);
        }
        Ok(report)
    }

    /// Arm the run. Requires feasibility to have passed (not Exceeded). Opens the
    /// Sim backend, builds the persistent step, and moves to `SysId`.
    pub fn arm(&mut self) -> Result<(), ServiceError> {
        match self.state {
            RunState::Feasibility => self.arm_inner(),
            RunState::FeasibilityBlocked => Err(ServiceError::BadState(
                "arming HARD-BLOCKED: feasibility Exceeded; use arm_with_override(ack)".into(),
            )),
            _ => Err(ServiceError::BadState(format!("arm from {:?}", self.state))),
        }
    }

    /// Arm past a feasibility exceedance with an explicit operator acknowledgment.
    pub fn arm_with_override(&mut self, ack: &str) -> Result<(), ServiceError> {
        if !matches!(self.state, RunState::FeasibilityBlocked) {
            return Err(ServiceError::BadState(format!(
                "arm_with_override from {:?} (only valid when FeasibilityBlocked)",
                self.state
            )));
        }
        if ack.trim().is_empty() {
            return Err(ServiceError::BadState(
                "override acknowledgment must not be empty".into(),
            ));
        }
        self.override_ack = Some(ack.to_string());
        if let Some(d) = self.feasibility.as_mut() {
            d.override_ack = Some(ack.to_string());
        }
        self.log.push(EventKind::OverrideAck {
            ack: ack.to_string(),
        });
        self.arm_inner()
    }

    /// Open the station backend up to Running and enter SysId. (The step itself
    /// is built at `coherence_gate` engage, so its start frequency + level + the
    /// sysID feedforward are seeded from the sysID pass.)
    fn arm_inner(&mut self) -> Result<(), ServiceError> {
        // THE sysID CANCELLATION FLAG IS CLEARED HERE, AT THE ARM, and nowhere else.
        //
        // It used to be cleared by `run_sysid` as its first act, which reads as
        // defensive and is not: the flag's whole purpose is to carry a request that
        // arrives from ANOTHER THREAD while the burst holds the service mutex, so a
        // burst that clears it on entry SWALLOWS every request raised in the window
        // between the arm and its own first instruction. That window is not small -
        // a host reports the run as armed, a controller sees it and sends Stop, and
        // the pump thread has not been scheduled yet.
        //
        // MEASURED, not reasoned: the wire probe's Stop-during-sysID row, once it was
        // given a positive control, showed the stop taking 11.2 s against a 12.9 s
        // uninterrupted burst - the burst running to completion with the daemon
        // journal carrying ZERO cancellations. Two earlier versions of that row had
        // passed because the burst simply finished first.
        //
        // The ARM is the correct owner: it is where a new run begins, so a stale
        // cancel from a previous one cannot leak into this one, and from that moment
        // the flag means only what a caller has put in it.
        self.sysid_cancel.store(false, Ordering::Release);
        // Taken out and put back so the rest of `self` stays borrowable. On any
        // failure below the backend is dropped and a second arm reports it
        // (there is no re-arm path; the caller rebuilds the service).
        let mut backend = self.backend.take().ok_or_else(|| {
            ServiceError::BadState(
                "the station backend was already consumed by an earlier arm attempt; \
                 rebuild the service to run again"
                    .into(),
            )
        })?;
        backend
            .open()
            .map_err(|e| ServiceError::Plant(e.to_string()))?;

        // THE ACQUISITION SEAM, IN TWO PASSES.
        //
        // Pass one states the rate this service wants. What comes back is the
        // rate the DEVICE programmed, and on this chassis that is routinely a
        // different number (the 9234 rides a 13.1072 MHz/(256n) ladder: ask for
        // 8192 and it programs 8533.33). The block this service asked for was
        // derived from the pre-configure rate, so a coerced rate makes the block
        // the wrong size IN TIME - and the block period is what the engine's tick
        // cadence, ring depth and scheduling-health budget are all built from.
        //
        // Pass two re-derives the block from the ANSWER and reconfigures if it
        // changed. Skipped when it did not, because a reconfigure is not free
        // (it re-commits the DAQmx tasks) and a no-op one would be a re-commit
        // for nothing.
        let requested_rate = self.sample_rate_hz;
        let acq = hertzforge_hal::AcquisitionConfig::new(requested_rate, self.block_frames);
        let mut actual = backend
            .configure(&self.table, &acq)
            .map_err(|e| ServiceError::Plant(e.to_string()))?;
        let want_block = Self::default_block_frames(self.station, actual.actual_rate_hz);
        if want_block != self.block_frames && actual.actual_rate_hz > 0.0 {
            self.log.push(EventKind::Note {
                text: format!(
                    "block re-derived from the ADOPTED rate: {} -> {want_block} frames \
                     ({:.3} ms at {:.3} Hz). The first pass sized the block against the \
                     pre-configure rate.",
                    self.block_frames,
                    want_block as f64 / actual.actual_rate_hz * 1e3,
                    actual.actual_rate_hz
                ),
            });
            self.block_frames = want_block;
            let acq2 = hertzforge_hal::AcquisitionConfig::new(actual.actual_rate_hz, want_block);
            actual = backend
                .configure(&self.table, &acq2)
                .map_err(|e| ServiceError::Plant(e.to_string()))?;
        }

        // ADOPT, then derive. Everything Hz-denominated this service owns is
        // re-derived from `actual` below; nothing downstream may read the request
        // again. This is Random's 6.25x lesson: it asked for 8192 Hz,
        // the 9234 programmed 51 200, and every profile frequency landed 6.25x
        // high on the shaker because the loop kept synthesizing as if it played
        // at 8192.
        // The engage lead-in occupies exactly `min_lookahead_frames`, and the
        // device's own reported group delay rides on top - together they are the
        // best transport-lag ESTIMATE available before sysID measures the real
        // one. See `check_tracking_lag`.
        let lookahead_est = backend
            .caps()
            .output_buffer
            .map(|o| o.min_lookahead_frames as usize)
            .unwrap_or(0)
            + actual.group_delay_frames.unwrap_or(0) as usize;
        self.adopt_configured_rate(
            actual,
            requested_rate,
            backend.caps().max_output_channels,
            lookahead_est,
        )?;

        // THE ARM GATE. Sine requests `{HostSynthesized, HostPerTick}`: it
        // synthesizes every drive sample and its servo/notch/feedforward
        // correct INSIDE the run, per tick. That request carries the shared-clock
        // precondition with it - pointed at a device that closes its own
        // loop, or at a clock domain not proven Shared, arm() hard-refuses with
        // the gate's operator-readable reason rather than running host servo
        // math the hardware cannot honor.
        //
        // `None` FOR THE PROFILE RATE IS CORRECT HERE, and the comment this
        // replaces said the gate was "what makes a coerced rate visible". It is
        // not, for Sine. That parameter is the rate a SAMPLED RECORD was authored
        // at, and the gate refuses when the record's rate contradicts the
        // device's. Sine's profile is a swept BAND - amplitude against frequency,
        // with no sample clock of its own - so there is no authored rate to
        // contradict, and there never will be. Passing the REQUESTED rate would
        // refuse every legitimately coerced run with a nonsense reason; passing
        // the ADOPTED rate would compare the device's answer with itself, making
        // both arms of the gate one authority and the Some->None red mutation
        // unable to red.
        //
        // Coercion visibility is `adopt_configured_rate`'s job, above: a typed
        // COERCION WARNING event, both rates carried on the status, and a refusal
        // when the device answers a rate nothing can run at.
        let station = self.station;
        hertzforge_module_abi::arm_gate(
            &backend.caps(),
            station,
            hertzforge_module_abi::Resolution::host_closed_loop(),
            None,
            actual.actual_rate_hz,
        )
        .map_err(|reason| ServiceError::BadState(format!("arm refused: {reason}")))?;

        backend
            .arm()
            .map_err(|e| ServiceError::Plant(e.to_string()))?;
        backend
            .start()
            .map_err(|e| ServiceError::Plant(e.to_string()))?;
        self.backend = Some(backend);
        self.transition(RunState::SysId);
        Ok(())
    }

    /// Adopt what `configure()` answered as the rate this run is denominated in,
    /// and re-derive everything that was baked against the request.
    ///
    /// Called from `arm_inner` BEFORE the arm gate and before any step is built,
    /// because ~8 arm-time consumers read `self.sample_rate_hz` and every one of
    /// them is wrong if this has not run: the sysID FFT sizing, the burst length,
    /// the FRF bin spacing, the coherence band indices, the step's `fs_hz`, the
    /// settle-window arithmetic, the capture rate, and the control-tick bound below.
    /// Run the sweep-rate-vs-tracking-bandwidth check and act on its verdict.
    /// `provenance` says where the lag number came from, because a refusal that
    /// does not is unarguable-with.
    ///
    /// # Why this is a REFUSAL and not just a warning
    ///
    /// A demodulator that cannot keep up with the sweep does not fail loudly. It
    /// under-reads, the servo closes on the under-read and overdrives the true
    /// response by the same factor, and EVERY protection on the demodulated path
    /// (the scalar abort, the kinematic monitor, the servo-fault detector) is
    /// looking at the same drooped number and sees nothing wrong. The one
    /// protection that can see it is the raw-ring watchdog, which by design does
    /// not read that path, and by the time it fires the article has had the
    /// overdrive. There is no in-run detection to fall back on, which is what
    /// makes this a configuration that should not start rather than one to watch.
    ///
    /// Between the advisory and refuse thresholds it is a WARNING: 0.34 to 1.9 dB
    /// of systematic under-read is real and worth putting in the report, but it
    /// is inside what an operator can reasonably choose to accept.
    ///
    /// # Why the check does NOT credit the delay compensation
    ///
    /// The delay compensation retards the demodulation reference by this same
    /// measured lag, which on paper cancels the beat this check is measuring - so
    /// it would be tempting to assess the residual and let a compensated run
    /// through. Deliberately not done, for one reason: whether the sysID BURST
    /// path's lag is the ENGAGED path's lag is UNMEASURED. The engaged run carries
    /// the AO delivery queue, and the chassis investigation's own magnitude
    /// accounting says the equilibrium droop at 2089 frames explains only about a
    /// third of the observed overdrive - which points at an engaged-path lag LARGER
    /// than the one the burst measured. Crediting a compensation sized on the
    /// smaller number would turn a conservative refusal into a false pass, and the
    /// two errors do not cost the same: a false refusal costs a config edit, a
    /// false pass costs an armature. One chassis row (cross-correlate the CAPTURED
    /// drive against the captured response of an engaged run) settles it; until
    /// then the gate reads the raw lag.
    fn check_tracking_lag(
        &mut self,
        lag_frames: usize,
        fs_hz: f64,
        provenance: &str,
    ) -> Result<(), ServiceError> {
        // Use the LIVE band, which `adopt_configured_rate` may have clamped to
        // Nyquist, rather than the config's original one.
        let verdict = match &self.cfg.mode {
            SineMode::Sweep(c) => hertzforge_sine_assess_tracking_lag(
                self.band,
                c.rate,
                self.cfg.tracking_bw,
                lag_frames,
                fs_hz,
            ),
            _ => return Ok(()),
        };
        match verdict {
            TrackingLagVerdict::Ok { .. } => Ok(()),
            TrackingLagVerdict::Advisory { message, .. } => {
                self.log.push(EventKind::Note {
                    text: format!("tracking-lag ADVISORY ({provenance}): {message}"),
                });
                Ok(())
            }
            TrackingLagVerdict::Refuse { message, .. } => Err(ServiceError::Config(
                ConfigError::BadConfig(format!("{message} (lag {provenance})")),
            )),
        }
    }

    fn adopt_configured_rate(
        &mut self,
        actual: hertzforge_hal::ConfiguredActual,
        requested_hz: f64,
        caps_out: u32,
        lag_estimate_frames: usize,
    ) -> Result<(), ServiceError> {
        let fs = actual.actual_rate_hz;

        // A NON-RATE IS A REFUSAL, NOT A DEFAULT. A backend answering 0 (or NaN,
        // or a negative) has not told us what it programmed, and every derivation
        // below would silently produce infinities or zeros - a sweep at 0 Hz, an
        // FFT with no bin spacing, a settle window of forever. "Not known" is
        // never "the same as what I asked for" (the `group_delay_frames: None`
        // convention, applied to the rate).
        if !fs.is_finite() || fs <= 0.0 {
            return Err(ServiceError::Plant(format!(
                "the backend configured at {fs} Hz, which is not a rate this run can be \
                 denominated in. Requested {requested_hz:.3} Hz. Refusing to arm rather than \
                 deriving a sweep, an FFT and a settle window from it."
            )));
        }

        // THE COERCION WARNING. Relative, not absolute: a 0.01 Hz difference is
        // nothing at 51.2 kHz and everything at 1 Hz. 1e-6 is tight enough that a
        // real ladder step (8192 -> 8533.33 is 4%) is unmissable and loose enough
        // that an f64 round-trip through the driver is not a false alarm.
        let coerced = (requested_hz - fs).abs() / fs > 1e-6;
        if coerced {
            self.log.push(EventKind::RateCoerced {
                requested_hz,
                actual_hz: fs,
            });
        }
        self.sample_rate_hz = fs;
        // `configured` is set only once the answer has been judged fit to run on:
        // a caller reading it is reading the authority for the block period, the
        // ring depth and the health budget, and a 0 Hz answer must not be
        // reachable through it.
        self.configured = Some(actual);

        // THE NYQUIST / BAND GUARD, at the adopted rate.
        //
        // 0.45 x fs, not 0.5: at exactly Nyquist the NCO's tone is
        // indistinguishable from DC and the tracking filter has no band left to
        // sit in, and `SweepOscillator::set_frequency` PANICS out of band. The
        // 10% back-off is the same one the oscillator's own clamp uses.
        //
        // The LOW edge is a REFUSAL and the HIGH edge is a CLAMP, deliberately: a
        // band whose bottom is above Nyquist has nothing left to run and silently
        // narrowing it to a single point would be inventing a test; a band whose
        // top is above Nyquist still has a real test underneath it, and the
        // operator is told exactly what was taken away.
        let nyq = 0.45 * fs;
        if self.band.0 >= nyq {
            return Err(ServiceError::Config(ConfigError::BadConfig(format!(
                "the run band starts at {:.3} Hz, at or above 0.45 x the ADOPTED rate \
                 ({nyq:.3} Hz of {fs:.3} Hz). Nothing in this band can be synthesized. \
                 (Requested rate was {requested_hz:.3} Hz{}.)",
                self.band.0,
                if coerced { ", COERCED" } else { "" }
            ))));
        }
        if self.band.1 > nyq {
            self.log.push(EventKind::BandClamped {
                from_hz: self.band.1,
                to_hz: nyq,
                reason: format!(
                    "0.45 x the adopted rate {fs:.3} Hz; the sweep will turn around here \
                     instead of at the configured edge"
                ),
            });
            self.band.1 = nyq;
        }

        // THE SWEEP-RATE vs TRACKING-BANDWIDTH GATE, at the ADOPTED
        // rate and the CLAMPED band, because both are inputs to it.
        //
        // This pass uses the ESTIMATE (lead-in + reported group delay), which is
        // a floor: it cannot see device FIFO depth, so it will pass a config the
        // measured lag later refuses. That is the right asymmetry - refusing
        // early is free, and the authoritative pass runs after sysID measures the
        // real lag (`check_tracking_lag` at the sysID alignment).
        self.check_tracking_lag(lag_estimate_frames, fs, "estimated from the engage lead-in")?;

        // THE CONTROL-TICK BOUND, RESTATED IN THE UNITS THE PUMP USES, and
        // STATION-SPLIT for the same reason `default_block_frames`,
        // `default_jitter_frac` and `default_compute_budget` above are.
        //
        // What it protects: the command mailbox is drained by the STEP at a
        // control tick. A control tick fires every `control_tick_frames` engine
        // FRAMES. If no control tick fires inside the unit the pump makes
        // decisions on, STOP is never seen, the scheduler never advances, and the
        // run spins doing nothing while reporting Engaged. The old bound compared
        // that frame count against `chunk_ticks`, a number handed to the engine as
        // max_TICKS - and one tick is one BLOCK, so on hardware it compared frames
        // to blocks and was vacuous by three orders of magnitude.
        //
        // HARDWARE: the bound is the BATCH, `TICKS_PER_BATCH * block`. That is a
        // stop-LATENCY guarantee - the batch is what stands between an operator's
        // Stop and the drive acting on it, and a control tick that spans several
        // batches multiplies that latency by however many.
        //
        // HERTZFORGE-SIM: the bound is the CHUNK, deliberately NOT the batch form. The
        // Sim's natural block is ONE FRAME (`SIM_NATURAL_BLOCK_FRAMES`), so the
        // batch form is a 2-frame budget and would refuse every HERTZFORGE-SIM run in
        // the tree - including the ones that prove the control law. It is also the
        // wrong question there: the harness has no wall clock, so there is no
        // latency for the batch to bound, and the decision unit really is the chunk
        // (`run_chunk` is where status is drained, the abort latch read, the stop
        // flag read and the state machine advanced). The step's `tick_frames`
        // accumulator persists across batches, so a control tick spanning several
        // of them is correct behaviour, not a stall.
        let (budget_frames, unit) = if self.station.is_hardware() {
            (
                TICKS_PER_BATCH as usize * actual.block_frames.max(1),
                "BATCH",
            )
        } else {
            ((fs * CHUNK_SECONDS).max(MIN_CHUNK_FRAMES) as usize, "CHUNK")
        };
        if self.cfg.control_tick_frames > budget_frames {
            return Err(ServiceError::Config(ConfigError::BadConfig(format!(
                "control_tick_frames ({}) exceeds the per-{unit} frame budget ({budget_frames} \
                 frames = {:.3} ms at the ADOPTED {fs:.3} Hz; block {} frame(s), \
                 {TICKS_PER_BATCH} tick(s) per batch). At least one control tick must fire per \
                 {unit} or the command mailbox is never drained, STOP is not honored, and the \
                 scheduler stalls while the run reports Engaged.",
                self.cfg.control_tick_frames,
                budget_frames as f64 / fs * 1e3,
                actual.block_frames,
            ))));
        }

        // THE COLA WIDTH RE-CHECK. The construction-time check read
        // `caps.max_output_channels` before anything was configured; this reads
        // what the CONFIGURED station presents. A station that advertised two
        // outputs and configured one would otherwise reach the step with
        // `out_channels = 2` and write a COLA frame into a device with nowhere to
        // put it.
        //
        // `caps_out` is a PARAMETER and not read off `self.backend`, because
        // `arm_inner` has taken the backend out of the service for the duration -
        // reading the field here answers `None`, i.e. zero outputs, i.e. a refusal
        // for every COLA run. (Found by `stop_paths::the_stop_waits_for_the_cola_
        // aux_too` on the first run: the check refused its own healthy fixture.)
        if self.cfg.cola.enabled && caps_out < 2 {
            return Err(ServiceError::Config(ConfigError::BadCola(format!(
                "COLA needs 2 output channels; the CONFIGURED station presents \
                 {caps_out}. (The construction-time check read the pre-configure caps.)"
            ))));
        }

        Ok(())
    }

    /// Whether the device coerced the requested rate (the run is denominated in
    /// the ANSWER either way; this is what makes the difference visible).
    pub fn rate_was_coerced(&self) -> bool {
        self.log
            .events()
            .iter()
            .any(|e| matches!(e.kind, EventKind::RateCoerced { .. }))
    }

    // --- sysID + coherence gate --------------------------------------------------

    /// Run the pre-test system-ID: a low-level broadband noise burst driven
    /// through the plant, then `h_estimate_hv_with_coherence` (drive -> control
    /// channel) to build the feedforward table + trust mask + FRF display, and
    /// the band coherence. Transitions `SysId -> CoherenceGate`. `pretest_frames`
    /// is the burst length (>= a few nfft; the service picks a default if 0).
    pub fn run_sysid(&mut self, pretest_frames: usize) -> Result<(), ServiceError> {
        if !matches!(self.state, RunState::SysId) {
            return Err(ServiceError::BadState(format!(
                "run_sysid from {:?}",
                self.state
            )));
        }
        let fs = self.sample_rate_hz;
        // The sysID FRF must RESOLVE the sharpest resonance in band or the
        // feedforward under-predicts the drive cut at the peak and the +-1 dB accuracy
        // bar fails (the old 4096-point FRF gave df ~= 4 Hz at 16 kHz, but a Q=50
        // peak at 250 Hz is only f/Q ~= 5 Hz wide, so ~1 bin spanned the peak -
        // far too coarse). Choose nfft so several bins span the narrowest expected
        // peak: target df <= (f_lo / Q_target) / 4, i.e. at least ~4 bins across a
        // peak of bandwidth f/Q at the low band edge (the worst case, since the
        // fractional bandwidth f/Q/f = 1/Q is constant but the absolute width
        // shrinks with f). Floor at 16384 (~1 Hz bins at 16 kHz) so the default
        // is always fine for the Q<=50 fixtures even without an explicit Q hint.
        let q_target = D8_SYSID_Q_TARGET;
        let f_lo = self.band.0.max(1.0);
        let target_df = (f_lo / q_target / 4.0).max(0.05);
        let mut nfft = 16_384usize;
        while (fs / nfft as f64) > target_df && nfft < 1 << 20 {
            nfft *= 2;
        }
        // Lengthen the burst with the FFT size so the averaged FRF has enough
        // segments to keep the coherence + |H| estimate low-variance at the fine
        // resolution (n >= nfft * 8).
        let n = pretest_frames.max(nfft * 8);
        // Build a low-level WHITE noise burst (bare xorshift, no shaping - the xcorr must treat it as white) as the sysID drive (in
        // NCO-command units: the drive is recorded in drive-scale units,
        // not volts). Reuse the sim PRNG shape via a simple LCG for determinism.
        let sysid_scale = 10f64.powf(self.cfg.sysid_level_db / 20.0) * 0.05;
        let mut rng: u64 = self.sysid_seed ^ 0x5EED_1234;
        let mut drive: Vec<f64> = Vec::with_capacity(n);
        for _ in 0..n {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let u = (rng >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
            drive.push(u * 2.0 * sysid_scale);
        }

        // Drive the plant burst through the backend and record the control
        // channel's response, one frame per write (frames_per_tick = 1 shape).
        let response = self.drive_burst(&drive)?;

        // PARK THE DEVICE BEFORE THE GATE COMPUTE.
        //
        // What follows this line is a full FRF over up to 262 144 bins plus a
        // feedforward-table build, and until now the AO ran through all of it -
        // live, at the sysID level, with nothing watching it and no control loop
        // to answer for it. If the gate then HELD, the device stayed there
        // indefinitely: `CoherenceHold` has no release path, so "parked" meant a
        // pin driving noise until someone rebuilt the service. The engage that
        // follows a PASS is a cold start either way (the step has not been built
        // yet), so there is nothing to preserve by leaving it running.
        self.park_after_sysid();

        // ALIGN drive and response by the pipeline lag - MEASURED, not derived.
        //
        // On a hardware station the response trails the drive by the output
        // lookahead, the device group delay, AND device/USB FIFO depths no caps
        // field reports. H and coherence correlate the two SIGNALS, so without the
        // shift every nfft window pairs the drive with a DIFFERENT slice of the
        // response, and in-band coherence on a perfectly coherent jumpered wire
        // reads far below 1. Random measured 0.04 on this exact plant before its
        // xcorr port and TWR the identical failure at 0.010; Sine's
        // un-ported ceiling on the chassis was 0.990, which passed a 0.5 floor and
        // looked fine.
        //
        // A peak that is not decisively above the correlation floor leaves the
        // alignment OFF: a wrong confident shift would MANUFACTURE coherence where
        // there is none, and refusing exactly that run is the gate's whole job.
        // The Sim plant peaks at lag 0 and the shift is a no-op, which is why
        // every Sim gate passed while the hardware one held.
        let (drive, response) = {
            let (lag, peak_r, searched, diag) = measured_pipeline_lag(&drive, &response, fs);
            self.sysid_lag_frames = lag;
            self.log.push(EventKind::Note {
                text: format!(
                    "sysID align: record {} samples, nfft {nfft}, lag {lag} frames, peak |r| \
                     {peak_r:.3}, searched {searched} lags; {diag}",
                    drive.len()
                ),
            });
            if lag > 0 && lag < response.len() {
                let mut resp = response;
                resp.drain(..lag);
                let mut d = drive;
                d.truncate(resp.len());
                (d, resp)
            } else {
                (drive, response)
            }
        };

        // THE AUTHORITATIVE SWEEP-RATE vs TRACKING-BANDWIDTH GATE.
        //
        // The `configure` pass ran on an ESTIMATE (the engage lead-in plus the
        // device's reported group delay) which cannot see FIFO depth and is
        // therefore a floor. THIS lag is the one the FRF path just measured by
        // cross-correlating drive against response - the same number the FRF
        // alignment above trusts enough to shift a whole record by, so it is the
        // number the tracking check has to use too. Placed here, before the
        // coherence gate and long before engage, with the device already parked
        // by `park_after_sysid`: a refusal leaves nothing energized.
        self.check_tracking_lag(self.sysid_lag_frames, fs, "MEASURED by sysID")?;

        let (h_full, coh_full) =
            hertzforge_dsp::frf::h_estimate_hv_with_coherence(&drive, &response, fs, nfft);
        let df = fs / nfft as f64;
        // DROP the DC bin (k = 0): its frequency is 0 Hz, and the feedforward
        // builder (`FeedforwardTable::from_sysid`) REQUIRES a strictly-positive
        // frequency grid, so a grid that starts at 0 makes it reject the WHOLE
        // table (every bin untrusted -> the feedforward silently never engages,
        // leaving the loop on feedback alone: the ~2.2 dB through-resonance
        // residual the +-1 dB bar was hitting). DC carries no sine-FRF information
        // anyway. Start the grid at bin 1 (the real feedforward fix).
        let coh: Vec<f64> = coh_full.iter().skip(1).copied().collect();
        let h_mag: Vec<f64> = h_full
            .iter()
            .skip(1)
            .map(|&(re, im)| (re * re + im * im).sqrt())
            .collect();
        let freqs: Vec<f64> = (1..=h_mag.len()).map(|k| k as f64 * df).collect();

        // Band coherence: mean coherence over the run band. For a DWELL the band
        // is degenerate `(f, f)`, so widen it to a +-1 octave window around the
        // dwell frequency so the gate assesses the plant near where the test
        // actually excites (a zero-width band gives an empty, meaningless mean).
        let (b_lo, b_hi) = if (self.band.1 - self.band.0).abs() < 1e-6 {
            let f = self.band.0.max(df);
            (f * 0.5, f * 2.0)
        } else {
            self.band
        };
        // The DC bin was dropped, so `coh[i]` is FFT bin `i + 1` (frequency
        // `(i + 1) * df`). Convert the absolute band bins to array indices by
        // subtracting the 1-bin DC offset.
        let lo_bin_abs = ((b_lo / df).floor() as usize).max(1);
        let hi_bin_abs = (b_hi / df).ceil() as usize;
        let lo_idx = lo_bin_abs
            .saturating_sub(1)
            .min(coh.len().saturating_sub(1));
        let hi_idx = hi_bin_abs.saturating_sub(1).max(lo_idx + 1).min(coh.len());
        let n_band = (hi_idx.saturating_sub(lo_idx)).max(1);
        self.band_coherence = coh[lo_idx..hi_idx].iter().sum::<f64>() / n_band as f64;

        // Build the feedforward table (target = full profile in g pk). The |H| is
        // response-g per unit NCO drive command.
        let profile = self.cfg.profile.clone();
        let ff = FeedforwardTable::from_sysid(
            &freqs,
            &h_mag,
            &coh,
            |f| profile.target_accel_g_pk(f),
            self.cfg.feedforward,
        )
        .ok();
        let trusted: Vec<bool> = ff
            .as_ref()
            .map(|t| t.trusted().to_vec())
            .unwrap_or_else(|| vec![false; freqs.len()]);
        self.frf = Some(FrfDisplay {
            freqs,
            h_mag,
            coherence: coh,
            trusted,
        });
        self.feedforward = ff;
        self.transition(RunState::CoherenceGate);
        Ok(())
    }

    /// Take the device off drive between the sysID burst and the gate decision.
    ///
    /// The burst ends with the AO holding its last sample and the task running.
    /// Everything after it - the lag search, the FRF, the feedforward build, and
    /// the operator's own look at the numbers if `review_hold` is on - happens
    /// with no control loop and nothing watching that pin. This ramps it out
    /// through the backend's own controlled taper and stops the tasks, so the two
    /// parks (`CoherenceHold`, `AwaitingReview`) hold with the drive at zero
    /// rather than live and unwatched.
    ///
    /// The engage that follows a PASS is a COLD START in either case - the step
    /// does not exist until `engage()` builds it - so nothing is lost by stopping
    /// here, and `engage()` re-arms and re-starts the device with a real-drive
    /// lead-in rather than the silence a bare restart would prime.
    fn park_after_sysid(&mut self) {
        self.drain_generation();
        if let Some(b) = self.backend.as_mut() {
            // `Backend::stop` is the CONTROLLED stop: the NI path tapers the last
            // sample out over its own ramp before it kills the task. An
            // `emergency_stop` here would be the wrong verb - nothing is wrong.
            if let Err(e) = b.stop() {
                self.log.push(EventKind::Note {
                    text: format!("parking the device after sysID reported {e}"),
                });
            }
        }
        self.device_parked = true;
    }

    /// Synthesize the engage LEAD-IN from the built step and load it into the
    /// device, head-until-refusal.
    ///
    /// # Why this is not the record modules' lead-in, and why it is still needed
    ///
    /// The record modules' lead-in rails assume a PRECOMPUTED RECORD whose head
    /// carries a startup ramp; Sine has no record - the NCO synthesizes per frame,
    /// and its first frames ARE the real drive. What Sine shares with them is the
    /// hazard at the other end: a non-regenerating AO refuses to start on an empty
    /// buffer, so `NiBackend::start` tops the queue up with SILENCE - and that
    /// silence is then the first thing the clock plays after the engage. The fix is
    /// the same shape: put real drive in the queue BEFORE start, so there is
    /// nothing for the backend's own prefill to add.
    ///
    /// The drive comes from [`engine::SineProcessStep::synthesize_leadin`], which
    /// advances the NCO and writes frames through the SAME per-frame writer
    /// `process` uses, but runs no control tick. Calling `process` on zeros
    /// instead - Explorer's arm shape, which is legitimate there because Explorer
    /// has no servo - runs real ticks against a measured amplitude of zero and
    /// spends the level-settle window before the run has acquired a frame; see
    /// that method for the measurement.
    ///
    /// HEAD-UNTIL-REFUSAL MINUS A HOLD-BACK, never a cursor offset into a record.
    /// The delivery queue keeps a permanent low-water reserve it cannot be drained
    /// past, so filling to refusal and then letting the engine continue from frame
    /// zero is correct; a cursor offset was tried on the chassis and refuted in
    /// one instrumented run (zero tail + lost runway). The hold-back
    /// leaves the engine room to deliver its first block without the queue being
    /// full.
    fn prefill_engage_leadin(&mut self) -> Result<(), ServiceError> {
        let out_ch = self.cfg.out_channels().max(1);
        let block = self
            .configured
            .map(|a| a.block_frames)
            .unwrap_or(self.block_frames)
            .max(1);
        let (Some(step), Some(backend)) = (self.step.as_mut(), self.backend.as_mut()) else {
            return Ok(());
        };
        // EXACTLY THE LOOKAHEAD, and not one frame more.
        //
        // The lead-in exists to occupy the space `Backend::start` would otherwise
        // fill with SILENCE, which is `min_lookahead_frames` - the low-water
        // reserve a non-regenerating AO refuses to start below. Filling further is
        // not "more safety margin", it is DEPTH, and depth is drive the servo
        // cannot take back: every frame queued ahead is a frame of dead time
        // between a control decision and its response. MEASURED 2026-08-08:
        // prefilling all available space on the Sim (a 4096-frame buffer against a
        // 4-frame lookahead) put ~1000x the intended delay in the loop and five
        // sim_e2e fixtures aborted low-side while the servo wound up against a
        // stale response. The queue-depth tradeoff belongs to the
        // operator per rig; this code's job is to match the backend's own stated
        // reserve, not to have an opinion about it.
        let lookahead = backend
            .caps()
            .output_buffer
            .map(|o| o.min_lookahead_frames as usize)
            .unwrap_or(0);
        // How much room there is. `None` = a lockstep backend with no independent
        // output buffer, which cannot start on an empty one either.
        let Some(space) = backend.output_space() else {
            return Ok(());
        };
        // `Some(0)` HERE IS THE UNKNOWN, NOT A FULL DEVICE.
        // `Backend::output_space` renders a failed space query as `Some(0)` -
        // the conservative answer for a RUNNING task, and a lie on a PARKED one
        // whose buffer is empty by construction. Believing it skips the lead-in
        // entirely, and `Backend::start` then fills exactly this space with
        // SILENCE: the cold-start silence the record lead-in exists to prevent,
        // reintroduced by a query failure. Measured in the classical firer's
        // equivalent loop on the cDAQ-9178, 2026-08-08. Ask for the lookahead
        // and let `write_drive`'s accept count (the loop's `Ok(0) => break`) be
        // the authority on what actually fits.
        let want = if space == 0 {
            lookahead
        } else {
            lookahead.min(space)
        };
        if want == 0 {
            return Ok(());
        }
        let mut leadin: Vec<f32> = Vec::with_capacity(want * out_ch);
        step.synthesize_leadin(want, &mut leadin);
        let mut written = 0usize;
        while written < want {
            match backend.write_drive(&leadin[written * out_ch..]) {
                Ok(0) => break, // refusal: the queue is as full as it will get
                Ok(n) => written += n,
                Err(e) => {
                    return Err(ServiceError::Plant(format!("engage lead-in: {e}")));
                }
            }
        }
        self.log.push(EventKind::Note {
            text: format!(
                "engage cold start: {written} frame(s) of REAL drive loaded before start \
                 (queue had {space}, held back {block} for the first tick). The device's own \
                 start prefill has nothing left to fill with silence."
            ),
        });
        Ok(())
    }

    /// Bring the device back up for the engaged run, from the parked state
    /// `park_after_sysid` left it in.
    ///
    /// Re-`arm` + `start`. What makes this safe to do is what the caller does
    /// FIRST: `engage()` has already built the step and pushed a real-drive
    /// lead-in, so `NiBackend::start`'s own prefill has nothing left to top up
    /// with silence.
    fn unpark_for_engage(&mut self) -> Result<(), ServiceError> {
        if !self.device_parked {
            return Ok(());
        }
        let backend = self
            .backend
            .as_mut()
            .ok_or_else(|| ServiceError::BadState("not armed".into()))?;
        backend
            .arm()
            .map_err(|e| ServiceError::Plant(format!("re-arm for engage: {e}")))?;
        backend
            .start()
            .map_err(|e| ServiceError::Plant(format!("re-start for engage: {e}")))?;
        self.device_parked = false;
        Ok(())
    }

    /// Ask a running sysID burst to stop at the next block boundary.
    ///
    /// The handle is an `Arc` so a caller can take it BEFORE locking the service
    /// and set it from another thread while `run_sysid` holds the lock. Without
    /// that, a Stop during sysID cannot be seen until the whole burst finishes -
    /// which at `t ~= 1600 / f_lo` seconds is 27 s at 60 Hz and 80 s at 20 Hz,
    /// with the AO live at the sysID level throughout.
    pub fn sysid_cancel_flag(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.sysid_cancel.clone()
    }

    /// The pipeline lag the sysID measured, in frames (0 = no decisive peak, so
    /// no alignment was applied).
    pub fn sysid_lag_frames(&self) -> usize {
        self.sysid_lag_frames
    }

    /// Drive a burst of drive samples through the persistent backend and return
    /// the control channel's response, one plant frame per drive sample (the
    /// frames_per_tick = 1 shape: every drive value reaches the plant).
    fn drive_burst(&mut self, drive: &[f64]) -> Result<Vec<f64>, ServiceError> {
        let n_in = self.n_input_slots().max(1);
        let backend = self
            .backend
            .as_mut()
            .ok_or_else(|| ServiceError::BadState("not armed".into()))?;
        let ctrl_slot = self.control_slots.first().copied().unwrap_or(0);

        // BLOCK-WISE, through the shared delivery helper. This loop used to
        // write ONE frame and read ONE frame per sample, and said so:
        // "the sysID burst is a Sim-only path". It was - and nothing stopped it
        // running on hardware once the arm gate let Sine arm on a proven clock.
        //
        // On a driver-paced device it cannot keep up. The device produces a
        // frame every 19.5 us at 51.2 kHz while a single-frame DAQmx write+read
        // round trip costs several times that, so across a sysID burst (n =
        // nfft * 8, which is ~2 M samples at the resolution the +-1 dB bar needs)
        // the reader falls behind the acquisition. MEASURED on the real
        // cDAQ-9178 2026-07-30: the burst either dies at -200279 ("not able to
        // keep up with the hardware acquisition") or limps through with drive
        // and response desynchronized and reports a garbage band coherence
        // (0.07 on a jumpered WIRE, where it must be ~1.0). Both outcomes are
        // this one defect; which one you get is a race.
        //
        // `play_block_wise` is the contract the rest of the tree already uses:
        // it honors `write_drive`'s ACCEPTED count, polls `NoDataYet` against a
        // deadline instead of assuming a frame is there, and hands back only
        // frames the device actually delivered - so `drive[k]` and the returned
        // `response[k]` stay paired, which is the whole premise of the FRF and
        // coherence estimate downstream.
        // WIDEN THE BURST TO THE STATION'S OUTPUT WIDTH.
        //
        // `write_drive` takes frames interleaved at `n_output_channels`, and this
        // used to hand it a MONO buffer with `n_drive = 1`. With COLA off the two
        // agree and nothing showed. With COLA ON the station drives 2 outputs, so
        // a mono buffer is read as half as many 2-channel frames with the burst
        // alternating across BOTH DACs - the sysID would identify the plant from a
        // drive that never existed, and on this mock it deadlocked outright
        // ("accepted nothing after 131071 frames"). Found by
        // `stop_paths::the_stop_waits_for_the_cola_aux_too`, 2026-08-08.
        //
        // The aux carries ZERO through sysID. It is a reference output for the
        // article's own instrumentation, not part of the plant being identified,
        // and the parks either side of the burst hold it at zero anyway.
        let out_ch = self.cfg.out_channels().max(1);
        let mut drive_f32: Vec<f32> = Vec::with_capacity(drive.len() * out_ch);
        for &d in drive {
            drive_f32.push(d as f32);
            drive_f32.extend(std::iter::repeat_n(0.0f32, out_ch - 1));
        }
        let block = self.block_frames.max(1);
        let mut scratch = vec![0.0f32; block * n_in];
        let play = hertzforge_hal::BlockPlay::new(block);
        let mut out = Vec::with_capacity(drive.len());
        // THE CANCELLATION FLAG (the same seam the Shock module uses). Read once per
        // block-loop iteration, so a Stop or an Abort raised while the burst is
        // playing takes effect within ONE BLOCK instead of waiting out
        // `t ~= 1600 / f_lo` seconds - 27 s at a 60 Hz band edge, 80 s at 20 Hz, with
        // the AO live at the sysID level the whole time and no control loop watching
        // it. That window was the longest unanswerable stop in the module.
        // NOT cleared here. `arm_inner` owns that, and the comment there says why:
        // a burst that resets its own cancellation flag on entry swallows every
        // request raised between the arm and its first instruction, which is exactly
        // the window a controller's Stop lands in.
        let cancel = self.sysid_cancel.clone();
        hertzforge_hal::play_block_wise(
            backend.as_mut(),
            &drive_f32,
            out_ch,
            n_in,
            &mut scratch,
            &play,
            // Nothing pre-committed: the sysID burst plays into a task that is
            // already running (the pre-commit seam is for the shock modules'
            // per-shot cold task, which this is not).
            0,
            Some(&cancel),
            |frames, n| {
                for f in 0..n {
                    out.push(frames[f * n_in + ctrl_slot] as f64);
                }
            },
        )
        .map_err(|e| match e {
            // A CANCELLED burst is not a plant fault, and the accounting has to be
            // honest about what the device will still play: `frames_committed` is
            // what it ACCEPTED, and everything between `frames_done` and that is
            // already queued in hardware and will come out of the pin whatever the
            // host does next. The caller's job after this is the drain and the
            // stop, not to pretend the drive is already off.
            hertzforge_hal::StreamError::Cancelled {
                frames_done,
                frames_committed,
                frames_expected,
            } => ServiceError::BadState(format!(
                "sysID burst CANCELLED after {frames_done} of {frames_expected} frames;                  {} frame(s) are committed at the device and will still play",
                frames_committed.saturating_sub(frames_done)
            )),
            other => ServiceError::Plant(other.to_string()),
        })?;
        Ok(out)
    }

    /// The coherence gate (report-and-hold, never auto-raise).
    ///
    /// Returns whether the gate PASSED - which is not the same as "the run
    /// engaged". Three outcomes:
    ///
    /// * below the floor: park at [`RunState::CoherenceHold`] and return `false`.
    ///   A gate failure, releasable only by rebuilding the service.
    /// * at or above the floor with `review_hold` set (the DEFAULT): park at
    ///   [`RunState::AwaitingReview`] and return `true`. Nothing is built and
    ///   nothing is driven until [`SineService::proceed_after_review`].
    /// * at or above the floor with `review_hold` clear: engage immediately and
    ///   return `true`.
    ///
    /// A caller that treats `true` as "engaged" will spin doing nothing against a
    /// held run, which is why the two parks are distinct STATES rather than a
    /// flag on one.
    pub fn coherence_gate(&mut self) -> Result<bool, ServiceError> {
        if !matches!(self.state, RunState::CoherenceGate) {
            return Err(ServiceError::BadState(format!(
                "coherence_gate from {:?}",
                self.state
            )));
        }
        let passed = self.band_coherence >= self.cfg.coherence_floor;
        self.log.push(EventKind::CoherenceGate {
            band_coherence: self.band_coherence,
            floor: self.cfg.coherence_floor,
            passed,
        });
        if !passed {
            self.log.push(EventKind::Note {
                text: format!(
                    "COHERENCE GATE HOLD: band coherence {:.3} < floor {:.3}. Parked; the run \
                     will NOT auto-raise level. Operator must improve setup + retry.",
                    self.band_coherence, self.cfg.coherence_floor
                ),
            });
            self.transition(RunState::CoherenceHold);
            return Ok(false);
        }
        if self.cfg.review_hold {
            self.log.push(EventKind::Note {
                text: format!(
                    "REVIEW HOLD: coherence gate PASSED at {:.4} (floor {:.3}). The sysID |H| \
                     and coherence are published; the loop is NOT closed and nothing is driven \
                     until proceed_after_review().",
                    self.band_coherence, self.cfg.coherence_floor
                ),
            });
            self.transition(RunState::AwaitingReview);
            return Ok(true);
        }
        self.engage()?;
        Ok(true)
    }

    /// Release the [`RunState::AwaitingReview`] park: build the step and engage.
    ///
    /// **Refuses from [`RunState::CoherenceHold`], by name.** That park is the
    /// gate FAILING, and the two look adjacent enough on a panel that mapping one
    /// release verb onto both is an easy mistake to make - it would ship a
    /// one-click bypass of a safety refusal. The only exit from CoherenceHold is
    /// rebuilding the service, which forces the operator back through setup.
    pub fn proceed_after_review(&mut self) -> Result<(), ServiceError> {
        match self.state {
            RunState::AwaitingReview => self.engage(),
            RunState::CoherenceHold => Err(ServiceError::BadState(format!(
                "proceed_after_review REFUSED from CoherenceHold: the coherence gate FAILED \
                 ({:.4} < floor {:.3}), it did not pass and pause. This park has no release \
                 verb - improve the setup and reset the run. Only AwaitingReview (a gate that \
                 PASSED) is releasable.",
                self.band_coherence, self.cfg.coherence_floor
            ))),
            other => Err(ServiceError::BadState(format!(
                "proceed_after_review from {other:?} (only valid at AwaitingReview)"
            ))),
        }
    }

    /// Build the step and enter [`RunState::Engaged`] at the start level. The ONE
    /// engage path: after a gate that passed with no hold, and after the review
    /// park is released.
    fn engage(&mut self) -> Result<(), ServiceError> {
        self.build_step()?;
        // THE COLD START. The device is parked - `park_after_sysid`
        // stopped the tasks so the gate compute and the review park did not sit on
        // a live pin - and bringing it back up naively means the first thing the
        // clock plays is the backend's own silence prefill, at the moment the loop
        // engages. Synthesize the lead-in FROM THE BUILT STEP first, so the
        // device's queue holds REAL DRIVE before `start` has anything to top up.
        self.prefill_engage_leadin()?;
        self.unpark_for_engage()?;
        // Record the start level in the history.
        let level = self.cfg.ladder.start_db;
        self.level_history.push(LevelHistoryEntry {
            level_db: level,
            auto: true,
            at_tick: 0,
        });
        self.log.push(EventKind::LevelChange {
            from_db: level,
            to_db: level,
            auto: true,
        });
        self.transition(RunState::Engaged);
        Ok(())
    }

    /// Build the persistent [`SineProcessStep`] from the resolved config +
    /// feedforward + limit specs, and keep its shared handles.
    fn build_step(&mut self) -> Result<(), ServiceError> {
        let fs = self.sample_rate_hz;
        let n_input = if self.configured.is_some() {
            self.n_input_slots()
        } else {
            self.control_slots.len().max(1)
        };

        // The servo: normalized target 1.0; (ki, max_rate) from the
        // compression preset; a generous drive ceiling.
        let (ki, max_rate) = self.cfg.compression.gains();
        let servo = AmplitudeServo::new(1.0, max_rate)
            .with_gains(0.3, ki)
            .with_drive_ceiling(1e6)
            .with_fault_fraction(self.cfg.fault_fraction_effective());
        let blend = BlendLaw::new(BlendConfig {
            w: self.cfg.blend_w,
            max_step_db_per_tick: 1.0,
        })
        .map_err(|e| ServiceError::Config(ConfigError::BadConfig(e.to_string())))?;

        // Limit channels: build a NotchGovernor over them, and a spec per channel
        // carrying the sysID |H| fallback (the FRF grid restricted to that
        // channel; here all limit channels share the control-channel FRF because
        // the Sim presents one plant response - a real MIMO sysID would give a
        // per-channel FRF).
        let notch = if self.limit_specs.is_empty() {
            None
        } else {
            Some(
                NotchGovernor::new(self.cfg.notch, self.limit_specs.len())
                    .map_err(|e| ServiceError::Config(ConfigError::BadConfig(e.to_string())))?,
            )
        };
        let sysid_grid = self
            .frf
            .as_ref()
            .map(|f| (f.freqs.clone(), f.h_mag.clone()));
        let limit_channels: Vec<LimitChannelSpec> = self
            .limit_specs
            .iter()
            .map(|(slot, idx, prof)| LimitChannelSpec {
                input_slot: *slot,
                channel_index: *idx,
                profile: prof.clone(),
                sysid_h: sysid_grid.clone(),
            })
            .collect();

        // The scheduler, from the mode.
        let scheduler = self.build_scheduler()?;
        let ladder = LadderPlan::new(self.cfg.ladder.clone())
            .map_err(|e| ServiceError::Config(ConfigError::BadConfig(e.to_string())))?;

        let start_f = self.start_freq();
        let start_scale = 10f64.powf(self.cfg.ladder.start_db / 20.0) * self.seed_drive_scale();

        let cola = ColaState {
            enabled: self.cfg.cola.enabled,
            amp: self.cfg.cola.volts_pk,
            waveform: self.cfg.cola.waveform,
        };
        let (max_v, max_d) = self.kinematic_bounds();
        let build = StepBuild {
            fs_hz: fs,
            control_tick_frames: self.cfg.control_tick_frames,
            out_channels: self.cfg.out_channels(),
            profile: self.cfg.profile.clone(),
            n_input_channels: n_input,
            control_slots: self.control_slots.clone(),
            control_weights: self.cfg.control.weights(),
            composite_mode: self.cfg.control.mode,
            tracking_bw: self.cfg.tracking_bw,
            servo,
            blend,
            feedforward: self.feedforward.clone(),
            notch,
            limit_channels,
            scheduler,
            ladder,
            cola,
            drive_guard: DriveGuard::new(self.cfg.drive_vpk_ceiling, self.cfg.drive_vpk_abort)
                .map_err(|e| ServiceError::Config(ConfigError::BadConfig(e.to_string())))?,
            kinematic_monitor: self.cfg.kinematic_monitor && (max_v.is_some() || max_d.is_some()),
            max_velocity_ips: max_v,
            max_displacement_in_pp: max_d,
            fault_debounce_ticks: self.cfg.fault_debounce_ticks,
            // The SERVICE-SIDE collapse fraction, threaded explicitly. It is
            // deliberately WIDER than the servo's own freeze fraction
            // (`fault_fraction_effective()`) so the debounced service-side
            // servo-fault abort - not the servo's silent internal freeze - owns
            // the fault decision. See `fault_fraction_effective` for the pairing.
            fault_fraction: self.cfg.service_fault_fraction(),
            settle_ticks: self.settle_ticks(),
            scalar_abort_debounce: 3,
            start_f_hz: start_f,
            start_level_scale: start_scale,
            run_mode: self.run_mode,
            // Retard the tracking filters' demodulation reference by
            // the lag the sysID actually MEASURED on this plant. Zero on
            // HERTZFORGE-SIM (the Sim plant peaks at lag 0, which is why the FRF
            // alignment is a no-op there), so every Sim fixture runs the exact
            // pre-fix arithmetic and this is a hardware-only behaviour change.
            tracking_lag_frames: self.sysid_lag_frames,
            ring_blocks: self.ring_blocks(),
            block_frames: self
                .configured
                .map(|a| a.block_frames)
                .unwrap_or(self.block_frames)
                .max(1),
        };
        let (step, handles) = SineProcessStep::new(build);
        self.step = Some(step);
        self.handles = Some(handles);
        Ok(())
    }

    /// Build the scheduler state from the mode config, ON THE ADOPTED BAND.
    ///
    /// `self.band` is the config's band NARROWED against the device's real
    /// Nyquist limit at adoption. The scheduler must be built from that, not from
    /// `cfg.mode`'s copy: the scheduler is the frequency AUTHORITY (its clamp is
    /// what `freq_hz()`, the NCO and the status echo all follow), so a scheduler
    /// carrying the unclamped band would sweep straight through the limit and
    /// reach `try_set_frequency` with a refusal - a fault, on an ordinary sweep,
    /// for a band the operator was already told had been clamped.
    fn build_scheduler(&self) -> Result<SchedulerBuild, ServiceError> {
        let bad = |e: crate::control::ControlError| {
            ServiceError::Config(ConfigError::BadConfig(e.to_string()))
        };
        let band = self.band;
        Ok(match &self.cfg.mode {
            SineMode::Sweep(c) => {
                let mut c = *c;
                c.band = band;
                c.start_hz = crate::scheduler::clamp_to_band(c.start_hz, band);
                SchedulerBuild::Sweep(BidirSweep::new(c).map_err(bad)?)
            }
            SineMode::Dwell(p) => {
                let mut p = *p;
                p.freq_hz = crate::scheduler::clamp_to_band(p.freq_hz, band);
                SchedulerBuild::Dwell(DwellRun::new(p).map_err(bad)?)
            }
            SineMode::StepSine(c) => {
                let mut c = *c;
                c.band = band;
                SchedulerBuild::StepSine(StepSine::new(c).map_err(bad)?)
            }
            SineMode::Rstd(c) => {
                let mut c = *c;
                c.band = band;
                SchedulerBuild::Rstd(RstdPlan::new(c).map_err(bad)?)
            }
        })
    }

    /// The starting frequency for the run mode, on the ADOPTED band (see
    /// [`SineService::build_scheduler`] - the seed drive scale and the step's
    /// start frequency must be the same number the scheduler was built with).
    fn start_freq(&self) -> f64 {
        let f = match &self.cfg.mode {
            SineMode::Sweep(c) => c.start_hz,
            SineMode::Dwell(p) => p.freq_hz,
            SineMode::StepSine(c) => c.band.0,
            SineMode::Rstd(c) => c.band.0,
        };
        crate::scheduler::clamp_to_band(f, self.band)
    }

    /// The seed drive scale from the feedforward table at the start frequency (so
    /// the loop starts near the plant-inverse prediction), falling back to a
    /// small default when untrusted.
    fn seed_drive_scale(&self) -> f64 {
        let f = self.start_freq();
        if let Some(ff) = self.feedforward.as_ref() {
            if let Some(db) = ff.ff_db(f) {
                return 10f64.powf(db / 20.0).clamp(1e-4, 1e4);
            }
        }
        0.05
    }

    /// The level-settle window in control ticks: enough for the startup ramp to
    /// bring the response onto the band before the scalar abort / kinematic /
    /// servo-fault arm. Sized to a few servo settling times: the servo climbs at
    /// the compression `max_rate` dB/s and may need ~20 dB from the seed, so
    /// ~1 s of settle; converted to ticks via the control-tick period. Bounded so
    /// a very short test does not spend its whole life settling.
    fn settle_ticks(&self) -> u32 {
        let (_, max_rate) = self.cfg.compression.gains();
        let tick_dt = self.cfg.control_tick_frames as f64 / self.sample_rate_hz;
        // Time to ramp the full startup gap at max_rate, plus a tracking-filter
        // settle margin. The startup drive can sit ~40 dB below the level that
        // puts the response on target (the seed is deliberately conservative), so
        // budget the full 40 dB climb so the protections do not arm mid-ramp.
        let ramp_s = 40.0 / max_rate.max(1.0);
        let settle_s = ramp_s + 0.3;
        ((settle_s / tick_dt.max(1e-9)).ceil() as u32).clamp(4, 20_000)
    }

    /// The kinematic monitor bounds from the shaker record (velocity in/s, pk-pk
    /// displacement in).
    fn kinematic_bounds(&self) -> (Option<f64>, Option<f64>) {
        let Some(shaker) = self.cfg.shaker.as_ref() else {
            return (None, None);
        };
        let v = shaker.max_velocity_ips.map(|r: RatingValue| r.value);
        let d = shaker.max_displacement_in_pp.map(|r: RatingValue| r.value);
        (v, d)
    }

    // --- the run loop (chunked Engine::run) ---------------------------

    /// Run the whole AUTO ladder to completion in chunks: each chunk is
    /// `chunk_ticks` engine ticks (frames), re-arming the Sim between chunks;
    /// between chunks the service drains status, checks the abort latch + stop
    /// flag + scheduler/ladder completion, and advances the state machine.
    /// `max_chunks` bounds the run. Returns the final state.
    pub fn run_auto(
        &mut self,
        pretest_frames: usize,
        max_chunks: u64,
    ) -> Result<RunState, ServiceError> {
        self.assess_feasibility()?;
        if matches!(self.state, RunState::FeasibilityBlocked) {
            return Ok(self.state);
        }
        self.arm()?;
        self.run_sysid(pretest_frames)?;
        if !self.coherence_gate()? {
            return Ok(self.state);
        }
        // THE REVIEW PARK IS HONORED HERE, not released. `run_auto` is the
        // unattended entry point and it would be one line to auto-proceed - which
        // is exactly why it does not: the hold would then be a no-op for every
        // caller that used this door, silently, and the config flag would still
        // read `true`. A caller that wants an unattended run says so in the config
        // (`review_hold: false`); one that parks here gets the state back and
        // calls `proceed_after_review`.
        if matches!(self.state, RunState::AwaitingReview) {
            return Ok(self.state);
        }
        if matches!(self.cfg.capture.rule, CaptureRule::EntireTest) {
            let _ = self.start_capture("auto_entire_test");
        }
        let mut chunks = 0u64;
        while matches!(self.state, RunState::Engaged | RunState::FullLevel) && chunks < max_chunks {
            if !self.run_chunk()? {
                break;
            }
            chunks += 1;
        }
        if matches!(self.state, RunState::Engaged | RunState::FullLevel) {
            // Safety cap hit; stop cleanly.
            self.stop()?;
        }
        Ok(self.state)
    }

    /// Run ONE chunk - `CHUNK_SECONDS` of TEST TIME - and service the state
    /// machine afterwards. Returns `false` when the run stopped/aborted.
    ///
    /// # The external contract is unchanged; the inside is not
    ///
    /// A chunk still means "0.25 s of test time", which is what every caller
    /// pacing this loop assumes. What it no longer means is one `Engine::run`.
    ///
    /// `Engine::run`'s epilogue STOPS THE BACKEND. On the Sim that is invisible
    /// (arm/start are no-ops on a running plant); on the chassis it is a taper, a
    /// TASK_UNRESERVE and a silence re-prefill at every boundary - and the
    /// boundary was not 0.25 s: `chunk_ticks` counted FRAMES into a parameter
    /// measured in BLOCKS, so at a 1024-frame block one "chunk" was 12 800 blocks,
    /// about **256 seconds** of test time. Two defects in one line: a periodic
    /// transient into the UUT at a four-minute cadence, and a Stop
    /// that could go unhonored until the next boundary.
    ///
    /// A chunk is now a number of BOUNDED `run_ticks` batches over ONE
    /// continuously running backend, sized from the block so the TIME comes out
    /// right at any rate.
    pub fn run_chunk(&mut self) -> Result<bool, ServiceError> {
        if !matches!(self.state, RunState::Engaged | RunState::FullLevel) {
            return Err(ServiceError::BadState(format!(
                "run_chunk from {:?}",
                self.state
            )));
        }
        let alive = self.run_batches(self.batches_per_chunk())?;

        // Drain the status snapshot.
        if let Some(h) = self.handles.as_ref() {
            self.last_snapshot = h.status.read();
        }

        self.service_capture();

        // Surface scheduler events (RSTD, alarm) into the log.
        self.surface_events();

        if !alive {
            return Ok(false);
        }

        // Advance the state machine: scheduler / ladder completion.
        self.advance_state_between_chunks()?;
        Ok(matches!(
            self.state,
            RunState::Engaged | RunState::FullLevel
        ))
    }

    /// Batches that fill `CHUNK_SECONDS` of TEST TIME at the adopted rate.
    ///
    /// The arithmetic is the whole point of the conversion: frames of time,
    /// divided by frames per batch (`TICKS_PER_BATCH` ticks x the ACTUAL block),
    /// rounded up and floored at one. Denominated in the device's answer on both
    /// sides, so a chunk is the same DURATION on a 16 kHz Sim and a 51.2 kHz
    /// chassis - which the old `max_ticks` form was not.
    fn batches_per_chunk(&self) -> u32 {
        let block = self
            .configured
            .map(|a| a.block_frames)
            .unwrap_or(self.block_frames)
            .max(1);
        let per_batch = (TICKS_PER_BATCH as usize * block).max(1) as f64;
        let frames = (self.sample_rate_hz * CHUNK_SECONDS).max(MIN_CHUNK_FRAMES);
        ((frames / per_batch).ceil() as u32).max(1)
    }

    /// Run up to `batches` BOUNDED tick batches over the persistent engine,
    /// servicing the stop / abort state between them. Returns `true` while the run
    /// may continue.
    ///
    /// Called BY the caller's pump (`run_chunk`, or a host's own loop) - it never
    /// spawns a thread, so the caller owns the cadence and the shutdown and can
    /// interleave [`SineService::service_analysis_tapped`] between batches.
    pub fn run_batches(&mut self, batches: u32) -> Result<bool, ServiceError> {
        if !matches!(self.state, RunState::Engaged | RunState::FullLevel) {
            return Ok(false);
        }
        self.ensure_engine()?;
        for _ in 0..batches {
            // 1. THE ABORT LATCH, first: a trip set on the last batch's control
            //    tick has already started the drive ramping, and everything below
            //    would otherwise run one more batch against a faulted run.
            if let Some(h) = self.handles.as_ref() {
                if h.abort.is_tripped() {
                    let record = h.abort.take_record();
                    self.finish_abort(record);
                    return Ok(false);
                }
            }

            // 2. THE TWO-PHASE STOP. Phase one is the RAMP, which runs on the
            //    control tick and is already in flight by the time the flag is
            //    visible here. Phase two is THIS: the tasks are stopped only once
            //    every output is at zero AND the device has actually played out
            //    what it was given. Stopping earlier freezes the output at
            //    whatever the ramp had reached, because the device holds its last
            //    sample - the same defect as a cliff, with a nicer-looking buffer.
            let stopping = self
                .handles
                .as_ref()
                .map(|h| h.stop_requested.load(Ordering::Acquire))
                .unwrap_or(false);
            if stopping && self.drive_is_zero() {
                self.finish(
                    RunState::Stopped,
                    "operator stop: drive ramped to zero, generation drained, tasks stopped".into(),
                );
                return Ok(false);
            }

            if !self.run_one_batch()? {
                return Ok(false);
            }
        }
        Ok(matches!(
            self.state,
            RunState::Engaged | RunState::FullLevel
        ))
    }

    /// EVERY drive output is at zero (ch0 and, when COLA is on, the aux).
    fn drive_is_zero(&self) -> bool {
        self.step
            .as_ref()
            .map(|s| s.drive_is_zero())
            .unwrap_or(true)
    }

    /// Build the persistent engine if it does not exist yet.
    ///
    /// Built ONCE per run so its `OutputDelivery` (the drive lookahead)
    /// PERSISTS across batches. Rebuilding it re-primes the delivery with
    /// `min_lookahead` ZERO frames and discards the same number of computed
    /// frames - a zero-drive notch plus an amplitude discontinuity at every
    /// boundary.
    fn ensure_engine(&mut self) -> Result<(), ServiceError> {
        use hertzforge_engine::{Engine, EngineConfig};
        if self.engine.is_some() {
            return Ok(());
        }
        let out_channels = self.cfg.out_channels();
        let n_in = if self.configured.is_some() {
            self.n_input_slots()
        } else {
            1
        };
        // The block cadence (and with it the ring depth and the scheduling-health
        // budget) comes from what the backend ACTUALLY configured.
        let actual = self
            .configured
            .ok_or_else(|| ServiceError::BadState("engine batch before configure()".into()))?;
        let mut ecfg = EngineConfig::with_block(n_in, out_channels, &actual);
        // The sine drive is a clean sine, not peaky random; but the accel-axis
        // watchdog is spike-sensitive by default. Keep a modest sustain so a
        // single noisy sample does not false-trip.
        //
        // The frame count is a TIME only against a rate. HERTZFORGE-SIM
        // keeps the literal; hardware converts a stated quarter-millisecond at the
        // rate the driver actually programmed.
        ecfg.watchdog_sustain = if self.station.is_hardware() {
            hertzforge_engine::watchdog_sustain_frames(
                WATCHDOG_SUSTAIN_SECONDS,
                actual.actual_rate_hz,
            )
        } else {
            SIM_WATCHDOG_SUSTAIN_FRAMES
        };
        ecfg.jitter_frac = self.jitter_frac;
        ecfg.compute_budget = self.compute_budget;
        let backend = self
            .backend
            .as_ref()
            .ok_or_else(|| ServiceError::BadState("not armed".into()))?;
        self.engine = Some(Engine::new(ecfg, &self.table, backend));
        Ok(())
    }

    /// ONE bounded batch of `run_ticks`, under `catch_unwind`.
    ///
    /// # Why the whole body is caught
    ///
    /// A panic anywhere under `run_ticks` unwinds past every stop path in this
    /// file while the AO task keeps clocking whatever sample it last had. A drive
    /// frozen at its last value is the one failure mode a stopped host cannot
    /// correct, and nothing else in the process is in a position to notice. The
    /// panic is NOT swallowed: `emergency_stop` is issued, the run is terminal,
    /// and there is no resume path.
    fn run_one_batch(&mut self) -> Result<bool, ServiceError> {
        use hertzforge_engine::{StopReason, VecCaptureSink};
        let station = self.station;
        let (Some(backend), Some(engine), Some(step)) = (
            self.backend.as_mut(),
            self.engine.as_mut(),
            self.step.as_mut(),
        ) else {
            return Err(ServiceError::BadState("not armed".into()));
        };
        let mut sink = VecCaptureSink::new();
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine.run_ticks(backend.as_mut(), step, &mut sink, TICKS_PER_BATCH)
        }));
        let report = match caught {
            Ok(r) => r,
            Err(payload) => {
                let what = panic_message(&payload);
                self.emergency_stop_guarded();
                self.finish(
                    RunState::Aborted,
                    format!("pump PANICKED: {what}; emergency_stop issued"),
                );
                return Err(ServiceError::Plant(format!(
                    "sine pump panicked ({what}); emergency_stop was issued and the run is \
                     aborted"
                )));
            }
        };
        self.tick_count += report.ticks;

        // DRAIN the driver's own diagnostics
        // every batch. `take_warnings` empties the backend's buffer, so a caller
        // that does not pull them is not leaving them for later - it is discarding
        // them. Drained BEFORE any fault arm below can return early: a batch that
        // FAULTED is exactly the one whose driver warnings matter most.
        self.drain_driver_warnings();

        match report.reason {
            // THE BATCH BUDGET RAN OUT: the only healthy exit from a bounded batch
            // on a continuous acquisition. `InputEnded` here means `max_ticks`, not
            // a stream that finished - and the pre-conversion `_ => break` arm
            // treated it as a run end, which post-conversion would end every run
            // after one batch.
            StopReason::InputEnded => Ok(true),
            StopReason::AbortTripped(trip) => {
                // The engine's raw-ring watchdog already called `emergency_stop`.
                // Latch the typed trip so the run ends through the SAME
                // `finish_abort` path as the scalar / kinematic / drive aborts -
                // which is what carries the backend stop on this third terminal
                // path.
                // THE RECORD QUOTES THE BREACHING FRAME, NOT THE BATCH END.
                // `AbortTrip.at_frame` was being DISCARDED here and the frequency +
                // level re-read off the live step, which answers "what is the
                // scheduler commanding now" - a different question, and by up to a
                // block's worth of control ticks a different answer. On the chassis
                // tracking-lag trips it put the recorded level on the wrong ladder
                // rung (-12 dB against a journal that shows 0 dB drive), and that
                // wrong rung is what made the trips look "level-independent" and
                // sent the investigation after a phantom electrical feature.
                let ctx = self
                    .step
                    .as_ref()
                    .map(|s| s.context_at_frame(trip.at_frame))
                    .unwrap_or_default();
                let record = TripRecord {
                    kind: TripKind::WatchdogAccel {
                        channel_index: trip.channel_index,
                        axis: format!("{:?}", trip.axis).to_lowercase(),
                        value: trip.value,
                        limit: trip.limit,
                        at_frame: trip.at_frame,
                        measured: ctx.measured,
                        drive_scale: ctx.drive_scale,
                    },
                    at_freq_hz: ctx.freq_hz,
                    at_level_db: ctx.level_db,
                    at_tick: self.tick_count,
                };
                self.finish_abort(Some(record));
                Ok(false)
            }
            StopReason::SchedulingFault(status) => {
                // NO RETRY, on any station. The pre-conversion loop allowed one on
                // HERTZFORGE-SIM when the chunk had made zero progress; a bounded batch
                // over a PERSISTENT backend has no such thing as a chunk that can
                // be re-run, and on hardware the engine has already issued
                // `emergency_stop` while a shaker was moving.
                self.finish(
                    RunState::Aborted,
                    format!(
                        "scheduling fault ({status:?}) on station {station}, {} frame(s) \
                         acquired; the engine already called emergency_stop",
                        report.frames_in
                    ),
                );
                Err(ServiceError::Plant(format!(
                    "scheduling fault ({status:?}) on station {station}"
                )))
            }
            StopReason::InputStalled { elapsed } => {
                self.finish(
                    RunState::Aborted,
                    format!("input stalled: no frames for {elapsed:?}"),
                );
                Err(ServiceError::Plant(format!(
                    "input stalled: no frames for {elapsed:?}"
                )))
            }
            StopReason::Disconnected(m) => {
                self.finish(RunState::Aborted, format!("device link lost: {m}"));
                Err(ServiceError::Plant(format!(
                    "device link lost mid-run: {m}"
                )))
            }
            StopReason::DriveUnderrun(u) => {
                self.emergency_stop_guarded();
                self.finish(RunState::Aborted, format!("drive underrun: {u:?}"));
                Err(ServiceError::Plant(format!("drive underrun abort: {u:?}")))
            }
            StopReason::RingOverrun { frames } => {
                self.finish(
                    RunState::Aborted,
                    format!("raw ring overran by {frames} frame(s) on a limits-armed run"),
                );
                Err(ServiceError::Plant(format!(
                    "raw ring overrun: {frames} frame(s) the abort watchdog never examined"
                )))
            }
            StopReason::BackendError(m) => {
                self.emergency_stop_guarded();
                self.finish(RunState::Aborted, format!("backend error: {m}"));
                Err(ServiceError::Plant(format!("engine backend error: {m}")))
            }
            StopReason::CaptureFatal(m) => {
                self.emergency_stop_guarded();
                self.finish(RunState::Aborted, format!("capture fatal: {m}"));
                Err(ServiceError::Plant(format!("capture fatal: {m}")))
            }
        }
    }

    fn drain_driver_warnings(&mut self) {
        if let Some(b) = self.backend.as_mut() {
            for w in b.take_warnings() {
                self.log.push(EventKind::Note {
                    text: format!("driver: {w}"),
                });
            }
        }
    }

    /// Advance the state machine between chunks: Engaged -> FullLevel when the
    /// ladder reaches its full rung; FullLevel -> Stopped when the scheduler's
    /// completion target is met. Records level history on rung changes.
    fn advance_state_between_chunks(&mut self) -> Result<(), ServiceError> {
        let (ladder_done, scheduler_done, level_db, sweeps) = {
            let step = self.step.as_ref().unwrap();
            (
                step.ladder_done(),
                step.scheduler_done(),
                step.level_db(),
                step.sweeps_done(),
            )
        };
        // Record a level change if the ladder advanced since last snapshot.
        let last_level = self.level_history.last().map(|e| e.level_db);
        if let Some(last_db) = last_level {
            if (last_db - level_db).abs() > 1e-9 {
                self.level_history.push(LevelHistoryEntry {
                    level_db,
                    auto: true,
                    at_tick: self.tick_count,
                });
                self.log.push(EventKind::LevelChange {
                    from_db: last_db,
                    to_db: level_db,
                    auto: true,
                });
            }
        }
        let _ = sweeps;
        // FullLevel when the ladder finishes.
        if ladder_done && matches!(self.state, RunState::Engaged) {
            self.transition(RunState::FullLevel);
        }
        // Stopped when the scheduler completes (the mode's target met).
        if scheduler_done {
            self.stop()?;
        }
        Ok(())
    }

    /// Surface scheduler-side events (RSTD, scalar alarm) into the event log from
    /// the step's getters + snapshot.
    fn surface_events(&mut self) {
        let (est, setpoint, alarm, freq, measured, level) = {
            let step = self.step.as_ref().unwrap();
            (
                step.rstd_estimate(),
                step.rstd_setpoint_rad(),
                self.last_snapshot.alarm_active,
                self.last_snapshot.freq_hz,
                self.last_snapshot.measured_g,
                self.last_snapshot.level_db,
            )
        };
        // RSTD (log once each), INCREMENTALLY. These were "cheap linear scans; the
        // log is small" - and the log is not small any more. It grows for the life
        // of the run, this method runs once per chunk, and post-conversion a chunk
        // is 0.25 s of test time rather than 256 s: on a twenty-minute soak that is
        // ~4800 full scans of a log with thousands of entries, on the thread that
        // owns the drive clock. Two bools say the same thing in O(1) and cannot
        // drift, because the only thing that sets them is the push beside them.
        if let Some(e) = est {
            if !self.surfaced_rstd_search {
                self.surfaced_rstd_search = true;
                self.log.push(EventKind::RstdSearchDone {
                    f_peak: e.f_peak,
                    q: e.q,
                });
            }
        }
        if let Some(p) = setpoint {
            if !self.surfaced_rstd_setpoint {
                self.surfaced_rstd_setpoint = true;
                self.log
                    .push(EventKind::RstdSetpointCaptured { phase_rad: p });
            }
        }
        if alarm {
            let rung_scale = 10f64.powf(level / 20.0);
            let (side, limit) = self
                .cfg
                .profile
                .alarm_bounds(freq)
                .map(|(lo, hi)| {
                    if measured > hi * rung_scale {
                        ("high".to_string(), hi * rung_scale)
                    } else {
                        ("low".to_string(), lo * rung_scale)
                    }
                })
                .unwrap_or_else(|| ("high".to_string(), 0.0));
            self.log.push(EventKind::ScalarAlarm {
                side,
                freq_hz: freq,
                measured_g: measured,
                limit_g: limit,
            });
        }
    }

    /// Finalize an abort: record the trip, stop capture, transition to Aborted.
    fn finish_abort(&mut self, record: Option<TripRecord>) {
        let reason = match record {
            Some(r) => {
                let text = format!("protection trip: {:?}", r.kind);
                self.log.push(EventKind::Trip { record: r.clone() });
                self.trip = Some(r);
                text
            }
            None => "abort with no trip record".into(),
        };
        // THE ABORT PATH REACHES THE BACKEND. Before the conversion it did not:
        // `finish_abort` recorded the trip, stopped capture, transitioned to
        // Aborted, and left the device running - the backend stop was
        // `Engine::run`'s epilogue, which a control-tick trip never went through.
        // `finish` is the one helper that carries it on all three terminal paths.
        self.finish(RunState::Aborted, reason);
    }

    /// THE ONE TERMINAL PATH. Stop capture, take the drive off the device, record
    /// the reason, transition.
    ///
    /// # Why one helper
    ///
    /// Converting `Engine::run` to `run_ticks` deletes Sine's ONLY backend stop -
    /// `run`'s epilogue - and there are THREE paths that needed it, not two:
    /// `stop()`, `finish_abort()` (reached from control-tick trips: DriveVpk,
    /// scalar, kinematic, servo-fault; none of which ever touched the backend),
    /// and the engine-watchdog `AbortTripped` path. Three call sites, one helper,
    /// so a fourth terminal path cannot be added without inheriting the stop.
    ///
    /// ABORTED takes `emergency_stop` (the backend's own 25 ms taper); everything
    /// else takes the ramped `stop()`, preceded by a GENERATION DRAIN.
    fn finish(&mut self, state: RunState, reason: String) {
        let _ = self.stop_capture_inner();
        if state == RunState::Aborted {
            self.emergency_stop_guarded();
        } else {
            self.drain_generation();
            if let Some(b) = self.backend.as_mut() {
                if let Err(e) = b.stop() {
                    self.log.push(EventKind::Note {
                        text: format!("backend stop reported {e}"),
                    });
                }
            }
        }
        self.log.push(EventKind::Note {
            text: format!("run ended: {reason}"),
        });
        // The SAME sentence the log just took, kept where a host can read it
        // without walking the event log. Set on every terminal path because this
        // is the one terminal helper - a seventh path cannot be added without
        // inheriting it, which is the same argument that put the backend stop
        // here.
        self.end_reason = Some(reason);
        if self.state != RunState::Aborted || state == RunState::Aborted {
            self.transition(state);
        }
    }

    /// `Backend::emergency_stop` with a NESTED `catch_unwind`.
    ///
    /// This is called from inside the pump's panic handler among other places, and
    /// a panic while panicking is a process ABORT. That is the one moment in the
    /// run where the process dying means the 9263 holds its last sample with
    /// nobody left to clear it, so the call that exists to prevent exactly that
    /// must not be able to cause it.
    fn emergency_stop_guarded(&mut self) {
        let backend = match self.backend.as_mut() {
            Some(b) => b,
            None => return,
        };
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| backend.emergency_stop()));
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(e)) => self.log.push(EventKind::Note {
                text: format!("emergency_stop reported {e}"),
            }),
            Err(_) => self.log.push(EventKind::Note {
                text: "emergency_stop PANICKED; the nested catch_unwind kept the process \
                       alive so this could be recorded. Treat the device as still driving \
                       and use the physical E-stop."
                    .into(),
            }),
        }
    }

    /// WAIT for what the device has already been given to actually reach the pin,
    /// before the tasks are stopped.
    ///
    /// The drive scale being zero means the HOST has stopped generating; it says
    /// nothing about the frames already sitting in the device's output buffer. A
    /// task stopped with drive still queued freezes the output at whatever the
    /// device had reached, because the hardware holds its last sample.
    ///
    /// **Do NOT rely on `NiBackend::stop` for this.** It does taper and it does
    /// wait for generation to catch up - but only inside `ramp_drive_to_zero`,
    /// which EARLY-RETURNS when the last accepted frame was all zeros
    /// (`backend.rs`). Which is precisely the state a correctly ramped stop hands
    /// it. The guard that makes the backend's own wait unnecessary is the same
    /// guard that skips it, so the wait belongs here.
    ///
    /// The signal is `output_space()`: the device's free buffer space stops
    /// growing when there is nothing left to play. Bounded in TIME - a device that
    /// never drains is a device that has stopped generating, and waiting longer
    /// does not fix it.
    fn drain_generation(&mut self) {
        let deadline = std::time::Instant::now() + DRAIN_DEADLINE;
        let Some(backend) = self.backend.as_mut() else {
            return;
        };
        let mut best = match backend.output_space() {
            Some(v) => v,
            // A backend with no independent output buffer (the lockstep Sim) has
            // nothing to drain: `write_drive` accepted everything the moment it
            // was offered.
            None => return,
        };
        let mut stable = 0u32;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(DRAIN_POLL);
            let Some(space) = backend.output_space() else {
                return;
            };
            if space > best {
                best = space;
                stable = 0;
            } else {
                stable += 1;
                // Three consecutive polls with no further space freed: the buffer
                // has stopped draining, which for a device that was generating
                // means it is empty.
                if stable >= 3 {
                    return;
                }
            }
        }
        self.log.push(EventKind::Note {
            text: format!(
                "generation drain did not settle within {DRAIN_DEADLINE:?}; stopping the \
                 tasks anyway. The device may hold the last sample it played."
            ),
        });
    }

    // --- operator transport (posts into the step mailbox) -----------------------

    /// Post an operator command into the step's lock-free mailbox.
    fn post(&self, cmd: Command) -> Result<(), ServiceError> {
        match self.handles.as_ref() {
            Some(h) => {
                h.mailbox.post(cmd);
                Ok(())
            }
            None => Err(ServiceError::BadState("not engaged (no step)".into())),
        }
    }

    /// Request a clean stop (honored within one chunk).
    pub fn request_stop(&self) -> Result<(), ServiceError> {
        self.post(Command::Stop)
    }
    /// Hold the sweep/dwell at the current frequency.
    pub fn hold(&self) -> Result<(), ServiceError> {
        self.post(Command::Hold)
    }
    /// Resume from a hold.
    pub fn resume(&self) -> Result<(), ServiceError> {
        self.post(Command::Resume)
    }
    /// Reverse the sweep direction.
    pub fn reverse(&self) -> Result<(), ServiceError> {
        self.post(Command::Reverse)
    }
    /// The mode name for a frequency-intent refusal.
    ///
    /// Only [`SineMode::Sweep`] owns a movable tone. A dwell sits at one
    /// frequency by definition; step-sine's tone is the ladder it is walking; the
    /// RSTD tracker owns its own frequency and an operator nudge would fight the
    /// phase loop. Those are not "not implemented yet", they are the mode, so the
    /// refusal NAMES it (a silent no-op is a control that lies).
    fn mode_label(&self) -> &'static str {
        match &self.cfg.mode {
            SineMode::Sweep(_) => "sweep",
            SineMode::Dwell(_) => "dwell",
            SineMode::StepSine(_) => "step-sine",
            SineMode::Rstd(_) => "RSTD",
        }
    }

    fn require_sweep(&self, verb: &str) -> Result<(), ServiceError> {
        if matches!(self.cfg.mode, SineMode::Sweep(_)) {
            return Ok(());
        }
        Err(ServiceError::BadState(format!(
            "{verb} is refused in {} mode: only a sweep has a tone the operator moves. \
             (A dwell holds one frequency by definition, step-sine's tone IS its ladder, \
             and RSTD's phase tracker owns its own.)",
            self.mode_label()
        )))
    }

    /// Jog the frequency by `delta_hz`. Sweep mode only.
    ///
    /// Returns the frequency the run will adopt - the CLAMPED value, from the one
    /// clamp expression the scheduler also applies
    /// ([`crate::scheduler::clamp_to_band`]), so the echo cannot disagree with
    /// the NCO. The move itself happens at the next control tick.
    pub fn jog(&self, delta_hz: f64) -> Result<f64, ServiceError> {
        self.require_sweep("jog")?;
        let now = self
            .step
            .as_ref()
            .map(|s| s.freq_hz())
            .unwrap_or(self.band.0);
        let applied = crate::scheduler::clamp_to_band(now + delta_hz, self.band);
        self.post(Command::Jog(delta_hz))?;
        Ok(applied)
    }
    /// Command an ABSOLUTE frequency. Sweep mode only. Returns the clamped value
    /// actually adopted (the echo).
    pub fn go_to(&self, hz: f64) -> Result<f64, ServiceError> {
        self.require_sweep("go_to")?;
        let applied = crate::scheduler::clamp_to_band(hz, self.band);
        self.post(Command::GoTo(hz))?;
        Ok(applied)
    }
    /// Manual: step the ladder level by `delta_db`.
    pub fn step_level(&self, delta_db: f64) -> Result<(), ServiceError> {
        self.post(Command::StepLevel(delta_db))
    }
    /// Manual: jump to ladder rung `i`.
    pub fn jump_to_level(&self, i: usize) -> Result<(), ServiceError> {
        self.post(Command::JumpLevel(i))
    }
    /// Command a level in dB, resolved to the NEAREST ladder rung. Returns the
    /// rung level actually applied (the echo), never the request.
    ///
    /// Out-of-table requests land on the end rung they are nearest, which is the
    /// clamp - expressed by the same nearest-rung arithmetic rather than a
    /// separate branch that could disagree with it. The resolution lives in
    /// [`crate::scheduler::LadderPlan::nearest_rung`] and both this echo and the
    /// ladder's own move read it, so the panel and the drive cannot diverge.
    pub fn set_level_db(&self, db: f64) -> Result<f64, ServiceError> {
        let (_, applied) =
            crate::scheduler::LadderPlan::nearest_rung(&self.cfg.ladder.table_db, db).ok_or_else(
                || {
                    ServiceError::Config(ConfigError::BadConfig(format!(
                        "cannot resolve a level of {db} dB: the ladder table is empty or the \
                         request is not a number"
                    )))
                },
            )?;
        self.post(Command::SetLevelDb(db))?;
        Ok(applied)
    }
    /// Set AUTO / MANUAL.
    ///
    /// Legal BEFORE the step exists, which is the point: a MANUAL run is chosen
    /// at Start and the step is not built until the coherence gate releases, so a
    /// mode that could only be posted into the mailbox would arrive after the
    /// first rung rule had already been evaluated. The choice is stored and
    /// carried into the build; when a step IS live it is also posted so the change
    /// takes effect on the next control tick.
    pub fn set_run_mode(&mut self, mode: RunMode) -> Result<(), ServiceError> {
        self.run_mode = mode;
        if self.step.is_some() {
            self.post(Command::SetRunMode(mode))?;
        }
        self.log.push(EventKind::Note {
            text: format!("run mode set to {mode:?}"),
        });
        Ok(())
    }
    /// The run mode currently selected.
    pub fn run_mode(&self) -> RunMode {
        self.run_mode
    }
    /// Set the blend weight `w`.
    pub fn set_w(&self, w: f64) -> Result<(), ServiceError> {
        self.post(Command::SetW(w))
    }

    // --- capture ----------------------------------------------------------------

    /// Start streaming telemetry capture to `data_root/<run_name>/`.
    pub fn start_capture(&mut self, run_name: &str) -> Result<PathBuf, ServiceError> {
        if let Some(sink) = self.capture.as_ref() {
            return Ok(sink.run_dir().to_path_buf());
        }
        // THE CAPTURE'S RATE IS ONLY HONEST AFTER ARM. `CaptureConfig::at_rate`
        // stamps `self.sample_rate_hz` into the sidecar, and that is the request
        // until `configure()` has answered - so a capture started before arm would
        // write a file whose header says 8192 Hz over samples the device clocked
        // at 8533.33. Every consumer of that file (the report's time axis, any
        // re-analysis) would be silently wrong, and nothing in the artifact would
        // show it. Refuse rather than stamp a rate that is not the run's.
        if self.configured.is_none() {
            return Err(ServiceError::BadState(
                "capture cannot start before arm: the sidecar stamps the sample rate, and \
                 until configure() has answered that number is the REQUEST, not what the \
                 device programmed. Arm first."
                    .into(),
            ));
        }
        let n_ch = if self.configured.is_some() {
            self.n_input_slots()
        } else {
            1
        };
        let cfg = CaptureConfig::at_rate(n_ch, self.sample_rate_hz);
        let settings_json =
            serde_json::to_string(&self.report_settings()).unwrap_or_else(|_| "{}".into());
        let sink = CaptureSink::start(
            &self.data_root,
            run_name,
            cfg,
            self.table.clone(),
            settings_json,
        )
        .map_err(|e| ServiceError::Io(e.to_string()))?;
        let run_dir = sink.run_dir().to_path_buf();
        self.log.push(EventKind::CaptureStart {
            run_dir: run_dir.display().to_string(),
            trigger: format!("{:?}", self.cfg.capture.rule),
        });
        self.capture = Some(sink);
        Ok(run_dir)
    }

    /// Stop the capture, flush the sidecar, log the drop count. No-op if not
    /// capturing. This is the DEFERRED-FINALIZE point: the testkit
    /// sink's real finish happens HERE at service stop, not per chunk.
    pub fn stop_capture(&mut self) -> Result<(), ServiceError> {
        self.stop_capture_inner()
    }

    fn stop_capture_inner(&mut self) -> Result<(), ServiceError> {
        if let Some(sink) = self.capture.take() {
            let written = sink.written();
            let dropped = sink.dropped();
            sink.finish().map_err(|e| ServiceError::Io(e.to_string()))?;
            self.log.push(EventKind::CaptureStop {
                chunks_written: written,
                dropped,
            });
        }
        Ok(())
    }

    /// Service the capture between chunks: the persistent step captured the raw
    /// frames into the engine's per-chunk VecCaptureSink; the disk streaming here
    /// is a snapshot-driven proxy on the Sim (the real per-block bridge lands with
    /// the direct-DAC backend). On Sim we periodically save a snapshot as the
    /// captured artifact so `EntireTest`/`Manual` capture produces a folder.
    fn service_capture(&mut self) {
        // THE INTERVAL IS AN INTERVAL. This read `periodic_save_s` only to check
        // that it was set and then saved a snapshot EVERY chunk - so a
        // `periodic_save_s: Some(30.0)` produced a file every 0.25 s of test time,
        // and post-conversion that is 25-33 file writes a second on the pump
        // thread. The setting was documented, configurable, and never once
        // consulted as a duration.
        //
        // The clock is the RUN's own test time (frames delivered / adopted rate),
        // not a wall clock: everything else in this service is paced that way, and
        // a snapshot cadence that drifted against the run it is sampling would put
        // two different times in one artifact.
        let Some(interval) = self.cfg.capture.periodic_save_s else {
            return;
        };
        if interval <= 0.0 || !interval.is_finite() || self.capture.is_none() {
            return;
        }
        let now_s = self.last_snapshot.elapsed_test_s;
        let due = match self.last_periodic_save_s {
            Some(last) => now_s - last >= interval,
            // The first save is due one full interval IN, not at t=0: a snapshot
            // taken before the run has produced anything records nothing.
            None => now_s >= interval,
        };
        if due {
            self.last_periodic_save_s = Some(now_s);
            let _ = self.save_snapshot(Vec::new());
        }
    }

    /// Save a periodic snapshot bundle: the current control state + the marks.
    pub fn save_snapshot(&mut self, marks: Vec<Mark>) -> Result<PathBuf, ServiceError> {
        let unix_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let run_dir = self
            .capture
            .as_ref()
            .map(|s| s.run_dir().to_path_buf())
            .unwrap_or_else(|| self.data_root.join("snapshots"));
        let settings_json =
            serde_json::to_string(&self.report_settings()).unwrap_or_else(|_| "{}".into());
        let s = &self.last_snapshot;
        let payload = SineSnapshotPayload {
            freq_hz: s.freq_hz,
            level_db: s.level_db,
            target_g: s.target_g,
            measured_g: s.measured_g,
            per_channel_tracked: s.per_channel_tracked.clone(),
            per_channel_broadband: s.per_channel_broadband.clone(),
            thd: s.thd,
            band_coherence: self.band_coherence,
            notch_binding: s.notch_binding,
        };
        let snapshot = Snapshot {
            unix_secs,
            settings_json,
            marks,
            payload,
        };
        let path =
            save_snapshot(&run_dir, &snapshot).map_err(|e| ServiceError::Io(e.to_string()))?;
        self.log.push(EventKind::Snapshot {
            path: path.display().to_string(),
        });
        Ok(path)
    }

    // --- stop -------------------------------------------------------------------

    /// Clean stop: finish any capture, ramp the drive down (the step handles the
    /// ramp via the Stop command), transition to Stopped (unless already
    /// Aborted).
    /// Clean stop. Idempotent, and legal from any state.
    ///
    /// # Two phases, and why the second one is here as well as in the pump
    ///
    /// Phase one is the RAMP: `Command::Stop` reaches the step at the next control
    /// tick and the drive starts decaying at [`engine::RAMP_DB_PER_S`]. Phase two
    /// is the backend stop, which must wait until every output is at zero AND the
    /// device has played out what it was given.
    ///
    /// When the caller is pumping (`run_chunk` / `run_batches`), phase two happens
    /// THERE, once the ramp has actually arrived - that is the good path and the
    /// one the drive continuity depends on. This method also carries a phase two
    /// for the caller who stops a run they are not pumping (the safety cap in
    /// `run_auto`, a host tearing down): it drives the ramp to completion itself
    /// rather than returning with the device still driving.
    pub fn stop(&mut self) -> Result<(), ServiceError> {
        if matches!(self.state, RunState::Stopped | RunState::Aborted) {
            return Ok(());
        }
        let _ = self.post(Command::Stop);
        // Ramp the drive down HERE if the caller is not going to pump it down.
        // The step's ramp only advances inside `process`, so a stop issued by a
        // non-pumping caller would otherwise leave the drive wherever it was.
        if self.step.is_some() && !self.drive_is_zero() {
            self.pump_ramp_to_zero();
        }
        self.finish(RunState::Stopped, "operator stop".into());
        Ok(())
    }

    /// ABORT: `emergency_stop` immediately, no ramp.
    ///
    /// The absence of a ramp is the point. A ramp is the right answer when a run
    /// is ENDING and the only question is how gracefully; an abort is the answer
    /// when something is already wrong, and the backend's own taper (the HAL
    /// contract's `RampDownThenAbort`, 25 ms on the NI path) is the backstop that
    /// keeps `emergency_stop` from being a cliff. Trading abort latency for
    /// smoothness at this layer is a tradeoff that belongs to the operator per
    /// rig, not to this code.
    ///
    /// Before the conversion Sine had no operator abort at all: the Tauri host's
    /// `abort` was `self.stop()`, so the panel's ABORT button issued a RAMP.
    pub fn abort(&mut self, reason: &str) -> Result<(), ServiceError> {
        if matches!(self.state, RunState::Stopped | RunState::Aborted) {
            return Ok(());
        }
        // Tell the step too, so the ramp starts even if something downstream keeps
        // pumping: an abort must never leave the step generating at level.
        if let Some(s) = self.step.as_mut() {
            s.begin_ramp_down();
        }
        self.finish(RunState::Aborted, format!("operator abort: {reason}"));
        Ok(())
    }

    /// Pump batches until the drive reaches zero, bounded by the ramp's own stated
    /// duration.
    ///
    /// The bound is [`engine::SineProcessStep::safe_stop_seconds`] from the drive
    /// guard's ceiling, plus a margin - i.e. the ramp's OWN arithmetic, not a
    /// number someone watched once. If the drive has not arrived by then the ramp
    /// law is not doing what it says, and continuing to pump would hide that.
    fn pump_ramp_to_zero(&mut self) {
        let budget_s = engine::SineProcessStep::safe_stop_seconds(self.cfg.drive_vpk_ceiling) * 2.0
            + CHUNK_SECONDS;
        let per_chunk = CHUNK_SECONDS.max(1e-6);
        let max_chunks = ((budget_s / per_chunk).ceil() as u32).clamp(1, 10_000);
        for _ in 0..max_chunks {
            if self.drive_is_zero() {
                return;
            }
            if self.run_batches(self.batches_per_chunk()).is_err() {
                return;
            }
            if !matches!(self.state, RunState::Engaged | RunState::FullLevel) {
                return;
            }
        }
        self.log.push(EventKind::Note {
            text: format!(
                "the stop ramp did not reach zero within {budget_s:.2} s (the law's own \
                 safe-stop budget at {RAMP_DB_PER_S} dB/s from the {:.3} drive ceiling); \
                 stopping anyway",
                self.cfg.drive_vpk_ceiling
            ),
        });
    }

    // --- report + status --------------------------------------------------------

    /// The report settings digest.
    fn report_settings(&self) -> RunSettings {
        let mode = match &self.cfg.mode {
            SineMode::Sweep(_) => "sweep",
            SineMode::Dwell(_) => "dwell",
            SineMode::StepSine(_) => "step",
            SineMode::Rstd(_) => "rstd",
        };
        RunSettings {
            mode: mode.into(),
            band_hz: self.band,
            control_indices: self.cfg.control.indices(),
            control_weights: self.cfg.control.weights(),
            composite_mode: CompositeModeRecord::from(self.cfg.control.mode),
            global_alarm_db: self.cfg.profile.global_alarm_db,
            global_abort_db: self.cfg.profile.global_abort_db,
            blend_w: self.cfg.blend_w,
            sysid_level_db: self.cfg.sysid_level_db,
            coherence_floor: self.cfg.coherence_floor,
            start_db: self.cfg.ladder.start_db,
            table_db: self.cfg.ladder.table_db.clone(),
            drive_vpk_ceiling: self.cfg.drive_vpk_ceiling,
            drive_vpk_abort: self.cfg.drive_vpk_abort,
            fault_debounce_ticks: self.cfg.fault_debounce_ticks,
            kinematic_monitor: self.cfg.kinematic_monitor,
            cola_enabled: self.cfg.cola.enabled,
        }
    }

    /// Assemble the serde-serializable report data.
    pub fn report_data(&self) -> report::ReportData {
        let control_indices = self.cfg.control.indices();
        let limit_indices: Vec<u32> = self
            .cfg
            .limit_profiles
            .iter()
            .map(|p| p.channel_index)
            .collect();
        let channels: Vec<ChannelRow> = self
            .table
            .channels
            .iter()
            .filter(|c| c.enabled)
            .map(|c| {
                let is_control = control_indices.contains(&c.index);
                let weight = self
                    .cfg
                    .control
                    .channels
                    .iter()
                    .find(|cc| cc.index == c.index)
                    .map(|cc| cc.weight);
                ChannelRow {
                    index: c.index,
                    role: format!("{:?}", c.role),
                    units: units_label(&c.units),
                    is_control,
                    control_weight: weight,
                    has_limit_profile: limit_indices.contains(&c.index),
                }
            })
            .collect();

        // Build the per-channel traces from the step's trace log.
        let channel_traces = self.channel_traces();

        report::ReportData {
            settings: self.report_settings(),
            channels,
            feasibility: self.feasibility.clone(),
            level_history: self.level_history.clone(),
            event_log: self.log.clone(),
            channel_traces,
            frf: self.frf.clone(),
            trip: self.trip.clone(),
            band_coherence: self.band_coherence,
            metadata: self
                .cfg
                .metadata
                .iter()
                .map(|m| (m.key.clone(), m.value.clone()))
                .collect(),
            honesty: Honesty::default(),
        }
    }

    /// The per-channel final swept traces, drained from the
    /// step's trace log. Only the CONTROL channels are surfaced.
    fn channel_traces(&self) -> Vec<ChannelTrace> {
        let Some(h) = self.handles.as_ref() else {
            return Vec::new();
        };
        let tracked = h.trace.tracked.lock().ok();
        let broadband = h.trace.broadband.lock().ok();
        let (Some(tracked), Some(broadband)) = (tracked, broadband) else {
            return Vec::new();
        };
        self.cfg
            .control
            .channels
            .iter()
            .zip(self.control_slots.iter())
            .map(|(cc, &slot)| {
                let t = tracked
                    .get(slot)
                    .map(|v| {
                        v.iter()
                            .map(|&(f, a)| TracePoint {
                                freq_hz: f,
                                value: a,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let b = broadband
                    .get(slot)
                    .map(|v| {
                        v.iter()
                            .map(|&(f, a)| TracePoint {
                                freq_hz: f,
                                value: a,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                ChannelTrace {
                    channel_index: cc.index,
                    tracked: t,
                    broadband: b,
                }
            })
            .collect()
    }

    /// The live status snapshot for the UI.
    pub fn status(&self) -> SineStatus {
        let s = &self.last_snapshot;
        let control_indices = self.cfg.control.indices();
        let overload_latches: Vec<(u32, bool)> = control_indices
            .iter()
            .zip(s.overload_latched.iter())
            .map(|(&idx, &l)| (idx, l))
            .collect();
        SineStatus {
            state: self.state,
            freq_hz: s.freq_hz,
            sweeps_done: s.sweeps_done,
            progress: self.progress(),
            level_db: s.level_db,
            target_g: s.target_g,
            measured_g: if s.measured_g > 0.0 {
                Some(s.measured_g)
            } else {
                None
            },
            per_channel_tracked: s.per_channel_tracked.clone(),
            per_channel_broadband: s.per_channel_broadband.clone(),
            thd: if s.thd.is_finite() { Some(s.thd) } else { None },
            band_coherence: self.band_coherence,
            notch_binding: s.notch_binding,
            alarm_active: s.alarm_active,
            abort_active: matches!(self.state, RunState::Aborted),
            capturing: self.capture.is_some(),
            elapsed_test_s: s.elapsed_test_s,
            remaining_test_s: None,
            overload_latches,
            trip: self.trip.clone(),
        }
    }

    /// A best-effort sweep/dwell progress fraction for the status, `[0, 1]`.
    ///
    /// PUBLIC because it is the only authority for the number. A remote front end has
    /// `elapsed_s` and `remaining_s` and could divide them, and the answer would be
    /// wrong in exactly the cases that matter: a hold pauses test time, a rung's dwell
    /// is not the sweep's, and the two ends would then disagree silently. The daemon
    /// mirrors THIS into its lock-free status path rather than letting the display
    /// invent a second one. `None` = no determinable end (a dwell, a step run, a
    /// free-running schedule), which renders as no progress bar rather than as 0%.
    pub fn progress(&self) -> Option<f64> {
        self.progress_fraction()
    }

    fn progress_fraction(&self) -> Option<f64> {
        match &self.cfg.mode {
            SineMode::Sweep(c) => match c.target {
                crate::scheduler::SweepTarget::Sweeps(n)
                | crate::scheduler::SweepTarget::WhicheverFirst { sweeps: n, .. } => {
                    Some((self.last_snapshot.sweeps_done as f64 / n as f64).min(1.0))
                }
                crate::scheduler::SweepTarget::DurationS(d) => {
                    Some((self.last_snapshot.elapsed_test_s / d).min(1.0))
                }
            },
            _ => None,
        }
    }
}

impl SineTestConfig {
    /// The SERVO's own internal fault fraction: a tight floor (1e-3) so the
    /// servo's own fault detection stays quiet while the debounced service-side
    /// servo-fault abort owns the abort decision.
    ///
    /// This is DISTINCT FROM (and intentionally TIGHTER than) the service's
    /// collapse fraction [`Self::service_fault_fraction`] (1e-2). The two differ
    /// by design: the service-side collapse check must fire on a WIDER band than
    /// the servo's silent freeze, so the debounced, typed service abort is what
    /// stops the run, not a silent servo freeze. Keeping them equal, or making the
    /// servo's wider than the service's, would reopen the silent-freeze gap.
    fn fault_fraction_effective(&self) -> f64 {
        1e-3
    }

    /// The SERVICE-SIDE collapse fraction for the debounced servo-fault abort,
    /// threaded into [`StepBuild::fault_fraction`]. Deliberately WIDER than
    /// [`Self::fault_fraction_effective`] (the servo's own freeze floor) so the
    /// service-side abort owns the fault decision. See that method for the pairing
    /// rationale.
    fn service_fault_fraction(&self) -> f64 {
        1e-2
    }
}

/// The drive->response pipeline lag, MEASURED by cross-correlation.
///
/// Ported from `hertzforge_random::service::measured_pipeline_lag` (which is where it
/// was paid for, in two incidents on 2026-08-01 and 2026-08-03). Kept as a
/// crate-local copy rather than shared, for the same reason the ring is: a
/// cross-crate dependency between two module crates that otherwise have no reason
/// to know about each other is a worse coupling than a second copy of forty lines
/// with the incident history attached. If a third module needs it, it moves to
/// `hertzforge-dsp` - which is where both copies point.
///
/// Returns `(lag frames, peak |r|, lags searched)`. A lag of `0` means "align
/// nothing", which is what leaves a genuinely incoherent path failing the gate
/// honestly. The trust threshold is deliberately modest: a real wired path peaks
/// far above it (~0.8 on the cDAQ jumper) and a noise floor sits far below
/// (~0.02).
///
/// BOUNDED, not exhaustive. Correlating the whole record at every candidate lag
/// is O(len^2/3), which at a multi-megasample sysID record puts a huge multiply
/// count between the last pretest drive and the first control drive - exactly the
/// window in which nothing is feeding the AO. The peak does not need the whole
/// record to be decisive: a fixed window against a coarse-then-refined ladder
/// finds the same peak in a fraction of the work.
/// Diagnostics the align note carries so a chassis red names its own diluter
/// (2026-08-08 chassis sitting: `lag 0, peak 0.040` was unexplainable without
/// these numbers).
#[derive(Debug, Clone, Copy)]
pub struct LagDiag {
    /// Where the correlation window was anchored.
    pub win_start: usize,
    /// Correlation window length in frames.
    pub w: usize,
    /// Mean-removed RMS of the drive window / the whole response.
    pub d_rms: f64,
    pub r_rms: f64,
    /// The removed means (a large `r_mean` names DC dilution outright).
    pub d_mean: f64,
    pub r_mean: f64,
    /// Whether the shift was APPLIED. `lag 0, applied` is a measured zero (the
    /// Sim's true value); `lag 0, NOT applied` is a no-peak reading - the two
    /// were indistinguishable in the 2026-08-08 chassis note and cost a diagnosis
    /// cycle.
    pub applied: bool,
}

impl std::fmt::Display for LagDiag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}; win {}+{}, d_rms {:.4e}, r_rms {:.4e}, d_mean {:.4e}, r_mean {:.4e}",
            if self.applied {
                "APPLIED"
            } else {
                "NO-PEAK (alignment OFF)"
            },
            self.win_start,
            self.w,
            self.d_rms,
            self.r_rms,
            self.d_mean,
            self.r_mean
        )
    }
}

/// Test-only re-export: the lag measurement is private mechanics, but its
/// failure shapes are chassis-class and the regression fixtures live in
/// `tests/` (sysid_lag_robustness.rs), which can only reach `pub` items.
pub fn measured_pipeline_lag_for_tests(
    drive: &[f64],
    resp: &[f64],
    fs_hz: f64,
) -> (usize, f64, usize, LagDiag) {
    measured_pipeline_lag(drive, resp, fs_hz)
}

fn measured_pipeline_lag(drive: &[f64], resp: &[f64], fs_hz: f64) -> (usize, f64, usize, LagDiag) {
    /// Normalized |r| a peak must clear to be trusted.
    const TRUST: f64 = 0.2;
    /// Multiply-accumulate budget for the coarse pass.
    const OP_BUDGET: usize = 100_000_000;

    let empty = LagDiag {
        win_start: 0,
        w: 0,
        d_rms: 0.0,
        r_rms: 0.0,
        d_mean: 0.0,
        r_mean: 0.0,
        applied: false,
    };
    let len = drive.len().min(resp.len());
    if len < 64 {
        return (0, 0.0, 0, empty);
    }
    // The lag is set by the DEVICE queue, not by the analysis block, so one
    // second of frames is generous headroom at any nfft. Range is PHYSICAL; the
    // correlation window is what buys estimate quality and must not be traded for
    // range (Random tried the swap and made it worse).
    let phys_cap = fs_hz.max(1.0) as usize;
    // SINGLE-FRAME stride, BY CONSTRUCTION. Sine's burst is WHITE to Nyquist
    // (run_sysid's bare xorshift; the chassis proved it - r_rms/d_rms 0.883 is
    // exactly the full-band ZOH loss, 2026-08-08), so the xcorr lobe through
    // the loopback is 2-3 frames and ANY coarser grid steps over the peak:
    // the shipped band_hi-derived stride of 25 returned the max of ~2000 pure
    // noise draws (|r| 0.043 = 4.1/sqrt(8192)) at every context while the same
    // pair cohered at 0.990 per Welch window. Random's twin keeps its lobe
    // formula legitimately - its pretest drive IS band-limited; Sine ported
    // the function without its premise. There is deliberately no band
    // parameter left to get this wrong with. Window pays for resolution:
    // w = OP_BUDGET / evals ~= 1953 frames, noise ceiling 4.8/sqrt(1953) =
    // 0.109, still under TRUST = 0.2; the true peak reads ~0.88 (ZOH).
    let stride = 1usize;
    let max_lag_wanted = phys_cap.min(len.saturating_sub(64));
    let evals = (max_lag_wanted / stride).max(1);
    let w = (OP_BUDGET / evals).clamp(256, 8192).min(len / 2);
    // Anchor the window a quarter into the record, not at its head: sysID bursts
    // ramp in, and a head-anchored window correlates mostly-attenuated drive
    // (2026-08-08 chassis sitting: the head window + an offset-bearing response
    // read `lag 0, peak 0.040` on a jumpered wire whose windowed coherence was
    // 0.990). Clamp so the full lag range still fits.
    let win_start = (len / 4).min(len.saturating_sub(w + max_lag_wanted));
    let max_lag = max_lag_wanted.min(len.saturating_sub(win_start + w));
    if max_lag == 0 || len < win_start + w + max_lag {
        return (0, 0.0, 0, empty);
    }
    // MEAN-REMOVED correlation. Any common DC (an AI offset, drift) rides both
    // signals and scores as confident correlation at EVERY lag - the fixture
    // for this function measured |r| 0.75 between two genuinely unrelated
    // streams that merely shared a -0.5 mean. Remove the means; the normalized
    // score is then a real correlation coefficient.
    let dwin = &drive[win_start..win_start + w];
    let d_mean = dwin.iter().sum::<f64>() / w as f64;
    let r_mean = resp[..len].iter().sum::<f64>() / len as f64;
    let d_rms = (dwin
        .iter()
        .map(|x| (x - d_mean) * (x - d_mean))
        .sum::<f64>()
        / w as f64)
        .sqrt();
    let r_rms = (resp[..len]
        .iter()
        .map(|x| (x - r_mean) * (x - r_mean))
        .sum::<f64>()
        / len as f64)
        .sqrt();
    let mut diag = LagDiag {
        win_start,
        w,
        d_rms,
        r_rms,
        d_mean,
        r_mean,
        applied: false,
    };
    if d_rms <= 0.0 || r_rms <= 0.0 {
        return (0, 0.0, 0, diag);
    }
    let score = |lag: usize| -> f64 {
        let mut acc = 0.0;
        for i in 0..w {
            acc += (dwin[i] - d_mean) * (resp[win_start + i + lag] - r_mean);
        }
        ((acc / w as f64) / (d_rms * r_rms)).abs()
    };
    let mut best = (0usize, 0.0f64);
    let mut lag = 0usize;
    while lag < max_lag {
        let r = score(lag);
        if r > best.1 {
            best = (lag, r);
        }
        lag += stride;
    }
    // Refine around the coarse winner at single-frame resolution.
    if stride > 1 {
        let lo = best.0.saturating_sub(stride);
        let hi = (best.0 + stride).min(max_lag);
        for lag in lo..hi {
            let r = score(lag);
            if r > best.1 {
                best = (lag, r);
            }
        }
    }
    let searched = (max_lag / stride.max(1)).max(1);
    if best.1 < TRUST {
        // NOT decisive. Align nothing and let the gate judge the estimate as
        // measured - a confident wrong shift manufactures coherence.
        return (0, best.1, searched, diag);
    }
    diag.applied = true;
    (best.0, best.1, searched, diag)
}

/// Human unit label for the report.
fn units_label(u: &Units) -> String {
    match u {
        Units::AccelG => "g".into(),
        Units::VelocityIps => "in/s".into(),
        Units::DisplacementMil => "mil".into(),
        Units::ForceLbf => "lbf".into(),
        Units::VoltageV => "V".into(),
        Units::Custom(s) => s.clone(),
    }
}

// Re-export the config + report surface at the service module root for ergonomic
// paths (mirrors hertzforge-random).
pub use config::{
    CaptureConfig as SineCaptureConfig, ColaConfig, ColaWaveform as SineColaWaveform,
    ControlChannel, MetaField, SineMode as Mode,
};
pub use report::{
    ChannelRow as ReportChannelRow, Event as ReportEvent, EventKind as ReportEventKind, ReportData,
    RunState as SineRunState, SineStatus as Status, TripKind, TripRecord as SineTripRecord,
};
