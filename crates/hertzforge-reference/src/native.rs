//! The native reference file: a binary payload and a JSON sidecar in one folder.
//!
//! It is the same family as the controller's other run records: the numbers in a
//! headerless-to-JSON little-endian binary file, everything a reader needs to
//! interpret them in a JSON sidecar written last. The sidecar is a capture
//! [`Snapshot`] envelope (`unix_secs`, `settings_json`, `marks`, `payload`) whose
//! payload is a [`ReferenceSidecar`].
//!
//! ```text
//! <folder>/
//!   reference.bin    the content, little-endian, laid out below
//!   reference.json   the sidecar
//! ```
//!
//! `reference.bin`, every field 8-byte aligned:
//!
//! | Field | Type |
//! |---|---|
//! | magic `HFREF01` and a zero byte | 8 bytes |
//! | lines, n, band low, band high | 4 x u64 |
//! | line spacing in Hz | f64 |
//! | flags: bit 0 a response transformation, bit 1 a drive transformation | u64 |
//! | unit | string |
//! | the `n` reference labels | `n` strings |
//! | response transformation, when flagged: its lines, rows (= `n`), columns, then a label per column | 3 x u64, strings |
//! | drive transformation, when flagged: its lines, rows, columns, then a label per row and per column | 3 x u64, strings |
//! | the SDM, packed upper triangle per line, row-major | `lines * n (n + 1) / 2` complex |
//! | alarm upper, alarm lower, abort upper, abort lower, line-major (`NaN` for none) | 4 x `lines * n` f64 |
//! | response transformation values, row-major per line | complex |
//! | drive transformation values, row-major per line | complex |
//!
//! A string is a u64 byte count, the UTF-8 bytes, and zero bytes to the next
//! multiple of 8. A complex value is two f64, real then imaginary.
//!
//! The content hash is the SHA-256 of `reference.bin`, written `sha256:<hex>`.
//! It covers the reference and nothing about how it was made (the provenance lives
//! in the sidecar only), so two routes to the same reference give the same hash.

use crate::{Provenance, Reference, ReferenceError, Transformation, C64};
use hertzforge_capture::Snapshot;
use hertzforge_sdm::{HermitianStack, MatrixStack};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The binary payload's file name inside the folder.
pub const BINARY_FILE: &str = "reference.bin";
/// The sidecar's file name inside the folder.
pub const SIDECAR_FILE: &str = "reference.json";
/// The `format` the sidecar names.
pub const FORMAT: &str = "hertzforge-reference";
/// The format version this crate writes and reads.
pub const FORMAT_VERSION: u32 = 1;
/// The first eight bytes of `reference.bin`.
pub const MAGIC: &[u8; 8] = b"HFREF01\0";
/// The spectral convention the file's values are in.
pub const CONVENTION: &str = "G = E[X X^H] per line";

/// The sidecar payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReferenceSidecar {
    pub format: String,
    pub format_version: u32,
    pub binary_file: String,
    pub binary_bytes: u64,
    /// `sha256:<hex>` of the binary file.
    pub content_hash: String,
    pub convention: String,
    pub lines: usize,
    pub n: usize,
    pub df_hz: f64,
    pub band: [usize; 2],
    pub labels: Vec<String>,
    pub unit: String,
    /// `physical` or `virtual`.
    pub space: String,
    /// `[lines, rows, cols]` of the response transformation, when present.
    #[serde(default)]
    pub response_transformation: Option<[usize; 3]>,
    /// `[lines, rows, cols]` of the drive transformation, when present.
    #[serde(default)]
    pub drive_transformation: Option<[usize; 3]>,
    pub provenance: Provenance,
}

/// `sha256:<hex>` of some bytes.
pub fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(7 + 64);
    s.push_str("sha256:");
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

struct Writer(Vec<u8>);

impl Writer {
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn c64(&mut self, v: C64) {
        self.f64(v.re);
        self.f64(v.im);
    }
    fn string(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.0.extend_from_slice(s.as_bytes());
        while !self.0.len().is_multiple_of(8) {
            self.0.push(0);
        }
    }
}

/// The binary encoding of a reference (the content of `reference.bin`).
pub fn encode(r: &Reference) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.0.extend_from_slice(MAGIC);
    w.u64(r.lines() as u64);
    w.u64(r.n() as u64);
    w.u64(r.band.0 as u64);
    w.u64(r.band.1 as u64);
    w.f64(r.df_hz);
    let flags = u64::from(r.response_transformation.is_some())
        | (u64::from(r.drive_transformation.is_some()) << 1);
    w.u64(flags);
    w.string(&r.unit);
    for l in &r.labels {
        w.string(l);
    }
    if let Some(t) = &r.response_transformation {
        let (tl, rows, cols) = t.matrix.dims();
        w.u64(tl as u64);
        w.u64(rows as u64);
        w.u64(cols as u64);
        for l in &t.col_labels {
            w.string(l);
        }
    }
    if let Some(t) = &r.drive_transformation {
        let (tl, rows, cols) = t.matrix.dims();
        w.u64(tl as u64);
        w.u64(rows as u64);
        w.u64(cols as u64);
        for l in t.row_labels.iter().chain(&t.col_labels) {
            w.string(l);
        }
    }
    for &v in r.sdm.data() {
        w.c64(v);
    }
    for v in [
        &r.tolerances.alarm_upper,
        &r.tolerances.alarm_lower,
        &r.tolerances.abort_upper,
        &r.tolerances.abort_lower,
    ] {
        for &x in v.iter() {
            w.f64(x);
        }
    }
    for t in [&r.response_transformation, &r.drive_transformation]
        .into_iter()
        .flatten()
    {
        for &v in t.matrix.data() {
            w.c64(v);
        }
    }
    w.0
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

fn corrupt(what: &str) -> ReferenceError {
    ReferenceError::File(format!(
        "the reference file is damaged or truncated ({what}); write it again from its source"
    ))
}

impl Reader<'_> {
    fn take(&mut self, n: usize, what: &str) -> Result<&[u8], ReferenceError> {
        let end = self.at.checked_add(n).ok_or_else(|| corrupt(what))?;
        if end > self.b.len() {
            return Err(corrupt(what));
        }
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u64(&mut self, what: &str) -> Result<u64, ReferenceError> {
        let s = self.take(8, what)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Ok(u64::from_le_bytes(a))
    }
    fn usize(&mut self, what: &str) -> Result<usize, ReferenceError> {
        usize::try_from(self.u64(what)?).map_err(|_| corrupt(what))
    }
    fn f64(&mut self, what: &str) -> Result<f64, ReferenceError> {
        Ok(f64::from_bits(self.u64(what)?))
    }
    fn c64(&mut self, what: &str) -> Result<C64, ReferenceError> {
        Ok(C64::new(self.f64(what)?, self.f64(what)?))
    }
    fn string(&mut self, what: &str) -> Result<String, ReferenceError> {
        let len = self.usize(what)?;
        let bytes = self.take(len, what)?.to_vec();
        let pad = (8 - len % 8) % 8;
        self.take(pad, what)?;
        String::from_utf8(bytes).map_err(|_| corrupt(what))
    }
    fn f64s(&mut self, count: usize, what: &str) -> Result<Vec<f64>, ReferenceError> {
        // Refuse a count the remaining bytes cannot hold before allocating.
        if count.saturating_mul(8) > self.b.len() - self.at {
            return Err(corrupt(what));
        }
        (0..count).map(|_| self.f64(what)).collect()
    }
    fn c64s(&mut self, count: usize, what: &str) -> Result<Vec<C64>, ReferenceError> {
        if count.saturating_mul(16) > self.b.len() - self.at {
            return Err(corrupt(what));
        }
        (0..count).map(|_| self.c64(what)).collect()
    }
}

/// Decode `reference.bin` into a reference carrying `provenance`. The result is
/// not yet validated.
pub fn decode(bytes: &[u8], provenance: Provenance) -> Result<Reference, ReferenceError> {
    let mut r = Reader { b: bytes, at: 0 };
    if r.take(8, "magic")? != MAGIC {
        return Err(ReferenceError::File(
            "this is not a reference file of a version this software reads (the first bytes \
             are not the reference magic); open it with the software that wrote it"
                .into(),
        ));
    }
    let lines = r.usize("lines")?;
    let n = r.usize("order")?;
    let band = (r.usize("band")?, r.usize("band")?);
    let df_hz = r.f64("line spacing")?;
    let flags = r.u64("flags")?;
    if flags & !3 != 0 {
        return Err(corrupt("unknown flags"));
    }
    let unit = r.string("unit")?;
    let labels = (0..n)
        .map(|_| r.string("labels"))
        .collect::<Result<Vec<_>, _>>()?;
    let resp_dims = if flags & 1 != 0 {
        let d = (
            r.usize("response dims")?,
            r.usize("response dims")?,
            r.usize("response dims")?,
        );
        let cols = (0..d.2)
            .map(|_| r.string("response labels"))
            .collect::<Result<Vec<_>, _>>()?;
        Some((d, cols))
    } else {
        None
    };
    let drive_dims = if flags & 2 != 0 {
        let d = (
            r.usize("drive dims")?,
            r.usize("drive dims")?,
            r.usize("drive dims")?,
        );
        let rows = (0..d.1)
            .map(|_| r.string("drive labels"))
            .collect::<Result<Vec<_>, _>>()?;
        let cols = (0..d.2)
            .map(|_| r.string("drive labels"))
            .collect::<Result<Vec<_>, _>>()?;
        Some((d, rows, cols))
    } else {
        None
    };
    let packed = lines
        .checked_mul(HermitianStack::packed_len(n))
        .ok_or_else(|| corrupt("order"))?;
    let sdm = HermitianStack::from_packed(lines, n, r.c64s(packed, "spectral matrix")?)?;
    let ln = lines.checked_mul(n).ok_or_else(|| corrupt("order"))?;
    let tolerances = crate::Tolerances {
        alarm_upper: r.f64s(ln, "tolerances")?,
        alarm_lower: r.f64s(ln, "tolerances")?,
        abort_upper: r.f64s(ln, "tolerances")?,
        abort_lower: r.f64s(ln, "tolerances")?,
    };
    let mut stack = |d: (usize, usize, usize), what: &str| -> Result<MatrixStack, ReferenceError> {
        let count =
            d.0.checked_mul(d.1)
                .and_then(|x| x.checked_mul(d.2))
                .ok_or_else(|| corrupt(what))?;
        Ok(MatrixStack::from_vec(d.0, d.1, d.2, r.c64s(count, what)?)?)
    };
    let response_transformation = match resp_dims {
        Some((d, cols)) => Some(Transformation {
            matrix: stack(d, "response transformation")?,
            row_labels: labels.clone(),
            col_labels: cols,
        }),
        None => None,
    };
    let drive_transformation = match drive_dims {
        Some((d, rows, cols)) => Some(Transformation {
            matrix: stack(d, "drive transformation")?,
            row_labels: rows,
            col_labels: cols,
        }),
        None => None,
    };
    if r.at != bytes.len() {
        return Err(corrupt("bytes after the last array"));
    }
    Ok(Reference {
        sdm,
        df_hz,
        band,
        labels,
        unit,
        tolerances,
        response_transformation,
        drive_transformation,
        provenance,
    })
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

fn io_err(path: &Path, e: std::io::Error) -> ReferenceError {
    ReferenceError::File(format!(
        "{}: {e}; check the path and that the folder can be written",
        path.display()
    ))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ReferenceError> {
    let tmp = path.with_extension("partial");
    std::fs::write(&tmp, bytes).map_err(|e| io_err(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| io_err(path, e))
}

/// The sidecar describing `r` and its encoding.
pub fn sidecar(r: &Reference, binary: &[u8]) -> ReferenceSidecar {
    ReferenceSidecar {
        format: FORMAT.into(),
        format_version: FORMAT_VERSION,
        binary_file: BINARY_FILE.into(),
        binary_bytes: binary.len() as u64,
        content_hash: content_hash(binary),
        convention: CONVENTION.into(),
        lines: r.lines(),
        n: r.n(),
        df_hz: r.df_hz,
        band: [r.band.0, r.band.1],
        labels: r.labels.clone(),
        unit: r.unit.clone(),
        space: if r.is_virtual() {
            "virtual"
        } else {
            "physical"
        }
        .into(),
        response_transformation: r.response_transformation.as_ref().map(|t| {
            let d = t.matrix.dims();
            [d.0, d.1, d.2]
        }),
        drive_transformation: r.drive_transformation.as_ref().map(|t| {
            let d = t.matrix.dims();
            [d.0, d.1, d.2]
        }),
        provenance: r.provenance.clone(),
    }
}

/// Validate `r` and write it into the folder `dir` (created if needed): the
/// binary first, the sidecar last, each through a temporary file and a rename.
/// Returns the content hash.
pub fn write(r: &Reference, dir: &Path) -> Result<String, ReferenceError> {
    r.validate()?;
    std::fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
    let binary = encode(r);
    let side = sidecar(r, &binary);
    let snapshot = Snapshot {
        unix_secs: r.provenance.created_unix_secs,
        settings_json: String::new(),
        marks: Vec::new(),
        payload: side,
    };
    let json = serde_json::to_string_pretty(&snapshot)
        .map_err(|e| ReferenceError::File(format!("could not write the sidecar: {e}")))?;
    write_atomic(&dir.join(BINARY_FILE), &binary)?;
    write_atomic(&dir.join(SIDECAR_FILE), json.as_bytes())?;
    Ok(snapshot.payload.content_hash)
}

/// The two paths of a reference folder.
pub fn paths(dir: &Path) -> (PathBuf, PathBuf) {
    (dir.join(BINARY_FILE), dir.join(SIDECAR_FILE))
}

/// Read and validate the reference in the folder `dir`. The binary's hash must
/// match the sidecar's, and the sidecar's description must match the binary.
pub fn read(dir: &Path) -> Result<Reference, ReferenceError> {
    let (bin_path, side_path) = paths(dir);
    let text = std::fs::read_to_string(&side_path).map_err(|e| io_err(&side_path, e))?;
    let snapshot: Snapshot<ReferenceSidecar> = serde_json::from_str(&text).map_err(|e| {
        ReferenceError::File(format!(
            "{}: the sidecar is not a reference sidecar ({e}); write the reference again",
            side_path.display()
        ))
    })?;
    let side = snapshot.payload;
    if side.format != FORMAT {
        return Err(ReferenceError::File(format!(
            "{}: the sidecar names the format {:?}, not {FORMAT:?}",
            side_path.display(),
            side.format
        )));
    }
    if side.format_version != FORMAT_VERSION {
        return Err(ReferenceError::File(format!(
            "{}: format version {} is not the version this software reads ({FORMAT_VERSION}); \
             open it with a matching version",
            side_path.display(),
            side.format_version
        )));
    }
    let bytes = std::fs::read(&bin_path).map_err(|e| io_err(&bin_path, e))?;
    let hash = content_hash(&bytes);
    if hash != side.content_hash {
        return Err(ReferenceError::File(format!(
            "{}: the content hash is {hash} but the sidecar records {}; the binary was changed \
             or damaged after it was written, so write it again from its source",
            bin_path.display(),
            side.content_hash
        )));
    }
    let r = decode(&bytes, side.provenance.clone())?;
    let mismatch = r.lines() != side.lines
        || r.n() != side.n
        || r.df_hz.to_bits() != side.df_hz.to_bits()
        || [r.band.0, r.band.1] != side.band
        || r.labels != side.labels
        || r.unit != side.unit
        || bytes.len() as u64 != side.binary_bytes;
    if mismatch {
        return Err(ReferenceError::File(format!(
            "{}: the sidecar does not describe the binary beside it; write the reference again",
            side_path.display()
        )));
    }
    r.validate()?;
    Ok(r)
}
