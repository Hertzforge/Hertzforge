//! Event log, report data, honesty section, and the UI status surface for the
//! SRS-shock run service, mirroring `hertzforge-random::service::report` /
//! the Explorer service's conventions: a timestamped event log of every state
//! change / trip / capture / snapshot, a serde-serializable [`ReportData`] the UI's
//! report consumes, and an honesty section carried verbatim into every report.

use crate::iterate::SynthesisEngine;
use crate::law::LawLimitStatus;
use crate::limit::NotchReport;
use crate::service::config::{MetaField, ShockCaptureTrigger, SrsToleranceBand};
use hertzforge_shaker_db::FeasibilityReport;
use hertzforge_shock_common::{DriveLimit, DrivePath, PulseRecord, ShockLevelSchedule};
use serde::{Deserialize, Serialize};

/// The SRS-shock run state machine (mirrors Random's state machine with shock semantics):
/// `Idle -> Feasibility -> SysId -> Armed -> Firing -> Stopped`, plus `Aborted` from
/// any running state, plus the `FeasibilityBlocked` hard block (an
/// Exceeded feasibility HARD-BLOCKS arming until an explicit override-ack).
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
}

impl RunState {
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

/// One pulse's row in the report's pulse table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PulseRow {
    pub seq: u64,
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
    /// How many component gains the limit cap bound this pulse.
    pub limited_components: usize,
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
}
