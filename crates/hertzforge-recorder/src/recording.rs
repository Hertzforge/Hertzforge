//! Recording writer bridge + reader.
//!
//! [`RecordingWriter`] bridges the recorder onto the testkit
//! [`CaptureSink`] (dedicated writer thread + bounded ring, drop-on-full):
//! it batches incoming frames into ~100 ms chunks before pushing,
//! keeps position-accurate gap records when the ring drops, and owns a
//! recorder-side `recorder.json` sidecar written PROVISIONALLY (`clean: false`)
//! before any data hits disk so an unclean death is recoverable.
//! Zero testkit changes: `recorder.json` rides alongside testkit's
//! `capture.json`.
//!
//! [`RecordingReader`] opens a run directory, reads BOTH sidecars, recovers
//! runs whose writer died before `capture.json` existed, and DISTRUSTS sidecar
//! frame counts: the `telemetry.f32` file size is the authority (sidecars can
//! lie, as a sidecar bug in the Random crate showed). Region
//! reads are chunked seeks - the reader never loads the whole file.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use hertzforge_capture::{disk_budget_max_seconds, CaptureConfig, CaptureSidecar, CaptureSink};
use hertzforge_channel_table::ChannelTable;
use serde::{Deserialize, Serialize};

use crate::acq::AcquisitionConfig;
use crate::trigger::TriggerEvent;

/// The `t0_convention` value for trigger-relative time (t=0 at the trigger
/// sample; pre-trigger frames have negative time).
pub const T0_TRIGGER: &str = "trigger";

/// The `t0_convention` value for start-relative time (t=0 at the first frame).
pub const T0_START: &str = "start";

/// The ring depth (in ~100 ms chunks) handed to the testkit sink: 512 chunks
/// at ~100 ms each absorbs a ~51 s disk stall before anything drops.
pub const DEFAULT_RING_CAPACITY: usize = 512;

/// A recorded ring drop: `dropped_frames` frames are MISSING from the file at
/// file-frame position `at_frame` (i.e. the frame that now sits at index
/// `at_frame` was recorded `dropped_frames` frames after the one at
/// `at_frame - 1`). Position-accurate gap accounting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GapRecord {
    /// File-frame index where the dropped frames would have landed.
    pub at_frame: u64,
    /// Number of frames dropped at that position.
    pub dropped_frames: u64,
}

/// The recorder-owned sidecar (`recorder.json`), written alongside testkit's
/// `capture.json`. Written provisionally (`clean: false`, empty gaps) BEFORE
/// any data, finalized at [`RecordingWriter::finish`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecorderSidecar {
    /// `false` until the writer finishes cleanly. A run found on disk with
    /// `clean: false` died mid-run (or mid-finalize) and must be recovered
    /// from the file itself.
    pub clean: bool,
    /// The acquisition selection at record time (analysis-side settings ride
    /// along for review; capture itself is always native / decimation 1).
    pub config: AcquisitionConfig,
    /// Frames of pre-trigger window at the START of the file (0 for FreeRun /
    /// Manual-without-ring recordings).
    pub pretrigger_frames: u64,
    /// The trigger frame in the TRIGGER ENGINE's global counter (diagnostic;
    /// in FILE coordinates the trigger sample sits at index
    /// `pretrigger_frames`).
    pub trigger_frame_global: Option<u64>,
    /// Time-zero convention: [`T0_TRIGGER`] or [`T0_START`].
    pub t0_convention: String,
    /// Position-accurate ring-drop records (empty when gap-free).
    pub gaps: Vec<GapRecord>,
    /// Channels per interleaved frame.
    pub n_channels: usize,
    /// The native capture rate in Hz (recording is ALWAYS native).
    pub native_rate_hz: f64,
}

/// Summary returned by [`RecordingWriter::finish`].
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingSummary {
    /// The run directory (name may differ from the requested one after
    /// sanitizing / collision suffixing).
    pub run_dir: PathBuf,
    /// Frames actually written to `telemetry.f32`.
    pub frames_written: u64,
    /// Position-accurate drop records.
    pub gaps: Vec<GapRecord>,
    /// True only when NOTHING dropped: zero sink drop count AND zero gap
    /// records (a provable property, not a hope).
    pub gap_free: bool,
    /// Bytes of telemetry written (`frames_written * n_channels * 4`).
    pub bytes: u64,
}

/// The canonical recorder capture config: NATIVE rate, decimation 1 (the
/// recorder never writes decimated data), bounded ring of
/// [`DEFAULT_RING_CAPACITY`] chunks.
pub fn recorder_capture_config(acq: &AcquisitionConfig, n_channels: usize) -> CaptureConfig {
    CaptureConfig {
        n_channels,
        sample_rate_hz: acq.native_rate_hz,
        decimation: 1,
        ring_capacity: DEFAULT_RING_CAPACITY,
    }
}

/// How many whole seconds of native-rate recording fit in `free_bytes`.
///
/// Exact math: bytes/s = `n_channels * 4 * native_rate_hz`. At the
/// v1 default of 8 channels x 4 B x 65,536 Hz that is 2,097,152 B/s = 2.10 MB/s
/// (SI megabytes), i.e. one gigabyte holds ~511 s of 8-channel native capture.
/// `free_bytes` stays a caller-supplied parameter - the `statfs` plumbing that
/// stats the data-root volume lives with the caller.
pub fn recorder_disk_budget_seconds(
    acq: &AcquisitionConfig,
    n_channels: usize,
    free_bytes: u64,
) -> u64 {
    disk_budget_max_seconds(&recorder_capture_config(acq, n_channels), free_bytes)
}

/// Sanitize a requested run name into a safe directory name: keep ASCII
/// alphanumerics, map separators (space, `-`, `_`, `.`, `/`, `\`) to a single
/// `_`, drop everything else, trim leading/trailing underscores (mirrors the
/// shaker-db filename rule). An all-punctuation name sanitizes to `"run"`.
fn sanitize_run_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_us = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_us = false;
        } else if matches!(c, ' ' | '-' | '_' | '.' | '/' | '\\') && !last_us && !out.is_empty() {
            out.push('_');
            last_us = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("run");
    }
    out
}

/// Resolve a collision-free run directory name under `data_root`: the
/// sanitized name as-is if free, else `<name>_2`, `<name>_3`, ... (same-second
/// collision handling).
fn unique_run_name(data_root: &Path, wanted: &str) -> io::Result<String> {
    if !data_root.join(wanted).exists() {
        return Ok(wanted.to_string());
    }
    for suffix in 2..10_000u32 {
        let candidate = format!("{wanted}_{suffix}");
        if !data_root.join(&candidate).exists() {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("could not find a free run name for '{wanted}'"),
    ))
}

fn write_recorder_sidecar(run_dir: &Path, sidecar: &RecorderSidecar) -> io::Result<()> {
    let json = serde_json::to_string_pretty(sidecar)
        .map_err(|e| io::Error::other(format!("recorder sidecar serialize: {e}")))?;
    std::fs::write(run_dir.join("recorder.json"), json)
}

/// A running recording: the batching bridge onto the testkit [`CaptureSink`]
/// plus the recorder-owned sidecar lifecycle.
///
/// If the writer is dropped without [`RecordingWriter::finish`] (crash,
/// abort), the sink's own `Drop` stops its thread and `recorder.json` stays
/// provisional (`clean: false`) - exactly what [`RecordingReader`] recovers.
pub struct RecordingWriter {
    sink: CaptureSink,
    run_dir: PathBuf,
    acq: AcquisitionConfig,
    n_channels: usize,
    /// Batch size in frames (~100 ms at the native rate).
    chunk_frames: usize,
    /// Interleaved frames waiting for a full chunk.
    pending: Vec<f32>,
    /// Frames handed to the sink and ACCEPTED (== the file-frame position of
    /// the next accepted frame).
    frames_accepted: u64,
    /// All frames fed in (accepted + dropped).
    frames_fed: u64,
    gaps: Vec<GapRecord>,
    pretrigger_frames: u64,
    trigger_frame_global: Option<u64>,
    t0_convention: String,
}

impl RecordingWriter {
    /// Start a recording under `data_root/<sanitized run_name>/`.
    ///
    /// Order of operations: sanitize + de-collide the name, create
    /// the directory, write the PROVISIONAL `recorder.json` (`clean: false`)
    /// BEFORE any data, then start the testkit sink (native rate, decimation
    /// 1, ring [`DEFAULT_RING_CAPACITY`]). If `pretrigger` is given, its
    /// pre-window frames are pushed FIRST and the sidecar records
    /// `pretrigger_frames` / `trigger_frame_global` / t0 = `"trigger"`.
    ///
    /// The interleaved frame layout fed to [`RecordingWriter::push_frames`]
    /// must match `table.channels.len()`.
    pub fn start(
        data_root: &Path,
        run_name: &str,
        acq: &AcquisitionConfig,
        table: ChannelTable,
        pretrigger: Option<TriggerEvent>,
    ) -> io::Result<Self> {
        Self::start_with_ring_capacity(
            data_root,
            run_name,
            acq,
            table,
            pretrigger,
            DEFAULT_RING_CAPACITY,
        )
    }

    /// [`RecordingWriter::start`] with an explicit sink ring capacity (in
    /// chunks). Exposed for tests and tuning; production uses
    /// [`DEFAULT_RING_CAPACITY`].
    pub fn start_with_ring_capacity(
        data_root: &Path,
        run_name: &str,
        acq: &AcquisitionConfig,
        table: ChannelTable,
        pretrigger: Option<TriggerEvent>,
        ring_capacity: usize,
    ) -> io::Result<Self> {
        let n_channels = table.channels.len();
        if n_channels == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "channel table has no channels",
            ));
        }
        if acq.native_rate_hz <= 0.0 || acq.native_rate_hz.is_nan() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native_rate_hz must be positive",
            ));
        }

        let name = unique_run_name(data_root, &sanitize_run_name(run_name))?;
        let run_dir = data_root.join(&name);
        std::fs::create_dir_all(&run_dir)?;

        let (pretrigger_frames, trigger_frame_global, t0_convention) = match &pretrigger {
            Some(ev) => (
                ev.pretrigger_frames,
                Some(ev.trigger_frame_global),
                T0_TRIGGER.to_string(),
            ),
            None => (0, None, T0_START.to_string()),
        };

        // The provisional sidecar hits disk BEFORE any data.
        write_recorder_sidecar(
            &run_dir,
            &RecorderSidecar {
                clean: false,
                config: acq.clone(),
                pretrigger_frames,
                trigger_frame_global,
                t0_convention: t0_convention.clone(),
                gaps: Vec::new(),
                n_channels,
                native_rate_hz: acq.native_rate_hz,
            },
        )?;

        let mut cfg = recorder_capture_config(acq, n_channels);
        cfg.ring_capacity = ring_capacity;
        let run_config_json = serde_json::to_string(acq)
            .map_err(|e| io::Error::other(format!("acquisition config serialize: {e}")))?;
        let sink = CaptureSink::start(data_root, &name, cfg, table, run_config_json)?;

        // ~100 ms of frames, rounded up to a power of two (8,192 at 65,536 Hz).
        let chunk_frames = ((acq.native_rate_hz * 0.1).round() as usize)
            .next_power_of_two()
            .max(1);

        let mut writer = Self {
            sink,
            run_dir,
            acq: acq.clone(),
            n_channels,
            chunk_frames,
            pending: Vec::with_capacity(2 * chunk_frames * n_channels),
            frames_accepted: 0,
            frames_fed: 0,
            gaps: Vec::new(),
            pretrigger_frames,
            trigger_frame_global,
            t0_convention,
        };
        if let Some(ev) = pretrigger {
            let n = ev.pretrigger_frames as usize;
            debug_assert_eq!(
                ev.pretrigger.len(),
                n * n_channels,
                "pretrigger window layout must match the channel table"
            );
            writer.push_frames(&ev.pretrigger, n);
        }
        Ok(writer)
    }

    /// Feed interleaved frames (`frames[..n_frames * n_channels]`). Frames are
    /// batched into ~100 ms chunks before hitting the sink ring;
    /// a partial chunk stays pending until [`RecordingWriter::finish`]. When
    /// the sink ring is full the chunk is DROPPED (the feeder never blocks)
    /// and a position-accurate [`GapRecord`] is kept.
    pub fn push_frames(&mut self, frames: &[f32], n_frames: usize) {
        debug_assert!(
            frames.len() >= n_frames * self.n_channels,
            "push_frames slice shorter than n_frames * n_channels"
        );
        self.pending
            .extend_from_slice(&frames[..n_frames * self.n_channels]);
        self.frames_fed += n_frames as u64;

        let chunk_len = self.chunk_frames * self.n_channels;
        let full_chunks = self.pending.len() / chunk_len;
        for c in 0..full_chunks {
            let chunk = &self.pending[c * chunk_len..(c + 1) * chunk_len];
            if self.sink.push(chunk, self.chunk_frames) {
                self.frames_accepted += self.chunk_frames as u64;
            } else {
                self.gaps.push(GapRecord {
                    at_frame: self.frames_accepted,
                    dropped_frames: self.chunk_frames as u64,
                });
            }
        }
        self.pending.drain(..full_chunks * chunk_len);
    }

    /// The run directory (post-sanitize, post-collision name).
    pub fn run_dir(&self) -> &Path {
        &self.run_dir
    }

    /// The batching chunk size in frames (~100 ms at the native rate).
    pub fn chunk_frames(&self) -> usize {
        self.chunk_frames
    }

    /// Gap records so far.
    pub fn gaps(&self) -> &[GapRecord] {
        &self.gaps
    }

    /// Frames fed in so far (accepted + dropped; excludes the pending partial
    /// chunk only in the sense that pending frames are counted as fed but not
    /// yet pushed).
    pub fn frames_fed(&self) -> u64 {
        self.frames_fed
    }

    /// Frames accepted by the sink ring so far.
    pub fn frames_accepted(&self) -> u64 {
        self.frames_accepted
    }

    /// Channels per interleaved frame.
    pub fn n_channels(&self) -> usize {
        self.n_channels
    }

    /// Flush the pending partial chunk, finish the sink (writes
    /// `capture.json`), and finalize `recorder.json` (`clean: true` + final
    /// gap list).
    pub fn finish(mut self) -> io::Result<RecordingSummary> {
        // Flush the partial chunk through the same drop-accounted path.
        if !self.pending.is_empty() {
            let n_frames = self.pending.len() / self.n_channels;
            let pending = std::mem::take(&mut self.pending);
            if self.sink.push(&pending, n_frames) {
                self.frames_accepted += n_frames as u64;
            } else {
                self.gaps.push(GapRecord {
                    at_frame: self.frames_accepted,
                    dropped_frames: n_frames as u64,
                });
            }
        }

        let capture_sidecar = self.sink.finish()?;
        let gap_free = self.gaps.is_empty() && capture_sidecar.chunks_dropped == 0;

        write_recorder_sidecar(
            &self.run_dir,
            &RecorderSidecar {
                clean: true,
                config: self.acq.clone(),
                pretrigger_frames: self.pretrigger_frames,
                trigger_frame_global: self.trigger_frame_global,
                t0_convention: self.t0_convention.clone(),
                gaps: self.gaps.clone(),
                n_channels: self.n_channels,
                native_rate_hz: self.acq.native_rate_hz,
            },
        )?;

        Ok(RecordingSummary {
            run_dir: self.run_dir,
            frames_written: capture_sidecar.frames_written,
            gaps: self.gaps,
            gap_free,
            bytes: capture_sidecar.frames_written * self.n_channels as u64 * 4,
        })
    }
}

/// Everything a reviewer needs to interpret an opened recording.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingInfo {
    /// Sample rate of the frames in the file (native; recorder writes
    /// decimation 1 only).
    pub rate_hz: f64,
    /// Channels per interleaved frame.
    pub n_channels: usize,
    /// Frame count - derived from the FILE SIZE, never from a sidecar.
    pub frames: u64,
    /// `frames / rate_hz`.
    pub duration_s: f64,
    /// [`T0_TRIGGER`] or [`T0_START`].
    pub t0_convention: String,
    /// The trigger frame in the trigger engine's global counter (diagnostic).
    pub trigger_frame_global: Option<u64>,
    /// Pre-trigger frames at the start of the file; in FILE coordinates the
    /// trigger sample sits at this index.
    pub pretrigger_frames: u64,
    /// Position-accurate drop records from the recorder sidecar.
    pub gaps: Vec<GapRecord>,
    /// The recorder sidecar's clean flag (false for unclean/recovered runs and
    /// for legacy runs judged only by `capture.json` presence).
    pub clean: bool,
    /// True when `capture.json` was missing and the run was recovered from the
    /// provisional `recorder.json` + the file size.
    pub recovered: bool,
    /// Present when a sidecar's frame count disagrees with the file size; the
    /// file wins, and this string states both numbers.
    pub metadata_mismatch: Option<String>,
    /// Human-readable status: `"clean"`, `"unclean"`, or `"recovered (unclean)"`.
    pub status: String,
}

/// A reader over a run directory (`telemetry.f32` + sidecars). Region reads
/// are chunked seeks; the whole file is never loaded.
pub struct RecordingReader {
    file: std::fs::File,
    info: RecordingInfo,
    /// The testkit sidecar, when present (channel table, EU context).
    capture_sidecar: Option<CaptureSidecar>,
    /// The recorder sidecar, when present.
    recorder_sidecar: Option<RecorderSidecar>,
}

impl RecordingReader {
    /// Open a run directory.
    ///
    /// Normal path: both `capture.json` and `recorder.json` exist.
    /// CRASH RECOVERY: `capture.json` MISSING but a provisional
    /// `recorder.json` exists - channels/rate come from `recorder.json`,
    /// frames from the `telemetry.f32` size, status `"recovered (unclean)"`.
    /// Plain testkit runs (only `capture.json`) also open, with t0 =
    /// `"start"` and no gap info.
    ///
    /// SANITY: when `capture.json` claims a frame count, it is
    /// checked against the file size. On disagreement the FILE is trusted and
    /// [`RecordingInfo::metadata_mismatch`] describes both numbers.
    pub fn open(run_dir: &Path) -> io::Result<Self> {
        let bin_path = run_dir.join("telemetry.f32");
        let file = std::fs::File::open(&bin_path)?;
        let file_bytes = file.metadata()?.len();

        let capture_sidecar: Option<CaptureSidecar> =
            match std::fs::read_to_string(run_dir.join("capture.json")) {
                Ok(text) => Some(serde_json::from_str(&text).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("capture.json: {e}"))
                })?),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
        let recorder_sidecar: Option<RecorderSidecar> =
            match std::fs::read_to_string(run_dir.join("recorder.json")) {
                Ok(text) => Some(serde_json::from_str(&text).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("recorder.json: {e}"))
                })?),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };

        // Channels + rate: the recorder sidecar is authoritative when present
        // (it exists from before the first data byte); else the testkit one.
        let (n_channels, rate_hz) = match (&recorder_sidecar, &capture_sidecar) {
            (Some(r), _) => (r.n_channels, r.native_rate_hz),
            (None, Some(c)) => (c.config.n_channels, c.config.sample_rate_hz),
            (None, None) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "no sidecar found (neither recorder.json nor capture.json)",
                ));
            }
        };
        if n_channels == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "sidecar claims zero channels",
            ));
        }

        // The FILE is the authority on frame count.
        let frame_bytes = n_channels as u64 * 4;
        let frames = file_bytes / frame_bytes;
        let mut mismatch: Option<String> = None;
        if file_bytes % frame_bytes != 0 {
            mismatch = Some(format!(
                "telemetry.f32 is {file_bytes} bytes, not a whole number of \
                 {n_channels}-channel frames; trailing partial frame ignored \
                 (using {frames} frames)"
            ));
        }
        if let Some(cap) = &capture_sidecar {
            if cap.frames_written != frames {
                mismatch = Some(format!(
                    "capture.json claims {} frames ({} bytes) but telemetry.f32 \
                     holds {} frames ({} bytes); trusting the file",
                    cap.frames_written,
                    cap.frames_written * frame_bytes,
                    frames,
                    file_bytes
                ));
            }
        }

        let recovered = capture_sidecar.is_none();
        let clean = match &recorder_sidecar {
            Some(r) => r.clean,
            // Legacy testkit-only run: capture.json presence is the best signal.
            None => capture_sidecar.is_some(),
        };
        let status = if recovered {
            "recovered (unclean)".to_string()
        } else if clean {
            "clean".to_string()
        } else {
            "unclean".to_string()
        };

        let (t0_convention, trigger_frame_global, pretrigger_frames, gaps) = match &recorder_sidecar
        {
            Some(r) => (
                r.t0_convention.clone(),
                r.trigger_frame_global,
                r.pretrigger_frames,
                r.gaps.clone(),
            ),
            None => (T0_START.to_string(), None, 0, Vec::new()),
        };

        let info = RecordingInfo {
            rate_hz,
            n_channels,
            frames,
            duration_s: frames as f64 / rate_hz,
            t0_convention,
            trigger_frame_global,
            pretrigger_frames,
            gaps,
            clean,
            recovered,
            metadata_mismatch: mismatch,
            status,
        };
        Ok(Self {
            file,
            info,
            capture_sidecar,
            recorder_sidecar,
        })
    }

    /// The interpreted recording info.
    pub fn info(&self) -> &RecordingInfo {
        &self.info
    }

    /// The testkit sidecar (channel table / EU context), when present.
    pub fn capture_sidecar(&self) -> Option<&CaptureSidecar> {
        self.capture_sidecar.as_ref()
    }

    /// The recorder sidecar, when present.
    pub fn recorder_sidecar(&self) -> Option<&RecorderSidecar> {
        self.recorder_sidecar.as_ref()
    }

    /// Read `n_frames` interleaved frames starting at `start_frame` via a
    /// direct seek (`byte = frame * n_channels * 4` - the file is never loaded
    /// whole). Requests past EOF are clamped: the returned vector holds only
    /// the frames that exist.
    pub fn read_region(&mut self, start_frame: u64, n_frames: usize) -> io::Result<Vec<f32>> {
        if start_frame >= self.info.frames {
            return Ok(Vec::new());
        }
        let avail = (self.info.frames - start_frame) as usize;
        let n = n_frames.min(avail);
        let n_ch = self.info.n_channels;
        self.file
            .seek(SeekFrom::Start(start_frame * n_ch as u64 * 4))?;
        let mut bytes = vec![0u8; n * n_ch * 4];
        self.file.read_exact(&mut bytes)?;
        Ok(bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect())
    }

    /// Read one channel's samples over a frame region (deinterleaved).
    pub fn read_channel_region(
        &mut self,
        ch: usize,
        start_frame: u64,
        n_frames: usize,
    ) -> io::Result<Vec<f32>> {
        let n_ch = self.info.n_channels;
        if ch >= n_ch {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("channel {ch} out of range (recording has {n_ch} channels)"),
            ));
        }
        let region = self.read_region(start_frame, n_frames)?;
        Ok(region.chunks_exact(n_ch).map(|frame| frame[ch]).collect())
    }

    /// A display-only min/max envelope of one channel: the recording is split
    /// into up to `n_columns` equal frame spans and each span's (min, max) is
    /// returned, via a CHUNKED sequential scan (never the whole file at once).
    /// Fewer than `n_columns` pairs come back when the recording has fewer
    /// frames than columns.
    pub fn overview(&mut self, ch: usize, n_columns: usize) -> io::Result<Vec<(f32, f32)>> {
        let n_ch = self.info.n_channels;
        if ch >= n_ch {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("channel {ch} out of range (recording has {n_ch} channels)"),
            ));
        }
        let frames = self.info.frames;
        if n_columns == 0 || frames == 0 {
            return Ok(Vec::new());
        }
        let col_width = frames.div_ceil(n_columns as u64);
        let n_cols_used = frames.div_ceil(col_width) as usize;
        let mut out: Vec<(f32, f32)> = vec![(f32::INFINITY, f32::NEG_INFINITY); n_cols_used];

        // Chunked scan: 64k frames per read.
        const SCAN_FRAMES: u64 = 65_536;
        self.file.seek(SeekFrom::Start(0))?;
        let mut frame_idx: u64 = 0;
        let mut bytes = vec![0u8; (SCAN_FRAMES as usize) * n_ch * 4];
        while frame_idx < frames {
            let this = (frames - frame_idx).min(SCAN_FRAMES) as usize;
            let byte_len = this * n_ch * 4;
            self.file.read_exact(&mut bytes[..byte_len])?;
            for f in 0..this {
                let off = (f * n_ch + ch) * 4;
                let v = f32::from_le_bytes([
                    bytes[off],
                    bytes[off + 1],
                    bytes[off + 2],
                    bytes[off + 3],
                ]);
                let col = ((frame_idx + f as u64) / col_width) as usize;
                let (lo, hi) = &mut out[col];
                if v < *lo {
                    *lo = v;
                }
                if v > *hi {
                    *hi = v;
                }
            }
            frame_idx += this as u64;
        }
        Ok(out)
    }

    /// The time of a FILE frame index, honoring the t0 convention:
    /// with t0 = `"trigger"`, t=0 is the trigger sample (file frame
    /// `pretrigger_frames`) and earlier frames have NEGATIVE time; with
    /// t0 = `"start"`, t=0 is file frame 0.
    pub fn time_of_frame(&self, frame: u64) -> f64 {
        let f = frame as f64;
        if self.info.t0_convention == T0_TRIGGER {
            (f - self.info.pretrigger_frames as f64) / self.info.rate_hz
        } else {
            f / self.info.rate_hz
        }
    }
}
