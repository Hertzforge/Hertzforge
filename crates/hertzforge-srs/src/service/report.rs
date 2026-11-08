//! Event log, report data, honesty section, and the UI status surface for the
//! SRS-shock run service, mirroring `hertzforge-random::service::report` /
//! the Explorer service's conventions: a timestamped event log of every state
//! change / trip / capture / snapshot, a serde-serializable [`ReportData`] the UI's
//! report consumes, and an honesty section carried verbatim into every report.

use crate::iterate::SynthesisEngine;
use crate::law::LawLimitStatus;
use crate::limit::NotchReport;
use crate::service::config::{MetaField, ShockCaptureTrigger, SrsToleranceBand, SysIdExcitation};
use hertzforge_shaker_db::FeasibilityReport;
use hertzforge_shock_common::law_guard::{LawCall, LawDescriptor, LawFault, LawValue};
use hertzforge_shock_common::{DriveLimit, DrivePath, PulseRecord, ShockLevelSchedule};
use serde::{Deserialize, Serialize};

/// The SRS-shock run state machine (mirrors Random's state machine with shock semantics):
/// `Idle -> Feasibility -> SysId -> Armed -> Firing -> Stopped`, plus `Aborted` from
/// any running state, plus the `FeasibilityBlocked` hard block (an
/// Exceeded feasibility HARD-BLOCKS arming until an explicit override-ack).
///
/// A closed-loop run (`SrsTestConfig::control` set) adds two more ends: `Held`
/// (identification held the run before any pulse) and `NotConverged` (the
/// iteration limit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState {
    /// No run started.
    Idle,
    /// Pre-test deterministic feasibility assessed (crest = 1.0).
    Feasibility,
    /// Feasibility Exceeded: arming is HARD-BLOCKED until `arm_with_override`.
    FeasibilityBlocked,
    /// Reduced-level system ID in progress (Path B only).
    SysId,
    /// System ID passed and the run is HOLDING for operator review. Nothing drives
    /// here: the FRF and coherence are on screen and the operator decides whether to
    /// proceed.
    ///
    /// A deliberate stop-and-look, not a fault - a failing gate parks at
    /// `FeasibilityBlocked` or aborts instead. Bypassable per test
    /// (`SrsTestConfig::review_after_sysid`) because the cost of being wrong is
    /// asymmetric: a one-off article earns a human look, a repeat production part
    /// does not.
    ///
    /// The fire path is PARKED on entry (tasks stopped and unreserved), so a hold
    /// can last as long as the operator wants without the input ring overrunning
    /// or an output task sitting reserved. The module still OWNS the core through
    /// the hold: a host's `holds()` answers `Configured = true` AND `Live = true`,
    /// and a second module must not be able to bind the chassis.
    AwaitingReview,
    /// Armed: the drive is synthesized to the target at the start level, the
    /// predictive notch (if limit channels are present) is applied, the law is
    /// seeded, and the ladder is at its start rung. The next `fire_pulse` fires.
    Armed,
    /// Pulses are being fired through the per-pulse pipeline.
    Firing,
    /// Cleanly stopped (ladder completed or operator stop).
    Stopped,
    /// A protection engine tripped; stopped immediately with a trip record.
    Aborted,
    /// Closed loop: identification held the run before any pulse (its mean
    /// in-band coherence was under the floor). Terminal; [`Status::held`]
    /// says why.
    Held,
    /// Closed loop: the iteration limit ended the run (pulses at one level
    /// without a pass, or the run's pulse cap). Terminal; the trip record is
    /// [`TripKind::NotConverged`].
    NotConverged,
}

impl RunState {
    /// True once the run has ended: stopped, aborted, held or not converged.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunState::Stopped | RunState::Aborted | RunState::Held | RunState::NotConverged
        )
    }

    /// True while a pulse could still fire (a protection abort can trip).
    ///
    /// [`RunState::AwaitingReview`] is deliberately NOT running: the hold parks
    /// the fire path and refuses every driving verb, so nothing can trip. It is
    /// still a LIVE run for ownership purposes - see the variant's docs - which is
    /// a different question from this one.
    pub fn is_running(self) -> bool {
        matches!(self, RunState::SysId | RunState::Armed | RunState::Firing)
    }
}

/// Which iteration limit ended a run that did not converge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConvergenceLimit {
    /// `max_pulses_per_level` pulses at one level without a pass (AUTO), or
    /// failing pulses in a row at one level (MANUAL).
    PerLevel,
    /// The run's `max_pulses`.
    Run,
}

impl ConvergenceLimit {
    /// The stable wire name: `per_level` or `run`.
    pub fn as_str(self) -> &'static str {
        match self {
            ConvergenceLimit::PerLevel => "per_level",
            ConvergenceLimit::Run => "run",
        }
    }
}

/// Why identification held a closed-loop run before any pulse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeldReason {
    /// The identification's mean in-band coherence was under the floor.
    LowCoherence {
        /// The mean in-band coherence measured.
        mean: f64,
        /// The floor it had to reach.
        floor: f64,
    },
}

impl HeldReason {
    /// The stable name: `low_coherence`.
    pub fn as_str(&self) -> &'static str {
        match self {
            HeldReason::LowCoherence { .. } => "low_coherence",
        }
    }
}

impl std::fmt::Display for HeldReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeldReason::LowCoherence { mean, floor } => write!(
                f,
                "identification coherence {mean:.3} is under the floor of {floor:.3}; raise \
                 the identification level or check the control channel"
            ),
        }
    }
}

/// The kind of protection trip that aborted the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TripKind {
    /// The tolerance-envelope abort: a band's measured SRS breached the abort
    /// band (level-normalized).
    Envelope {
        /// Number of bands that breached the abort band this pulse.
        abort_bands: usize,
        /// The worst breaching band's index and natural frequency.
        worst_band: usize,
        worst_band_freq_hz: f64,
        /// The worst signed error, dB.
        worst_err_db: f64,
    },
    /// A measured per-pulse kinematic ceiling breach (the pure-
    /// integrator kinematics).
    Kinematic {
        /// Which axis breached: `"accel"`, `"velocity"`, or `"displacement"`.
        axis: String,
        /// The measured value (g, in/s, or in).
        value: f64,
        /// The configured ceiling.
        limit: f64,
        /// The breach was found on the closed loop's prediction of the next
        /// pulse, before it fired: nothing was driven for it. `false` for a
        /// measured breach.
        #[serde(default)]
        predicted: bool,
    },
    /// A drive-limit breach: a predicted or measured peak
    /// reached `drive_abort_factor * ceiling`.
    DriveBreach {
        /// The monitor channel (or `"peak accel (predicted)"` for the pre-fire
        /// path).
        channel: String,
        /// Predicted or measured peak, channel EU.
        peak: f64,
        /// The configured ceiling.
        ceiling: f64,
        /// True when the breach was a PRE-FIRE prediction (the pulse never fired);
        /// false for a measured between-pulse breach (that pulse
        /// already fired).
        predicted: bool,
    },
    /// The plant/backend refused an operation mid-run.
    Plant { message: String },
    /// Closed loop: a commanded drive over the drive abort, volts peak. Nothing
    /// was played for it.
    DriveVpk {
        /// The drive's peak, volts.
        vpk: f64,
        /// The drive abort, volts peak.
        abort_vpk: f64,
        /// Found on the predicted drive at proceed (at the highest level the
        /// run is scheduled to reach), before any pulse; `false` for a drive
        /// refused just before it would have played.
        predicted: bool,
    },
    /// Closed loop: the iteration limit ended the run. The run ends
    /// [`RunState::NotConverged`].
    NotConverged {
        /// Which limit.
        limit: ConvergenceLimit,
        /// Pulses counted against it: at the level for
        /// [`ConvergenceLimit::PerLevel`], in the run for
        /// [`ConvergenceLimit::Run`].
        pulses: u64,
    },
    /// Closed loop: the control law gave no usable gains `consecutive` times
    /// in a row, or its seed was refused or could not be built. Nothing more
    /// fired and the law was not called again.
    LawFault {
        /// Why the last call was not used (`fault.reason()` is the wire name).
        fault: LawFault,
        /// Unusable calls in a row when the run stopped.
        consecutive: u32,
    },
}

impl TripKind {
    /// The stable name of the trip's kind: `envelope`, `kinematic`, `accel`
    /// (a predicted response peak at the abort, [`TripKind::DriveBreach`] with
    /// `predicted`), `monitor` (a measured monitor peak, `DriveBreach` without
    /// it), `plant`, `drive_vpk`, `not_converged` or `law_fault`.
    pub fn name(&self) -> &'static str {
        match self {
            TripKind::Envelope { .. } => "envelope",
            TripKind::Kinematic { .. } => "kinematic",
            TripKind::DriveBreach {
                predicted: true, ..
            } => "accel",
            TripKind::DriveBreach { .. } => "monitor",
            TripKind::Plant { .. } => "plant",
            TripKind::DriveVpk { .. } => "drive_vpk",
            TripKind::NotConverged { .. } => "not_converged",
            TripKind::LawFault { .. } => "law_fault",
        }
    }

    /// True for a trip found on a prediction before a pulse fired, so nothing
    /// was driven for it.
    pub fn predicted(&self) -> bool {
        match self {
            TripKind::Kinematic { predicted, .. }
            | TripKind::DriveVpk { predicted, .. }
            | TripKind::DriveBreach { predicted, .. } => *predicted,
            _ => false,
        }
    }
}

/// A record of the trip that aborted the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TripRecord {
    pub kind: TripKind,
    /// The ladder level (dB) when it tripped.
    pub at_level_db: f64,
    /// The pulse sequence number at which the trip fired (the pulse being processed;
    /// for a pre-fire trip, the pulse that would have fired).
    pub at_pulse: u64,
}

/// A single event in the run log with a monotonic index + coarse wall-clock
/// timestamp.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub unix_secs: u64,
    pub kind: EventKind,
}

/// The kinds of event the service logs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EventKind {
    /// A run-state transition.
    StateChange { from: RunState, to: RunState },
    /// Feasibility assessed with its overall verdict.
    Feasibility {
        overall: String,
        exceeded: bool,
        has_advisory: bool,
    },
    /// An override acknowledgment was logged to arm past a feasibility exceedance.
    OverrideAck { ack: String },
    /// System ID completed: the channels identified and the mean
    /// in-band coherence of the drive->control estimate.
    SysId {
        channels: Vec<String>,
        band_coherence: f64,
        level_db: f64,
    },
    /// The predictive notch was planned at arm: what it notched and how well
    /// it converged.
    NotchPlanned {
        notched_bands: usize,
        worst_before_db: Option<f64>,
        worst_after_db: Option<f64>,
        converged: bool,
    },
    /// The ladder moved (AUTO advance, manual step, or jump).
    LevelChange {
        from_db: f64,
        to_db: f64,
        auto: bool,
    },
    /// One pulse fired and was analyzed.
    PulseFired {
        seq: u64,
        level_db: f64,
        peak_accel_g: f64,
        worst_err_db: Option<f64>,
    },
    /// The envelope ALARM breached (surfaced, not stopped).
    EnvelopeAlarm { bands: usize, worst_err_db: f64 },
    /// The pre-fire drive limiter scaled the pulse down before firing (the
    /// predictive side).
    DriveScaled { scale: f64, reason: String },
    /// A between-pulse monitor commanded a next-pulse scale-down (the
    /// measured side).
    MonitorScaled { scale: f64, channel: String },
    /// A protection trip aborted the run.
    Trip { record: TripRecord },
    /// Capture started.
    CaptureStart { run_dir: String, trigger: String },
    /// Capture stopped, with the drop count.
    CaptureStop { chunks_written: u64, dropped: u64 },
    /// A snapshot was saved.
    Snapshot { path: String },
    /// The delegated reference was downloaded (Path A: the UNCOMPENSATED synthesis;
    /// the device compensates on-device).
    DelegatedDownload { samples: usize },
    /// A delegated absolute-scalar fire (Path A: `10^(dB/20)`).
    DelegatedFire { scale: f64, level_db: f64 },
    /// A freeform note (an honesty caveat surfaced during the run).
    Note { text: String },
    /// Closed loop: identification finished.
    Identified {
        /// The burst's peak as played, volts.
        drive_vpk: f64,
        /// Its cap, volts.
        max_vpk: f64,
        /// Mean in-band coherence.
        coherence_mean: f64,
        /// The floor it had to reach.
        min_mean_coherence: f64,
    },
    /// Closed loop: identification held the run before any pulse.
    Held { reason: HeldReason },
    /// The law was called: whether its output was used, and why not.
    LawCalled {
        /// Pulses fired before the call: 0 at the seed.
        pulse: u64,
        /// The seed or an iteration.
        call: LawCall,
        used: bool,
        /// The miss's wire name.
        reason: Option<String>,
        /// The miss in words.
        detail: Option<String>,
        call_ms: f64,
    },
}

/// The event log: a growing list of timestamped events.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventLog {
    events: Vec<Event>,
    next_seq: u64,
}

impl EventLog {
    /// Append an event with the current wall-clock timestamp.
    pub fn push(&mut self, kind: EventKind) {
        let unix_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let seq = self.next_seq;
        self.next_seq += 1;
        self.events.push(Event {
            seq,
            unix_secs,
            kind,
        });
    }

    /// The events, in order.
    pub fn events(&self) -> &[Event] {
        &self.events
    }
}

/// The honest limitations carried verbatim into every SRS-shock report (including
/// the zero-phase prediction caveat).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Honesty {
    /// Which drive path this run is on and what that means for the control law.
    pub path: String,
    /// Where Hertzforge's control law runs.
    pub control_law: String,
    /// The predictive-notch model caveat (zero-phase |H| prediction).
    pub predictive_notch: String,
    /// The system-ID posture.
    pub sysid: String,
    /// The feasibility posture (deterministic, crest = 1.0).
    pub feasibility: String,
    /// The drive V/I limiting split.
    pub vi_limiting: String,
}

impl Honesty {
    /// The Path-B (host) honesty block.
    pub fn host() -> Self {
        Self {
            path:
                "This run is Path B (host-closes-loop): Hertzforge synthesizes every pulse, plays \
                   it into the plant, and closes the SRS iteration on the host. The plant today \
                   is the hertzforge-hal-sim synthetic resonance plant; real-hardware Path B homes \
                   (direct-DAC hardware, NI) are gated on separate workstreams."
                    .into(),
            control_law: "The SrsSynthesisLaw per-band gain iteration (and its predictive- \
                          limit cap) runs ON THE HOST, pulse by pulse. It never runs on the \
                          delegated controller."
                .into(),
            predictive_notch: "Limit-channel predictions go through MAGNITUDE-only FRFs (a \
                               zero-phase response model, SAND2016-1335C reduced-level system \
                               ID): per-band amplitudes are right, inter-band phase alignment \
                               is approximate, so a prediction is an estimate, not a bound. \
                               The headroom_db budget exists to absorb exactly that slack."
                .into(),
            sysid: "System ID is HOST-driven at reduced level. Path-B-only: on the \
                    delegated path the device owns system ID at arm and Hertzforge only observes."
                .into(),
            feasibility: "Feasibility is DETERMINISTIC: crest factor exactly \
                          1.0, pure-integrator kinematics (no high-pass), assessed \
                          on the target-converged full-level reference (the intended control- \
                          point motion), consuming force_shock like-for-like by its rating \
                          basis. An Exceeded verdict HARD-BLOCKS arming until an explicit \
                          operator override-ack."
                .into(),
            vi_limiting: "Peak-ACCEL limiting is predictive pre-fire (via the measured drive-> \
                          control FRF); CURRENT/VOLTAGE ceilings are between-pulse MEASURED \
                          enforcement on wired monitor channels - the first over-ceiling pulse \
                          has already fired by the time it is caught."
                .into(),
        }
    }

    /// The Path-A (delegated controller) honesty block.
    pub fn delegated() -> Self {
        Self {
            path: "This run is Path A (delegated controller): Hertzforge downloads the synthesized \
                   reference ONCE and fires absolute 10^(dB/20) scalars. The DEVICE closes \
                   the loop."
                .into(),
            control_law: "Hertzforge's control law does NOT run on this path: no \
                          per-pulse gain updates, no predictive notching, no host re-synthesis. \
                          Hertzforge observes the pulses and enforces only the host-side observable \
                          protections (envelope, kinematics)."
                .into(),
            predictive_notch: "Predictive notching is INERT on this path: there is no host \
                               drive loop to notch, and no reduced-level host \
                               system ID to predict through."
                .into(),
            sysid: "The device owns system ID at arm; Hertzforge observes. No host \
                    FRF exists on this path."
                .into(),
            feasibility: "Feasibility is DETERMINISTIC and runs on the host \
                          before download, on the uncompensated reference. The download is the \
                          UNCOMPENSATED synthesis: the device compensates on-device, so host \
                          compensation would double-compensate and over-stroke."
                .into(),
            vi_limiting: "V/I are UNOBSERVABLE through the delegated path: every V/I monitor \
                          ceiling is inert/greyed (PathCapabilities:: \
                          vi_limiting_observable = false)."
                .into(),
        }
    }

    /// The block for `path`.
    pub fn for_path(path: DrivePath) -> Self {
        match path {
            DrivePath::HostDac => Self::host(),
            DrivePath::Delegated => Self::delegated(),
        }
    }
}

/// The run settings digest recorded in the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSettings {
    /// The drive path (Path A delegated / Path B host).
    pub path: DrivePath,
    pub f_lo_hz: f64,
    pub f_hi_hz: f64,
    /// The band grid actually analyzed (fractional-octave centers, Hz).
    pub band_freqs: Vec<f64>,
    /// The SDOF quality factor.
    pub q: f64,
    /// The synthesis engine name (`"wavsyn"` / `"seds"`).
    pub engine: String,
    /// The offline iteration knobs.
    pub max_iters: usize,
    pub tol_db: f64,
    pub max_gain_db_per_iter: f64,
    /// The level schedule.
    pub schedule: ShockLevelSchedule,
    /// The per-band envelope actually enforced.
    pub envelope_bands: Vec<SrsToleranceBand>,
    /// The drive-limit configuration.
    pub drive_limit: DriveLimit,
    pub drive_abort_factor: f64,
    /// Whether PVSS was computed per pulse.
    pub compute_pvss: bool,
    /// System-ID level, dB (None = system ID skipped / delegated).
    pub sysid_level_db: Option<f64>,
    /// Sample rate the run fired at, Hz - the rate the DEVICE configured and the
    /// service adopted at arm.
    pub fs_hz: f64,
    /// Sample rate the fire path was ASKED for. Differs from [`Self::fs_hz`]
    /// whenever the device coerced the request (routine on a cDAQ-9234, which
    /// rounds up a `13.1072 MHz/(256 n)` ladder). Both authorities are reported so
    /// a reader can see the coercion instead of inferring it.
    #[serde(default)]
    pub requested_fs_hz: f64,
    /// The ring-out window in SAMPLES at `fs_hz` (the tail of every fired record).
    #[serde(default)]
    pub ring_out_samples: usize,
    /// Whether this run held after a passing system ID for operator review.
    #[serde(default)]
    pub review_after_sysid: bool,
}

/// The feasibility digest carried in the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeasibilityDigest {
    /// The full deterministic feasibility report (items + overall + assumptions).
    pub report: FeasibilityReport,
    /// The shaker name assessed against (or a note if none was configured).
    pub shaker_name: String,
    /// The override acknowledgment logged to arm past an exceedance, if any.
    pub override_ack: Option<String>,
}

/// One identified FRF's summary (drive-referenced FRFs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrfSummary {
    /// The channel the FRF runs drive -> channel to (`"control"` or a limit/monitor
    /// channel name).
    pub channel: String,
    /// How the FRF was obtained: `"measured (reduced-level system ID)"` or
    /// `"user-supplied"`.
    pub source: String,
    /// Number of frequency points.
    pub points: usize,
    /// Mean in-band coherence of the estimate (measured FRFs only).
    pub band_coherence: Option<f64>,
    /// The FRF magnitude sampled on its grid (for the report's FRF plot).
    pub freqs_hz: Vec<f64>,
    pub mag: Vec<f64>,
}

/// One pulse's row in the report's pulse table. A row describes the pulse as
/// played: the gains it played and the law call that produced them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PulseRow {
    pub seq: u64,
    /// The scheduled (ladder) level, dB.
    pub level_db: f64,
    pub peak_accel_g: f64,
    pub peak_velocity_ips: f64,
    pub pk_pk_disp_in: f64,
    /// Worst signed per-band error vs the level-normalized target, dB.
    pub worst_err_db: Option<f64>,
    /// The band index of the worst error.
    pub worst_band: Option<usize>,
    /// Envelope alarm/abort band counts this pulse.
    pub alarm_bands: usize,
    pub abort_bands: usize,
    /// How many components the limit cap held back in the gains this pulse
    /// played (the cap applied after the pulse before it).
    pub limited_components: usize,
    /// The scheduled (ladder) level, dB; the same as `level_db`.
    #[serde(default)]
    pub scheduled_db: f64,
    /// The level the pulse was commanded at, dB: the scheduled level with the
    /// core's safety trims (the between-pulse monitors, the pre-fire peak
    /// scale, the drive ceiling).
    #[serde(default)]
    pub commanded_db: f64,
    /// The product of those safety trims, linear (1 when nothing trimmed).
    #[serde(default = "one")]
    pub limit_scale: f64,
    /// The drive's peak as played, volts (on a run without `control`, the
    /// synthesis in g written as volts).
    #[serde(default)]
    pub drive_vpk: Option<f64>,
    /// No band was outside the alarm band. `None` when the pulse was not
    /// judged.
    #[serde(default)]
    pub passed: Option<bool>,
    /// The worst band's centre frequency, Hz.
    #[serde(default)]
    pub worst_band_hz: Option<f64>,
    /// The worst predicted limit-channel exceedance for this pulse's drive,
    /// dB over the allowed level; `None` without limit channels.
    #[serde(default)]
    pub worst_limit_db: Option<f64>,
    /// The law call that produced the gains this pulse played; `None` on a run
    /// without `control`.
    #[serde(default)]
    pub law: Option<PulseLawCall>,
    /// When the pulse started, seconds on the run's pacer.
    #[serde(default)]
    pub at_s: Option<f64>,
}

fn one() -> f64 {
    1.0
}

/// The law call behind the gains a pulse played.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PulseLawCall {
    /// The call's gains were used; `false` when the gains in force were kept
    /// after a miss.
    pub used: bool,
    /// Why the call was not used.
    pub miss: Option<LawFault>,
    /// How long the call took, ms.
    pub call_ms: f64,
}

/// One fired pulse's records, for the pulse views: the measured SRS against
/// the target at the level fired with the envelope's bands, the measured
/// pulse, the drive as played, and the pulse's velocity and displacement.
#[derive(Debug, Clone, PartialEq)]
pub struct PulseView {
    /// The pulse's sequence number.
    pub pulse: u64,
    /// The rate every time history is at, Hz.
    pub fs_hz: f64,
    /// The scheduled level, dB.
    pub level_db: f64,
    /// The commanded level, dB.
    pub commanded_db: f64,
    /// The band centres, Hz.
    pub band_hz: Vec<f64>,
    /// The measured control SRS, EU, at the run's Q.
    pub measured_srs: Vec<f64>,
    /// The target at the level fired, EU.
    pub target_srs: Vec<f64>,
    /// The envelope's alarm band at the level fired, EU: `(low, high)` per
    /// band.
    pub alarm: Vec<(f64, f64)>,
    /// The envelope's abort band at the level fired, EU.
    pub abort: Vec<(f64, f64)>,
    /// The measured control record, EU.
    pub measured: Vec<f64>,
    /// The drive as played, volts (on a run without `control`, g written as
    /// volts), before its ring-out.
    pub drive: Vec<f64>,
    /// The measured record's velocity, in/s.
    pub velocity_ips: Vec<f64>,
    /// The measured record's displacement, in.
    pub displacement_in: Vec<f64>,
}

/// What identification found, as the status shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentificationSummary {
    /// The burst's shape.
    pub excitation: SysIdExcitation,
    /// The burst's peak as played, volts.
    pub drive_vpk: f64,
    /// Its cap, volts.
    pub max_vpk: f64,
    /// Mean in-band coherence of the control channel's gain.
    pub coherence_mean: f64,
    /// The hold's floor.
    pub min_mean_coherence: f64,
    /// The coherence reached the floor.
    pub passed: bool,
}

/// The drive the core predicts for the table in force, at the highest level
/// the run is scheduled to reach (AUTO), or the present level (MANUAL).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrivePrediction {
    /// The level the prediction is for, dB.
    pub level_db: f64,
    /// The drive's peak there, volts.
    pub drive_vpk: f64,
    /// The drive ceiling, volts peak.
    pub drive_vpk_ceiling: f64,
    /// The drive abort, volts peak.
    pub drive_vpk_abort: f64,
    /// The predicted control response's peak, EU.
    pub response_peak_g: f64,
    /// Its peak velocity, in/s.
    pub velocity_pk_ips: f64,
    /// Its peak-to-peak displacement, in.
    pub displacement_pk_pk_in: f64,
}

/// The arm-time limit notch, as the status shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotchSummary {
    /// Bands the notch cut.
    pub notched_bands: usize,
    /// The worst predicted exceedance before the notch, dB over the allowed
    /// level.
    pub worst_before_db: Option<f64>,
    /// The worst predicted exceedance after it.
    pub worst_after_db: Option<f64>,
    /// The notch's fit converged.
    pub converged: bool,
}

/// The law in force, as the status shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawStatus {
    /// The law's identity.
    pub descriptor: LawDescriptor,
    /// Unusable calls in a row now.
    pub misses_in_a_row: u32,
    /// Unusable calls this run.
    pub misses: u64,
    /// Calls this run, the seed included.
    pub calls: u64,
    /// The values the law reported on its last usable call.
    pub values: Vec<LawValue>,
}

/// The per-band achieved-vs-target digest at the LAST pulse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BandResult {
    pub freq_hz: f64,
    /// The full-level target, g.
    pub target_g: f64,
    /// The last pulse's measured SRS, g.
    pub measured_g: f64,
    /// The signed error vs the LEVEL-NORMALIZED target, dB.
    pub err_db: Option<f64>,
}

/// The notch/limit report section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitReport {
    /// The arm-time predictive-notch plan report.
    pub arm_plan: Option<NotchReport>,
    /// The per-channel bound curves on the band grid, g.
    pub bounds_g: Vec<(String, Vec<f64>)>,
    /// The LAST pulse's predicted limit-channel SRS, one per channel, on the band
    /// grid (through the reduced-level FRFs - a zero-phase estimate; see Honesty).
    pub last_predicted_srs: Vec<(String, Vec<f64>)>,
    /// The limit-cap status on the last pulse.
    pub last_status: LawLimitStatus,
    /// The prediction headroom, dB.
    pub headroom_db: f64,
}

/// One level-history entry: the rung and how it was entered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LevelHistoryEntry {
    pub level_db: f64,
    pub auto: bool,
    /// The pulse count when the level was entered.
    pub at_pulse: u64,
}

/// The delegated-path fire record (Path A honesty: what was downloaded, what was
/// fired).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DelegatedFireLog {
    /// Samples in the downloaded (uncompensated) reference. 0 = no
    /// download happened.
    pub reference_samples: usize,
    /// Every absolute scalar fired, in order.
    pub fired_scalars: Vec<f64>,
}

/// The serde-serializable report data the UI's report consumes. Self-contained.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportData {
    /// The run settings digest.
    pub settings: RunSettings,
    /// The feasibility digest.
    pub feasibility: Option<FeasibilityDigest>,
    /// The identified/user-supplied FRFs.
    pub frf_summary: Vec<FrfSummary>,
    /// The FINAL synthesis component table (the drive that fired the last pulse).
    pub components: Option<SynthesisEngine>,
    /// Per-band achieved-vs-target at the last pulse.
    pub band_results: Vec<BandResult>,
    /// The last pulse's PVSS on the band grid, in/s, if computed.
    pub last_pvss: Option<Vec<f64>>,
    /// The per-pulse table.
    pub pulses: Vec<PulseRow>,
    /// The full per-pulse records (time histories + SRS), for overlays.
    pub pulse_records: Vec<PulseRecord>,
    /// The notch/limit report, when limiting was configured.
    pub limit_report: Option<LimitReport>,
    /// The level history.
    pub level_history: Vec<LevelHistoryEntry>,
    /// The full timestamped event log.
    pub event_log: EventLog,
    /// The trip record if the run aborted.
    pub trip: Option<TripRecord>,
    /// The delegated fire log (Path A only; empty on Path B).
    pub delegated: DelegatedFireLog,
    /// User metadata.
    pub metadata: Vec<MetaField>,
    /// The honest limitations, path-matched.
    pub honesty: Honesty,
    /// The law's line and its counts; `None` on a run without `control`.
    #[serde(default)]
    pub law: Option<LawReport>,
}

/// The report's law section: one line naming the law, and the guard counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawReport {
    /// "name version (built in)", or the law file's short hash in place of
    /// "built in".
    pub line: String,
    pub descriptor: LawDescriptor,
    /// Calls, the seed included.
    pub calls: u64,
    /// Calls whose output was not used.
    pub misses: u64,
    /// Of those, outputs the core refused.
    pub refused: u64,
    /// Component gains the limit cap held back, over the run.
    pub limited_components: u64,
    /// The slowest call, ms.
    pub worst_call_ms: f64,
    /// One call's budget, ms.
    pub budget_ms: f64,
}

impl LawReport {
    /// The report's one line for a law: its name, version and a short hash
    /// (or "built in"), as classical shock's report writes it.
    pub fn line_for(d: &LawDescriptor) -> String {
        match &d.sha256 {
            Some(h) => format!("{} {} ({})", d.name, d.version, &h[..h.len().min(12)]),
            None => format!("{} {} (built in)", d.name, d.version),
        }
    }
}

/// The one status snapshot the UI polls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub state: RunState,
    /// The current commanded level, dB.
    pub current_level_db: f64,
    /// The current ladder table index.
    pub level_index: usize,
    /// Pulses fired so far (total).
    pub pulses_fired: u64,
    /// Pulses fired at the current level.
    pub pulses_at_level: u32,
    /// The last pulse's worst signed per-band error, dB.
    pub last_worst_err_db: Option<f64>,
    /// The last pulse's limit-cap status.
    pub last_limit_status: LawLimitStatus,
    /// Whether the last pulse alarmed the envelope.
    pub alarm_active: bool,
    /// Whether an abort has fired.
    pub abort_active: bool,
    /// Whether the AUTO ladder has completed its top rung.
    pub ladder_completed: bool,
    /// Whether a capture is open.
    pub capturing: bool,
    /// Mean in-band coherence from the system ID (0 when skipped).
    pub band_coherence: f64,
    /// The capture trigger in effect.
    pub capture_trigger: ShockCaptureTrigger,
    /// Seconds until the next pulse may start (the time between pulses);
    /// `None` when no pulse is waiting on the spacing.
    #[serde(default)]
    pub next_pulse_s: Option<f64>,
    /// Pulses with every band inside the alarm band.
    #[serde(default)]
    pub pulses_passed: u64,
    /// What identification found; `None` until it is done (or on a run
    /// without `control`).
    #[serde(default)]
    pub identification: Option<IdentificationSummary>,
    /// The drive the core predicts for the table in force; `None` before the
    /// law is seeded.
    #[serde(default)]
    pub prediction: Option<DrivePrediction>,
    /// The arm-time limit notch; `None` without limit channels.
    #[serde(default)]
    pub notch: Option<NotchSummary>,
    /// The law in force; `None` before it is built.
    #[serde(default)]
    pub law: Option<LawStatus>,
    /// Why identification held the run.
    #[serde(default)]
    pub held: Option<HeldReason>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_ends_are_terminal_and_not_running() {
        for s in [RunState::Held, RunState::NotConverged] {
            assert!(s.is_terminal());
            assert!(!s.is_running());
        }
        assert!(RunState::Stopped.is_terminal() && RunState::Aborted.is_terminal());
        assert!(!RunState::AwaitingReview.is_terminal());
        assert_eq!(ConvergenceLimit::PerLevel.as_str(), "per_level");
        assert_eq!(ConvergenceLimit::Run.as_str(), "run");
        assert_eq!(
            HeldReason::LowCoherence {
                mean: 0.5,
                floor: 0.8
            }
            .as_str(),
            "low_coherence"
        );
    }

    #[test]
    fn each_trip_has_its_wire_name_and_says_whether_it_was_predicted() {
        use hertzforge_shock_common::law_guard::OutputRule;
        let cases: Vec<(TripKind, &str, bool)> = vec![
            (
                TripKind::Envelope {
                    abort_bands: 1,
                    worst_band: 0,
                    worst_band_freq_hz: 100.0,
                    worst_err_db: 7.0,
                },
                "envelope",
                false,
            ),
            (
                TripKind::Kinematic {
                    axis: "displacement".into(),
                    value: 1.0,
                    limit: 0.5,
                    predicted: true,
                },
                "kinematic",
                true,
            ),
            (
                TripKind::DriveBreach {
                    channel: "peak accel (predicted)".into(),
                    peak: 2.0,
                    ceiling: 1.0,
                    predicted: true,
                },
                "accel",
                true,
            ),
            (
                TripKind::DriveBreach {
                    channel: "ch2".into(),
                    peak: 2.0,
                    ceiling: 1.0,
                    predicted: false,
                },
                "monitor",
                false,
            ),
            (
                TripKind::Plant {
                    message: "x".into(),
                },
                "plant",
                false,
            ),
            (
                TripKind::DriveVpk {
                    vpk: 0.9,
                    abort_vpk: 0.8,
                    predicted: true,
                },
                "drive_vpk",
                true,
            ),
            (
                TripKind::NotConverged {
                    limit: ConvergenceLimit::PerLevel,
                    pulses: 5,
                },
                "not_converged",
                false,
            ),
            (
                TripKind::LawFault {
                    fault: LawFault::BadOutput(OutputRule::GainRise),
                    consecutive: 3,
                },
                "law_fault",
                false,
            ),
        ];
        for (k, name, predicted) in cases {
            assert_eq!(k.name(), name, "{k:?}");
            assert_eq!(k.predicted(), predicted, "{k:?}");
            let back: TripKind = serde_json::from_value(serde_json::to_value(&k).unwrap()).unwrap();
            assert_eq!(back, k);
        }
    }

    #[test]
    fn records_written_before_the_new_fields_still_read() {
        // A kinematic trip with no `predicted` was measured.
        let k: TripKind =
            serde_json::from_str(r#"{"Kinematic":{"axis":"velocity","value":2.0,"limit":1.0}}"#)
                .unwrap();
        assert!(!k.predicted());
        // A pulse row with only the old fields.
        let row: PulseRow = serde_json::from_str(
            r#"{"seq":0,"level_db":-12.0,"peak_accel_g":1.0,"peak_velocity_ips":0.1,
                "pk_pk_disp_in":0.01,"worst_err_db":0.5,"worst_band":2,"alarm_bands":0,
                "abort_bands":0,"limited_components":0}"#,
        )
        .unwrap();
        assert_eq!(row.limit_scale, 1.0);
        assert_eq!(row.passed, None);
        assert_eq!(row.law, None);
    }
}
