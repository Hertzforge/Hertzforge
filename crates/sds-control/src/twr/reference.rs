//! TWR references on the wire (schema/PROTOCOL.md, "TWR References"): reading a
//! record by the path of its file or as pasted text, keeping it in the local data
//! folder under its SHA-256, and preparing a stored record as a configuration would
//! for `twr.reference.preview`.
//!
//! # What the SHA-256 is of
//!
//! The record as read: its rate, its unit and its samples, in a fixed byte layout
//! ([`record_bytes`]). Not the file's bytes: one file read at two columns is two
//! records, and the same samples pasted or read from a file are one. A
//! configuration that names the file it came from is checked by reading that file
//! again with the options it was imported with and comparing the two hashes
//! (`reference_changed`).
//!
//! # What is kept
//!
//! `<data root>/twr/references/<sha256>.rec`, the record's bytes exactly as hashed,
//! and `<sha256>.json`, its facts and how it was read. The raw record is kept;
//! preparation is applied at configure, never to a stored record, so a preparation
//! can never compound.

use crate::codec::Arrays;
use crate::protocol::{num, ErrorCode, MethodError};
use crate::shock::view::{sig, time_axis, time_series};
use crate::views::{self, Axis, Role, Scale};
use hertzforge_twr::ReferenceWaveform;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The folder under the data root the references are kept in.
pub const REFERENCE_FOLDER: &str = "twr/references";

/// The longest record an import takes, samples. A section of it, on the record
/// grid, is held to the record cap at configure.
pub const MAX_IMPORT_SAMPLES: usize = 1 << 23;

/// The points the import's view is decimated to.
pub const IMPORT_POINTS: usize = 2048;

/// The largest step deviation a time column may have from its median step, a
/// fraction of the step.
const TIME_UNIFORMITY: f64 = 0.01;

/// The unit a record is in when none is named.
pub const DEFAULT_UNITS: &str = "g";

const MAGIC: &[u8] = b"hertzforge twr reference 1\n";

/// A stored record: its facts and its samples.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub sha256: String,
    /// The file it was read from; `None` for pasted text.
    pub path: Option<String>,
    pub name: String,
    /// `csv`, `tsv`, `wav`, `json` or `text`.
    pub format: &'static str,
    pub fs_hz: f64,
    pub units: String,
    pub samples: Vec<f64>,
    /// How it was read, so the file can be read again the same way.
    pub options: ImportOptions,
}

/// How a record was read from its file or text.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportOptions {
    /// The rate given; `None` when the record carries its own.
    pub fs_hz: Option<f64>,
    /// The data column (delimited) or channel (WAV), from 0.
    pub column: usize,
    /// The unit given; `None` for the default or the record's own.
    pub units: Option<String>,
}

impl Stored {
    pub fn duration_s(&self) -> f64 {
        self.samples.len() as f64 / self.fs_hz
    }

    pub fn peak(&self) -> f64 {
        self.samples.iter().fold(0.0f64, |m, &v| m.max(v.abs()))
    }

    pub fn rms(&self) -> f64 {
        hertzforge_dsp::stats::rms(&self.samples)
    }

    /// The facts on the wire.
    pub fn facts_json(&self) -> Value {
        json!({
            "sha256": self.sha256,
            "path": self.path,
            "name": self.name,
            "format": self.format,
            "fs_hz": num(sig(self.fs_hz, 9)),
            "samples": self.samples.len(),
            "duration_s": num(sig(self.duration_s(), 6)),
            "units": self.units,
            "peak": num(sig(self.peak(), 6)),
            "rms": num(sig(self.rms(), 6)),
        })
    }

    /// The record as the controller's reference waveform.
    pub fn waveform(&self) -> ReferenceWaveform {
        let mut w = ReferenceWaveform {
            label: self.name.clone(),
            fs_hz: self.fs_hz,
            samples: self.samples.clone(),
            eu: self.units.clone(),
            source: hertzforge_twr::reference::ImportSource::Delimited,
        };
        w.source = match self.format {
            "wav" => hertzforge_twr::reference::ImportSource::Wav,
            "json" => hertzforge_twr::reference::ImportSource::Json,
            _ => hertzforge_twr::reference::ImportSource::Delimited,
        };
        w
    }
}

/// The bytes a record's SHA-256 is of: a fixed header, the rate, the unit and the
/// samples, little-endian.
pub fn record_bytes(fs_hz: f64, units: &str, samples: &[f64]) -> Vec<u8> {
    let mut b = Vec::with_capacity(MAGIC.len() + 24 + units.len() + 8 * samples.len());
    b.extend_from_slice(MAGIC);
    b.extend_from_slice(&fs_hz.to_le_bytes());
    b.extend_from_slice(&(units.len() as u32).to_le_bytes());
    b.extend_from_slice(units.as_bytes());
    b.extend_from_slice(&(samples.len() as u64).to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    b
}

/// The SHA-256 of `bytes`, lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// The SHA-256 a record is kept under.
pub fn record_sha256(fs_hz: f64, units: &str, samples: &[f64]) -> String {
    sha256_hex(&record_bytes(fs_hz, units, samples))
}

/// Whether `s` is a SHA-256 in lowercase hex.
pub fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn take(b: &[u8], n: usize) -> Option<(&[u8], &[u8])> {
    (b.len() >= n).then(|| b.split_at(n))
}

fn parse_record(bytes: &[u8]) -> Option<(f64, String, Vec<f64>)> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let (fs, rest) = take(rest, 8)?;
    let fs_hz = f64::from_le_bytes(fs.try_into().ok()?);
    let (ul, rest) = take(rest, 4)?;
    let ul = u32::from_le_bytes(ul.try_into().ok()?) as usize;
    let (u, rest) = take(rest, ul)?;
    let units = std::str::from_utf8(u).ok()?.to_owned();
    let (n, rest) = take(rest, 8)?;
    let n = u64::from_le_bytes(n.try_into().ok()?) as usize;
    if rest.len() != n.checked_mul(8)? {
        return None;
    }
    let samples = rest
        .chunks_exact(8)
        .map(|c| f64::from_le_bytes(c.try_into().unwrap_or([0; 8])))
        .collect();
    Some((fs_hz, units, samples))
}

fn unreadable(message: impl Into<String>) -> MethodError {
    MethodError::named(ErrorCode::Refused, "reference_unreadable", message)
}

// ---------------------------------------------------------------------------------
// Reading a record.
// ---------------------------------------------------------------------------------

/// How the delimited text reads: the rate it gives (from a time column) and the
/// samples of the chosen column.
fn read_delimited(
    text: &str,
    fs_hz: Option<f64>,
    column: usize,
) -> Result<(f64, Vec<f64>), String> {
    let mut times = Vec::new();
    let mut values = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line
            .split(|c: char| c == ',' || c == '\t' || c == ';' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .collect();
        if fields.is_empty() {
            continue;
        }
        let parsed: Option<Vec<f64>> = fields.iter().map(|s| s.parse::<f64>().ok()).collect();
        let Some(nums) = parsed else {
            if values.is_empty() {
                // A header row before the data.
                continue;
            }
            return Err(format!(
                "line {}: a row that is not numbers after the data began",
                i + 1
            ));
        };
        let at = if fs_hz.is_some() { column } else { column + 1 };
        let Some(&v) = nums.get(at) else {
            return Err(if fs_hz.is_none() && nums.len() < 2 {
                format!(
                    "line {}: one column and no fs_hz; a record without a time column needs \
                     fs_hz",
                    i + 1
                )
            } else {
                format!(
                    "line {}: {} column{}, and data column {column} is not one of them",
                    i + 1,
                    nums.len(),
                    if nums.len() == 1 { "" } else { "s" }
                )
            });
        };
        if !v.is_finite() {
            return Err(format!("line {}: the value is not a finite number", i + 1));
        }
        if fs_hz.is_none() {
            times.push(nums[0]);
        }
        values.push(v);
        if values.len() > MAX_IMPORT_SAMPLES {
            return Err(format!(
                "the record is longer than {MAX_IMPORT_SAMPLES} samples"
            ));
        }
    }
    if values.len() < 2 {
        return Err("the text holds fewer than two samples".into());
    }
    let fs = match fs_hz {
        Some(f) => f,
        None => {
            let mut steps: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
            steps.sort_by(f64::total_cmp);
            let median = steps[steps.len() / 2];
            if median.is_nan() || median <= 0.0 {
                return Err("the time column does not increase".into());
            }
            let worst = steps
                .iter()
                .map(|s| (s - median).abs() / median)
                .fold(0.0f64, f64::max);
            if worst > TIME_UNIFORMITY {
                return Err(format!(
                    "the time column is not evenly spaced (a step is {}% off the median step)",
                    num(sig(100.0 * worst, 3))
                ));
            }
            1.0 / median
        }
    };
    Ok((fs, values))
}

/// What an import asks for, from its parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportRequest {
    pub source: Source,
    pub options: ImportOptions,
    pub name: Option<String>,
}

/// Where a record comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Path(String),
    Text(String),
}

/// Parse `twr.reference.import`'s parameters.
pub fn parse_import(params: &Map<String, Value>) -> Result<ImportRequest, MethodError> {
    let s = |k: &str| crate::protocol::named_str(params, k);
    let source =
        match (s("path")?, s("text")?) {
            (Some(p), None) => {
                if !Path::new(p).is_absolute() {
                    return Err(MethodError::bad_params(
                        "path",
                        "Give the reference file's full path on this computer.",
                    ));
                }
                Source::Path(p.to_owned())
            }
            (None, Some(t)) => Source::Text(t.to_owned()),
            _ => return Err(MethodError::bad_params(
                "path",
                "An import names a file by its \"path\" or gives the record as \"text\", one of \
                 the two.",
            )),
        };
    let fs_hz = crate::protocol::named_f64(params, "fs_hz")?;
    if let Some(f) = fs_hz {
        if f.is_nan() || f <= 0.0 {
            return Err(MethodError::bad_params("fs_hz", "\"fs_hz\" is above 0."));
        }
    }
    let column = match params.get("column") {
        None | Some(Value::Null) => 0,
        Some(v) => v.as_u64().filter(|n| *n < 4096).ok_or_else(|| {
            MethodError::bad_params("column", "\"column\" is a whole number from 0.")
        })? as usize,
    };
    let units = s("units")?.map(str::to_owned);
    if units.as_deref().is_some_and(|u| u.trim().is_empty()) {
        return Err(MethodError::bad_params("units", "\"units\" names a unit."));
    }
    let name = s("name")?.map(str::to_owned);
    Ok(ImportRequest {
        source,
        options: ImportOptions {
            fs_hz,
            column,
            units,
        },
        name,
    })
}

fn format_of(path: Option<&Path>, bytes: &[u8]) -> &'static str {
    let ext = path
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("wav") => return "wav",
        Some("json") => return "json",
        Some("csv") => return "csv",
        Some("tsv") | Some("tab") => return "tsv",
        _ => {}
    }
    if bytes.starts_with(b"RIFF") {
        return "wav";
    }
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(256)]);
    if text.trim_start().starts_with('{') {
        return "json";
    }
    if path.is_none() {
        return "text";
    }
    if text.contains('\t') {
        "tsv"
    } else {
        "csv"
    }
}

/// Read a record from `bytes` (the file at `path`, or pasted text) as `opts` say.
pub fn read(
    bytes: &[u8],
    path: Option<&Path>,
    opts: &ImportOptions,
) -> Result<(&'static str, f64, String, Vec<f64>), String> {
    let format = format_of(path, bytes);
    let given_units = opts.units.clone();
    let (fs, units, samples) = match format {
        "wav" => {
            let w =
                ReferenceWaveform::from_wav_bytes(bytes, opts.column).map_err(|e| e.to_string())?;
            if let Some(f) = opts.fs_hz {
                if (f - w.fs_hz).abs() > 1e-9 {
                    return Err(format!(
                        "the file states {} Hz, not the fs_hz given ({} Hz)",
                        num(w.fs_hz),
                        num(f)
                    ));
                }
            }
            (
                w.fs_hz,
                given_units.unwrap_or_else(|| DEFAULT_UNITS.into()),
                w.samples,
            )
        }
        "json" => {
            let text = std::str::from_utf8(bytes).map_err(|_| "the file is not UTF-8 text")?;
            let w = ReferenceWaveform::from_json(text).map_err(|e| e.to_string())?;
            if let Some(u) = &given_units {
                if u != &w.eu {
                    return Err(format!(
                        "the record states its unit, {}, not the units given ({u})",
                        w.eu
                    ));
                }
            }
            if let Some(f) = opts.fs_hz {
                if (f - w.fs_hz).abs() > 1e-9 {
                    return Err(format!(
                        "the record states {} Hz, not the fs_hz given ({} Hz)",
                        num(w.fs_hz),
                        num(f)
                    ));
                }
            }
            (w.fs_hz, w.eu, w.samples)
        }
        _ => {
            let text = std::str::from_utf8(bytes).map_err(|_| "the text is not UTF-8")?;
            let (fs, samples) = read_delimited(text, opts.fs_hz, opts.column)?;
            (
                fs,
                given_units.unwrap_or_else(|| DEFAULT_UNITS.into()),
                samples,
            )
        }
    };
    if samples.len() > MAX_IMPORT_SAMPLES {
        return Err(format!(
            "the record is longer than {MAX_IMPORT_SAMPLES} samples"
        ));
    }
    if samples.len() < 2 {
        return Err("the record holds fewer than two samples".into());
    }
    if let Some(i) = samples.iter().position(|v| !v.is_finite()) {
        return Err(format!("sample {i} is not a finite number"));
    }
    if !(fs.is_finite() && fs > 0.0) {
        return Err("the record's rate is not above 0".into());
    }
    Ok((format, fs, units, samples))
}

fn folder(root: &Path) -> PathBuf {
    root.join(REFERENCE_FOLDER)
}

fn sidecar_json(s: &Stored) -> Value {
    json!({
        "sha256": s.sha256,
        "path": s.path,
        "name": s.name,
        "format": s.format,
        "fs_hz": s.fs_hz,
        "units": s.units,
        "samples": s.samples.len(),
        "options": {
            "fs_hz": s.options.fs_hz,
            "column": s.options.column,
            "units": s.options.units,
        },
    })
}

fn static_format(f: &str) -> &'static str {
    match f {
        "csv" => "csv",
        "tsv" => "tsv",
        "wav" => "wav",
        "json" => "json",
        _ => "text",
    }
}

/// Import a record and keep it under the data root. Blocking (it reads a file and
/// writes two).
pub fn import(root: &Path, req: &ImportRequest) -> Result<Stored, MethodError> {
    let (bytes, path) = match &req.source {
        Source::Path(p) => {
            let bytes = std::fs::read(p).map_err(|e| {
                unreadable(format!(
                    "{p} could not be read ({e}). Check the path and that the file is readable."
                ))
            })?;
            (bytes, Some(PathBuf::from(p)))
        }
        Source::Text(t) => (t.clone().into_bytes(), None),
    };
    let (format, fs_hz, units, samples) =
        read(&bytes, path.as_deref(), &req.options).map_err(|why| {
            unreadable(match &path {
                Some(p) => format!(
                    "{} is not a record this service can read: {why}.",
                    p.display()
                ),
                None => format!("The text is not a record this service can read: {why}."),
            })
        })?;
    let name = req.name.clone().unwrap_or_else(|| match &path {
        Some(p) => p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("reference")
            .to_owned(),
        None => "pasted".into(),
    });
    let bytes = record_bytes(fs_hz, &units, &samples);
    let sha256 = sha256_hex(&bytes);
    let stored = Stored {
        sha256: sha256.clone(),
        path: path.map(|p| p.display().to_string()),
        name,
        format,
        fs_hz,
        units,
        samples,
        options: req.options.clone(),
    };
    let dir = folder(root);
    std::fs::create_dir_all(&dir).map_err(|e| {
        MethodError::internal(format!(
            "the reference could not be kept in {} ({e})",
            dir.display()
        ))
    })?;
    let rec = dir.join(format!("{sha256}.rec"));
    if !rec.exists() {
        write_atomic(&rec, &bytes)?;
    }
    let side = serde_json::to_vec_pretty(&sidecar_json(&stored))
        .map_err(|e| MethodError::internal(format!("the reference's facts: {e}")))?;
    write_atomic(&dir.join(format!("{sha256}.json")), &side)?;
    Ok(stored)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), MethodError> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            MethodError::internal(format!("{} could not be written ({e})", path.display()))
        })
}

/// The stored record with this SHA-256, or `None` when none is kept (or what is
/// kept no longer hashes to it).
pub fn load(root: &Path, sha256: &str) -> Option<Stored> {
    if !is_sha256(sha256) {
        return None;
    }
    let dir = folder(root);
    let bytes = std::fs::read(dir.join(format!("{sha256}.rec"))).ok()?;
    if sha256_hex(&bytes) != sha256 {
        return None;
    }
    let (fs_hz, units, samples) = parse_record(&bytes)?;
    let side: Value = std::fs::read(dir.join(format!("{sha256}.json")))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    let o = &side["options"];
    Some(Stored {
        sha256: sha256.to_owned(),
        path: side["path"].as_str().map(str::to_owned),
        name: side["name"].as_str().unwrap_or("reference").to_owned(),
        format: static_format(side["format"].as_str().unwrap_or("text")),
        fs_hz,
        units,
        samples,
        options: ImportOptions {
            fs_hz: o["fs_hz"].as_f64(),
            column: o["column"].as_u64().unwrap_or(0) as usize,
            units: o["units"].as_str().map(str::to_owned),
        },
    })
}

/// Whether the file at `path` still reads, with `stored`'s options, to the record
/// kept under `stored.sha256`. `Err` carries the words for the refusal.
pub fn still_matches(stored: &Stored, path: &str) -> Result<(), String> {
    let p = Path::new(path);
    let bytes = std::fs::read(p).map_err(|e| format!("{path} could not be read ({e})"))?;
    let (_, fs, units, samples) = read(&bytes, Some(p), &stored.options)
        .map_err(|why| format!("{path} no longer reads as a record: {why}"))?;
    let now = record_sha256(fs, &units, &samples);
    if now == stored.sha256 {
        Ok(())
    } else {
        Err(format!(
            "{path} now reads to a record with SHA-256 {}, not the one configured; import it \
             again to use what it holds now",
            &now[..12]
        ))
    }
}

/// The `twr.reference.import` result: the facts, the warnings and the Reference
/// view.
pub fn import_json(s: &Stored, arrays: &mut Arrays) -> Value {
    let n = s.samples.len();
    let view = views::spectrum(
        arrays,
        "Reference",
        &Axis::new("Time", Some("s"), Scale::Linear),
        &Axis::new("Reference", Some(&s.units), Scale::Linear),
        vec![time_series(
            "Reference",
            Role::Reference,
            time_axis(0, n, s.fs_hz),
            &s.samples,
            Some(IMPORT_POINTS),
        )],
        Some("time"),
    );
    json!({"reference": s.facts_json(), "warnings": [], "views": [view]})
}

/// A pasted record, read as the import reads it.
pub fn read_text(text: &str, opts: &ImportOptions) -> Result<(f64, String, Vec<f64>), String> {
    read(text.as_bytes(), None, opts).map(|(_, fs, u, s)| (fs, u, s))
}

// ---------------------------------------------------------------------------------
// Preparing a stored record as a run would.
// ---------------------------------------------------------------------------------

/// A stored record prepared as a configuration prepares it: the record every play
/// plays, its section, its spectrum, its kinematics and what the trusted bins can
/// cover of it before identification.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub grid: hertzforge_twr::RecordGrid,
    /// The prepared reference (after resampling, detrend, high-pass, scale and
    /// taper), before its section is taken.
    pub prepared: Vec<f64>,
    /// The section's first sample in the prepared reference.
    pub section_from: usize,
    /// The section at full level.
    pub section: Vec<f64>,
    /// The full-level record: the section between its pads.
    pub target: Vec<f64>,
    /// `abs(R)` on the record grid's bins.
    pub spectrum: Vec<f64>,
    pub kinematics: hertzforge_twr::reference::WaveformKinematics,
    pub feasibility: super::config::Feasibility,
    pub coverage: f64,
    pub band_hz: (f64, f64),
    pub ref_floor_db: f64,
    pub kinematics_hp_hz: f64,
    pub units: String,
}

fn item(reason: &'static str, field: &str, message: impl Into<String>) -> crate::table::Item {
    crate::table::Item::field(reason, field, message)
}

/// The feasibility verdict in words, from the core's report.
pub fn feasibility(
    report: &hertzforge_twr::FeasibilityReport,
    any: bool,
) -> super::config::Feasibility {
    let wire = |name: &str| -> String {
        match name {
            "peak acceleration" => "peak_accel".into(),
            "peak velocity (ips)" => "peak_velocity_ips".into(),
            "displacement pk-pk (in)" => "disp_pkpk_in".into(),
            other => other.to_owned(),
        }
    };
    let checks: Vec<(String, f64, f64, bool)> = report
        .checks
        .iter()
        .map(|c| (wire(&c.name), c.value, c.limit, c.pass))
        .collect();
    if !any || checks.is_empty() {
        return super::config::Feasibility {
            verdict: "not_evaluated",
            message: "No kinematic ceiling is set, so the section was not checked against one."
                .into(),
            checks,
        };
    }
    if report.blocked {
        let over: Vec<String> = report
            .checks
            .iter()
            .filter(|c| !c.pass)
            .map(|c| {
                format!(
                    "{} {} over {}",
                    c.name,
                    num(sig(c.value, 4)),
                    num(sig(c.limit, 4))
                )
            })
            .collect();
        return super::config::Feasibility {
            verdict: "exceeded",
            message: format!(
                "The section at full level exceeds its kinematic ceilings: {}.",
                over.join(", ")
            ),
            checks,
        };
    }
    super::config::Feasibility {
        verdict: "pass",
        message: "The section at full level is within its kinematic ceilings.".into(),
        checks,
    }
}

/// Prepare `stored` as `shape` asks, at `fs_hz`, with the correction band `band`.
/// The order and every operation are the core's own (resample, detrend,
/// high-pass, scale, taper, then the section on its padded grid), so what is shown
/// is what the run plays.
pub fn prepare(
    stored: &Stored,
    shape: &super::config::WireShape,
    fs_hz: f64,
    band: (f64, f64),
) -> Result<Prepared, crate::table::Item> {
    use hertzforge_twr::grid::{rfft_half, RecordGrid};
    let p = &shape.prep;
    let mut r = stored.waveform();
    if (r.fs_hz - fs_hz).abs() > 1e-9 {
        r = r
            .resample(fs_hz)
            .map_err(|e| item("rate_mismatch", "acquisition.fs_hz", e.to_string()))?;
    }
    if let Some(linear) = p.detrend.core() {
        r = r.detrend(linear);
    }
    if let Some(hp) = p.highpass_hz {
        r = r
            .highpass(hp)
            .map_err(|e| item("out_of_range", "prep.highpass_hz", e.to_string()))?;
    }
    if (p.scale - 1.0).abs() > 0.0 {
        r = r
            .scale(p.scale)
            .map_err(|e| item("out_of_range", "prep.scale", e.to_string()))?;
    }
    if let Some(t) = p.taper_frac {
        r = r
            .taper(t)
            .map_err(|e| item("out_of_range", "prep.taper_frac", e.to_string()))?;
    }
    let n = r.samples.len();
    let (from, len) = shape.section.unwrap_or((0, n));
    if len == 0 || from.checked_add(len).is_none_or(|end| end > n) {
        return Err(item(
            "section_outside_reference",
            "section.len",
            format!(
                "The section runs from sample {from} for {len} samples; the prepared reference \
                 has {n}."
            ),
        ));
    }
    let (pre, post) = shape.pads;
    let total = len
        .checked_add(pre)
        .and_then(|t| t.checked_add(post))
        .filter(|t| *t <= hertzforge_twr::control::MAX_RECORD_LEN)
        .ok_or_else(|| {
            item(
                "record_too_long",
                "section.len",
                format!(
                    "The section and its pads are {} samples; the most a run takes is {} on \
                     its record.",
                    len.saturating_add(pre).saturating_add(post),
                    hertzforge_twr::control::MAX_RECORD_LEN
                ),
            )
        })?;
    let _ = total;
    let section = r.samples[from..from + len].to_vec();
    let grid = RecordGrid::new(fs_hz, len, pre, post);
    let target = grid.window_from_section(&section);
    let half = rfft_half(&target, grid.n);
    let spectrum: Vec<f64> = half
        .iter()
        .map(|&(re, im)| (re * re + im * im).sqrt())
        .collect();
    let coverage = coverage_of(&half, &grid, band, shape.correction.ref_floor_db);
    let f = &shape.feasibility;
    let kinematics = ReferenceWaveform::from_samples("section", fs_hz, section.clone())
        .map(|w| w.kinematics(f.kinematics_hp_hz))
        .map_err(|e| item("section_outside_reference", "section.len", e.to_string()))?;
    let feasibility = feasibility(&f.ceilings().assess(&kinematics), f.any());
    Ok(Prepared {
        grid,
        prepared: r.samples,
        section_from: from,
        section,
        target,
        spectrum,
        kinematics,
        feasibility,
        coverage,
        band_hz: band,
        ref_floor_db: shape.correction.ref_floor_db,
        kinematics_hp_hz: f.kinematics_hp_hz,
        units: stored.units.clone(),
    })
}

/// The fraction of the record's spectral energy inside `band` and within
/// `floor_db` of its in-band peak: the core's band and reference-floor gates, before
/// identification adds its coherence gate.
pub fn coverage_of(
    half: &[(f64, f64)],
    grid: &hertzforge_twr::RecordGrid,
    band: (f64, f64),
    floor_db: f64,
) -> f64 {
    let inside = |k: usize| {
        let f = grid.freq(k);
        f >= band.0 && f <= band.1
    };
    let mag = |c: (f64, f64)| (c.0 * c.0 + c.1 * c.1).sqrt();
    let peak = (0..half.len())
        .filter(|&k| inside(k))
        .map(|k| mag(half[k]))
        .fold(0.0f64, f64::max);
    let floor = peak * 10f64.powf(-floor_db / 20.0);
    let (mut total, mut covered) = (0.0f64, 0.0f64);
    for (k, &c) in half.iter().enumerate() {
        let e = c.0 * c.0 + c.1 * c.1;
        total += e;
        if inside(k) && mag(c) >= floor {
            covered += e;
        }
    }
    if total > 0.0 {
        covered / total
    } else {
        0.0
    }
}

impl Prepared {
    /// What a preview computes from it.
    pub fn derived(&self, _shape: &super::config::WireShape) -> super::config::PreviewDerived {
        let s = &self.section;
        super::config::PreviewDerived {
            grid: super::config::Grid {
                fs_hz: self.grid.fs_hz,
                record_len: self.grid.n,
                section_start: self.grid.pad_pre,
                section_len: self.grid.section_len,
                actual: false,
            },
            section_peak: s.iter().fold(0.0f64, |m, &v| m.max(v.abs())),
            section_rms: hertzforge_dsp::stats::rms(s),
            section_s: s.len() as f64 / self.grid.fs_hz,
            peak_accel: self.kinematics.peak_accel,
            peak_velocity_ips: self.kinematics.peak_velocity_ips,
            disp_pkpk_in: self.kinematics.disp_pkpk_in,
            kinematics_hp_hz: self.kinematics_hp_hz,
            band_hz: self.band_hz,
            ref_floor_db: self.ref_floor_db,
            coverage: self.coverage,
            feasibility: self.feasibility.clone(),
        }
    }

    /// The figures the preview's views are laid out from: the record, its
    /// spectrum from DC to half the rate, the reference floor, and the velocity
    /// and displacement integrated after the kinematics high-pass.
    pub fn figures(&self) -> PreviewFigures {
        let fs = self.grid.fs_hz;
        let freqs_hz: Vec<f64> = (0..self.spectrum.len())
            .map(|k| self.grid.freq(k))
            .collect();
        let peak = (0..self.spectrum.len())
            .filter(|&k| freqs_hz[k] >= self.band_hz.0 && freqs_hz[k] <= self.band_hz.1)
            .map(|k| self.spectrum[k])
            .fold(0.0f64, f64::max);
        let hp = self.kinematics_hp_hz;
        let velocity = hertzforge_dsp::integrate::accel_to_velocity(&self.target, fs, hp);
        let displacement = hertzforge_dsp::integrate::velocity_to_displacement(&velocity, fs, hp);
        PreviewFigures {
            fs_hz: fs,
            units: self.units.clone(),
            target: self.target.clone(),
            freqs_hz,
            spectrum: self.spectrum.clone(),
            band_hz: self.band_hz,
            floor: peak * 10f64.powf(-self.ref_floor_db / 20.0),
            velocity,
            displacement,
        }
    }

    /// The preview's views, each min/max-decimated to `points`.
    pub fn views(&self, points: usize, arrays: &mut Arrays) -> Vec<Value> {
        preview_views(&self.figures(), Some(points), arrays)
    }
}

/// What a preview's views show.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewFigures {
    pub fs_hz: f64,
    pub units: String,
    /// The full-level record, from its first sample.
    pub target: Vec<f64>,
    pub freqs_hz: Vec<f64>,
    /// `abs(R)` on `freqs_hz`.
    pub spectrum: Vec<f64>,
    pub band_hz: (f64, f64),
    /// The reference floor, `abs(R)`.
    pub floor: f64,
    pub velocity: Vec<f64>,
    pub displacement: Vec<f64>,
}

/// The preview's views: Section (the full-level record, time), Spectrum
/// (`abs(R)` on the record grid, with the reference floor across the correction
/// band), Velocity and Displacement (time), each min/max-decimated to `points`.
pub fn preview_views(f: &PreviewFigures, points: Option<usize>, arrays: &mut Arrays) -> Vec<Value> {
    let n = f.target.len();
    let t = || time_axis(0, n, f.fs_hz);
    let time_x = Axis::new("Time", Some("s"), Scale::Linear);
    let section = views::spectrum(
        arrays,
        "Section",
        &time_x,
        &Axis::new("Reference", Some(&f.units), Scale::Linear),
        vec![time_series(
            "Reference",
            Role::Reference,
            t(),
            &f.target,
            points,
        )],
        Some("time"),
    );
    let spectrum = views::spectrum(
        arrays,
        "Spectrum",
        &Axis::new("Frequency", Some("Hz"), Scale::Linear),
        &Axis::new("Magnitude", Some(&f.units), Scale::Log),
        vec![
            time_series(
                "Reference",
                Role::Reference,
                f.freqs_hz.clone(),
                &f.spectrum,
                points,
            ),
            crate::views::Series::new(
                "Reference floor",
                Role::Tolerance,
                vec![f.band_hz.0, f.band_hz.1],
                vec![f.floor, f.floor],
            ),
        ],
        Some("frequency"),
    );
    let velocity = views::spectrum(
        arrays,
        "Velocity",
        &time_x,
        &Axis::new("Velocity", Some("in/s"), Scale::Linear),
        vec![time_series(
            "Velocity",
            Role::Reference,
            t(),
            &f.velocity,
            points,
        )],
        Some("time"),
    );
    let displacement = views::spectrum(
        arrays,
        "Displacement",
        &time_x,
        &Axis::new("Displacement", Some("in"), Scale::Linear),
        vec![time_series(
            "Displacement",
            Role::Reference,
            t(),
            &f.displacement,
            points,
        )],
        Some("time"),
    );
    vec![section, spectrum, velocity, displacement]
}
