//! Event log, run-state machine, trip record, honesty section, and the UI
//! status + report surfaces for the Sine run service.
//!
//! Mirrors `eigen-random`'s `service::report` conventions (a timestamped event
//! log, a serde-serializable [`ReportData`], a live [`SineStatus`], and an
//! honesty section carried verbatim into every report), transposed to the sine
//! world: a swept scalar at one frequency instead of a per-bin PSD.

use crate::control::CompositeMode;
use eigen_shaker_db::FeasibilityReport;
use serde::{Deserialize, Serialize};

/// The sine run-state machine. Mirrors Random's shape:
/// `Idle -> Feasibility -> SysId -> CoherenceGate -> Engaged -> FullLevel ->
/// Stopped`, plus `Aborted` from any running state, a `CoherenceHold` park
/// (report-and-hold, never auto-raises), and a `FeasibilityBlocked` hard block
/// (the override path).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState {
    /// No run started.
    Idle,
    /// Pre-test feasibility assessed. If Exceeded, parks at
    /// [`RunState::FeasibilityBlocked`] until an explicit override arms.
    Feasibility,
    /// Feasibility Exceeded: arming is HARD-BLOCKED until `arm_with_override`.
    FeasibilityBlocked,
    /// System-ID at the sysID level: the broadband-burst H + coherence estimate.
    SysId,
    /// Coherence gate: band coherence checked against the floor.
    CoherenceGate,
    /// Coherence gate HELD (below floor): parked, never auto-raises.
    ///
    /// A gate FAILURE. There is no release verb: only `SineState::reset` (which
    /// rebuilds the service) clears it. See [`RunState::AwaitingReview`] for the
    /// park that IS releasable, and why conflating the two would ship a one-click
    /// bypass of a safety refusal.
    CoherenceHold,
    /// Coherence gate PASSED and the run is holding for an operator (config
    /// `review_hold`, default ON): the sysID |H| and coherence are published, the
    /// step is NOT built, and nothing is driven until
    /// `SineService::proceed_after_review`.
    ///
    /// The releasable park. Distinct from [`RunState::CoherenceHold`] in the one
    /// way that matters: this one is reached by a gate that PASSED.
    AwaitingReview,
    /// The loop is engaged and progressing through the level ladder.
    Engaged,
    /// At full level, holding for the mode's completion target.
    FullLevel,
    /// Cleanly stopped (target met or operator stop).
    Stopped,
    /// A protection engine tripped; stopped immediately with a trip record.
    Aborted,
}

impl RunState {
    /// True while the loop is actively closing (a protection abort can fire).
    pub fn is_running(self) -> bool {
        matches!(
            self,
            RunState::SysId | RunState::CoherenceGate | RunState::Engaged | RunState::FullLevel
        )
    }
}

/// A serde-friendly copy of the composite mode for the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompositeModeRecord {
    WeightedAvg,
    Min,
    Max,
}

impl From<CompositeMode> for CompositeModeRecord {
    fn from(m: CompositeMode) -> Self {
        match m {
            CompositeMode::WeightedAvg => CompositeModeRecord::WeightedAvg,
            CompositeMode::Min => CompositeModeRecord::Min,
            CompositeMode::Max => CompositeModeRecord::Max,
        }
    }
}

/// Which axis / kind of protection tripped the run. Mirrors
/// `eigen_engine::AbortTrip`'s field set for the reactive axes (channel / axis /
/// value / limit) and adds the sine-specific kinds (scalar alarm-band abort at
/// the current frequency, drive-Vpk abort, debounced servo fault). Every trip
/// carries the frequency it fired at (the runtime check is scalar-at-f).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TripKind {
    /// The scalar alarm/abort band abort: the composite tracked amplitude
    /// left the profile's abort band at the current frequency.
    ScalarAbort {
        /// `"high"` or `"low"` band side.
        side: String,
        measured_g: f64,
        limit_g: f64,
    },
    /// The analytic kinematic monitor abort: the tracked response's
    /// analytic peak velocity or pk-pk displacement exceeded the shaker rating.
    Kinematic {
        /// `"velocity"` or `"displacement"`.
        axis: String,
        value: f64,
        limit: f64,
    },
    /// A limit-channel abort: a limit channel's own abort band was breached.
    LimitChannel {
        channel_index: u32,
        measured: f64,
        limit: f64,
    },
    /// The drive-Vpk ceiling abort.
    DriveVpk { realized_vpk: f64, limit_vpk: f64 },
    /// The debounced servo-fault abort: the control channel dropped
    /// out (measured composite << target) for `consecutive_ticks` control ticks.
    ServoFault { consecutive_ticks: u32 },
    /// The raw-ring accel watchdog abort: the engine's `AbortWatchdog` fired
    /// on the RAW input ring at raw-sample temporal resolution (a sub-frame peak
    /// above the conservative constant profile+abort accel ceiling). This is the
    /// coarse fast backstop that the demodulated scalar abort (`ScalarAbort`)
    /// cannot catch at control-tick cadence.
    WatchdogAccel {
        channel_index: u32,
        /// The tri-field axis that tripped (`"accel"` for the armed sine path).
        axis: String,
        value: f64,
        limit: f64,
        /// The RUN FRAME (1-based) the sustained breach completed on, straight
        /// from `AbortTrip.at_frame`.
        ///
        /// This is the only field that says WHERE in the run the
        /// breach was at raw-sample resolution; the surrounding
        /// `TripRecord::at_freq_hz` / `at_level_db` are now resolved FROM it
        /// (see `SineProcessStep::context_at_frame`) instead of being re-read
        /// after the batch unwound. Carrying it also lets a reader re-derive
        /// those two independently from a capture file.
        at_frame: u64,
        /// The tracked COMPOSITE (EU pk) the servo was acting on at `at_frame`.
        ///
        /// The whole point of the raw-ring watchdog is that it never reads the
        /// demodulated path, so this is the number it DISAGREED with. A large
        /// gap between `measured` and `value` is the signature of the tracking-lag
        /// overdrive: the demodulator under-reading a fast sweep while the true
        /// response climbs toward the ceiling.
        measured: f64,
        /// The committed drive scale at `at_frame` - what the servo had wound
        /// the drive up to while reading `measured`.
        drive_scale: f64,
    },
    /// The oscillator REFUSED a frequency the scheduler commanded: out of
    /// `[0, fs/2)` at the ADOPTED rate.
    ///
    /// A run should never reach this - the scheduler's band clamp is narrowed
    /// against `0.45 x fs` at adoption, so an ordinary sweep cannot walk past it.
    /// Reaching it means that clamp failed, and the correct answer is a typed
    /// abort with the two numbers in it, not a panic on the RT thread (which
    /// would leave the AO clocked and the device holding its last sample) and not
    /// a silent hold (which would leave the servo chasing a target the excitation
    /// is no longer at).
    OscillatorRange { requested_hz: f64, fs_hz: f64 },
}

/// A record of the trip that aborted the run: the kind, the frequency it fired
/// at, the level (dB), and the loop/tick index. Mirrors Random's
/// `TripRecord` plus the sine `at_freq_hz`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TripRecord {
    pub kind: TripKind,
    /// The frequency (Hz) excited when the trip fired.
    pub at_freq_hz: f64,
    /// The level (dB) the run was at when it tripped.
    pub at_level_db: f64,
    /// The control-tick index at which the trip fired.
    pub at_tick: u64,
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
    /// Feasibility assessed with its overall verdict text.
    Feasibility {
        overall: String,
        exceeded: bool,
        has_advisory: bool,
    },
    /// An override acknowledgment was logged to arm past a feasibility
    /// exceedance.
    OverrideAck { ack: String },
    /// The coherence gate result.
    CoherenceGate {
        band_coherence: f64,
        floor: f64,
        passed: bool,
    },
    /// A level target changed (the ladder advanced, or an operator step / jump).
    LevelChange {
        from_db: f64,
        to_db: f64,
        auto: bool,
    },
    /// A scalar alarm breached at the current frequency (surfaced, not stopped).
    ScalarAlarm {
        side: String,
        freq_hz: f64,
        measured_g: f64,
        limit_g: f64,
    },
    /// The RSTD search found a resonance.
    RstdSearchDone { f_peak: f64, q: f64 },
    /// The RSTD phase setpoint was captured.
    RstdSetpointCaptured { phase_rad: f64 },
    /// The RSTD phase tracker exited the band (alarm + hold).
    RstdBandExit,
    /// A protection trip aborted the run.
    Trip { record: TripRecord },
    /// Capture started.
    CaptureStart { run_dir: String, trigger: String },
    /// Capture stopped, with the drop count.
    CaptureStop { chunks_written: u64, dropped: u64 },
    /// A snapshot was saved.
    Snapshot { path: String },
    /// COERCION WARNING: the device programmed a rate other than the one asked
    /// for, and the run adopted the device's answer.
    ///
    /// Typed rather than a `Note` because a panel and a wire status both have to
    /// find it without reading prose, and because this is the event class that
    /// put every Random profile frequency 6.25x high on a shaker.
    /// Both numbers ride the event: an "actual" with no "requested" beside it
    /// cannot show a coercion at all.
    RateCoerced { requested_hz: f64, actual_hz: f64 },
    /// The run band was narrowed against the ADOPTED rate's Nyquist limit. The
    /// sweep will turn around at `to_hz`, not at the configured edge.
    BandClamped {
        from_hz: f64,
        to_hz: f64,
        reason: String,
    },
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

/// The honest limitations carried verbatim into every Sine report (Path-B / Sim
/// posture + the open-loop notes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Honesty {
    /// Which hardware path this run is on and what that means.
    pub path: String,
    /// The drive-abort honesty (real on Path B / Sim; estimate on the
    /// delegated path).
    pub drive_abort: String,
    /// The delegated-path note (no Eigen closed-loop sine control on Path A).
    pub delegated: String,
    /// The kinematic / open-loop note: the analytic monitor is exact
    /// for a sine; the debounced servo fault is a host-side open-loop detector.
    pub kinematic: String,
}

impl Default for Honesty {
    fn default() -> Self {
        Self {
            path: "This run is Path B (host-closes-loop) on the eigen-hal-sim synthetic plant. \
                 Eigen's own closed-loop sine engine (SineProcessStep on the real Engine) closes \
                 the loop; the Sim is the plant today. Real-hardware Path B homes are \
                 direct-DAC hardware and NI hardware, both gated on separate workstreams \
                 (SINE_BUILD_STATUS.md)."
                .into(),
            drive_abort:
                "The drive-Vpk ceiling clamp and drive-Vpk abort are REAL on Path B (the host \
                 synthesizes the drive and knows its commanded value). On the delegated controller \
                 path the device owns the drive, so the drive value would be a host ESTIMATE and the \
                 abort estimate-based."
                    .into(),
            delegated:
                "The delegated controller sine path carries NO Eigen closed-loop control: the device \
                 runs a fully autonomous on-device sweep from a downloaded config image. None of \
                 the closed-loop features (blend law, notch governor, tracking-filter servo) run \
                 on that path. Path A is not buildable this slice (SINE_BUILD_STATUS.md)."
                    .into(),
            kinematic:
                "The velocity / displacement kinematic abort is ANALYTIC and exact for a sine \
                 (v = a*G/(2*pi*f), d_pp = 2*a*G/(2*pi*f)^2), replacing the engine watchdog's \
                 HP-integrator vel/disp axes (which reset per chunk and under-read at low f); the \
                 engine watchdog keeps the ACCEL axis. That raw-ring accel watchdog is a COARSE \
                 FAST BACKSTOP, not the fine accel abort: it is armed at worst-case-profile- \
                 abort-high times the max ladder rung times 4x headroom, so at low rungs it sits \
                 well above the commanded level (dormant relative to the level actually driven) \
                 and even at full level fires only about 4x above the profile-plus-abort ceiling. \
                 It catches a sub-frame raw peak the demodulated path cannot see; it does NOT \
                 replace the FINE per-frequency, per-rung acceleration abort, which is the \
                 demodulated scalar-abort path's job. The servo-fault abort is a HOST-SIDE \
                 open-loop detector (composite response collapsed vs target), debounced over N \
                 control ticks (audit F9b / F14)."
                    .into(),
        }
    }
}

/// The run settings recorded in the report: a serde-serializable digest of the
/// config that matters for the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSettings {
    /// The run mode as a short label (`"sweep"`, `"dwell"`, `"step"`, `"rstd"`).
    pub mode: String,
    /// The band `(lo, hi)` Hz.
    pub band_hz: (f64, f64),
    pub control_indices: Vec<u32>,
    pub control_weights: Vec<f64>,
    pub composite_mode: CompositeModeRecord,
    /// The profile global alarm / abort bands, `(minus_db, plus_db)`.
    pub global_alarm_db: (f64, f64),
    pub global_abort_db: (f64, f64),
    pub blend_w: f64,
    pub sysid_level_db: f64,
    pub coherence_floor: f64,
    pub start_db: f64,
    pub table_db: Vec<f64>,
    pub drive_vpk_ceiling: f64,
    pub drive_vpk_abort: f64,
    pub fault_debounce_ticks: u32,
    pub kinematic_monitor: bool,
    pub cola_enabled: bool,
}

/// One channel's row in the report's channel-table digest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelRow {
    pub index: u32,
    pub role: String,
    pub units: String,
    pub is_control: bool,
    pub control_weight: Option<f64>,
    pub has_limit_profile: bool,
}

/// The feasibility digest carried in the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeasibilityDigest {
    /// The full feasibility report (items + overall + assumptions).
    pub report: FeasibilityReport,
    /// The shaker name assessed against (or a note if none configured).
    pub shaker_name: String,
    /// The override acknowledgment logged to arm past an exceedance, if any.
    pub override_ack: Option<String>,
}

/// One level-history entry: the level dB and how it was entered.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LevelHistoryEntry {
    pub level_db: f64,
    pub auto: bool,
    pub at_tick: u64,
}

/// One point on a swept trace: the frequency and the value logged there (the
/// tracked-vs-f and broadband-vs-f trace families).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TracePoint {
    pub freq_hz: f64,
    pub value: f64,
}

/// The per-channel final traces for the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelTrace {
    pub channel_index: u32,
    /// TRACKED amplitude vs frequency (the "filtered" trace).
    pub tracked: Vec<TracePoint>,
    /// BROADBAND RMS vs frequency (the "unfiltered" trace).
    pub broadband: Vec<TracePoint>,
}

/// The FRF display data from sysID: the grid, `|H|`, and the per-bin trust
/// mask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrfDisplay {
    pub freqs: Vec<f64>,
    pub h_mag: Vec<f64>,
    pub coherence: Vec<f64>,
    pub trusted: Vec<bool>,
}

/// The serde-serializable report data the UI's report consumes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportData {
    /// The run settings digest.
    pub settings: RunSettings,
    /// The channel table digest.
    pub channels: Vec<ChannelRow>,
    /// The feasibility digest.
    pub feasibility: Option<FeasibilityDigest>,
    /// The level history.
    pub level_history: Vec<LevelHistoryEntry>,
    /// The full timestamped event log.
    pub event_log: EventLog,
    /// The per-channel final swept traces.
    pub channel_traces: Vec<ChannelTrace>,
    /// The sysID FRF display data.
    pub frf: Option<FrfDisplay>,
    /// The trip record if the run aborted.
    pub trip: Option<TripRecord>,
    /// The final band-coherence value from the sysID / gate.
    pub band_coherence: f64,
    /// The user-defined metadata.
    pub metadata: Vec<(String, String)>,
    /// The honest limitations.
    pub honesty: Honesty,
}

/// The one status snapshot the UI polls. serde-serializable; everything the live
/// UI needs in one struct.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SineStatus {
    /// The run state.
    pub state: RunState,
    /// The current excitation frequency (Hz).
    pub freq_hz: f64,
    /// Completed one-direction sweep passes (sweep mode); 0 otherwise.
    pub sweeps_done: u32,
    /// Sweep/dwell progress fraction `[0, 1]` (best-effort; `None` when N/A).
    pub progress: Option<f64>,
    /// The current level target in dB.
    pub level_db: f64,
    /// The target control amplitude at the current frequency and level (g pk).
    pub target_g: f64,
    /// The last-measured composite tracked amplitude (g pk).
    pub measured_g: Option<f64>,
    /// Per-control-channel tracked amplitude (g pk), in control-set order.
    pub per_channel_tracked: Vec<f64>,
    /// Per-control-channel broadband RMS (g), in control-set order.
    pub per_channel_broadband: Vec<f64>,
    /// Total harmonic distortion on the control channel (fraction), if measured.
    pub thd: Option<f64>,
    /// The sysID band coherence (feature: coherence display).
    pub band_coherence: f64,
    /// Whether the notch governor is currently binding.
    pub notch_binding: bool,
    /// Whether a scalar alarm indicator is currently set.
    pub alarm_active: bool,
    /// Whether an abort has fired (the run is Aborted).
    pub abort_active: bool,
    /// Whether a capture is currently running.
    pub capturing: bool,
    /// Test time elapsed (s) since engage (holds excluded).
    pub elapsed_test_s: f64,
    /// Test time remaining (s), or `None` when not determinable.
    pub remaining_test_s: Option<f64>,
    /// Per-control-channel overload latch (index -> latched).
    pub overload_latches: Vec<(u32, bool)>,
    /// The trip record if aborted.
    pub trip: Option<TripRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_state_is_running_matches_the_active_states() {
        assert!(RunState::SysId.is_running());
        assert!(RunState::CoherenceGate.is_running());
        assert!(RunState::Engaged.is_running());
        assert!(RunState::FullLevel.is_running());
        assert!(!RunState::Idle.is_running());
        assert!(!RunState::CoherenceHold.is_running());
        assert!(!RunState::Aborted.is_running());
        assert!(!RunState::Stopped.is_running());
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
    fn honesty_carries_the_path_b_and_kinematic_notes() {
        let h = Honesty::default();
        assert!(h.path.contains("Path B"));
        assert!(h.drive_abort.contains("ESTIMATE"));
        assert!(h.delegated.contains("NO Eigen closed-loop"));
        assert!(h.kinematic.contains("ANALYTIC"));
    }
}
