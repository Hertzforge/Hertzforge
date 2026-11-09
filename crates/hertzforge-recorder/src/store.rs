//! The record store: the seam between the service and whatever writes a recording.
//!
//! The service never names the writer. It hands a [`RecordStore`] the five things a
//! recording needs: start one, push frames into it, finish it, ask whether writing has
//! failed, and ask how many frames have reached the disk. [`WriterStore`] puts the
//! crate's own [`RecordingWriter`] behind it; tests put doubles behind it that block,
//! fail or crawl, so what the service does when the disk misbehaves can be shown
//! without a disk that misbehaves.

use std::path::PathBuf;
use std::time::Duration;

use hertzforge_capture::{DurabilityConfig, FreeSpaceFloor, DEFAULT_FLUSH_INTERVAL};
use hertzforge_channel_table::ChannelTable;

use crate::acq::AcquisitionConfig;
use crate::recording::{GapRecord, RecordingSummary, RecordingWriter, StationFacts, WriterFacts};
use crate::trigger::TriggerEvent;

/// What a recording is, given to the store when it begins.
#[derive(Debug, Clone)]
pub struct RecordingSpec {
    /// The folder recordings are made under.
    pub data_root: PathBuf,
    /// The recording's name, `rec_NNNN`.
    pub name: String,
    /// The recorded rows, in column order.
    pub table: ChannelTable,
    /// The acquisition and analysis selection (its rate is the rate the station
    /// programmed: the file is at it).
    pub acq: AcquisitionConfig,
    /// The rate asked of the station, Hz.
    pub requested_fs_hz: f64,
    /// The free space the recording leaves on the data volume, bytes: it is not
    /// begun under it, and it stops writing, with the fault stated, if the volume
    /// falls under it.
    pub floor_bytes: u64,
    /// The trigger that began it, if any; its pre-window is the file's first frames.
    pub pretrigger: Option<TriggerEvent>,
    /// What began it, the station's group delay, its first station frame and the
    /// pre-window asked, for its record.
    pub station: StationFacts,
}

/// The seam between the service and the writer. One recording at a time.
pub trait RecordStore: Send {
    /// Begin a recording. Answers the folder name it was given (a store may change
    /// the name asked to keep it unique).
    fn start(&mut self, spec: RecordingSpec) -> Result<String, String>;
    /// Hand interleaved frames (`frames[..n_frames * rows]`) to the recording.
    fn push(&mut self, frames: &[f32], n_frames: usize);
    /// Finish the recording in progress and say what was written.
    fn finish(&mut self) -> Result<RecordingSummary, String>;
    /// The fault that stopped writing, in its own words, if any.
    fn fault(&self) -> Option<String>;
    /// Frames of the recording in progress that have reached the disk.
    fn frames_on_disk(&self) -> u64;
    /// Whether the recording in progress has lost no frame so far.
    fn gap_free(&self) -> bool {
        true
    }
    /// Note frames the recording in progress never received, at a file position.
    fn gap(&mut self, _gap: GapRecord) {}
}

/// The crate's own writer behind the seam.
pub struct WriterStore {
    writer: Option<RecordingWriter>,
    flush_interval: Duration,
}

impl Default for WriterStore {
    fn default() -> Self {
        Self {
            writer: None,
            flush_interval: DEFAULT_FLUSH_INTERVAL,
        }
    }
}

impl WriterStore {
    /// A store whose writer flushes and syncs every `flush_interval`.
    pub fn with_flush_interval(flush_interval: Duration) -> Self {
        Self {
            writer: None,
            flush_interval,
        }
    }

    /// The writer's flush and sync cadence.
    pub fn flush_interval(&self) -> Duration {
        self.flush_interval
    }
}

impl RecordStore for WriterStore {
    fn start(&mut self, spec: RecordingSpec) -> Result<String, String> {
        if self.writer.is_some() {
            return Err("a recording is already in progress".into());
        }
        let writer = RecordingWriter::start_with(
            &spec.data_root,
            &spec.name,
            &spec.acq,
            spec.table,
            spec.pretrigger,
            WriterFacts {
                requested_fs_hz: Some(spec.requested_fs_hz),
                durability: DurabilityConfig {
                    flush_interval: self.flush_interval,
                    free_space_floor: FreeSpaceFloor::Bytes(spec.floor_bytes),
                    allow_unknown_free_space: false,
                },
                station: spec.station,
                ..WriterFacts::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let name = writer
            .run_dir()
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or(spec.name);
        self.writer = Some(writer);
        Ok(name)
    }

    fn push(&mut self, frames: &[f32], n_frames: usize) {
        if let Some(w) = self.writer.as_mut() {
            w.push_frames(frames, n_frames);
        }
    }

    fn finish(&mut self) -> Result<RecordingSummary, String> {
        let writer = self
            .writer
            .take()
            .ok_or_else(|| "no recording is in progress".to_string())?;
        writer.finish().map_err(|e| e.to_string())
    }

    fn fault(&self) -> Option<String> {
        self.writer.as_ref().and_then(|w| w.fault())
    }

    fn frames_on_disk(&self) -> u64 {
        self.writer.as_ref().map_or(0, |w| w.frames_written())
    }

    fn gap_free(&self) -> bool {
        self.writer.as_ref().is_none_or(|w| w.gaps().is_empty())
    }

    fn gap(&mut self, gap: GapRecord) {
        if let Some(w) = self.writer.as_mut() {
            w.note_gap(gap);
        }
    }
}
