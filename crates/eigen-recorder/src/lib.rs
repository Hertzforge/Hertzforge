//! `eigen-recorder` - the Data Recorder module core.
//!
//! Four pure-library pieces, no engine dependency yet:
//!
//! * [`acq`] - [`AcquisitionConfig`]: the operator's "standard DAQ selection tools"
//!   as a first-class, validated, serializable type (sample-rate ladder, block size,
//!   window, overlap, averaging) plus the derived readouts (df, spectral lines,
//!   span). HONEST SEMANTICS: recording is always at the native rate; the decimation
//!   selector models the ANALYSIS-side ladder only.
//! * [`trigger`] - [`TriggerEngine`] + [`PreTriggerRing`]: hysteresis-armed level
//!   triggering in the frame domain (EU samples), an interleaved pre-trigger ring
//!   whose snapshot joins gap-free with the live continuation, holdoff that
//!   never re-arms before the ring has refilled, and auto-rearm counting.
//! * [`recording`] - [`RecordingWriter`] bridges onto the testkit
//!   [`CaptureSink`](eigen_capture::CaptureSink) (~100 ms batched pushes,
//!   position-accurate gap accounting) and owns a crash-safe provisional
//!   `recorder.json` sidecar. [`RecordingReader`] reads the run back
//!   (chunked region reads, min/max overview, trigger-relative time axis) and
//!   DISTRUSTS sidecars: the file size is the authority on frame count.
//! * [`analysis`] - the analysis pipeline on eigen-dsp:
//!   [`ChannelDecimator`] (anti-aliased analysis-span ladder) ->
//!   [`BlockAssembler`] (lockstep overlap blocks) -> [`SpectrumPipeline`]
//!   (EU output-type taxonomy) and [`FrfPipeline`] (H1 default +
//!   labeled Hv + coherence with the single-average trap gated),
//!   plus time stats (vibration-convention kurtosis), velocity/
//!   displacement derivation (spectral omega-arithmetic + trimmed time-domain
//!   integration), and the OFFLINE region functions [`analyze_region`] /
//!   [`analyze_region_frf`] that reuse the same structs.
//!
//! ## On-disk run format
//!
//! A run directory contains:
//!
//! * `telemetry.f32` - headerless interleaved little-endian f32 frames (testkit
//!   contract; frame = `n_channels` consecutive f32s).
//! * `capture.json` - eigen-capture's [`CaptureSidecar`](eigen_capture::CaptureSidecar)
//!   (written by `CaptureSink::finish`; MISSING when the writer died mid-run).
//! * `recorder.json` - this crate's [`RecorderSidecar`]: written PROVISIONALLY
//!   (`clean: false`) before any data, finalized (`clean: true` + gap list) at
//!   stop. Zero testkit changes; the recorder sidecar rides alongside.

pub mod acq;
pub mod analysis;
pub mod recording;
pub mod service;
pub mod trigger;

pub use acq::{AcquisitionConfig, AveragingKind, WindowKind};
pub use analysis::{
    analyze_region, analyze_region_frf, hop_frames, integrate_region, psd_accel_to_displacement,
    psd_accel_to_velocity, psd_velocity_to_displacement, settle_trim_samples, time_stats,
    window_vector, BlockAssembler, ChannelAnalysis, ChannelDecimator, FrfAnalysis, FrfPipeline,
    RegionAnalysis, SpectrumOutput, SpectrumPipeline, TimeStats, TimeStatsAccumulator,
};
pub use recording::{
    recorder_capture_config, recorder_disk_budget_seconds, GapRecord, RecorderSidecar,
    RecordingInfo, RecordingReader, RecordingSummary, RecordingWriter,
};
pub use trigger::{LevelTrigger, PreTriggerRing, Slope, TriggerEngine, TriggerEvent, TriggerMode};

pub use service::{
    ChannelMeter, FrfSnapshot, MetaField, RecState, RecStatus, RecorderConfig, RecorderError,
    RecorderService, RecordingInfoLite, ReportData,
};

// Re-export the testkit disk-budget primitive so service glue has one import surface.
pub use eigen_capture::disk_budget_max_seconds;
