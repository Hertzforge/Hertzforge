//! `eigen-formats` - import/export of test-data file formats. It replaced the earlier
//! `eigen-export` stub (retired 2026-08-09).
//!
//! Parsers are written FROM PUBLIC SPECIFICATIONS (the UC-SDRL Universal File Format
//! dataset descriptions for UFF/UNV). Zero third-party parser
//! code; `hound` (Apache-2.0) decodes WAV sample frames only.
//!
//! Liberal-read / strict-write: readers tolerate the documented wild-dialect
//! damage and surface per-file WARNINGS; writers emit spec-conformant output
//! only. NAMES-NOT-IDS is structural: writers consume `FunctionRecord`s,
//! which carry no channel ids at all.

pub mod csv;
pub mod native;
pub mod uff;
pub mod wav;

use eigen_data::FunctionRecord;
use serde::{Deserialize, Serialize};

/// Typed import failure. `warnings` on `ImportResult`
/// carry the non-fatal findings; this enum is the fatal path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ImportError {
    /// File unreadable at the OS level.
    Unreadable { detail: String },
    /// No registered importer recognizes the content.
    UnknownFormat,
    /// Recognized but structurally broken.
    Malformed { detail: String, offset: Option<u64> },
    /// Recognized, valid, but a feature this build does not carry.
    Unsupported { feature: String, wave: String },
    /// Refused by the size policy.
    TooLarge { bytes: u64, limit: u64 },
    /// A job is already running (single-job v1 policy).
    Busy,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Unreadable { detail } => write!(f, "cannot read file: {detail}"),
            ImportError::UnknownFormat => write!(f, "unrecognized file format"),
            ImportError::Malformed { detail, offset } => match offset {
                Some(o) => write!(f, "malformed content at byte {o}: {detail}"),
                None => write!(f, "malformed content: {detail}"),
            },
            ImportError::Unsupported { feature, wave } => {
                write!(f, "{feature} is not supported yet (planned: {wave})")
            }
            ImportError::TooLarge { bytes, limit } => {
                write!(f, "file is {bytes} bytes; the current limit is {limit}")
            }
            ImportError::Busy => write!(f, "another import job is already running"),
        }
    }
}

impl std::error::Error for ImportError {}

/// Sniff confidence, ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    No,
    Maybe,
    Yes,
}

#[derive(Debug, Clone, Default)]
pub struct ImportResult {
    pub functions: Vec<FunctionRecord>,
    pub warnings: Vec<String>,
}

/// Progress/cancel plumbing for long imports. `progress` receives
/// (records_done, bytes_done, bytes_total); `cancel` is polled at record
/// boundaries.
pub struct JobCtx<'a> {
    pub progress: &'a mut dyn FnMut(usize, u64, u64),
    pub cancel: &'a dyn Fn() -> bool,
}

impl JobCtx<'_> {
    pub fn cancelled(&self) -> bool {
        (self.cancel)()
    }
}

/// A no-op context for synchronous/small imports and tests.
pub fn sync_ctx() -> (impl FnMut(usize, u64, u64), impl Fn() -> bool) {
    (|_: usize, _: u64, _: u64| {}, || false)
}

/// Format identification by content + extension. Order matters: content
/// signatures first, extension hints last.
pub fn sniff(head: &[u8], path: &str) -> (&'static str, Confidence) {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if uff::sniff_head(head) == Confidence::Yes {
        return ("uff", Confidence::Yes);
    }
    if head.starts_with(b"RIFF") && head.len() >= 12 && &head[8..12] == b"WAVE" {
        return ("wav", Confidence::Yes);
    }
    if native::sniff_capture(path) {
        return ("eigen-capture", Confidence::Yes);
    }
    if native::sniff_snapshot(head, path) {
        return ("eigen-snapshot", Confidence::Yes);
    }
    match ext.as_str() {
        "unv" | "uff" | "uf" => ("uff", Confidence::Maybe),
        "csv" | "txt" | "tsv" | "asc" => ("csv", Confidence::Maybe),
        "wav" => ("wav", Confidence::Maybe),
        // Wave-2 formats: recognized so the error is HONEST, not "unknown".
        "mat" => ("mat", Confidence::Maybe),
        "tdms" => ("tdms", Confidence::Maybe),
        "rsp" | "rpc" | "tim" => ("rpc3", Confidence::Maybe),
        _ => ("", Confidence::No),
    }
}

/// The wave-2 stub map (a typed Unsupported that names the wave).
pub fn wave2_stub(format: &str) -> Option<ImportError> {
    match format {
        "mat" => Some(ImportError::Unsupported {
            feature: "MATLAB .mat import".into(),
            wave: "Insight wave 2".into(),
        }),
        "tdms" => Some(ImportError::Unsupported {
            feature: "NI TDMS import".into(),
            wave: "Insight wave 2".into(),
        }),
        "rpc3" => Some(ImportError::Unsupported {
            feature: "MTS RPC-3 import".into(),
            wave: "Insight wave 2".into(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave2_stubs_are_typed_and_named() {
        for f in ["mat", "tdms", "rpc3"] {
            match wave2_stub(f) {
                Some(ImportError::Unsupported { wave, .. }) => {
                    assert!(wave.contains("wave 2"));
                }
                other => panic!("{f}: expected Unsupported, got {other:?}"),
            }
        }
        assert!(wave2_stub("uff").is_none());
    }

    #[test]
    fn sniff_recognizes_wav_signature() {
        let head = b"RIFF\x24\x00\x00\x00WAVEfmt ";
        assert_eq!(sniff(head, "x.bin"), ("wav", Confidence::Yes));
    }
}
