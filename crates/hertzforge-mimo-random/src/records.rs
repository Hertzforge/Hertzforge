//! The run's records (design 11, Q38).
//!
//! On by default. A run's folder under the local data root holds:
//!
//! | File | What | Cadence |
//! |---|---|---|
//! | `telemetry.f32`, `capture.json` | the raw input stream (`hertzforge_capture::CaptureSink`) | every segment |
//! | `identification.bin` / `.json` | the identification set with its setup hash | once |
//! | `loops_diagonal.bin` / `.json` | per loop: every control's PSD and error, every drive's PSD | every loop |
//! | `loops_summary.jsonl` | per loop: level, state, rms errors, drive rms, guards, timing | every loop |
//! | `sdm_<loop>.bin` / `.json` | the full control response and drive spectral matrices | every `snapshot_every_s`, plus the first and last loop at full level |
//! | `law.json` | the law: name, version, hash, parameters | once |
//! | `report.json` | the report data | at the end |
//!
//! Binary payloads are little-endian `f64`, complex values as `re, im` pairs,
//! Hermitian stacks packed (upper triangle, row-major, line-major), matrix stacks
//! row-major per line; each sidecar names the arrays, their shapes and their byte
//! offsets. The conversion to the `.hforge` run file is the analysis service's
//! (decision C, option 2).

use crate::report::{GuardSummary, LawLine, LoopTiming, MimoReport, MimoState};
use crate::service::IdentificationSet;
use hertzforge_capture::{
    append_binary_record, read_binary_record, write_atomic, write_binary_record, CaptureConfig,
    CaptureSink,
};
use hertzforge_channel_table::ChannelTable;
use hertzforge_random::service::engine::SegmentResult;
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The identification record's name.
pub const IDENTIFICATION: &str = "identification";
/// The per-loop diagonal record's name.
pub const LOOPS_DIAGONAL: &str = "loops_diagonal";
/// The per-loop summary file.
pub const LOOPS_SUMMARY: &str = "loops_summary.jsonl";

/// One loop's summary line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopSummary {
    pub loop_index: u64,
    pub level_db: f64,
    pub state: MimoState,
    pub error_db: Vec<f64>,
    pub drive_rms: Vec<f64>,
    pub guards: GuardSummary,
    pub timing: LoopTiming,
    pub law_misses: u32,
}

/// One array in a binary payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArrayInfo {
    pub name: String,
    /// `f64` or `c64` (two `f64` per value).
    pub kind: String,
    /// Logical shape; a packed Hermitian stack is `[lines, n, n]` with
    /// `packed: true`.
    pub shape: Vec<usize>,
    pub packed: bool,
    pub offset_bytes: usize,
    pub len_bytes: usize,
}

/// A payload being assembled.
#[derive(Default)]
struct Payload {
    bytes: Vec<u8>,
    arrays: Vec<ArrayInfo>,
}

impl Payload {
    fn f64s(&mut self, name: &str, shape: Vec<usize>, v: &[f64]) {
        let off = self.bytes.len();
        for x in v {
            self.bytes.extend_from_slice(&x.to_le_bytes());
        }
        self.arrays.push(ArrayInfo {
            name: name.into(),
            kind: "f64".into(),
            shape,
            packed: false,
            offset_bytes: off,
            len_bytes: self.bytes.len() - off,
        });
    }

    fn c64s(&mut self, name: &str, shape: Vec<usize>, packed: bool, v: &[C64]) {
        let off = self.bytes.len();
        for x in v {
            self.bytes.extend_from_slice(&x.re.to_le_bytes());
            self.bytes.extend_from_slice(&x.im.to_le_bytes());
        }
        self.arrays.push(ArrayInfo {
            name: name.into(),
            kind: "c64".into(),
            shape,
            packed,
            offset_bytes: off,
            len_bytes: self.bytes.len() - off,
        });
    }

    fn herm(&mut self, name: &str, g: &HermitianStack) {
        self.c64s(name, vec![g.lines(), g.n(), g.n()], true, g.data());
    }

    fn mat(&mut self, name: &str, m: &MatrixStack) {
        let (l, r, c) = m.dims();
        self.c64s(name, vec![l, r, c], false, m.data());
    }
}

/// The identification record's sidecar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentificationSidecar {
    pub format: String,
    pub setup_hash: String,
    pub drive_vrms: Vec<f64>,
    pub level_db: f64,
    pub frames: usize,
    pub steps: usize,
    pub lag_frames: usize,
    pub arrays: Vec<ArrayInfo>,
}

/// The format tag of an identification record.
pub const IDENT_FORMAT: &str = "hertzforge-mimo-identification/1";

fn read_f64(b: &[u8], a: &ArrayInfo) -> Result<Vec<f64>, String> {
    let s = b
        .get(a.offset_bytes..a.offset_bytes + a.len_bytes)
        .ok_or_else(|| format!("array {} runs past the payload", a.name))?;
    Ok(s.chunks_exact(8)
        .map(|c| f64::from_le_bytes(c.try_into().expect("8 bytes")))
        .collect())
}

fn read_c64(b: &[u8], a: &ArrayInfo) -> Result<Vec<C64>, String> {
    let v = read_f64(b, a)?;
    Ok(v.chunks_exact(2).map(|p| C64::new(p[0], p[1])).collect())
}

fn find<'a>(arrays: &'a [ArrayInfo], name: &str) -> Result<&'a ArrayInfo, String> {
    arrays
        .iter()
        .find(|a| a.name == name)
        .ok_or_else(|| format!("the record has no array `{name}`"))
}

fn herm_of(b: &[u8], arrays: &[ArrayInfo], name: &str) -> Result<HermitianStack, String> {
    let a = find(arrays, name)?;
    let (l, n) = (a.shape[0], a.shape[1]);
    HermitianStack::from_packed(l, n, read_c64(b, a)?).map_err(|e| e.0)
}

fn mat_of(b: &[u8], arrays: &[ArrayInfo], name: &str) -> Result<MatrixStack, String> {
    let a = find(arrays, name)?;
    MatrixStack::from_vec(a.shape[0], a.shape[1], a.shape[2], read_c64(b, a)?).map_err(|e| e.0)
}

/// Write an identification set as a binary record in `dir`.
pub fn write_identification(dir: &Path, set: &IdentificationSet) -> Result<PathBuf, String> {
    let mut p = Payload::default();
    p.mat("frf", &set.frf);
    p.herm("g_cc", &set.g_cc);
    p.mat("g_rd", &set.g_rd);
    p.herm("g_dd", &set.g_dd);
    p.herm("noise", &set.noise);
    let lines = set.frf.lines();
    p.f64s(
        "coherence",
        vec![lines, set.coherence.len() / lines.max(1)],
        &set.coherence,
    );
    p.f64s("condition", vec![lines], &set.condition);
    let side = IdentificationSidecar {
        format: IDENT_FORMAT.into(),
        setup_hash: set.setup_hash.clone(),
        drive_vrms: set.drive_vrms.clone(),
        level_db: set.level_db,
        frames: set.frames,
        steps: set.steps,
        lag_frames: set.lag_frames,
        arrays: p.arrays,
    };
    write_binary_record(dir, IDENTIFICATION, &side, &p.bytes).map_err(|e| e.to_string())
}

/// Read an identification set back from a run folder, for
/// `MimoRandomService::load_identification` (which checks the setup hash).
pub fn read_identification(dir: &Path) -> Result<IdentificationSet, String> {
    let (side, bytes) = read_binary_record(dir, IDENTIFICATION)
        .map_err(|e| format!("no identification record in {}: {e}", dir.display()))?;
    let side: IdentificationSidecar = serde_json::from_value(side)
        .map_err(|e| format!("the identification sidecar is not readable: {e}"))?;
    if side.format != IDENT_FORMAT {
        return Err(format!(
            "the identification record is format `{}`, this program reads `{IDENT_FORMAT}`",
            side.format
        ));
    }
    let a = &side.arrays;
    Ok(IdentificationSet {
        setup_hash: side.setup_hash.clone(),
        frf: mat_of(&bytes, a, "frf")?,
        g_cc: herm_of(&bytes, a, "g_cc")?,
        g_rd: mat_of(&bytes, a, "g_rd")?,
        g_dd: herm_of(&bytes, a, "g_dd")?,
        noise: herm_of(&bytes, a, "noise")?,
        coherence: read_f64(&bytes, find(a, "coherence")?)?,
        condition: read_f64(&bytes, find(a, "condition")?)?,
        drive_vrms: side.drive_vrms,
        level_db: side.level_db,
        frames: side.frames,
        steps: side.steps,
        lag_frames: side.lag_frames,
    })
}

/// The per-loop diagonal record's sidecar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagonalSidecar {
    pub lines: usize,
    pub controls: usize,
    pub drives: usize,
    /// Loops in the payload; each is `lines * (2 * controls + drives)` `f64`:
    /// the control PSDs at full level, the control errors in dB (NaN out of band),
    /// the drive PSDs at full level, each line-major.
    pub loops: u64,
    pub loop_index: Vec<u64>,
}

/// The snapshot sidecar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotSidecar {
    pub loop_index: u64,
    pub why: String,
    pub arrays: Vec<ArrayInfo>,
}

/// Writes one run's records.
pub struct RunRecorder {
    dir: PathBuf,
    sink: Option<CaptureSink>,
    diag: Option<DiagonalSidecar>,
    snapshot_every_s: f64,
    since_snapshot_s: f64,
    first_full_written: bool,
    last_full: Option<(u64, HermitianStack, HermitianStack)>,
    loop_index: u64,
}

impl RunRecorder {
    /// Start the run's records under `data_root/name` with the raw stream of
    /// `n_ch` inputs at `fs_hz`.
    pub fn start(
        data_root: &Path,
        name: &str,
        n_ch: usize,
        fs_hz: f64,
        table: ChannelTable,
        settings_json: &str,
    ) -> Result<Self, String> {
        let sink = CaptureSink::start(
            data_root,
            name,
            CaptureConfig::at_rate(n_ch, fs_hz),
            table,
            settings_json.to_string(),
        )
        .map_err(|e| e.to_string())?;
        Ok(RunRecorder {
            dir: sink.run_dir().to_path_buf(),
            sink: Some(sink),
            diag: None,
            snapshot_every_s: crate::config::DEFAULT_SNAPSHOT_EVERY_S,
            since_snapshot_s: 0.0,
            first_full_written: false,
            last_full: None,
            loop_index: 0,
        })
    }

    /// Set the snapshot cadence, seconds.
    pub fn set_snapshot_every_s(&mut self, s: f64) {
        self.snapshot_every_s = s;
    }

    /// The run folder.
    pub fn run_dir(&self) -> &Path {
        &self.dir
    }

    /// Feed one segment's raw interleaved inputs (never blocks; drops on a full
    /// ring, counted by the sink).
    pub fn feed_raw(&mut self, seg: &SegmentResult) {
        if let Some(s) = self.sink.as_ref() {
            let frames = seg.interleaved.len() / seg.n_ch.max(1);
            let samples: Vec<f32> = seg.interleaved.iter().map(|&v| v as f32).collect();
            s.push(&samples, frames);
        }
    }

    /// Record the identification set.
    pub fn write_identification(&mut self, set: &IdentificationSet) -> Result<String, String> {
        write_identification(&self.dir, set).map(|p| p.display().to_string())
    }

    fn snapshot(
        &self,
        loop_index: u64,
        why: &str,
        resp: &HermitianStack,
        drive: &HermitianStack,
    ) -> Result<(), String> {
        let mut p = Payload::default();
        p.herm("response_full_level", resp);
        p.herm("drive_full_level", drive);
        let side = SnapshotSidecar {
            loop_index,
            why: why.into(),
            arrays: p.arrays,
        };
        write_binary_record(&self.dir, &format!("sdm_{loop_index:06}"), &side, &p.bytes)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Record one loop: the summary, the diagonals, and a full snapshot when one
    /// is due.
    pub fn write_loop(
        &mut self,
        summary: &LoopSummary,
        resp_full: Option<&HermitianStack>,
        drive_full: &HermitianStack,
        error_lines: &[f64],
        at_full_level: bool,
        segment_s: f64,
    ) -> Result<(), String> {
        use std::io::Write;
        self.loop_index = summary.loop_index;
        let line = serde_json::to_string(summary).map_err(|e| e.to_string())?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(LOOPS_SUMMARY))
            .map_err(|e| e.to_string())?;
        writeln!(f, "{line}").map_err(|e| e.to_string())?;
        let Some(resp) = resp_full else {
            return Ok(());
        };
        let (lines, nc, nd) = (resp.lines(), resp.n(), drive_full.n());
        let mut chunk = Vec::with_capacity(lines * (2 * nc + nd) * 8);
        for line in 0..lines {
            for i in 0..nc {
                chunk.extend_from_slice(&resp.get(line, i, i).re.to_le_bytes());
            }
        }
        for line in 0..lines {
            for i in 0..nc {
                let e = error_lines.get(line * nc + i).copied().unwrap_or(f64::NAN);
                chunk.extend_from_slice(&e.to_le_bytes());
            }
        }
        for line in 0..lines {
            for k in 0..nd {
                chunk.extend_from_slice(&drive_full.get(line, k, k).re.to_le_bytes());
            }
        }
        let side = self.diag.get_or_insert(DiagonalSidecar {
            lines,
            controls: nc,
            drives: nd,
            loops: 0,
            loop_index: Vec::new(),
        });
        side.loops += 1;
        side.loop_index.push(summary.loop_index);
        append_binary_record(&self.dir, LOOPS_DIAGONAL, side, &chunk).map_err(|e| e.to_string())?;
        self.since_snapshot_s += segment_s;
        if at_full_level && !self.first_full_written {
            self.first_full_written = true;
            self.since_snapshot_s = 0.0;
            self.snapshot(
                summary.loop_index,
                "first loop at full level",
                resp,
                drive_full,
            )?;
        } else if self.since_snapshot_s >= self.snapshot_every_s {
            self.since_snapshot_s = 0.0;
            self.snapshot(summary.loop_index, "cadence", resp, drive_full)?;
        }
        if at_full_level {
            self.last_full = Some((summary.loop_index, resp.clone(), drive_full.clone()));
        }
        Ok(())
    }

    /// Finish: the last full-level snapshot, the law record, the report, the raw
    /// stream's sidecar.
    pub fn finish(&mut self, law: Option<&LawLine>, report: &MimoReport) -> Result<(), String> {
        if let Some((idx, r, d)) = self.last_full.take() {
            let name = format!("sdm_{idx:06}");
            if !self.dir.join(format!("{name}.bin")).exists() {
                self.snapshot(idx, "last loop at full level", &r, &d)?;
            }
        }
        if let Some(l) = law {
            let json = serde_json::to_vec_pretty(l).map_err(|e| e.to_string())?;
            write_atomic(&self.dir.join("law.json"), &json).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_vec_pretty(report).map_err(|e| e.to_string())?;
        write_atomic(&self.dir.join("report.json"), &json).map_err(|e| e.to_string())?;
        if let Some(s) = self.sink.take() {
            s.finish().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// Read a full spectral-matrix snapshot back: the response and drive at full
/// level.
pub fn read_snapshot(
    dir: &Path,
    loop_index: u64,
) -> Result<(HermitianStack, HermitianStack), String> {
    let (side, bytes) =
        read_binary_record(dir, &format!("sdm_{loop_index:06}")).map_err(|e| e.to_string())?;
    let side: SnapshotSidecar = serde_json::from_value(side).map_err(|e| e.to_string())?;
    Ok((
        herm_of(&bytes, &side.arrays, "response_full_level")?,
        herm_of(&bytes, &side.arrays, "drive_full_level")?,
    ))
}
