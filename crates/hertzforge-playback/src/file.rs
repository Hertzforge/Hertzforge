//! The native drive file (`docs/drive-file.md`): a folder holding `drive.bin` and its
//! sidecar `drive.json`.
//!
//! `drive.bin` is the 8-byte magic [`MAGIC`], then little-endian `f32` samples
//! interleaved by sample, `samples x channels`, in volts at the drive output.
//! `drive.json` says what the samples are: `format` (`hertzforge-drive`), `version`
//! (1), `rate_hz`, `samples`, `channels` (each `name`, `unit` `V` and its `source`
//! signal), the `sha256` of `drive.bin`, the `source` it was converted from, the
//! `fixes` made to it, the `tool` and `created_utc`.
//!
//! [`DriveFile::open`] reads the sidecar and checks the layout (the format, the
//! version, the units, the size against the sidecar, the magic). It reads no sample:
//! the samples are read by the check ([`crate::check`]) and by the run's reader
//! thread ([`crate::source`]), both streaming, so a file is limited only by the disk.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// The first eight bytes of `drive.bin`.
pub const MAGIC: &[u8; 8] = b"HFDRV01\0";
/// The sidecar's `format`.
pub const FORMAT: &str = "hertzforge-drive";
/// The sidecar version this reader takes.
pub const VERSION: u64 = 1;
/// The bytes before the first sample.
pub const HEADER_BYTES: u64 = 8;
/// The samples file and the sidecar, inside the folder.
pub const BIN_NAME: &str = "drive.bin";
pub const JSON_NAME: &str = "drive.json";

/// Why a drive file was refused: a stable reason, the words, and the facts.
#[derive(Debug, Clone, PartialEq)]
pub struct FileError {
    pub reason: &'static str,
    pub message: String,
    pub detail: Value,
}

impl FileError {
    pub fn new(reason: &'static str, message: impl Into<String>, detail: Value) -> FileError {
        FileError {
            reason,
            message: message.into(),
            detail,
        }
    }
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.reason, self.message)
    }
}

impl std::error::Error for FileError {}

/// One channel of the file, as its sidecar names it.
#[derive(Debug, Clone, PartialEq)]
pub struct DriveChannel {
    pub name: String,
    pub unit: String,
    /// The signal it was made from, as the sidecar states it.
    pub source: Value,
}

/// An opened drive file: its folder, its sidecar whole and the facts read from it.
#[derive(Debug, Clone, PartialEq)]
pub struct DriveFile {
    pub folder: PathBuf,
    pub rate_hz: f64,
    pub samples: u64,
    pub channels: Vec<DriveChannel>,
    /// The SHA-256 the sidecar states for `drive.bin`, lowercase hexadecimal.
    pub sha256: String,
    /// The sidecar exactly as read.
    pub sidecar: Value,
}

fn unreadable(path: &Path, e: impl std::fmt::Display) -> FileError {
    FileError::new(
        "drive_unreadable",
        format!(
            "The drive file could not be read: {} ({e}).",
            path.display()
        ),
        serde_json::json!({"path": path.display().to_string(), "reason": e.to_string()}),
    )
}

fn bad(field: &str, message: String) -> FileError {
    FileError::new("drive_format", message, serde_json::json!({"field": field}))
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl DriveFile {
    /// Open the drive folder `folder` (or a `drive.json` / `drive.bin` inside it):
    /// read the sidecar and check the layout. No sample is read.
    pub fn open(folder: &Path) -> Result<DriveFile, FileError> {
        let folder = match folder.file_name().and_then(|n| n.to_str()) {
            Some(BIN_NAME) | Some(JSON_NAME) => folder.parent().unwrap_or(folder).to_path_buf(),
            _ => folder.to_path_buf(),
        };
        let json_path = folder.join(JSON_NAME);
        let text = std::fs::read(&json_path).map_err(|e| unreadable(&json_path, e))?;
        let sidecar: Value =
            serde_json::from_slice(&text).map_err(|e| unreadable(&json_path, e))?;
        let format = sidecar.get("format").and_then(Value::as_str);
        if format != Some(FORMAT) {
            return Err(bad(
                "format",
                format!(
                    "The drive file's sidecar says format {}, not {FORMAT}.",
                    format.map_or("nothing".to_owned(), |f| format!("\"{f}\""))
                ),
            ));
        }
        let version = sidecar.get("version").and_then(Value::as_u64);
        if version != Some(VERSION) {
            return Err(FileError::new(
                "drive_version",
                format!(
                    "The drive file is version {}; this service reads version {VERSION}.",
                    version.map_or("unknown".to_owned(), |v| v.to_string())
                ),
                serde_json::json!({"version": sidecar.get("version"), "reads": VERSION}),
            ));
        }
        let rate_hz = sidecar
            .get("rate_hz")
            .and_then(Value::as_f64)
            .filter(|r| r.is_finite() && *r > 0.0)
            .ok_or_else(|| {
                bad(
                    "rate_hz",
                    "The drive file states no rate above 0 Hz.".into(),
                )
            })?;
        let samples = sidecar
            .get("samples")
            .and_then(Value::as_u64)
            .filter(|s| *s > 0)
            .ok_or_else(|| bad("samples", "The drive file states no samples.".into()))?;
        let list = sidecar
            .get("channels")
            .and_then(Value::as_array)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| bad("channels", "The drive file states no channels.".into()))?;
        let mut channels = Vec::with_capacity(list.len());
        for (k, c) in list.iter().enumerate() {
            let name = c
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let unit = c.get("unit").and_then(Value::as_str).unwrap_or("");
            if unit != "V" {
                return Err(FileError::new(
                    "drive_unit",
                    format!(
                        "Drive file channel {k} ({name}) is in \"{unit}\"; a drive file holds volts \
                         at the drive output (\"V\")."
                    ),
                    serde_json::json!({"channel": k, "name": name, "unit": unit}),
                ));
            }
            channels.push(DriveChannel {
                name,
                unit: unit.to_owned(),
                source: c.get("source").cloned().unwrap_or(Value::Null),
            });
        }
        let sha256 = sidecar
            .get("sha256")
            .and_then(Value::as_str)
            .filter(|s| is_hex64(s))
            .map(str::to_ascii_lowercase)
            .ok_or_else(|| {
                bad(
                    "sha256",
                    "The drive file's sidecar states no SHA-256 of drive.bin.".into(),
                )
            })?;
        let f = DriveFile {
            folder,
            rate_hz,
            samples,
            channels,
            sha256,
            sidecar,
        };
        f.check_layout()?;
        Ok(f)
    }

    pub fn bin_path(&self) -> PathBuf {
        self.folder.join(BIN_NAME)
    }

    /// The bytes `drive.bin` must hold: the magic and every sample.
    pub fn expected_bytes(&self) -> u64 {
        HEADER_BYTES + 4 * self.channels.len() as u64 * self.samples
    }

    /// The record's length, seconds.
    pub fn duration_s(&self) -> f64 {
        self.samples as f64 / self.rate_hz
    }

    /// The size against the sidecar, and the magic.
    fn check_layout(&self) -> Result<(), FileError> {
        let path = self.bin_path();
        let size = std::fs::metadata(&path)
            .map_err(|e| unreadable(&path, e))?
            .len();
        let want = self.expected_bytes();
        if size != want {
            return Err(FileError::new(
                "drive_size",
                format!(
                    "drive.bin holds {size} bytes; its sidecar's {} samples of {} channels need \
                     {want} (8 of header, then 4 per sample).",
                    self.samples,
                    self.channels.len()
                ),
                serde_json::json!({"bytes": size, "expected": want,
                                   "samples": self.samples, "channels": self.channels.len()}),
            ));
        }
        let mut head = [0u8; 8];
        File::open(&path)
            .and_then(|mut f| f.read_exact(&mut head))
            .map_err(|e| unreadable(&path, e))?;
        if &head != MAGIC {
            return Err(FileError::new(
                "drive_magic",
                "drive.bin does not begin with the drive file's magic (HFDRV01).",
                serde_json::json!({"found": head.iter().map(|b| format!("{b:02x}")).collect::<String>()}),
            ));
        }
        Ok(())
    }

    /// `drive.bin` opened at its first sample.
    pub fn samples_reader(&self) -> Result<File, FileError> {
        let path = self.bin_path();
        let mut f = File::open(&path).map_err(|e| unreadable(&path, e))?;
        let mut head = [0u8; 8];
        f.read_exact(&mut head).map_err(|e| unreadable(&path, e))?;
        Ok(f)
    }
}

/// Lowercase hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The SHA-256 of a file, read in pieces.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// Write a drive folder: for tests, and for a caller that makes a drive in memory.
/// `samples` is frame-interleaved (`frames x channels.len()`). The sidecar states the
/// SHA-256 of what was written; `extra` members (`source`, `fixes`, `tool`,
/// `created_utc`) are added as given.
pub fn write_folder(
    folder: &Path,
    rate_hz: f64,
    channels: &[&str],
    samples: &[f32],
    extra: serde_json::Map<String, Value>,
) -> io::Result<String> {
    std::fs::create_dir_all(folder)?;
    let width = channels.len().max(1);
    let mut bytes = Vec::with_capacity(8 + samples.len() * 4);
    bytes.extend_from_slice(MAGIC);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    let sha = hex(&Sha256::digest(&bytes));
    std::fs::write(folder.join(BIN_NAME), &bytes)?;
    let mut m = serde_json::Map::new();
    m.insert("format".into(), Value::from(FORMAT));
    m.insert("version".into(), Value::from(VERSION));
    m.insert("rate_hz".into(), Value::from(rate_hz));
    m.insert(
        "samples".into(),
        Value::from((samples.len() / width) as u64),
    );
    m.insert(
        "channels".into(),
        Value::Array(
            channels
                .iter()
                .map(|n| serde_json::json!({"name": n, "unit": "V", "source": n}))
                .collect(),
        ),
    );
    m.insert("sha256".into(), Value::from(sha.as_str()));
    for k in ["source", "fixes", "tool", "created_utc"] {
        m.insert(
            k.into(),
            extra.get(k).cloned().unwrap_or(match k {
                "fixes" => Value::Array(Vec::new()),
                "source" => Value::Null,
                _ => Value::from(""),
            }),
        );
    }
    std::fs::write(
        folder.join(JSON_NAME),
        serde_json::to_vec_pretty(&Value::Object(m))?,
    )?;
    Ok(sha)
}
