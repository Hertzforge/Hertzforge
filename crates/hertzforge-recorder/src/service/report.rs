//! The Recorder service's live surfaces and its report data.
//!
//! * [`RecStatus`] - the one-poll live snapshot a front end reads.
//! * [`ReportData`] - the serde-serializable report: the settings, the rows, the
//!   recordings, the log and the run's facts in plain words ([`RunFacts`]).
//! * [`EventLog`] - a growing list of monotonic-session-second timestamped
//!   event strings (state changes, trigger fires, gaps, overloads, faults).
//! * [`ChannelMeter`] / [`FrfSnapshot`] / [`RecordingInfoLite`] - the small
//!   live/list payloads.

use serde::{Deserialize, Serialize};

use crate::analysis::TimeStats;
use crate::recording::GapRecord;

use super::config::{MetaField, RecState, RecorderConfig};

/// A serde-friendly copy of a channel's [`TimeStats`] for the report (the analysis
/// [`TimeStats`] is deliberately not serde; this mirrors its fields).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChannelStat {
    /// Enabled-input slot.
    pub ch: usize,
    pub mean: f64,
    pub rms: f64,
    pub peak: f64,
    pub pk_pk: f64,
    pub crest: f64,
    pub skew: f64,
    /// Vibration-convention kurtosis (Gaussian = 3).
    pub kurtosis: f64,
}

impl ChannelStat {
    /// Build from a [`TimeStats`] on an enabled-input slot.
    pub fn from_stats(ch: usize, s: &TimeStats) -> Self {
        Self {
            ch,
            mean: s.mean,
            rms: s.rms,
            peak: s.peak,
            pk_pk: s.pk_pk,
            crest: s.crest,
            skew: s.skew,
            kurtosis: s.kurtosis,
        }
    }
}

/// Per-channel live meter reading (RMS + peak over the most recent analysis
/// window).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChannelMeter {
    /// Enabled-input channel slot (0-based).
    pub ch: usize,
    /// True RMS over the last accumulated window.
    pub rms: f64,
    /// Peak absolute value over the last accumulated window.
    pub peak: f64,
}

/// A live FRF snapshot for the configured pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrfSnapshot {
    /// `|H1|` per bin (magnitude of the default estimator).
    pub h1_mag: Vec<f64>,
    /// The phase of H1 per bin, degrees, in (-180, 180].
    #[serde(default)]
    pub h1_phase_deg: Vec<f64>,
    /// Ordinary coherence per bin (only meaningful when `valid`).
    pub coherence: Vec<f64>,
    /// True once at least two averages have completed (the single-average
    /// trap); the coherence trace must be suppressed until then.
    pub valid: bool,
    /// Frequency resolution in Hz.
    pub df_hz: f64,
    /// Completed block-pair averages.
    pub n_averages: usize,
}

/// A lightweight recordings-list row (for the review picker).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingInfoLite {
    /// The run directory name.
    pub name: String,
    /// Frame count (from the FILE size, never a sidecar).
    pub frames: u64,
    /// Duration in seconds.
    pub duration_s: f64,
    /// Channels per frame.
    pub n_channels: usize,
    /// Human status: `"clean"`, `"unclean"`, `"recovered (unclean)"`.
    pub status: String,
    /// True only when nothing dropped and no discontinuities.
    pub gap_free: bool,
}

/// The one live status snapshot the UI polls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecStatus {
    /// The run state.
    pub state: RecState,
    /// Station seconds since start: the frames read over the programmed rate.
    pub monitoring_seconds: f64,
    /// The active recording's run name (when Recording).
    pub recording_run: Option<String>,
    /// Frames written to the active recording so far.
    pub recording_frames: u64,
    /// Whether the active recording is gap-free so far (0 drops).
    pub gap_free_so_far: bool,
    /// Seconds of native-rate recording left in the disk budget, given the
    /// caller-supplied free-space figure.
    pub disk_budget_seconds: u64,
    /// Per-channel overload latches (enabled-input slot -> latched).
    pub overload_latches: Vec<(usize, bool)>,
    /// Whether the trigger engine is armed and waiting.
    pub trigger_armed: bool,
    /// Automatic re-arms remaining after the current fire.
    pub rearms_remaining: u32,
    /// Analysis averages accumulated / whether the configured average is
    /// complete (the first enabled channel's pipeline).
    pub n_averages: usize,
    /// Whether the analysis average is complete.
    pub averages_complete: bool,
    /// Whether the live FRF coherence is valid (>= 2 averages).
    pub coherence_valid: bool,
    /// The marks made so far (limit crossings and overload latches).
    #[serde(default)]
    pub marks: u64,
    /// The rate asked of the station, Hz.
    #[serde(default)]
    pub requested_fs_hz: f64,
    /// The rate the station programmed once it is up (the rate asked until then), Hz.
    #[serde(default)]
    pub fs_hz: f64,
    /// Frames the station has delivered since start (station frames).
    #[serde(default)]
    pub station_frames: u64,
    /// Batches read since start.
    #[serde(default)]
    pub batches: u64,
    /// The analysis pass's last time, ms (null before the first pass).
    #[serde(default)]
    pub pass_ms_last: Option<f64>,
    /// The analysis pass's worst time, ms (null before the first pass).
    #[serde(default)]
    pub pass_ms_worst: Option<f64>,
    /// The longest gap between the end of one batch's read and the start of the
    /// next, ms (null until two batches have run).
    #[serde(default)]
    pub batch_gap_ms_max: Option<f64>,
    /// Trigger fires since start, every mode (held: never reset by a later pass).
    #[serde(default)]
    pub trigger_fires: u64,
    /// The station frame of the last trigger (null before one).
    #[serde(default)]
    pub last_fire_station_frame: Option<u64>,
    /// The frames the pre-window holds now.
    #[serde(default)]
    pub pre_frames_held: u64,
    /// The pre-window asked, frames.
    #[serde(default)]
    pub pre_frames_asked: u64,
    /// Recordings finished since start.
    #[serde(default)]
    pub recordings_saved: u64,
    /// Frames handed to the recording in progress (its pre-window included).
    #[serde(default)]
    pub frames_fed: u64,
    /// Frames of the recording in progress on disk.
    #[serde(default)]
    pub frames_on_disk: u64,
    /// The writer's fault in its own words, if writing has failed.
    #[serde(default)]
    pub write_fault: Option<String>,
    /// How the run ended on its own, if it did.
    #[serde(default)]
    pub run_fault: Option<RunFault>,
    /// The recording in progress, if any.
    #[serde(default)]
    pub recording: Option<RecordingNow>,
}

/// A mark: a row passing one of its limits, or its overload indicator latching.
/// Marks are held: once made, a mark stays in the run's list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mark {
    /// A row passed a limit: `axis` `accel`, `velocity` or `displacement`, the value
    /// of its first sample past the limit, in the axis's unit (g, in/s, mil).
    Limit {
        row: u32,
        axis: String,
        value: f64,
        limit: f64,
        station_frame: u64,
        recording: Option<String>,
    },
    /// A row's overload indicator latched: its first sample whose magnitude reached
    /// `level`, 0.9 of the row's range.
    Overload {
        row: u32,
        value: f64,
        level: f64,
        station_frame: u64,
        recording: Option<String>,
    },
}

/// The recording in progress, as the status states it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingNow {
    /// Its name, `rec_NNNN`.
    pub name: String,
    /// What began it: `start`, `manual` or `level`.
    pub trigger: String,
    /// The station frame of its first frame.
    pub first_station_frame: u64,
    /// The station frame of its trigger (t = 0).
    pub trigger_station_frame: u64,
    /// The pre-window it holds, and the pre-window asked.
    pub pre_frames: u64,
    pub pre_frames_asked: u64,
    /// Frames handed to the record thread (its pre-window included).
    pub frames_fed: u64,
    /// Frames on disk.
    pub frames_on_disk: u64,
    /// The frames it will hold when timed (the pre-window plus its length), or none.
    pub target_frames: Option<u64>,
}

/// Why a run ended on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndCause {
    /// The station or the service's loop failed (an input buffer overflow, a lost
    /// device, an input stall, an ended input stream).
    Fault,
    /// A write failed, or the free space fell under the floor.
    WriteFailed,
    /// The disk fell so far behind that a batch could not be queued.
    WriteBehind,
}

/// How a run ended on its own: the cause, its words, and the station frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunFault {
    pub cause: EndCause,
    pub message: String,
    /// The station frames read when it ended.
    pub station_frame: u64,
}

/// A single event in the recorder log: a monotonic session-second timestamp +
/// a human string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecEvent {
    /// Monotonic index.
    pub seq: u64,
    /// Session seconds (frame-derived) at which the event was logged.
    pub session_s: f64,
    /// The event text.
    pub text: String,
}

/// The event log: a growing list of timestamped event strings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventLog {
    events: Vec<RecEvent>,
    next_seq: u64,
}

impl EventLog {
    /// Append an event stamped at `session_s`.
    pub fn push(&mut self, session_s: f64, text: impl Into<String>) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.events.push(RecEvent {
            seq,
            session_s,
            text: text.into(),
        });
    }

    /// The events, in order.
    pub fn events(&self) -> &[RecEvent] {
        &self.events
    }
}

/// What every Recorder run is, stated in its report in plain words.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunFacts {
    /// What a run records.
    pub concurrent: String,
    /// The rate the recordings are at.
    pub capture_rate: String,
    /// What a row's limits do.
    pub limits: String,
    /// What the overload indicator is.
    pub overload: String,
}

impl Default for RunFacts {
    fn default() -> Self {
        Self {
            concurrent: "A Recorder run records its own station's inputs; it cannot record \
                         during another module's run."
                .into(),
            capture_rate: "Recordings are at the rate the station programmed, with no \
                           decimation; the analysis decimation changes the analysis only."
                .into(),
            limits: "A row's limits are marks: a row passing one is marked at its first \
                     sample past it, once in each episode, and recording goes on."
                .into(),
            overload: "A row's overload indicator latches the first time a sample's \
                       magnitude reaches 0.9 of the row's range, and holds for the run. It \
                       is computed from the samples, not read from the converter."
                .into(),
        }
    }
}

/// A digest of one channel-table row for the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelRow {
    /// Table index.
    pub index: u32,
    /// Role name.
    pub role: String,
    /// Units name.
    pub units: String,
    /// Full-scale range.
    pub range_max: f64,
}

/// One recording produced this session, for the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRecording {
    /// The run directory name.
    pub name: String,
    /// Frames written.
    pub frames: u64,
    /// Whether it was gap-free.
    pub gap_free: bool,
    /// Position-accurate gap records.
    pub gaps: Vec<GapRecord>,
    /// Pre-trigger frames prepended (0 for free-run / manual-without-ring).
    pub pretrigger_frames: u64,
}

/// The serde-serializable report data the UI's report dialog consumes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportData {
    /// The config echo (settings snapshot).
    pub settings: RecorderConfig,
    /// The channel-table digest.
    pub channels: Vec<ChannelRow>,
    /// The recordings produced this session.
    pub recordings: Vec<SessionRecording>,
    /// The full timestamped event log.
    pub event_log: EventLog,
    /// The user-defined metadata.
    pub metadata: Vec<MetaField>,
    /// Per-channel final time stats (last accumulated window), enabled-input
    /// slot order.
    pub channel_stats: Vec<ChannelStat>,
    /// What every Recorder run is, in plain words.
    pub facts: RunFacts,
}
