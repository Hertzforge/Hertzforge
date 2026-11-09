//! The recorder service's live surfaces + report data (mirrors the
//! RandomService status/report SHAPE).
//!
//! * [`RecStatus`] - the one-poll live snapshot the UI reads.
//! * [`ReportData`] - the serde-serializable report the UI's report dialog
//!   consumes; carries the concurrent-recording honesty line verbatim.
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
    /// Monotonic seconds the engine loop has been live this session (monitor +
    /// record). Frame-derived (Sim free-runs), not wall-clock.
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
    /// Whether the abort watchdog is ARMED for this session (the resolved value,
    /// not the config request). See `config::resolve_watchdog`.
    #[serde(default)]
    pub watchdog_armed: bool,
    /// True when a HARDWARE station's table carries abort limits and the
    /// watchdog was explicitly turned off anyway. The pane renders a persistent
    /// LIMITS DISARMED banner on this flag.
    #[serde(default)]
    pub limits_disarmed: bool,
    /// The one-line basis for whatever the watchdog policy decided, verbatim.
    #[serde(default)]
    pub watchdog_basis: String,
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

/// The honest limitations carried verbatim into every recorder report. The
/// concurrent-recording line is the load-bearing one: the recorder monitors ITS OWN acquisition
/// session and cannot capture during another module's running test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Honesty {
    /// Concurrent-recording honesty.
    pub concurrent: String,
    /// Capture-rate honesty: recording is always native; the sample
    /// selector is analysis-side decimation.
    pub capture_rate: String,
    /// Watchdog honesty: off by default; a monitor should not
    /// e-stop a test it is not driving.
    pub watchdog: String,
    /// Overload-proxy honesty: a host-side proxy, not observed ADC
    /// saturation.
    pub overload_proxy: String,
}

impl Default for Honesty {
    fn default() -> Self {
        Self {
            concurrent:
                "v1 records its own acquisition session; concurrent capture during another \
                 module's test requires the shared-acquisition layer."
                    .into(),
            capture_rate:
                "Recording rate is always native (65,536 Hz, decimation 1). The sample-rate \
                 selector is ANALYSIS-side anti-aliased decimation; it never changes the \
                 capture rate (RD1 / audit F2)."
                    .into(),
            watchdog:
                "The kinematic abort watchdog is OFF by default: a monitor module should not \
                 e-stop a test it is not driving (RD2). When enabled it acts on the recorder's \
                 OWN backend only. Per-channel overload latches always run."
                    .into(),
            overload_proxy:
                "The per-channel overload indicator is a HOST-SIDE PROXY (|eu| >= fraction * \
                 range_max), not observed ADC saturation (finding C3)."
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
    /// The honest limitations, including the concurrent-recording line.
    pub honesty: Honesty,
}
