//! `ClassicalShockService` - the CLASSICAL-SHOCK run orchestration, mirroring
//! the eigen-random / Explorer service shape: a state machine, a timestamped event log, a serde
//! [`report::ReportData`], honesty markers, and a [`report::Status`] snapshot
//! for the UI.
//!
//! # State machine
//!
//! ```text
//! Idle -> Feasibility -> Armed -> Firing -> Stopped
//!            |                       |
//!            v                       v
//!   FeasibilityBlocked           Aborted   (tolerance ABORT / kinematic
//!   (arm_with_override(ack)                 breach / refused pre-fire)
//!    hard-block)
//! ```
//!
//! # The per-pulse pipeline (every fired pulse)
//!
//! 1. **Pre-fire drive limit**: the predicted peak (deterministic
//!    crest = 1 on the commanded waveform) vs the
//!    configured peak-accel ceiling; a predicted breach scales the pulse DOWN
//!    before firing, an invalid prediction refuses to fire (abort).
//! 2. **Fire** through the [`firer::PulseFirer`] seam. Path B (HostDac) plays
//!    the HOST-COMPENSATED composite into the backend (the eigen-hal-sim plant
//!    today); the delegated path fires an absolute `10^(dB/20)` setpoint
//!    against the UNCOMPENSATED reference downloaded at arm (the
//!    `ReferenceForPath::delegated` seam enforces it).
//! 3. **Record**: the measured control-channel record is ingested
//!    into the shared `eigen_shock_common::PulseStore` (kinematics + SRS +
//!    optional PVSS computed on ingest) and streamed to the capture sink.
//! 4. **Tolerance**: the three checks (amplitude envelope, dV,
//!    substitution equivalence) against the COMMANDED amplitude; a failed
//!    report aborts (or alarms, per config).
//! 5. **Kinematic breach**: measured peak velocity / pk-pk displacement vs the
//!    shaker record -> abort on breach.
//! 6. **Between-pulse drive limit**: measured monitor peaks vs
//!    ceilings scale the NEXT pulse (V/I monitors are inert on Path A).
//! 7. **Ladder tick**: AUTO walks `[-12, -9, -6, -3, 0]` dB with
//!    N pulses per level; MANUAL steps +/- 1 dB / jumps table levels.
//!
//! # Pulse trains + gunfire
//!
//! [`ClassicalShockService::fire_train`] fires `count` pulses at the rate-model
//! floor or a fixed delay ([`ClassicalShockService::fire_gunfire_burst`]
//! converts rounds/min -> `60/rate` s spacing, MIL-STD-810 519.8 Procedure IV).
//! The rate model REFUSES spacings below its floors; at-rate trains are
//! HostDac-only per `PathCapabilities` (the delegated device recomputes
//! compensation per pulse); and every multi-pulse plan carries the
//! HARDWARE-BLOCKED marker (the write-triggers-fire semantic is an
//! undecoded hardware unknown - trains are Sim + mock-transport complete, live
//! hardware awaits the inbound-capture decode). Trains fire at a PINNED level;
//! they do not walk the AUTO ladder mid-burst.
//!
//! # Honesty
//!
//! [`report::Honesty::for_path`] is carried verbatim into every report; the
//! capability split is read from `eigen_shock_common::PathCapabilities` - the
//! one source of truth - never re-derived here.

pub mod config;
pub mod firer;
pub mod pacer;
pub mod report;

use crate::spec::ClassicalPulseSpec;
use crate::tolerance::{check_tolerance, ToleranceReport};
use config::{ClassicalTestConfig, ConfigError};
use eigen_capture::{default_data_root, save_snapshot, CaptureConfig, CaptureSink, Mark};
use eigen_channel_table::{ChannelRole, ChannelTable, Units};
use eigen_dsp::shock::CompensatedPulse;
use eigen_dsp::srs::{octave_freqs, srs_analyze, OctaveFraction};
use eigen_shaker_db::{FeasibilityReport, Verdict};
use eigen_shock_common::pulse_store::{build_snapshot, capture_pulse, DEFAULT_SRS_Q};
use eigen_shock_common::train::TrainPlan;
use eigen_shock_common::{
    gunfire_spacing, pulse_feasibility, MeasuredPeak, PulseStore, ShockLadderState, ShockRunMode,
    TrainError, TrainSpec,
};
use firer::{quiet_lead_frames, FireControl, PulseFirer};
use pacer::{RealPacer, TrainPacer};
use report::{
    ChannelRow, DriveLimitActionRecord, EventKind, EventLog, FeasibilityDigest, Honesty,
    LevelHistoryEntry, PulseOrigin, PulseRow, ReportData, RunState, Status, ToleranceSummary,
    TrainRecord, TripKind, TripRecord,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// How often the inter-pulse wait re-reads the `should_continue` flag.
///
/// The gap between two pulses is up to seconds long (a 30 rounds/min gunfire
/// burst is 2.0 s), and an operator Stop that took a full spacing to be noticed
/// would be indistinguishable from a hung service. The wait is therefore chunked
/// at this period rather than slept in one call, which bounds Stop latency
/// BETWEEN pulses at ~50 ms + the remaining fire. It does not bound Stop latency
/// INSIDE a pulse - that bound is the committed drive queue and belongs to
/// `eigen_hal::StreamError::Cancelled`, not here.
const CANCEL_POLL_S: f64 = 0.050;

/// Relative rate difference above which the adopted rate is logged as a
/// COERCION rather than passing silently. Matches
/// `eigen_module_abi::RATE_EXACT_TOLERANCE`'s intent: below this the request and
/// the device agree for every practical purpose.
const RATE_COERCION_NOTE_TOL: f64 = 1e-6;

/// `true` when a `should_continue` input says keep going (or there is none).
///
/// POLARITY WARNING, because two flags with opposite senses meet in this call
/// path: `should_continue` is TRUE to continue, while
/// `eigen_hal::play_block_wise`'s `cancel` is TRUE to stop. They are not the
/// same flag and must never be passed to each other.
#[inline]
fn still_running(should_continue: Option<&AtomicBool>) -> bool {
    should_continue.is_none_or(|f| f.load(Ordering::Relaxed))
}

/// Errors from a service operation.
#[derive(Debug, Clone, PartialEq)]
pub enum ServiceError {
    /// The config was invalid.
    Config(ConfigError),
    /// The operation is not allowed in the current state.
    BadState(String),
    /// The fire path (backend / delegated wire) refused an operation.
    Fire(String),
    /// A pulse train / gunfire burst was refused (rate below the
    /// floors, or an at-rate train on a path that forbids it).
    Train(TrainError),
    /// A capture/snapshot IO error.
    Io(String),
    /// A fire was CANCELLED at a block boundary because the host raised the
    /// cancel flag (Stop / Abort). NOT a fault: the operator asked.
    ///
    /// The frames the device already ACCEPTED kept playing - mid-record
    /// preemption is refused by this chassis (`-200289`) - so
    /// `frames_committed - frames_done` is what was still queued when the flag
    /// was seen. See [`ClassicalShockService::abort`] for the latency
    /// arithmetic.
    Cancelled {
        frames_done: usize,
        frames_committed: usize,
        frames_expected: usize,
    },
    /// An invariant this service is supposed to maintain did not hold.
    ///
    /// These were once five `expect()` calls. Each one was true when it
    /// was written and each one was a panic that would take a daemon host down
    /// with it, in a crate whose whole job is to be the thing that does not
    /// panic while a shaker is armed.
    Internal(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::Config(e) => write!(f, "{e}"),
            ServiceError::BadState(s) => write!(f, "bad state: {s}"),
            ServiceError::Fire(s) => write!(f, "fire path error: {s}"),
            ServiceError::Train(e) => write!(f, "train refused: {e}"),
            ServiceError::Io(s) => write!(f, "io error: {s}"),
            ServiceError::Cancelled {
                frames_done,
                frames_committed,
                frames_expected,
            } => write!(
                f,
                "fire CANCELLED after {frames_done} of {frames_expected} frames; \
                 {} committed frame(s) were still queued at the device",
                frames_committed.saturating_sub(*frames_done)
            ),
            ServiceError::Internal(s) => write!(f, "internal invariant violated: {s}"),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<ConfigError> for ServiceError {
    fn from(e: ConfigError) -> Self {
        ServiceError::Config(e)
    }
}

/// `10^(db/20)`.
#[inline]
fn db_scale(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// The classical-shock run service (see the module docs). Generic over the
/// [`PulseFirer`] fire-path seam: [`firer::BackendPulseFirer`] for Path B
/// (HostDac; the Sim plant in tests), or
/// the delegated firer for the delegated path (tested over a mock transport) -
/// which lives with the delegated backend because it names a concrete backend
/// and this crate may not.
pub struct ClassicalShockService<F: PulseFirer> {
    cfg: ClassicalTestConfig,
    table: ChannelTable,
    firer: F,
    state: RunState,
    log: EventLog,

    // --- run scaffolding ---------------------------------------------------
    /// The level ladder, built at arm from the validated schedule.
    ladder: Option<ShockLadderState>,
    /// The shared per-pulse record store (browse/overlay + report).
    store: PulseStore,
    /// The SRS natural-frequency grid every pulse is analyzed on. Re-derived at
    /// arm against the ADOPTED rate's Nyquist limit.
    srs_freqs: Vec<f64>,
    /// The rate the DEVICE actually programmed, adopted at arm from
    /// [`PulseFirer::actual_fs_hz`]. `None` before arm.
    ///
    /// **This, not `cfg.fs_hz`, is what every Hz-denominated derivation reads,**
    /// through [`ClassicalShockService::fs_hz`] - the only place the fallback to
    /// the requested rate lives.
    fs_actual_hz: Option<f64>,
    /// The HOST-COMPENSATED composite, synthesized at arm. Path B drives it
    /// (scaled); on the delegated path it is UNUSED for firing (the device
    /// holds the uncompensated download) and serves only the
    /// deterministic peak prediction + the feasibility estimate.
    composite: Option<CompensatedPulse>,
    /// Peak |g| of the composite (the crest = 1 deterministic predictor base).
    full_peak_g: f64,
    /// The control channel's report name for between-pulse measured peaks
    /// (`ch<index>` of the first enabled Control channel).
    control_channel_name: String,

    // --- run bookkeeping ---------------------------------------------------
    pulses_fired: u64,
    /// SIMULATED run clock, seconds since arm, advanced by the realized inter-pulse
    /// spacing. REPORTING ONLY: it is `run_clock_s += spacing_s`, so asserting that its
    /// deltas equal the spacing asserts `(x + s) - x == s` and can never fail (a result
    /// that cannot fail is not evidence). The real timeline is
    /// [`TrainRecord::wall_times_s`], measured through the injected [`TrainPacer`].
    run_clock_s: f64,
    /// The WALL clock EVERY fire is paced against - trains and the AUTO ladder
    /// alike. `RealPacer` in production; the tests and the bench
    /// instruments swap in `VirtualPacer` via
    /// [`ClassicalShockService::set_pacer`].
    pacer: Box<dyn TrainPacer>,
    /// Wall time of the last pulse START, on the pacer's epoch. The deadline for
    /// the next pulse is this plus the realized spacing - measured, not
    /// accumulated. `None` from arm until the first pulse (the first pulse after
    /// arm never waits, on either the ladder or a train).
    last_pulse_wall_s: Option<f64>,
    /// The HOST's cancel flag: TRUE means STOP AT THE NEXT BOUNDARY.
    ///
    /// An `Arc` and not a plain field because the service is SYNCHRONOUS - a
    /// fire blocks its caller for the whole record - so a host that reaches the
    /// service through a `Mutex` cannot call `stop()` while a fire is in flight;
    /// it would block on the lock the fire is holding. The flag is the one
    /// channel that does not need the lock. Hosts either supply their own with
    /// [`ClassicalShockService::with_cancel_flag`] or take a handle to the
    /// internal one with [`ClassicalShockService::cancel_flag`].
    cancel: Arc<AtomicBool>,
    /// Host tap called from INSIDE the fire's block loop with
    /// `(frames_delivered, frames_expected)` - the seam for the daemon's
    /// watchdog beat, StatusCell write and display tap.
    block_observer: Option<Box<dyn FnMut(usize, usize) + Send>>,
    base_unix: u64,
    /// Between-pulse drive-limit scale carried into the next pulse.
    next_pulse_scale: f64,
    alarm_active: bool,
    delegated_note_logged: bool,

    // --- report accumulators -----------------------------------------------
    pulse_rows: Vec<PulseRow>,
    level_history: Vec<LevelHistoryEntry>,
    drive_limit_actions: Vec<DriveLimitActionRecord>,
    train_records: Vec<TrainRecord>,
    last_tolerance: Option<ToleranceReport>,
    tolerance_checked: u64,
    tolerance_passed: u64,
    feasibility: Option<FeasibilityDigest>,
    override_ack: Option<String>,
    trip: Option<TripRecord>,

    // --- capture -----------------------------------------------------------
    capture: Option<CaptureSink>,
    data_root: PathBuf,
}

impl<F: PulseFirer> ClassicalShockService<F> {
    /// Build a service for `cfg` against `table` over `firer`, validating the
    /// config up front. `data_root` is the LOCAL capture root (default
    /// [`default_data_root`] if `None`).
    pub fn new(
        cfg: ClassicalTestConfig,
        table: ChannelTable,
        firer: F,
        data_root: Option<PathBuf>,
    ) -> Result<Self, ServiceError> {
        cfg.validate()?;
        table.validate().map_err(|e| {
            ServiceError::Config(ConfigError::BadConfig(format!("channel table: {e}")))
        })?;
        // The PROVISIONAL grid, at the REQUESTED rate. Re-derived at arm against
        // the rate the device actually programmed - a config-time grid
        // cannot know the device's Nyquist.
        let (srs_freqs, _) = srs_grid(&cfg, cfg.fs_hz)?;
        let control_channel_name = table
            .channels
            .iter()
            .find(|c| c.enabled && c.role == ChannelRole::Control)
            .map(|c| format!("ch{}", c.index))
            .unwrap_or_else(|| "ch0".into());
        let data_root = data_root.unwrap_or_else(default_data_root);

        Ok(ClassicalShockService {
            cfg,
            table,
            firer,
            state: RunState::Idle,
            log: EventLog::default(),
            ladder: None,
            store: PulseStore::new(),
            srs_freqs,
            fs_actual_hz: None,
            composite: None,
            full_peak_g: 0.0,
            control_channel_name,
            pulses_fired: 0,
            run_clock_s: 0.0,
            pacer: Box::new(RealPacer::new()),
            last_pulse_wall_s: None,
            cancel: Arc::new(AtomicBool::new(false)),
            block_observer: None,
            base_unix: 0,
            next_pulse_scale: 1.0,
            alarm_active: false,
            delegated_note_logged: false,
            pulse_rows: Vec::new(),
            level_history: Vec::new(),
            drive_limit_actions: Vec::new(),
            train_records: Vec::new(),
            last_tolerance: None,
            tolerance_checked: 0,
            tolerance_passed: 0,
            feasibility: None,
            override_ack: None,
            trip: None,
            capture: None,
            data_root,
        })
    }

    /// Install the HOST's cancel flag before the run starts. Without
    /// it the service uses an internal one, which a host can still reach with
    /// [`Self::cancel_flag`].
    ///
    /// TRUE means STOP - the opposite sense to `fire_train_cancellable`'s
    /// `should_continue`.
    pub fn with_cancel_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancel = flag;
        self
    }

    // --- introspection ------------------------------------------------------

    pub fn state(&self) -> RunState {
        self.state
    }

    /// **The rate every Hz-denominated derivation in this service runs at.**
    ///
    /// The ADOPTED device rate once armed; the profile's REQUEST before that, which is
    /// the only honest answer available pre-arm and is used only by the pre-arm
    /// feasibility estimate. This one function is the whole fix for the requested-rate
    /// defect: before it, six call sites read `cfg.fs_hz` directly and the record was
    /// synthesized at 5120 Hz while the chassis played it at 51 200.
    pub fn fs_hz(&self) -> f64 {
        self.fs_actual_hz.unwrap_or(self.cfg.fs_hz)
    }

    /// The rate the DEVICE actually programmed, `None` before arm.
    pub fn actual_fs_hz(&self) -> Option<f64> {
        self.fs_actual_hz
    }

    /// The rate the PROFILE requested (`cfg.fs_hz`). Not an authority.
    pub fn requested_fs_hz(&self) -> f64 {
        self.cfg.fs_hz
    }

    /// A handle on the cancel flag, so a host holding the service
    /// behind a `Mutex` can raise it WITHOUT the lock a fire in flight is
    /// holding. See the `cancel` field docs.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// Install the per-block observer: called from INSIDE the fire's
    /// block loop with `(frames_delivered, frames_expected)`.
    ///
    /// This is the seam the daemon host attaches its watchdog beat, StatusCell
    /// write and display tap to. A fire is the longest thing this service
    /// does without returning, so anything that must advance DURING one has to
    /// enter here or it does not advance at all.
    pub fn set_block_observer(&mut self, f: Box<dyn FnMut(usize, usize) + Send>) {
        self.block_observer = Some(f);
    }

    /// Remove the per-block observer.
    pub fn clear_block_observer(&mut self) {
        self.block_observer = None;
    }
    /// The config this service was built from. Exposed so a caller holding ONLY a
    /// configured service can rebuild an equivalent one without re-asking the UI to
    /// reassemble the config - the operator's Reset escape hatch
    /// (`ClassicalState::reset`) must work even when the UI's assembled config is
    /// momentarily un-buildable, which is exactly the state that stranded the
    /// 2026-07-30 real-chassis Random run.
    pub fn config(&self) -> &ClassicalTestConfig {
        &self.cfg
    }
    pub fn log(&self) -> &EventLog {
        &self.log
    }
    /// The shared per-pulse record store (browse + overlay).
    pub fn pulse_store(&self) -> &PulseStore {
        &self.store
    }
    /// The trip record if the run aborted.
    pub fn trip(&self) -> Option<&TripRecord> {
        self.trip.as_ref()
    }
    pub fn feasibility(&self) -> Option<&FeasibilityDigest> {
        self.feasibility.as_ref()
    }
    /// The SRS natural-frequency grid this run analyzes pulses on.
    pub fn srs_freqs(&self) -> &[f64] {
        &self.srs_freqs
    }
    /// The currently commanded level, dB (the schedule start before arm).
    pub fn current_level_db(&self) -> f64 {
        self.ladder
            .as_ref()
            .map(|l| l.current_level_db())
            .unwrap_or(self.cfg.schedule.start_db)
    }

    fn transition(&mut self, to: RunState) {
        let from = self.state;
        if from != to {
            self.state = to;
            self.log.push(EventKind::StateChange { from, to });
        }
    }

    // --- feasibility + arming ---------------------------------------

    /// Run the pre-arm feasibility assessment: the
    /// DETERMINISTIC-kinematics path (`eigen_shock_common::pulse_feasibility`,
    /// crest = 1.0, pure integrator, `force_shock` consumed by basis)
    /// against the HOST-COMPENSATED composite at the schedule's FULL level (the
    /// run's maximum demand). Transitions `Idle -> Feasibility`; an Exceeded
    /// verdict parks at `FeasibilityBlocked` (arming hard-blocked until
    /// [`Self::arm_with_override`]). No shaker configured => a NotEvaluated
    /// report + a note (arming allowed without a shaker-limit block).
    ///
    /// On the delegated path the assessed waveform is a host-side ESTIMATE of
    /// the on-device compensation demand (the device compensates on-device);
    /// the estimate note is logged.
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
                    overall: Verdict::NotEvaluated,
                    assumptions: vec!["No shaker record configured.".into()],
                }
            }
            Some(shaker) => {
                if self.firer.path().is_delegated() {
                    self.log.push(EventKind::Note {
                        text: "Path A feasibility: the assessed waveform is the HOST-side \
                               compensated composite - an ESTIMATE of the on-device \
                               compensation demand (the device compensates on-device, F1)."
                            .into(),
                    });
                }
                // Pre-arm, so `fs_hz()` is still the REQUEST: the device has not
                // been configured and there is nothing else to answer with.
                // Honest here in a way it was not downstream, because the
                // assessed quantities are physical (peak g, in/s, in) and a
                // pulse of the same duration has the same kinematics at either
                // rate - only the SAMPLE COUNT differs. The rate divorce bit
                // where sample counts became durations, which is the fire path.
                let fs = self.fs_hz();
                let comp = self.cfg.spec.synthesize(fs);
                let full_scale = db_scale(self.cfg.schedule.full_db());
                let wave: Vec<f64> = comp.waveform_g.iter().map(|&v| v * full_scale).collect();
                pulse_feasibility(
                    shaker,
                    &self.cfg.moving_mass,
                    &wave,
                    fs,
                    self.cfg.feasibility_tolerances(),
                )
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
    /// exceedance use [`Self::arm_with_override`] instead (the hard block).
    pub fn arm(&mut self) -> Result<(), ServiceError> {
        match self.state {
            RunState::Feasibility => self.arm_inner(),
            RunState::FeasibilityBlocked => Err(ServiceError::BadState(
                "arming HARD-BLOCKED: feasibility Exceeded; use arm_with_override(ack)".into(),
            )),
            _ => Err(ServiceError::BadState(format!("arm from {:?}", self.state))),
        }
    }

    /// Arm past a feasibility exceedance with an explicit operator
    /// acknowledgment. Logs the ack verbatim and records it in the
    /// feasibility digest.
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

    /// Arm the fire path, ADOPT the device's rate, then build everything
    /// Hz-denominated at that rate, and enter `Armed`. Shared by both arm paths.
    ///
    /// # The order is the fix
    ///
    /// It used to be: synthesize the composite at `cfg.fs_hz`, build the ladder,
    /// THEN arm the fire path. So the record was built at whatever the profile
    /// asked for and handed to a device running at whatever the device chose,
    /// and the arm gate compared the backend's own `caps().sample_rate_hz` to
    /// the backend's own `ConfiguredActual` - the device against itself, a green
    /// that could not fail. On the cDAQ-9178 the request was 5120 Hz and the
    /// 9234 ran at 51 200, so an 11 ms half-sine went out ~1.1 ms wide and every
    /// downstream number (tolerance, kinematics, SRS, the capture sidecar's
    /// declared rate) described a pulse that had never existed.
    ///
    /// Now: **arm first, adopt, then derive.** Nothing Hz-denominated may be
    /// built above this call, and the one accessor everything reads is
    /// [`Self::fs_hz`].
    fn arm_inner(&mut self) -> Result<(), ServiceError> {
        // Config validation that needs no hardware, first: a bad schedule should
        // refuse before anything is reserved on a chassis.
        let ladder = ShockLadderState::new(self.cfg.schedule.clone())
            .map_err(|e| ServiceError::Config(ConfigError::BadSchedule(e.to_string())))?;

        // 1. ARM THE FIRE PATH. Until this returns, the rate is a REQUEST.
        self.firer.arm()?;
        self.drain_warnings(None);

        // 2. ADOPT. A firer that cannot say what rate it runs at cannot have
        //    anything denominated against it, and silently falling back to the
        //    request is exactly the defect this ordering exists to kill.
        let requested = self.cfg.fs_hz;
        let adopted = self.firer.actual_fs_hz().ok_or_else(|| {
            ServiceError::Fire(
                "the fire path reports no actual sample rate after arm; nothing can be \
                 synthesized against it (the device owns the rate)"
                    .into(),
            )
        })?;
        if !(adopted.is_finite() && adopted > 0.0) {
            return Err(ServiceError::Fire(format!(
                "the fire path reports an unusable actual sample rate ({adopted} Hz)"
            )));
        }
        self.fs_actual_hz = Some(adopted);
        let rel = (requested - adopted).abs() / adopted;
        if rel > RATE_COERCION_NOTE_TOL {
            // The check with two genuine authorities: the OPERATOR's requested
            // rate against the DEVICE's programmed rate. It logs rather than
            // refuses, because refusing would refuse this chassis outright -
            // the 9234 rides a 13.1072 MHz/(256n) ladder and coerces almost
            // every request. The refusal-grade version of this comparison would
            // be a rate policy the operator opts into, not a silent default.
            self.log.push(EventKind::RateAdopted {
                requested_hz: requested,
                actual_hz: adopted,
                relative_error: rel,
            });
        }

        // 3. DERIVE, all of it at the adopted rate.
        let (freqs, clamped_to) = srs_grid(&self.cfg, adopted)?;
        if let Some(nyquist) = clamped_to {
            self.log.push(EventKind::Note {
                text: format!(
                    "SRS analysis grid CLAMPED to the adopted rate's Nyquist limit \
                     ({nyquist:.4} Hz): the configured upper edge {:.4} Hz is not \
                     analyzable at {adopted:.4} Hz. {} bands retained.",
                    self.cfg.srs_f_hi_hz,
                    freqs.len()
                ),
            });
        }
        self.srs_freqs = freqs;

        let comp = self.cfg.spec.synthesize(adopted);
        self.full_peak_g = comp.waveform_g.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
        self.composite = Some(comp);

        // The quiet surround is TIME-denominated: a fixed sample
        // count meant 39 ms at 5120 Hz and 3.9 ms at 51 200, and at the higher
        // rate too short for the standard's +/-0.4 T window on an 11 ms pulse.
        self.firer.set_quiet_frames(quiet_lead_frames(
            self.cfg.spec.duration_ms / 1000.0,
            adopted,
        ));

        // A fresh run must not start already cancelled.
        self.cancel.store(false, Ordering::SeqCst);
        self.last_pulse_wall_s = None;

        self.base_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.run_clock_s = 0.0;
        self.level_history.push(LevelHistoryEntry {
            level_db: ladder.current_level_db(),
            auto: self.cfg.schedule.mode == ShockRunMode::Auto,
            at_pulse: 0,
        });
        self.ladder = Some(ladder);

        // Configured V/I monitors are inert on a path where V/I are
        // unobservable - say so once, up front, from the one capability table.
        let caps = self.firer.path().capabilities();
        if !caps.vi_limiting_observable && self.cfg.drive_limit.monitors.iter().any(|m| m.is_vi()) {
            self.log.push(EventKind::Note {
                text: "V/I drive monitors are configured but INERT on this drive path (V/I \
                       unobservable on the delegated path); ceilings are greyed."
                    .into(),
            });
        }

        if self.cfg.capture.auto_start {
            let name = self.cfg.capture.run_name.clone();
            self.start_capture(&name)?;
        }
        self.transition(RunState::Armed);
        Ok(())
    }

    // --- manual level controls ------------------------------------------------

    /// Nudge the commanded level up by 1 dB (manual step). Allowed
    /// in any mode while armed/firing (the operator can trim an AUTO run).
    pub fn step_up(&mut self) -> Result<f64, ServiceError> {
        self.manual_step(true)
    }

    /// Nudge the commanded level down by 1 dB (manual step).
    pub fn step_down(&mut self) -> Result<f64, ServiceError> {
        self.manual_step(false)
    }

    fn manual_step(&mut self, up: bool) -> Result<f64, ServiceError> {
        if !self.state.is_running() {
            return Err(ServiceError::BadState(format!(
                "step from {:?} (arm first)",
                self.state
            )));
        }
        // Was `.expect("ladder present while running")`: true when it
        // was written, and a panic in a library a daemon hosts.
        let ladder = self.ladder.as_mut().ok_or_else(|| {
            ServiceError::Internal("no ladder while running (manual step)".into())
        })?;
        let from = ladder.current_level_db();
        if up {
            ladder.step_up();
        } else {
            ladder.step_down();
        }
        let to = ladder.current_level_db();
        self.level_history.push(LevelHistoryEntry {
            level_db: to,
            auto: false,
            at_pulse: self.pulses_fired,
        });
        self.log.push(EventKind::LevelChange {
            from_db: from,
            to_db: to,
            auto: false,
        });
        Ok(to)
    }

    /// Jump directly to table level `i`, clearing any manual
    /// offset. Returns the new level dB.
    pub fn jump_to_level(&mut self, i: usize) -> Result<f64, ServiceError> {
        if !self.state.is_running() {
            return Err(ServiceError::BadState(format!(
                "jump_to_level from {:?} (arm first)",
                self.state
            )));
        }
        // Was `.expect("ladder present while running")`.
        let ladder = self.ladder.as_mut().ok_or_else(|| {
            ServiceError::Internal("no ladder while running (jump_to_level)".into())
        })?;
        let from = ladder.current_level_db();
        ladder
            .jump_to_level(i)
            .map_err(|e| ServiceError::BadState(e.to_string()))?;
        let to = ladder.current_level_db();
        self.level_history.push(LevelHistoryEntry {
            level_db: to,
            auto: false,
            at_pulse: self.pulses_fired,
        });
        self.log.push(EventKind::LevelChange {
            from_db: from,
            to_db: to,
            auto: false,
        });
        Ok(to)
    }

    // --- firing ---------------------------------------------------------------

    /// Fire ONE pulse at the ladder's current level and tick the ladder (AUTO
    /// advances after `pulses_for(level)`; MANUAL only counts).
    ///
    /// **Paced in WALL time** against the rate-model floor for this path,
    /// off the injected [`TrainPacer`] - see
    /// [`Self::fire_next_pulse_cancellable`], which this defers to with no
    /// cancel channel. Returns `true` if the run is still running.
    pub fn fire_next_pulse(&mut self) -> Result<bool, ServiceError> {
        self.fire_next_pulse_cancellable(None)
    }

    /// [`Self::fire_next_pulse`] plus a `should_continue` flag (TRUE to keep
    /// going) read during the inter-pulse wait.
    ///
    /// # The ladder pace is REAL now
    ///
    /// This is where the AUTO ladder waits. Until 2026-08-08 it did not: it
    /// charged the rate-model floor to `run_clock_s` - a `f64` field - and
    /// returned, so on the chassis `run_auto` fired all five rungs back to back
    /// as fast as the fire path could return, with the armature getting none of
    /// the cooling / settling / compute time the rate-model floors exist to
    /// reserve. The ladder walks UP in level, so the fastest firing happened at
    /// the highest drive. Trains were given wall-clock pacing first and the
    /// ladder was not in that scope; this closes the gap on the same
    /// `wait_until` + `pacer.pulse_start_s()` shape.
    ///
    /// The FIRST pulse after arm does not wait - the same rule trains follow,
    /// because the floor is an INTER-pulse constraint and there is no previous
    /// pulse to be spaced from. The timestamp is shared with the train loop, so
    /// a ladder pulse after a burst waits out the floor from the burst's last
    /// pulse rather than from nothing.
    pub fn fire_next_pulse_cancellable(
        &mut self,
        should_continue: Option<&AtomicBool>,
    ) -> Result<bool, ServiceError> {
        let level = self.current_level_db();
        let spacing = self.cfg.rate_model.effective_min_spacing(self.firer.path());
        if !self.state.is_running() {
            return Err(ServiceError::BadState(format!(
                "fire from {:?} (arm first)",
                self.state
            )));
        }
        if !spacing.is_finite() {
            return Err(ServiceError::BadState(
                "rate model refuses all spacing (read cadence unsustainable)".into(),
            ));
        }
        if let Some(prev) = self.last_pulse_wall_s {
            if !self.wait_until(prev + spacing, should_continue) {
                // Cancelled inside the gap: nothing is queued at the device and
                // nothing fired. Not an abort - the operator asked.
                self.log.push(EventKind::Note {
                    text: "ladder pulse cancelled during the inter-pulse wait; nothing fired"
                        .into(),
                });
                return Ok(false);
            }
        }
        self.last_pulse_wall_s = Some(self.pacer.pulse_start_s());
        self.fire_one(level, spacing, true, PulseOrigin::Ladder)
    }

    /// Run the whole AUTO ladder: fire the scheduled pulses level by level
    /// until the ladder completes, then stop cleanly. Bounded by `max_pulses`
    /// (a safety cap). Requires `Armed` and an AUTO schedule. Returns the
    /// final state (`Stopped`, or `Aborted` on a protection trip).
    ///
    /// Every rung is WALL-clock paced and the cancel flag is read
    /// between rungs and throughout each inter-pulse wait, so a Stop lands
    /// within [`CANCEL_POLL_S`] of being raised instead of at the end of a
    /// multi-second gap.
    pub fn run_auto(&mut self, max_pulses: u64) -> Result<RunState, ServiceError> {
        if self.cfg.schedule.mode != ShockRunMode::Auto {
            return Err(ServiceError::BadState(
                "run_auto requires an AUTO schedule".into(),
            ));
        }
        if !matches!(self.state, RunState::Armed) {
            return Err(ServiceError::BadState(format!(
                "run_auto from {:?} (arm first)",
                self.state
            )));
        }
        let mut fired = 0u64;
        while fired < max_pulses {
            let completed = self.ladder.as_ref().map(|l| l.completed()).unwrap_or(true);
            if completed {
                break;
            }
            if self.cancel.load(Ordering::Relaxed) {
                break;
            }
            if !self.fire_next_pulse()? {
                if self.state == RunState::Aborted {
                    return Ok(self.state); // aborted inside the pipeline
                }
                break; // cancelled in a gap; fall through to the clean stop
            }
            fired += 1;
        }
        self.stop()?;
        Ok(self.state)
    }

    /// Replace the wall clock EVERY fire is paced against - trains and the AUTO
    /// ladder alike (default [`pacer::RealPacer`]).
    ///
    /// Exists so a spacing assertion does not cost its own spacing: a 3-pulse
    /// 2.0 s train tested against the real clock would add 4 s to the suite, and
    /// a suite that slow gets the assertion loosened rather than kept. The bench
    /// instruments use it for the same reason.
    ///
    /// Renamed from `set_train_pacer` when the AUTO ladder moved onto the same
    /// clock: a name that said "train" would have kept implying the ladder was
    /// somebody else's problem, which is how the two paths got out of step.
    pub fn set_pacer(&mut self, pacer: Box<dyn TrainPacer>) {
        self.pacer = pacer;
    }

    /// Fire the CONFIGURED pulse train at the
    /// current level. See [`Self::fire_train_with`].
    pub fn fire_train(&mut self) -> Result<TrainPlan, ServiceError> {
        let spec = self.cfg.train;
        self.fire_train_spec(spec, PulseOrigin::Train, None, None)
    }

    /// Fire an explicit pulse train at the current level. The spec is planned against
    /// the rate model + the path capability table first (a spacing below the floors, or
    /// an at-rate train on the delegated path, is REFUSED); the returned plan carries
    /// the HARDWARE-BLOCKED marker (the delegated write-triggers-fire semantic is
    /// undecoded). Trains fire at a PINNED level - they do not walk the AUTO ladder
    /// mid-burst (a gunfire burst is constant-level by definition).
    pub fn fire_train_with(&mut self, spec: TrainSpec) -> Result<TrainPlan, ServiceError> {
        self.fire_train_spec(spec, PulseOrigin::Train, None, None)
    }

    /// [`Self::fire_train_with`], plus a `should_continue` flag a HOST can lower
    /// to end the burst at the next inter-pulse boundary.
    ///
    /// The flag is TRUE to keep going (opposite sense to
    /// `eigen_hal::play_block_wise`'s `cancel`, which is TRUE to stop - they
    /// meet in this call path and are not interchangeable). It is read between
    /// pulses and every [`CANCEL_POLL_S`] during the inter-pulse wait, never
    /// inside a pulse: a pulse in flight is committed drive, and what happens to
    /// it is the ABORT path's decision (`eigen_hal::StreamError::Cancelled`),
    /// not a train-loop decision.
    pub fn fire_train_cancellable(
        &mut self,
        spec: TrainSpec,
        should_continue: Option<&AtomicBool>,
    ) -> Result<TrainPlan, ServiceError> {
        self.fire_train_spec(spec, PulseOrigin::Train, None, should_continue)
    }

    /// Fire the configured GUNFIRE burst (MIL-STD-810 519.8 Procedure IV):
    /// `rounds` pulses spaced `60 / rounds_per_min` seconds via
    /// `eigen_shock_common::gunfire_spacing`. The rate-model floor still
    /// applies - a rate faster than the floors allow is REFUSED.
    pub fn fire_gunfire_burst(&mut self) -> Result<TrainPlan, ServiceError> {
        self.fire_gunfire_burst_cancellable(None)
    }

    /// [`Self::fire_gunfire_burst`] with the same `should_continue` flag
    /// [`Self::fire_train_cancellable`] takes.
    pub fn fire_gunfire_burst_cancellable(
        &mut self,
        should_continue: Option<&AtomicBool>,
    ) -> Result<TrainPlan, ServiceError> {
        let g = self.cfg.gunfire.ok_or_else(|| {
            ServiceError::BadState("no gunfire configured (cfg.gunfire is None)".into())
        })?;
        let spacing = gunfire_spacing(g.rounds_per_min).map_err(|e| {
            self.log.push(EventKind::TrainRefused {
                reason: e.to_string(),
            });
            ServiceError::Train(e)
        })?;
        let spec = TrainSpec {
            count: g.rounds,
            spacing,
        };
        self.fire_train_spec(
            spec,
            PulseOrigin::Gunfire,
            Some(g.rounds_per_min),
            should_continue,
        )
    }

    /// The train loop. Paced in WALL time through [`Self::pacer`]:
    /// the rate model's floors are a thermal / settling / compute constraint on
    /// the HARDWARE, so enforcing them against a number this function increments
    /// enforces nothing. The pulse `k+1` deadline is `wall_time_of_pulse_k +
    /// realized_spacing_s`, measured, not accumulated - so a slow fire shows up
    /// as an overrun in [`TrainRecord::wall_times_s`] instead of being hidden by
    /// a shortened next gap.
    fn fire_train_spec(
        &mut self,
        spec: TrainSpec,
        origin: PulseOrigin,
        gunfire_rounds_per_min: Option<f64>,
        should_continue: Option<&AtomicBool>,
    ) -> Result<TrainPlan, ServiceError> {
        if !self.state.is_running() {
            return Err(ServiceError::BadState(format!(
                "fire_train from {:?} (arm first)",
                self.state
            )));
        }
        let plan = spec
            .plan(self.firer.path(), &self.cfg.rate_model)
            .map_err(|e| {
                self.log.push(EventKind::TrainRefused {
                    reason: e.to_string(),
                });
                ServiceError::Train(e)
            })?;
        self.log.push(EventKind::TrainPlanned {
            count: plan.count,
            realized_spacing_s: plan.realized_spacing_s,
            floor_s: plan.floor_s,
            hardware_blocked: plan.hardware_blocked,
            gunfire_rounds_per_min,
        });
        let level = self.current_level_db();
        let mut pulse_numbers = Vec::with_capacity(plan.count as usize);
        let mut wall_times_s = Vec::with_capacity(plan.count as usize);
        let mut cancelled = false;
        for i in 0..plan.count {
            if i > 0 {
                // The inter-pulse gap, in real time, off the last pulse's OWN
                // wall timestamp. `wait_until` returns false when the host
                // lowered `should_continue` - the burst ends HERE, between
                // pulses, with nothing queued at the device.
                let deadline = wall_times_s[wall_times_s.len() - 1] + plan.realized_spacing_s;
                if !self.wait_until(deadline, should_continue) {
                    cancelled = true;
                    break;
                }
            } else if !self.keep_going(should_continue) {
                // Lowered before the first pulse: fire nothing.
                cancelled = true;
                break;
            }
            let fired_at_wall_s = self.pacer.pulse_start_s();
            // Shared with the AUTO ladder: a ladder pulse after a burst
            // waits out the floor from the burst's LAST pulse, not from arm.
            self.last_pulse_wall_s = Some(fired_at_wall_s);
            let n = self.pulses_fired;
            let running = self.fire_one(level, plan.realized_spacing_s, false, origin)?;
            if self.pulses_fired > n {
                pulse_numbers.push(n);
                wall_times_s.push(fired_at_wall_s);
            }
            if !running {
                break;
            }
        }
        if cancelled {
            self.log.push(EventKind::TrainCancelled {
                fired: pulse_numbers.len() as u32,
                of_count: plan.count,
            });
        }
        self.train_records.push(TrainRecord {
            plan,
            pulse_numbers,
            wall_times_s,
            gunfire_rounds_per_min,
            level_db: level,
        });
        Ok(plan)
    }

    /// `true` when BOTH cancellation channels say keep going: the caller's
    /// `should_continue` (TRUE to continue) and the service's own cancel flag
    /// (TRUE to stop).
    ///
    /// Two channels because they answer to different people. `should_continue`
    /// belongs to whoever called `fire_train_cancellable`; the cancel flag is
    /// the one a host can raise WITHOUT the lock a fire in flight is holding,
    /// and it is the same flag the fire path itself watches per block.
    #[inline]
    fn keep_going(&self, should_continue: Option<&AtomicBool>) -> bool {
        still_running(should_continue) && !self.cancel.load(Ordering::Relaxed)
    }

    /// Wait until `deadline_s` on the injected pacer, re-reading both cancel
    /// channels every [`CANCEL_POLL_S`]. Returns `false` if either said stop
    /// (the caller must not fire), `true` if the deadline arrived.
    fn wait_until(&mut self, deadline_s: f64, should_continue: Option<&AtomicBool>) -> bool {
        loop {
            if !self.keep_going(should_continue) {
                return false;
            }
            let now = self.pacer.now_s();
            if now >= deadline_s {
                return true;
            }
            // Chunked, so a Stop raised DURING the gap is seen inside
            // CANCEL_POLL_S rather than at the end of a 2 s spacing. The LAST
            // chunk targets `deadline_s` itself rather than `now + remainder`:
            // the two differ by a float ulp, and on the wrong side of it the
            // `now >= deadline_s` test above spins on ever-smaller steps.
            if deadline_s - now <= CANCEL_POLL_S {
                self.pacer.sleep_until(deadline_s);
            } else {
                self.pacer.sleep_until(now + CANCEL_POLL_S);
            }
        }
    }

    /// The per-pulse pipeline (see the module docs). `spacing_s` is charged to
    /// the run clock AFTER the pulse (so a train's fire times are spaced by the
    /// realized spacing). Returns `true` if the run is still running.
    fn fire_one(
        &mut self,
        scheduled_level_db: f64,
        spacing_s: f64,
        tick_ladder: bool,
        origin: PulseOrigin,
    ) -> Result<bool, ServiceError> {
        if !self.state.is_running() {
            return Err(ServiceError::BadState(format!(
                "fire from {:?} (arm first)",
                self.state
            )));
        }
        if !spacing_s.is_finite() {
            return Err(ServiceError::BadState(
                "rate model refuses all spacing (read cadence unsustainable)".into(),
            ));
        }
        if self.state == RunState::Armed {
            self.transition(RunState::Firing);
        }

        let ladder_scale = db_scale(scheduled_level_db);
        // The between-pulse carry is consumed by THIS pulse.
        let mut extra_scale = self.next_pulse_scale;
        self.next_pulse_scale = 1.0;

        // 1. Pre-fire predictive peak-accel check: deterministic
        //    crest = 1 prediction on the commanded waveform. A
        //    predicted breach scales the pulse DOWN before firing; an invalid
        //    prediction refuses to fire.
        let predicted = self.full_peak_g * ladder_scale * extra_scale;
        let decision = self.cfg.drive_limit.pre_fire_accel(predicted);
        if !decision.allowed {
            if decision.scale_factor > 0.0 {
                extra_scale *= decision.scale_factor;
                self.drive_limit_actions.push(DriveLimitActionRecord {
                    pulse: self.pulses_fired,
                    phase: "pre-fire".into(),
                    scale: decision.scale_factor,
                    reason: decision.reason.clone(),
                });
                self.log.push(EventKind::DriveLimitAction {
                    pulse: self.pulses_fired,
                    phase: "pre-fire".into(),
                    scale: decision.scale_factor,
                    reason: decision.reason,
                });
            } else {
                self.latch_trip(TripKind::PreFireRefused {
                    reason: decision.reason,
                });
                return Ok(false);
            }
        }

        let commanded_db = scheduled_level_db + 20.0 * extra_scale.log10();
        let total_scale = ladder_scale * extra_scale;
        let commanded_amplitude_g = self.cfg.spec.amplitude_g * total_scale;

        // 2. Fire. Path B drives the scaled HOST-COMPENSATED composite; the
        //    delegated firer IGNORES the waveform (the device
        //    holds the uncompensated download) and fires the absolute setpoint.
        //
        //    The composite was synthesized at the ADOPTED rate in `arm_inner`;
        //    was `.expect("composite present at arm")`.
        let composite = self.composite.as_ref().ok_or_else(|| {
            ServiceError::Internal("no composite while running (arm builds it)".into())
        })?;
        let drive: Vec<f64> = composite
            .waveform_g
            .iter()
            .map(|&v| v * total_scale)
            .collect();
        let fired_at_s = self.run_clock_s;
        let fired_at_wall_s = self.last_pulse_wall_s.unwrap_or(0.0);
        // The host's live channels for THIS fire (the cancel flag + the block observer).
        // The observer is moved out and back so the borrow checker sees one
        // mutable path into `self` at a time; a fire that panicked would drop
        // the observer, and a dropped observer is a tap that stops beating
        // rather than one that reports stale progress.
        let cancel = Arc::clone(&self.cancel);
        let mut observer = self.block_observer.take();
        let fired = {
            let mut ctl = FireControl {
                cancel: Some(cancel.as_ref()),
                observer: observer.as_deref_mut(),
            };
            self.firer.fire(&drive, commanded_db, &mut ctl)
        };
        self.block_observer = observer;

        // DRIVER DIAGNOSTICS, drained after EVERY fire including a failed one:
        // the NI backend has always collected DAQmx underruns and
        // coercions, and nothing in the shock path ever asked for them.
        let pulse_no = self.pulses_fired;
        self.drain_warnings(Some(pulse_no));

        let measured = match fired {
            Ok(m) => m,
            Err(ServiceError::Cancelled {
                frames_done,
                frames_committed,
                frames_expected,
            }) => {
                // The operator asked. Not a fault, not a trip: record the
                // committed-queue arithmetic, park the fire path, and land in
                // Stopped. What the device had already accepted kept playing -
                // mid-record preemption is refused by this chassis.
                self.log.push(EventKind::FireCancelled {
                    pulse: pulse_no,
                    frames_done,
                    frames_committed,
                    frames_expected,
                });
                let _ = self.stop_capture_inner();
                let _ = self.firer.stop();
                if self.state != RunState::Aborted {
                    self.transition(RunState::Stopped);
                }
                return Ok(false);
            }
            Err(e) => return Err(e),
        };
        self.pulses_fired += 1;
        self.run_clock_s += spacing_s;

        let mut row = PulseRow {
            pulse: pulse_no,
            store_seq: None,
            origin,
            scheduled_level_db,
            commanded_db,
            commanded_amplitude_g,
            drive_limit_scale: extra_scale,
            at_run_time_s: fired_at_s,
            at_wall_time_s: fired_at_wall_s,
            measured: false,
            peak_g: None,
            velocity_change_ips: None,
            pk_pk_disp_in: None,
            tolerance_overall: None,
            envelope_pass: None,
            velocity_change_pass: None,
            substitution_pass: None,
            srs_band_pass: None,
            srs_bands_pass: None,
            srs_bands_total: None,
            srs_worst_dev_db: None,
        };
        let mut abort_after: Option<TripKind> = None;

        match measured {
            Some(record_g) => {
                row.measured = true;
                // 3. Record: ingest -> kinematics + SRS (+ PVSS).
                let fired_at_unix = self.base_unix + fired_at_s.max(0.0) as u64;
                // At the ADOPTED rate: kinematics integrate a SAMPLE COUNT into
                // seconds, so this is the number that decides whether the
                // measured pulse is 11 ms or 1.1 ms wide.
                let seq = self.store.ingest(
                    record_g.clone(),
                    self.fs_hz(),
                    self.srs_freqs.clone(),
                    commanded_db,
                    self.cfg.compute_pvss,
                    fired_at_unix,
                );
                row.store_seq = Some(seq);

                let (peak_g, peak_velocity_ips, pk_pk_disp_in, meas_srs, record_len) = {
                    // Was `.expect("just ingested")`.
                    let rec = self.store.get(seq).ok_or_else(|| {
                        ServiceError::Internal(format!(
                            "pulse store lost record {seq} between ingest and read"
                        ))
                    })?;
                    if let Some(sink) = self.capture.as_ref() {
                        let _ = capture_pulse(sink, rec, 1);
                    }
                    (
                        rec.kinematics.peak_accel_g,
                        rec.kinematics.peak_velocity_ips,
                        rec.kinematics.pk_pk_disp_in,
                        rec.srs.clone(),
                        rec.time_history.len(),
                    )
                };
                row.peak_g = Some(peak_g);
                row.pk_pk_disp_in = Some(pk_pk_disp_in);

                // SRS fidelity per band (measured vs COMMANDED drive SRS) -
                // informational; the tolerance checks are the acceptance.
                let (band_pass, worst_db) = self.srs_band_check(&meas_srs, &drive, record_len);
                row.srs_bands_total = Some(band_pass.len());
                row.srs_bands_pass = Some(band_pass.iter().filter(|&&b| b).count());
                row.srs_worst_dev_db = Some(worst_db);
                row.srs_band_pass = Some(band_pass);

                // 4. Tolerance at the COMMANDED amplitude. A
                //    drive-limit scale-down is a deliberate deviation from the
                //    schedule; the pulse is judged against what was COMMANDED
                //    (the deviation itself is recorded in the row + actions).
                //    At the ADOPTED rate: the checker generates the nominal
                //    pulse at `fs_hz` and its surround / dV windows are
                //    `fraction * n_nominal` SAMPLES, so the wrong rate moves
                //    every window boundary.
                let mut scaled_spec: ClassicalPulseSpec = self.cfg.spec.clone();
                scaled_spec.amplitude_g = commanded_amplitude_g;
                let tol = check_tolerance(&record_g, &scaled_spec, self.fs_hz(), self.cfg.standard);
                row.tolerance_overall = Some(tol.overall_pass);
                row.envelope_pass = Some(tol.amplitude_envelope.pass);
                row.velocity_change_pass = Some(tol.velocity_change.pass);
                row.substitution_pass = Some(tol.substitution.pass);
                row.velocity_change_ips = Some(tol.velocity_change.measured_dv_ips);
                self.tolerance_checked += 1;
                if tol.overall_pass {
                    self.tolerance_passed += 1;
                    self.alarm_active = false;
                }

                // 5. Kinematic breach vs the shaker record (run-time check).
                if let Some(shaker) = &self.cfg.shaker {
                    if let Some(v) = &shaker.max_velocity_ips {
                        if peak_velocity_ips > v.value {
                            abort_after = Some(TripKind::Kinematic {
                                quantity: "peak velocity (in/s)".into(),
                                value: peak_velocity_ips,
                                limit: v.value,
                                pulse: pulse_no,
                            });
                        }
                    }
                    if abort_after.is_none() {
                        if let Some(d) = &shaker.max_displacement_in_pp {
                            if pk_pk_disp_in > d.value {
                                abort_after = Some(TripKind::Kinematic {
                                    quantity: "pk-pk displacement (in)".into(),
                                    value: pk_pk_disp_in,
                                    limit: d.value,
                                    pulse: pulse_no,
                                });
                            }
                        }
                    }
                }

                // 6. Between-pulse measured enforcement: scale the
                //    NEXT pulse; V/I monitors inert on Path A per the table.
                let caps = self.firer.path().capabilities();
                let peaks = vec![MeasuredPeak {
                    channel: self.control_channel_name.clone(),
                    peak_eu: peak_g,
                }];
                let bp = self.cfg.drive_limit.between_pulse(&caps, &peaks);
                if bp.scale_factor < 1.0 {
                    self.next_pulse_scale = bp.scale_factor;
                }
                for a in bp.actions {
                    if a.scale_factor < 1.0 {
                        self.drive_limit_actions.push(DriveLimitActionRecord {
                            pulse: pulse_no,
                            phase: "between-pulse".into(),
                            scale: a.scale_factor,
                            reason: a.reason.clone(),
                        });
                        self.log.push(EventKind::DriveLimitAction {
                            pulse: pulse_no,
                            phase: "between-pulse".into(),
                            scale: a.scale_factor,
                            reason: a.reason,
                        });
                    }
                }

                // Tolerance verdict -> abort or alarm.
                if abort_after.is_none() && !tol.overall_pass {
                    if self.cfg.abort_on_tolerance_failure {
                        abort_after = Some(TripKind::Tolerance {
                            pulse: pulse_no,
                            envelope_pass: tol.amplitude_envelope.pass,
                            velocity_change_pass: tol.velocity_change.pass,
                            error_frac: tol.velocity_change.error_frac,
                            worst_deviation_frac_of_peak: tol
                                .amplitude_envelope
                                .worst_deviation_frac_of_peak,
                        });
                    } else {
                        self.alarm_active = true;
                        self.log.push(EventKind::ToleranceAlarm {
                            pulse: pulse_no,
                            error_frac: tol.velocity_change.error_frac,
                            worst_deviation_frac_of_peak: tol
                                .amplitude_envelope
                                .worst_deviation_frac_of_peak,
                        });
                    }
                }
                self.last_tolerance = Some(tol);
            }
            None => {
                // Delegated path: fired, unobservable - honesty
                // note once, never a fabricated record.
                if !self.delegated_note_logged {
                    self.delegated_note_logged = true;
                    self.log.push(EventKind::Note {
                        text: "Pulse response is NOT observable on this drive path (inbound \
                               telemetry undecoded, finding F8): pulses are recorded as FIRED \
                               without measurement; tolerance/SRS/kinematics not evaluated."
                            .into(),
                    });
                }
            }
        }

        self.log.push(EventKind::PulseFired {
            pulse: pulse_no,
            scheduled_level_db,
            commanded_db,
            measured: row.measured,
            peak_g: row.peak_g,
            velocity_change_ips: row.velocity_change_ips,
            tolerance_pass: row.tolerance_overall,
        });
        self.pulse_rows.push(row);

        if let Some(kind) = abort_after {
            self.latch_trip(kind);
            return Ok(false);
        }

        // 7. Ladder tick. Trains are level-pinned and skip this.
        if tick_ladder {
            let change = {
                // Was `.expect("ladder present while running")`.
                let ladder = self.ladder.as_mut().ok_or_else(|| {
                    ServiceError::Internal("no ladder while running (ladder tick)".into())
                })?;
                let before = ladder.current_level_db();
                ladder.on_pulse_fired();
                let after = ladder.current_level_db();
                ((after - before).abs() > 1e-12).then_some((before, after))
            };
            if let Some((from_db, to_db)) = change {
                self.level_history.push(LevelHistoryEntry {
                    level_db: to_db,
                    auto: true,
                    at_pulse: self.pulses_fired,
                });
                self.log.push(EventKind::LevelChange {
                    from_db,
                    to_db,
                    auto: true,
                });
            }
        }
        Ok(true)
    }

    /// Per-band SRS fidelity (the report's "SRS pass/fail per band" column):
    /// the measured pulse SRS vs the SRS of the COMMANDED drive waveform
    /// (zero-padded to the record length so residual response develops), each
    /// band passing when the deviation is within `srs_band_tol_db`.
    /// Informational - the time-domain tolerance checks are the classical acceptance.
    fn srs_band_check(
        &self,
        measured_srs: &[f64],
        commanded_drive_g: &[f64],
        record_len: usize,
    ) -> (Vec<bool>, f64) {
        let mut reference = commanded_drive_g.to_vec();
        if reference.len() < record_len {
            reference.resize(record_len, 0.0);
        }
        let ref_srs = srs_analyze(&reference, self.fs_hz(), &self.srs_freqs, DEFAULT_SRS_Q);
        let tol = self.cfg.srs_band_tol_db;
        let mut worst = 0.0f64;
        let mut flags = Vec::with_capacity(measured_srs.len());
        for (&m, &r) in measured_srs.iter().zip(ref_srs.iter()) {
            if r <= 0.0 && m <= 0.0 {
                flags.push(true);
                continue;
            }
            if r <= 0.0 || m <= 0.0 {
                worst = f64::INFINITY;
                flags.push(false);
                continue;
            }
            let dev = 20.0 * (m / r).log10();
            if dev.abs() > worst {
                worst = dev.abs();
            }
            flags.push(dev.abs() <= tol);
        }
        (flags, worst)
    }

    // --- abort + stop ---------------------------------------------------------

    /// Drain the fire path's driver diagnostics into the event log
    /// (`Backend::take_warnings`).
    ///
    /// Called after every fire, at arm, and at every terminal path. Absence of a
    /// drain is how a real DAQmx underrun goes missing: the NI backend has
    /// always collected these, and until this landed nothing in the shock path
    /// had the `&mut` needed to ask for them.
    fn drain_warnings(&mut self, pulse: Option<u64>) {
        for text in self.firer.take_warnings() {
            self.log.push(EventKind::DriverWarning { pulse, text });
        }
    }

    /// Latch a protection trip: record it, EMERGENCY-stop the fire path, close
    /// capture, transition to `Aborted`. The single abort path for every
    /// protection, and for the operator's [`Self::abort`].
    fn latch_trip(&mut self, kind: TripKind) {
        let record = TripRecord {
            kind,
            at_level_db: self.current_level_db(),
            at_pulse: self.pulses_fired,
        };
        self.log.push(EventKind::Trip {
            record: record.clone(),
        });
        self.trip = Some(record);
        // Raise the cancel flag FIRST: a fire in flight (on another thread,
        // reaching this service through the host's lock) checks it per block,
        // and raising it after the emergency stop would let one more block of
        // drive be offered to a task that is being killed.
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.stop_capture_inner();
        let _ = self.firer.emergency_stop();
        self.drain_warnings(None);
        self.transition(RunState::Aborted);
    }

    /// Clean STOP: cancel at the next boundary, close capture, park
    /// the fire path, transition to `Stopped` (unless already `Aborted`).
    ///
    /// # What "at the next boundary" means here
    ///
    /// Raising the cancel flag is the FIRST thing this does, because it is the
    /// only part that reaches a fire already in flight: the service is
    /// synchronous, so a host calling `stop()` through a `Mutex` would otherwise
    /// be waiting behind the fire it is trying to end. The pulse in flight
    /// completes or returns `Cancelled` at its next block boundary; a train ends
    /// at its next INTER-pulse boundary with nothing queued.
    ///
    /// The fire path's own shutdown ramp is a documented NO-OP for a transient
    /// producer: every shock record ends in the quiet lead-out, so there is
    /// nothing to taper from. "Stop is ramped" is a true statement about the
    /// backend and a vacuous one about this module, and pretending otherwise
    /// would be claiming a protection that never engages.
    pub fn stop(&mut self) -> Result<(), ServiceError> {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.stop_capture_inner();
        if self.state != RunState::Aborted {
            if self.state.is_running() {
                let _ = self.firer.stop();
            }
            self.drain_warnings(None);
            self.transition(RunState::Stopped);
        }
        Ok(())
    }

    /// Operator ABORT: cancel + `Backend::emergency_stop` + latch
    /// `Aborted`.
    ///
    /// # How fast this actually is, with the arithmetic
    ///
    /// Not instant, and the number matters more than the word. Two terms bound
    /// how long drive stays on the wire after this returns:
    ///
    /// * **The committed queue.** Mid-record preemption of a DAQmx output is
    ///   REFUSED by the cDAQ-9178 (`-200289`), so whatever the device
    ///   has already ACCEPTED plays out. The cancel is checked once per block,
    ///   and the backend's own lookahead is what is queued ahead of the read
    ///   cursor: at the 1024-frame block and 2-block lookahead this backend
    ///   configures, that is up to 3072 frames, which at 51 200 Hz is ~60 ms.
    ///   **That bound is longer than the entire pulse** (an 11 ms half-sine with
    ///   its compensation shoulders is ~44 ms), which is the real finding:
    ///   "kill the drive mid-record" does not exist on this hardware, and a
    ///   shock abort is a promise about the NEXT pulse, not this one.
    /// * **The emergency ramp.** `emergency_stop` tapers over its own
    ///   `ABORT_RAMP_MS` before killing the tasks - from the last frame WRITTEN,
    ///   which for a shock record is a zero in the lead-out, so the taper is a
    ///   no-op here and costs only its own duration.
    ///
    /// `emergency_stop` also UNRESERVES both tasks, which is what makes
    /// abort -> re-arm -> fire work instead of restarting a zombie AO that
    /// "succeeds" while consuming nothing (measured 2026-08-03). And it stops
    /// the INPUT task too, so **abort evidence can never be
    /// AI-side** - the DMM on the output node is the only admissible witness.
    pub fn abort(&mut self) -> Result<(), ServiceError> {
        self.abort_with_reason("")
    }

    /// [`Self::abort`] with the host's reason recorded verbatim in the trip.
    pub fn abort_with_reason(&mut self, reason: &str) -> Result<(), ServiceError> {
        if self.state == RunState::Aborted {
            return Ok(());
        }
        self.latch_trip(TripKind::OperatorAbort {
            reason: reason.to_string(),
        });
        Ok(())
    }

    // --- capture + snapshot ---------------------------------------------------

    /// Start the durable capture sink at `data_root/<run_name>/` (every
    /// measured pulse's record is streamed to `telemetry.f32`).
    pub fn start_capture(&mut self, run_name: &str) -> Result<PathBuf, ServiceError> {
        if let Some(sink) = self.capture.as_ref() {
            return Ok(sink.run_dir().to_path_buf());
        }
        let cap_cfg = CaptureConfig {
            n_channels: 1,
            // The ADOPTED rate: the sidecar's declared rate is what
            // any later reader denominates the capture against, and a sidecar
            // that says 5120 over samples clocked at 51 200 is a file that
            // lies about its own time base.
            sample_rate_hz: self.fs_hz(),
            decimation: 1,
            ring_capacity: 256,
        };
        let settings_json = serde_json::to_string(&self.cfg).unwrap_or_else(|_| "{}".into());
        let sink = CaptureSink::start(
            &self.data_root,
            run_name,
            cap_cfg,
            self.table.clone(),
            settings_json,
        )
        .map_err(|e| ServiceError::Io(e.to_string()))?;
        let run_dir = sink.run_dir().to_path_buf();
        self.log.push(EventKind::CaptureStart {
            run_dir: run_dir.display().to_string(),
        });
        self.capture = Some(sink);
        Ok(run_dir)
    }

    /// Stop the capture, flush the sidecar, and log the drop count. No-op if
    /// not capturing.
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

    /// Save a snapshot bundle: the full pulse store + the run
    /// context as the shock `Snapshot` payload, plus the caller's marks.
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
        let settings_json = serde_json::to_string(&self.cfg).unwrap_or_else(|_| "{}".into());
        let payload = self
            .store
            .snapshot_payload(self.current_level_db(), DEFAULT_SRS_Q, None);
        let snapshot = build_snapshot(unix_secs, settings_json, marks, payload);
        let path =
            save_snapshot(&run_dir, &snapshot).map_err(|e| ServiceError::Io(e.to_string()))?;
        self.log.push(EventKind::Snapshot {
            path: path.display().to_string(),
        });
        Ok(path)
    }

    // --- report + status ---------------------------------------------------------

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
            })
            .collect();
        let path = self.firer.path();
        ReportData {
            settings: self.cfg.clone(),
            channels,
            path,
            capabilities: path.capabilities(),
            feasibility: self.feasibility.clone(),
            pulses: self.pulse_rows.clone(),
            level_history: self.level_history.clone(),
            event_log: self.log.clone(),
            tolerance: ToleranceSummary {
                standard: self.cfg.standard,
                params: self.cfg.standard.params(),
                checked: self.tolerance_checked,
                passed: self.tolerance_passed,
                last: self.last_tolerance.clone(),
            },
            drive_limit_actions: self.drive_limit_actions.clone(),
            trains: self.train_records.clone(),
            trip: self.trip.clone(),
            metadata: self.cfg.metadata.clone(),
            honesty: Honesty::for_path(path),
        }
    }

    /// The live status snapshot the UI polls (the status surface).
    pub fn status(&self) -> Status {
        Status {
            state: self.state,
            path: self.firer.path(),
            current_level_db: self.current_level_db(),
            level_index: self.ladder.as_ref().map(|l| l.level_index()).unwrap_or(0),
            pulses_at_level: self
                .ladder
                .as_ref()
                .map(|l| l.pulses_at_level())
                .unwrap_or(0),
            ladder_completed: self.ladder.as_ref().map(|l| l.completed()).unwrap_or(false),
            pulses_fired: self.pulses_fired,
            pulses_recorded: self.store.len(),
            last_tolerance_pass: self.last_tolerance.as_ref().map(|t| t.overall_pass),
            last_peak_g: self.store.last().map(|r| r.kinematics.peak_accel_g),
            last_velocity_change_ips: self
                .last_tolerance
                .as_ref()
                .map(|t| t.velocity_change.measured_dv_ips),
            alarm_active: self.alarm_active,
            abort_active: self.state == RunState::Aborted,
            capturing: self.capture.is_some(),
            next_pulse_scale: self.next_pulse_scale,
            run_clock_s: self.run_clock_s,
            // BOTH rates, never one. A UI that shows a single number
            // cannot tell an operator that the box coerced their request, and
            // the coercion on this chassis is a factor of ten.
            requested_fs_hz: self.cfg.fs_hz,
            actual_fs_hz: self.fs_actual_hz,
        }
    }
}

/// The SRS analysis grid for `cfg` at `fs_hz`, bounded by Nyquist.
///
/// Returns the grid plus `Some(nyquist)` when the configured upper edge had to
/// be clamped. A band ABOVE Nyquist is a natural frequency the record cannot
/// represent, and `srs_analyze` would happily return a number for it - the exact
/// shape of a result that means nothing and looks like a result. The grid is
/// re-derived at arm because the config cannot know the device's rate: a profile
/// written for 5120 Hz asks for bands up to 2000 Hz, which is fine at 51 200 and
/// would be past Nyquist at 2048.
fn srs_grid(
    cfg: &ClassicalTestConfig,
    fs_hz: f64,
) -> Result<(Vec<f64>, Option<f64>), ServiceError> {
    let nyquist = 0.5 * fs_hz;
    let clamped = cfg.srs_f_hi_hz >= nyquist;
    let hi = if clamped { nyquist } else { cfg.srs_f_hi_hz };
    let mut freqs = octave_freqs(
        cfg.srs_f_lo_hz,
        hi,
        OctaveFraction::Arbitrary(cfg.srs_bands_per_octave),
    );
    freqs.retain(|f| *f < nyquist);
    if freqs.is_empty() {
        return Err(ServiceError::Config(ConfigError::BadConfig(format!(
            "SRS analysis grid is empty: no band between {:.4} and {:.4} Hz is below the \
             {nyquist:.4} Hz Nyquist limit",
            cfg.srs_f_lo_hz, cfg.srs_f_hi_hz
        ))));
    }
    Ok((freqs, clamped.then_some(nyquist)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{ClassicalPulseSpec, ClassicalShape};
    use eigen_dsp::shock::{CompensationConfig, CompensationShape};
    use eigen_shaker_db::{RatingValue, ShakerRecord};
    use eigen_shock_common::{
        DriveLimit, DriveMonitor, DriveMonitorKind, DrivePath, RateModel, ShockLevelSchedule,
    };

    /// A scripted fire path for unit tests: "measures" the commanded drive
    /// times a gain (clean plant = 1.0), padded with a short quiet surround,
    /// or reports the pulse unobservable (the delegated posture).
    ///
    /// `actual_fs_hz` is the rate-adoption double: a firer whose
    /// device rate DIFFERS from the profile's request is the whole point, and a
    /// double that could only ever agree with the config would make the adoption
    /// tests unable to fail.
    struct ScriptedFirer {
        path: DrivePath,
        gain: f64,
        observable: bool,
        armed: bool,
        stopped: bool,
        estopped: bool,
        fired_db: Vec<f64>,
        /// Frames of drive handed to each fire, in order.
        fired_lens: Vec<usize>,
        /// The device rate this firer reports after arm.
        actual_fs_hz: f64,
        /// `false` = report no rate at all, the posture the service must refuse.
        reports_rate: bool,
        quiet_frames: usize,
        warnings: Vec<String>,
        /// Raised by the fire to simulate a cancel landing mid-record.
        cancel_after_frames: Option<usize>,
    }

    impl ScriptedFirer {
        fn new(path: DrivePath, gain: f64, observable: bool) -> Self {
            ScriptedFirer {
                path,
                gain,
                observable,
                armed: false,
                stopped: false,
                estopped: false,
                fired_db: Vec::new(),
                fired_lens: Vec::new(),
                // Agrees with `config::DEFAULT_FS_HZ`, so every fixture that is
                // not about rate adoption behaves as it always did.
                actual_fs_hz: crate::service::config::DEFAULT_FS_HZ,
                reports_rate: true,
                quiet_frames: 0,
                warnings: Vec::new(),
                cancel_after_frames: None,
            }
        }

        fn host(gain: f64) -> Self {
            ScriptedFirer::new(DrivePath::HostDac, gain, true)
        }

        fn delegated_like() -> Self {
            ScriptedFirer::new(DrivePath::Delegated, 1.0, false)
        }

        /// A host firer whose DEVICE runs at `fs_hz` whatever the profile asked
        /// for - the coercing chassis in miniature.
        fn host_at_rate(gain: f64, fs_hz: f64) -> Self {
            let mut f = ScriptedFirer::host(gain);
            f.actual_fs_hz = fs_hz;
            f
        }
    }

    impl PulseFirer for ScriptedFirer {
        fn path(&self) -> DrivePath {
            self.path
        }
        fn arm(&mut self) -> Result<(), ServiceError> {
            self.armed = true;
            Ok(())
        }
        fn actual_fs_hz(&self) -> Option<f64> {
            self.reports_rate.then_some(self.actual_fs_hz)
        }
        fn set_quiet_frames(&mut self, frames: usize) {
            self.quiet_frames = frames;
        }
        fn fire(
            &mut self,
            drive_g: &[f64],
            commanded_db: f64,
            ctl: &mut FireControl<'_>,
        ) -> Result<Option<Vec<f64>>, ServiceError> {
            assert!(self.armed, "fire before arm");
            self.fired_db.push(commanded_db);
            self.fired_lens.push(drive_g.len());
            if !self.observable {
                return Ok(None);
            }
            let quiet = self.quiet_frames;
            let total = drive_g.len() + 2 * quiet;
            // One "block" per 32 frames, so the observer tap has something to
            // report and a cancel has a boundary to land on.
            let mut delivered = 0usize;
            let mut rec = Vec::with_capacity(total);
            while delivered < total {
                if ctl.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
                    || self.cancel_after_frames.is_some_and(|n| delivered >= n)
                {
                    return Err(ServiceError::Cancelled {
                        frames_done: delivered,
                        frames_committed: (delivered + 64).min(total),
                        frames_expected: total,
                    });
                }
                let n = 32usize.min(total - delivered);
                for i in delivered..delivered + n {
                    let v = if i < quiet || i >= quiet + drive_g.len() {
                        0.0
                    } else {
                        drive_g[i - quiet] * self.gain
                    };
                    rec.push(v);
                }
                delivered += n;
                if let Some(obs) = ctl.observer.as_mut() {
                    obs(delivered, total);
                }
            }
            Ok(Some(rec))
        }
        fn take_warnings(&mut self) -> Vec<String> {
            std::mem::take(&mut self.warnings)
        }
        fn stop(&mut self) -> Result<(), ServiceError> {
            self.stopped = true;
            Ok(())
        }
        fn emergency_stop(&mut self) -> Result<(), ServiceError> {
            self.estopped = true;
            self.stopped = true;
            Ok(())
        }
    }

    /// The sim_path_b geometry: rectangular shoulders at 4x the main duration
    /// solve to ~-8% of the peak - inside the MIL surround band.
    fn spec_20g_11ms() -> ClassicalPulseSpec {
        let mut spec = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, 11.0);
        spec.compensation = CompensationConfig {
            shape: CompensationShape::Rectangular,
            pre_frac: 4.0,
            post_frac: 4.0,
            symmetric: true,
        };
        spec
    }

    fn cfg() -> ClassicalTestConfig {
        ClassicalTestConfig::new(spec_20g_11ms())
    }

    fn svc(cfg: ClassicalTestConfig, firer: ScriptedFirer) -> ClassicalShockService<ScriptedFirer> {
        // Fixture table: these tests name its CH1 Control row (deprecated outside tests).
        #[allow(deprecated)]
        let mut s = ClassicalShockService::new(
            cfg,
            ChannelTable::nonuniform_default_8ch(),
            firer,
            Some(std::env::temp_dir().join("eigen-shock-host-unit")),
        )
        .unwrap();
        // EVERY fixture here runs on a VIRTUAL clock, because since 2026-08-08 the
        // AUTO ladder really waits: a 5-rung ladder at the conservative default
        // 1.0 s floor is 4 s of wall time per fixture, and the delegated
        // fixture's 20 s floor is 80 s. The virtual pacer keeps the wall-time
        // assertions real (they read the timeline the loop actually waited out)
        // and costs nothing. `service_sim.rs`'s `g_` and `h_` rows are the
        // controls that prove the SHIPPING default is the real clock.
        s.set_pacer(Box::new(pacer::VirtualPacer::new(0.010)));
        s
    }

    #[test]
    fn state_machine_gates_the_order() {
        let mut s = svc(cfg(), ScriptedFirer::host(1.0));
        assert_eq!(s.state(), RunState::Idle);
        // fire / arm before feasibility are refused.
        assert!(matches!(
            s.fire_next_pulse(),
            Err(ServiceError::BadState(_))
        ));
        assert!(matches!(s.arm(), Err(ServiceError::BadState(_))));
        s.assess_feasibility().unwrap();
        assert_eq!(s.state(), RunState::Feasibility);
        // arm_with_override is only for the blocked state.
        assert!(matches!(
            s.arm_with_override("ack"),
            Err(ServiceError::BadState(_))
        ));
        s.arm().unwrap();
        assert_eq!(s.state(), RunState::Armed);
        assert!(s.fire_next_pulse().unwrap());
        assert_eq!(s.state(), RunState::Firing);
        s.stop().unwrap();
        assert_eq!(s.state(), RunState::Stopped);
    }

    #[test]
    fn auto_ladder_fires_the_schedule_and_passes_tolerance() {
        let mut s = svc(cfg(), ScriptedFirer::host(1.0));
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        let end = s.run_auto(100).unwrap();
        assert_eq!(end, RunState::Stopped);

        let report = s.report_data();
        assert_eq!(report.pulses.len(), 5, "default ladder = 5 pulses");
        for row in &report.pulses {
            assert!(row.measured);
            assert_eq!(row.tolerance_overall, Some(true), "{row:?}");
            assert!(row.srs_bands_total.unwrap() > 0);
        }
        let levels: Vec<f64> = report.pulses.iter().map(|r| r.scheduled_level_db).collect();
        assert_eq!(levels, vec![-12.0, -9.0, -6.0, -3.0, 0.0]);
        assert_eq!(report.tolerance.checked, 5);
        assert_eq!(report.tolerance.passed, 5);
        // Level history: start + 4 auto advances.
        assert_eq!(report.level_history.len(), 5);
        assert_eq!(s.pulse_store().len(), 5);
    }

    #[test]
    fn tolerance_breach_aborts_the_run() {
        // A 1.3x plant gain: +30% amplitude fails both MIL bands on the first
        // pulse and the run aborts with a Tolerance trip.
        let mut s = svc(cfg(), ScriptedFirer::host(1.3));
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        let end = s.run_auto(100).unwrap();
        assert_eq!(end, RunState::Aborted);
        let trip = s.trip().expect("trip recorded");
        assert!(matches!(trip.kind, TripKind::Tolerance { pulse: 0, .. }));
        assert_eq!(s.report_data().tolerance.passed, 0);
        assert!(s.status().abort_active);
    }

    #[test]
    fn tolerance_breach_alarms_when_abort_disabled() {
        let mut c = cfg();
        c.abort_on_tolerance_failure = false;
        let mut s = svc(c, ScriptedFirer::host(1.3));
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        let end = s.run_auto(100).unwrap();
        assert_eq!(end, RunState::Stopped, "alarm-only posture completes");
        assert_eq!(s.report_data().pulses.len(), 5);
        assert!(s.status().alarm_active);
        assert!(s
            .log()
            .events()
            .iter()
            .any(|e| matches!(e.kind, EventKind::ToleranceAlarm { .. })));
    }

    #[test]
    fn between_pulse_monitor_scales_the_next_pulse() {
        // A between-pulse ceiling on the control channel below the first
        // pulse's measured peak: the NEXT pulse is scaled by ceiling/peak.
        let mut c = cfg();
        c.drive_limit = DriveLimit {
            peak_accel_g: None,
            monitors: vec![DriveMonitor {
                channel: "ch1".into(), // nonuniform_default_8ch CH1 = Control
                eu: "g".into(),
                peak_ceiling: 4.0,
                kind: DriveMonitorKind::BetweenPulse,
            }],
        };
        // Alarm-only so the (scaled) later pulses do not abort the assertion
        // path; the clean gain keeps tolerance passing anyway.
        let mut s = svc(c, ScriptedFirer::host(1.0));
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        // Pulse 0 at -12 dB: peak ~5.02 g > 4.0 ceiling -> next scale ~0.796.
        assert!(s.fire_next_pulse().unwrap());
        let expected_scale = 4.0 / (20.0 * db_scale(-12.0));
        let st = s.status();
        assert!(
            (st.next_pulse_scale - expected_scale).abs() < 0.01,
            "carry {} vs {}",
            st.next_pulse_scale,
            expected_scale
        );
        // Pulse 1 consumes the carry.
        assert!(s.fire_next_pulse().unwrap());
        let report = s.report_data();
        assert!((report.pulses[1].drive_limit_scale - expected_scale).abs() < 0.01);
        assert!(report
            .drive_limit_actions
            .iter()
            .any(|a| a.phase == "between-pulse" && a.scale < 1.0));
        s.stop().unwrap();
    }

    #[test]
    fn delegated_posture_fires_without_measurement_and_refuses_trains() {
        let mut c = cfg();
        c.schedule = ShockLevelSchedule::auto_default();
        let mut s = svc(c, ScriptedFirer::delegated_like());
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        // A multi-pulse train on the delegated path is refused.
        let err = s.fire_train_with(TrainSpec::max_rate(3)).unwrap_err();
        assert!(matches!(
            err,
            ServiceError::Train(TrainError::AtRateNotOnPath { .. })
        ));
        // The ladder fires (setpoints only), pulses recorded WITHOUT records.
        let end = s.run_auto(100).unwrap();
        assert_eq!(end, RunState::Stopped);
        let report = s.report_data();
        assert_eq!(report.pulses.len(), 5);
        assert!(report.pulses.iter().all(|r| !r.measured));
        assert_eq!(
            s.pulse_store().len(),
            0,
            "no fabricated records (telemetry undecoded)"
        );
        assert_eq!(report.tolerance.checked, 0);
        assert!(
            !report.capabilities.host_compensation,
            "host compensation greyed"
        );
        assert!(s
            .log()
            .events()
            .iter()
            .any(|e| matches!(&e.kind, EventKind::Note { text } if text.contains("F8"))));
    }

    /// Trains and gunfire bursts fire at the planned spacing IN WALL TIME.
    ///
    /// REPLACES the old assertion in this test, which read consecutive
    /// `PulseRow::at_run_time_s` values and checked their deltas against
    /// `realized_spacing_s`. That field is the simulated `run_clock_s`, whose
    /// only mutation is `run_clock_s += spacing_s` - the assertion was
    /// `(x + s) - x == s` and could not fail for any implementation, including
    /// the one this test was ostensibly guarding: a train that fires five pulses back
    /// to back as fast as the fire path returns. It is the documented cannot-fail shape
    /// (a result that cannot fail is not evidence), and it stayed green through the
    /// entire period when NOTHING waited between pulses.
    ///
    /// What is asserted now is `TrainRecord::wall_times_s`, read off the
    /// injected `TrainPacer`. RED-MUTATION: delete the `wait_until` call in
    /// `fire_train_spec`'s loop and every wall delta collapses to the
    /// `VirtualPacer`'s `fire_cost_s` (0.01 s), reddening both loops below. The
    /// `at_run_time_s` checks are kept, one line each, purely to pin that the
    /// REPORTING clock still tracks the plan - they are not the spacing
    /// evidence and are labelled so.
    ///
    /// The pacer is virtual so this costs no wall time; a real-clock version of
    /// this test would add 3 x 0.5 + 2 x 2.0 = 5.5 s to the suite, and a suite
    /// that slow gets its assertions loosened rather than kept.
    #[test]
    fn train_and_gunfire_fire_at_the_planned_spacing_in_wall_time() {
        let mut c = cfg();
        c.schedule = ShockLevelSchedule::manual_default();
        c.rate_model = RateModel {
            cooling_floor_s: 0.5,
            settling_floor_s: 0.2,
            compute_floor_s: 0.1,
            device_recompute_floor_s: 20.0,
            read_cadence_ok: true,
        };
        c.gunfire = Some(config::GunfireSpec {
            rounds_per_min: 30.0,
            rounds: 3,
        });
        let mut s = svc(c, ScriptedFirer::host(1.0));
        // Each modelled fire costs 10 ms of wall clock, well inside both
        // spacings, so the loop MUST wait for the difference.
        s.set_pacer(Box::new(pacer::VirtualPacer::new(0.010)));
        s.assess_feasibility().unwrap();
        s.arm().unwrap();

        // MaxRate train of 4: realized spacing = the 0.5 s floor.
        let plan = s.fire_train_with(TrainSpec::max_rate(4)).unwrap();
        assert_eq!(plan.count, 4);
        assert!((plan.realized_spacing_s - 0.5).abs() < 1e-12);
        assert!(
            !plan.hardware_blocked,
            "hardware-blocked is a DELEGATED-path caveat; a host train never carried it honestly"
        );

        // Gunfire burst: spacing = 60 / 30 = 2.0 s.
        let gplan = s.fire_gunfire_burst().unwrap();
        assert_eq!(gplan.count, 3);
        assert!((gplan.realized_spacing_s - 2.0).abs() < 1e-12);

        // A gunfire rate below the floor is REFUSED.
        let err = s.fire_train_with(TrainSpec::delayed(3, 0.05)).unwrap_err();
        assert!(matches!(
            err,
            ServiceError::Train(TrainError::SpacingBelowFloor { .. })
        ));

        let report = s.report_data();
        assert_eq!(report.pulses.len(), 7);
        assert_eq!(report.trains.len(), 2);
        assert_eq!(report.trains[0].pulse_numbers, vec![0, 1, 2, 3]);
        assert_eq!(report.trains[1].gunfire_rounds_per_min, Some(30.0));

        // THE SPACING EVIDENCE: wall time off the injected pacer.
        let w = &report.trains[0].wall_times_s;
        assert_eq!(w.len(), 4, "one wall timestamp per fired pulse");
        for i in 1..w.len() {
            assert!(
                ((w[i] - w[i - 1]) - 0.5).abs() < 1e-9,
                "train pulse {i} landed {:.6} s after {}, planned 0.5 s: {w:?}",
                w[i] - w[i - 1],
                i - 1
            );
        }
        let g = &report.trains[1].wall_times_s;
        assert_eq!(g.len(), 3);
        for i in 1..g.len() {
            assert!(
                ((g[i] - g[i - 1]) - 2.0).abs() < 1e-9,
                "gunfire pulse {i} landed {:.6} s after {}, planned 2.0 s: {g:?}",
                g[i] - g[i - 1],
                i - 1
            );
        }
        // The gunfire burst starts AFTER the train finished - one monotonic
        // timeline, not a per-burst clock that restarts.
        assert!(
            g[0] > w[w.len() - 1],
            "wall clock is monotonic across bursts"
        );

        // NOT the spacing evidence: the simulated reporting clock, pinned only
        // so it keeps tracking the plan it is derived from.
        let t: Vec<f64> = report.pulses.iter().map(|r| r.at_run_time_s).collect();
        assert!(
            ((t[1] - t[0]) - 0.5).abs() < 1e-12,
            "reporting clock, train"
        );
        assert!(
            ((t[6] - t[5]) - 2.0).abs() < 1e-12,
            "reporting clock, gunfire"
        );
        s.stop().unwrap();
    }

    /// A host lowering `should_continue` mid-burst ends the train at the next
    /// INTER-PULSE boundary: the pulse in flight completes, the remainder never
    /// fires, and the event log says how far it got.
    ///
    /// RED-MUTATION: delete the `wait_until` early return (or the `i > 0`
    /// branch's `cancelled = true; break;`) and all five pulses fire - both the
    /// `pulse_numbers.len()` assert and the `TrainCancelled` search red.
    #[test]
    fn a_train_cancels_between_pulses_and_says_how_far_it_got() {
        let mut c = cfg();
        c.schedule = ShockLevelSchedule::manual_default();
        c.rate_model = RateModel {
            cooling_floor_s: 0.5,
            settling_floor_s: 0.2,
            compute_floor_s: 0.1,
            device_recompute_floor_s: 20.0,
            read_cadence_ok: true,
        };
        let mut s = svc(c, ScriptedFirer::host(1.0));
        s.set_pacer(Box::new(pacer::VirtualPacer::new(0.010)));
        s.assess_feasibility().unwrap();
        s.arm().unwrap();

        // The firer double cannot reach the flag, so the flag is lowered off the
        // pacer's own timeline instead: `CancelAfter` runs the virtual clock
        // normally and drops the flag once the burst has consumed 1.2 s of it -
        // AFTER pulse 3 (t = 1.0 s) and BEFORE pulse 4 was due (t = 1.5 s), i.e.
        // squarely inside a gap, which is the case being tested.
        struct CancelAfter {
            inner: pacer::VirtualPacer,
            at_s: f64,
            flag: std::sync::Arc<AtomicBool>,
        }
        impl pacer::TrainPacer for CancelAfter {
            fn now_s(&mut self) -> f64 {
                let t = self.inner.now_s();
                if t >= self.at_s {
                    self.flag.store(false, Ordering::Relaxed);
                }
                t
            }
            fn sleep_until(&mut self, d: f64) {
                self.inner.sleep_until(d)
            }
            fn pulse_start_s(&mut self) -> f64 {
                self.inner.pulse_start_s()
            }
        }
        let keep_going = std::sync::Arc::new(AtomicBool::new(true));
        s.set_pacer(Box::new(CancelAfter {
            inner: pacer::VirtualPacer::new(0.010),
            at_s: 1.2,
            flag: keep_going.clone(),
        }));

        let plan = s
            .fire_train_cancellable(TrainSpec::max_rate(5), Some(&keep_going))
            .unwrap();
        assert_eq!(plan.count, 5, "the PLAN is still the full burst");

        let report = s.report_data();
        let rec = &report.trains[0];
        assert_eq!(
            rec.pulse_numbers.len(),
            3,
            "3 pulses fired before the flag went low at t=1.2 s (pulses at 0.0, \
             0.5, 1.0; the 4th was due at 1.5): {:?}",
            rec.wall_times_s
        );
        assert_eq!(rec.wall_times_s.len(), 3);
        assert!(
            s.log().events().iter().any(|e| matches!(
                &e.kind,
                EventKind::TrainCancelled {
                    fired: 3,
                    of_count: 5
                }
            )),
            "the log must record how far the burst got: {:?}",
            s.log().events().last()
        );
        // Cancelling a train is NOT an abort: the run is still firing and the
        // operator can fire again.
        assert!(
            s.state().is_running(),
            "state after cancel: {:?}",
            s.state()
        );
        s.stop().unwrap();
    }

    /// A flag already low when the burst starts fires NOTHING - the guard is on
    /// the first pulse too, not only on the gaps.
    #[test]
    fn a_train_cancelled_before_it_starts_fires_nothing() {
        let mut c = cfg();
        c.schedule = ShockLevelSchedule::manual_default();
        let mut s = svc(c, ScriptedFirer::host(1.0));
        s.set_pacer(Box::new(pacer::VirtualPacer::new(0.010)));
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        let stop = AtomicBool::new(false);
        let plan = s
            .fire_train_cancellable(TrainSpec::max_rate(3), Some(&stop))
            .unwrap();
        assert_eq!(plan.count, 3);
        let report = s.report_data();
        assert!(report.pulses.is_empty(), "no pulse may have fired");
        assert!(report.trains[0].wall_times_s.is_empty());
        assert!(s.log().events().iter().any(|e| matches!(
            &e.kind,
            EventKind::TrainCancelled {
                fired: 0,
                of_count: 3
            }
        )));
    }

    #[test]
    fn manual_steps_and_jumps_move_the_level() {
        let mut c = cfg();
        c.schedule = ShockLevelSchedule::manual_default();
        let mut s = svc(c, ScriptedFirer::host(1.0));
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        assert_eq!(s.current_level_db(), -12.0);
        assert_eq!(s.step_up().unwrap(), -11.0);
        assert_eq!(s.step_down().unwrap(), -12.0);
        assert_eq!(s.jump_to_level(4).unwrap(), 0.0);
        assert!(matches!(
            s.jump_to_level(99),
            Err(ServiceError::BadState(_))
        ));
        // MANUAL fire at the jumped level.
        assert!(s.fire_next_pulse().unwrap());
        let report = s.report_data();
        assert_eq!(report.pulses[0].scheduled_level_db, 0.0);
        s.stop().unwrap();
    }

    #[test]
    fn feasibility_block_and_override_path() {
        // An overweight article: force_shock / m_total = 5000/500 = 10 g < the
        // 20 g demand -> Exceeded -> hard block -> override arms.
        let mut c = cfg();
        let mut shaker = ShakerRecord::new("little");
        shaker.force_shock = Some(RatingValue::peak(5000.0));
        c.shaker = Some(shaker);
        c.moving_mass.test_article_lb = 500.0;
        let mut s = svc(c, ScriptedFirer::host(1.0));
        let report = s.assess_feasibility().unwrap();
        assert!(report.exceeded());
        assert_eq!(s.state(), RunState::FeasibilityBlocked);
        assert!(matches!(s.arm(), Err(ServiceError::BadState(_))));
        assert!(matches!(
            s.arm_with_override("  "),
            Err(ServiceError::BadState(_))
        ));
        s.arm_with_override("operator accepts the exceedance")
            .unwrap();
        assert_eq!(s.state(), RunState::Armed);
        assert_eq!(
            s.feasibility().unwrap().override_ack.as_deref(),
            Some("operator accepts the exceedance")
        );
        s.stop().unwrap();
    }

    #[test]
    fn report_data_round_trips_through_json() {
        let mut s = svc(cfg(), ScriptedFirer::host(1.0));
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        s.run_auto(100).unwrap();
        let report = s.report_data();
        let json = serde_json::to_string(&report).unwrap();
        let back: ReportData = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pulses.len(), report.pulses.len());
        assert_eq!(back.path, DrivePath::HostDac);
        assert_eq!(back.tolerance.passed, 5);
        assert!(back.honesty.trains.contains("F8"));
        // The status surface serializes too.
        let sj = serde_json::to_string(&s.status()).unwrap();
        let sb: Status = serde_json::from_str(&sj).unwrap();
        assert_eq!(sb.pulses_fired, 5);
    }

    #[test]
    fn pre_fire_ceiling_scales_the_full_level_pulse() {
        let mut c = cfg();
        c.drive_limit.peak_accel_g = Some(15.0);
        let mut s = svc(c, ScriptedFirer::host(1.0));
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        let end = s.run_auto(100).unwrap();
        assert_eq!(end, RunState::Stopped);
        let report = s.report_data();
        let last = report.pulses.last().unwrap();
        assert_eq!(last.scheduled_level_db, 0.0);
        // The predictor uses the SAMPLED composite peak (19.9918 g for the
        // even-N half-sine grid), so the scale is ceiling/sampled-peak ~ 0.7503
        // and the fired peak is exactly the 15 g ceiling.
        assert!((last.drive_limit_scale - 0.75).abs() < 1e-2, "{last:?}");
        assert!((last.peak_g.unwrap() - 15.0).abs() < 1e-6);
        assert_eq!(last.tolerance_overall, Some(true));
        assert!(report
            .drive_limit_actions
            .iter()
            .any(|a| a.phase == "pre-fire"));
    }

    /// The AUTO ladder waits out the rate-model floor IN WALL TIME.
    ///
    /// REPLACES the assertion this test used to carry, which read consecutive
    /// `at_run_time_s` values and checked their deltas against the floor. That
    /// field is the simulated `run_clock_s`, whose only mutation is
    /// `run_clock_s += spacing_s`: the assertion was `(x + s) - x == s`, could
    /// not fail for any implementation, and stayed green through the entire
    /// period when `fire_next_pulse` charged the floor to a `f64` and returned
    /// immediately - firing five rungs back to back on the chassis, fastest at
    /// the highest level. It is the documented cannot-fail shape (a result that
    /// cannot fail is not evidence) and it is exactly the shape the train tests
    /// had already removed while this one kept it.
    ///
    /// What is asserted now is `PulseRow::at_wall_time_s`, read off the injected
    /// pacer. RED-MUTATION: delete the `wait_until` call in
    /// `fire_next_pulse_cancellable` and every wall delta collapses to the
    /// `VirtualPacer`'s `fire_cost_s` (0.01 s), reddening the loop below. The
    /// `at_run_time_s` line is kept, once, purely to pin that the REPORTING
    /// clock still tracks the plan - it is labelled as not being the evidence.
    #[test]
    fn spacing_between_ladder_pulses_is_the_rate_floor_in_wall_time() {
        // Default conservative model: HostDac floor = max(1.0, 0.5, 0.25) = 1 s.
        let mut s = svc(cfg(), ScriptedFirer::host(1.0));
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        s.run_auto(100).unwrap();
        let rows = s.report_data().pulses;
        assert_eq!(rows.len(), 5);
        let w: Vec<f64> = rows.iter().map(|r| r.at_wall_time_s).collect();
        for i in 1..w.len() {
            assert!(
                ((w[i] - w[i - 1]) - 1.0).abs() < 1e-9,
                "ladder rung {i} landed {:.6} s after {}, the 1.0 s rate-model floor                  says otherwise: {w:?}",
                w[i] - w[i - 1],
                i - 1
            );
        }
        // The first rung does NOT wait - the floor is an INTER-pulse constraint
        // and there is no previous pulse to be spaced from (same rule as trains).
        assert!(w[0] < 0.5, "the first rung waited: {w:?}");

        // NOT the evidence: the simulated reporting clock, pinned only so it
        // keeps tracking the plan it is derived from.
        let t: Vec<f64> = rows.iter().map(|r| r.at_run_time_s).collect();
        assert!(((t[1] - t[0]) - 1.0).abs() < 1e-12, "reporting clock");
    }

    // -----------------------------------------------------------------------
    // The Stop / Abort split
    // -----------------------------------------------------------------------

    /// STOP raises the cancel flag, parks the fire path, and lands in `Stopped`.
    /// It does NOT emergency-stop: a clean stop is not a trip and must not be
    /// recorded as one.
    ///
    /// RED-MUTATION: drop the `self.cancel.store(true, ..)` from `stop()` and
    /// the flag assertion reds - which is the whole of what a host holding this
    /// service behind a `Mutex` can reach while a fire is in flight.
    #[test]
    fn stop_is_a_boundary_cancel_and_a_park_not_a_trip() {
        let mut s = svc(cfg(), ScriptedFirer::host(1.0));
        let flag = s.cancel_flag();
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        assert!(
            !flag.load(Ordering::Relaxed),
            "arming a fresh run must LOWER the flag; a run that starts cancelled \
             fires nothing and says nothing about why"
        );
        assert!(s.fire_next_pulse().unwrap());

        s.stop().unwrap();
        assert!(
            flag.load(Ordering::Relaxed),
            "stop must raise the cancel flag"
        );
        assert_eq!(s.state(), RunState::Stopped);
        assert!(s.trip().is_none(), "a clean stop is not a trip");
        assert!(!s.status().abort_active);
    }

    /// ABORT raises the flag, EMERGENCY-stops the fire path (which is what
    /// unreserves the tasks, so abort -> re-arm -> fire is not a zombie), and
    /// latches `Aborted` with an `OperatorAbort` trip carrying the reason.
    ///
    /// RED-MUTATION: point `abort_with_reason` at `firer.stop()` instead of
    /// `emergency_stop()` and the `estopped` assertion reds - the distinction
    /// between the two verbs is `TASK_UNRESERVE`, and losing it reintroduces the
    /// zombie AO measured on 2026-08-03.
    #[test]
    fn abort_emergency_stops_the_fire_path_and_latches_the_operator_trip() {
        let mut s = svc(cfg(), ScriptedFirer::host(1.0));
        let flag = s.cancel_flag();
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        assert!(s.fire_next_pulse().unwrap());

        s.abort_with_reason("operator pressed abort").unwrap();
        assert!(flag.load(Ordering::Relaxed));
        assert_eq!(s.state(), RunState::Aborted);
        assert!(s.status().abort_active);
        let trip = s.trip().expect("an operator abort is recorded");
        match &trip.kind {
            TripKind::OperatorAbort { reason } => {
                assert_eq!(reason, "operator pressed abort")
            }
            k => panic!("wrong trip kind: {k:?}"),
        }
        // The verb that reached the fire path was the EMERGENCY one.
        assert!(
            s.firer.estopped,
            "abort must go through Backend::emergency_stop, not a clean stop - \
             the difference is the task unreserve"
        );
        // Idempotent: a second abort does not overwrite the first trip.
        s.abort_with_reason("again").unwrap();
        match &s.trip().unwrap().kind {
            TripKind::OperatorAbort { reason } => assert_eq!(reason, "operator pressed abort"),
            k => panic!("wrong trip kind: {k:?}"),
        }
    }

    /// A HOST-installed cancel flag reaches the fire path MID-RECORD, and the
    /// service records the committed-queue arithmetic rather than treating the
    /// cancel as a fault.
    ///
    /// The frames the device already accepted keep playing - mid-record
    /// preemption is refused by this chassis - so the numbers on the event are
    /// the honest statement of what was still on the wire.
    #[test]
    fn a_host_cancel_lands_mid_record_and_records_what_was_still_queued() {
        let external = Arc::new(AtomicBool::new(false));
        let mut s = svc(cfg(), ScriptedFirer::host(1.0)).with_cancel_flag(Arc::clone(&external));
        // Raise the host's flag from INSIDE the fire, off the block observer, so
        // the cancel genuinely lands mid-record rather than before the first
        // block. This is also the shape the daemon will have: the tap that
        // watches a fire is the thing best placed to end one.
        let raiser = Arc::clone(&external);
        s.set_block_observer(Box::new(move |done, _total| {
            if done >= 64 {
                raiser.store(true, Ordering::SeqCst);
            }
        }));
        s.assess_feasibility().unwrap();
        s.arm().unwrap();

        assert!(
            !s.fire_next_pulse().unwrap(),
            "a cancelled fire reports the run as no longer running"
        );
        assert_eq!(s.state(), RunState::Stopped, "cancel is not an abort");
        assert!(s.trip().is_none());
        let ev = s
            .log()
            .events()
            .iter()
            .find_map(|e| match &e.kind {
                EventKind::FireCancelled {
                    frames_done,
                    frames_committed,
                    frames_expected,
                    ..
                } => Some((*frames_done, *frames_committed, *frames_expected)),
                _ => None,
            })
            .expect("the cancel must be on the record with its arithmetic");
        let (done, committed, expected) = ev;
        assert!(
            expected > 0,
            "frames_expected must be the real record length"
        );
        assert!(
            done > 0 && done < expected,
            "the cancel must land MID-record, not before the first block or after \
             the last: {done} of {expected}"
        );
        assert!(
            committed >= done,
            "committed ({committed}) cannot be behind delivered ({done}) - the \
             frames still queued are what keeps playing"
        );
    }

    /// Driver diagnostics are drained after EVERY fire and land in the event
    /// log. Nothing in the shock path used to ask for them.
    #[test]
    fn driver_warnings_are_drained_after_every_fire() {
        let mut firer = ScriptedFirer::host(1.0);
        firer.warnings = vec!["AO underrun on cDAQ1Mod2".into()];
        let mut s = svc(cfg(), firer);
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        // Arm drains too - the warning above is reported at arm and the fire
        // below gets a fresh one.
        s.firer.warnings = vec!["late sample".into()];
        assert!(s.fire_next_pulse().unwrap());

        let texts: Vec<(Option<u64>, String)> = s
            .log()
            .events()
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::DriverWarning { pulse, text } => Some((*pulse, text.clone())),
                _ => None,
            })
            .collect();
        assert!(
            texts
                .iter()
                .any(|(p, t)| p.is_none() && t.contains("underrun")),
            "the arm-time drain must be there: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|(p, t)| *p == Some(0) && t.contains("late sample")),
            "the per-fire drain must name the pulse: {texts:?}"
        );
    }

    /// The five production `expect()`s are typed errors now. This is the one
    /// that can be reached without corrupting the service by hand: a ladder that
    /// is gone while the state says running returns `Internal`, not a panic.
    ///
    /// The panic mattered because this crate is hosted by a daemon: an
    /// `expect()` here takes the whole backend down and every other module's run
    /// with it.
    #[test]
    fn a_missing_ladder_is_a_typed_error_not_a_panic() {
        let mut s = svc(cfg(), ScriptedFirer::host(1.0));
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        s.ladder = None; // the invariant the `expect()` asserted
        let err = s.step_up().expect_err("must not panic");
        assert!(matches!(err, ServiceError::Internal(_)), "{err:?}");
        assert!(err.to_string().contains("internal invariant"));
        let err = s.jump_to_level(1).expect_err("must not panic");
        assert!(matches!(err, ServiceError::Internal(_)), "{err:?}");
    }

    /// The SRS analysis grid is re-derived at arm and bounded by the ADOPTED
    /// rate's Nyquist limit. A band above Nyquist is a natural frequency the
    /// record cannot represent, and `srs_analyze` returns a number for it
    /// regardless - a result that means nothing and looks like a result.
    #[test]
    fn the_srs_grid_is_rederived_against_the_adopted_nyquist() {
        let mut c = cfg();
        c.srs_f_hi_hz = 2000.0;
        // The device runs at 2048 Hz: Nyquist is 1024, so every band above it
        // has to go.
        let mut s = svc(c, ScriptedFirer::host_at_rate(1.0, 2048.0));
        let before = s.srs_freqs().len();
        s.assess_feasibility().unwrap();
        s.arm().unwrap();
        let after = s.srs_freqs().to_vec();
        assert!(
            after.len() < before,
            "the grid must shrink: {before} -> {}",
            after.len()
        );
        assert!(
            after.iter().all(|f| *f < 1024.0),
            "no band may sit at or above Nyquist: {after:?}"
        );
        assert!(s.log().events().iter().any(|e| matches!(
            &e.kind,
            EventKind::Note { text } if text.contains("CLAMPED")
        )));
    }
}
