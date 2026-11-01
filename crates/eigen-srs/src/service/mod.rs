//! `SrsShockService` - the SRS-shock run orchestration, mirroring
//! `eigen-random::service::RandomService` / the Explorer service's shape: an explicit
//! state machine, a timestamped event log, a serde [`report::ReportData`], and a live
//! [`report::Status`] snapshot.
//!
//! # The state machine
//!
//! ```text
//! Idle -> Feasibility -> SysId -> Armed -> Firing -> Stopped
//!            |  (hard block)       ^
//!            v                     | (delegated / sysid disabled skips SysId)
//!       FeasibilityBlocked --------+   (arm_with_override)
//! ```
//!
//! plus `Aborted` from any running state (envelope abort, kinematic ceiling,
//! drive breach, plant fault).
//!
//! # The per-pulse pipeline (Path B, [`SrsShockService::fire_pulse`])
//!
//! 1. **Synthesize** the drive from the CURRENT component table
//!    ([`crate::iterate::SynthesisEngine`]) and scale it by the ladder rung
//!    (the ladder owns the level; the law owns the shape).
//! 2. **Pre-fire drive check** (predictive side): predict the peak
//!    acceleration through the measured drive->control FRF and scale down (or
//!    abort at `drive_abort_factor x ceiling`) BEFORE firing.
//! 3. **Fire**: play the drive into the plant ([`plant::PulsePlant`]) and capture
//!    the control + monitor responses (with a ring-out window).
//! 4. **Ingest** into the [`eigen_shock_common::PulseStore`] (SRS +
//!    kinematics + optional PVSS computed on ingest) and stream to the capture
//!    sink when capturing.
//! 5. **Protect**: measured kinematic ceilings, then the
//!    log-octave tolerance envelope (level-normalized alarm/abort per band), then
//!    the between-pulse measured drive monitors (measured side; V/I
//!    inert on Path A via [`eigen_shock_common::PathCapabilities`]).
//! 6. **Iterate**: predict each limit channel's response SRS through its
//!    reduced-level FRF ([`crate::limit::predict_response_srs`]) and hand the
//!    measured control SRS + the predictions to [`crate::law::SrsSynthesisLaw`]
//!    ([`eigen_shock_common::ShockControlLaw::on_pulse`]); apply the returned
//!    per-component gains to the table (the next pulse re-synthesizes from them).
//!    Limit-down beats control-up (the Random arbitration rule).
//! 7. **Advance** the ladder (AUTO: N pulses per level; MANUAL: operator steps).
//!    A completed ladder stops the run cleanly.
//!
//! # The level/trim arbitration (documented precisely)
//!
//! Three scalars can shrink a fired pulse, and they are arbitrated differently:
//!
//! - **The ladder rung** is the commanded LEVEL. Both the envelope
//!   normalization and the law's target aim scale by it.
//! - **The between-pulse monitor trim** and the **pre-fire accel
//!   scale** are SAFETY scales. The LAW's target aim includes them (the law must
//!   not fight a limiter by pumping gains back up - the Random lesson), but the
//!   ENVELOPE normalization deliberately does NOT: a pulse held down by a drive
//!   limit reads LOW against the test's tolerance, which is exactly what the
//!   operator must see (alarm/abort-low is the surfacing mechanism for "this test
//!   cannot reach level under its drive limits").
//!
//! # Path honesty (cited per event in the log)
//!
//! Path B is the host loop above - on the Sim plant today. The DELEGATED path
//! (Path A) is reference-download + absolute-scalar fires: Eigen's control law
//! does NOT run on the device, the download is the UNCOMPENSATED
//! synthesis, there is no host system ID, and V/I
//! ceilings are inert. The capability truth is read from
//! [`eigen_shock_common::PathCapabilities`] - never re-derived here.

pub mod config;
pub mod plant;
pub mod report;

use crate::iterate::{synthesize_to_target, SynthesisEngine};
use crate::law::{LawLimits, SrsSynthesisLaw};
use crate::limit::{predict_response_srs, LimitChannel, PredictiveNotch};
use crate::table::{DampedSineTable, WaveletTable};
use config::{
    ConfigError, EngineKind, FrfSpec, LimitTarget, ShockCaptureTrigger, SrsEnvelope, SrsTestConfig,
    SysIdExcitation,
};
use eigen_capture::{default_data_root, save_snapshot, CaptureConfig, CaptureSink, Mark};
use eigen_channel_table::ChannelTable;
use eigen_dsp::srs::srs_analyze;
use eigen_shaker_db::FeasibilityReport;
use eigen_shock_common::pulse_store::{build_snapshot, capture_pulse};
use eigen_shock_common::{
    predict_peak_accel_g, pulse_feasibility, MeasuredPeak, MonitorEnforcement, PathCapabilities,
    PulseStore, ShockControlLaw, ShockLadderState, ShockLawInputs,
};
use plant::{FirePath, PlantError};
use report::{
    BandResult, DelegatedFireLog, EventKind, EventLog, FeasibilityDigest, FrfSummary, Honesty,
    LevelHistoryEntry, LimitReport, PulseRow, ReportData, RunSettings, RunState, Status, TripKind,
    TripRecord,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Errors from a service operation.
#[derive(Debug, Clone, PartialEq)]
pub enum ServiceError {
    /// The config was invalid.
    Config(ConfigError),
    /// The operation is not allowed in the current state.
    BadState(String),
    /// The plant/backend/device refused an operation.
    Plant(String),
    /// A capture/snapshot IO error.
    Io(String),
    /// The operator's cancel flag stopped the operation at a block boundary
    /// NOT a fault: no protection tripped and no trip record is
    /// written. The message carries the committed-queue arithmetic from
    /// [`plant::PlantError::Cancelled`].
    Cancelled(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::Config(e) => write!(f, "{e}"),
            ServiceError::BadState(s) => write!(f, "bad state: {s}"),
            ServiceError::Plant(s) => write!(f, "plant error: {s}"),
            ServiceError::Io(s) => write!(f, "io error: {s}"),
            ServiceError::Cancelled(s) => write!(f, "cancelled: {s}"),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<ConfigError> for ServiceError {
    fn from(e: ConfigError) -> Self {
        ServiceError::Config(e)
    }
}

/// A measured drive->channel FRF from the reduced-level system ID.
#[derive(Debug, Clone, PartialEq)]
struct MeasuredFrf {
    freqs_hz: Vec<f64>,
    mag: Vec<f64>,
    band_coherence: f64,
}

/// The SRS-shock run service. Owns the fire path, the synthesis component
/// table, the run-time law, the protections, the pulse store, the capture sink, and
/// the event log.
pub struct SrsShockService {
    cfg: SrsTestConfig,
    table: ChannelTable,
    path: FirePath,
    caps: PathCapabilities,
    /// The rate every Hz-denominated quantity below is derived from. Seeded from
    /// the fire path at construction and RE-ADOPTED from the device at arm - the
    /// device owns the rate.
    fs_hz: f64,
    /// What the fire path was ASKED for, kept so the report can name both
    /// authorities instead of only the one that won.
    requested_fs_hz: f64,
    /// `ring_out_s` in SAMPLES at `fs_hz`. Re-derived at arm with everything else;
    /// computing it inline from `cfg.ring_out_s * self.fs_hz` at each use site is
    /// how a stale rate used to leak into the record length.
    ring_out_samples: usize,
    state: RunState,
    log: EventLog,
    /// The operator cancel flag, read once per block inside a fire.
    ///
    /// Service-held and host-INSTALLABLE ([`SrsShockService::set_cancel_handle`]):
    /// a daemon or GUI host that needs to cancel from another thread installs its
    /// own `Arc` (or clones this one via [`SrsShockService::cancel_handle`]) and
    /// flips it. The service CONSUMES the request - it clears the flag on the
    /// terminal path that acts on it - so a cancelled run does not immediately
    /// cancel its own retry.
    cancel: Arc<AtomicBool>,

    // --- grids + target ---------------------------------------------------------
    band_freqs: Vec<f64>,
    /// The FULL (0 dB) target SRS on the band grid, g.
    target_full: Vec<f64>,
    envelope: SrsEnvelope,

    // --- synthesis + law --------------------------------------------------------
    /// The MODEL-space table (converged offline against the target; pre-FRF-seed).
    model_engine: Option<SynthesisEngine>,
    /// Whether the offline model convergence reached `tol_db`.
    model_converged: bool,
    /// The DRIVE table (post FRF feedforward seed + arm-time notch); re-synthesized
    /// every pulse from the law's gain updates. Path B only.
    engine: Option<SynthesisEngine>,
    law: Option<SrsSynthesisLaw>,

    // --- system-ID artifacts (Path B) ---------------------------------------------
    control_frf: Option<MeasuredFrf>,
    /// Measured drive->limit-channel FRFs by channel name.
    measured_limit_frfs: Vec<(String, MeasuredFrf)>,
    /// The resolved limit channels (bounds + FRFs), built at arm.
    limit_channels: Vec<LimitChannel>,
    arm_notch_report: Option<crate::limit::NotchReport>,
    /// The LAST pulse's predicted limit-channel SRS (channel name, per-band g).
    last_predicted_limit_srs: Vec<(String, Vec<f64>)>,

    // --- run state ----------------------------------------------------------------
    ladder: Option<ShockLadderState>,
    store: PulseStore,
    level_history: Vec<LevelHistoryEntry>,
    pulse_rows: Vec<PulseRow>,
    /// The last pulse's measured SRS at the config `q` (band grid).
    last_measured_srs: Option<Vec<f64>>,
    /// Persistent multiplicative next-pulse trim from the between-pulse monitors
    /// (monotone non-increasing - a safety trim never relaxes itself).
    monitor_trim: f64,
    alarm_active: bool,
    trip: Option<TripRecord>,

    // --- feasibility -------------------------------------------------------
    feasibility: Option<FeasibilityDigest>,
    override_ack: Option<String>,

    // --- capture ----------------------------------------------------------------
    capture: Option<CaptureSink>,
    data_root: PathBuf,

    // --- delegated (Path A) -------------------------------------------------------
    delegated_log: DelegatedFireLog,

    // --- one-shot honesty notes ---------------------------------------------------
    noted_no_frf_prediction: bool,
}

impl SrsShockService {
    /// Build a service for `cfg` on `path` against `table`. Validates the config up
    /// front and resolves the band grid + target + envelope. `data_root` is the
    /// LOCAL capture root (resolved via [`default_data_root`] if `None`).
    pub fn new(
        cfg: SrsTestConfig,
        table: ChannelTable,
        path: FirePath,
        data_root: Option<PathBuf>,
    ) -> Result<Self, ServiceError> {
        cfg.validate()?;
        table.validate().map_err(|e| {
            ServiceError::Config(ConfigError::BadConfig(format!("channel table: {e}")))
        })?;
        let fs_hz = path.fs_hz();
        if fs_hz <= 0.0 || !fs_hz.is_finite() {
            return Err(ServiceError::Plant(format!(
                "fire path reports a bad sample rate ({fs_hz})"
            )));
        }
        if cfg.f_hi_hz >= fs_hz / 2.0 {
            return Err(ServiceError::Config(ConfigError::BadConfig(format!(
                "band top {} Hz is at/above Nyquist ({} Hz)",
                cfg.f_hi_hz,
                fs_hz / 2.0
            ))));
        }
        let band_freqs = cfg.band_freqs();
        let target_full = cfg.target_on_grid()?;
        let envelope = cfg.envelope.resolve(band_freqs.len())?;
        let caps = path.drive_path().capabilities();
        let data_root = data_root.unwrap_or_else(default_data_root);
        let ring_out_s = cfg.ring_out_s;

        let mut svc = SrsShockService {
            cfg,
            table,
            caps,
            fs_hz,
            requested_fs_hz: fs_hz,
            ring_out_samples: ring_out_samples(ring_out_s, fs_hz),
            path,
            state: RunState::Idle,
            log: EventLog::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            band_freqs,
            target_full,
            envelope,
            model_engine: None,
            model_converged: false,
            engine: None,
            law: None,
            control_frf: None,
            measured_limit_frfs: Vec::new(),
            limit_channels: Vec::new(),
            arm_notch_report: None,
            last_predicted_limit_srs: Vec::new(),
            ladder: None,
            store: PulseStore::new(),
            level_history: Vec::new(),
            pulse_rows: Vec::new(),
            last_measured_srs: None,
            monitor_trim: 1.0,
            alarm_active: false,
            trip: None,
            feasibility: None,
            override_ack: None,
            capture: None,
            data_root,
            delegated_log: DelegatedFireLog::default(),
            noted_no_frf_prediction: false,
        };
        // Path honesty is logged up front, not discovered mid-run.
        let honesty = Honesty::for_path(svc.path.drive_path());
        svc.log.push(EventKind::Note { text: honesty.path });
        if svc.path.drive_path().is_delegated() {
            svc.log.push(EventKind::Note {
                text: honesty.control_law,
            });
        }
        Ok(svc)
    }

    // --- introspection ------------------------------------------------------------

    pub fn state(&self) -> RunState {
        self.state
    }
    /// The config this service was built from. Exposed so a caller holding ONLY a
    /// configured service can rebuild an equivalent one without re-asking the UI to
    /// reassemble the config - the operator's Reset escape hatch (`SrsState::reset`)
    /// must work even when the UI's assembled config is momentarily un-buildable,
    /// which is exactly the state that stranded the 2026-07-30 real-chassis Random
    /// run.
    pub fn config(&self) -> &SrsTestConfig {
        &self.cfg
    }
    pub fn log(&self) -> &EventLog {
        &self.log
    }
    /// The analysis band grid (fractional-octave centers, Hz).
    pub fn band_freqs(&self) -> &[f64] {
        &self.band_freqs
    }
    /// The FULL (0 dB) target SRS on the band grid, g.
    pub fn target_full(&self) -> &[f64] {
        &self.target_full
    }
    /// The per-pulse store (browse + overlay).
    pub fn store(&self) -> &PulseStore {
        &self.store
    }
    /// The trip record if the run aborted.
    pub fn trip(&self) -> Option<&TripRecord> {
        self.trip.as_ref()
    }
    pub fn feasibility(&self) -> Option<&FeasibilityDigest> {
        self.feasibility.as_ref()
    }
    /// The current commanded ladder level, dB (0.0 before arm).
    pub fn current_level_db(&self) -> f64 {
        self.ladder.as_ref().map_or(0.0, |l| l.current_level_db())
    }
    /// The DRIVE component table (post seed/notch/law updates), once armed.
    pub fn drive_components(&self) -> Option<&SynthesisEngine> {
        self.engine.as_ref()
    }
    /// The last pulse's measured control SRS at the config `q`, on the band grid.
    pub fn last_measured_srs(&self) -> Option<&[f64]> {
        self.last_measured_srs.as_deref()
    }
    /// The last pulse's predicted limit-channel SRS `(name, per-band g)`.
    pub fn last_predicted_limit_srs(&self) -> &[(String, Vec<f64>)] {
        &self.last_predicted_limit_srs
    }
    /// The arm-time predictive-notch plan report, once armed with limits.
    pub fn arm_notch_report(&self) -> Option<&crate::limit::NotchReport> {
        self.arm_notch_report.as_ref()
    }
    /// The rate the run is ACTUALLY denominated in. Equal to
    /// [`Self::requested_fs_hz`] until arm adopts what the device configured.
    pub fn fs_hz(&self) -> f64 {
        self.fs_hz
    }
    /// The rate the fire path was ASKED for. Kept alongside [`Self::fs_hz`] so a
    /// coercion is visible rather than absorbed.
    pub fn requested_fs_hz(&self) -> f64 {
        self.requested_fs_hz
    }
    /// The ring-out window in SAMPLES at the adopted rate.
    pub fn ring_out_samples(&self) -> usize {
        self.ring_out_samples
    }
    /// A clone of the cancel flag this service reads inside a fire.
    /// Flip it from any thread to cancel at the next block boundary.
    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }
    /// INSTALL a host-owned cancel flag, replacing the service's own.
    ///
    /// For a host that already owns a run-wide cancel/stop flag (a daemon pump,
    /// the Tauri worker) and wants one flag rather than two to keep in step. The
    /// flag is cleared on install: an incoming handle that happens to be set would
    /// otherwise cancel the next fire before it started.
    pub fn set_cancel_handle(&mut self, flag: Arc<AtomicBool>) {
        flag.store(false, Ordering::Relaxed);
        self.cancel = flag;
    }
    /// Request cancellation at the next block boundary (the same effect as
    /// flipping [`Self::cancel_handle`]).
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    /// Whether this run HOLDS after a passing system ID for operator review.
    /// Read from the config; `true` by default.
    pub fn review_after_sysid(&self) -> bool {
        self.cfg.review_after_sysid
    }

    fn transition(&mut self, to: RunState) {
        let from = self.state;
        if from != to {
            self.state = to;
            self.log.push(EventKind::StateChange { from, to });
        }
    }

    fn now_unix() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    // --- feasibility ---------------------------------------------

    /// Run the pre-test DETERMINISTIC feasibility assessment:
    /// converge the synthesis offline against the full-level target, then assess the
    /// converged reference's pure-integrator kinematics (crest = 1.0) against the
    /// shaker record via [`eigen_shock_common::pulse_feasibility`] (which consumes
    /// `force_shock` like-for-like by its rating basis).
    ///
    /// Transitions `Idle -> Feasibility`; an Exceeded verdict parks at
    /// `FeasibilityBlocked` (arming HARD-BLOCKED until
    /// [`Self::arm_with_override`]). No shaker configured => a permissive
    /// NotEvaluated report + a logged note.
    pub fn assess_feasibility(&mut self) -> Result<FeasibilityReport, ServiceError> {
        if !matches!(self.state, RunState::Idle) {
            return Err(ServiceError::BadState(format!(
                "assess_feasibility from {:?}",
                self.state
            )));
        }
        self.transition(RunState::Feasibility);

        // Offline model convergence: the reference the test intends the
        // control point to see. Deterministic - a pure function of the config.
        let seed = self.default_engine()?;
        let result = synthesize_to_target(
            &self.target_full,
            &self.band_freqs,
            self.fs_hz,
            seed,
            self.cfg.synthesis,
        );
        if !result.converged {
            self.log.push(EventKind::Note {
                text: format!(
                    "offline synthesis did NOT converge to tol {:.2} dB in {} iterations \
                     (worst band still off); the run-time law refines from here",
                    self.cfg.synthesis.tol_db, result.iters
                ),
            });
        }
        self.model_converged = result.converged;
        let reference = result.waveform.clone();
        self.model_engine = Some(result.components);

        let report = match &self.cfg.shaker {
            None => {
                self.log.push(EventKind::Note {
                    text: "No shaker configured; feasibility NOT evaluated. \
                           Arming allowed without a shaker-limit block."
                        .into(),
                });
                FeasibilityReport {
                    items: Vec::new(),
                    overall: eigen_shaker_db::Verdict::NotEvaluated,
                    assumptions: vec!["No shaker record configured.".into()],
                }
            }
            Some(shaker) => pulse_feasibility(
                shaker,
                &self.cfg.moving_mass,
                &reference,
                self.fs_hz,
                self.cfg.feasibility_tol.into(),
            ),
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

    /// Arm the run. Requires feasibility to have passed (not Exceeded); on an
    /// exceedance use [`Self::arm_with_override`]. Path B with system
    /// ID configured enters `SysId` (drive [`Self::run_sysid`] next); otherwise the
    /// drive is armed directly.
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
    /// Logs the ack verbatim and records it in the digest.
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

    fn arm_inner(&mut self) -> Result<(), ServiceError> {
        if self.path.drive_path().is_host() {
            if let FirePath::Host(p) = &mut self.path {
                p.ensure_open().map_err(ServiceError::Plant)?;
            }
            // The plant has now CONFIGURED the device and adopted its rate.
            // Everything the service denominates in Hz is re-derived here, BEFORE
            // any synthesis, system ID or arm-time notch reads `self.fs_hz`.
            self.adopt_rate_at_arm()?;
            if self.cfg.sysid.is_some() {
                // Reduced-level system ID before the drive is armed.
                self.transition(RunState::SysId);
                return Ok(());
            }
            self.log.push(EventKind::Note {
                text: "System ID DISABLED: the drive seed assumes a UNITY plant and \
                       predictive peak-accel limiting has no FRF to predict through. \
                       The run-time law corrects from the first pulse."
                    .into(),
            });
            self.arm_drive()
        } else {
            if self.cfg.sysid.is_some() {
                // The DEVICE owns system ID at arm on Path A.
                self.log.push(EventKind::Note {
                    text: "System ID SKIPPED on the delegated path: the device owns system \
                           ID at arm; Eigen observes."
                        .into(),
                });
            }
            // The delegated device's rate is fixed and there is no configure seam
            // to coerce it, so this re-derivation is a no-op there. It runs anyway
            // so the two paths cannot drift into different arm orders.
            self.adopt_rate_at_arm()?;
            self.arm_drive()
        }
    }

    /// ADOPT the fire path's ACTUAL rate and re-derive every Hz-denominated
    /// quantity the service owns.
    ///
    /// # Why this exists at all
    ///
    /// These were all derived ONCE, in `new()`, from `path.fs_hz()` - which before
    /// arm is the rate the backend was ASKED for, not the rate it plays at. On a
    /// cDAQ-9234 the request is coerced up a `13.1072 MHz/(256 n)` ladder at
    /// configure, so the two differ on every real run; SRS only ever agreed with
    /// the device by ordering luck. The fix is not to compare-and-refuse (SRS
    /// synthesizes fresh every shot, so adopting is free and refusing would refuse
    /// every run on this chassis) but to RE-DERIVE at the adopted rate.
    ///
    /// What gets re-derived, and what each one is denominated in:
    ///
    /// * the NYQUIST check - the only one of these that CAN refuse, and the honest
    ///   rate authority for this module: a rate coerced DOWN can put the analysis
    ///   band top at or above `fs/2`, and that must stop the arm, not be silently
    ///   analyzed past;
    /// * `band_freqs()` and `target_on_grid()` - the analysis grid and the target
    ///   on it. Their VALUES do not move with the rate (fractional-octave centers
    ///   are pure frequency), but they are re-derived from the config after the
    ///   Nyquist gate so the grid the run reports is the grid the run validated;
    /// * `envelope.resolve()` - built at `new()` today, sized to the grid;
    /// * the ring-out window in SAMPLES, which scales directly with the rate and
    ///   sets the captured record length.
    fn adopt_rate_at_arm(&mut self) -> Result<(), ServiceError> {
        let actual = self.path.fs_hz();
        if !(actual.is_finite() && actual > 0.0) {
            return Err(ServiceError::Plant(format!(
                "fire path reports a bad sample rate after bring-up ({actual})"
            )));
        }
        let moved = (actual - self.fs_hz).abs() > f64::EPSILON * actual.abs().max(1.0);
        self.fs_hz = actual;

        if self.cfg.f_hi_hz >= actual / 2.0 {
            return Err(ServiceError::Config(ConfigError::BadConfig(format!(
                "band top {} Hz is at/above Nyquist ({} Hz) at the rate the device \
                 CONFIGURED ({actual} Hz; {} Hz was requested)",
                self.cfg.f_hi_hz,
                actual / 2.0,
                self.requested_fs_hz
            ))));
        }
        self.band_freqs = self.cfg.band_freqs();
        self.target_full = self.cfg.target_on_grid()?;
        self.envelope = self.cfg.envelope.resolve(self.band_freqs.len())?;
        self.ring_out_samples = ring_out_samples(self.cfg.ring_out_s, actual);

        if moved {
            self.log.push(EventKind::Note {
                text: format!(
                    "DEVICE RATE ADOPTED: {} Hz requested, {actual} Hz configured. Synthesis, \
                     the ring-out window ({} samples), SRS analysis and the tolerance envelope \
                     are all re-derived at the configured rate. The offline model convergence \
                     and the feasibility assessment were computed at the REQUESTED rate; the \
                     run-time law refines from the first pulse.",
                    self.requested_fs_hz, self.ring_out_samples
                ),
            });
        }
        Ok(())
    }

    /// Named refusal for the driving verbs while the review hold is up.
    fn refuse_if_awaiting_review(&self, verb: &str) -> Result<(), ServiceError> {
        if matches!(self.state, RunState::AwaitingReview) {
            return Err(ServiceError::BadState(format!(
                "{verb} refused: the run is HOLDING for operator review after system ID. \
                 Nothing drives until proceed_after_review()."
            )));
        }
        Ok(())
    }

    /// Park the fire path on a clean terminal path. Best effort: a
    /// teardown that fails is LOGGED, never allowed to block reaching a terminal
    /// state.
    fn park_plant(&mut self) {
        if let Err(e) = self.path.stop() {
            self.log.push(EventKind::Note {
                text: format!("plant park (stop) reported: {e}"),
            });
        }
    }

    /// Emergency-park the fire path. Same best-effort contract as
    /// [`Self::park_plant`].
    fn emergency_park_plant(&mut self) {
        if let Err(e) = self.path.emergency_stop() {
            self.log.push(EventKind::Note {
                text: format!("plant park (emergency_stop) reported: {e}"),
            });
        }
    }

    // --- system ID (Path B only) ------------------------

    /// Run the reduced-level system ID: drive a low-level excitation
    /// (SHAPED = the target-converged reference scaled down; STANDARD = a short
    /// broadband pulse), estimate the drive->control FRF (+ coherence) and the
    /// drive->limit-channel FRFs via
    /// [`eigen_dsp::frf::h_estimate_hv_with_coherence`], then arm the drive
    /// (feedforward-seeded through the measured FRF).
    ///
    /// Transitions `SysId -> Armed`. Path B only.
    pub fn run_sysid(&mut self) -> Result<(), ServiceError> {
        if !matches!(self.state, RunState::SysId) {
            return Err(ServiceError::BadState(format!(
                "run_sysid from {:?}",
                self.state
            )));
        }
        let spec = self.cfg.sysid.ok_or_else(|| {
            ServiceError::BadState("run_sysid with no sysid config (arm skipped it)".into())
        })?;
        let model = self
            .model_engine
            .as_ref()
            .ok_or_else(|| ServiceError::BadState("no model engine (assess first)".into()))?;

        // Build the excitation burst at the sysID level.
        let level = 10.0_f64.powf(spec.level_db / 20.0);
        let model_wave = model.synthesize(self.fs_hz);
        if model_wave.is_empty() {
            return Err(ServiceError::Config(ConfigError::BadConfig(
                "model synthesis produced an empty waveform".into(),
            )));
        }
        let peak = model_wave.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
        let burst: Vec<f64> = match spec.excitation {
            SysIdExcitation::Shaped => model_wave.iter().map(|&v| v * level).collect(),
            SysIdExcitation::StandardPulse => {
                // A few-sample broadband rectangle (the classical low-level pulse).
                let width = ((self.fs_hz / (4.0 * self.cfg.f_hi_hz)).round() as usize).max(1);
                let mut p = vec![peak * level; width];
                // Pad to a couple of low-band periods so the response develops.
                let pad = ((2.0 * self.fs_hz / self.cfg.f_lo_hz).round() as usize).max(width);
                p.resize(pad, 0.0);
                p
            }
        };

        // Tile the burst (with ring-out gaps) so Welch has at least ~4 segments.
        let ring = self.ring_out_samples;
        let min_len = 4 * spec.nfft;
        let mut drive: Vec<f64> = Vec::with_capacity(min_len + burst.len() + ring);
        while drive.len() < min_len {
            drive.extend_from_slice(&burst);
            drive.extend(std::iter::repeat_n(0.0, ring));
        }

        // The sysID is ONE long fire, not a train, so the cancellation hook inside
        // `play_block_wise` is the only place an operator Stop can land during it. It is
        // also the longest single play SRS ever commands - `4 x nfft` frames plus ring-out
        // gaps - which is exactly why it is the fire that needs cancelling.
        let cancel = Arc::clone(&self.cancel);
        let fired = match &mut self.path {
            FirePath::Host(p) => p.fire(&drive, ring, Some(&cancel)),
            FirePath::Delegated(_) => {
                return Err(ServiceError::BadState(
                    "run_sysid on the delegated path (the device owns system ID)".into(),
                ))
            }
        };
        let responses = match fired {
            Ok(r) => r,
            Err(cancelled @ PlantError::Cancelled { .. }) => {
                // TYPED EARLY-OUT. Nothing was measured, so the drive must NOT be
                // armed: the run stays in `SysId`, which is the honest state (a
                // system ID is owed) and is re-runnable. The plant is parked, so
                // nothing is left reserved-and-running, and the flag is consumed so
                // the retry is not cancelled before it starts. The one state that
                // must never be reachable from here is Armed-without-an-FRF.
                let e = cancelled.to_string();
                self.park_plant();
                self.cancel.store(false, Ordering::Relaxed);
                self.log.push(EventKind::Note {
                    text: format!(
                        "SYSTEM ID CANCELLED at a block boundary: {e}. No FRF was measured, so \
                         the drive was NOT armed; the run stays in SysId and run_sysid() can be \
                         called again."
                    ),
                });
                return Err(ServiceError::Cancelled(e));
            }
            Err(e) => {
                // A failed sysID fire leaves the task running by the plant's
                // contract; park it before reporting.
                self.park_plant();
                return Err(ServiceError::Plant(e.to_string()));
            }
        };
        // The drive during the ring-out is zero; pad so the estimator sees aligned
        // records.
        let mut drive_padded = drive;
        drive_padded.resize(responses.control_g.len(), 0.0);

        let control_frf = self.estimate_frf(&drive_padded, &responses.control_g, spec.nfft);
        let band_coh = control_frf.band_coherence;

        // Measure the drive->limit-channel FRFs for spec'd channels without a
        // user-supplied FRF.
        self.measured_limit_frfs.clear();
        let mut identified = vec!["control".to_string()];
        if let Some(limits) = &self.cfg.limits {
            for ch in &limits.channels {
                if ch.frf.is_some() {
                    continue;
                }
                let Some((_, rec)) = responses.monitors.iter().find(|(n, _)| *n == ch.name) else {
                    return Err(ServiceError::Config(ConfigError::BadConfig(format!(
                        "limit channel '{}' has no user FRF and no matching plant tap to \
                         measure one from (name a Monitor channel '{}' in the channel table)",
                        ch.name, ch.name
                    ))));
                };
                let frf = self.estimate_frf(&drive_padded, rec, spec.nfft);
                identified.push(ch.name.clone());
                self.measured_limit_frfs.push((ch.name.clone(), frf));
            }
        }
        self.control_frf = Some(control_frf);
        self.log.push(EventKind::SysId {
            channels: identified,
            band_coherence: band_coh,
            level_db: spec.level_db,
        });

        // REVIEW HOLD. The system ID passed; this is the deliberate
        // stop-and-look before anything drives, not a fault. Bypassable per test
        // (`cfg.review_after_sysid`) because the cost of being wrong is asymmetric:
        // a one-off article is worth a human look at the FRF and coherence, a
        // repeat production part is not.
        if self.cfg.review_after_sysid {
            self.log.push(EventKind::Note {
                text: "SYSTEM ID COMPLETE - HOLDING for operator review. The FRF and \
                       coherence are final; nothing drives until proceed_after_review(). \
                       Clear 'review_after_sysid' in the config to let a repeat test run \
                       straight through."
                    .into(),
            });
            self.transition(RunState::AwaitingReview);
            // The hold PARKS the hardware. A hold is operator-length - minutes,
            // not milliseconds - and leaving the tasks reserved and running
            // through one is what produced the `-200279` input overrun 11 ms after
            // Proceed on the Random side (2026-08-02). The cold-task shape makes
            // this free: the next fire re-arms and starts anyway.
            self.park_plant();
            return Ok(());
        }
        self.arm_drive()
    }

    /// Operator reviewed the system ID and chose to PROCEED.
    ///
    /// Valid only from [`RunState::AwaitingReview`]; from anywhere else this is a
    /// refusal rather than a no-op, because "proceed" arriving in a state that
    /// cannot proceed means the two ends disagree about what is happening.
    ///
    /// Re-arms the fire path (the hold parked it) and continues into the same
    /// `arm_drive` the no-hold path takes, so the two cannot drift.
    ///
    /// # Liveness during the hold, for a daemon host
    ///
    /// While the hold is up the module still OWNS the core: `Configured = true`
    /// AND `Live = true`. Nothing is driving, but the chassis stays claimed and a
    /// second module must NOT be able to bind it - a hold is a paused run, not an
    /// idle one, and the operator's next action is Proceed. A host's `holds()`
    /// must answer accordingly, its `is_live()` must include this state (so a
    /// reconfigure is refused or takes the full drop-and-rebuild path rather than
    /// racing a held run), and a controller vanishing during the hold must leave
    /// the hold STANDING with the hardware parked - which it does, because the
    /// hold parks on entry and [`SrsShockService`]'s `Drop` parks again.
    ///
    /// The watchdog must NOT beat during the hold: there is no planned pulse
    /// spacing to measure against, and an operator-length pause is not a stall.
    pub fn proceed_after_review(&mut self) -> Result<(), ServiceError> {
        if !matches!(self.state, RunState::AwaitingReview) {
            return Err(ServiceError::BadState(format!(
                "proceed_after_review from {:?} (only valid while AwaitingReview)",
                self.state
            )));
        }
        if let FirePath::Host(p) = &mut self.path {
            p.ensure_open().map_err(ServiceError::Plant)?;
        }
        // The FRF, the drive table and the analysis grid were all built at the rate
        // adopted at arm. If a re-bring-up came back on a different rate they are
        // all stale, and re-deriving them would silently discard a measured system
        // ID. Refuse by name instead.
        let now = self.path.fs_hz();
        if (now - self.fs_hz).abs() > f64::EPSILON * self.fs_hz.abs().max(1.0) {
            return Err(ServiceError::Plant(format!(
                "proceed refused: the fire path came back at {now} Hz but the system ID and \
                 the drive table were built at {} Hz; re-run the system ID",
                self.fs_hz
            )));
        }
        self.log.push(EventKind::Note {
            text: "operator reviewed the system ID and proceeded".into(),
        });
        self.arm_drive()
    }

    /// Welch H + coherence via [`eigen_dsp::frf::h_estimate_hv_with_coherence`],
    /// digested to a magnitude curve + mean in-band coherence.
    fn estimate_frf(&self, drive: &[f64], response: &[f64], nfft: usize) -> MeasuredFrf {
        let (h, coh) =
            eigen_dsp::frf::h_estimate_hv_with_coherence(drive, response, self.fs_hz, nfft);
        let bin_hz = self.fs_hz / nfft as f64;
        let freqs: Vec<f64> = (0..h.len()).map(|k| k as f64 * bin_hz).collect();
        let mag: Vec<f64> = h
            .iter()
            .map(|&(re, im)| (re * re + im * im).sqrt())
            .collect();
        let (mut acc, mut n) = (0.0f64, 0usize);
        for (k, &f) in freqs.iter().enumerate() {
            if f >= self.cfg.f_lo_hz && f <= self.cfg.f_hi_hz {
                acc += coh.get(k).copied().unwrap_or(0.0);
                n += 1;
            }
        }
        MeasuredFrf {
            freqs_hz: freqs,
            mag,
            band_coherence: if n > 0 { acc / n as f64 } else { 0.0 },
        }
    }

    // --- arming the drive -----------------------------------------------------------

    /// The default (pre-iteration) component table for the configured engine: one
    /// component per band.
    fn default_engine(&self) -> Result<SynthesisEngine, ServiceError> {
        let frac = self.cfg.fraction.into();
        Ok(match self.cfg.engine {
            EngineKind::Wavsyn => SynthesisEngine::Wavsyn(
                WaveletTable::default_over_band(self.cfg.f_lo_hz, self.cfg.f_hi_hz, frac)
                    .map_err(|e| ServiceError::Config(ConfigError::BadConfig(e.to_string())))?,
            ),
            EngineKind::Seds => SynthesisEngine::Seds(
                DampedSineTable::default_over_band(self.cfg.f_lo_hz, self.cfg.f_hi_hz, frac)
                    .map_err(|e| ServiceError::Config(ConfigError::BadConfig(e.to_string())))?,
            ),
        })
    }

    /// Synthesize-and-arm: seed the DRIVE table from the model (FRF feedforward on
    /// Path B), apply the arm-time predictive notch when limit channels are
    /// present, build + seed the law, place the ladder at its start rung, and (Path
    /// A) download the reference. Enters `Armed`.
    fn arm_drive(&mut self) -> Result<(), ServiceError> {
        let model = self
            .model_engine
            .clone()
            .ok_or_else(|| ServiceError::BadState("no model engine (assess first)".into()))?;

        let ladder = ShockLadderState::new(self.cfg.schedule.clone())
            .map_err(|e| ServiceError::Config(ConfigError::BadConfig(e.to_string())))?;

        match self.path.drive_path() {
            eigen_shock_common::DrivePath::HostDac => {
                let mut engine = model;

                // FRF feedforward seed: divide each component's drive amplitude by
                // |H(f_c)| (floored) so the FIRST pulse's response lands near the
                // target (the "drive prediction" use of the FRF).
                if let (Some(frf), Some(spec)) = (&self.control_frf, &self.cfg.sysid) {
                    let mut h_max = 0.0f64;
                    for (k, &f) in frf.freqs_hz.iter().enumerate() {
                        if f >= self.cfg.f_lo_hz && f <= self.cfg.f_hi_hz {
                            h_max = h_max.max(frf.mag[k]);
                        }
                    }
                    let floor = (spec.h_floor_frac * h_max).max(f64::MIN_POSITIVE);
                    let freqs = engine.component_freqs();
                    for (j, &fc) in freqs.iter().enumerate() {
                        let h = interp_clamped(&frf.freqs_hz, &frf.mag, fc).max(floor);
                        engine.scale_amplitude(j, 1.0 / h);
                    }
                }

                // Resolve the limit channels (bounds + FRFs) and plan the
                // arm-time predictive notch at FULL level (the binding case).
                self.limit_channels = self.resolve_limit_channels()?;
                if !self.limit_channels.is_empty() {
                    let comps = component_waveforms(&engine, self.fs_hz);
                    let notch = PredictiveNotch {
                        limit_channels: self.limit_channels.clone(),
                        headroom_db: self.cfg.limits.as_ref().map_or(0.0, |l| l.headroom_db),
                    };
                    let outcome = notch
                        .plan(&comps, self.fs_hz, &self.band_freqs, self.cfg.q)
                        .map_err(|e| ServiceError::Config(ConfigError::BadConfig(e.to_string())))?;
                    for (j, &g) in outcome.component_gains.iter().enumerate() {
                        engine.scale_amplitude(j, g);
                    }
                    self.log.push(EventKind::NotchPlanned {
                        notched_bands: outcome.report.notched_bands.len(),
                        worst_before_db: outcome.report.worst_exceedance_before_db,
                        worst_after_db: outcome.report.worst_exceedance_after_db,
                        converged: outcome.report.converged,
                    });
                    self.arm_notch_report = Some(outcome.report);
                }

                // The run-time law: limit-aware when limit channels
                // exist, plain per-band ratio otherwise.
                let comp_freqs = engine.component_freqs();
                let law = if self.limit_channels.is_empty() {
                    SrsSynthesisLaw::with_clamp(
                        comp_freqs.clone(),
                        self.cfg.synthesis.max_gain_db_per_iter,
                    )
                } else {
                    SrsSynthesisLaw::with_limits(
                        comp_freqs.clone(),
                        self.cfg.synthesis.max_gain_db_per_iter,
                        LawLimits {
                            bounds: self
                                .limit_channels
                                .iter()
                                .map(|c| c.limit_srs.clone())
                                .collect(),
                            headroom_db: self.cfg.limits.as_ref().map_or(0.0, |l| l.headroom_db),
                        },
                    )
                }
                .map_err(|e| ServiceError::Config(ConfigError::BadConfig(e.to_string())))?;

                let mut law = law;
                let zeros = vec![0.0f64; self.band_freqs.len()];
                let scaled_target: Vec<f64> = self
                    .target_full
                    .iter()
                    .map(|&t| t * ladder.current_scale())
                    .collect();
                let _ = law.init(&ShockLawInputs {
                    target_srs: &scaled_target,
                    measured_srs: &zeros,
                    band_freqs: &self.band_freqs,
                    component_freqs: &comp_freqs,
                    predicted_limit_srs: None,
                    current_scale: ladder.current_scale(),
                });
                self.law = Some(law);
                self.engine = Some(engine);
            }
            eigen_shock_common::DrivePath::Delegated => {
                // The delegated download is the UNCOMPENSATED
                // synthesis - the device compensates on-device. For WAVSYN the
                // composite is self-nulling (no compensation exists to strip); for
                // SEDS the RAW composite is downloaded WITHOUT the Smallwood-Nord
                // compensators.
                let reference = match &model {
                    SynthesisEngine::Wavsyn(_) => model.synthesize(self.fs_hz),
                    SynthesisEngine::Seds(t) => {
                        let n = model.synthesize(self.fs_hz).len();
                        crate::seds::seds_composite_raw(t, self.fs_hz, n)
                    }
                };
                let FirePath::Delegated(dev) = &mut self.path else {
                    unreachable!("path checked above");
                };
                dev.download_reference(&reference)
                    .map_err(ServiceError::Plant)?;
                self.delegated_log.reference_samples = reference.len();
                self.log.push(EventKind::DelegatedDownload {
                    samples: reference.len(),
                });
                // No host drive table, law, or notch on Path A;
                // the model table is still reported as "the synthesis".
                self.engine = Some(model);
            }
        }

        self.level_history.push(LevelHistoryEntry {
            level_db: ladder.current_level_db(),
            auto: matches!(
                self.cfg.schedule.mode,
                eigen_shock_common::ShockRunMode::Auto
            ),
            at_pulse: 0,
        });
        self.ladder = Some(ladder);

        // Capture: EveryPulse opens the sink at arm; Manual waits for the operator.
        if matches!(self.cfg.capture.trigger, ShockCaptureTrigger::EveryPulse) {
            let name = format!("srs_run_{}", Self::now_unix());
            self.start_capture(&name)?;
        }
        self.transition(RunState::Armed);
        Ok(())
    }

    /// Resolve the config's limit-channel specs into [`LimitChannel`]s: bounds onto
    /// the band grid; FRFs from the spec (user-supplied) or the system-ID
    /// measurement. A channel with neither is an arm error - there is nothing to
    /// predict through.
    fn resolve_limit_channels(&self) -> Result<Vec<LimitChannel>, ServiceError> {
        let Some(limits) = &self.cfg.limits else {
            return Ok(Vec::new());
        };
        let nbands = self.band_freqs.len();
        let mut out = Vec::with_capacity(limits.channels.len());
        for spec in &limits.channels {
            let bounds = match &spec.limit {
                LimitTarget::Flat { accel_g } => vec![*accel_g; nbands],
                LimitTarget::PerBand { bounds_g } => bounds_g.clone(),
            };
            let (frf_freqs, frf_mag, source) = match (&spec.frf, self.find_measured_frf(&spec.name))
            {
                (Some(FrfSpec { freqs_hz, mag }), _) => {
                    (freqs_hz.clone(), mag.clone(), "user-supplied")
                }
                (None, Some(m)) => (m.freqs_hz.clone(), m.mag.clone(), "measured"),
                (None, None) => {
                    return Err(ServiceError::Config(ConfigError::BadConfig(format!(
                        "limit channel '{}': no FRF (user-supplied or measured); predictive \
                         notching has nothing to predict through",
                        spec.name
                    ))));
                }
            };
            let _ = source;
            let ch = LimitChannel {
                name: spec.name.clone(),
                frf_mag,
                frf_freqs,
                limit_srs: bounds,
            };
            ch.validate(nbands)
                .map_err(|e| ServiceError::Config(ConfigError::BadConfig(e.to_string())))?;
            out.push(ch);
        }
        Ok(out)
    }

    fn find_measured_frf(&self, name: &str) -> Option<&MeasuredFrf> {
        self.measured_limit_frfs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, f)| f)
    }

    // --- the per-pulse pipeline -----------------------------------------------------

    /// Fire one pulse through the per-pulse pipeline (module docs). Returns `true`
    /// while the run keeps going, `false` when it stopped or aborted this pulse.
    pub fn fire_pulse(&mut self) -> Result<bool, ServiceError> {
        self.refuse_if_awaiting_review("fire_pulse")?;
        if !matches!(self.state, RunState::Armed | RunState::Firing) {
            return Err(ServiceError::BadState(format!(
                "fire_pulse from {:?}",
                self.state
            )));
        }
        self.transition(RunState::Firing);
        let ladder = self
            .ladder
            .as_ref()
            .ok_or_else(|| ServiceError::BadState("no ladder (not armed)".into()))?;
        if ladder.completed() {
            self.stop()?;
            return Ok(false);
        }
        let level_db = ladder.current_level_db();
        let level_scale = ladder.current_scale();
        let pulse_seq = self.store.len() as u64;

        // --- synthesize + fire (path split) ------------------------------------
        let is_host = self.path.drive_path().is_host();
        let (control, monitors, fire_scale_for_law) = if is_host {
            let engine = self
                .engine
                .as_ref()
                .ok_or_else(|| ServiceError::BadState("no drive engine (not armed)".into()))?;
            let full = engine.synthesize(self.fs_hz);
            let mut fire_scale = level_scale * self.monitor_trim;

            // Pre-fire peak-accel prediction (the predictive side).
            if self.cfg.drive_limit.peak_accel_g.is_some() {
                match &self.control_frf {
                    Some(frf) => {
                        let drive_now: Vec<f64> = full.iter().map(|&v| v * fire_scale).collect();
                        let pred =
                            predict_peak_accel_g(&drive_now, self.fs_hz, &frf.freqs_hz, &frf.mag);
                        let decision = self.cfg.drive_limit.pre_fire_accel(pred);
                        if !decision.allowed {
                            let ceiling = self.cfg.drive_limit.peak_accel_g.unwrap();
                            if pred >= self.cfg.drive_abort_factor * ceiling
                                || decision.scale_factor <= 0.0
                            {
                                self.abort(
                                    TripKind::DriveBreach {
                                        channel: "peak accel (predicted)".into(),
                                        peak: pred,
                                        ceiling,
                                        predicted: true,
                                    },
                                    level_db,
                                    pulse_seq,
                                );
                                return Ok(false);
                            }
                            fire_scale *= decision.scale_factor;
                            self.log.push(EventKind::DriveScaled {
                                scale: decision.scale_factor,
                                reason: decision.reason,
                            });
                        }
                    }
                    None => {
                        if !self.noted_no_frf_prediction {
                            self.noted_no_frf_prediction = true;
                            self.log.push(EventKind::Note {
                                text: "peak-accel ceiling configured but no drive->control FRF \
                                       exists (system ID skipped): pre-fire prediction is \
                                       UNAVAILABLE (prediction requires the \
                                       measured FRF)."
                                    .into(),
                            });
                        }
                    }
                }
            }

            let drive: Vec<f64> = full.iter().map(|&v| v * fire_scale).collect();
            let ring = self.ring_out_samples;
            let cancel = Arc::clone(&self.cancel);
            let FirePath::Host(p) = &mut self.path else {
                unreachable!("path checked above");
            };
            let fired = match p.fire(&drive, ring, Some(&cancel)) {
                Ok(f) => f,
                Err(cancelled @ PlantError::Cancelled { .. }) => {
                    // A CLEAN stop, not a trip: no protection engine fired, the
                    // operator asked. The pulse is discarded rather than ingested -
                    // a record truncated at a block boundary is not a measurement,
                    // and an SRS computed from one would be a lie with a sequence
                    // number. `stop()` parks the plant and lets the committed queue
                    // (which this hardware will not preempt) drain.
                    let text = format!(
                        "PULSE CANCELLED mid-record at pulse {pulse_seq}, level {level_db} dB: \
                         {cancelled}. The partial record was DISCARDED (not ingested)."
                    );
                    self.log.push(EventKind::Note { text });
                    self.cancel.store(false, Ordering::Relaxed);
                    self.stop()?;
                    return Ok(false);
                }
                Err(e) => {
                    self.abort(
                        TripKind::Plant {
                            message: e.to_string(),
                        },
                        level_db,
                        pulse_seq,
                    );
                    return Ok(false);
                }
            };

            // Predict each limit channel's response SRS for the drive AS FIRED
            // (through the reduced-level FRFs - a zero-phase estimate; see Honesty).
            self.last_predicted_limit_srs = self
                .limit_channels
                .iter()
                .map(|ch| {
                    (
                        ch.name.clone(),
                        predict_response_srs(
                            &drive,
                            self.fs_hz,
                            &ch.frf_freqs,
                            &ch.frf_mag,
                            &self.band_freqs,
                            self.cfg.q,
                        ),
                    )
                })
                .collect();

            (fired.control_g, fired.monitors, fire_scale)
        } else {
            // Path A: absolute-scalar fire; no host drive, law, or predictions.
            let FirePath::Delegated(dev) = &mut self.path else {
                unreachable!("path checked above");
            };
            let control = match dev.fire_scalar(level_scale) {
                Ok(c) => c,
                Err(e) => {
                    self.abort(TripKind::Plant { message: e }, level_db, pulse_seq);
                    return Ok(false);
                }
            };
            self.delegated_log.fired_scalars.push(level_scale);
            self.log.push(EventKind::DelegatedFire {
                scale: level_scale,
                level_db,
            });
            (control, Vec::new(), level_scale)
        };

        // --- ingest (SRS + kinematics + PVSS on ingest) ----------------
        let seq = self.store.ingest(
            control.clone(),
            self.fs_hz,
            self.band_freqs.clone(),
            level_db,
            self.cfg.compute_pvss,
            Self::now_unix(),
        );
        if let (Some(sink), true) = (
            self.capture.as_ref(),
            !matches!(self.cfg.capture.trigger, ShockCaptureTrigger::Off),
        ) {
            if let Some(rec) = self.store.get(seq) {
                capture_pulse(sink, rec, 1);
            }
        }

        // The service-side measured SRS at the CONFIG q (the pulse store's own SRS
        // is at the pinned shock-common Q=10 convention; identical at the default).
        let measured_srs = srs_analyze(&control, self.fs_hz, &self.band_freqs, self.cfg.q);

        // --- kinematic ceilings (pure-integrator kinematics) -------
        let kin = self.store.get(seq).map(|r| r.kinematics);
        if let Some(k) = kin {
            let checks = [
                ("accel", k.peak_accel_g, self.cfg.kinematic_abort.accel_g),
                (
                    "velocity",
                    k.peak_velocity_ips,
                    self.cfg.kinematic_abort.velocity_ips,
                ),
                (
                    "displacement",
                    k.pk_pk_disp_in,
                    self.cfg.kinematic_abort.disp_pp_in,
                ),
            ];
            for (axis, value, limit) in checks {
                if let Some(lim) = limit {
                    if value > lim {
                        self.push_pulse_row(seq, level_db, &measured_srs, 0, 0, 0);
                        self.abort(
                            TripKind::Kinematic {
                                axis: axis.into(),
                                value,
                                limit: lim,
                            },
                            level_db,
                            seq,
                        );
                        return Ok(false);
                    }
                }
            }
        }

        // --- the tolerance envelope (level-normalized) ------------------------
        let check = self
            .envelope
            .check(&self.target_full, &measured_srs, level_scale);
        self.alarm_active = check.alarmed();
        if check.aborted() {
            let worst_band = check.worst_band.unwrap_or(0);
            self.push_pulse_row(
                seq,
                level_db,
                &measured_srs,
                check.alarm_bands.len(),
                check.abort_bands.len(),
                0,
            );
            self.abort(
                TripKind::Envelope {
                    abort_bands: check.abort_bands.len(),
                    worst_band,
                    worst_band_freq_hz: self.band_freqs.get(worst_band).copied().unwrap_or(0.0),
                    worst_err_db: check.worst_err_db.unwrap_or(0.0),
                },
                level_db,
                seq,
            );
            return Ok(false);
        }
        if check.alarmed() {
            self.log.push(EventKind::EnvelopeAlarm {
                bands: check.alarm_bands.len(),
                worst_err_db: check.worst_err_db.unwrap_or(0.0),
            });
        }

        // --- between-pulse drive monitors (measured side) -------------
        if !self.cfg.drive_limit.monitors.is_empty() {
            let peaks: Vec<MeasuredPeak> = monitors
                .iter()
                .map(|(name, rec)| MeasuredPeak {
                    channel: name.clone(),
                    peak_eu: rec.iter().fold(0.0f64, |m, &v| m.max(v.abs())),
                })
                .collect();
            let decision = self.cfg.drive_limit.between_pulse(&self.caps, &peaks);
            for action in &decision.actions {
                if action.enforcement == MonitorEnforcement::Enforced {
                    if let Some(peak) = action.measured_peak_eu {
                        if peak >= self.cfg.drive_abort_factor * action.peak_ceiling {
                            self.push_pulse_row(seq, level_db, &measured_srs, 0, 0, 0);
                            self.abort(
                                TripKind::DriveBreach {
                                    channel: action.channel.clone(),
                                    peak,
                                    ceiling: action.peak_ceiling,
                                    predicted: false,
                                },
                                level_db,
                                seq,
                            );
                            return Ok(false);
                        }
                    }
                }
            }
            if decision.scale_factor < 1.0 {
                // Monotone: a safety trim never relaxes itself (limit-down wins).
                self.monitor_trim *= decision.scale_factor;
                let channel = decision
                    .actions
                    .iter()
                    .filter(|a| a.scale_factor < 1.0)
                    .map(|a| a.channel.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.log.push(EventKind::MonitorScaled {
                    scale: decision.scale_factor,
                    channel,
                });
            }
        }

        // --- the law iteration (Path B) ----------------------------
        let mut limited_components = 0usize;
        if is_host {
            let engine = self.engine.as_mut().expect("engine present while firing");
            let comp_freqs = engine.component_freqs();
            // The law aims at the target scaled by everything actually COMMANDED
            // (level x safety trims): the law must not fight a limiter (the Random lesson).
            let law_target: Vec<f64> = self
                .target_full
                .iter()
                .map(|&t| t * fire_scale_for_law)
                .collect();
            let preds: Vec<Vec<f64>> = self
                .last_predicted_limit_srs
                .iter()
                .map(|(_, p)| p.clone())
                .collect();
            let law = self.law.as_mut().expect("law present while firing");
            let out = law.on_pulse(&ShockLawInputs {
                target_srs: &law_target,
                measured_srs: &measured_srs,
                band_freqs: &self.band_freqs,
                component_freqs: &comp_freqs,
                predicted_limit_srs: if preds.is_empty() { None } else { Some(&preds) },
                current_scale: level_scale,
            });
            limited_components = law.last_limit_status().limited_components;
            for (j, &g) in out.component_gain_updates.iter().enumerate() {
                engine.scale_amplitude(j, g);
            }
        }

        // --- bookkeeping + ladder ---------------------------------------------
        self.last_measured_srs = Some(measured_srs.clone());
        self.push_pulse_row(
            seq,
            level_db,
            &measured_srs,
            check.alarm_bands.len(),
            0,
            limited_components,
        );
        self.log.push(EventKind::PulseFired {
            seq,
            level_db,
            peak_accel_g: kin.map(|k| k.peak_accel_g).unwrap_or(0.0),
            worst_err_db: check.worst_err_db,
        });

        let ladder = self.ladder.as_mut().expect("ladder present while firing");
        let before_db = ladder.current_level_db();
        ladder.on_pulse_fired();
        let after_db = ladder.current_level_db();
        let completed = ladder.completed();
        if (after_db - before_db).abs() > 1e-12 {
            self.level_history.push(LevelHistoryEntry {
                level_db: after_db,
                auto: true,
                at_pulse: self.store.len() as u64,
            });
            self.log.push(EventKind::LevelChange {
                from_db: before_db,
                to_db: after_db,
                auto: true,
            });
        }
        if completed {
            self.stop()?;
            return Ok(false);
        }
        Ok(true)
    }

    fn push_pulse_row(
        &mut self,
        seq: u64,
        level_db: f64,
        measured_srs: &[f64],
        alarm_bands: usize,
        abort_bands: usize,
        limited_components: usize,
    ) {
        let kin = self.store.get(seq).map(|r| r.kinematics);
        let (mut worst, mut worst_band) = (None::<f64>, None::<usize>);
        let scale = 10.0_f64.powf(level_db / 20.0);
        for (b, (&t, &m)) in self.target_full.iter().zip(measured_srs).enumerate() {
            let tt = t * scale;
            if tt > 0.0 && m > 0.0 {
                let db = 20.0 * (m / tt).log10();
                if worst.map(|w: f64| db.abs() > w.abs()).unwrap_or(true) {
                    worst = Some(db);
                    worst_band = Some(b);
                }
            }
        }
        self.pulse_rows.push(PulseRow {
            seq,
            level_db,
            peak_accel_g: kin.map(|k| k.peak_accel_g).unwrap_or(0.0),
            peak_velocity_ips: kin.map(|k| k.peak_velocity_ips).unwrap_or(0.0),
            pk_pk_disp_in: kin.map(|k| k.pk_pk_disp_in).unwrap_or(0.0),
            worst_err_db: worst,
            worst_band,
            alarm_bands,
            abort_bands,
            limited_components,
        });
    }

    // --- level control (AUTO ladder + MANUAL +/- 1 dB) -----------------

    /// Nudge the commanded level up 1 dB (manual trim).
    pub fn step_up(&mut self) -> Result<f64, ServiceError> {
        self.manual_step(true)
    }

    /// Nudge the commanded level down 1 dB.
    pub fn step_down(&mut self) -> Result<f64, ServiceError> {
        self.manual_step(false)
    }

    fn manual_step(&mut self, up: bool) -> Result<f64, ServiceError> {
        self.refuse_if_awaiting_review("level step")?;
        if !matches!(self.state, RunState::Armed | RunState::Firing) {
            return Err(ServiceError::BadState(format!(
                "level step from {:?}",
                self.state
            )));
        }
        let ladder = self
            .ladder
            .as_mut()
            .ok_or_else(|| ServiceError::BadState("no ladder".into()))?;
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
            at_pulse: self.store.len() as u64,
        });
        self.log.push(EventKind::LevelChange {
            from_db: from,
            to_db: to,
            auto: false,
        });
        Ok(to)
    }

    /// Jump to ladder table level `i`, clearing any manual offset.
    pub fn jump_to_level(&mut self, i: usize) -> Result<f64, ServiceError> {
        self.refuse_if_awaiting_review("jump_to_level")?;
        if !matches!(self.state, RunState::Armed | RunState::Firing) {
            return Err(ServiceError::BadState(format!(
                "jump_to_level from {:?}",
                self.state
            )));
        }
        let ladder = self
            .ladder
            .as_mut()
            .ok_or_else(|| ServiceError::BadState("no ladder".into()))?;
        let from = ladder.current_level_db();
        ladder
            .jump_to_level(i)
            .map_err(|e| ServiceError::BadState(e.to_string()))?;
        let to = ladder.current_level_db();
        self.level_history.push(LevelHistoryEntry {
            level_db: to,
            auto: false,
            at_pulse: self.store.len() as u64,
        });
        self.log.push(EventKind::LevelChange {
            from_db: from,
            to_db: to,
            auto: false,
        });
        Ok(to)
    }

    // --- run-to-completion convenience ---------------------------------------------

    /// Run the whole AUTO sequence: feasibility -> arm -> (system ID) -> fire until
    /// the ladder completes, bounded by `max_pulses` (a safety cap). Returns the
    /// final state. Parks at `FeasibilityBlocked` on an exceedance (the caller
    /// decides whether to override).
    ///
    /// **It also parks at [`RunState::AwaitingReview`]** when the config asks for
    /// the review hold (the default). That is a return, not a failure: the
    /// caller looks at the FRF and calls [`Self::proceed_after_review`], then
    /// `run_auto` again (or drives `fire_pulse` itself). This convenience does NOT
    /// walk through the hold on the operator's behalf - a hold an automation can
    /// skip past is not a hold.
    pub fn run_auto(&mut self, max_pulses: u64) -> Result<RunState, ServiceError> {
        self.assess_feasibility()?;
        if matches!(self.state, RunState::FeasibilityBlocked) {
            return Ok(self.state);
        }
        self.arm()?;
        if matches!(self.state, RunState::SysId) {
            self.run_sysid()?;
        }
        let mut fired = 0u64;
        while matches!(self.state, RunState::Armed | RunState::Firing) && fired < max_pulses {
            if !self.fire_pulse()? {
                break;
            }
            fired += 1;
        }
        Ok(self.state)
    }

    // --- abort + stop ---------------------------------------------------------------

    /// Latch an abort: kill the drive, record the trip, stop the capture,
    /// transition to `Aborted`. The single abort path for every protection engine.
    ///
    /// # Ordering, and the abort-latency arithmetic
    ///
    /// The plant's [`plant::PulsePlant::emergency_stop`] runs FIRST, before the
    /// snapshot and capture IO. Those write files; a drive that stays live while
    /// the host is writing to disk is a drive that stays live for as long as the
    /// filesystem takes. This used to touch only the capture and the state enum -
    /// an envelope abort changed a run's colour on screen and left the device task
    /// reserved and running.
    ///
    /// What `emergency_stop` does NOT do is stop the drive instantly, and no code
    /// here can: mid-record preemption of a committed DAQmx output is REFUSED by
    /// the cDAQ-9178 (`-200289`). The bound on abort latency is therefore the
    /// COMMITTED QUEUE - the frames the device already accepted, ~60 ms on this
    /// chassis, which is longer than a whole shock pulse - plus the abort taper and
    /// the task stop. Aborting "mid-pulse" means aborting after the pulse. Abort
    /// evidence must come from the output node (a DMM on AO0), never from the input
    /// side, which `emergency_stop` also stops.
    fn abort(&mut self, kind: TripKind, at_level_db: f64, at_pulse: u64) {
        self.emergency_park_plant();
        let record = TripRecord {
            kind,
            at_level_db,
            at_pulse,
        };
        self.log.push(EventKind::Trip {
            record: record.clone(),
        });
        self.trip = Some(record);
        if self.cfg.capture.snapshot_on_stop {
            let _ = self.save_snapshot(Vec::new());
        }
        let _ = self.stop_capture_inner();
        self.transition(RunState::Aborted);
    }

    /// Clean stop: PARK THE PLANT, snapshot (if configured), finish any capture,
    /// transition to `Stopped` (unless already Aborted).
    ///
    /// The park is first and unconditional. A stop that reaches a
    /// terminal state with the device task still reserved is the zombie-AO
    /// precondition: the next arm restarts a task that consumes nothing. Unlike
    /// [`Self::abort`] this is the taper-and-drain teardown - whatever the device
    /// has committed plays out, which for a shock record is a zero tail.
    pub fn stop(&mut self) -> Result<(), ServiceError> {
        self.park_plant();
        if self.state != RunState::Aborted && self.cfg.capture.snapshot_on_stop {
            let _ = self.save_snapshot(Vec::new());
        }
        let _ = self.stop_capture_inner();
        if self.state != RunState::Aborted {
            self.transition(RunState::Stopped);
        }
        Ok(())
    }

    /// Latch an abort from OUTSIDE the protection engines (an operator Abort, a
    /// host tearing a run down). Same emergency teardown and same trip record
    /// shape; the reason is the caller's.
    pub fn abort_now(&mut self, message: impl Into<String>) {
        let level_db = self.current_level_db();
        let at_pulse = self.store.len() as u64;
        self.abort(
            TripKind::Plant {
                message: message.into(),
            },
            level_db,
            at_pulse,
        );
    }

    // --- capture + snapshots ---------------------------------------------

    /// Start streaming pulse capture to `data_root/<run_name>/`. EveryPulse runs
    /// open one automatically at arm; Manual callers use this directly.
    pub fn start_capture(&mut self, run_name: &str) -> Result<PathBuf, ServiceError> {
        if let Some(sink) = self.capture.as_ref() {
            return Ok(sink.run_dir().to_path_buf());
        }
        let cfg = CaptureConfig::decimated(1);
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

    /// Stop the capture, flush the sidecar, and log the drop count.
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

    /// Save a [`eigen_shock_common::ShockSnapshotPayload`] snapshot bundle (feature
    /// 8/9): every fired pulse + the run context, into the capture run folder (or
    /// `data_root/snapshots`).
    pub fn save_snapshot(&mut self, marks: Vec<Mark>) -> Result<PathBuf, ServiceError> {
        let run_dir = self
            .capture
            .as_ref()
            .map(|s| s.run_dir().to_path_buf())
            .unwrap_or_else(|| self.data_root.join("snapshots"));
        let settings_json =
            serde_json::to_string(&self.report_settings()).unwrap_or_else(|_| "{}".into());
        let payload = self.store.snapshot_payload(
            self.current_level_db(),
            self.cfg.q,
            Some(self.target_full.clone()),
        );
        let snapshot = build_snapshot(Self::now_unix(), settings_json, marks, payload);
        let path =
            save_snapshot(&run_dir, &snapshot).map_err(|e| ServiceError::Io(e.to_string()))?;
        self.log.push(EventKind::Snapshot {
            path: path.display().to_string(),
        });
        Ok(path)
    }

    // --- report + status -------------------------------------------------------------

    fn report_settings(&self) -> RunSettings {
        RunSettings {
            path: self.path.drive_path(),
            f_lo_hz: self.cfg.f_lo_hz,
            f_hi_hz: self.cfg.f_hi_hz,
            band_freqs: self.band_freqs.clone(),
            q: self.cfg.q,
            engine: match self.cfg.engine {
                EngineKind::Wavsyn => "wavsyn".into(),
                EngineKind::Seds => "seds".into(),
            },
            max_iters: self.cfg.synthesis.max_iters,
            tol_db: self.cfg.synthesis.tol_db,
            max_gain_db_per_iter: self.cfg.synthesis.max_gain_db_per_iter,
            schedule: self.cfg.schedule.clone(),
            envelope_bands: self.envelope.bands.clone(),
            drive_limit: self.cfg.drive_limit.clone(),
            drive_abort_factor: self.cfg.drive_abort_factor,
            compute_pvss: self.cfg.compute_pvss,
            sysid_level_db: if self.path.drive_path().is_host() {
                self.cfg.sysid.map(|s| s.level_db)
            } else {
                None
            },
            fs_hz: self.fs_hz,
            requested_fs_hz: self.requested_fs_hz,
            ring_out_samples: self.ring_out_samples,
            review_after_sysid: self.cfg.review_after_sysid,
        }
    }

    /// Assemble the serde-serializable [`ReportData`]: config digest, feasibility,
    /// FRF summary, synthesis components, per-band achieved-vs-target + PVSS, the
    /// per-pulse table, the notch/limit report, level history, event log, metadata,
    /// and the path-matched honesty block.
    pub fn report_data(&self) -> ReportData {
        let mut frf_summary = Vec::new();
        if let Some(f) = &self.control_frf {
            frf_summary.push(FrfSummary {
                channel: "control".into(),
                source: "measured (reduced-level system ID)".into(),
                points: f.freqs_hz.len(),
                band_coherence: Some(f.band_coherence),
                freqs_hz: f.freqs_hz.clone(),
                mag: f.mag.clone(),
            });
        }
        for (name, f) in &self.measured_limit_frfs {
            frf_summary.push(FrfSummary {
                channel: name.clone(),
                source: "measured (reduced-level system ID)".into(),
                points: f.freqs_hz.len(),
                band_coherence: Some(f.band_coherence),
                freqs_hz: f.freqs_hz.clone(),
                mag: f.mag.clone(),
            });
        }
        if let Some(limits) = &self.cfg.limits {
            for spec in &limits.channels {
                if let Some(frf) = &spec.frf {
                    frf_summary.push(FrfSummary {
                        channel: spec.name.clone(),
                        source: "user-supplied".into(),
                        points: frf.freqs_hz.len(),
                        band_coherence: None,
                        freqs_hz: frf.freqs_hz.clone(),
                        mag: frf.mag.clone(),
                    });
                }
            }
        }

        let last = self.store.last();
        let band_results: Vec<BandResult> = self
            .band_freqs
            .iter()
            .enumerate()
            .map(|(b, &f)| {
                let target_g = self.target_full[b];
                let measured_g = self
                    .last_measured_srs
                    .as_ref()
                    .and_then(|m| m.get(b).copied())
                    .unwrap_or(0.0);
                let scale = last
                    .map(|r| 10.0_f64.powf(r.level_db / 20.0))
                    .unwrap_or(1.0);
                let err_db = if target_g > 0.0 && measured_g > 0.0 {
                    Some(20.0 * (measured_g / (target_g * scale)).log10())
                } else {
                    None
                };
                BandResult {
                    freq_hz: f,
                    target_g,
                    measured_g,
                    err_db,
                }
            })
            .collect();

        let limit_report = if self.limit_channels.is_empty() && self.cfg.limits.is_none() {
            None
        } else {
            Some(LimitReport {
                arm_plan: self.arm_notch_report.clone(),
                bounds_g: self
                    .limit_channels
                    .iter()
                    .map(|c| (c.name.clone(), c.limit_srs.clone()))
                    .collect(),
                last_predicted_srs: self.last_predicted_limit_srs.clone(),
                last_status: self
                    .law
                    .as_ref()
                    .map(|l| l.last_limit_status())
                    .unwrap_or_default(),
                headroom_db: self.cfg.limits.as_ref().map_or(0.0, |l| l.headroom_db),
            })
        };

        ReportData {
            settings: self.report_settings(),
            feasibility: self.feasibility.clone(),
            frf_summary,
            components: self.engine.clone(),
            band_results,
            last_pvss: last.and_then(|r| r.pvss.clone()),
            pulses: self.pulse_rows.clone(),
            pulse_records: self.store.records().to_vec(),
            limit_report,
            level_history: self.level_history.clone(),
            event_log: self.log.clone(),
            trip: self.trip.clone(),
            delegated: self.delegated_log.clone(),
            metadata: self.cfg.metadata.clone(),
            honesty: Honesty::for_path(self.path.drive_path()),
        }
    }

    /// The live status snapshot the UI polls.
    pub fn status(&self) -> Status {
        Status {
            state: self.state,
            current_level_db: self.current_level_db(),
            level_index: self.ladder.as_ref().map_or(0, |l| l.level_index()),
            pulses_fired: self.store.len() as u64,
            pulses_at_level: self.ladder.as_ref().map_or(0, |l| l.pulses_at_level()),
            last_worst_err_db: self.pulse_rows.last().and_then(|r| r.worst_err_db),
            last_limit_status: self
                .law
                .as_ref()
                .map(|l| l.last_limit_status())
                .unwrap_or_default(),
            alarm_active: self.alarm_active,
            abort_active: matches!(self.state, RunState::Aborted),
            ladder_completed: self.ladder.as_ref().is_some_and(|l| l.completed()),
            capturing: self.capture.is_some(),
            band_coherence: self.control_frf.as_ref().map_or(0.0, |f| f.band_coherence),
            capture_trigger: self.cfg.capture.trigger,
        }
    }
}

impl Drop for SrsShockService {
    /// DROP-SAFETY. A host that vanishes - a controller link dropping, a
    /// worker thread unwinding, a `configure` replacing the live service - must not
    /// leave a device task reserved and running with drive on the wire. Parking on
    /// drop is the only teardown that survives a path the code did not plan for,
    /// and it is best-effort by construction: there is nobody left to report to.
    ///
    /// This is belt AND braces with the terminal paths, deliberately: `stop()`,
    /// `abort()` and the review hold all park explicitly, and parking twice is
    /// harmless (the backend's stop is idempotent from `Stopped`).
    fn drop(&mut self) {
        let _ = self.path.stop();
    }
}

/// `ring_out_s` seconds expressed in SAMPLES at `fs_hz`. One place, so the record
/// length cannot be computed from one rate at the sysID and another at a fire.
fn ring_out_samples(ring_out_s: f64, fs_hz: f64) -> usize {
    let n = (ring_out_s * fs_hz).round();
    if n.is_finite() && n > 0.0 {
        n as usize
    } else {
        0
    }
}

/// Per-component waveforms on a COMMON record length (the predictive-notch input:
/// their sample-for-sample sum IS the composite drive). WAVSYN components are the
/// wavelets; SEDS components are the RAW damped sinusoids (the Smallwood-Nord
/// compensators are NOT components - `crate::limit`'s plan docs).
fn component_waveforms(engine: &SynthesisEngine, fs_hz: f64) -> Vec<Vec<f64>> {
    let n = engine.synthesize(fs_hz).len();
    match engine {
        SynthesisEngine::Wavsyn(t) => t
            .components
            .iter()
            .map(|c| crate::wavsyn::wavsyn_component(c, fs_hz, n))
            .collect(),
        SynthesisEngine::Seds(t) => t
            .components
            .iter()
            .map(|c| {
                let mut out = vec![0.0f64; n];
                crate::seds::seds_component_into(c, fs_hz, &mut out);
                out
            })
            .collect(),
    }
}

/// Linear interpolation of `(xs, ys)` at `x`, clamped to the endpoint values outside
/// the sampled range. `xs` ascending.
fn interp_clamped(xs: &[f64], ys: &[f64], x: f64) -> f64 {
    let n = xs.len();
    if n == 0 {
        return 0.0;
    }
    if x <= xs[0] {
        return ys[0];
    }
    if x >= xs[n - 1] {
        return ys[n - 1];
    }
    let mut lo = 0usize;
    let mut hi = n - 1;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if xs[mid] <= x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (x0, x1) = (xs[lo], xs[hi]);
    let (y0, y1) = (ys[lo], ys[hi]);
    if x1 <= x0 {
        return y0;
    }
    y0 + (y1 - y0) * (x - x0) / (x1 - x0)
}

#[cfg(test)]
mod tests;
