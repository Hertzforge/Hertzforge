//! The NumPy `.npz` specification file: reader and mapping onto the grid.
//!
//! # The documented layout
//!
//! An `.npz` file is a zip archive of NumPy `.npy` arrays, one entry per key:
//!
//! | Key | Shape | Content |
//! |---|---|---|
//! | `cpsd` | `nf` by `nc` by `nc`, complex | the cross-spectral density matrix per line, in the control unit squared per hertz |
//! | `f` | `nf` | the frequency of each line in hertz (integer arrays occur and are accepted) |
//! | `warning_upper`, `warning_lower`, `abort_upper`, `abort_lower` | `nf` by `nc`, real, each optional | autospectrum limit lines; any subset may be present, each present one full size; `NaN` disables a line for a channel |
//!
//! Rows and columns follow the control channels in the order the test uses them
//! (the virtual degrees of freedom when a response transformation is set). The
//! file has no place for units; the import states the unit.
//!
//! The analysis service's own writer adds three keys that the layout above does
//! not have: `dof` (the channel names, read here as labels), `held` (which pairs
//! were specified) and `coordinate` (node and direction pairs). They are accepted
//! and the latter two are not needed here. Any other key is refused.
//!
//! # Mapping onto the controller's grid
//!
//! - **No interpolation.** The file's lines are placed on the controller's lines;
//!   a controller line the file does not hold is zero. A partial band is intended.
//! - **Matching.** A file frequency `f` is placed on line `k = round(f / df)` when
//!   `|f - k df|` is within `1e-9` of the line spacing; otherwise the file is
//!   refused, naming the first frequency that is not on the grid. The rule is this
//!   software's choice: the source documentation does not state one.
//! - **A coarser file.** A file at a whole multiple of the grid's spacing places
//!   its lines and leaves the lines between them at zero inside the band.
//!   Validation then refuses it (a control with no level inside the band), because
//!   controlling every other line to zero is never what a test means; run at the
//!   file's spacing instead.
//! - **Limits.** `warning_*` become the alarm lines, `abort_*` the abort lines; a
//!   `NaN`, or a line the file does not hold, is no line.
//!
//! # The cross-spectral convention
//!
//! The file's convention is not documented. This controller works in
//! `G = E[X X^H]`; the usual scientific-Python estimator gives the Bendat and
//! Piersol form, its conjugate, and a file made with it carries every cross term
//! conjugated relative to the controller. [`NpzImport::conjugate_on_import`]
//! decides, and **defaults to conjugating**: a file whose `cpsd[k, 0, 1]` has
//! phase `-phi` (channel 1 lagging channel 0 by `phi` in the Bendat form) arrives
//! with `G_01` at phase `+phi`. The choice is recorded in the provenance and is
//! open for a ruling once a file with a known phase is available.
//!
//! # The `.npy` reader
//!
//! Version 1, 2 and 3 headers; the header dictionary's `descr`, `fortran_order`
//! and `shape`; little-endian `complex128` (`<c16`) and `float64` (`<f8`); the
//! integer kinds (`<i8`, `<i4`, `<i2`, `|i1`, `<u8`, `<u4`, `<u2`, `|u1`) for
//! frequencies; `|b1` and `<U` strings for the analysis service's extra keys.
//! Anything else is refused with its descriptor named. C and Fortran order are both
//! read. Zip entries may be stored or deflated (`savez` and `savez_compressed`).

use crate::builders::Grid;
use crate::native::content_hash;
use crate::{
    band_of, Method, Provenance, PsdRepairReport, Reference, ReferenceError, ReferenceParts,
    RepairPolicy, Tolerances, C64,
};
use hertzforge_sdm::MatrixStack;
use std::io::Read;
use std::path::Path;

/// The relative tolerance (of the line spacing) within which a file frequency
/// matches a grid line.
pub const MATCH_REL_TOL: f64 = 1e-9;

/// The keys of the documented layout.
pub const DOCUMENTED_KEYS: [&str; 6] = [
    "cpsd",
    "f",
    "warning_upper",
    "warning_lower",
    "abort_upper",
    "abort_lower",
];

/// The keys the analysis service's writer adds.
pub const ANALYSIS_KEYS: [&str; 3] = ["dof", "held", "coordinate"];

fn file_err(name: &str, msg: String) -> ReferenceError {
    ReferenceError::File(format!("{name}: {msg}"))
}

// ---------------------------------------------------------------------------
// .npy
// ---------------------------------------------------------------------------

/// The values of one array, in C order.
#[derive(Clone, Debug, PartialEq)]
pub enum NpyData {
    F64(Vec<f64>),
    C128(Vec<C64>),
    Int(Vec<i64>),
    Bool(Vec<bool>),
    Str(Vec<String>),
}

/// One `.npy` array.
#[derive(Clone, Debug, PartialEq)]
pub struct NpyArray {
    pub descr: String,
    pub shape: Vec<usize>,
    pub data: NpyData,
}

impl NpyArray {
    /// Values as real numbers (floats and integers).
    pub fn as_f64(&self) -> Option<Vec<f64>> {
        match &self.data {
            NpyData::F64(v) => Some(v.clone()),
            NpyData::Int(v) => Some(v.iter().map(|&x| x as f64).collect()),
            _ => None,
        }
    }
}

/// The value of `key` in the header dictionary text, as the raw text up to the
/// next top-level comma or closing brace.
fn header_value<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    let pat_single = format!("'{key}'");
    let pat_double = format!("\"{key}\"");
    let at = header
        .find(&pat_single)
        .map(|i| i + pat_single.len())
        .or_else(|| header.find(&pat_double).map(|i| i + pat_double.len()))?;
    let rest = header[at..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for (i, ch) in rest.char_indices() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(ch),
            (None, '(' | '[') => depth += 1,
            (None, ')' | ']') => depth -= 1,
            (None, ',' | '}') if depth == 0 => return Some(rest[..i].trim()),
            _ => {}
        }
    }
    None
}

fn unquote(s: &str) -> Option<&str> {
    let s = s.trim();
    s.strip_prefix('\'')
        .and_then(|x| x.strip_suffix('\''))
        .or_else(|| s.strip_prefix('"').and_then(|x| x.strip_suffix('"')))
}

/// Parse one `.npy` file. `name` is used in messages.
pub fn parse_npy(bytes: &[u8], name: &str) -> Result<NpyArray, ReferenceError> {
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(file_err(
            name,
            "is not a NumPy array (the .npy magic is missing)".into(),
        ));
    }
    let major = bytes[6];
    let (hlen, start): (usize, usize) = match major {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        2 | 3 if bytes.len() >= 12 => (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12,
        ),
        _ => {
            return Err(file_err(
                name,
                format!("uses .npy format version {major}, which this reader does not know"),
            ))
        }
    };
    let end = start
        .checked_add(hlen)
        .filter(|&e| e <= bytes.len())
        .ok_or_else(|| file_err(name, "has a truncated .npy header".into()))?;
    let header = std::str::from_utf8(&bytes[start..end])
        .map_err(|_| file_err(name, "has a .npy header that is not text".into()))?;
    let descr = header_value(header, "descr")
        .and_then(unquote)
        .ok_or_else(|| {
            file_err(
                name,
                format!(
                    "has the element type {:?}, which this reader does not read (a structured \
                     or unknown type)",
                    header_value(header, "descr").unwrap_or("?")
                ),
            )
        })?
        .to_string();
    let fortran = match header_value(header, "fortran_order") {
        Some("True") => true,
        Some("False") => false,
        other => {
            return Err(file_err(
                name,
                format!("has an unreadable fortran_order {other:?} in its .npy header"),
            ))
        }
    };
    let shape_text = header_value(header, "shape")
        .ok_or_else(|| file_err(name, "has no shape in its .npy header".into()))?;
    let inner = shape_text
        .trim()
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| file_err(name, format!("has an unreadable shape {shape_text}")))?;
    let shape = inner
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_end_matches('L').parse::<usize>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| file_err(name, format!("has an unreadable shape {shape_text}")))?;
    let count = shape
        .iter()
        .try_fold(1usize, |a, &d| a.checked_mul(d))
        .ok_or_else(|| file_err(name, "has a shape too large to hold".into()))?;
    let body = &bytes[end..];
    let need = |size: usize| -> Result<&[u8], ReferenceError> {
        let n = count
            .checked_mul(size)
            .ok_or_else(|| file_err(name, "has a shape too large to hold".into()))?;
        if body.len() < n {
            return Err(file_err(
                name,
                format!(
                    "holds {} bytes of data where its shape {shape:?} needs {n}; the file is \
                     truncated",
                    body.len()
                ),
            ));
        }
        Ok(&body[..n])
    };
    let le8 = |c: &[u8]| {
        let mut a = [0u8; 8];
        a.copy_from_slice(c);
        a
    };
    let data = match descr.as_str() {
        "<f8" => NpyData::F64(
            need(8)?
                .chunks_exact(8)
                .map(|c| f64::from_le_bytes(le8(c)))
                .collect(),
        ),
        "<c16" => NpyData::C128(
            need(16)?
                .chunks_exact(16)
                .map(|c| {
                    C64::new(
                        f64::from_le_bytes(le8(&c[..8])),
                        f64::from_le_bytes(le8(&c[8..])),
                    )
                })
                .collect(),
        ),
        "<i8" => NpyData::Int(
            need(8)?
                .chunks_exact(8)
                .map(|c| i64::from_le_bytes(le8(c)))
                .collect(),
        ),
        "<u8" => NpyData::Int(
            need(8)?
                .chunks_exact(8)
                .map(|c| u64::from_le_bytes(le8(c)).min(i64::MAX as u64) as i64)
                .collect(),
        ),
        "<i4" => NpyData::Int(
            need(4)?
                .chunks_exact(4)
                .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as i64)
                .collect(),
        ),
        "<u4" => NpyData::Int(
            need(4)?
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as i64)
                .collect(),
        ),
        "<i2" => NpyData::Int(
            need(2)?
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as i64)
                .collect(),
        ),
        "<u2" => NpyData::Int(
            need(2)?
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]) as i64)
                .collect(),
        ),
        "|i1" => NpyData::Int(need(1)?.iter().map(|&b| b as i8 as i64).collect()),
        "|u1" => NpyData::Int(need(1)?.iter().map(|&b| b as i64).collect()),
        "|b1" => NpyData::Bool(need(1)?.iter().map(|&b| b != 0).collect()),
        d if d.starts_with("<U") => {
            let chars: usize = d[2..].parse().map_err(|_| {
                file_err(
                    name,
                    format!("has the element type {d:?}, which this reader does not read"),
                )
            })?;
            let size = chars
                .checked_mul(4)
                .ok_or_else(|| file_err(name, "has a string type too large".into()))?;
            let raw = need(size)?;
            let mut out = Vec::with_capacity(count);
            if size == 0 {
                out.resize(count, String::new());
            } else {
                for c in raw.chunks_exact(size) {
                    let s: String = c
                        .chunks_exact(4)
                        .map(|u| u32::from_le_bytes([u[0], u[1], u[2], u[3]]))
                        .take_while(|&u| u != 0)
                        .map(|u| char::from_u32(u).unwrap_or('\u{fffd}'))
                        .collect();
                    out.push(s);
                }
            }
            NpyData::Str(out)
        }
        other => {
            return Err(file_err(
                name,
                format!(
                    "has the element type {other:?}; this reader takes little-endian complex128 \
                     ('<c16') and float64 ('<f8'), and integers for frequencies. Save the array \
                     in one of those types"
                ),
            ))
        }
    };
    let data = if fortran && shape.len() > 1 {
        reorder_fortran(data, &shape)
    } else {
        data
    };
    Ok(NpyArray { descr, shape, data })
}

/// Fortran order to C order.
fn reorder_fortran(data: NpyData, shape: &[usize]) -> NpyData {
    fn perm<T: Clone>(v: Vec<T>, shape: &[usize]) -> Vec<T> {
        let count = v.len();
        let mut out = Vec::with_capacity(count);
        let mut idx = vec![0usize; shape.len()];
        for _ in 0..count {
            let mut f = 0usize;
            let mut stride = 1usize;
            for (k, &i) in idx.iter().enumerate() {
                f += i * stride;
                stride *= shape[k];
            }
            out.push(v[f].clone());
            for k in (0..shape.len()).rev() {
                idx[k] += 1;
                if idx[k] < shape[k] {
                    break;
                }
                idx[k] = 0;
            }
        }
        out
    }
    match data {
        NpyData::F64(v) => NpyData::F64(perm(v, shape)),
        NpyData::C128(v) => NpyData::C128(perm(v, shape)),
        NpyData::Int(v) => NpyData::Int(perm(v, shape)),
        NpyData::Bool(v) => NpyData::Bool(perm(v, shape)),
        NpyData::Str(v) => NpyData::Str(perm(v, shape)),
    }
}

/// Every array in an `.npz` archive, by key, in archive order. Entries the
/// documented layout and the analysis service do not know are refused.
pub fn read_archive(bytes: &[u8], name: &str) -> Result<Vec<(String, NpyArray)>, ReferenceError> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| {
        file_err(
            name,
            format!("is not a readable .npz (zip) archive: {e}; save it with numpy.savez"),
        )
    })?;
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| file_err(name, format!("entry {i} cannot be read: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        let entry_name = entry
            .name()
            .map_err(|e| file_err(name, format!("entry {i} has an unreadable name: {e}")))?
            .to_string();
        let key = entry_name
            .strip_suffix(".npy")
            .unwrap_or(&entry_name)
            .to_string();
        if !DOCUMENTED_KEYS.contains(&key.as_str()) && !ANALYSIS_KEYS.contains(&key.as_str()) {
            return Err(file_err(
                name,
                format!(
                    "holds the key {key:?}, which is not part of the specification layout; the \
                     keys are {} (and {} from the analysis service). Remove it or check that \
                     this is a specification file",
                    DOCUMENTED_KEYS.join(", "),
                    ANALYSIS_KEYS.join(", ")
                ),
            ));
        }
        if key == "coordinate" || key == "held" {
            continue; // accepted, not needed here
        }
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| file_err(name, format!("entry {entry_name} cannot be read: {e}")))?;
        let arr = parse_npy(&buf, &format!("{name} [{key}]"))?;
        out.push((key, arr));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Matching file lines to grid lines
// ---------------------------------------------------------------------------

/// The grid line of each file frequency, by the rule in the module docs.
pub fn match_lines(freqs: &[f64], grid: Grid, name: &str) -> Result<Vec<usize>, ReferenceError> {
    let mut used = vec![usize::MAX; grid.lines];
    let mut out = Vec::with_capacity(freqs.len());
    for (idx, &f) in freqs.iter().enumerate() {
        if !(f.is_finite() && f >= 0.0) {
            return Err(file_err(
                name,
                format!("frequency {idx} is {f}; every frequency must be a number at or above 0"),
            ));
        }
        let k = (f / grid.df_hz).round();
        if (f - k * grid.df_hz).abs() > MATCH_REL_TOL * grid.df_hz {
            return Err(file_err(
                name,
                format!(
                    "the line at {f} Hz (entry {idx}) is not on the controller's grid of {} Hz \
                     lines. The file's frequencies must fall on the grid, so its spacing must \
                     be a whole multiple of the grid's spacing; run at a spacing that divides \
                     the file's",
                    grid.df_hz
                ),
            ));
        }
        let k = k as usize;
        if k >= grid.lines {
            return Err(file_err(
                name,
                format!(
                    "the line at {f} Hz (entry {idx}) is above the grid's last line ({} Hz); \
                     raise the bandwidth or trim the file",
                    grid.freq(grid.lines - 1)
                ),
            ));
        }
        if used[k] != usize::MAX {
            return Err(file_err(
                name,
                format!(
                    "entries {} and {idx} both fall on the line at {} Hz; each line may appear \
                     once",
                    used[k],
                    grid.freq(k)
                ),
            ));
        }
        used[k] = idx;
        out.push(k);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The import
// ---------------------------------------------------------------------------

/// How an `.npz` specification is brought onto the controller's grid.
#[derive(Clone, Debug, PartialEq)]
pub struct NpzImport {
    pub grid: Grid,
    /// The control-space labels in the file's row order. When absent, the file's
    /// `dof` key is used if it has one, else `1`, `2`, and so on.
    pub labels: Option<Vec<String>>,
    /// The unit of the controls (the file cannot state one).
    pub unit: String,
    /// Conjugate every cross term on the way in (default true; see the module
    /// docs).
    pub conjugate_on_import: bool,
    /// Repair a line that is not positive semi-definite under this policy.
    pub repair: Option<RepairPolicy>,
}

impl NpzImport {
    /// The defaults: labels from the file, unit `g`, conjugate, no repair.
    pub fn new(grid: Grid) -> Self {
        NpzImport {
            grid,
            labels: None,
            unit: "g".into(),
            conjugate_on_import: true,
            repair: None,
        }
    }
}

/// Read an `.npz` specification file.
pub fn read(
    path: &Path,
    opts: &NpzImport,
) -> Result<(Reference, Option<PsdRepairReport>), ReferenceError> {
    let bytes = std::fs::read(path)
        .map_err(|e| ReferenceError::File(format!("{}: {e}; check the path", path.display())))?;
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    import(&bytes, &name, opts)
}

/// Import an `.npz` specification from its bytes; `name` is the source recorded in
/// the provenance and used in messages.
pub fn import(
    bytes: &[u8],
    name: &str,
    opts: &NpzImport,
) -> Result<(Reference, Option<PsdRepairReport>), ReferenceError> {
    let grid = Grid::new(opts.grid.df_hz, opts.grid.lines)?;
    let arrays = read_archive(bytes, name)?;
    let get = |key: &str| arrays.iter().find(|(k, _)| k == key).map(|(_, a)| a);
    let missing = |key: &str| {
        file_err(
            name,
            format!("has no {key:?} array; a specification file holds at least cpsd and f"),
        )
    };
    let cpsd = get("cpsd").ok_or_else(|| missing("cpsd"))?;
    let f = get("f").ok_or_else(|| missing("f"))?;
    let freqs = match (&f.data, f.shape.len()) {
        (NpyData::F64(_) | NpyData::Int(_), 1) => f.as_f64().unwrap_or_default(),
        _ => {
            return Err(file_err(
                name,
                format!(
                    "f is {:?} of type {}; it must be one row of frequencies in hertz",
                    f.shape, f.descr
                ),
            ))
        }
    };
    let nf = freqs.len();
    if cpsd.shape.len() != 3 || cpsd.shape[0] != nf || cpsd.shape[1] != cpsd.shape[2] {
        return Err(file_err(
            name,
            format!(
                "cpsd is {:?}; it must be lines by channels by channels ({nf} by n by n), one \
                 matrix per frequency in f",
                cpsd.shape
            ),
        ));
    }
    let nc = cpsd.shape[1];
    if nc == 0 || nf == 0 {
        return Err(file_err(name, "holds no lines or no channels".into()));
    }
    let values: Vec<C64> = match &cpsd.data {
        NpyData::C128(v) => v.clone(),
        NpyData::F64(v) => v.iter().map(|&x| C64::new(x, 0.0)).collect(),
        _ => {
            return Err(file_err(
                name,
                format!(
                    "cpsd has the element type {}; it must be complex128 or float64",
                    cpsd.descr
                ),
            ))
        }
    };
    let labels = match (&opts.labels, get("dof")) {
        (Some(l), _) => l.clone(),
        (None, Some(dof)) => match &dof.data {
            NpyData::Str(s) if dof.shape == [nc] => s.clone(),
            _ => {
                return Err(file_err(
                    name,
                    format!("dof is {:?}; it must be {nc} channel names", dof.shape),
                ))
            }
        },
        (None, None) => (1..=nc).map(|k| k.to_string()).collect(),
    };
    if labels.len() != nc {
        return Err(ReferenceError::Labels(format!(
            "{name}: the file has {nc} channels but {} labels were given; give one per channel \
             in the file's order",
            labels.len()
        )));
    }
    let lines = match_lines(&freqs, grid, name)?;
    let mut full = MatrixStack::zeros(grid.lines, nc, nc);
    for (idx, &k) in lines.iter().enumerate() {
        for r in 0..nc {
            for c in 0..nc {
                let v = values[(idx * nc + r) * nc + c];
                full.set(
                    k,
                    r,
                    c,
                    if opts.conjugate_on_import {
                        v.conj()
                    } else {
                        v
                    },
                );
            }
        }
    }
    let sdm = Reference::sdm_from_full(&full)?;
    let mut tolerances = Tolerances::none(grid.lines, nc);
    for (key, slot) in [
        ("warning_upper", &mut tolerances.alarm_upper),
        ("warning_lower", &mut tolerances.alarm_lower),
        ("abort_upper", &mut tolerances.abort_upper),
        ("abort_lower", &mut tolerances.abort_lower),
    ] {
        let Some(a) = get(key) else { continue };
        let v = match (a.as_f64(), a.shape == [nf, nc]) {
            (Some(v), true) => v,
            _ => {
                return Err(file_err(
                    name,
                    format!(
                        "{key} is {:?} of type {}; it must be real, lines by channels ({nf} by \
                         {nc})",
                        a.shape, a.descr
                    ),
                ))
            }
        };
        for (idx, &k) in lines.iter().enumerate() {
            for ch in 0..nc {
                slot[k * nc + ch] = v[idx * nc + ch];
            }
        }
    }
    let band = band_of(&sdm).ok_or_else(|| {
        file_err(
            name,
            "has no level on any line (every autospectrum is zero)".into(),
        )
    })?;
    let mut provenance = Provenance::new(Method::SpecificationNpz, name);
    provenance.source_sha256 = Some(content_hash(bytes));
    provenance.conjugated_on_import = Some(opts.conjugate_on_import);
    provenance.assumptions = vec![
        if opts.conjugate_on_import {
            "the file's cross-spectral convention is not documented; it was taken as the Bendat \
             and Piersol form, the conjugate of E[X X^H], and every cross term was conjugated"
                .to_string()
        } else {
            "the file's cross-spectral convention is not documented; it was taken as E[X X^H] \
             and read unchanged"
                .to_string()
        },
        format!(
            "file frequencies placed on grid lines within {MATCH_REL_TOL:e} of the {} Hz \
             spacing; grid lines absent from the file are zero",
            grid.df_hz
        ),
        "rows and columns taken in the control channels' order".to_string(),
    ];
    Reference::from_parts(
        ReferenceParts {
            sdm,
            df_hz: grid.df_hz,
            band,
            labels,
            unit: opts.unit.clone(),
            tolerances,
            response_transformation: None,
            drive_transformation: None,
            provenance,
        },
        opts.repair,
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// A `.npy` file from its descriptor, shape and little-endian body.
    pub(crate) fn npy(descr: &str, shape: &[usize], fortran: bool, body: &[u8]) -> Vec<u8> {
        let shape_txt = match shape.len() {
            0 => "()".to_string(),
            1 => format!("({},)", shape[0]),
            _ => format!(
                "({})",
                shape
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        let mut header = format!(
            "{{'descr': '{descr}', 'fortran_order': {}, 'shape': {shape_txt}, }}",
            if fortran { "True" } else { "False" }
        );
        while (10 + header.len() + 1) % 64 != 0 {
            header.push(' ');
        }
        header.push('\n');
        let mut out = b"\x93NUMPY\x01\x00".to_vec();
        out.extend_from_slice(&(header.len() as u16).to_le_bytes());
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(body);
        out
    }

    pub(crate) fn f64s(v: &[f64]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    pub(crate) fn c128s(v: &[C64]) -> Vec<u8> {
        v.iter()
            .flat_map(|x| x.re.to_le_bytes().into_iter().chain(x.im.to_le_bytes()))
            .collect()
    }

    /// An `.npz` archive of `(key, npy bytes)`; `deflate` compresses every entry.
    pub(crate) fn npz(entries: &[(&str, Vec<u8>)], deflate: bool) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let method = if deflate {
            zip::CompressionMethod::Deflated
        } else {
            zip::CompressionMethod::Stored
        };
        let opts = zip::write::SimpleFileOptions::default().compression_method(method);
        for (k, b) in entries {
            w.start_file(format!("{k}.npy"), opts).unwrap();
            w.write_all(b).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    /// Two channels, five lines at 10 to 50 Hz, autospectra 1 and 4, cross term of
    /// magnitude 1 at phase `-phi` in the file (Bendat form: channel 1 lags 0).
    fn cpsd2(phi_deg: f64) -> Vec<C64> {
        let x = C64::from_polar(1.0, -phi_deg.to_radians());
        let mut v = Vec::new();
        for _ in 0..5 {
            v.extend([C64::new(1.0, 0.0), x, x.conj(), C64::new(4.0, 0.0)]);
        }
        v
    }

    fn base_entries() -> Vec<(&'static str, Vec<u8>)> {
        let f: Vec<u8> = [10i64, 20, 30, 40, 50]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        vec![
            ("cpsd", npy("<c16", &[5, 2, 2], false, &c128s(&cpsd2(30.0)))),
            ("f", npy("<i8", &[5], false, &f)),
        ]
    }

    fn grid() -> Grid {
        Grid::new(10.0, 10).unwrap()
    }

    #[test]
    fn a_valid_file_with_integer_frequencies_and_nan_limits_imports() {
        let mut e = base_entries();
        let mut up = [2.0, 8.0].repeat(5);
        up[2 * 2 + 1] = f64::NAN; // no warning line for channel 2 at 30 Hz
        e.push(("warning_upper", npy("<f8", &[5, 2], false, &f64s(&up))));
        e.push((
            "abort_upper",
            npy("<f8", &[5, 2], false, &f64s(&[4.0, 16.0].repeat(5))),
        ));
        let bytes = npz(&e, false);
        let (r, rep) = import(&bytes, "spec.npz", &NpzImport::new(grid())).unwrap();
        assert!(rep.is_none());
        assert_eq!(r.band, (1, 6));
        assert_eq!(r.labels, vec!["1", "2"]);
        assert_eq!(r.sdm.get(3, 1, 1).re, 4.0);
        assert_eq!(r.sdm.get(0, 0, 0).re, 0.0);
        assert_eq!(r.tolerances.alarm_upper[3 * 2], 2.0);
        assert!(r.tolerances.alarm_upper[3 * 2 + 1].is_nan());
        assert!(r.tolerances.alarm_lower[3 * 2].is_nan());
        assert_eq!(r.tolerances.abort_upper[5 * 2 + 1], 16.0);
        assert!(r.tolerances.abort_upper[6 * 2 + 1].is_nan());
        assert_eq!(r.provenance.method, Method::SpecificationNpz);
        assert_eq!(r.provenance.conjugated_on_import, Some(true));
        assert!(r
            .provenance
            .source_sha256
            .as_ref()
            .unwrap()
            .starts_with("sha256:"));
        assert!(r.provenance.assumptions[0].contains("not documented"));
    }

    #[test]
    fn a_known_phase_arrives_with_the_documented_sign() {
        // Channel 1 lags channel 0 by 30 degrees. In the Bendat form the file holds
        // cpsd[k, 0, 1] at -30 degrees; the controller's E[X X^H] holds +30.
        let bytes = npz(&base_entries(), false);
        let (r, _) = import(&bytes, "s.npz", &NpzImport::new(grid())).unwrap();
        assert!((r.sdm.get(2, 0, 1).arg().to_degrees() - 30.0).abs() < 1e-12);
        let mut keep = NpzImport::new(grid());
        keep.conjugate_on_import = false;
        let (r, _) = import(&bytes, "s.npz", &keep).unwrap();
        assert!((r.sdm.get(2, 0, 1).arg().to_degrees() + 30.0).abs() < 1e-12);
        assert_eq!(r.provenance.conjugated_on_import, Some(false));
    }

    #[test]
    fn a_partial_band_leaves_the_rest_of_the_grid_at_zero() {
        let f = f64s(&[200.0, 210.0, 220.0, 230.0, 240.0]);
        let e = vec![
            ("cpsd", npy("<c16", &[5, 2, 2], false, &c128s(&cpsd2(0.0)))),
            ("f", npy("<f8", &[5], false, &f)),
        ];
        let (r, _) = import(
            &npz(&e, false),
            "p.npz",
            &NpzImport::new(Grid::new(10.0, 410).unwrap()),
        )
        .unwrap();
        assert_eq!(r.band, (20, 25));
        assert_eq!(r.sdm.get(30, 0, 0).re, 0.0);
        r.validate().unwrap();
    }

    #[test]
    fn integer_kinds_fortran_order_and_deflate_read_the_same() {
        let f32bit: Vec<u8> = [10i32, 20, 30, 40, 50]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        // The same cpsd in Fortran order: index (k, r, c) at k + 5 r + 10 c.
        let c_order = cpsd2(30.0);
        let mut f_order = vec![C64::new(0.0, 0.0); 20];
        for k in 0..5 {
            for r in 0..2 {
                for c in 0..2 {
                    f_order[k + 5 * r + 10 * c] = c_order[(k * 2 + r) * 2 + c];
                }
            }
        }
        let e = vec![
            ("cpsd", npy("<c16", &[5, 2, 2], true, &c128s(&f_order))),
            ("f", npy("<i4", &[5], false, &f32bit)),
        ];
        let (a, _) = import(&npz(&e, true), "x.npz", &NpzImport::new(grid())).unwrap();
        let (b, _) = import(
            &npz(&base_entries(), false),
            "x.npz",
            &NpzImport::new(grid()),
        )
        .unwrap();
        assert_eq!(a.sdm, b.sdm);
    }

    #[test]
    fn wrong_shapes_unknown_and_missing_keys_are_refused_by_name() {
        let mut e = base_entries();
        e[0].1 = npy(
            "<c16",
            &[5, 2, 3],
            false,
            &c128s(&vec![C64::new(1.0, 0.0); 30]),
        );
        let err = import(&npz(&e, false), "s.npz", &NpzImport::new(grid())).unwrap_err();
        assert!(err.to_string().contains("cpsd is [5, 2, 3]"), "{err}");

        let mut e = base_entries();
        e.push(("abort_lower", npy("<f8", &[5, 3], false, &f64s(&[1.0; 15]))));
        let err = import(&npz(&e, false), "s.npz", &NpzImport::new(grid())).unwrap_err();
        assert!(err.to_string().contains("abort_lower is [5, 3]"), "{err}");

        let mut e = base_entries();
        e.push(("extra", npy("<f8", &[1], false, &f64s(&[1.0]))));
        let err = import(&npz(&e, false), "s.npz", &NpzImport::new(grid())).unwrap_err();
        assert!(err.to_string().contains("\"extra\""), "{err}");

        let e = vec![base_entries().remove(0)];
        let err = import(&npz(&e, false), "s.npz", &NpzImport::new(grid())).unwrap_err();
        assert!(err.to_string().contains("no \"f\" array"), "{err}");

        let mut e = base_entries();
        e[0].1 = npy(">c16", &[5, 2, 2], false, &c128s(&cpsd2(0.0)));
        let err = import(&npz(&e, false), "s.npz", &NpzImport::new(grid())).unwrap_err();
        assert!(err.to_string().contains("\">c16\""), "{err}");

        let err = import(b"not a zip", "s.npz", &NpzImport::new(grid())).unwrap_err();
        assert!(err.to_string().contains("not a readable .npz"), "{err}");
    }

    #[test]
    fn a_spacing_that_does_not_divide_the_grid_is_refused_at_the_first_unmatched_line() {
        let mut e = base_entries();
        e[1].1 = npy("<f8", &[5], false, &f64s(&[0.0, 15.0, 30.0, 45.0, 60.0]));
        let err = import(&npz(&e, false), "s.npz", &NpzImport::new(grid())).unwrap_err();
        assert!(err.to_string().contains("line at 15 Hz (entry 1)"), "{err}");
        // A rounding-level offset still matches.
        let mut e = base_entries();
        e[1].1 = npy(
            "<f8",
            &[5],
            false,
            &f64s(&[10.0 + 1e-12, 20.0, 30.0, 40.0, 50.0 - 1e-12]),
        );
        assert!(import(&npz(&e, false), "s.npz", &NpzImport::new(grid())).is_ok());
    }

    #[test]
    fn a_coarser_file_leaves_zero_lines_in_band_and_is_refused_by_validation() {
        // 20 Hz lines on a 10 Hz grid: the documented rule puts zeros between them.
        let mut e = base_entries();
        e[1].1 = npy("<f8", &[5], false, &f64s(&[20.0, 40.0, 60.0, 80.0, 100.0]));
        let err = import(
            &npz(&e, false),
            "s.npz",
            &NpzImport::new(Grid::new(10.0, 20).unwrap()),
        )
        .unwrap_err();
        assert!(
            matches!(err, ReferenceError::ZeroDiagonalInBand { line: 3, .. }),
            "{err:?}"
        );
    }

    #[test]
    fn channel_names_come_from_dof_or_the_caller() {
        let mut e = base_entries();
        let names: Vec<u8> = ["101X+", "102Y-"]
            .iter()
            .flat_map(|s| {
                let mut u: Vec<u32> = s.chars().map(|c| c as u32).collect();
                u.resize(5, 0);
                u.into_iter().flat_map(|x| x.to_le_bytes())
            })
            .collect();
        e.push(("dof", npy("<U5", &[2], false, &names)));
        e.push(("held", npy("|b1", &[2, 2], false, &[1, 1, 1, 1])));
        let (r, _) = import(&npz(&e, false), "s.npz", &NpzImport::new(grid())).unwrap();
        assert_eq!(r.labels, vec!["101X+", "102Y-"]);
        let mut opts = NpzImport::new(grid());
        opts.labels = Some(vec!["front".into(), "rear".into()]);
        let (r, _) = import(&npz(&e, false), "s.npz", &opts).unwrap();
        assert_eq!(r.labels, vec!["front", "rear"]);
        opts.labels = Some(vec!["only".into()]);
        assert!(matches!(
            import(&npz(&e, false), "s.npz", &opts),
            Err(ReferenceError::Labels(_))
        ));
    }

    #[test]
    fn a_non_hermitian_file_is_refused() {
        let mut v = cpsd2(30.0);
        v[2 * 4 + 2] = C64::new(0.3, 0.0); // line 2, element (1, 0)
        let mut e = base_entries();
        e[0].1 = npy("<c16", &[5, 2, 2], false, &c128s(&v));
        let err = import(&npz(&e, false), "s.npz", &NpzImport::new(grid())).unwrap_err();
        assert!(
            matches!(err, ReferenceError::NotHermitian { line: 3, .. }),
            "{err:?}"
        );
    }
}
