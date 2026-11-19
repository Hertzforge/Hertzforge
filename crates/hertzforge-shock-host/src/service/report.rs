//! Event log, report data, honesty section, and the UI status surface for the
//! classical-shock run service.
//!
//! Mirrors the hertzforge-random / Explorer `service::{Event, EventLog, ReportData,
//! Honesty, TripRecord, Status}` conventions: a timestamped event log of every
//! state change / fired pulse / trip / capture / snapshot, a serde-serializable
//! [`ReportData`] the UI's report consumes, and an [`Honesty`] section carried
//! verbatim into every report.

use crate::service::config::{ClassicalTestConfig, MetaField};
use crate::tolerance::{ToleranceParams, ToleranceReport, ToleranceStandard};
use hertzforge_shaker_db::FeasibilityReport;
use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault};
use hertzforge_shock_common::train::TrainPlan;
use hertzforge_shock_common::{DrivePath, PathCapabilities};
use serde::{Deserialize, Serialize};

/// The run state machine (mirroring Random's level-ladder shape with shock
/// semantics): `Idle -> Feasibility -> Armed -> Firing -> Stopped`, plus
/// `FeasibilityBlocked` (hard block until `arm_with_override`) and
/// `Aborted` from any firing state (tolerance ABORT / kinematic breach /
/// refused pre-fire).
///
/// A closed-loop run (`ClassicalTestConfig::control` set) adds
/// `Armed -> SysId -> AwaitingReview -> Firing` (identification, then the
/// review hold when it is on), and two more ends: `Held` (identification held
/// the run before any pulse) and `NotConverged` (the iteration limit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState {
    /// No run started.
    Idle,
    /// Pre-test feasibility assessed. If Exceeded, the state parks at
    /// [`RunState::FeasibilityBlocked`] until an explicit override arms.
    Feasibility,
    /// Feasibility Exceeded: arming is HARD-BLOCKED until `arm_with_override`
    /// (same override-with-ack pattern as Random's feasibility block).
    FeasibilityBlocked,
    /// Armed: the fire path is open (overlay/reference downloaded on the
    /// delegated path; the backend loop running on Path B), no pulse fired yet.
    Armed,
    /// Pulses are being fired (ladder / train / gunfire).
    Firing,
    /// Cleanly stopped (ladder completed or operator stop).
    Stopped,
    /// A protection engine tripped; stopped immediately with a trip record.
    Aborted,
    /// Closed loop: system identification is playing (one fire of band-limited
    /// random in volts).
    SysId,
    /// Closed loop: identification is done and the run holds for the operator
    /// to review it; nothing fires until `proceed`.
    AwaitingReview,
    /// Closed loop: identification held the run before any pulse (its in-band
    /// coherence was under the floor, or the trusted bins did not cover enough
    /// of the reference's energy). Terminal; [`Status::held`] says which.
    Held,
    /// Closed loop: the iteration limit ended the run (pulses at one level
    /// without a pass, or the run's pulse cap). Terminal; the trip record is
    /// [`TripKind::NotConverged`].
    NotConverged,
}

impl RunState {
    /// True while pulses can fire (a protection abort can trip).
    pub fn is_running(self) -> bool {
        matches!(self, RunState::Armed | RunState::Firing)
    }

    /// True once the run has ended: stopped, aborted, held or not converged.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunState::Stopped | RunState::Aborted | RunState::Held | RunState::NotConverged
        )
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
    /// The bins the core trusts cover less of the reference's energy than the
    /// run asks for: reference content outside the correctable band.
    ReferenceOutOfBand {
        /// The fraction of the reference's energy the trusted bins cover.
        coverage: f64,
        /// The fraction asked for (`min_band_energy`).
        min: f64,
    },
}

impl HeldReason {
    /// The stable name: `low_coherence` or `reference_out_of_band`.
    pub fn as_str(&self) -> &'static str {
        match self {
            HeldReason::LowCoherence { .. } => "low_coherence",
            HeldReason::ReferenceOutOfBand { .. } => "reference_out_of_band",
        }
    }
}

impl std::fmt::Display for HeldReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeldReason::LowCoherence { mean, floor } => write!(
                f,
                "identification coherence {mean:.3} is under the floor of {floor:.3}; raise \
                 the identification drive or lengthen it"
            ),
            HeldReason::ReferenceOutOfBand { coverage, min } => write!(
                f,
                "the trusted bins cover {:.1}% of the reference's energy, under the {:.1}% \
                 asked for; the pulse has content outside the correctable band",
                coverage * 100.0,
                min * 100.0
            ),
        }
    }
}

/// Where a fired pulse came from (the report's per-pulse provenance column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PulseOrigin {
    /// A ladder-scheduled pulse (AUTO progression or a MANUAL single fire).
    Ladder,
    /// A pulse-train burst pulse.
    Train,
    /// A gunfire Procedure-IV burst pulse.
    Gunfire,
}

/// The kind of protection trip that aborted the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TripKind {
    /// A fired pulse failed the tolerance report (the tolerance abort).
    Tolerance {
        /// The service-level pulse number that failed.
        pulse: u64,
        /// The failed report's envelope verdict.
        envelope_pass: bool,
        /// The failed report's velocity-change verdict.
        velocity_change_pass: bool,
        /// The dV error fraction of the failed pulse.
        error_frac: f64,
        /// Worst envelope deviation as a fraction of the nominal peak.
        worst_deviation_frac_of_peak: f64,
    },
    /// A measured pulse kinematic exceeded a shaker rating (peak velocity or
    /// pk-pk displacement vs the shaker record - the run-time breach check,
    /// feasibility types reused).
    Kinematic {
        /// Which kinematic breached ("peak velocity (in/s)" / "pk-pk
        /// displacement (in)").
        quantity: String,
        value: f64,
        limit: f64,
        pulse: u64,
        /// The breach was found on the closed loop's prediction of the next
        /// pulse, before it fired: nothing was driven for it. `false` for a
        /// measured breach.
        #[serde(default)]
        predicted: bool,
    },
    /// The pre-fire drive-limit check refused to fire (invalid prediction,
    /// Garbage in must not fire a pulse).
    PreFireRefused { reason: String },
    /// The OPERATOR aborted: `ClassicalShockService::abort` raised the
    /// cancel flag and issued `Backend::emergency_stop`. Distinct from a
    /// protection trip because nothing was out of tolerance - the run was ended
    /// by a person, and a report that called that a "trip" would misattribute
    /// it.
    OperatorAbort {
        /// What the host said it was aborting for, verbatim (empty when the host
        /// gave no reason).
        reason: String,
    },
    /// Closed loop: a measured peak over `(1 + overshoot_abort)` times the
    /// commanded peak, at any level.
    Overshoot {
        /// The pulse that overshot.
        pulse: u64,
        /// Measured peak over commanded peak.
        peak_ratio: f64,
        /// The ratio that aborts, `1 + overshoot_abort`.
        limit_ratio: f64,
    },
    /// Closed loop: a commanded drive over the drive abort, volts peak. Nothing
    /// was played for it.
    DriveVpk {
        /// The pulse it would have been (the pulses fired so far).
        pulse: u64,
        /// The drive's peak, volts.
        vpk: f64,
        /// The drive abort, volts peak.
        abort_vpk: f64,
        /// Found on the predicted full-level drive at proceed, before any
        /// pulse; `false` for a drive refused just before it would have played.
        predicted: bool,
    },
    /// Closed loop: the error rose by more than 1 dB on `pulses` pulses in a
    /// row at one level.
    Divergence {
        /// The pulse that completed the run of rises.
        pulse: u64,
        /// Pulses in a row with a rising error.
        pulses: u32,
        /// That pulse's error, percent of the reference.
        error_pct: f64,
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
    /// Closed loop: the control law gave no usable drive `consecutive` times in
    /// a row, or its seed was refused or could not be built. Nothing more
    /// fired and the law was not called again.
    LawFault {
        /// Why the last call was not used (`fault.reason()` is the wire name).
        fault: LawFault,
        /// Unusable calls in a row when the run stopped.
        consecutive: u32,
    },
}

impl TripKind {
    /// The stable name of the trip's kind: `tolerance`, `kinematic`, `accel`
    /// (the pre-fire check refused), `operator_abort`, `overshoot`,
    /// `drive_vpk`, `divergence`, `not_converged` or `law_fault`.
    pub fn name(&self) -> &'static str {
        match self {
            TripKind::Tolerance { .. } => "tolerance",
            TripKind::Kinematic { .. } => "kinematic",
            TripKind::PreFireRefused { .. } => "accel",
            TripKind::OperatorAbort { .. } => "operator_abort",
            TripKind::Overshoot { .. } => "overshoot",
            TripKind::DriveVpk { .. } => "drive_vpk",
            TripKind::Divergence { .. } => "divergence",
            TripKind::NotConverged { .. } => "not_converged",
            TripKind::LawFault { .. } => "law_fault",
        }
    }

    /// True for a trip found on a prediction before a pulse fired, so nothing
    /// was driven for it.
    pub fn predicted(&self) -> bool {
        match self {
            TripKind::Kinematic { predicted, .. } | TripKind::DriveVpk { predicted, .. } => {
                *predicted
            }
            _ => false,
        }
    }
}

/// A record of the trip that aborted the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TripRecord {
    pub kind: TripKind,
    /// The commanded level (dB) at the trip.
    pub at_level_db: f64,
    /// The service-level pulse count when the trip fired.
    pub at_pulse: u64,
}

/// A single event in the run log with a monotonic index + coarse wall-clock
/// timestamp (the hertzforge-random / Explorer event-log convention).
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
    /// Feasibility assessed with its overall verdict text.
    Feasibility {
        overall: String,
        exceeded: bool,
        has_advisory: bool,
    },
    /// An override acknowledgment was logged to arm past a feasibility
    /// exceedance.
    OverrideAck { ack: String },
    /// A level target changed (AUTO ladder advance, or an operator step/jump).
    LevelChange {
        from_db: f64,
        to_db: f64,
        auto: bool,
    },
    /// One pulse fired (the per-pulse pipeline ran). `measured` is `false` on
    /// the delegated path where the response is unobservable.
    PulseFired {
        pulse: u64,
        scheduled_level_db: f64,
        commanded_db: f64,
        measured: bool,
        peak_g: Option<f64>,
        velocity_change_ips: Option<f64>,
        tolerance_pass: Option<bool>,
    },
    /// A tolerance breach with abort-on-tolerance disabled (alarm-only).
    ToleranceAlarm {
        pulse: u64,
        error_frac: f64,
        worst_deviation_frac_of_peak: f64,
    },
    /// A drive-limit action scaled (or refused) a pulse.
    DriveLimitAction {
        pulse: u64,
        /// `"pre-fire"` (predictive accel) or `"between-pulse"` (measured
        /// monitor enforcement).
        phase: String,
        scale: f64,
        reason: String,
    },
    /// A pulse train / gunfire burst was planned.
    TrainPlanned {
        count: u32,
        realized_spacing_s: f64,
        floor_s: f64,
        hardware_blocked: bool,
        gunfire_rounds_per_min: Option<f64>,
    },
    /// A pulse train / gunfire burst was REFUSED by the rate model or the path
    /// capability table.
    TrainRefused { reason: String },
    /// A pulse train ended EARLY because the host lowered its `should_continue`
    /// flag. The cancel lands between pulses, so `fired` pulses went out whole
    /// and nothing was queued at the device when the burst ended.
    TrainCancelled {
        /// Pulses that actually fired before the flag went low.
        fired: u32,
        /// Pulses the plan called for.
        of_count: u32,
    },
    /// Capture started.
    CaptureStart { run_dir: String },
    /// Capture stopped, with the drop count.
    CaptureStop { chunks_written: u64, dropped: u64 },
    /// A snapshot was saved.
    Snapshot { path: String },
    /// A protection trip aborted the run.
    Trip { record: TripRecord },
    /// A fire was CANCELLED at a block boundary by the host's cancel flag
    /// (Stop / Abort). Carries the committed-queue arithmetic verbatim from
    /// `hertzforge_hal::StreamError::Cancelled`: mid-record preemption is REFUSED by
    /// this chassis, so `frames_committed - frames_done` frames were still ahead
    /// of the read cursor and kept playing.
    FireCancelled {
        pulse: u64,
        frames_done: usize,
        frames_committed: usize,
        frames_expected: usize,
    },
    /// A non-fatal DRIVER diagnostic drained off the fire path after a fire
    /// (`Backend::take_warnings`): a DAQmx underrun, a coerced rate, a late
    /// sample. Nothing in the shock path used to drain these, so the
    /// driver could report an underrun and no operator would ever see it.
    DriverWarning {
        /// The pulse the drain followed (`None` at arm / stop).
        pulse: Option<u64>,
        text: String,
    },
    /// The device COERCED the requested sample rate and the service adopted the
    /// device's. Everything Hz-denominated from here on is
    /// built at `actual_hz`; `requested_hz` is what the profile asked for.
    RateAdopted {
        requested_hz: f64,
        actual_hz: f64,
        /// `|requested - actual| / actual`.
        relative_error: f64,
    },
    /// A freeform note (an honesty caveat surfaced during the run).
    Note { text: String },
    /// Closed loop: system identification finished.
    Identified {
        /// Mean in-band coherence.
        coherence_mean: f64,
        /// The floor it had to reach.
        min_mean_coherence: f64,
        /// Bins the core trusts on the pulse grid.
        trusted_bins: usize,
        /// The fraction of the reference's energy they cover.
        band_energy: f64,
        /// The identification drive's peak as played, volts.
        drive_vpk: f64,
    },
    /// Closed loop: identification held the run before any pulse.
    Held { reason: HeldReason },
    /// Closed loop: the control law was called.
    LawCalled {
        /// Pulses fired before the call.
        pulse: u64,
        /// The seed or an iteration.
        call: hertzforge_shock_common::law_guard::LawCall,
        /// The output was used.
        used: bool,
        /// Why it was not (the wire name), when it was not.
        reason: Option<String>,
        /// What the core said about it, when it was not used.
        detail: Option<String>,
        /// How long the call took, ms.
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

/// The honest limitations carried verbatim into every classical-shock report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Honesty {
    /// Which drive path this run is on and what the compensation split means for it.
    pub path: String,
    /// Where the train hardware-block does and does not apply (path-aware since
    /// 2026-08-08).
    pub trains: String,
    /// The rate-model floors are PLACEHOLDERS pending hardware
    /// characterization.
    pub rate_model: String,
    /// The drive-limit honesty split (predictive accel vs between-pulse
    /// measured V/I; V/I inert on Path A).
    pub drive_limit: String,
    /// Feasibility is DETERMINISTIC (crest = 1.0, pure integrator) and, on Path A,
    /// a host-side ESTIMATE of the on-device compensation demand.
    pub feasibility: String,
    /// The encoded tolerance band parameters must be verified against the
    /// controlling standard revision before FORMAL compliance.
    pub tolerance: String,
    /// Response capture on the delegated path is not yet decoded: delegated
    /// pulses are recorded as fired WITHOUT measurement.
    pub delegated_capture: String,
}

impl Honesty {
    /// The honesty section for a run on `path`.
    pub fn for_path(path: DrivePath) -> Self {
        let path_text = match path {
            DrivePath::HostDac => {
                "This run is Path B (HostDac): Hertzforge synthesizes the HOST-COMPENSATED composite \
                 and drives the DAC directly - the hertzforge-hal-sim plant today; \
                 direct-DAC on real hardware is gated on the cap_cal decode. Host compensation, \
                 at-rate trains, and V/I monitors are available per PathCapabilities."
                    .to_string()
            }
            DrivePath::Delegated => {
                "This run is Path A (delegated controller): the device closes the loop on-device. The \
                 download is the UNCOMPENSATED main pulse (finding F1 - host compensation here \
                 would DOUBLE-COMPENSATE and over-stroke; the ReferenceForPath::delegated seam \
                 enforces it) and fires are absolute 10^(dB/20) setpoints. Host compensation, \
                 at-rate trains, V/I monitors, and Hertzforge's control law are greyed per \
                 PathCapabilities."
                    .to_string()
            }
        };
        Honesty {
            path: path_text,
            trains: match path {
                DrivePath::Delegated => {
                    "Pulse trains and gunfire bursts are HARDWARE-BLOCKED on this path: \
                     whether a setpoint write triggers a fire or a separate \
                     trigger exists is an undecoded hardware unknown (the decoded -12 dB \
                     repeats may be re-asserts). At-rate trains are refused here outright \
                     per PathCapabilities; live-hardware delegated trains await the \
                     inbound-capture decode."
                }
                DrivePath::HostDac => {
                    "Finding F8 (the undecoded write-triggers-fire semantic) is a DELEGATED \
                     -path unknown and does NOT apply to this run: Path B writes its own \
                     drive samples into the DAC, so there is no trigger semantic to decode. \
                     Until 2026-08-08 every host train carried the F8 marker anyway, which \
                     described nothing about this path. What DOES bound a host train is the \
                     rate model below: its floors are enforced in WALL time and its numbers \
                     are placeholders."
                }
            }
            .into(),
            rate_model: "The rate-model floors (cooling / settling / compute / device-recompute) \
                         are conservative PLACEHOLDERS pending the SH4 hardware \
                         characterization (finding F7). The model refuses spacings below the \
                         floors; at-rate trains are HostDac-only."
                .into(),
            drive_limit: "Peak-accel limiting is PREDICTIVE pre-fire (deterministic crest = 1 \
                          prediction on the commanded waveform); CURRENT/VOLTAGE ceilings are \
                          BETWEEN-PULSE measured enforcement on wired monitor channels - the \
                          first breach has already fired by the time it is caught. V/I monitors \
                          are INERT (greyed) on Path A where the drops are \
                          unobservable."
                .into(),
            feasibility: "Feasibility is the DETERMINISTIC kinematics path: crest \
                          factor = 1.0, pure no-high-pass double integration, \
                          force_shock consumed like-for-like by its rating basis. On Path A the \
                          assessed waveform is the HOST-side compensated composite - an ESTIMATE \
                          of the on-device compensation demand (the device compensates \
                          on-device, F1)."
                .into(),
            tolerance: "The per-standard tolerance band constants (MIL-STD-810 516 +/-15% amp / \
                        +/-10% dV; IEC 60068-2-27 +/-20% / +/-15%) encode the widely-reproduced \
                        figures; verify against the controlling standard revision before FORMAL \
                        compliance (see the caveat in hertzforge_shock_host::tolerance)."
                .into(),
            delegated_capture: "On the delegated path the pulse RESPONSE is not observable \
                                (inbound telemetry undecoded): delegated pulses are \
                                recorded as FIRED without a measured record, so per-pulse \
                                tolerance/SRS/kinematics are not evaluated there. Path B (Sim) \
                                measures every pulse."
                .into(),
        }
    }
}

/// The feasibility digest carried in the report: the full report + the
/// shaker name + any override acknowledgment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeasibilityDigest {
    /// The full feasibility report (items + overall + assumptions).
    pub report: FeasibilityReport,
    /// The shaker name assessed against (or a note if no shaker was configured).
    pub shaker_name: String,
    /// The override acknowledgment logged to arm past an exceedance, if any.
    pub override_ack: Option<String>,
}

/// One level-history entry: the level dB and how it was entered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LevelHistoryEntry {
    pub level_db: f64,
    /// `true` for AUTO ladder progression, `false` for operator steps/jumps.
    pub auto: bool,
    /// The service-level pulse count when the level was entered.
    pub at_pulse: u64,
}

/// One channel's row in the report's channel-table digest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelRow {
    pub index: u32,
    pub role: String,
    pub units: String,
    pub range_max: f64,
}

/// One fired pulse's row in the report (the per-pulse table -
/// level, peak g, dV, SRS pass/fail per band, tolerance verdicts). `measured`
/// is `false` (and the measurement columns `None`) for delegated-path pulses
/// whose response is unobservable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PulseRow {
    /// Service-level fire number (0-based, every fired pulse).
    pub pulse: u64,
    /// The `PulseStore` sequence of the ingested record, when measured.
    pub store_seq: Option<u64>,
    /// Where the pulse came from (ladder / train / gunfire).
    pub origin: PulseOrigin,
    /// The ladder-scheduled level, dB.
    pub scheduled_level_db: f64,
    /// The commanded level actually fired, dB (scheduled + any drive-limit
    /// scale-down expressed in dB).
    pub commanded_db: f64,
    /// The commanded peak amplitude, g (`spec.amplitude_g * 10^(commanded/20)`).
    pub commanded_amplitude_g: f64,
    /// The extra drive-limit scale applied to this pulse (1.0 = none).
    pub drive_limit_scale: f64,
    /// SIMULATED run-clock fire time, seconds since arm: `run_clock_s`, whose
    /// only mutation is `run_clock_s += spacing_s`.
    ///
    /// **Not spacing evidence.** Asserting that consecutive values differ by the
    /// spacing asserts `(x + s) - x == s`, which cannot fail. Use
    /// [`PulseRow::at_wall_time_s`].
    pub at_run_time_s: f64,
    /// WALL-clock fire time, seconds on the service's injected `TrainPacer`
    /// epoch - the timestamp taken once the station's start call returned, its
    /// clock running (`FireControl::hooks`), which is when the pulse started.
    ///
    /// THE spacing evidence for a single pulse, and the per-pulse analogue of
    /// `TrainRecord::wall_times_s`. It exists because the AUTO ladder had no
    /// wall-time record at all until 2026-08-08: `run_auto` charged the
    /// rate-model floor to the simulated clock and fired its rungs back to back,
    /// fastest at the highest drive.
    pub at_wall_time_s: f64,
    /// The pulse's bring-up, timed: the drive's checks, the arm, the commit, the
    /// wait for the time between pulses and the start call. `None` on a path
    /// that reports no shot timing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bring_up: Option<hertzforge_shock_common::BringUp>,
    /// Whether a response was measured (false on the delegated path).
    pub measured: bool,
    /// Measured peak acceleration, g.
    pub peak_g: Option<f64>,
    /// Measured velocity change over the standard's dV window, in/s.
    pub velocity_change_ips: Option<f64>,
    /// Measured pk-pk displacement, in.
    pub pk_pk_disp_in: Option<f64>,
    /// The overall tolerance verdict.
    pub tolerance_overall: Option<bool>,
    /// Tolerance check (a): amplitude envelope.
    pub envelope_pass: Option<bool>,
    /// Tolerance check (b): velocity change.
    pub velocity_change_pass: Option<bool>,
    /// Tolerance check (c): substitution dV equivalence.
    pub substitution_pass: Option<bool>,
    /// Per-band SRS fidelity flags (measured vs commanded SRS within the config
    /// tolerance), on the run's SRS grid. Informational (see config docs).
    pub srs_band_pass: Option<Vec<bool>>,
    /// Count of passing SRS bands.
    pub srs_bands_pass: Option<usize>,
    /// Total SRS bands checked.
    pub srs_bands_total: Option<usize>,
    /// Worst absolute per-band SRS deviation, dB.
    pub srs_worst_dev_db: Option<f64>,
    /// The commanded drive's peak, volts (the record as played, after every
    /// limit). On the open loop the drive is the reference in g written as
    /// volts, so this is its peak in g.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive_vpk: Option<f64>,
    /// RMS error over the standard's judged window, percent of the commanded
    /// pulse's RMS there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_pct: Option<f64>,
    /// Measured peak over commanded peak.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_ratio: Option<f64>,
    /// Measured velocity change over the nominal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity_change_ratio: Option<f64>,
    /// The law call that produced the drive this pulse played (closed loop
    /// only; `None` for the open loop and for a pulse fired in force by a
    /// train).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub law: Option<PulseLawCall>,
}

/// The law call behind one pulse's drive, as the pulse table shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PulseLawCall {
    /// The call's output was used. `false`: the pulse refired the drive in
    /// force after a miss.
    pub used: bool,
    /// Why the call was not used.
    pub miss: Option<LawFault>,
    /// Drive bins the core's per-pulse cap held back on that output.
    pub capped_bins: u64,
    /// The core's per-pulse cap held the drive's peak back.
    pub peak_capped: bool,
    /// How long the call took, ms.
    pub call_ms: f64,
}

/// One fired pulse's records on one time axis: the record as the station
/// played and measured it (`fs_hz`, sample 0 the first sample played).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PulseView {
    /// The pulse number.
    pub pulse: u64,
    /// The rate the record was played at, Hz.
    pub fs_hz: f64,
    /// The commanded level, dB.
    pub commanded_db: f64,
    /// The reference at the level fired: what the pulse aimed for, EU.
    pub reference: Vec<f64>,
    /// The measured control channel, EU.
    pub measured: Vec<f64>,
    /// The drive as played, volts (g written as volts on the open loop).
    pub drive: Vec<f64>,
    /// The standard's tolerance band around the aligned nominal pulse, over
    /// the judged window only.
    pub band: ToleranceBand,
}

/// The tolerance band over the judged window: the nominal main pulse at the
/// commanded amplitude, plus and minus the standard's amplitude tolerance, and
/// zero plus and minus it in the quiet windows either side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToleranceBand {
    /// The record sample the band's first value is at (may be negative when
    /// the window starts before the record).
    pub start: i64,
    /// The main pulse's first sample, as the checker aligned it.
    pub main_start: i64,
    /// The main pulse's length, samples.
    pub main_len: usize,
    /// The band's lower edge, EU.
    pub lower: Vec<f64>,
    /// The band's upper edge, EU.
    pub upper: Vec<f64>,
}

/// What system identification found, for the status and the review hold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentificationSummary {
    /// The identification drive, volts rms.
    pub drive_vrms: f64,
    /// Its peak as played, volts.
    pub drive_vpk: f64,
    /// Mean in-band coherence.
    pub coherence_mean: f64,
    /// The floor it had to reach.
    pub min_mean_coherence: f64,
    /// Bins the core trusts on the pulse grid.
    pub trusted_bins: usize,
    /// The fraction of the reference's energy the trusted bins cover.
    pub band_energy: f64,
    /// Identification let the run go on.
    pub passed: bool,
}

/// The core's prediction of the full-level drive and response, made at
/// proceed from the law's seed and kept up to date after each law call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrivePrediction {
    /// The full-level drive's peak, volts.
    pub drive_vpk: f64,
    /// The drive ceiling, volts peak.
    pub drive_vpk_ceiling: f64,
    /// The drive abort, volts peak.
    pub drive_vpk_abort: f64,
    /// The predicted response's peak, EU.
    pub response_peak_g: f64,
    /// The predicted response's peak velocity, in/s.
    pub velocity_pk_ips: f64,
    /// The predicted response's pk-pk displacement, in.
    pub displacement_pk_pk_in: f64,
}

/// The control law a closed-loop run is using, and how its calls are going.
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
    /// The law's values on its last usable call.
    pub values: Vec<hertzforge_shock_common::law_guard::LawValue>,
}

/// One drive-limit action recorded for the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriveLimitActionRecord {
    /// The service-level pulse the action applied to.
    pub pulse: u64,
    /// `"pre-fire"` or `"between-pulse"`.
    pub phase: String,
    /// The scale applied (to THIS pulse pre-fire; to the NEXT pulse
    /// between-pulse).
    pub scale: f64,
    pub reason: String,
}

/// One planned-and-fired pulse train / gunfire burst.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrainRecord {
    /// The realized plan (spacing, floor, HARDWARE-BLOCKED marker).
    pub plan: TrainPlan,
    /// The service-level pulse numbers fired in this burst.
    pub pulse_numbers: Vec<u64>,
    /// WALL-CLOCK start time of each fired pulse, seconds on the service's
    /// [`TrainPacer`](crate::service::pacer::TrainPacer) epoch, parallel to
    /// `pulse_numbers`.
    ///
    /// This is the burst's REALIZED timeline and the only honest evidence that
    /// the rate-model spacing was kept: `PulseRow::at_run_time_s` comes from the
    /// simulated `run_clock_s`, which is incremented BY the spacing and
    /// therefore always agrees with it.
    #[serde(default)]
    pub wall_times_s: Vec<f64>,
    /// The gunfire rate when this burst was a gunfire burst (spacing =
    /// `60 / rate`), else `None`.
    pub gunfire_rounds_per_min: Option<f64>,
    /// The pinned level the burst fired at, dB (trains are level-pinned; they
    /// do not walk the AUTO ladder mid-burst).
    pub level_db: f64,
}

/// The tolerance summary for the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToleranceSummary {
    pub standard: ToleranceStandard,
    /// The encoded band parameters applied (verify before formal
    /// compliance, see [`Honesty::tolerance`]).
    pub params: ToleranceParams,
    /// Pulses with a measured record that were checked.
    pub checked: u64,
    /// Pulses whose tolerance report passed overall.
    pub passed: u64,
    /// The last pulse's full tolerance report.
    pub last: Option<ToleranceReport>,
}

/// The serde-serializable report data the UI's report consumes.
/// Everything the report needs, self-contained.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportData {
    /// The full test configuration (itself serde - no digest mirror to drift).
    pub settings: ClassicalTestConfig,
    /// The channel-table digest.
    pub channels: Vec<ChannelRow>,
    /// The drive path this run was on (the compensation discriminant).
    pub path: DrivePath,
    /// The path's capability table as the run believed it (one source of truth).
    pub capabilities: PathCapabilities,
    /// The feasibility digest + override ack.
    pub feasibility: Option<FeasibilityDigest>,
    /// The per-pulse table.
    pub pulses: Vec<PulseRow>,
    /// The level history.
    pub level_history: Vec<LevelHistoryEntry>,
    /// The full timestamped event log.
    pub event_log: EventLog,
    /// The tolerance summary.
    pub tolerance: ToleranceSummary,
    /// Every drive-limit action taken.
    pub drive_limit_actions: Vec<DriveLimitActionRecord>,
    /// Every pulse train / gunfire burst fired.
    pub trains: Vec<TrainRecord>,
    /// The trip record if the run aborted.
    pub trip: Option<TripRecord>,
    /// The user-defined metadata.
    pub metadata: Vec<MetaField>,
    /// The honest limitations.
    pub honesty: Honesty,
    /// Closed loop: what identification found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identification: Option<IdentificationSummary>,
    /// Closed loop: the control law's line and its guard counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub law: Option<LawReport>,
}

/// The report's control-law line and the guard counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawReport {
    /// One line: the law's name, version and short hash.
    pub line: String,
    /// The law's identity.
    pub descriptor: LawDescriptor,
    /// Calls, the seed included.
    pub calls: u64,
    /// Calls whose output was not used.
    pub misses: u64,
    /// Calls whose output the core refused (a subset of `misses`).
    pub refused: u64,
    /// Drive bins over the per-pulse cap, over the run's refused outputs.
    pub capped_bins: u64,
    /// Outputs whose peak was over the per-pulse cap.
    pub capped_peaks: u64,
    /// The slowest call, ms.
    pub worst_call_ms: f64,
    /// One call's budget, ms.
    pub budget_ms: f64,
}

impl LawReport {
    /// The report's one line for `d`: name, version and the first 12 hex
    /// digits of a law file's hash ("built in" for a built-in law).
    pub fn line_for(d: &LawDescriptor) -> String {
        match &d.sha256 {
            Some(h) => format!("{} {} ({})", d.name, d.version, &h[..h.len().min(12)]),
            None => format!("{} {} (built in)", d.name, d.version),
        }
    }
}

/// The one status snapshot the UI polls (the status surface).
/// serde-serializable; everything the live UI needs in one struct.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// The run state.
    pub state: RunState,
    /// The drive path (the UI greys features off its capability table).
    pub path: DrivePath,
    /// The currently commanded level, dB.
    pub current_level_db: f64,
    /// The current ladder table index.
    pub level_index: usize,
    /// Pulses fired at the current level.
    pub pulses_at_level: u32,
    /// Whether the AUTO ladder has completed.
    pub ladder_completed: bool,
    /// Total pulses fired this run (measured or not).
    pub pulses_fired: u64,
    /// Pulses with a measured record in the store.
    pub pulses_recorded: usize,
    /// The last measured pulse's overall tolerance verdict.
    pub last_tolerance_pass: Option<bool>,
    /// The last measured pulse's peak, g.
    pub last_peak_g: Option<f64>,
    /// The last measured pulse's dV, in/s.
    pub last_velocity_change_ips: Option<f64>,
    /// Whether the tolerance alarm indicator is set (alarm-only posture).
    pub alarm_active: bool,
    /// Whether an abort has fired (the run is Aborted).
    pub abort_active: bool,
    /// Whether a capture sink is running.
    pub capturing: bool,
    /// The between-pulse drive-limit scale carried into the next pulse (1.0 =
    /// none).
    pub next_pulse_scale: f64,
    /// The run clock, seconds since arm (advanced by the realized inter-pulse
    /// spacing).
    pub run_clock_s: f64,
    /// The sample rate the PROFILE asked for, Hz (`ClassicalTestConfig::fs_hz`).
    /// A request, never an authority - see [`Status::actual_fs_hz`].
    pub requested_fs_hz: f64,
    /// The sample rate the DEVICE actually programmed, Hz, adopted at arm (the
    /// device owns the rate). `None` before arm, because until the device has been
    /// configured there is no answer - and the UI must show the difference
    /// rather than one number that could be either.
    pub actual_fs_hz: Option<f64>,
    /// Seconds until the next pulse may fire, as of this snapshot: the rest of
    /// the time between pulses. `None` when no pulse has fired yet (the first
    /// does not wait) or the run cannot fire.
    #[serde(default)]
    pub next_pulse_s: Option<f64>,
    /// Pulses that passed their tolerance check.
    #[serde(default)]
    pub pulses_passed: u64,
    /// The last pulse's drive peak, volts.
    #[serde(default)]
    pub last_drive_vpk: Option<f64>,
    /// The last pulse's RMS error, percent.
    #[serde(default)]
    pub last_error_pct: Option<f64>,
    /// The last pulse's envelope verdict.
    #[serde(default)]
    pub last_envelope_pass: Option<bool>,
    /// The last pulse's velocity-change verdict.
    #[serde(default)]
    pub last_velocity_change_pass: Option<bool>,
    /// The last pulse's substitution verdict.
    #[serde(default)]
    pub last_substitution_pass: Option<bool>,
    /// What identification found (closed loop, after identification).
    #[serde(default)]
    pub identification: Option<IdentificationSummary>,
    /// The predicted full-level drive and response (closed loop, from proceed).
    #[serde(default)]
    pub prediction: Option<DrivePrediction>,
    /// The control law and its calls (closed loop, from proceed).
    #[serde(default)]
    pub law: Option<LawStatus>,
    /// Why identification held the run (state [`RunState::Held`]).
    #[serde(default)]
    pub held: Option<HeldReason>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_state_is_running_matches_the_firing_states() {
        assert!(RunState::Armed.is_running());
        assert!(RunState::Firing.is_running());
        assert!(!RunState::Idle.is_running());
        assert!(!RunState::Feasibility.is_running());
        assert!(!RunState::FeasibilityBlocked.is_running());
        assert!(!RunState::Stopped.is_running());
        assert!(!RunState::Aborted.is_running());
    }

    #[test]
    fn the_closed_loop_states_are_not_running_and_their_ends_are_terminal() {
        for s in [RunState::SysId, RunState::AwaitingReview] {
            assert!(!s.is_running() && !s.is_terminal(), "{s:?}");
        }
        for s in [
            RunState::Stopped,
            RunState::Aborted,
            RunState::Held,
            RunState::NotConverged,
        ] {
            assert!(s.is_terminal(), "{s:?}");
        }
        assert!(!RunState::Firing.is_terminal());
    }

    #[test]
    fn trip_kinds_have_stable_names_and_the_predicted_flag() {
        use hertzforge_shock_common::law_guard::OutputRule;
        let cases = [
            (
                TripKind::Overshoot {
                    pulse: 0,
                    peak_ratio: 1.58,
                    limit_ratio: 1.3,
                },
                "overshoot",
                false,
            ),
            (
                TripKind::DriveVpk {
                    pulse: 0,
                    vpk: 0.4,
                    abort_vpk: 0.38,
                    predicted: true,
                },
                "drive_vpk",
                true,
            ),
            (
                TripKind::Kinematic {
                    quantity: "pk-pk displacement (in)".into(),
                    value: 1.0,
                    limit: 0.9,
                    pulse: 4,
                    predicted: true,
                },
                "kinematic",
                true,
            ),
            (
                TripKind::Divergence {
                    pulse: 3,
                    pulses: 3,
                    error_pct: 40.0,
                },
                "divergence",
                false,
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
                    fault: LawFault::BadOutput(OutputRule::FirstDrive),
                    consecutive: 1,
                },
                "law_fault",
                false,
            ),
            (
                TripKind::PreFireRefused { reason: "x".into() },
                "accel",
                false,
            ),
        ];
        for (k, name, predicted) in cases {
            assert_eq!(k.name(), name);
            assert_eq!(k.predicted(), predicted, "{name}");
            let back: TripKind = serde_json::from_str(&serde_json::to_string(&k).unwrap()).unwrap();
            assert_eq!(back, k);
        }
        // A kinematic trip written before `predicted` existed reads as measured.
        let old = r#"{"Kinematic":{"quantity":"q","value":1.0,"limit":0.5,"pulse":2}}"#;
        assert!(!serde_json::from_str::<TripKind>(old).unwrap().predicted());
        assert_eq!(ConvergenceLimit::Run.as_str(), "run");
        assert_eq!(
            HeldReason::ReferenceOutOfBand {
                coverage: 0.5,
                min: 0.9
            }
            .as_str(),
            "reference_out_of_band"
        );
    }

    #[test]
    fn event_log_is_ordered_and_timestamped() {
        let mut log = EventLog::default();
        log.push(EventKind::StateChange {
            from: RunState::Idle,
            to: RunState::Feasibility,
        });
        log.push(EventKind::Note { text: "hi".into() });
        let events = log.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].seq, 0);
        assert_eq!(events[1].seq, 1);
    }

    #[test]
    fn honesty_carries_the_finding_markers_for_both_paths() {
        let b = Honesty::for_path(DrivePath::HostDac);
        assert!(b.path.contains("Path B"));
        // The trains caveat is path-aware (2026-08-08): on the HOST path it says the
        // block does NOT apply, and saying so is the point - a blanket
        // "HARDWARE-BLOCKED" here described the delegated device.
        assert!(!b.trains.contains("HARDWARE-BLOCKED"));
        assert!(b.trains.contains("F8"));
        assert!(b.trains.contains("DELEGATED"));
        assert!(b.rate_model.contains("PLACEHOLDER"));
        assert!(b.feasibility.contains("crest"));
        assert!(b.tolerance.contains("FORMAL"));

        let a = Honesty::for_path(DrivePath::Delegated);
        assert!(a.trains.contains("HARDWARE-BLOCKED"));
        assert!(a.path.contains("UNCOMPENSATED"));
        assert!(a.path.contains("F1"));
        assert!(a.delegated_capture.contains("not observable"));
    }

    #[test]
    fn status_round_trips_through_json() {
        let s = Status {
            state: RunState::Firing,
            path: DrivePath::HostDac,
            current_level_db: -6.0,
            level_index: 2,
            pulses_at_level: 1,
            ladder_completed: false,
            pulses_fired: 3,
            pulses_recorded: 3,
            last_tolerance_pass: Some(true),
            last_peak_g: Some(10.02),
            last_velocity_change_ips: Some(27.1),
            alarm_active: false,
            abort_active: false,
            capturing: true,
            next_pulse_scale: 1.0,
            run_clock_s: 3.0,
            requested_fs_hz: 5120.0,
            actual_fs_hz: Some(51_200.0),
            next_pulse_s: Some(0.6),
            pulses_passed: 3,
            last_drive_vpk: Some(0.283),
            last_error_pct: Some(1.9),
            last_envelope_pass: Some(true),
            last_velocity_change_pass: Some(true),
            last_substitution_pass: Some(true),
            identification: None,
            prediction: None,
            law: None,
            held: Some(HeldReason::LowCoherence {
                mean: 0.5,
                floor: 0.8,
            }),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Status = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
