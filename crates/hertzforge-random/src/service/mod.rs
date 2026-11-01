//! `RandomService` - the Path-B run orchestration + the level ladder state machine.
//!
//! The service owns a [`crate::control::ControlLaw`] (the shipped [`crate::control::HinvLaw`]),
//! a persistent Sim [`engine::PlantLoop`], the protection engines, the capture sink,
//! and the [`report::EventLog`]. It runs the state machine
//! (`Idle -> Feasibility -> SysId -> CoherenceGate -> Engaged -> FullLevel -> Stopped`,
//! plus `Aborted` from any running state) and assembles the serde-serializable
//! [`report::ReportData`] + the live [`report::Status`] surface.
//!
//! # The level/schedule split (documented precisely)
//!
//! Two things scale the drive, and they live in DIFFERENT places:
//!
//! - **The SERVICE owns the SCHEDULE level** (the feedforward scale). At schedule level
//!   `L` dB the service commands the law to synthesize at `10^(L/20)` of the full shaped
//!   target: it sets the law's servo target to `full_target_rms * 10^(L/20)` and its
//!   feedforward scale to match. This is the ladder rung (sysID -> -12 -> ... -> 0 dB).
//! - **The LAW owns the TRIM/SERVO around that level** (the law's contract): the scalar
//!   servo holds the realized in-band RMS on the *scheduled* target, and the
//!   mean-preserving per-bin trim flattens the shape. The service NEVER reaches inside
//!   the servo state; it only re-targets it when the schedule advances a rung.
//!
//! So: the schedule sets WHERE the loop aims (the rung), the law's servo + trim drive
//! the measured response to that rung and hold the shape. This is the clean seam that
//! keeps the ladder (service) from fighting the servo (law).
//!
//! # Loop tick (the closed-loop step)
//!
//! One `loop_tick`: synthesize the drive from the law's current PSD via COLA, apply the
//! notch ceiling (notch seam, AFTER synthesis), run one plant segment, measure each control
//! channel's PSD (Welch), composite them (weights/mode), build [`crate::control::LoopInputs`]
//! (with the band + the previous loop's notch mask), call `law.iterate`, then run every
//! protection engine (envelope alarm/abort, kinematic tri-field abort, drive-Vrms
//! clamp+abort, divergence abort, overload proxy).

pub mod config;
pub mod engine;
pub mod report;

use crate::control::{composite_psd, ControlLaw, HinvLaw, LoopInputs, NotchReport, NotchSet};
use config::{CaptureTrigger, ConfigError, RandomTestConfig, RunMode};
use engine::{DriveSynth, KinematicMonitor, KinematicTrip, LoopConfig, PlantLoop};
use hertzforge_breakpoints::{Envelope, LineSpectrum};
use hertzforge_capture::{
    default_data_root, save_snapshot, CaptureConfig, CaptureSink, Mark, Snapshot,
};
use hertzforge_channel_table::{ChannelRole, ChannelTable, Units};
use hertzforge_shaker_db::{FeasibilityCalculator, FeasibilityReport};
use report::{
    ChannelRow, CompositeModeRecord, EventKind, EventLog, FeasibilityDigest, Honesty,
    LevelHistoryEntry, LimitProfileRow, ReportData, RunSettings, RunState, Status, TripKind,
    TripRecord,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The mode payload for a periodic-save [`Snapshot`] (the report spectra).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RandomSnapshotPayload {
    pub composite_psd: Vec<f64>,
    pub per_channel_psds: Vec<Vec<f64>>,
    pub drive_psd: Vec<f64>,
    pub level_db: f64,
    pub h_mag2: Vec<f64>,
    pub coherence: Vec<f64>,
    /// Notch active-line count at the snapshot.
    pub notch_lines: usize,
    /// Envelope alarm/abort line counts at the snapshot.
    pub alarm_lines: usize,
    pub abort_lines: usize,
    /// df of the line grid (Hz), so the snapshot is self-describing.
    pub df_hz: f64,
}

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

/// Per-channel overload proxy latch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct OverloadLatch {
    channel_index: u32,
    range_max: f64,
    latched: bool,
}

/// One requested FRF pair, in service terms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrfPairReq {
    /// `None` = the synthesized DRIVE is the reference (needs the sysID lag
    /// applied); `Some(slot)` = an acquired channel (shares the clock, no
    /// shift - see `sysid_lag_frames`).
    pub reference_slot: Option<usize>,
    /// The response channel's slot in the acquired frame.
    pub response_slot: usize,
    /// The operator-facing ids, carried through for labelling.
    pub reference_ch: Option<u32>,
    pub response_ch: u32,
}

/// One pair's measured FRF for the last control loop.
#[derive(Debug, Clone, Default)]
pub struct FrfPairResult {
    /// `|H|` per line.
    pub mag: Vec<f64>,
    /// Ordinary coherence per line, 0..1.
    pub coherence: Vec<f64>,
    /// Phase per line, RADIANS, wrapped to (-pi, pi].
    ///
    /// Published wrapped on purpose: the estimate itself wraps,
    /// and unwrapping is a DISPLAY choice the pane makes where the operator
    /// can see and toggle it. Unwrapping here would bake one interpretation
    /// into the measurement.
    pub phase: Vec<f64>,
}

/// Soft-start climb duration when the operator set no explicit level ramp.
const DEFAULT_SOFT_START_S: f64 = 5.0;

/// The host-side Random run service (Path B). Owns the control law + the plant
/// loop + the protection engines + capture + the event log.
pub struct RandomService {
    cfg: RandomTestConfig,
    loop_cfg: LoopConfig,
    /// The station backend, held from construction until `arm()` moves it into the
    /// [`PlantLoop`]. The service is HANDED a backend (the UI's station factory
    /// builds it) instead of manufacturing a `SimBackend` out of a plant description.
    /// `None` after arm - the loop owns it from then on, and there is no re-arm path
    /// (the UI rebuilds the whole service on every `random_configure`).
    backend: Option<Box<dyn hertzforge_hal::Backend>>,
    /// The station class the arm gate is given and the `SchedulingFault` retry rule
    /// keys off. Supplied with the backend, because the class is a property of what
    /// the backend is bound to, not of the backend type.
    station: hertzforge_module_abi::StationClass,
    table: ChannelTable,
    state: RunState,
    log: EventLog,

    // --- line grid + target (interpolated from the breakpoints onto the plant grid) --
    /// In-band window `[lo,hi)` on the line grid.
    band: (usize, usize),
    df_hz: f64,
    /// The FULL (0 dB) shaped target PSD on the line grid.
    full_target: Vec<f64>,
    /// The reference (target) line spectrum for the report.
    target_line: LineSpectrum,
    /// The DISPLAY reference (whole breakpoint span); see `Grid::display_target`.
    display_target: LineSpectrum,
    reference_grms: f64,

    // --- sysID artifacts -----------------------------------------------------------
    h_mag2: Vec<f64>,
    coherence: Vec<f64>,
    band_coherence: f64,

    // --- control law + loop state ---------------------------------------------------
    law: Option<HinvLaw>,
    /// The current schedule level in dB (the feedforward rung).
    schedule_level_db: f64,
    /// The current level table index (AUTO) or the manual step position within the
    /// table for jump-to-level.
    level_index: usize,
    /// Measurement: when the previous segment's acquisition ended, so the
    /// inter-segment (AO-unfed) window can be timed.
    last_segment_end: Option<std::time::Instant>,
    /// Measurement: the last segment's timing, for a host to log.
    last_segment_timing: Option<SegmentTiming>,
    /// Measurement: start of the pre-acquisition (AO-unfed) phase.
    pre_segment_start: Option<std::time::Instant>,
    /// The most recent segment's non-empty driver diagnostics.
    last_driver_warnings: Vec<String>,
    /// Hold after sysID/gate for operator review before anything drives.
    /// Default ON - the expensive-article case is the one where being wrong is
    /// unrecoverable; a production operator turns it off once.
    review_after_sysid: bool,
    /// What the last segment boundary's arm/start did.
    last_boundary: Option<engine::BoundaryTrace>,
    /// Set by a live MANUAL -> AUTO takeover when the operator's current
    /// level sits BETWEEN rungs: the next dwell-elapse advances TO
    /// `table_db[level_index]` (the nearest rung above) instead of past it, so
    /// the resume never jumps more than one rung's worth.
    auto_resume_at_rung: bool,
    level_history: Vec<LevelHistoryEntry>,
    loop_count: u64,
    /// Control windows remaining in the ENGAGE SETTLE (2026-08-03): windows
    /// dominated by the deliberate restart transient, exempt from envelope
    /// judgment, law adaptation and divergence counting. Absolute protections
    /// (kinematic g, drive Vrms) stay live throughout.
    engage_settle_loops: u32,
    /// The ACCUMULATED control estimate (per-bin EMA of the composite measured
    /// PSD, 2026-08-03). The per-loop Welch estimate has only `2 x nAvg` DOF -
    /// at nAvg 4 that is sigma ~2.2 dB per line, and everything that consumed
    /// it raw was chasing that scatter: the LAW wrote it into the drive shape,
    /// the ENVELOPE alarmed on 114-123 of 991 lines at steady level on a
    /// perfect wire, the display drew fuzz where the operator expects the
    /// control line to sit ON the reference. The EMA (alpha 0.25) roughly
    /// quadruples the effective DOF within a second. Scaled in place on level
    /// change (the estimate is level-proportional power), cleared at engage.
    control_est: Option<Vec<f64>>,
    /// The level the operator COMMANDED (the ramp walks `schedule_level_db`
    /// toward it; equal when no ramp is in flight).
    level_target_db: f64,
    /// The in-flight ramp rate, dB/s (set per transition from `level_ramp_s`).
    level_ramp_db_per_s: f64,
    /// A controlled stop is pending: when the ramp-down reaches its target,
    /// the run stops. Aborts NEVER take this path - they stay immediate.
    stop_after_ramp: bool,
    /// The soft-start floor this run engaged at, while the climb to the start
    /// level is still in progress (`None` = no soft start / already arrived).
    soft_start_floor_db: Option<f64>,
    /// The FRF pairs this run measures alongside the control loop.
    frf_pairs: Vec<FrfPairReq>,
    /// Last loop's results, one per requested pair, in the same order.
    frf_results: Vec<FrfPairResult>,
    /// The pipeline lag MEASURED at sysID, in frames.
    ///
    /// Applied to a DRIVE-referenced FRF pair and to nothing else:
    /// the drive is synthesized ahead of the response by the device queue, but
    /// two ACQUIRED channels share one clock and shifting either would
    /// manufacture a phase error out of nothing. Stored rather than
    /// re-measured per loop because it is a queue depth, not a variable.
    sysid_lag_frames: usize,
    /// The last synthesized drive record, kept so a DRIVE-referenced FRF pair
    /// has its reference signal (the drive is not an acquired channel).
    last_drive_record: Vec<f64>,
    /// The previous loop's notch mask (fed back into LoopInputs, notch seam).
    notch_mask: Vec<bool>,
    /// Consecutive loops with a non-empty divergent-bin set (divergence caller contract).
    divergence_streak: u32,

    // --- last-loop measurements (for status + report) -------------------------------
    last_composite_psd: Option<Vec<f64>>,
    last_per_channel_psds: Vec<Vec<f64>>,
    last_drive_psd: Option<Vec<f64>>,
    last_measured_grms: Option<f64>,
    last_notch_lines: usize,
    last_alarm_lines: usize,
    last_abort_lines: usize,
    alarm_active: bool,

    // --- protection engines ---------------------------------------------------------
    envelope: Option<Envelope>,
    kinematic: Option<KinematicMonitor>,
    overloads: Vec<OverloadLatch>,
    /// The last plant segment's interleaved response samples (display tap for a
    /// hosting layer's raw time-domain stream). Empty before the first loop.
    last_segment_interleaved: Vec<f32>,
    /// Channel count of `last_segment_interleaved`'s interleave.
    last_segment_n_ch: usize,

    // --- plant loop + drive synth ---------------------------------------------------
    plant_loop: Option<PlantLoop>,
    /// Per-tick display tap, remembered so `arm()` can re-install it on
    /// the freshly built plant loop.
    block_observer: Option<engine::BlockObserver>,
    synth: Option<DriveSynth>,

    // --- capture --------------------------------------------------------------------
    capture: Option<CaptureSink>,
    data_root: PathBuf,
    /// Seconds elapsed at the current level (integrated in segment-seconds).
    elapsed_at_level_s: f64,
    /// AUTO capture window remaining (s) after a debounced level entry.
    auto_capture_remaining_s: f64,
    /// Time (s) since the last periodic snapshot.
    since_last_snapshot_s: f64,
    /// Whether a level-entry debounce is pending (settling) before the AUTO window opens.
    level_settle_remaining_s: f64,

    // --- feasibility + trip ---------------------------------------------------------
    feasibility: Option<FeasibilityDigest>,
    trip: Option<TripRecord>,
    /// Whether an override was armed past a feasibility exceedance.
    override_ack: Option<String>,
}

/// The reference-on-the-line-grid state: everything the service derives from
/// `LoopConfig`'s `fs/nfft` rather than from the test config alone.
///
/// It exists as a type because it is derived TWICE - once at construction, and
/// again at arm if the station programmed a rate the loop did not ask for. Those
/// two derivations must be the same derivation; the alternative (a second copy of
/// the interpolation inlined in `arm_inner`) is exactly the duplicated derivation
/// that drifts, and here it would drift the ONE thing the arm-time rebuild exists to
/// keep true.
struct Grid {
    df_hz: f64,
    /// In-band window `[lo,hi)` on the line grid.
    band: (usize, usize),
    /// The FULL (0 dB) shaped target PSD on the line grid.
    full_target: Vec<f64>,
    /// The reference (target) line spectrum for the report.
    target_line: LineSpectrum,
    /// The reference across the WHOLE breakpoint span, for DISPLAY only.
    ///
    /// `full_target` is deliberately zeroed outside the interior control band:
    /// the loop must not chase lines the plant cannot resolve, and a nonzero
    /// out-of-band reference breaches the abort-LOW band spuriously. That is a
    /// CONTROL decision, and publishing it as the DISPLAY reference made the
    /// reference and alarm/abort bands stop a bin or two inside the operator's
    /// own test band while the axis started at the breakpoint (reported
    /// 2026-08-02: bands absent below ~25 Hz on a 20 Hz profile). The display
    /// draws the profile that was authored; the loop keeps its restriction.
    display_target: LineSpectrum,
}

impl Grid {
    /// Interpolate the reference onto `loop_cfg`'s COLA line grid; zero it outside
    /// the resolvable interior band (the demo/adversarial convention).
    fn derive(cfg: &RandomTestConfig, loop_cfg: &LoopConfig) -> Result<Self, ServiceError> {
        let df_hz = loop_cfg.df();
        let nbins = loop_cfg.nbins();
        let band = loop_cfg.band_bins();
        let f_max = df_hz * (nbins as f64 - 1.0);

        let line = cfg
            .breakpoints
            .interpolate(df_hz, f_max)
            .map_err(|e| ServiceError::Config(ConfigError::BadBreakpoints(e.to_string())))?;
        let mut full_target = vec![0.0f64; nbins];
        for (k, t) in full_target.iter_mut().enumerate().take(band.1).skip(band.0) {
            *t = line.psd.get(k).copied().unwrap_or(0.0);
        }
        let inband_energy: f64 = full_target[band.0..band.1].iter().sum();
        if inband_energy <= 0.0 {
            return Err(ServiceError::Config(ConfigError::BadConfig(
                "reference has no energy in the plant's resolvable band".into(),
            )));
        }

        let target_line = LineSpectrum {
            f0_hz: 0.0,
            df_hz,
            psd: full_target.clone(),
        };
        // The authored profile, un-restricted: every bin the breakpoints
        // resolve, so the drawn reference/bands span the whole test band.
        let display_target = LineSpectrum {
            f0_hz: 0.0,
            df_hz,
            psd: line.psd.clone(),
        };
        Ok(Grid {
            df_hz,
            band,
            full_target,
            target_line,
            display_target,
        })
    }
}

impl RandomService {
    /// Build a service for `cfg` against `table` on `backend`, validating the config
    /// up front and interpolating the reference onto the loop's line grid. `data_root`
    /// is the LOCAL capture root (resolved via [`default_data_root`] if `None`).
    ///
    /// `backend` + `station` replace the old `PlantConfig`'s Sim half. The
    /// service no longer manufactures its own plant, so a Random run can be pointed
    /// at an NI station without touching this crate. The backend is held (unopened)
    /// until `arm()`, which is where the old code built one.
    pub fn new(
        cfg: RandomTestConfig,
        table: ChannelTable,
        loop_cfg: LoopConfig,
        backend: Box<dyn hertzforge_hal::Backend>,
        station: hertzforge_module_abi::StationClass,
        data_root: Option<PathBuf>,
    ) -> Result<Self, ServiceError> {
        cfg.validate()?;
        table.validate().map_err(|e| {
            ServiceError::Config(ConfigError::BadConfig(format!("channel table: {e}")))
        })?;

        let nbins = loop_cfg.nbins();
        let grid = Grid::derive(&cfg, &loop_cfg)?;
        let Grid {
            df_hz,
            band,
            full_target,
            target_line,
            display_target,
        } = grid;
        let reference_grms = cfg.breakpoints.grms_of_table().unwrap_or(0.0);

        // The overload latches: one per enabled non-drive channel.
        let overloads: Vec<OverloadLatch> = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .map(|c| OverloadLatch {
                channel_index: c.index,
                range_max: c.range_max,
                latched: false,
            })
            .collect();

        let data_root = data_root.unwrap_or_else(default_data_root);

        Ok(RandomService {
            cfg,
            loop_cfg,
            backend: Some(backend),
            station,
            table,
            state: RunState::Idle,
            log: EventLog::default(),
            band,
            df_hz,
            full_target,
            target_line,
            display_target,
            reference_grms,
            h_mag2: vec![0.0; nbins],
            coherence: vec![0.0; nbins],
            band_coherence: 0.0,
            law: None,
            schedule_level_db: 0.0,
            level_index: 0,
            auto_resume_at_rung: false,
            last_segment_end: None,
            last_segment_timing: None,
            pre_segment_start: None,
            last_driver_warnings: Vec::new(),
            review_after_sysid: true,
            last_boundary: None,
            level_history: Vec::new(),
            loop_count: 0,
            engage_settle_loops: 0,
            control_est: None,
            level_target_db: 0.0,
            level_ramp_db_per_s: 0.0,
            stop_after_ramp: false,
            soft_start_floor_db: None,
            frf_pairs: Vec::new(),
            frf_results: Vec::new(),
            sysid_lag_frames: 0,
            last_drive_record: Vec::new(),
            notch_mask: Vec::new(),
            divergence_streak: 0,
            last_composite_psd: None,
            last_per_channel_psds: Vec::new(),
            last_drive_psd: None,
            last_measured_grms: None,
            last_notch_lines: 0,
            last_alarm_lines: 0,
            last_abort_lines: 0,
            alarm_active: false,
            envelope: None,
            kinematic: None,
            last_segment_interleaved: Vec::new(),
            last_segment_n_ch: 0,
            overloads,
            plant_loop: None,
            block_observer: None,
            synth: None,
            capture: None,
            data_root,
            elapsed_at_level_s: 0.0,
            auto_capture_remaining_s: 0.0,
            since_last_snapshot_s: 0.0,
            level_settle_remaining_s: 0.0,
            feasibility: None,
            trip: None,
            override_ack: None,
        })
    }

    // --- introspection --------------------------------------------------------------

    pub fn state(&self) -> RunState {
        self.state
    }
    /// The config this service was built from. Exposed so a caller holding ONLY a
    /// configured service can rebuild an equivalent one without re-asking the UI to
    /// reassemble the config - the operator's Reset escape hatch (`RandomState::reset`)
    /// must work even when the UI's assembled config is momentarily un-buildable, which
    /// is exactly the state that stranded the 2026-07-30 real-chassis run.
    pub fn config(&self) -> &RandomTestConfig {
        &self.cfg
    }
    /// The loop this service will run (or is running). Before `arm()` it is what the
    /// caller supplied; after `arm()` it is what the STATION programmed - the two
    /// differ whenever the device coerced the rate, and this accessor is how a caller
    /// or a test reads which one it got.
    pub fn loop_config(&self) -> &LoopConfig {
        &self.loop_cfg
    }
    /// The REALIZED line resolution (Hz/bin) every spectrum in this run is binned
    /// against. Distinct from the REQUESTED `RandomTestConfig::df_hz` the report
    /// records: after an arm-time rate adoption the two are not the same number.
    pub fn realized_df_hz(&self) -> f64 {
        self.df_hz
    }
    pub fn log(&self) -> &EventLog {
        &self.log
    }
    pub fn band_coherence(&self) -> f64 {
        self.band_coherence
    }
    /// What the last segment boundary's arm/start did.
    pub fn last_boundary(&self) -> Option<engine::BoundaryTrace> {
        self.last_boundary
    }

    /// The most recent segment's DRIVER diagnostics, if any.
    pub fn last_driver_warnings(&self) -> &[String] {
        &self.last_driver_warnings
    }

    /// The last segment's drive-continuity timing, for a host to log.
    /// `None` before the first segment.
    pub fn last_segment_timing(&self) -> Option<SegmentTiming> {
        self.last_segment_timing
    }

    pub fn schedule_level_db(&self) -> f64 {
        self.schedule_level_db
    }
    /// The in-band gRMS of the FULL (0 dB) shaped target.
    pub fn full_target_grms(&self) -> f64 {
        Self::inband_rms(&self.full_target, self.band, self.df_hz)
    }
    /// The trip record if the run aborted.
    pub fn trip(&self) -> Option<&TripRecord> {
        self.trip.as_ref()
    }
    pub fn feasibility(&self) -> Option<&FeasibilityDigest> {
        self.feasibility.as_ref()
    }

    fn transition(&mut self, to: RunState) {
        let from = self.state;
        if from != to {
            self.state = to;
            self.log.push(EventKind::StateChange { from, to });
        }
    }

    // --- feasibility + arming (the ladder's first rung) -----------------------------

    /// Run the pre-test feasibility assessment. Transitions
    /// `Idle -> Feasibility`; if the shaker force/kinematics are Exceeded, parks at
    /// `FeasibilityBlocked` (arming hard-blocked until [`Self::arm_with_override`]).
    /// Returns the feasibility report. No shaker configured => a permissive Advisory
    /// report + a note (feasibility not evaluated).
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
                    text: "No shaker configured; feasibility NOT evaluated. Arming \
                           allowed without a shaker-limit block."
                        .into(),
                });
                FeasibilityReport {
                    items: Vec::new(),
                    overall: hertzforge_shaker_db::Verdict::NotEvaluated,
                    assumptions: vec!["No shaker record configured.".into()],
                }
            }
            Some(shaker) => {
                // Prefer the analytic table kinematics (exact per-segment integrals).
                FeasibilityCalculator::assess_with_table(
                    shaker,
                    self.cfg.moving_mass,
                    self.cfg.geometry,
                    &self.cfg.breakpoints,
                    &self.target_line,
                    self.cfg.crest_factor,
                )
                .unwrap_or_else(|_| {
                    FeasibilityCalculator::assess(
                        shaker,
                        self.cfg.moving_mass,
                        self.cfg.geometry,
                        &self.target_line,
                        self.cfg.crest_factor,
                    )
                })
            }
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

    /// Arm the run. Requires feasibility to have passed (not Exceeded). On an
    /// exceedance use [`Self::arm_with_override`] instead. Arming opens the plant
    /// loop and moves to `SysId` (the caller then drives [`Self::run_sysid`]).
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
    /// Logs the ack verbatim and records it in the feasibility digest.
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

    /// Open the plant loop + drive synth and enter SysId. Shared by both arm paths.
    fn arm_inner(&mut self) -> Result<(), ServiceError> {
        let backend = self.backend.take().ok_or_else(|| {
            ServiceError::Plant(
                "the station backend was already consumed by an earlier arm; rebuild the \
                 service to run again"
                    .into(),
            )
        })?;
        // The ENGINE table: the operator table plus a synthetic Drive row when
        // it carries none - the same pattern the recorder uses, for the same
        // reason. `Backend::configure` REPLACES the backend's assignments from
        // the table it is handed; a drive-less table wipes the factory-derived
        // AO, so the NI backend builds no AO task, derives no clock routing,
        // and the clock resolves Unknown - the arm gate then refuses a pair
        // the proven set covers. Found on the first real-chassis Random arm
        // (2026-07-30); invisible on NI-SIM because its pass condition was
        // non-convergence, which a driveless run also satisfies.
        let engine_table = engine_table_with_drive(&self.table);
        // `&mut self.loop_cfg`: the loop comes back describing what the STATION will
        // really run, not what this service asked for. Everything below is built from
        // the adopted config - which is the whole point, and the ordering is the fix.
        let station = self.station;
        let plant_loop = PlantLoop::open(backend, station, &mut self.loop_cfg, engine_table)
            .map_err(ServiceError::Plant)?;
        let adoption = *plant_loop.adoption();
        // Re-install the display tap on the new loop.
        let mut plant_loop = plant_loop;
        plant_loop.set_block_observer(self.block_observer.clone());
        self.plant_loop = Some(plant_loop);

        // The station programmed a rate this service did not ask for, so every
        // grid-denominated thing built at construction describes the wrong loop.
        // Rebuild them from the adopted config BEFORE the synth, the envelope and the
        // kinematic monitor are built below.
        //
        // The breakpoint table and the band edges are Hz-denominated, so they carry
        // across unchanged; what moves is `df`, the bin window they land in, and the
        // interpolated target. See `LoopConfig::adopt_station_actual`.
        if adoption.grid_changed() {
            let grid = Grid::derive(&self.cfg, &self.loop_cfg)?;
            self.df_hz = grid.df_hz;
            self.band = grid.band;
            self.full_target = grid.full_target;
            self.target_line = grid.target_line;
            self.display_target = grid.display_target;
            let nbins = self.loop_cfg.nbins();
            self.h_mag2 = vec![0.0; nbins];
            self.coherence = vec![0.0; nbins];
        }
        if adoption.rate_changed() || adoption.guards_restored() {
            self.log.push(EventKind::StationAdopted {
                station: adoption.station.to_string(),
                requested_fs_hz: adoption.before.fs_hz,
                actual_fs_hz: adoption.after.fs_hz,
                df_hz: adoption.after.df_hz,
                control_lines: adoption.after.lines,
                block_frames: adoption.after.block_frames,
                scheduling_guards_restored: adoption.guards_restored(),
            });
        }

        self.synth = Some(DriveSynth::new(&self.loop_cfg));

        // Build the frequency-domain envelope on the line grid, then
        // RESTRICT it to the CONTROL band: the reference resolves across the whole
        // breakpoint span, but the loop only drives the interior band the plant can
        // resolve (the target is zeroed outside it). Out-of-band lines the loop cannot
        // fill would breach the abort-LOW band spuriously (measured ~ noise floor << a
        // nonzero out-of-band reference). Zeroing the envelope where the control target
        // is zero makes the alarm/abort check track exactly the driven band.
        let f_max = self.df_hz * (self.loop_cfg.nbins() as f64 - 1.0);
        match Envelope::from_table_on_grid(
            &self.cfg.breakpoints,
            self.df_hz,
            f_max,
            self.cfg.alarm_db,
            self.cfg.abort_db,
        ) {
            Ok(mut env) => {
                let n = env.reference.psd.len();
                for k in 0..n {
                    let driven = self.full_target.get(k).copied().unwrap_or(0.0) > 0.0;
                    if !driven {
                        // Out of the control band: no breach possible (matches the
                        // reference's own `<= 0` out-of-band skip in Envelope::check).
                        env.reference.psd[k] = 0.0;
                        env.alarm_hi[k] = f64::INFINITY;
                        env.alarm_lo[k] = 0.0;
                        env.abort_hi[k] = f64::INFINITY;
                        env.abort_lo[k] = 0.0;
                    }
                }
                self.envelope = Some(env);
            }
            Err(e) => self.log.push(EventKind::Note {
                text: format!("envelope not built ({e}); envelope protection disabled this run"),
            }),
        }

        // Build the kinematic monitor (reactive tri-field) from the channel table.
        let kin = KinematicMonitor::new(
            &self.table,
            self.loop_cfg.fs_hz,
            5.0, // HP corner, decade below typical bands
            1,   // sustain (spike-sensitive; the settle window suppresses derived-axis)
            5.0, // settle HP constants
        );
        self.kinematic = if kin.is_active() { Some(kin) } else { None };

        self.notch_mask = vec![false; self.loop_cfg.nbins()];
        self.transition(RunState::SysId);
        Ok(())
    }

    // --- sysID + coherence gate ------------------------------------------------

    /// Run the pre-test system-ID at the sysID level (default -20 dB
    /// amplitude of full). Drives a low-level flat band-limited excitation for a few
    /// segments, estimates `|H|^2` + coherence, seeds the control law, and computes the
    /// band coherence. Transitions `SysId -> CoherenceGate`. `pretest_segments` bounds
    /// the sysID length (>= 1).
    pub fn run_sysid(&mut self, pretest_segments: usize) -> Result<(), ServiceError> {
        if !matches!(self.state, RunState::SysId) {
            return Err(ServiceError::BadState(format!(
                "run_sysid from {:?}",
                self.state
            )));
        }
        let (band, nbins, df) = (self.band, self.loop_cfg.nbins(), self.df_hz);
        // sysID excitation: a flat band-limited PSD at the sysID amplitude relative to a
        // small pretest reference. Amplitude scale is 10^(sysid_level_db/20) of a modest
        // pretest base (kept low so the plant is gently excited).
        let base = 0.05f64; // pretest drive amplitude base (matches the demo)
        let amp = base * 10f64.powf(self.cfg.sysid_level_db / 20.0);
        let lvl = amp * amp;
        let mut pretest_psd = vec![0.0f64; nbins];
        if self.cfg.shaped_pretest {
            // SHAPED pretest (operator knob): excite the plant the way the
            // test will - proportional to the reference in band, normalized
            // so the band-mean power matches the flat pretest level. A
            // resonant article gets its energy where the run will put it.
            let mean = {
                let sl = &self.full_target[band.0..band.1.min(self.full_target.len())];
                let m = sl.iter().sum::<f64>() / sl.len().max(1) as f64;
                if m > 0.0 {
                    m
                } else {
                    1.0
                }
            };
            for (p, &t) in pretest_psd[band.0..band.1]
                .iter_mut()
                .zip(self.full_target[band.0..].iter())
            {
                *p = lvl * (t / mean).max(0.0);
            }
        } else {
            for p in pretest_psd[band.0..band.1].iter_mut() {
                *p = lvl;
            }
        }
        // Operator Vrms bounds on the pretest drive. The FLOOR raises a
        // too-quiet pretest into the coherent range; the CEILING is the
        // safety bound and is applied LAST so it always wins.
        let vrms = (pretest_psd.iter().sum::<f64>() * df).sqrt();
        if vrms > 0.0 {
            let mut scale2 = 1.0f64;
            if let Some(mn) = self.cfg.pretest_min_vrms {
                if vrms * scale2.sqrt() < mn {
                    scale2 = (mn / vrms).powi(2);
                }
            }
            if let Some(mx) = self.cfg.pretest_max_vrms {
                if vrms * scale2.sqrt() > mx {
                    scale2 = (mx / vrms).powi(2);
                }
            }
            if (scale2 - 1.0).abs() > 1e-12 {
                for p in pretest_psd.iter_mut() {
                    *p *= scale2;
                }
                self.log.push(EventKind::Note {
                    text: format!(
                        "pretest drive scaled to Vrms bounds: {:.4} -> {:.4} Vrms",
                        vrms,
                        vrms * scale2.sqrt()
                    ),
                });
            }
        }

        // The FULL pretest record, not the last segment (the 2026-08-03 arm-refused
        // defect). The estimate used to keep only the final segment, whose length is
        // `nAvg x nPts` frames - at the operator's 1024 x 4 that is 4096 samples,
        // SHORTER than this chassis's measured ~2900-frame pipeline lag at fs 16384.
        // The lag search (capped at a third of the record) could not even reach the
        // true peak, alignment stayed off, and band coherence on a perfectly-jumpered
        // loop read 0.140 - the same failure class an earlier jumpered-loop defect
        // measured at ~0.04. The wire stream is gapless across segment boundaries (the
        // engine carries its queue across segments), so concatenation is a valid longer
        // look at the same signals.
        let mut all_drive: Vec<f64> = Vec::new();
        let mut all_resp: Vec<f64> = Vec::new();
        let segs = pretest_segments.max(1);
        // Record floor: one second of lag headroom plus eight estimation
        // windows. `pretest_segments` is a MINIMUM; a configuration whose
        // segments are short simply runs more of them instead of silently
        // producing an estimate too short to align. The cap bounds a
        // misconfigured loop, not a healthy one.
        let min_record = self.loop_cfg.fs_hz as usize + 8 * self.loop_cfg.nfft;
        // The EXTENSION is bounded in TIME, not just in segments: at a large
        // block size one segment is seconds long, so "up to 64 more" was a
        // multi-minute pretest nobody asked for. A pretest that cannot satisfy
        // the record floor within this budget proceeds with what it has and
        // says so - the coherence gate then judges the shorter estimate on its
        // merits, which is the honest outcome.
        const MAX_EXTEND_S: f64 = 20.0;
        let seg_s = self.loop_cfg.segment_seconds().max(1e-6);
        let max_extra = ((MAX_EXTEND_S / seg_s).ceil() as usize).clamp(1, 64);
        let mut ran = 0usize;
        while ran < segs || (all_drive.len() < min_record && ran < segs + max_extra) {
            // The wall tracks nfft, NOT segment length (measured
            // 2026-08-04: nfft 16384 x nAvg 8 makes the same 131072-frame
            // segment as nfft 32768 x nAvg 4 and PASSES, while nfft 32768 fails
            // at nAvg 2 with a segment HALF that size). So the suspect is work
            // that scales with nfft and happens while the AO is running on a
            // finite buffer - and synthesis is exactly that. Nothing writes AO
            // during it, so a synth longer than the committed AO queue starves
            // the output before the segment's first tick.
            let t_synth = std::time::Instant::now();
            let (mut leadin, mut drive) = self.synth_segment(&pretest_psd)?;
            let synth_ms = t_synth.elapsed().as_secs_f64() * 1e3;
            // eprintln, not a log Note: in the failing case the pretest aborts on
            // segment 0, and the abort path's event tail did not carry the note
            // through. A diagnostic that only survives the HEALTHY path is no
            // diagnostic at all - it is absent from exactly the run you need it for.
            eprintln!(
                "pretest segment {ran}: synth {synth_ms:.1} ms for {} frames (nfft {})",
                self.loop_cfg.window_len(),
                self.loop_cfg.nfft
            );
            if let Some(sig) = self.cfg.drive_clip_sigma {
                Self::clip_sigma(&mut leadin, sig);
                Self::clip_sigma(&mut drive, sig);
            }
            let seg = self.run_plant_segment(leadin, drive.clone())?;
            let r = self.composite_control_signal(&seg);
            // STEP 4. Drive and response are each CORRECT (ratio 1.0076,
            // in-band fraction 0.9991 at nfft 32768) yet do not correlate, so
            // the fault is in the PAIRING. The concatenated records only line
            // up if every segment contributes the SAME count to both; a
            // per-segment mismatch drifts the two apart cumulatively, and no
            // single global lag can align a drift - which is exactly what a
            // collapsed xcorr peak looks like.
            // A SHORT segment is a FAULT, not a short read (root cause of the nPts
            // 32768 coherence hold, 2026-08-03). At nfft 32768 the acquisition returned
            // 2304 frames against a 131072-frame drive - every segment, silently - and
            // the concatenated records were then paired sample-for-sample as if they
            // matched. Two individually PERFECT signals produced an uncorrelated FRF
            // and a 0.15 coherence hold, and every earlier check missed it because a
            // short record still passes a spectral test: the drive matched its command,
            // the response carried the driven band, and neither check looked at LENGTH.
            //
            // 2304 = the engine's staging deadman (2048) plus one 256-frame
            // tick, so the pretest was hitting the same AO stall already fixed
            // for the control loop - but here it truncated instead of
            // faulting, because nothing checked.
            // ...but ONLY when the segment offers no reason of its own. A
            // kinematic trip or an engine fault legitimately ends a segment
            // early, and reporting THAT as a pairing fault would bury the real
            // cause behind a length complaint (caught by the (e) kinematic-
            // abort acceptance test, which trips a tight monitor limit during
            // the pretest on purpose).
            // A FAULTED engine stop is an ERROR here too. `loop_tick` already says
            // so (see its step 5.0), but the PRETEST path never got the
            // rule, and `run_plant_segment` deliberately returns the segment
            // rather than judging it. So an engine fault mid-pretest arrived
            // here as nothing more than a short record - and the length guard
            // below then EXCUSED it, because a fault counts as a reason. Two
            // correct-looking pieces of caution composed into silence: the
            // record was truncated at the staging cap, concatenated, and paired
            // sample-for-sample with a full drive (2026-08-03).
            if let Some(msg) = seg.faulted.clone() {
                return Err(ServiceError::Plant(format!(
                    "pretest segment {ran} faulted: {msg}"
                )));
            }
            let unexplained = seg.watchdog_trip.is_none();
            if r.len() != drive.len() && unexplained {
                return Err(ServiceError::Plant(format!(
                    "pretest segment {ran} returned {} response samples against a {} \
                     sample drive (nfft {}). A truncated acquisition cannot be paired \
                     with a full drive record - the FRF built from it would be \
                     meaningless and the coherence gate would read it as a bad setup \
                     rather than a short read.",
                    r.len(),
                    drive.len(),
                    self.loop_cfg.nfft
                )));
            }
            all_resp.extend(r);
            all_drive.extend(drive);
            ran += 1;
        }
        if all_drive.len() < min_record {
            self.log.push(EventKind::Note {
                text: format!(
                    "pretest record {} samples is short of the {} wanted for lag                      alignment (extension capped at {:.0} s); the gate judges this                      estimate as measured",
                    all_drive.len(),
                    min_record,
                    MAX_EXTEND_S
                ),
            });
        }
        // STEP 2 - INSTRUMENTATION ONLY, no fix (four hypotheses have
        // already died on this defect; the deliverable here is a NUMBER).
        //
        // The question nobody has asked: is the SYNTHESIZED DRIVE at this
        // block size what we asked for? Everything measured so far compared
        // drive against response together, so a synthesis fault and an
        // acquisition fault look identical. This compares the drive against
        // its OWN command and needs no response at all.
        //
        // Measured on the LAST pretest segment, not the first:
        // the first is the COLA start-up ramp and would read low at EVERY
        // block size, for a reason that is not the bug.
        //
        // PASS CONDITION, stated before running it: the ratio
        // reads ~1.0 at nfft 8192 (known good). If it ALSO reads ~1.0 at
        // 32768, synthesis is EXONERATED and the next look is the response
        // record - not another synthesis theory.
        {
            let tail = all_drive.len().saturating_sub(self.loop_cfg.window_len());
            let last_seg = &all_drive[tail..];
            if last_seg.len() >= nbins {
                let (_f, dpsd) = hertzforge_dsp::spectrum::psd_welch(
                    last_seg,
                    self.loop_cfg.fs_hz,
                    self.loop_cfg.nfft,
                    self.loop_cfg.overlap,
                );
                let band_rms = |p: &[f64]| -> f64 {
                    let hi = band.1.min(p.len());
                    if band.0 >= hi {
                        return 0.0;
                    }
                    (p[band.0..hi].iter().sum::<f64>() * df).max(0.0).sqrt()
                };
                let commanded = band_rms(&pretest_psd);
                let realized = band_rms(&dpsd);
                let ratio = if commanded > 0.0 {
                    realized / commanded
                } else {
                    f64::NAN
                };
                self.log.push(EventKind::Note {
                    text: format!(
                        "sysID drive check (nfft {}): commanded band-RMS {commanded:.6},                          realized {realized:.6}, ratio {ratio:.4} (1.0 = synthesis                          delivers what was asked)",
                        self.loop_cfg.nfft
                    ),
                });
            }
        }

        // STEP 3. Synthesis is EXONERATED (drive/command ratio 1.0076 at
        // nfft 32768 vs 1.0131 at 8192 - measured 2026-08-03), so the fault is
        // downstream of the drive. This is the response half of the same
        // question: does the RESPONSE contain the band we drove?
        //
        // On a jumpered loop the response IS the drive, so band-RMS should
        // track the drive's within the loopback gain and the in-band/total
        // ratio should be near 1. A response whose energy sits OUTSIDE the
        // driven band would mean the acquisition is not looking where we think
        // (a rate/grid mismatch), which is the remaining family after
        // synthesis, lag range, and the COLA ramp were each refuted.
        {
            let tail = all_resp.len().saturating_sub(self.loop_cfg.window_len());
            let last_seg = &all_resp[tail..];
            if last_seg.len() >= nbins {
                let (_f, rpsd) = hertzforge_dsp::spectrum::psd_welch(
                    last_seg,
                    self.loop_cfg.fs_hz,
                    self.loop_cfg.nfft,
                    self.loop_cfg.overlap,
                );
                let hi = band.1.min(rpsd.len());
                let inband: f64 = if band.0 < hi {
                    rpsd[band.0..hi].iter().sum()
                } else {
                    0.0
                };
                let total: f64 = rpsd.iter().sum();
                let frac = if total > 0.0 {
                    inband / total
                } else {
                    f64::NAN
                };
                self.log.push(EventKind::Note {
                    text: format!(
                        "sysID response check (nfft {}): in-band RMS {:.6}, total RMS \
                         {:.6}, in-band fraction {frac:.4} (near 1.0 = the response \
                         carries the band we drove)",
                        self.loop_cfg.nfft,
                        (inband * df).max(0.0).sqrt(),
                        (total * df).max(0.0).sqrt()
                    ),
                });
            }
        }

        let mut last_drive = all_drive;
        let mut last_resp = all_resp;

        // ALIGN drive and response by the pipeline lag - MEASURED, not derived -
        // before estimating.
        //
        // On a hardware station the response trails the drive by the output
        // lookahead, the device group delay, AND device/USB FIFO depths no caps
        // field reports. H and coherence correlate the two SIGNALS, so without the
        // shift every nfft window pairs the drive with a DIFFERENT random segment
        // and in-band coherence on a perfectly coherent jumpered wire reads ~0.04 -
        // what the first remote Random run measured (2026-08-01); TWR fixed
        // the identical failure at its 0.010. Deriving the lag
        // (`PlantLoop::pipeline_lag_frames`) was tried first and UNDERSHOOTS on the
        // real cDAQ: derived 295 frames, xcorr truth 1449 (`ni_lag_xcorr`,
        // 2026-08-01) - the committed device queue is invisible to caps. So sysID
        // measures its own lag from the signals it already has: the pretest is
        // known noise, and the xcorr peak IS the pipeline. The Sim plant peaks at 0
        // and the shift is a no-op, which is why every Sim gate passed while the
        // hardware gate held. Gate 6's "RUN CLEAN" evidence never exercised this
        // estimate - a result that cannot fail is not evidence.
        //
        // A peak that is not decisively above the correlation floor leaves the
        // alignment OFF: a wrong confident shift would manufacture coherence where
        // there is none, and the gate's job is to refuse exactly that run.
        let band_hi_hz = band.1 as f64 * df;
        let (lag, peak_r, searched) = measured_pipeline_lag(
            &last_drive,
            &last_resp,
            self.loop_cfg.fs_hz,
            band_hi_hz,
            self.loop_cfg.window_len(),
        );
        // Instrumentation: the alignment is the single point where a
        // healthy jumper turns into an unusable FRF, and until now its inputs
        // were invisible. Peak |r| near 0.8 is a wired path; near the 0.2
        // threshold means the search did not find the pipeline and the
        // estimate that follows is worthless.
        // Needs this later for drive-referenced FRF pairs.
        self.sysid_lag_frames = lag;
        self.log.push(EventKind::Note {
            text: format!(
                "sysID align: record {} samples, nfft {}, lag {lag} frames, peak |r|                  {peak_r:.3}, searched {searched} lags",
                last_drive.len(),
                self.loop_cfg.nfft
            ),
        });
        if lag > 0 && lag < last_resp.len() {
            last_resp.drain(..lag);
            last_drive.truncate(last_resp.len());
        }

        // H + coherence from the last pretest segment.
        let (h, coh) = hertzforge_dsp::frf::h_estimate_hv_with_coherence(
            &last_drive,
            &last_resp,
            self.loop_cfg.fs_hz,
            self.loop_cfg.nfft,
        );
        self.h_mag2 = h.iter().map(|&(re, im)| re * re + im * im).collect();
        if self.h_mag2.len() < nbins {
            self.h_mag2.resize(nbins, 0.0);
        }
        self.coherence = coh;
        if self.coherence.len() < nbins {
            self.coherence.resize(nbins, 0.0);
        }

        // Band coherence = mean coherence over the in-band window.
        let (lo, hi) = band;
        let n = (hi - lo).max(1);
        self.band_coherence = self.coherence[lo..hi.min(self.coherence.len())]
            .iter()
            .sum::<f64>()
            / n as f64;

        // Seed the control law from the sysID H.
        let mut law = HinvLaw::new(self.cfg.hinv.clone(), &self.full_target, band, df)
            .map_err(|e| ServiceError::Config(ConfigError::BadControl(e.to_string())))?;
        let empty: Vec<bool> = Vec::new();
        let seed_inputs = LoopInputs {
            target_psd: &self.full_target,
            measured_psd: &vec![0.0; nbins],
            h_mag2: &self.h_mag2,
            coherence: &self.coherence,
            current_level_db: 0.0,
            df_hz: df,
            notched_bins: &empty,
            band,
        };
        let _ = law.init(&seed_inputs);
        self.law = Some(law);

        let _ = df; // silence if the grid becomes implicit later
        self.transition(RunState::CoherenceGate);
        Ok(())
    }

    /// The coherence gate. If the band coherence is at or above the floor,
    /// engage the loop at the START level and transition `CoherenceGate -> Engaged`.
    /// If below, PARK at `CoherenceHold` (report + event, NEVER auto-raise). Returns
    /// `true` if the gate passed (engaged), `false` if it held.
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
            // The hold is unbounded: stop the device tasks so the AO cannot
            // drain-and-halt and the AI cannot overrun while parked. The next
            // segment's arm/start restarts them (see PlantLoop::suspend).
            if let Some(p) = self.plant_loop.as_mut() {
                p.suspend();
            }
            return Ok(false);
        }
        // HOLD for operator review before anything drives, unless the
        // test bypassed it. The safety gates above have already run and passed
        // - this is the deliberate stop-and-look, not a fault - so the run sits
        // at AwaitingReview until `proceed_after_review()` (or is abandoned by
        // the ordinary stop path).
        if self.review_after_sysid {
            self.log.push(EventKind::Note {
                text: "SYSID COMPLETE - holding for operator review. The FRF and \
                       coherence are final; nothing drives until Proceed. Disable \
                       'Pause after sysID for review' in Control to let a repeat \
                       test run straight through."
                    .into(),
            });
            self.transition(RunState::AwaitingReview);
            // Same as the coherence hold: an operator-length pause with the
            // tasks Running is what produced the -200279 AI overrun 11 ms
            // after Proceed (2026-08-02). Park the hardware with the run.
            if let Some(p) = self.plant_loop.as_mut() {
                p.suspend();
            }
            return Ok(false);
        }
        self.engage_at_start();
        Ok(true)
    }

    /// Engage the loop at the schedule's start level. Shared by the direct path
    /// and the post-review resume so the two cannot drift.
    fn engage_at_start(&mut self) {
        // FULL suspend/resume at the sysID -> engage seam, not just a software
        // re-prime. During the gate window (lag xcorr + H estimate + law
        // seeding) the acquisition keeps running with nobody reading: the AI
        // backlog grows by fs x gate_ms frames. Resuming lockstep then
        // consumes that backlog in a burst - the engine produces drive faster
        // than the AO drains at fs, the excess stages, and the 2048-frame
        // staging deadman (125 ms at fs 16384) trips on the first control
        // segments. Measured 2026-08-03: the mark-only variant died at every
        // block size while the boundary traces showed a healthy device
        // (space_before ~ the gap drain) - the backlog, not the AO, was the
        // killer; the 2026-08-02 fs-8192 passes simply sat under the budget.
        //
        // The stale backlog is HISTORY, not demand. Stopping the tasks
        // discards it (and the AO tail) at the driver; the next segment's
        // arm/start restarts clean - `arm()` accepts Stopped, and the cold
        // prologue loads the head of the next segment's REAL drive before
        // `start()` (the 2026-08-08 cold-start fix), so the resume plays drive from its
        // first clocked sample; what remains of the old "bounded quiet seam"
        // is only the task-restart gap itself. After a review hold `suspend()`
        // already ran at hold entry and this second call is a no-op.
        if let Some(p) = self.plant_loop.as_mut() {
            p.suspend();
        }
        // Two windows of settle: the first is the restart transient itself,
        // the second may straddle it (the estimator's overlap reaches back).
        self.engage_settle_loops = 2;
        self.control_est = None;
        // SOFT START (operator, 2026-08-03): engage `soft_start_db` below the
        // LOWEST scheduled level and walk up to the start level, so a big
        // article - a slip table especially - is taken hold of gently rather
        // than arrived at. Below `soft_start_floor_db` the envelope is judged
        // with a relaxed band (see `envelope_relax_db`); absolute protections
        // are untouched the whole way.
        let lowest = self
            .cfg
            .schedule
            .table_db
            .iter()
            .copied()
            .fold(self.cfg.schedule.start_db, f64::min);
        let soft = self.cfg.soft_start_db.max(0.0);
        if soft > 0.0 {
            let floor = lowest - soft;
            self.soft_start_floor_db = Some(floor);
            // Engage AT the floor, then ramp to the start level. The ramp rate
            // is the operator's level-ramp when they set one; otherwise a
            // deliberate default so the soft start is a RAMP, never a step.
            self.apply_level_now(floor);
            self.schedule_level_db = floor;
            self.level_target_db = self.cfg.schedule.start_db;
            let ramp_s = if self.cfg.level_ramp_s > f64::EPSILON {
                self.cfg.level_ramp_s
            } else {
                DEFAULT_SOFT_START_S
            };
            self.level_ramp_db_per_s =
                ((self.cfg.schedule.start_db - floor).abs() / ramp_s).max(0.1);
            self.log.push(EventKind::Note {
                text: format!(
                    "SOFT START: engaging at {floor:.1} dB ({soft:.0} dB below the lowest                      scheduled level) and ramping to {:.1} dB over {ramp_s:.0} s; envelope                      enforcement relaxed by {:.0} dB until the start level is reached",
                    self.cfg.schedule.start_db, self.cfg.soft_start_relax_db
                ),
            });
        } else {
            self.soft_start_floor_db = None;
            self.set_schedule_level(self.cfg.schedule.start_db, true);
        }
        self.level_index = 0;
        self.transition(RunState::Engaged);
    }

    /// Extra dB of envelope headroom in force RIGHT NOW.
    ///
    /// Non-zero only while a soft start is still climbing to the start level.
    /// It widens the ALARM and ABORT bands; it does not touch the kinematic
    /// limits, the drive-Vrms clamp/abort or the overload latch, which judge
    /// the hardware rather than an estimate against an authored envelope.
    fn envelope_relax_db(&self) -> f64 {
        match self.soft_start_floor_db {
            Some(_) if self.schedule_level_db < self.cfg.schedule.start_db - 1e-9 => {
                self.cfg.soft_start_relax_db.max(0.0)
            }
            _ => 0.0,
        }
    }

    /// Set the SOFT-START depth in dB below the lowest scheduled level
    /// (2026-08-03). `0` disables it. Configure-time.
    pub fn set_soft_start_db(&mut self, db: f64) {
        self.cfg.soft_start_db = db.max(0.0);
    }

    /// Whether this run holds after sysID for operator review.
    pub fn review_after_sysid(&self) -> bool {
        self.review_after_sysid
    }

    /// Set the review-hold policy. Configure-time; a run already holding is
    /// released with [`Self::proceed_after_review`], never by flipping this.
    pub fn set_review_after_sysid(&mut self, on: bool) {
        self.review_after_sysid = on;
    }

    /// Operator reviewed the sysID and chose to PROCEED.
    ///
    /// Valid only from [`RunState::AwaitingReview`]; from anywhere else this is
    /// a refusal rather than a no-op, because "proceed" arriving in a state that
    /// cannot proceed means the two ends disagree about what is happening.
    pub fn proceed_after_review(&mut self) -> Result<(), ServiceError> {
        if !matches!(self.state, RunState::AwaitingReview) {
            return Err(ServiceError::BadState(format!(
                "proceed_after_review from {:?}",
                self.state
            )));
        }
        self.log.push(EventKind::Note {
            text: "operator reviewed the sysID and proceeded".into(),
        });
        self.engage_at_start();
        Ok(())
    }

    // --- the level ladder ------------------------------------------------------

    /// Set the schedule level (the feedforward rung). Re-targets the law's
    /// servo to `full_target_rms * 10^(level_db/20)` and its feedforward scale to match,
    /// resets the level dwell clock, logs the level change, and (for capture) opens the
    /// debounce for a new level entry.
    /// Apply a commanded level RIGHT NOW: scale the estimate, retarget the
    /// servo, restart the divergence accounting. The RAMP walks through here
    /// step by step; the bookkeeping that belongs to the operator's COMMAND
    /// (history, dwell clock, capture debounce) lives in
    /// [`Self::set_schedule_level`] and happens once per command.
    fn apply_level_now(&mut self, level_db: f64) {
        let from = self.schedule_level_db;
        self.schedule_level_db = level_db;
        let scale = 10f64.powf(level_db / 20.0);
        // The accumulated control estimate is level-proportional POWER: scale
        // it with the rung instead of letting it lag the step (an unscaled EMA
        // would read a +6 dB step as "-6 dB low" for ~1/alpha loops and feed
        // both the envelope and the law a fiction).
        if (level_db - from).abs() > 1e-12 {
            let pow_ratio = 10f64.powf((level_db - from) / 10.0);
            if let Some(est) = self.control_est.as_mut() {
                for p in est.iter_mut() {
                    *p *= pow_ratio;
                }
            }
        }
        let full_rms = self.full_target_grms();
        if let Some(law) = self.law.as_mut() {
            // Re-target the law's scalar servo to the scheduled rung and set the
            // feedforward scale so the drive starts at the rung (the servo trims around
            // it). This is the ONLY place the service reaches the servo.
            law.retarget_level(full_rms * scale, scale);
        }
        // The service's own consecutive-divergent-loop counter restarts with the
        // law's per-bin streaks: same rationale, one level up (see
        // `HinvLaw::retarget_level`).
        self.divergence_streak = 0;
    }

    fn set_schedule_level(&mut self, level_db: f64, auto: bool) {
        let from = self.schedule_level_db;
        self.level_target_db = level_db;
        // RAMPED when the operator configured a ramp and the loop is driving:
        // the level WALKS to the target in per-segment steps (see loop_tick).
        // Instant otherwise - including every non-driving state, where there
        // is no loop to walk it.
        let ramping = self.cfg.level_ramp_s > f64::EPSILON
            && matches!(self.state, RunState::Engaged | RunState::FullLevel)
            && (level_db - from).abs() > 1e-9;
        if ramping {
            self.level_ramp_db_per_s = ((level_db - from).abs() / self.cfg.level_ramp_s).max(0.1);
        } else {
            self.apply_level_now(level_db);
        }
        self.elapsed_at_level_s = 0.0;
        self.level_history.push(LevelHistoryEntry {
            level_db,
            auto,
            at_loop: self.loop_count,
        });
        self.log.push(EventKind::LevelChange {
            from_db: from,
            to_db: level_db,
            auto,
        });
        // A new level entry starts the capture debounce: after level_settle_s of
        // settling, the AUTO capture window opens (and RESTARTS if re-entered).
        if let CaptureTrigger::AutoFirstSeconds { .. } = self.cfg.capture.trigger {
            self.level_settle_remaining_s = self.cfg.capture.level_settle_s;
            self.auto_capture_remaining_s = 0.0;
        }
    }

    /// A CONTROLLED stop: ramp the level down over `shutdown_ramp_s`, then
    /// stop. Falls back to an immediate stop when no ramp is configured or
    /// nothing is driving. Aborts NEVER come through here - a protection trip
    /// stops now, and softening it would trade the article for politeness.
    pub fn request_stop(&mut self) -> Result<(), ServiceError> {
        if self.cfg.shutdown_ramp_s > f64::EPSILON
            && matches!(self.state, RunState::Engaged | RunState::FullLevel)
        {
            let down_db = self.schedule_level_db - 40.0;
            self.level_target_db = down_db;
            self.level_ramp_db_per_s = (40.0 / self.cfg.shutdown_ramp_s).max(0.1);
            self.stop_after_ramp = true;
            self.log.push(EventKind::Note {
                text: format!(
                    "controlled stop: ramping down over {:.1} s, then stopping",
                    self.cfg.shutdown_ramp_s
                ),
            });
            Ok(())
        } else {
            self.stop()
        }
    }

    /// Advance the AUTO ladder one rung if the current level's dwell has elapsed. When
    /// the last (full) rung is reached, transition `Engaged -> FullLevel`. No-op in
    /// MANUAL mode or non-Engaged states. Returns `true` if a rung advanced.
    fn maybe_advance_auto(&mut self) -> bool {
        if !matches!(self.state, RunState::Engaged)
            || !matches!(self.cfg.schedule.mode, RunMode::Auto)
        {
            return false;
        }
        // A between-rungs resume dwells at the OPERATOR's level first,
        // governed by the rung BELOW's dwell - the level they are "at" - not
        // the target rung's. The distinction bit on real hardware 2026-08-02:
        // the UI gives the FULL rung a zero dwell (its hold is the full-level
        // duration), so a resume from -2 dB consulted dwell_for(full) = 0 and
        // jumped to full level after ONE tick instead of dwelling.
        let dwell_index = if self.auto_resume_at_rung {
            self.level_index.saturating_sub(1)
        } else {
            self.level_index
        };
        let dwell = self.cfg.schedule.dwell_for(dwell_index);
        if self.elapsed_at_level_s < dwell {
            return false;
        }
        // A live MANUAL -> AUTO takeover left the level BETWEEN rungs: this
        // dwell-elapse advances TO the nearest rung above, then the
        // ladder proceeds normally.
        if self.auto_resume_at_rung {
            self.auto_resume_at_rung = false;
            let rung = self.cfg.schedule.table_db[self.level_index];
            self.set_schedule_level(rung, true);
            return true;
        }
        // Dwell elapsed: advance to the next table rung, or enter FullLevel if at the
        // top.
        if self.level_index + 1 < self.cfg.schedule.table_db.len() {
            self.level_index += 1;
            let next = self.cfg.schedule.table_db[self.level_index];
            self.set_schedule_level(next, true);
            true
        } else {
            // At the full rung: hold for the test duration.
            self.transition(RunState::FullLevel);
            self.elapsed_at_level_s = 0.0;
            false
        }
    }

    /// LIVE ladder-mode takeover, both directions (the operator's spec,
    /// 2026-08-02: "the operator needs to be able to interrupt auto to take over,
    /// and then switch back to auto if desired").
    ///
    /// **The transition is LEVEL-NEUTRAL in both directions** - taking over must
    /// never move the shaker:
    ///
    /// - `AUTO -> MANUAL`: the schedule freezes exactly where it is (the dwell
    ///   timers simply stop being consulted); the level verbs drive from here.
    /// - `MANUAL -> AUTO`: the ladder re-engages FROM THE LEVEL THE OPERATOR IS
    ///   AT - never from the top of the schedule. On a rung: that rung, dwell
    ///   restarted. Between rungs: the dwell runs at the operator's level, then
    ///   the ladder advances TO the nearest rung above (`auto_resume_at_rung`).
    ///   At or above full: FullLevel, full-duration clock restarted.
    ///
    /// Valid mid-run (Engaged / FullLevel). Outside a live run it simply records
    /// the mode for the next start (mode is test data at configure time; this
    /// keeps the two writers agreeing).
    pub fn set_run_mode(&mut self, mode: RunMode) -> Result<(), ServiceError> {
        if self.cfg.schedule.mode == mode {
            return Ok(());
        }
        if !matches!(self.state, RunState::Engaged | RunState::FullLevel) {
            // Not live: a plain configure-time mode change.
            self.cfg.schedule.mode = mode;
            self.auto_resume_at_rung = false;
            return Ok(());
        }
        match mode {
            RunMode::Manual => {
                self.cfg.schedule.mode = RunMode::Manual;
                self.auto_resume_at_rung = false;
                self.log.push(EventKind::Note {
                    text: format!(
                        "LADDER MODE auto -> manual (operator takeover); holding {:.2} dB",
                        self.schedule_level_db
                    ),
                });
            }
            RunMode::Auto => {
                let table = &self.cfg.schedule.table_db;
                let full = self.cfg.schedule.full_db();
                let eps = 1e-9;
                // Nearest rung AT the current level, else the nearest rung above
                // (advance-to-rung resume). At/above full: hold full.
                let on_rung = table
                    .iter()
                    .position(|&t| (t - self.schedule_level_db).abs() < eps);
                match on_rung {
                    Some(i) => {
                        self.level_index = i;
                        self.auto_resume_at_rung = false;
                    }
                    None => match table.iter().position(|&t| t > self.schedule_level_db + eps) {
                        Some(above) => {
                            self.level_index = above;
                            self.auto_resume_at_rung = true;
                        }
                        None => {
                            // Above every rung (past full): treat as full.
                            self.level_index = table.len().saturating_sub(1);
                            self.auto_resume_at_rung = false;
                        }
                    },
                }
                self.cfg.schedule.mode = RunMode::Auto;
                // Restart the dwell (or the full-duration clock) from the
                // takeover instant - re-engaging must never inherit a stale
                // partial dwell.
                self.elapsed_at_level_s = 0.0;
                if self.schedule_level_db >= full - eps {
                    self.transition(RunState::FullLevel);
                } else {
                    self.transition(RunState::Engaged);
                }
                self.log.push(EventKind::Note {
                    text: format!(
                        "LADDER MODE manual -> auto; re-engaged at {:.2} dB (rung index {}{}), dwell restarted",
                        self.schedule_level_db,
                        self.level_index,
                        if self.auto_resume_at_rung {
                            ", advancing to the rung above on dwell elapse"
                        } else {
                            ""
                        }
                    ),
                });
            }
        }
        Ok(())
    }

    /// MANUAL: step the level by `delta_db` (the `+/- 1 dB` buttons). Clamps to
    /// the table's `[first, last]` dB range. Valid only in Engaged/FullLevel MANUAL.
    /// Returns the new level dB.
    pub fn step_level(&mut self, delta_db: f64) -> Result<f64, ServiceError> {
        if !matches!(self.cfg.schedule.mode, RunMode::Manual) {
            return Err(ServiceError::BadState(
                "step_level requires MANUAL mode".into(),
            ));
        }
        if !matches!(self.state, RunState::Engaged | RunState::FullLevel) {
            return Err(ServiceError::BadState(format!(
                "step_level from {:?}",
                self.state
            )));
        }
        let lo = *self
            .cfg
            .schedule
            .table_db
            .first()
            .unwrap_or(&self.cfg.schedule.start_db);
        let hi = self.cfg.schedule.full_db();
        let target = (self.schedule_level_db + delta_db).clamp(lo, hi);
        self.set_schedule_level(target, false);
        // A manual step below full drops back to Engaged; at full it is FullLevel.
        if target >= hi - 1e-9 {
            self.transition(RunState::FullLevel);
        } else {
            self.transition(RunState::Engaged);
        }
        Ok(target)
    }

    /// MANUAL: jump to table level index `i` (jump-to-level from the table).
    /// Valid only in Engaged/FullLevel MANUAL. Returns the new level dB.
    pub fn jump_to_table_level(&mut self, i: usize) -> Result<f64, ServiceError> {
        if !matches!(self.cfg.schedule.mode, RunMode::Manual) {
            return Err(ServiceError::BadState(
                "jump_to_table_level requires MANUAL mode".into(),
            ));
        }
        if !matches!(self.state, RunState::Engaged | RunState::FullLevel) {
            return Err(ServiceError::BadState(format!(
                "jump_to_table_level from {:?}",
                self.state
            )));
        }
        let level = *self
            .cfg
            .schedule
            .table_db
            .get(i)
            .ok_or_else(|| ServiceError::BadState(format!("no table level {i}")))?;
        self.level_index = i;
        self.set_schedule_level(level, false);
        if level >= self.cfg.schedule.full_db() - 1e-9 {
            self.transition(RunState::FullLevel);
        } else {
            self.transition(RunState::Engaged);
        }
        Ok(level)
    }

    // --- the loop tick (closed-loop step + all protections) -------------------------

    /// Advance the closed loop one segment: synthesize the drive from the law's PSD,
    /// apply the notch ceiling (notch seam, AFTER synthesis), run one plant segment,
    /// measure + composite the control channels, call `law.iterate`, then run every
    /// protection engine. Advances the level ladder (AUTO) and the capture/save clocks.
    /// Returns `true` if the run is still running, `false` if it stopped/aborted this
    /// tick.
    ///
    /// Valid in Engaged / FullLevel. In MANUAL the caller paces the ticks; in AUTO the
    /// tick advances the ladder + the full-duration stop.
    /// Install the per-tick DISPLAY tap, so a host can stream the time
    /// domain at the ENGINE's tick rate instead of once per control segment.
    /// Safe to call before OR during a run; `None` clears it. Remembered across
    /// arm, since the plant loop is (re)built there.
    pub fn set_block_observer(&mut self, o: Option<engine::BlockObserver>) {
        self.block_observer = o.clone();
        if let Some(pl) = self.plant_loop.as_mut() {
            pl.set_block_observer(o);
        }
    }

    pub fn loop_tick(&mut self) -> Result<bool, ServiceError> {
        if !matches!(self.state, RunState::Engaged | RunState::FullLevel) {
            return Err(ServiceError::BadState(format!(
                "loop_tick from {:?}",
                self.state
            )));
        }
        let nbins = self.loop_cfg.nbins();
        let band = self.band;
        let df = self.df_hz;
        // Everything from here to the next acquisition is AO-unfed.
        self.pre_segment_start = Some(std::time::Instant::now());

        // 1. The law's current drive PSD (built from its shape * servo scale).
        let mut drive_psd = self
            .law
            .as_ref()
            .map(|l| l.drive_psd())
            .unwrap_or_else(|| vec![0.0; nbins]);

        // 2. Build the notch from the limit profiles (notch seam) and apply AFTER synthesis.
        let notch = self.build_notch();
        let notch_report: NotchReport = notch.apply(&mut drive_psd);
        self.notch_mask = notch_report.notched.clone();
        self.last_notch_lines = notch_report.notched_lines.len();

        // 3. Drive-Vrms ceiling clamp: clamp the drive PSD so its
        //    realized Vrms does not exceed the ceiling; a hard breach beyond the abort
        //    threshold aborts. The Vrms is sqrt(sum(psd)*df) over the whole grid.
        self.clamp_drive_vrms(&mut drive_psd);
        let realized_vrms = Self::psd_vrms(&drive_psd, df);
        if let Some(abort_v) = self.cfg.drive_abort_vrms {
            if realized_vrms >= abort_v {
                self.abort(TripKind::DriveVrms {
                    realized_vrms,
                    limit_vrms: abort_v,
                });
                return Ok(false);
            }
        }
        self.last_drive_psd = Some(drive_psd.clone());

        // 4. Synthesize the drive stream + run one plant segment.
        // Phase timing: synthesis is the LAST thing before acquisition
        // resumes, so it sits squarely inside the AO-unfed window.
        // LEVEL RAMP walker: one bounded step per segment toward the
        // commanded target, feedforward-retargeted each step - smooth by
        // construction, never stepwise (a step into an armature is a
        // broadband impulse). When a controlled stop armed the ramp-down,
        // reaching the floor stops the run.
        if (self.schedule_level_db - self.level_target_db).abs() > 1e-9 {
            let step = (self.level_ramp_db_per_s * self.loop_cfg.segment_seconds()).max(1e-3);
            let delta = self.level_target_db - self.schedule_level_db;
            let next = if delta.abs() <= step {
                self.level_target_db
            } else {
                self.schedule_level_db + step.copysign(delta)
            };
            self.apply_level_now(next);
            // A moving level re-arms a short judgment settle: the estimate is
            // being rescaled every segment and the envelope must not chase it.
            self.engage_settle_loops = self.engage_settle_loops.max(1);
        } else if self.soft_start_floor_db.is_some()
            && self.schedule_level_db >= self.cfg.schedule.start_db - 1e-9
        {
            // Arrived: full enforcement from here on.
            self.soft_start_floor_db = None;
            self.log.push(EventKind::Note {
                text: "soft start complete: at the start level, full envelope enforcement".into(),
            });
        } else if self.stop_after_ramp {
            self.stop_after_ramp = false;
            self.stop()?;
            return Ok(false);
        }

        let t_synth = std::time::Instant::now();
        let (mut leadin, mut drive) = self.synth_segment(&drive_psd)?;
        // Sigma clip (operator "max sigma"): bound the crest factor delivered
        // to the amplifier. Applied AFTER synthesis, BEFORE the plant - the
        // clip is part of the drive the loop then measures through.
        if let Some(sig) = self.cfg.drive_clip_sigma {
            Self::clip_sigma(&mut leadin, sig);
            Self::clip_sigma(&mut drive, sig);
        }
        let drive = drive;
        if !self.frf_pairs.is_empty() {
            self.last_drive_record = drive.clone();
        }
        let synth_ms = t_synth.elapsed().as_secs_f64() * 1e3;
        let t_pre = self.pre_segment_start.take();
        let seg = self.run_plant_segment(leadin, drive)?;
        if let Some(t) = self.last_segment_timing.as_mut() {
            t.synth_ms = synth_ms;
            t.pre_ms = t_pre
                .map(|x| x.elapsed().as_secs_f64() * 1e3)
                .unwrap_or(f64::NAN);
        }
        // DISPLAY TAP (remote raw stream, 2026-08-01): keep the segment's
        // interleaved samples so a host can stream the real time-domain view.
        // Reused buffer; ~0.4 MB at the demo grid. Read via `last_segment()`.
        self.last_segment_interleaved.clear();
        self.last_segment_interleaved
            .extend(seg.interleaved.iter().map(|&v| v as f32));
        self.last_segment_n_ch = seg.n_ch;
        self.loop_count += 1;
        self.elapsed_at_level_s += self.loop_cfg.segment_seconds();

        // 5.0 A FAULTED engine stop is an ERROR, never a quiet segment. Previously
        //     every stop except a watchdog trip was discarded, so a segment that
        //     stalled, overran the raw ring, lost the link or underran its drive came
        //     back indistinguishable from a clean one and the control law happily fed
        //     on whatever partial response it produced.
        if let Some(msg) = seg.faulted.clone() {
            return Err(ServiceError::Plant(msg));
        }

        // 5a. The engine's RAW-RING watchdog is the PRIMARY tri-field kinematic abort
        //     (it reads the raw ring before decimation, faster than a post-segment
        //     check). If it tripped this segment, surface it as a Kinematic trip.
        if let Some(t) = seg.watchdog_trip {
            let ktrip = KinematicTrip {
                channel_index: t.channel_index,
                axis: format!("{:?}", t.axis),
                value: t.value,
                limit: t.limit,
                at_frame: t.at_frame,
                at_level_db: self.schedule_level_db,
                at_loop: self.loop_count,
            };
            self.abort(TripKind::from_kinematic(ktrip));
            return Ok(false);
        }

        // 5b. The SERVICE's post-segment kinematic monitor is the SECONDARY check over
        //     the recorded interleaved channels (same hertzforge_engine::limits tri-field,
        //     with a settle window suppressing derived-axis startup transients). It
        //     catches anything the raw-ring watchdog is not configured for.
        if let Some(kin) = self.kinematic.as_mut() {
            if let Some(t) = kin.feed(&seg.interleaved) {
                let ktrip = KinematicTrip {
                    channel_index: t.channel_index,
                    axis: format!("{:?}", t.axis),
                    value: t.value,
                    limit: t.limit,
                    at_frame: t.at_frame,
                    at_level_db: self.schedule_level_db,
                    at_loop: self.loop_count,
                };
                self.abort(TripKind::from_kinematic(ktrip));
                return Ok(false);
            }
        }

        // 6. Capture the raw interleaved chunk (never blocks - drops on a full ring).
        self.feed_capture(&seg);

        // 7. Overload proxy per channel: |eu| >= fraction * range_max
        //    latches the per-channel indicator (NOT an abort).
        self.check_overloads(&seg);

        // 8. Measure per-channel PSDs + composite, then iterate the law.
        let per_channel = self.measure_per_channel(&seg);
        let raw_composite = self.composite_of(&per_channel)?;
        self.last_per_channel_psds = per_channel;
        // Fold this loop's raw estimate into the ACCUMULATOR; everything
        // downstream (law, envelope, display, gRMS) consumes the accumulated
        // estimate. See the `control_est` field doc for why raw was wrong.
        // Accumulator depth from the operator's target DOF: reach
        // `control_dof` total degrees of freedom given what one loop already
        // provides (`2 x nAvg`), then convert the window count to an EMA
        // alpha. Clamped so the estimate can never be less smooth than one
        // raw loop, nor so deep that the loop stops adapting.
        let per_loop_dof = (2 * self.loop_cfg.blocks_per_window).max(1) as f64;
        let windows = (self.cfg.control_dof / per_loop_dof).clamp(1.0, 64.0);
        let est_alpha = 2.0 / (windows + 1.0);
        let composite: Vec<f64> = match self.control_est.take() {
            Some(mut est) if est.len() == raw_composite.len() => {
                for (e, &m) in est.iter_mut().zip(raw_composite.iter()) {
                    *e += est_alpha * (m - *e);
                }
                est
            }
            _ => raw_composite.clone(),
        };
        self.control_est = Some(composite.clone());
        self.last_composite_psd = Some(composite.clone());
        self.last_measured_grms = Some(Self::inband_rms(&composite, band, df));

        // ENGAGE SETTLE (2026-08-03). The first control windows after a task
        // restart (engage after the gate, resume after a hold) measure the
        // deliberate startup transient - the AO's zero prefill and the fresh
        // AI task's settling - not the plant. Judging them is wrong in both
        // directions: the envelope's LOW side aborts on the quiet lead-in
        // (measured on the jumper: `Envelope { abort_lines: 1, worst_db:
        // -8.37 }` at loop 1, every run), and the law would over-correct
        // upward against a PSD it is about to stop seeing. So while settling:
        // no law iteration, no envelope judgment, no divergence counting.
        // The ABSOLUTE protections stay live through the settle - kinematic
        // g limits and the drive-Vrms clamp/abort judge the hardware, not the
        // estimate, and a real overtest during settle still aborts.
        if self.engage_settle_loops > 0 {
            self.engage_settle_loops -= 1;
            self.alarm_active = false;
            // A settle window is the restart transient - folding it into the
            // accumulated control estimate would hand the first JUDGED loop an
            // estimate still mostly made of the transient. The accumulator
            // starts from the first clean window instead.
            self.control_est = None;
            self.advance_capture_clocks();
            return Ok(true);
        }

        // The operator's FRF pairs, measured from the SAME segment the
        // control loop just consumed - so what the pane shows and what the
        // loop acted on are the same data, not two nearby acquisitions.
        self.measure_frf_pairs(&seg);

        let inputs = LoopInputs {
            target_psd: &self.full_target,
            measured_psd: &composite,
            h_mag2: &self.h_mag2,
            coherence: &self.coherence,
            current_level_db: self.schedule_level_db,
            df_hz: df,
            notched_bins: &self.notch_mask,
            band,
        };
        let out = self
            .law
            .as_mut()
            .expect("law present while running")
            .iterate(&inputs);

        // 9. Divergence abort (divergence caller contract): count consecutive loops with a
        //    non-empty divergent-bin set; abort at divergence_loops.
        //
        //    A GUARD BAND of EDGE_GUARD_BINS on each band edge is EXCLUDED from
        //    the abort set (first full-ladder hardware run, 2026-08-01):
        //    window leakage at a band edge next to zeroed out-of-band bins makes
        //    the edge bins read ~3.5 dB low on a PERFECT wire plant, the law
        //    pushes them forever, and after `divergence_loops` the run aborts -
        //    every run, at every level. That is an estimator artifact, not a
        //    plant fault. Excluding only the outermost bin was tried first and
        //    the run still died on the SECOND bin from the edge - the leakage
        //    skirt is the analysis window's main lobe, ~2 bins wide, not one.
        //    Guard-band bins are still CONTROLLED (clamped, notched, enveloped);
        //    they just cannot be the sole reason a run dies. Divergence anywhere
        //    in the interior still aborts exactly as before.
        const EDGE_GUARD_BINS: usize = 2;
        let interior_divergent = out
            .diagnostics
            .divergent_bins
            .iter()
            .filter(|&&b| b >= band.0 + EDGE_GUARD_BINS && b + EDGE_GUARD_BINS < band.1)
            .count();
        // Bench diagnostic, env-gated: WHICH bins are diverging, in Hz. The trip
        // record carries only a count, and an earlier false-Divergence hunt burned two
        // guesses on where the divergent bin sat; this prints the answer instead.
        if interior_divergent > 0 && std::env::var_os("HERTZFORGE_DIVERGE_DEBUG").is_some() {
            let hz: Vec<f64> = out
                .diagnostics
                .divergent_bins
                .iter()
                .map(|&b| b as f64 * self.df_hz)
                .collect();
            eprintln!(
                "[diverge-debug] bins {:?} = {:?} Hz (band {:?}, df {:.3})",
                out.diagnostics.divergent_bins, hz, band, self.df_hz
            );
        }
        if interior_divergent == 0 {
            self.divergence_streak = 0;
        } else {
            self.divergence_streak += 1;
            if self.divergence_streak >= self.cfg.divergence_loops {
                self.abort(TripKind::Divergence {
                    consecutive_loops: self.divergence_streak,
                    divergent_bins: interior_divergent,
                });
                return Ok(false);
            }
        }

        // 10. Envelope alarm/abort: check the composite measured PSD
        //     against the frequency-domain envelope.
        if !self.check_envelope(&composite) {
            return Ok(false); // aborted inside check_envelope
        }

        // 11. Capture/save clocks + AUTO ladder advance + full-duration stop.
        self.advance_capture_clocks();
        self.maybe_advance_auto();
        if matches!(self.state, RunState::FullLevel)
            && matches!(self.cfg.schedule.mode, RunMode::Auto)
            && self.elapsed_at_level_s >= self.cfg.full_level_duration_s
        {
            self.stop()?;
            return Ok(false);
        }
        Ok(true)
    }

    /// Run the whole AUTO ladder to completion: sysID -> gate -> engage -> ladder ->
    /// full -> duration -> stop, bounded by `max_ticks` loop ticks (a safety cap).
    /// Returns the final state. If the coherence gate HOLDs, returns early parked at
    /// `CoherenceHold`. Convenience for AUTO callers / tests; MANUAL callers drive the
    /// individual steps.
    pub fn run_auto(
        &mut self,
        pretest_segments: usize,
        max_ticks: u64,
    ) -> Result<RunState, ServiceError> {
        self.assess_feasibility()?;
        // AUTO run_auto only arms the clean path; an exceedance must be handled by the
        // caller via arm_with_override before calling run_auto again from Feasibility.
        if matches!(self.state, RunState::FeasibilityBlocked) {
            return Ok(self.state);
        }
        self.arm()?;
        self.run_sysid(pretest_segments)?;
        if !self.coherence_gate()? {
            return Ok(self.state); // parked at CoherenceHold
        }
        let mut ticks = 0u64;
        while matches!(self.state, RunState::Engaged | RunState::FullLevel) && ticks < max_ticks {
            if !self.loop_tick()? {
                break;
            }
            ticks += 1;
        }
        Ok(self.state)
    }

    // --- protection helpers ---------------------------------------------------------

    /// Build the drive-PSD notch from the configured limit profiles (notch seam). Each
    /// response profile is mapped through `|H|^2` into a drive ceiling; the tightest per
    /// bin wins. An empty profile set yields an unlimited notch.
    fn build_notch(&self) -> NotchSet {
        let nbins = self.loop_cfg.nbins();
        let mut notch = NotchSet::unlimited(nbins);
        let h_floor = self.cfg.hinv.trust_h_floor_frac.powi(2)
            * self.h_mag2[self.band.0..self.band.1.min(self.h_mag2.len())]
                .iter()
                .copied()
                .fold(0.0f64, f64::max);
        for profile in &self.cfg.limit_profiles {
            if profile.resp_ceiling.len() != nbins {
                continue; // grid mismatch: skip (logged at config time in a real UI)
            }
            let _ = notch.add_channel_ceiling(&profile.resp_ceiling, &self.h_mag2, h_floor);
            let _ = profile.kind; // Response and Force notch identically
        }
        notch
    }

    /// Clamp the drive PSD so its realized Vrms does not exceed the ceiling.
    /// Scales the whole PSD down uniformly if over-ceiling (a per-bin clamp would distort
    /// the shape; a uniform scale preserves it, which is the physical drive-limit
    /// behavior).
    fn clamp_drive_vrms(&self, drive_psd: &mut [f64]) {
        let vrms = Self::psd_vrms(drive_psd, self.df_hz);
        let ceiling = self.cfg.drive_ceiling_vrms;
        if vrms > ceiling && vrms > 0.0 {
            let scale2 = (ceiling / vrms).powi(2);
            for p in drive_psd.iter_mut() {
                *p *= scale2;
            }
        }
    }

    /// Check the composite measured PSD against the frequency-domain envelope. An
    /// ALARM breach is surfaced (event + indicator); an ABORT breach stops immediately.
    /// Returns `false` if it aborted.
    ///
    /// The envelope is built from the FULL (0 dB) reference, but the run measures at the
    /// CURRENT scheduled level. So the measured PSD is normalized UP to the full-level
    /// frame (`measured / level_scale^2`) before the check - i.e. the alarm/abort bands
    /// track the scheduled level, which is the standard random-controller behavior (a
    /// -12 dB engage does not spuriously breach the -3 dB abort floor of the full
    /// envelope). At full level the scale is 1 and this is a no-op.
    fn check_envelope(&mut self, composite: &[f64]) -> bool {
        let Some(env) = self.envelope.as_ref() else {
            self.alarm_active = false;
            return true;
        };
        let level_scale2 = 10f64.powf(self.schedule_level_db / 10.0);
        let inv = if level_scale2 > 0.0 {
            1.0 / level_scale2
        } else {
            1.0
        };
        let normalized: Vec<f64> = composite.iter().map(|&p| p * inv).collect();
        let measured = LineSpectrum {
            f0_hz: 0.0,
            df_hz: self.df_hz,
            psd: normalized,
        };
        // Soft-start relaxation: widen the authored bands while the run is
        // still climbing to the start level (see `envelope_relax_db`).
        let relax = self.envelope_relax_db();
        let report = if relax > 0.0 {
            let scale = 10f64.powf(relax / 10.0);
            let widened: Vec<f64> = measured.psd.iter().map(|&p| p / scale).collect();
            let lo = LineSpectrum {
                f0_hz: measured.f0_hz,
                df_hz: measured.df_hz,
                psd: measured.psd.iter().map(|&p| p * scale).collect(),
            };
            // Judge BOTH directions with the band widened by `relax`: a line is
            // only out if it is out even after the allowance.
            let hi_rep = env.check(&LineSpectrum {
                f0_hz: measured.f0_hz,
                df_hz: measured.df_hz,
                psd: widened,
            });
            let lo_rep = env.check(&lo);
            // Keep a breach only where it survives in the direction it claims.
            let mut r = env.check(&measured);
            r.alarm_bins
                .retain(|&b| hi_rep.alarm_bins.contains(&b) || lo_rep.alarm_bins.contains(&b));
            r.abort_bins
                .retain(|&b| hi_rep.abort_bins.contains(&b) || lo_rep.abort_bins.contains(&b));
            r
        } else {
            env.check(&measured)
        };
        self.last_alarm_lines = report.alarm_bins.len();
        self.last_abort_lines = report.abort_bins.len();
        // The abort fires at the operator's line budget (`max_lines_out`),
        // not on the first breached bin: per-line Welch scatter at small nAvg
        // statistically breaches a -6 dB floor on a perfect plant (see the
        // config field's doc for the 2026-08-03 measurement). Below the
        // budget, breaches ride the ALARM indicator instead.
        if report.abort_bins.len() >= self.cfg.max_lines_out.max(1) {
            let worst = report.worst_abort_db.unwrap_or(0.0);
            self.abort(TripKind::Envelope {
                abort_lines: report.abort_bins.len(),
                worst_db: worst,
            });
            return false;
        }
        self.alarm_active = report.alarmed();
        if report.alarmed() {
            self.log.push(EventKind::EnvelopeAlarm {
                lines: report.alarm_bins.len(),
                worst_db: report.worst_alarm_db.unwrap_or(0.0),
            });
        }
        true
    }

    /// Overload proxy per channel: if any raw sample on a channel reaches
    /// `fraction * range_max` in EU, latch that channel's indicator. NOT an abort.
    fn check_overloads(&mut self, seg: &engine::SegmentResult) {
        let frac = self.cfg.overload_fraction;
        // The interleave order matches the enabled-non-drive channel order the latches
        // were built in.
        let n_ch = seg.n_ch;
        if n_ch == 0 {
            return;
        }
        let frames = seg.interleaved.len() / n_ch;
        for (slot, latch) in self.overloads.iter_mut().enumerate() {
            if latch.latched || slot >= n_ch {
                continue;
            }
            let threshold = frac * latch.range_max;
            let mut hit = false;
            for f in 0..frames {
                if seg.interleaved[f * n_ch + slot].abs() >= threshold {
                    hit = true;
                    break;
                }
            }
            if hit {
                latch.latched = true;
                self.log.push(EventKind::OverloadLatch {
                    channel_index: latch.channel_index,
                });
            }
        }
    }

    /// Latch an abort: record the trip, stop the plant, transition to Aborted, and stop
    /// any capture. The single abort path for every protection engine.
    fn abort(&mut self, kind: TripKind) {
        let record = TripRecord {
            kind,
            at_level_db: self.schedule_level_db,
            at_loop: self.loop_count,
        };
        self.log.push(EventKind::Trip {
            record: record.clone(),
        });
        self.trip = Some(record);
        // Best-effort: stop capture (flush the sidecar) and transition.
        let _ = self.stop_capture_inner();
        self.transition(RunState::Aborted);
    }

    /// The control loop DIED on an engine/backend fault.
    ///
    /// Before this existed, a loop-thread fault logged a warning and exited -
    /// and `module_state` stayed `Engaged` for the rest of the session. Nothing
    /// drove, nothing streamed, and the wire said the run was fine: "engaged
    /// with no data" is indistinguishable from "engaged and quiet", which is
    /// exactly how the 2026-08-02 dead-stream run reached the operator as a
    /// mystery instead of a fault banner. A run that has stopped driving must
    /// never be able to report that it is driving.
    ///
    /// This is the terminal transition for that path: the fault text goes into
    /// the event log, the device tasks are parked (the loop is not coming
    /// back), and the state becomes `Aborted` so every consumer - status wire,
    /// run-end banner, report - sees the run END with a reason.
    pub fn fault_from_loop(&mut self, detail: &str) {
        self.log.push(EventKind::Note {
            text: format!("LOOP FAULT (terminal): {detail}"),
        });
        if let Some(p) = self.plant_loop.as_mut() {
            p.suspend();
        }
        let _ = self.stop_capture_inner();
        self.transition(RunState::Aborted);
    }

    // --- capture (capture rules + periodic save) ------------------------------------

    /// Start streaming telemetry capture to `data_root/<run_name>/`. Logs
    /// the start. Manual/EntireTest callers use this directly; AUTO opens/closes it
    /// automatically on the debounced level-entry windows.
    pub fn start_capture(&mut self, run_name: &str) -> Result<PathBuf, ServiceError> {
        if let Some(sink) = self.capture.as_ref() {
            return Ok(sink.run_dir().to_path_buf());
        }
        let n_ch = self
            .plant_loop
            .as_ref()
            .map(|p| p.n_input_channels())
            .unwrap_or(1);
        // Record at the ACTUAL plant/telemetry rate so the sidecar matches the frames
        // on disk. (Was CaptureConfig::decimated(n_ch), which mislabeled these 8192 Hz
        // plant frames as 5120 Hz / decimation 13 - flagged during the recorder work.
        // The sink does not decimate; declare truth.)
        let cfg = CaptureConfig::at_rate(n_ch, self.loop_cfg.fs_hz);
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
            trigger: format!("{:?}", self.cfg.capture.trigger),
        });
        self.capture = Some(sink);
        Ok(run_dir)
    }

    /// Stop the capture, flush the sidecar, and log the drop count. No-op if not
    /// capturing.
    pub fn stop_capture(&mut self) -> Result<(), ServiceError> {
        self.stop_capture_inner()
    }

    fn stop_capture_inner(&mut self) -> Result<(), ServiceError> {
        if let Some(sink) = self.capture.take() {
            let sidecar = sink.finish().map_err(|e| ServiceError::Io(e.to_string()))?;
            self.log.push(EventKind::CaptureStop {
                chunks_written: sidecar.chunks_written,
                dropped: sidecar.chunks_dropped,
            });
        }
        Ok(())
    }

    /// Feed the raw interleaved chunk into an active capture (drops on a full ring).
    fn feed_capture(&mut self, seg: &engine::SegmentResult) {
        if let Some(sink) = self.capture.as_ref() {
            let frames = seg.interleaved.len().checked_div(seg.n_ch).unwrap_or(0);
            let samples: Vec<f32> = seg.interleaved.iter().map(|&v| v as f32).collect();
            sink.push(&samples, frames);
        }
    }

    /// Advance the capture debounce + AUTO window + the periodic-save clock.
    fn advance_capture_clocks(&mut self) {
        let dt = self.loop_cfg.segment_seconds();
        // Periodic snapshot save.
        if let Some(interval) = self.cfg.capture.periodic_save_s {
            if interval > 0.0 {
                self.since_last_snapshot_s += dt;
                if self.since_last_snapshot_s >= interval {
                    self.since_last_snapshot_s = 0.0;
                    let _ = self.save_snapshot(Vec::new());
                }
            }
        }
        // AUTO first-N-seconds capture window: debounce the level entry, then open a
        // window for n_s; a re-entry RESTARTS it (handled in set_schedule_level).
        if let CaptureTrigger::AutoFirstSeconds { n_s } = self.cfg.capture.trigger {
            if self.level_settle_remaining_s > 0.0 {
                self.level_settle_remaining_s -= dt;
                if self.level_settle_remaining_s <= 0.0 {
                    // Settled: open the window (start a fresh capture for this level).
                    self.level_settle_remaining_s = 0.0;
                    self.auto_capture_remaining_s = n_s;
                    let run_name = format!(
                        "auto_level_{:+.0}dB_loop{}",
                        self.schedule_level_db, self.loop_count
                    );
                    // Close any prior AUTO window before opening the new one (a re-entry
                    // RESTARTS: finish the previous file, open a fresh one).
                    let _ = self.stop_capture_inner();
                    let _ = self.start_capture(&run_name);
                }
            } else if self.auto_capture_remaining_s > 0.0 {
                self.auto_capture_remaining_s -= dt;
                if self.auto_capture_remaining_s <= 0.0 {
                    self.auto_capture_remaining_s = 0.0;
                    let _ = self.stop_capture_inner();
                }
            }
        }
    }

    /// Save a periodic snapshot bundle: the current composite/per-channel/
    /// drive PSDs + level + H/coherence + notch/envelope state, plus the caller's marks.
    /// Written into the capture run folder (or `data_root/snapshots`).
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
        let payload = RandomSnapshotPayload {
            composite_psd: self.last_composite_psd.clone().unwrap_or_default(),
            per_channel_psds: self.last_per_channel_psds.clone(),
            drive_psd: self.last_drive_psd.clone().unwrap_or_default(),
            level_db: self.schedule_level_db,
            h_mag2: self.h_mag2.clone(),
            coherence: self.coherence.clone(),
            notch_lines: self.last_notch_lines,
            alarm_lines: self.last_alarm_lines,
            abort_lines: self.last_abort_lines,
            df_hz: self.df_hz,
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

    // --- stop -----------------------------------------------------------------------

    /// Clean stop: finish any capture, transition to Stopped (unless already Aborted).
    pub fn stop(&mut self) -> Result<(), ServiceError> {
        let _ = self.stop_capture_inner();
        if self.state != RunState::Aborted {
            self.transition(RunState::Stopped);
        }
        Ok(())
    }

    // --- measurement + composite ----------------------------------------------------

    /// Measure every requested FRF pair from this segment.
    ///
    /// Uses the SAME estimator sysID uses, so a drive-referenced pair and the
    /// sysID FRF are the same quantity measured at different times rather than
    /// two different definitions of `H`.
    fn measure_frf_pairs(&mut self, seg: &engine::SegmentResult) {
        if self.frf_pairs.is_empty() {
            self.frf_results.clear();
            return;
        }
        let nfft = self.loop_cfg.nfft;
        let fs = self.loop_cfg.fs_hz;
        let lag = self.sysid_lag_frames;
        let drive = self.last_drive_record.clone();
        let mut out = Vec::with_capacity(self.frf_pairs.len());
        for p in &self.frf_pairs {
            let resp = seg.channel(p.response_slot);
            // The REFERENCE: an acquired channel, or the synthesized drive.
            let (mut r, mut y) = match p.reference_slot {
                Some(slot) => (seg.channel(slot), resp),
                None => (drive.clone(), resp),
            };
            // Lag alignment applies ONLY to a drive reference: the
            // drive leads the response by the device queue, but two ACQUIRED
            // channels share one clock and shifting either would manufacture a
            // phase error out of nothing - and phase is exactly what this pair
            // exists to report.
            if p.reference_slot.is_none() && lag > 0 && lag < y.len() {
                y.drain(..lag);
                r.truncate(y.len());
            }
            let n = r.len().min(y.len());
            if n < nfft {
                out.push(FrfPairResult::default());
                continue;
            }
            r.truncate(n);
            y.truncate(n);
            let (h, coh) = hertzforge_dsp::frf::h_estimate_hv_with_coherence(&r, &y, fs, nfft);
            out.push(FrfPairResult {
                mag: h
                    .iter()
                    .map(|&(re, im)| (re * re + im * im).sqrt())
                    .collect(),
                phase: h.iter().map(|&(re, im)| im.atan2(re)).collect(),
                coherence: coh,
            });
        }
        self.frf_results = out;
    }

    /// The requested pairs (order matches [`Self::frf_results`]).
    pub fn frf_pairs(&self) -> &[FrfPairReq] {
        &self.frf_pairs
    }

    /// Last loop's per-pair FRF results.
    pub fn frf_results(&self) -> &[FrfPairResult] {
        &self.frf_results
    }

    /// Replace the requested FRF pairs. Configure-time.
    pub fn set_frf_pairs(&mut self, pairs: Vec<FrfPairReq>) {
        self.frf_pairs = pairs;
        self.frf_results.clear();
    }

    /// Clamp `drive` to `+/- sigma x its own RMS` (the standard random-drive
    /// sigma clip). A clipped sample is a deliberate crest-factor bound, not
    /// an error - the loop measures THROUGH the clip, so the small spectral
    /// distortion it introduces is inside the control path, not hidden.
    fn clip_sigma(drive: &mut [f64], sigma: f64) {
        // `<= 0.0 || is_nan()` so a NaN sigma still leaves the drive unclipped (the
        // hertzforge-breakpoints convention for clippy neg_cmp_op_on_partial_ord).
        if drive.is_empty() || sigma <= 0.0 || sigma.is_nan() {
            return;
        }
        let rms = (drive.iter().map(|x| x * x).sum::<f64>() / drive.len() as f64).sqrt();
        if rms <= 0.0 {
            return;
        }
        let lim = sigma * rms;
        for x in drive.iter_mut() {
            *x = x.clamp(-lim, lim);
        }
    }

    /// Synthesize one drive segment from a drive PSD via the persistent COLA synth.
    fn synth_segment(&mut self, psd: &[f64]) -> Result<(Vec<f64>, Vec<f64>), ServiceError> {
        // A COLD output (segment 0, resume after a hold) gets a
        // LEAD-IN of real drive so its first clocked sample is drive, not
        // silence. The lead-in is synthesized BEFORE the record, deliberately:
        // it is then the STREAM HEAD, so the COLA startup ramp (segment 0's
        // fresh synth) lands at the absolute start of the wire - exactly where
        // the old silence-then-ramp put it - and the RECORD stays ramp-free
        // and full-length. The lead-in is NOT part of the paired record; the
        // measured sysID lag absorbs it exactly as it absorbed the silence.
        let leadin_want = self
            .plant_loop
            .as_ref()
            .map(|p| p.cold_leadin_frames())
            .unwrap_or(0);
        let synth = self
            .synth
            .as_mut()
            .ok_or_else(|| ServiceError::BadState("no drive synth (not armed)".into()))?;
        let leadin = if leadin_want > 0 {
            synth.leadin(leadin_want, psd)
        } else {
            Vec::new()
        };
        Ok((leadin, synth.segment(psd)))
    }

    /// Run one plant segment through the persistent loop.
    fn run_plant_segment(
        &mut self,
        leadin: Vec<f64>,
        drive: Vec<f64>,
    ) -> Result<engine::SegmentResult, ServiceError> {
        self.plant_loop
            .as_ref()
            .ok_or_else(|| ServiceError::BadState("no plant loop (not armed)".into()))?;
        // MEASUREMENT: the two candidate producers of the measured
        // per-segment drive dropout, timed at their source.
        //  * `gap_ms` = the INTER-SEGMENT window - wall-clock from the previous
        //    segment's last acquired frame to this segment's first. Nothing
        //    writes AO during it (control DSP + synthesis), so if it exceeds
        //    the committed AO queue the output starves.
        //  * `zero_filled` = frames driven as literal zero because the tick
        //    budget outran the synthesized record (engine.rs `unwrap_or(0.0)`).
        // Exactly one of these should account for the measured ~30-45 ms.
        let gap_ms = self
            .last_segment_end
            .map(|t| t.elapsed().as_secs_f64() * 1e3)
            .unwrap_or(f64::NAN);
        let plant_loop = self.plant_loop.as_mut().expect("checked above");
        let started = std::time::Instant::now();
        let seg = plant_loop.run_segment(leadin, drive);
        let ran_ms = started.elapsed().as_secs_f64() * 1e3;
        self.last_segment_end = Some(std::time::Instant::now());
        if !seg.driver_warnings.is_empty() {
            self.last_driver_warnings = seg.driver_warnings.clone();
        }
        self.last_boundary = plant_loop.last_boundary();
        self.last_segment_timing = Some(SegmentTiming {
            gap_ms,
            ran_ms,
            zero_filled: seg.zero_filled,
            zero_ms: seg.zero_filled as f64 / self.loop_cfg.fs_hz.max(1.0) * 1e3,
            synth_ms: f64::NAN,
            pre_ms: f64::NAN,
        });
        Ok(seg)
    }

    /// The control interleave slot for a channel index (position among enabled non-drive
    /// channels). `None` if the channel is not an enabled input.
    fn slot_of(&self, channel_index: u32) -> Option<usize> {
        let mut slot = 0usize;
        for ch in &self.table.channels {
            if !ch.enabled {
                continue;
            }
            if ch.role == ChannelRole::Drive {
                slot += 1;
                continue;
            }
            if ch.index == channel_index {
                return Some(slot);
            }
            slot += 1;
        }
        None
    }

    /// The composite control SIGNAL (time domain) for the sysID FRF: for a single
    /// control channel, that channel's response; for multiple, their per-sample mean
    /// (the sysID only needs a representative response for the plant estimate).
    fn composite_control_signal(&self, seg: &engine::SegmentResult) -> Vec<f64> {
        let indices = self.cfg.control.indices();
        let slots: Vec<usize> = indices.iter().filter_map(|&i| self.slot_of(i)).collect();
        if slots.is_empty() {
            return seg.channel(0);
        }
        if slots.len() == 1 {
            return seg.channel(slots[0]);
        }
        let n_ch = seg.n_ch;
        let frames = seg.interleaved.len().checked_div(n_ch).unwrap_or(0);
        (0..frames)
            .map(|f| {
                let sum: f64 = slots
                    .iter()
                    .map(|&s| seg.interleaved.get(f * n_ch + s).copied().unwrap_or(0.0))
                    .sum();
                sum / slots.len() as f64
            })
            .collect()
    }

    /// Measure each control channel's PSD (Welch) over its recorded response.
    fn measure_per_channel(&self, seg: &engine::SegmentResult) -> Vec<Vec<f64>> {
        let nbins = self.loop_cfg.nbins();
        self.cfg
            .control
            .indices()
            .iter()
            .map(|&idx| {
                let slot = self.slot_of(idx).unwrap_or(0);
                let signal = seg.channel(slot);
                let (_f, mut psd) = hertzforge_dsp::spectrum::psd_welch(
                    &signal,
                    self.loop_cfg.fs_hz,
                    self.loop_cfg.nfft,
                    self.loop_cfg.overlap,
                );
                if psd.len() < nbins {
                    psd.resize(nbins, 0.0);
                }
                psd.truncate(nbins);
                psd
            })
            .collect()
    }

    /// Composite the per-channel PSDs (weights + mode).
    fn composite_of(&self, per_channel: &[Vec<f64>]) -> Result<Vec<f64>, ServiceError> {
        composite_psd(
            per_channel,
            &self.cfg.control.weights(),
            self.cfg.control.mode,
        )
        .map_err(|e| ServiceError::Config(ConfigError::BadControl(e.to_string())))
    }

    // --- report + status ------------------------------------------------------------

    /// The report settings digest.
    fn report_settings(&self) -> RunSettings {
        RunSettings {
            df_hz: self.cfg.df_hz,
            lines: self.cfg.lines,
            control_indices: self.cfg.control.indices(),
            control_weights: self.cfg.control.weights(),
            composite_mode: CompositeModeRecord::from(self.cfg.control.mode),
            alarm_db: self.cfg.alarm_db,
            abort_db: self.cfg.abort_db,
            mode: self.cfg.schedule.mode,
            start_db: self.cfg.schedule.start_db,
            table_db: self.cfg.schedule.table_db.clone(),
            sysid_level_db: self.cfg.sysid_level_db,
            coherence_floor: self.cfg.coherence_floor,
            full_level_duration_s: self.cfg.full_level_duration_s,
            drive_ceiling_vrms: self.cfg.drive_ceiling_vrms,
            drive_abort_vrms: self.cfg.drive_abort_vrms,
            divergence_loops: self.cfg.divergence_loops,
            overload_fraction: self.cfg.overload_fraction,
            hinv: self.cfg.hinv.clone(),
        }
    }

    /// Assemble the serde-serializable [`ReportData`].
    pub fn report_data(&self) -> ReportData {
        let channels: Vec<ChannelRow> = self
            .table
            .channels
            .iter()
            .filter(|c| c.enabled)
            .map(|c| ChannelRow {
                index: c.index,
                role: format!("{:?}", c.role),
                units: units_label(&c.units),
                range_max: c.range_max,
                abort_accel_g: c.abort.accel_g,
                abort_velocity_ips: c.abort.velocity_ips,
                abort_displacement_mil: c.abort.displacement_mil,
            })
            .collect();

        let limit_profiles: Vec<LimitProfileRow> = self
            .cfg
            .limit_profiles
            .iter()
            .map(|p| LimitProfileRow {
                channel_index: p.channel_index,
                kind: p.kind,
                limited_lines: p.resp_ceiling.iter().filter(|&&c| c.is_finite()).count(),
            })
            .collect();

        let to_line = |psd: &Option<Vec<f64>>| -> Option<LineSpectrum> {
            psd.as_ref().map(|p| LineSpectrum {
                f0_hz: 0.0,
                df_hz: self.df_hz,
                psd: p.clone(),
            })
        };
        let per_channel_lines: Vec<LineSpectrum> = self
            .last_per_channel_psds
            .iter()
            .map(|p| LineSpectrum {
                f0_hz: 0.0,
                df_hz: self.df_hz,
                psd: p.clone(),
            })
            .collect();

        ReportData {
            settings: self.report_settings(),
            channels,
            reference_grms: self.reference_grms,
            limit_profiles,
            feasibility: self.feasibility.clone(),
            level_history: self.level_history.clone(),
            event_log: self.log.clone(),
            final_composite_psd: to_line(&self.last_composite_psd),
            final_per_channel_psds: per_channel_lines,
            final_drive_psd: to_line(&self.last_drive_psd),
            // DISPLAY reference: the whole authored profile, not the
            // control-band-restricted target (see `display_target`). The
            // daemon's spectra_batch scales THIS into the reference + alarm +
            // abort traces, so the bands now span the operator's test band
            // instead of stopping a bin or two inside it.
            target_psd: self.display_target.clone(),
            final_notch_lines: self.last_notch_lines,
            final_alarm_lines: self.last_alarm_lines,
            final_abort_lines: self.last_abort_lines,
            overload_latches: self
                .overloads
                .iter()
                .map(|o| (o.channel_index, o.latched))
                .collect(),
            trip: self.trip.clone(),
            band_coherence: self.band_coherence,
            metadata: self.cfg.metadata.clone(),
            honesty: Honesty::default(),
        }
    }

    /// The live status snapshot for the UI (the status surface).
    /// The last loop's interleaved response segment `(samples, n_ch, fs_hz)`,
    /// or `None` before the first loop. Display-grade tap; see the field doc.
    /// The realized plant sample rate (Hz) the loop is running at - what the
    /// display tap needs to stamp its time base before a segment completes.
    /// The per-bin ordinary-coherence curve from the last sysID (empty before
    /// one has run). For the operator's post-sysID review (2026-08-03): the
    /// hold is only useful if the FRF and coherence are LOOKABLE-AT.
    pub fn coherence_curve(&self) -> &[f64] {
        &self.coherence
    }

    /// The per-bin `|H|^2` curve from the last sysID (empty before one has
    /// run). Companion to [`Self::coherence_curve`].
    pub fn frf_mag2(&self) -> &[f64] {
        &self.h_mag2
    }

    pub fn loop_fs_hz(&self) -> f64 {
        self.loop_cfg.fs_hz
    }

    pub fn last_segment(&self) -> Option<(&[f32], usize, f64)> {
        if self.last_segment_interleaved.is_empty() || self.last_segment_n_ch == 0 {
            return None;
        }
        Some((
            &self.last_segment_interleaved,
            self.last_segment_n_ch,
            self.loop_cfg.fs_hz,
        ))
    }

    pub fn status(&self) -> Status {
        let target_scale = 10f64.powf(self.schedule_level_db / 20.0);
        let target_grms = self.full_target_grms() * target_scale;
        let remaining = self.remaining_at_level_s();
        Status {
            state: self.state,
            current_level_db: self.schedule_level_db,
            target_grms,
            measured_grms: self.last_measured_grms,
            overload_latches: self
                .overloads
                .iter()
                .map(|o| (o.channel_index, o.latched))
                .collect(),
            alarm_active: self.alarm_active,
            abort_active: matches!(self.state, RunState::Aborted),
            notch_active_lines: self.last_notch_lines,
            band_coherence: self.band_coherence,
            capturing: self.capture.is_some(),
            elapsed_at_level_s: self.elapsed_at_level_s,
            remaining_at_level_s: remaining,
            run_mode: self.cfg.schedule.mode,
        }
    }

    /// Seconds remaining at the current level (dwell in AUTO Engaged; full duration in
    /// AUTO FullLevel; `None` in MANUAL / non-timed states).
    fn remaining_at_level_s(&self) -> Option<f64> {
        if !matches!(self.cfg.schedule.mode, RunMode::Auto) {
            return None;
        }
        match self.state {
            RunState::Engaged => {
                let dwell = self.cfg.schedule.dwell_for(self.level_index);
                Some((dwell - self.elapsed_at_level_s).max(0.0))
            }
            RunState::FullLevel => {
                Some((self.cfg.full_level_duration_s - self.elapsed_at_level_s).max(0.0))
            }
            _ => None,
        }
    }

    // --- small math helpers ---------------------------------------------------------

    /// In-band RMS (g) of a PSD over `band = [lo,hi)`: `sqrt(sum_band psd * df)`.
    fn inband_rms(psd: &[f64], band: (usize, usize), df_hz: f64) -> f64 {
        let (lo, hi) = band;
        let hi = hi.min(psd.len());
        if lo >= hi {
            return 0.0;
        }
        (psd[lo..hi].iter().sum::<f64>() * df_hz).max(0.0).sqrt()
    }

    /// Whole-grid Vrms of a drive PSD: `sqrt(sum psd * df)`.
    fn psd_vrms(psd: &[f64], df_hz: f64) -> f64 {
        (psd.iter().filter(|&&p| p > 0.0).sum::<f64>() * df_hz)
            .max(0.0)
            .sqrt()
    }
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

// Bridge: TripKind::from a KinematicTrip (kept here so report.rs need not know engine).
impl TripKind {
    fn from_kinematic(t: KinematicTrip) -> Self {
        TripKind::Kinematic {
            channel_index: t.channel_index,
            axis: t.axis,
            value: t.value,
            limit: t.limit,
            at_frame: t.at_frame,
        }
    }
}

// Re-export the report/config surface at the service module root for ergonomic paths.
pub use config::{ControlChannel, ControlSet, LimitProfile, MetaField};
pub use report::{Event as ReportEvent, EventKind as ReportEventKind};

/// The ENGINE table Random hands `PlantLoop::open`: the operator table plus a
/// synthetic Drive row when it carries none - the same pattern the recorder uses,
/// needed here for the same reason. `Backend::configure` REPLACES the backend's
/// assignments from the table it is handed; a drive-less table wipes the
/// factory-derived AO, so the NI backend builds no AO task, derives no clock
/// routing, and the clock resolves Unknown - the arm gate then refuses a pair
/// the proven set covers. Found on the first real-chassis Random arm
/// (2026-07-30); invisible on NI-SIM because its pass condition was
/// non-convergence, which a driveless run also satisfies.
fn engine_table_with_drive(table: &ChannelTable) -> ChannelTable {
    let mut engine_table = table.clone();
    if !engine_table
        .channels
        .iter()
        .any(|c| c.enabled && c.role == hertzforge_channel_table::ChannelRole::Drive)
    {
        let idx = engine_table
            .channels
            .iter()
            .map(|c| c.index)
            .max()
            .unwrap_or(0)
            + 1;
        let mut drive = hertzforge_channel_table::Channel::new(idx);
        drive.role = hertzforge_channel_table::ChannelRole::Drive;
        engine_table.channels.push(drive);
    }
    engine_table
}

/// The drive->response pipeline lag, measured by cross-correlation.
///
/// Returns the lag (frames) of the dominant correlation peak, or 0 when no peak
/// stands out: `0` means "align nothing", which leaves a genuinely incoherent
/// path to fail the gate honestly. The threshold is deliberately modest
/// (normalized |r| >= 0.2): a real wired path peaks far above it (~0.8 on the
/// cDAQ jumper), and a noise floor sits far below (~0.02).
///
/// BOUNDED, not exhaustive (2026-08-03). The original correlated the WHOLE
/// record at every candidate lag - O(len^2 / 3), which at a 32k-sample record
/// put ~3e8 multiplies inside the sysID -> engage window. That compute sits
/// between the last pretest drive and the first control drive, when nothing
/// feeds the AO; long enough and the output drains past its lookahead, halts,
/// and the first control segment dies on the staging deadman - a
/// gate-compute starvation. The peak does not need the whole record to be
/// decisive: a fixed window of drive against a coarse-then-refined lag ladder
/// finds the same peak in ~1% of the work, and the cost no longer scales with
/// the record squared.
fn measured_pipeline_lag(
    drive: &[f64],
    resp: &[f64],
    fs_hz: f64,
    band_hi_hz: f64,
    segment_frames: usize,
) -> (usize, f64, usize) {
    let len = drive.len().min(resp.len());
    if len < 64 {
        return (0, 0.0, 0);
    }
    // The pipeline lag is set by the DEVICE queue (frames_per_tick x lookahead)
    // - measured 1449 frames at fs 8192 - and NOT by the analysis block, so one
    // second of frames is generous headroom at any nPts. Scaling this with the
    // segment was tried on 2026-08-03 and made things WORSE: the op budget
    // below then divided the correlation window down to 763 samples (47 ms),
    // too short to correlate 20-2000 Hz noise at all. Range is physical; the
    // WINDOW is what buys estimate quality and must not be traded for range.
    let _ = segment_frames;
    let phys_cap = fs_hz.max(1.0) as usize;
    // The |xcorr| main lobe of band-limited noise is ~fs / (2 * band_hi) frames
    // wide. The coarse stride must stay inside it or the pass can step over the
    // true peak entirely; half the lobe width gives margin.
    let lobe = (fs_hz / (2.0 * band_hi_hz.max(1.0))).max(2.0) as usize;
    let stride = (lobe / 2).max(1);
    // Correlation window: sized against a fixed op budget so a narrow lobe
    // (many coarse evaluations) trades window length rather than blowing the
    // budget. sqrt(w) is the peak-to-floor ratio; even the floor of this range
    // (~2048) keeps a wired path's ~0.8 peak far above the 0.2 threshold.
    let max_lag_wanted = phys_cap.min(len.saturating_sub(64));
    let evals = (max_lag_wanted / stride).max(1);
    let w = (100_000_000usize / evals).clamp(256, 8192).min(len / 2);
    let max_lag = max_lag_wanted.min(len - w);
    if max_lag == 0 {
        return (0, 0.0, 0);
    }
    // SKIP THE SYNTHESIS RAMP. COLA overlap-add is only complete after one
    // HOP, so the head of the drive record is a partial sum - and the
    // correlation window used to start at sample 0. At nfft 8192 the hop is
    // 4096 and an 8192-sample window mostly cleared it; at nfft 32768 the hop
    // is 16384 and the window sat ENTIRELY inside the ramp. Measured on the
    // chassis 2026-08-03: nfft 8192 found lag 1577 at peak |r| 0.851, while
    // nfft 32768 - same window, same range, same stride - found nothing at
    // |r| 0.117, below the 0.2 trust threshold, so alignment stayed off and
    // band coherence read 0.088-0.15 on a jumpered wire.
    let skip = segment_frames
        .min(len.saturating_sub(w + max_lag))
        .min(len / 4);
    let drive = &drive[skip..];
    let resp = &resp[skip..];
    let len = drive.len().min(resp.len());
    if len < w + max_lag {
        return (0, 0.0, 0);
    }
    let d_rms = (drive[..w].iter().map(|x| x * x).sum::<f64>() / w as f64).sqrt();
    let r_rms = (resp.iter().map(|x| x * x).sum::<f64>() / len as f64).sqrt();
    if d_rms <= 0.0 || r_rms <= 0.0 {
        return (0, 0.0, 0);
    }
    let score = |lag: usize| -> f64 {
        let mut acc = 0.0;
        for i in 0..w {
            acc += drive[i] * resp[i + lag];
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
    // INSTRUMENTATION (temporary). The bounded search above covers one
    // second of frames and finds nothing at nfft 32768 on a jumpered wire. The
    // bounded result cannot distinguish "the peak is past the range" from "these
    // two records never correspond" - so scan the WHOLE record coarsely, with a
    // short window, and print where the correlation actually lives. This check
    // can fail in both directions, which is the point: a peak past 8533 frames
    // indicts the range, and a flat curve across the entire record indicts the
    // pairing itself.
    // Gated on FAILURE, deliberately. The scan is ~400M multiply-accumulates at
    // a large record - real time on the box - and a healthy sysID has no use
    // for it. A FAILED one has nothing better to spend that time on, and the
    // alternative (rebuild with instrumentation, redeploy, re-run) costs the
    // next person an hour to learn what this prints for free.
    if best.1 < 0.2 {
        let dw = 2048usize.min(len / 4);
        if dw >= 256 {
            let dr = (drive[..dw].iter().map(|x| x * x).sum::<f64>() / dw as f64).sqrt();
            let full = len.saturating_sub(dw);
            let dscore = |lag: usize| -> f64 {
                let mut acc = 0.0;
                for i in 0..dw {
                    acc += drive[i] * resp[i + lag];
                }
                ((acc / dw as f64) / (dr * r_rms)).abs()
            };
            let mut top = [(0usize, 0.0f64); 3];
            let dstride = stride.max(1);
            let mut lag = 0usize;
            while lag < full {
                let r = dscore(lag);
                if r > top[2].1 {
                    top[2] = (lag, r);
                    top.sort_by(|a, b| b.1.total_cmp(&a.1));
                }
                lag += dstride;
            }
            eprintln!(
                "sysID alignment FAILED - full-record scan: len {len} skip {skip} \
                 w {dw} stride {dstride} top3 lag/r = {}/{:.3}, {}/{:.3}, {}/{:.3} \
                 (a strong peak here means the bounded search range is too small; \
                 a flat curve means the two records do not correspond at all, \
                 which is a pairing or truncation fault, not an alignment one)",
                top[0].0, top[0].1, top[1].0, top[1].1, top[2].0, top[2].1
            );
        }
    }
    if best.1 >= 0.2 {
        (best.0, best.1, searched)
    } else {
        (0, best.1, searched)
    }
}

#[cfg(test)]
mod engine_table_tests {
    use super::*;
    use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};

    #[test]
    fn a_driveless_table_gains_exactly_one_synthetic_drive_row() {
        let mut t = ChannelTable::new();
        t.channels.push(Channel::new(1));
        t.channels.push(Channel::new(2));
        let e = engine_table_with_drive(&t);
        let drives: Vec<_> = e
            .channels
            .iter()
            .filter(|c| c.role == ChannelRole::Drive)
            .collect();
        assert_eq!(drives.len(), 1, "one synthetic drive row injected");
        assert_eq!(drives[0].index, 3, "appended after the highest index");
        assert_eq!(e.channels.len(), 3);
    }

    #[test]
    fn a_table_with_a_drive_row_is_untouched() {
        let mut t = ChannelTable::new();
        t.channels.push(Channel::new(1));
        let mut d = Channel::new(2);
        d.role = ChannelRole::Drive;
        t.channels.push(d);
        let e = engine_table_with_drive(&t);
        assert_eq!(e.channels.len(), 2, "no second drive row");
    }
}

#[cfg(test)]
mod clip_sigma_tests {
    use super::*;

    #[test]
    fn a_nan_or_non_positive_sigma_leaves_the_drive_unclipped() {
        // A NaN sigma reaching the clamp would panic (`f64::clamp` on NaN bounds).
        for sigma in [f64::NAN, 0.0, -1.0] {
            let mut d = vec![0.1, -5.0, 0.2, 4.0];
            let before = d.clone();
            RandomService::clip_sigma(&mut d, sigma);
            assert_eq!(d, before, "sigma {sigma}");
        }
    }
}

/// One segment's drive-continuity timing. The two candidate producers
/// of a per-segment drive dropout, measured at their source so a host can log
/// them without this crate taking a logging dependency.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentTiming {
    /// Wall-clock from the previous segment's end to this one's start. Nothing
    /// writes AO during this window (control DSP + drive synthesis), so a value
    /// approaching the committed AO queue depth means the output starves.
    pub gap_ms: f64,
    /// How long the segment's acquisition itself took.
    pub ran_ms: f64,
    /// Frames driven as literal zero because the engine's tick budget outran
    /// the synthesized drive record.
    pub zero_filled: usize,
    /// `zero_filled` expressed as milliseconds at the configured rate.
    pub zero_ms: f64,
    /// Drive synthesis alone (COLA), inside the unfed window.
    pub synth_ms: f64,
    /// The whole PRE-acquisition phase of this tick (law PSD -> notch -> clamp
    /// -> synthesis). The rest of `gap_ms` is the POST-acquisition analysis of
    /// the previous segment.
    pub pre_ms: f64,
}
