//! `hertzforge-recorder`: the Recorder's core. It records a station's inputs and
//! drives nothing.
//!
//! * [`acq`] - [`AcquisitionConfig`]: the acquisition and analysis selection (rate,
//!   block, window, overlap, averaging, analysis decimation) with its derived readouts
//!   (df, lines, span). Recordings are always at the rate the station programmed; the
//!   decimation is the analysis's only.
//! * [`trigger`] - [`TriggerEngine`] and [`PreTriggerRing`]: a level trigger with a
//!   hysteresis band and a holdoff, and a pre-window whose frames join the recording
//!   with no gap, counted in station frames.
//! * [`recording`] - [`RecordingWriter`] puts a recording onto the capture crate's
//!   writer thread ([`CaptureSink`](hertzforge_capture::CaptureSink)), in chunks of
//!   about 100 ms, with position-accurate gap records, and writes the recording's own
//!   record (`stream.json`) before any frame and again when it ends.
//!   [`RecordingReader`] reads a recording back and takes its length from the file,
//!   never from a record.
//! * [`store`] - [`RecordStore`], the seam between the service and the writer, with
//!   [`WriterStore`] behind it.
//! * [`record`] - the record thread, the one thread that touches a recording's files.
//! * [`analysis`] - the analysis on hertzforge-dsp: [`ChannelDecimator`] ->
//!   [`BlockAssembler`] -> [`SpectrumPipeline`] and [`FrfPipeline`] (H1, Hv and
//!   coherence, the coherence withheld until two averages), time statistics, velocity
//!   and displacement, and the offline region functions [`analyze_region`] and
//!   [`analyze_region_frf`] built from the same parts.
//! * [`service`] - [`RecorderService`]: a run on one station.
//!
//! ## A recording on disk
//!
//! A recording's folder holds:
//!
//! * `telemetry.f32` - no header; little-endian f32 frames, one value per recorded row.
//! * `capture.json` - the writer's record ([`CaptureSidecar`](hertzforge_capture::CaptureSidecar)),
//!   written before any frame with `complete: false` and finalized at the end.
//! * `stream.json` - the recording's record ([`RecorderSidecar`]): its rows, rates,
//!   station frames, pre-window, gaps, marks and fault, `complete: false` until the
//!   recording ends.

pub mod acq;
pub mod analysis;
pub mod record;
pub mod recording;
pub mod service;
pub mod store;
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
    RecordingInfo, RecordingReader, RecordingSummary, RecordingWriter, StationFacts, WriterFacts,
};
pub use store::{RecordStore, RecordingSpec, WriterStore};
pub use trigger::{LevelTrigger, PreTriggerRing, Slope, TriggerEngine, TriggerEvent, TriggerMode};

pub use service::{
    ChannelMeter, FrfSnapshot, MetaField, RecState, RecStatus, RecorderConfig, RecorderError,
    RecorderService, RecordingInfoLite, ReportData,
};

// The capture crate's disk budget, so a caller has one import surface.
pub use hertzforge_capture::disk_budget_max_seconds;
