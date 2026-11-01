//! Event log, report data, honesty section, and the UI status surface for the Random
//! run service.
//!
//! Mirrors the Explorer service's `service::{Event, EventLog, ReportData, Honesty,
//! TripRecord}` conventions: a timestamped event log of every state change / trip /
//! capture / snapshot, a serde-serializable [`ReportData`] the UI's report consumes,
//! and an honesty section carried verbatim into every report.

use crate::control::{CompositeMode, HinvConfig};
use crate::service::config::{LimitKind, MetaField, RunMode};
use crate::service::engine::KinematicTrip;
use hertzforge_breakpoints::LineSpectrum;
use hertzforge_shaker_db::FeasibilityReport;
use serde::{Deserialize, Serialize};

/// The run state machine. `Idle -> Feasibility -> SysId -> CoherenceGate
/// -> Engaged -> FullLevel -> Stopped`, plus `Aborted` from any running state, plus a
/// `CoherenceHold` park (never auto-raises) and a `FeasibilityBlocked` hard block
/// (with its override path).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState {
    /// No run started.
    Idle,
    /// Pre-test feasibility assessed. If Exceeded, the state parks at
    /// [`RunState::FeasibilityBlocked`] until an explicit override arms.
    Feasibility,
    /// Feasibility Exceeded: arming is HARD-BLOCKED until `arm_with_override`.
    FeasibilityBlocked,
    /// System-ID at the sysID level: H + coherence estimate.
    SysId,
    /// Coherence gate: band coherence checked against the floor.
    CoherenceGate,
    /// Coherence gate HELD (below floor): parked, never auto-raises.
    CoherenceHold,
    /// System ID passed its gate and the run is HOLDING for operator review.
    /// Nothing drives in this state - the FRF and coherence are on screen and
    /// the operator decides whether to proceed.
    ///
    /// This is a deliberate stop-and-look, not a fault: the automatic safety
    /// checks have already run and passed (a failure parks at
    /// `FeasibilityBlocked` or `CoherenceHold` instead). The operator asked for
    /// it because the cost of being wrong is asymmetric - "billion dollar
    /// satellite they'll want to hold, review data. Production part same thing
    /// over and over, just let it ride" - so it is bypassable per test.
    AwaitingReview,
    /// The loop is engaged and progressing through the level ladder.
    Engaged,
    /// At full level, holding for the test duration.
    FullLevel,
    /// Cleanly stopped (duration elapsed or operator stop).
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

/// The kind of protection trip that aborted the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TripKind {
    /// The frequency-domain envelope abort.
    Envelope {
        /// Number of lines that breached the abort band.
        abort_lines: usize,
        /// Worst signed dB deviation among the abort breaches.
        worst_db: f64,
    },
    /// A tri-field kinematic abort on a control/monitor channel (the runtime side of
    /// feasibility; reactive on the raw channels).
    Kinematic {
        channel_index: u32,
        axis: String,
        value: f64,
        limit: f64,
        at_frame: u64,
    },
    /// The drive-Vrms ceiling abort.
    DriveVrms { realized_vrms: f64, limit_vrms: f64 },
    /// The control-loop divergence abort: divergent bins persisted for
    /// `divergence_loops` consecutive loops.
    Divergence {
        consecutive_loops: u32,
        divergent_bins: usize,
    },
}

/// A record of the trip that aborted the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TripRecord {
    pub kind: TripKind,
    /// The level (dB) the run was at when it tripped.
    pub at_level_db: f64,
    /// The loop index at which the trip fired.
    pub at_loop: u64,
}

impl From<KinematicTrip> for TripRecord {
    fn from(t: KinematicTrip) -> Self {
        TripRecord {
            kind: TripKind::Kinematic {
                channel_index: t.channel_index,
                axis: t.axis,
                value: t.value,
                limit: t.limit,
                at_frame: t.at_frame,
            },
            at_level_db: t.at_level_db,
            at_loop: t.at_loop,
        }
    }
}

/// A single event in the run log with a monotonic index + coarse wall-clock timestamp.
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
    /// An override acknowledgment was logged to arm past a feasibility exceedance.
    OverrideAck { ack: String },
    /// The station programmed a loop this service did not ask for, and the loop was
    /// re-derived to match it (first real-chassis Random arm, 2026-07-30).
    ///
    /// A rate that moved under a run must be answerable from the log alone: the
    /// realized `df` and control-line count are what every spectrum in the report is
    /// binned against, and they are no longer inferable from the requested settings.
    StationAdopted {
        station: String,
        /// The rate the loop asked the backend to configure.
        requested_fs_hz: f64,
        /// The rate the backend reported it actually programmed. The loop runs here.
        actual_fs_hz: f64,
        /// Realized bin width (`actual_fs_hz / nfft`).
        df_hz: f64,
        /// Realized in-band control lines at that resolution.
        control_lines: usize,
        /// The acquisition block the loop now describes.
        block_frames: usize,
        /// Whether the deterministic-harness scheduling relaxations were dropped
        /// because this is a hardware station.
        scheduling_guards_restored: bool,
    },
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
    /// An envelope ALARM breached (surfaced, not stopped).
    EnvelopeAlarm { lines: usize, worst_db: f64 },
    /// An overload proxy latched on a channel (not an abort).
    OverloadLatch { channel_index: u32 },
    /// A protection trip aborted the run.
    Trip { record: TripRecord },
    /// Capture started.
    CaptureStart { run_dir: String, trigger: String },
    /// Capture stopped, with the drop count.
    CaptureStop { chunks_written: u64, dropped: u64 },
    /// A snapshot was saved.
    Snapshot { path: String },
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

/// The honest limitations carried verbatim into every Random report (the drive-abort
/// basis, the delegated-path note, the overload proxy, and the two-paths posture).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Honesty {
    /// Which hardware path this run is on and what that means for the control law.
    pub path: String,
    /// The drive-abort honesty (real on Path B / Sim, estimate-only on the
    /// delegated path).
    pub drive_abort: String,
    /// The delegated-path note (no Hertzforge control law on Path A).
    pub delegated: String,
    /// The overload-proxy note (host-side proxy, not observed ADC
    /// saturation).
    pub overload_proxy: String,
}

impl Default for Honesty {
    fn default() -> Self {
        Self {
            path: "This run is Path B (host-closes-loop) on the hertzforge-hal-sim synthetic plant. \
                 Hertzforge's own control law (HinvLaw) closes the loop; the Sim is the plant today. \
                 Real-hardware Path B homes are direct-DAC hardware and NI hardware, both \
                 gated on separate workstreams."
                .into(),
            drive_abort:
                "The drive-Vrms ceiling clamp and drive-Vrms abort are REAL on Path B (the host \
                 synthesizes the drive and measures its realized Vrms). On the delegated \
                 controller path the device owns the drive, so the drive value is a host ESTIMATE and the \
                 abort would be estimate-based (finding L1)."
                    .into(),
            delegated:
                "The delegated controller path carries NO Hertzforge control law: the device closes the \
                 loop internally and Hertzforge downloads a reference + schedules level nudges. None \
                 of the control-law features (Hinv knobs, weighting, response/force limiting) \
                 run on that path (finding L2)."
                    .into(),
            overload_proxy:
                "The per-channel overload indicator is a HOST-SIDE PROXY (|eu| >= fraction * \
                 range_max), not observed ADC saturation, which is not visible on the delegated \
                 path (finding C3)."
                    .into(),
        }
    }
}

/// The run settings recorded in the report. A serde-serializable snapshot
/// of the config that matters for the report + sidecar (the full config also
/// serializes, but this is the report-facing digest).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSettings {
    pub df_hz: f64,
    pub lines: usize,
    pub control_indices: Vec<u32>,
    pub control_weights: Vec<f64>,
    pub composite_mode: CompositeModeRecord,
    pub alarm_db: (f64, f64),
    pub abort_db: (f64, f64),
    pub mode: RunMode,
    pub start_db: f64,
    pub table_db: Vec<f64>,
    pub sysid_level_db: f64,
    pub coherence_floor: f64,
    pub full_level_duration_s: f64,
    pub drive_ceiling_vrms: f64,
    pub drive_abort_vrms: Option<f64>,
    pub divergence_loops: u32,
    pub overload_fraction: f64,
    pub hinv: HinvConfig,
}

/// One channel's row in the report's channel table digest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelRow {
    pub index: u32,
    pub role: String,
    pub units: String,
    pub range_max: f64,
    pub abort_accel_g: Option<f64>,
    pub abort_velocity_ips: Option<f64>,
    pub abort_displacement_mil: Option<f64>,
}

/// A limit-profile digest for the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitProfileRow {
    pub channel_index: u32,
    pub kind: LimitKind,
    /// Number of lines with a finite ceiling.
    pub limited_lines: usize,
}

/// The feasibility digest carried in the report: the report + the
/// assumptions + any override acknowledgment.
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
    pub auto: bool,
    pub at_loop: u64,
}

/// The serde-serializable report data the UI's report consumes.
/// Everything the report needs, self-contained.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportData {
    /// The run settings digest.
    pub settings: RunSettings,
    /// The channel table digest.
    pub channels: Vec<ChannelRow>,
    /// The reference breakpoint table + its gRMS.
    pub reference_grms: f64,
    /// The limit profiles digest.
    pub limit_profiles: Vec<LimitProfileRow>,
    /// The feasibility digest.
    pub feasibility: Option<FeasibilityDigest>,
    /// The level history.
    pub level_history: Vec<LevelHistoryEntry>,
    /// The full timestamped event log.
    pub event_log: EventLog,
    /// The final composite measured PSD (last loop), on the line grid.
    pub final_composite_psd: Option<LineSpectrum>,
    /// The final per-channel measured PSDs (last loop), on the line grid.
    pub final_per_channel_psds: Vec<LineSpectrum>,
    /// The final drive PSD (last loop), on the line grid.
    pub final_drive_psd: Option<LineSpectrum>,
    /// The reference (target) line spectrum.
    pub target_psd: LineSpectrum,
    /// The number of notch-active bins at the last loop (notch state).
    pub final_notch_lines: usize,
    /// The last envelope report's alarm/abort line counts.
    pub final_alarm_lines: usize,
    pub final_abort_lines: usize,
    /// The per-channel overload latches (index -> latched).
    pub overload_latches: Vec<(u32, bool)>,
    /// The trip record if the run aborted.
    pub trip: Option<TripRecord>,
    /// The final band-coherence value from the sysID / gate.
    pub band_coherence: f64,
    /// The user-defined metadata.
    pub metadata: Vec<MetaField>,
    /// The honest limitations.
    pub honesty: Honesty,
}

/// The one status snapshot the UI polls.
/// serde-serializable; everything the live UI needs in one struct.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// The run state.
    pub state: RunState,
    /// The current level target in dB.
    pub current_level_db: f64,
    /// The target composite gRMS (at the current level).
    pub target_grms: f64,
    /// The last-measured composite gRMS.
    pub measured_grms: Option<f64>,
    /// Per-channel overload latches (index -> latched; a host-side proxy).
    pub overload_latches: Vec<(u32, bool)>,
    /// Whether an alarm indicator is currently set.
    pub alarm_active: bool,
    /// Whether an abort has fired (the run is Aborted).
    pub abort_active: bool,
    /// The notch active-bin count at the last loop.
    pub notch_active_lines: usize,
    /// The band-coherence value (coherence display; from sysID / gate).
    pub band_coherence: f64,
    /// Whether a capture is currently running.
    pub capturing: bool,
    /// Seconds elapsed at the current level.
    pub elapsed_at_level_s: f64,
    /// Seconds remaining at the current level (dwell or full duration), or `None` in
    /// MANUAL / non-timed states.
    pub remaining_at_level_s: Option<f64>,
    /// Who is advancing the level schedule RIGHT NOW (live-switchable
    /// mid-run, so the panel must render the service's truth, not its own last
    /// request).
    pub run_mode: RunMode,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_state_is_running_matches_the_active_states() {
        assert!(RunState::SysId.is_running());
        // A review hold is NOT running: nothing drives, so no protection can
        // trip and no tick may be taken. It reports as Armed on the wire.
        assert!(!RunState::AwaitingReview.is_running());
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
    fn honesty_carries_the_path_b_and_l1_l2_c3_notes() {
        let h = Honesty::default();
        assert!(h.path.contains("Path B"));
        assert!(h.drive_abort.contains("estimate"));
        assert!(h.delegated.contains("NO Hertzforge control law"));
        assert!(h.overload_proxy.contains("PROXY"));
    }

    #[test]
    fn status_round_trips_through_json() {
        let s = Status {
            state: RunState::FullLevel,
            current_level_db: 0.0,
            target_grms: 6.06,
            measured_grms: Some(6.0),
            overload_latches: vec![(1, false), (2, true)],
            alarm_active: false,
            abort_active: false,
            notch_active_lines: 12,
            band_coherence: 0.98,
            capturing: true,
            elapsed_at_level_s: 2.5,
            remaining_at_level_s: Some(7.5),
            run_mode: RunMode::Auto,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Status = serde_json::from_str(&json).unwrap();
        assert_eq!(back.state, RunState::FullLevel);
        assert_eq!(back.overload_latches, vec![(1, false), (2, true)]);
        assert_eq!(back.notch_active_lines, 12);
        assert_eq!(back.run_mode, RunMode::Auto);
    }

    #[test]
    fn composite_mode_record_maps_all_variants() {
        assert_eq!(
            CompositeModeRecord::from(CompositeMode::WeightedAvg),
            CompositeModeRecord::WeightedAvg
        );
        assert_eq!(
            CompositeModeRecord::from(CompositeMode::Min),
            CompositeModeRecord::Min
        );
        assert_eq!(
            CompositeModeRecord::from(CompositeMode::Max),
            CompositeModeRecord::Max
        );
    }
}
