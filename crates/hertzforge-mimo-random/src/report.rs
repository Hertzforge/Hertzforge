//! States, events, trips, the live status and the end-of-run report data.
//!
//! The shapes follow the single-output module's `report` (an event log of every
//! state change, trip and note; a trip record with the level and loop it fired
//! at; a serde status and report), with the states, trips and readouts a
//! multiple-output run has.

use crate::guards::{OutputFault, OutputRepair};
use serde::{Deserialize, Serialize};

/// The run's state. `Idle -> Feasibility -> NoiseFloor -> SysId -> Gate -> Predict
/// -> Engaged -> FullLevel -> Stopped`, `Aborted` from any running state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MimoState {
    /// Nothing started.
    Idle,
    /// Feasibility assessed and the run held: a drive's shaker would be exceeded.
    /// `override_with` releases it.
    Feasibility,
    /// Armed; the noise floor (drives off) is next.
    NoiseFloor,
    /// The noise floor is measured; system identification is next.
    SysId,
    /// Identification done; the gates are on screen. `proceed` (or an override
    /// when a gate failed) goes on to the prediction.
    Gate,
    /// The law's first drive is predicted; `proceed` (or an override when a
    /// prediction breaches a ceiling) engages.
    Predict,
    /// The loop runs and the level schedule climbs.
    Engaged,
    /// At full level.
    FullLevel,
    /// Stopped cleanly (operator, duration, or a law fault).
    Stopped,
    /// A protection tripped; the drive went to zero at once.
    Aborted,
}

impl MimoState {
    /// True while something may be driving.
    pub fn is_running(self) -> bool {
        matches!(
            self,
            MimoState::NoiseFloor
                | MimoState::SysId
                | MimoState::Gate
                | MimoState::Predict
                | MimoState::Engaged
                | MimoState::FullLevel
        )
    }

    /// True once the run has ended.
    pub fn is_terminal(self) -> bool {
        matches!(self, MimoState::Stopped | MimoState::Aborted)
    }
}

/// Why the core did not use one law call's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MimoFaultReason {
    /// The law reported an error.
    Error,
    /// The law panicked; the core caught it.
    Panicked,
    /// The call took longer than its budget.
    OverBudget,
    /// The drive broke an output rule.
    BadOutput(OutputFault),
}

impl MimoFaultReason {
    /// In plain words, said of the law.
    pub fn words(self) -> &'static str {
        match self {
            MimoFaultReason::Error => "it reported an error",
            MimoFaultReason::Panicked => "it failed while computing",
            MimoFaultReason::OverBudget => "it took longer than its time budget",
            MimoFaultReason::BadOutput(f) => f.words(),
        }
    }
}

/// The protection that ended a run, or the law fault that stopped it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MimoTripKind {
    /// A control's PSD left its abort lines on too many lines.
    Envelope {
        /// Index in the control space.
        control: usize,
        abort_lines: usize,
        worst_db: f64,
    },
    /// The core's own judgement: a control's in-band error grew loop after loop.
    Divergence {
        control: usize,
        consecutive_loops: u32,
        error_db: f64,
    },
    /// A tri-field kinematic abort on an input channel.
    Kinematic {
        channel_index: u32,
        axis: String,
        value: f64,
        limit: f64,
    },
    /// A drive's realized rms reached its abort level.
    DriveVrms {
        /// Index among the drives.
        drive: usize,
        channel_index: u32,
        realized_vrms: f64,
        limit_vrms: f64,
    },
    /// A limit channel's measured PSD went past its ceiling plus the abort margin.
    Limit {
        channel_index: u32,
        line: usize,
        over_db: f64,
    },
    /// The law stopped producing a usable drive; the run took its controlled
    /// stop (not an abort: the drive in force was sound).
    LawFault {
        reason: MimoFaultReason,
        consecutive: u32,
    },
    /// The plant loop faulted (a stalled input, a lost link, an underrun).
    Plant { message: String },
}

impl MimoTripKind {
    /// The trip in a sentence for the operator.
    pub fn words(&self) -> String {
        match self {
            MimoTripKind::Envelope {
                control,
                abort_lines,
                worst_db,
            } => format!(
                "Control {control} was outside its abort lines on {abort_lines} lines (worst \
                 {worst_db:+.1} dB)."
            ),
            MimoTripKind::Divergence {
                control,
                consecutive_loops,
                error_db,
            } => format!(
                "Control {control}'s error grew for {consecutive_loops} loops in a row, to \
                 {error_db:.1} dB."
            ),
            MimoTripKind::Kinematic {
                channel_index,
                axis,
                value,
                limit,
            } => format!(
                "Kinematic abort on channel {channel_index}: {axis} {value:.3} against {limit:.3}."
            ),
            MimoTripKind::DriveVrms {
                drive,
                channel_index,
                realized_vrms,
                limit_vrms,
            } => format!(
                "Drive {drive} (channel {channel_index}) reached {realized_vrms:.3} V rms against \
                 its {limit_vrms:.3} V abort."
            ),
            MimoTripKind::Limit {
                channel_index,
                line,
                over_db,
            } => format!(
                "Limit channel {channel_index} was {over_db:.1} dB over its ceiling on line {line}."
            ),
            MimoTripKind::LawFault {
                reason,
                consecutive,
            } => format!(
                "The control law did not produce a usable drive for {consecutive} loops in a row \
                 ({}); the run stopped.",
                reason.words()
            ),
            MimoTripKind::Plant { message } => format!("The plant loop faulted: {message}."),
        }
    }
}

/// A trip with where it happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MimoTripRecord {
    pub kind: MimoTripKind,
    pub at_level_db: f64,
    pub at_loop: u64,
}

/// One event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MimoEventKind {
    StateChange {
        from: MimoState,
        to: MimoState,
    },
    LevelChange {
        from_db: f64,
        to_db: f64,
        auto: bool,
    },
    Override {
        what: String,
        ack: String,
    },
    EnvelopeAlarm {
        control: usize,
        lines: usize,
        worst_db: f64,
    },
    OverloadLatch {
        channel_index: u32,
    },
    LawMiss {
        reason: MimoFaultReason,
        detail: String,
    },
    Trip {
        record: MimoTripRecord,
    },
    Record {
        path: String,
    },
    Note {
        text: String,
    },
}

/// A timestamped event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MimoEvent {
    pub seq: u64,
    pub unix_secs: u64,
    pub kind: MimoEventKind,
}

/// The run's event log.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MimoEventLog {
    events: Vec<MimoEvent>,
}

impl MimoEventLog {
    pub fn push(&mut self, kind: MimoEventKind) {
        let unix_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let seq = self.events.len() as u64;
        self.events.push(MimoEvent {
            seq,
            unix_secs,
            kind,
        });
    }

    pub fn events(&self) -> &[MimoEvent] {
        &self.events
    }
}

/// One drive's feasibility verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriveFeasibilityRow {
    pub drive: usize,
    pub channel_index: u32,
    /// `Pass`, `Advisory`, `Exceeded` or `NotEvaluated`.
    pub verdict: String,
    pub detail: String,
}

/// The identification gates (design 2 step 5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateReport {
    /// Each control's mean multiple coherence over the band (control space).
    pub coherence: Vec<f64>,
    pub coherence_threshold: f64,
    /// Controls below the threshold.
    pub low_coherence: Vec<usize>,
    /// Lines whose condition number is above the threshold.
    pub ill_conditioned_lines: Vec<usize>,
    pub condition_threshold: f64,
    /// Largest condition number in the band.
    pub worst_condition: f64,
    pub passed: bool,
}

/// The prediction before engagement (design 2 step 6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prediction {
    /// Predicted rms per physical drive at full level, volts.
    pub drive_rms: Vec<f64>,
    /// Each drive's ceiling, volts rms.
    pub ceilings: Vec<f64>,
    /// Drives whose prediction is over the ceiling.
    pub breaches: Vec<usize>,
    /// Predicted in-band rms error per control (control space), dB.
    pub error_db: Vec<f64>,
    /// Times the prediction was iterated (0: the law's seed alone).
    pub iterations: u32,
    /// True when there are more controls than drives, so the achievable reference
    /// `P G_ref P^H` is shown.
    pub has_achievable: bool,
    /// In-band rms difference between the reference and its achievable part, dB,
    /// per control (0 for a square plant).
    pub unreachable_db: Vec<f64>,
}

/// The law's identity line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LawLine {
    pub id: String,
    pub name: String,
    pub version: String,
    /// The law file's SHA-256; empty for a built-in law.
    pub sha256: String,
    /// `key=value` per parameter.
    pub parameters: Vec<String>,
}

/// Milliseconds one loop spent in each stage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct LoopTiming {
    pub estimate_ms: f64,
    pub law_ms: f64,
    pub guards_ms: f64,
    pub synth_ms: f64,
    pub plant_ms: f64,
}

/// What the guards did on the last loop.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GuardSummary {
    pub repair: OutputRepair,
    /// Drive-lines held by the change cap.
    pub capped: usize,
    /// Drive-lines scaled by a voltage ceiling.
    pub ceiling_bound: usize,
    /// Lines scaled down by response limiting.
    pub limited_lines: usize,
    /// Largest change a drive's PSD asked for, dB.
    pub peak_change_db: f64,
}

/// Counts the law guards keep over the run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct LawStats {
    pub calls: u64,
    pub misses: u64,
    pub capped_drive_lines: u64,
    pub repaired_lines: u64,
}

/// The live status (design 10, `mimo_random.status`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MimoStatus {
    pub state: MimoState,
    pub level_db: f64,
    pub target_level_db: f64,
    pub loop_index: u64,
    /// In-band rms error per control (control space), dB; empty before the loop.
    pub error_db: Vec<f64>,
    pub gate: Option<GateReport>,
    pub prediction: Option<Prediction>,
    pub guards: GuardSummary,
    pub law: Option<LawLine>,
    pub law_stats: LawStats,
    pub faults: Vec<String>,
    pub trip: Option<MimoTripRecord>,
    pub timing: LoopTiming,
    /// Each drive's realized rms on the last loop, volts.
    pub drive_rms: Vec<f64>,
    /// Inputs whose overload latched, by channel index.
    pub overloads: Vec<u32>,
}

/// The identification summary in the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentSummary {
    /// The rms each drive played, volts.
    pub drive_vrms: Vec<f64>,
    /// The controls' in-band rms relative to the reference, dB.
    pub level_db: f64,
    pub frames: usize,
    pub steps: usize,
    /// The setup hash the set is saved under.
    pub setup_hash: String,
    /// True when the set was loaded from an earlier run rather than measured.
    pub loaded: bool,
}

/// The report data at the end of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MimoReport {
    pub law: Option<LawLine>,
    pub identification: Option<IdentSummary>,
    pub gate: Option<GateReport>,
    pub prediction: Option<Prediction>,
    pub feasibility: Vec<DriveFeasibilityRow>,
    /// Final in-band rms error per control (control space), dB.
    pub final_error_db: Vec<f64>,
    /// Final worst in-band line error per control, dB.
    pub final_worst_db: Vec<f64>,
    pub loops: u64,
    pub loops_at_full_level: u64,
    pub trip: Option<MimoTripRecord>,
    pub law_stats: LawStats,
    pub final_state: MimoState,
    pub seed: u64,
    pub events: Vec<MimoEvent>,
}
