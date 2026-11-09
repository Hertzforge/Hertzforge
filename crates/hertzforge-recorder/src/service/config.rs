//! Recorder service configuration + the run-state enum.
//!
//! [`RecorderConfig`] is the serde-serializable knob set the UI edits: the
//! acquisition selection ([`AcquisitionConfig`]), the trigger mode
//! ([`TriggerMode`]), the recorder-level policy (auto-rearm, timed duration,
//! watchdog toggle, overload fraction, FRF pair), and the report metadata.
//!
//! A row's abort limits never stop a Recorder run: they are marks. The engine runs
//! on a table with the limits stripped, and the service checks each enabled row's
//! limits on every sample of the analysis pass.
//!
//! `timed_record_s` is counted in frames at the programmed rate: a timed recording
//! holds `round(timed_record_s * fs)` frames after its trigger.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::acq::AcquisitionConfig;
use crate::trigger::TriggerMode;

/// One user-defined report metadata field (key / label / value). Matches the
/// `MetaField` shape the other module services use so the UI has one editor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaField {
    /// Stable key (machine name), e.g. `run_number`.
    pub key: String,
    /// Human label, e.g. `Run Number`.
    pub label: String,
    /// The value (freeform text; the UI supplies typed editors).
    pub value: String,
}

impl MetaField {
    /// A field with an empty value.
    pub fn new(key: &str, label: &str) -> Self {
        Self {
            key: key.to_string(),
            label: label.to_string(),
            value: String::new(),
        }
    }
}

/// The standard, initially-empty metadata field set every recorder report
/// carries (Run Number, Test Article, SN, Operator).
pub fn default_metadata() -> Vec<MetaField> {
    vec![
        MetaField::new("run_number", "Run Number"),
        MetaField::new("test_article", "Test Article"),
        MetaField::new("sn", "SN"),
        MetaField::new("operator", "Operator"),
    ]
}

/// The default overload-proxy fraction: a channel latches an
/// overload when `|eu| >= overload_fraction * range_max`.
pub const DEFAULT_OVERLOAD_FRACTION: f64 = 0.9;

/// The recorder service configuration (serde).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecorderConfig {
    /// The DAQ / analysis selection. Capture is ALWAYS native; the
    /// decimation selector models the analysis ladder only.
    pub acq: AcquisitionConfig,
    /// How recordings start: free-run, manual, or a level trigger.
    pub trigger_mode: TriggerMode,
    /// Auto-rearm count: after the first trigger fire, how many more times the
    /// service re-arms into a NEW sub-recording. 0 = single recording.
    pub auto_rearm: u32,
    /// Pre-trigger ring depth in SECONDS at the native rate. `None` uses
    /// the [`crate::trigger::DEFAULT_PRETRIGGER_S`] default; values above
    /// [`crate::trigger::MAX_PRETRIGGER_S`] clamp there.
    pub pretrigger_s: Option<f64>,
    /// Timed-recording duration in SECONDS, converted to frames internally
    /// (`round(timed_record_s * native_rate_hz)`). `None` = manual stop.
    pub timed_record_s: Option<f64>,
    /// Overload-proxy fraction. Default
    /// [`DEFAULT_OVERLOAD_FRACTION`].
    pub overload_fraction: f64,
    /// Optional live-FRF pair `(reference_channel, response_channel)` as
    /// 0-based indices into the ENABLED input channels. `None` = no live FRF.
    pub frf_pair: Option<(usize, usize)>,
    /// User-defined report metadata (defaults from [`default_metadata`]).
    pub metadata: Vec<MetaField>,
    /// Data root for run directories. `None` = the service's supplied default.
    pub data_root: Option<PathBuf>,
    /// The free space a run leaves on the data volume, bytes; `None` for the
    /// writer's least floor.
    #[serde(default)]
    pub keep_free_bytes: Option<u64>,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            acq: AcquisitionConfig::default(),
            trigger_mode: TriggerMode::Manual,
            auto_rearm: 0,
            pretrigger_s: None,
            timed_record_s: None,
            overload_fraction: DEFAULT_OVERLOAD_FRACTION,
            frf_pair: None,
            metadata: default_metadata(),
            data_root: None,
            keep_free_bytes: None,
        }
    }
}

impl RecorderConfig {
    /// Validate the config (delegates to the acquisition validation, then
    /// checks the recorder-level knobs). Returns the first violation.
    pub fn validate(&self) -> Result<(), String> {
        self.acq.validate()?;
        if let Some(s) = self.timed_record_s {
            if s <= 0.0 || s.is_nan() {
                return Err(format!("timed_record_s must be positive (got {s})"));
            }
        }
        if self.overload_fraction <= 0.0 || self.overload_fraction.is_nan() {
            return Err(format!(
                "overload_fraction must be positive (got {})",
                self.overload_fraction
            ));
        }
        Ok(())
    }

    /// Timed-recording length in FRAMES at the native rate, or `None`
    /// for a manual-stop recording.
    pub fn timed_record_frames(&self) -> Option<u64> {
        self.timed_record_s
            .map(|s| (s * self.acq.native_rate_hz).round().max(0.0) as u64)
    }
}

/// The recorder state machine.
///
/// `Reviewing` is deliberately NOT a run state: review/offline analysis loads a
/// recording from disk and drives the analysis structs directly, independent of
/// the live service state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecState {
    /// No config applied.
    Idle,
    /// Config applied and validated; the engine loop is not live.
    Configured,
    /// The engine loop is live and analysis is streaming; NO disk writes.
    Monitoring,
    /// Armed: the trigger engine is active and the pre-trigger ring is filling.
    Armed,
    /// A recording is in progress (the RecordingWriter is active).
    Recording,
    /// Cleanly stopped (operator stop, or all timed/auto-rearm recordings done).
    Stopped,
    /// The engine reported a BackendError (or the watchdog tripped); the run is
    /// latched faulted with the message preserved.
    Faulted,
}

impl RecState {
    /// True while the engine loop is live (Monitoring / Armed / Recording).
    pub fn is_live(self) -> bool {
        matches!(
            self,
            RecState::Monitoring | RecState::Armed | RecState::Recording
        )
    }
}
