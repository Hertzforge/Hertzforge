//! UFF/UNV reader + writer: datasets 58 (functions, ASCII + 58b binary), 151
//! (header) and 164 (units), per the UC-SDRL (University of Cincinnati Structural Dynamics
//! Research Lab) Universal File Format dataset specifications.
//!
//! Reader discipline is LIBERAL (the dialect damage seen in real files
//! is implemented as tolerances, each surfacing a warning); the writer is
//! STRICT spec-conformant ASCII (58b write is deferred - binary interop is the
//! format's least reliable corner).
//!
//! Structural rules that make this parser survive real files:
//! - Blocks are walked BYTE-WISE, so a `-1` inside a binary 58b payload can
//!   never terminate a block early (the payload length comes from the
//!   identifier line, never from scanning).
//! - Delimiter recognition = a line whose trimmed content is exactly "-1"
//!   (writers pad anywhere from 6 to 80 columns).
//! - Numeric records are sliced FIXED-WIDTH, never whitespace-split
//!   (run-together negatives are the #1 wild failure).
//! - `D` exponents are accepted everywhere a float is parsed.
//! - NVAL is authoritative: extra padding values beyond NVAL are dropped,
//!   short tails are an error only if MORE than one line is missing.

use crate::{Confidence, ImportError, ImportResult};
use eigen_data::{
    units, Abscissa, Direction, DofRef, FnId, FunctionKind, FunctionRecord, Ordinate, Provenance,
    SourceRef, Unit,
};

// ---------------------------------------------------------------------------
// Low-level field helpers
// ---------------------------------------------------------------------------

/// Slice a fixed-width field by 1-based inclusive column range, tolerant of
/// short lines (returns "" beyond end - trailing blanks are legal).
fn field(line: &str, c1: usize, c2: usize) -> &str {
    let bytes = line.as_bytes();
    let start = (c1 - 1).min(bytes.len());
    let end = c2.min(bytes.len());
    // Numeric records are ASCII by spec; ID lines are handled lossily upstream.
    std::str::from_utf8(&bytes[start..end]).unwrap_or("")
}

/// Fortran float: trims, accepts D/d exponents, blank -> 0.0.
fn ffloat(s: &str) -> Result<f64, String> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(0.0);
    }
    let n = t.replace(['D', 'd'], "E");
    n.parse::<f64>()
        .map_err(|_| format!("bad float field '{s}'"))
}

/// Fortran integer: trims, blank -> 0.
fn fint(s: &str) -> Result<i64, String> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(0);
    }
    t.parse::<i64>()
        .map_err(|_| format!("bad integer field '{s}'"))
}

/// E13.5 emission in the Fortran style the format's readers expect:
/// ` s1.23450E+00` right-aligned in 13 columns, 2-digit exponent (3-digit when
/// needed).
fn e13_5(v: f64) -> String {
    if v == 0.0 {
        return format!("{:>13}", "0.00000E+00");
    }
    let neg = v < 0.0;
    let a = v.abs();
    let mut exp = a.log10().floor() as i32;
    let mut mant = a / 10f64.powi(exp);
    // Guard rounding at the decade edge (e.g. 9.999995 -> 10.0).
    if mant >= 10.0 {
        mant /= 10.0;
        exp += 1;
    }
    let mant = format!("{mant:.5}");
    let (mant, exp) = if mant.starts_with("10.") {
        ("1.00000".to_string(), exp + 1)
    } else {
        (mant, exp)
    };
    let sign = if neg { "-" } else { "" };
    let esign = if exp < 0 { '-' } else { '+' };
    format!("{:>13}", format!("{sign}{mant}E{esign}{:02}", exp.abs()))
}

/// I10 emission.
fn i10(v: i64) -> String {
    format!("{v:>10}")
}

// ---------------------------------------------------------------------------
// Block walker
// ---------------------------------------------------------------------------

struct Block {
    dataset: String,
    /// Byte offset of the identifier line (for error reporting).
    offset: u64,
    /// ASCII lines of the block body (58b: the 11 header lines only).
    lines: Vec<String>,
    /// 58b raw payload.
    binary: Option<BinaryPayload>,
}

struct BinaryPayload {
    little_endian: bool,
    bytes: Vec<u8>,
}

/// True when the head of the file looks like a UFF block opener.
pub fn sniff_head(head: &[u8]) -> Confidence {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    match (lines.next(), lines.next()) {
        (Some(a), Some(b)) => {
            // The identifier line may carry trailing fields ("    58b     1 ...").
            let tok = b.split_whitespace().next().unwrap_or("");
            let num = tok.trim_end_matches(['b', 'B']);
            if a.trim() == "-1" && !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
                Confidence::Yes
            } else {
                Confidence::No
            }
        }
        _ => Confidence::No,
    }
}

/// Read one text line starting at `pos` (handles \n and \r\n); returns
/// (lossy-decoded content without terminator, next position).
fn take_line(bytes: &[u8], pos: usize) -> Option<(String, usize)> {
    if pos >= bytes.len() {
        return None;
    }
    let rel = bytes[pos..].iter().position(|&b| b == b'\n');
    let (end, next) = match rel {
        Some(r) => (pos + r, pos + r + 1),
        None => (bytes.len(), bytes.len()),
    };
    let mut slice = &bytes[pos..end];
    if slice.last() == Some(&b'\r') {
        slice = &slice[..slice.len() - 1];
    }
    Some((String::from_utf8_lossy(slice).into_owned(), next))
}

fn walk_blocks(bytes: &[u8], warnings: &mut Vec<String>) -> Result<Vec<Block>, ImportError> {
    let mut blocks = Vec::new();
    let mut pos = 0usize;
    loop {
        // Seek an opening delimiter.
        let mut opened = false;
        while let Some((line, next)) = take_line(bytes, pos) {
            pos = next;
            if line.trim() == "-1" {
                opened = true;
                break;
            } else if !line.trim().is_empty() {
                warnings.push(format!(
                    "skipped stray text before a block: '{}'",
                    line.trim()
                ));
            }
        }
        if !opened {
            return Ok(blocks);
        }
        // Identifier line.
        let Some((id_line, next)) = take_line(bytes, pos) else {
            warnings.push("file ends after an opening -1".into());
            return Ok(blocks);
        };
        let offset = pos as u64;
        pos = next;
        let id_trim = id_line.trim();
        if id_trim.is_empty() {
            continue;
        }
        // Binary 58b: column 7 (1-based) is 'b'.
        let is_binary = id_line.len() >= 7 && matches!(id_line.as_bytes()[6], b'b' | b'B');
        if is_binary {
            let byte_order = fint(field(&id_line, 8, 13)).unwrap_or(1);
            let n_ascii = fint(field(&id_line, 20, 31)).unwrap_or(11).max(0) as usize;
            let n_bytes = fint(field(&id_line, 32, 43)).unwrap_or(0).max(0) as usize;
            let mut lines = Vec::with_capacity(n_ascii);
            for _ in 0..n_ascii {
                match take_line(bytes, pos) {
                    Some((l, n)) => {
                        lines.push(l);
                        pos = n;
                    }
                    None => {
                        return Err(ImportError::Malformed {
                            detail: "58b truncated inside its ASCII header".into(),
                            offset: Some(pos as u64),
                        })
                    }
                }
            }
            if pos + n_bytes > bytes.len() {
                return Err(ImportError::Malformed {
                    detail: format!(
                        "58b declares {n_bytes} payload bytes but only {} remain",
                        bytes.len() - pos
                    ),
                    offset: Some(pos as u64),
                });
            }
            let payload = bytes[pos..pos + n_bytes].to_vec();
            pos += n_bytes;
            // Expect the closing -1; be liberal about interposed CR/LF (the
            // documented wild contamination) and scan to it with a warning.
            let mut closed = false;
            let scan_start = pos;
            while let Some((line, n)) = take_line(bytes, pos) {
                pos = n;
                if line.trim() == "-1" {
                    closed = true;
                    break;
                } else if !line.trim().is_empty() {
                    warnings.push(
                        "58b payload not immediately followed by -1 (scanning forward)".into(),
                    );
                }
            }
            if !closed {
                warnings.push("58b block missing its closing -1".into());
            } else if pos != scan_start && pos > scan_start {
                // no-op: bookkeeping clarity
            }
            blocks.push(Block {
                dataset: "58b".into(),
                offset,
                lines,
                binary: Some(BinaryPayload {
                    little_endian: byte_order != 2,
                    bytes: payload,
                }),
            });
            continue;
        }
        // ASCII dataset: collect lines until the closing -1.
        let dataset = id_trim.to_string();
        let mut lines = Vec::new();
        let mut closed = false;
        while let Some((line, n)) = take_line(bytes, pos) {
            pos = n;
            if line.trim() == "-1" {
                closed = true;
                break;
            }
            lines.push(line);
        }
        if !closed {
            warnings.push(format!("dataset {dataset} missing its closing -1 (EOF)"));
        }
        blocks.push(Block {
            dataset,
            offset,
            lines,
            binary: None,
        });
    }
}

// ---------------------------------------------------------------------------
// Dataset 58 semantics
// ---------------------------------------------------------------------------

fn kind_from_uff(func_type: i64) -> FunctionKind {
    match func_type {
        1 => FunctionKind::TimeHistory,
        2 => FunctionKind::Autopower,
        3 => FunctionKind::Cpsd,
        4 => FunctionKind::Frf,
        5 => FunctionKind::Correlation,
        6 => FunctionKind::Coherence,
        9 => FunctionKind::Psd,
        12 => FunctionKind::LinearSpectrum,
        24 => FunctionKind::Srs,
        _ => FunctionKind::Unknown,
    }
}

fn uff_type_from_kind(kind: FunctionKind) -> i64 {
    match kind {
        FunctionKind::TimeHistory => 1,
        FunctionKind::Autopower => 2,
        FunctionKind::Cpsd => 3,
        FunctionKind::Frf => 4,
        FunctionKind::Correlation => 5,
        FunctionKind::Coherence => 6,
        FunctionKind::Psd => 9,
        FunctionKind::LinearSpectrum | FunctionKind::PowerSpectrum => 12,
        FunctionKind::Srs => 24,
        FunctionKind::BandSpectrum | FunctionKind::Profile | FunctionKind::Unknown => 0,
    }
}

fn dir_from_uff(code: i64, warnings: &mut Vec<String>, ctx: &str) -> Direction {
    match code {
        -3..=3 => Direction::new(code as i8).unwrap_or(Direction::SCALAR),
        4..=6 | -6..=-4 => {
            warnings.push(format!(
                "{ctx}: rotational DOF direction {code} flattened to scalar (v1 limit)"
            ));
            Direction::SCALAR
        }
        _ => {
            warnings.push(format!(
                "{ctx}: direction code {code} out of range, using scalar"
            ));
            Direction::SCALAR
        }
    }
}

fn unit_from_label(label: &str) -> Unit {
    let t = label.trim();
    if t.is_empty() || t == "NONE" {
        return Unit::dimensionless();
    }
    units::unit(t).unwrap_or_else(|| Unit::unknown(t))
}

struct Rec58Header {
    id1: String,
    id_lines: Vec<String>,
    func_type: i64,
    resp: Option<DofRef>,
    refr: Option<DofRef>,
    ord_type: i64,
    nval: usize,
    even: bool,
    x_min: f64,
    x_step: f64,
    unit_absc: Unit,
    unit_ord: Unit,
}

fn parse_58_header(lines: &[String], warnings: &mut Vec<String>) -> Result<Rec58Header, String> {
    if lines.len() < 11 {
        return Err(format!(
            "dataset 58 needs 11 header records, found {}",
            lines.len()
        ));
    }
    let id1 = lines[0].trim().to_string();
    let l6 = &lines[5];
    let func_type = fint(field(l6, 1, 5))?;
    let resp_node = fint(field(l6, 42, 51))?;
    let resp_dir = fint(field(l6, 52, 55))?;
    let ref_node = fint(field(l6, 67, 76))?;
    let ref_dir = fint(field(l6, 77, 80))?;
    let mk_dof = |node: i64, dir: i64, warnings: &mut Vec<String>, ctx: &str| {
        let d = dir_from_uff(dir, warnings, ctx);
        if node <= 0 && d.is_scalar() {
            None
        } else {
            Some(DofRef {
                node: (node > 0).then_some(node as u32),
                dir: d,
            })
        }
    };
    let resp = mk_dof(resp_node, resp_dir, warnings, "response");
    let refr = mk_dof(ref_node, ref_dir, warnings, "reference");

    let l7 = &lines[6];
    let ord_type = fint(field(l7, 1, 10))?;
    let nval = fint(field(l7, 11, 20))?.max(0) as usize;
    let spacing = fint(field(l7, 21, 30))?;
    let x_min = ffloat(field(l7, 31, 43))?;
    let x_step = ffloat(field(l7, 44, 56))?;
    if !matches!(ord_type, 2 | 4 | 5 | 6) {
        return Err(format!(
            "dataset 58 ordinate data type {ord_type} is not in {{2,4,5,6}}"
        ));
    }
    let unit_absc = unit_from_label(field(&lines[7], 48, 67));
    let unit_ord = unit_from_label(field(&lines[8], 48, 67));
    Ok(Rec58Header {
        id1,
        id_lines: lines[..5]
            .iter()
            .map(|l| l.trim_end().to_string())
            .collect(),
        func_type,
        resp,
        refr,
        ord_type,
        nval,
        even: spacing == 1,
        x_min,
        x_step,
        unit_absc,
        unit_ord,
    })
}

/// Parse the ASCII data section: fixed strides per the 8-case table
/// of the dataset 58 specification. Values consumed left-to-right, line by line, stopping
/// at the required count; blank/short tails tolerated.
fn parse_58_ascii_values(
    lines: &[String],
    needed: usize,
    width: usize,
    warnings: &mut Vec<String>,
) -> Result<Vec<f64>, String> {
    let mut out = Vec::with_capacity(needed);
    'outer: for line in lines {
        let mut c = 0usize;
        let n = line.len();
        while c < n {
            let end = (c + width).min(n);
            let f = &line[c..end];
            c = end;
            if f.trim().is_empty() {
                continue;
            }
            out.push(ffloat(f)?);
            if out.len() == needed {
                break 'outer;
            }
        }
    }
    if out.len() < needed {
        return Err(format!(
            "data section ended early: {} of {needed} values",
            out.len()
        ));
    }
    // NVAL is authoritative; padding beyond it is dropped silently by design,
    // but note when the file carried noticeably more.
    let extra: usize = lines
        .iter()
        .map(|l| l.len() / width)
        .sum::<usize>()
        .saturating_sub(needed);
    if extra > 2 {
        warnings.push(format!(
            "data section carries ~{extra} values beyond NVAL (ignored)"
        ));
    }
    Ok(out)
}

fn read_binary_values(
    payload: &BinaryPayload,
    needed: usize,
    wide: bool,
    warnings: &mut Vec<String>,
) -> Result<Vec<f64>, String> {
    let size = if wide { 8 } else { 4 };
    let avail = payload.bytes.len() / size;
    if avail < needed {
        return Err(format!(
            "58b payload holds {avail} values of {size} bytes, header needs {needed}"
        ));
    }
    if !payload.bytes.len().is_multiple_of(size) {
        warnings.push(format!(
            "58b payload has {} residue bytes (CR/LF contamination is the usual cause)",
            payload.bytes.len() % size
        ));
    }
    let mut out = Vec::with_capacity(needed);
    for i in 0..needed {
        let b = &payload.bytes[i * size..(i + 1) * size];
        let v = if wide {
            let arr: [u8; 8] = b.try_into().unwrap();
            if payload.little_endian {
                f64::from_le_bytes(arr)
            } else {
                f64::from_be_bytes(arr)
            }
        } else {
            let arr: [u8; 4] = b.try_into().unwrap();
            (if payload.little_endian {
                f32::from_le_bytes(arr)
            } else {
                f32::from_be_bytes(arr)
            }) as f64
        };
        out.push(v);
    }
    Ok(out)
}

/// Interleaved value layout per case: how many raw scalars one SAMPLE takes,
/// and whether an abscissa value leads it.
fn sample_layout(complex: bool, even: bool) -> (usize, bool) {
    let ord = if complex { 2 } else { 1 };
    if even {
        (ord, false)
    } else {
        (ord + 1, true)
    }
}

fn assemble_record(
    h: Rec58Header,
    raw: Vec<f64>,
    path: &str,
    seq: &mut u64,
    warnings: &mut Vec<String>,
) -> FunctionRecord {
    let complex = matches!(h.ord_type, 5 | 6);
    let (stride, has_x) = sample_layout(complex, h.even);
    let n = raw.len() / stride;
    let mut xs = Vec::new();
    let mut re = Vec::with_capacity(n);
    let mut im = Vec::with_capacity(n);
    for s in 0..n {
        let base = s * stride;
        let mut k = base;
        if has_x {
            xs.push(raw[k]);
            k += 1;
        }
        re.push(raw[k]);
        if complex {
            im.push(raw[k + 1]);
        }
    }
    let abscissa = if h.even {
        Abscissa::Even {
            start: h.x_min,
            step: h.x_step,
            n,
        }
    } else {
        Abscissa::Explicit(xs)
    };
    let ordinate = if complex {
        Ordinate::Complex(re.into_iter().zip(im).collect())
    } else {
        Ordinate::Real(re)
    };
    let kind = kind_from_uff(h.func_type);
    if kind == FunctionKind::Unknown && h.func_type != 0 {
        warnings.push(format!(
            "function type {} has no Insight mapping; imported as Unknown",
            h.func_type
        ));
    }
    let dof_part = match (h.resp, h.refr) {
        (Some(r), Some(f)) => format!("{r}/{f}"),
        (Some(r), None) => r.to_string(),
        _ => String::new(),
    };
    let name = if !h.id1.is_empty() && h.id1 != "NONE" {
        if dof_part.is_empty() {
            h.id1.clone()
        } else {
            format!("{} {}", h.id1, dof_part)
        }
    } else if !dof_part.is_empty() {
        format!("{} {}", kind.label(), dof_part)
    } else {
        format!("{} {}", kind.label(), *seq)
    };
    *seq += 1;
    FunctionRecord {
        id: FnId(*seq),
        name,
        role: None,
        response: h.resp,
        reference: h.refr,
        kind,
        abscissa,
        ordinate,
        unit_ordinate: h.unit_ord,
        unit_abscissa: h.unit_absc,
        t0: None,
        provenance: Provenance::Imported {
            format: "uff".into(),
            path: path.into(),
            detail: format!("dataset 58, type {}", h.func_type),
        },
        source: SourceRef {
            path: path.into(),
            format: "uff".into(),
            imported_at: 0,
        },
        comments: h.id_lines,
    }
}

/// Import every dataset 58/58b in the byte stream. 151/164 blocks contribute
/// context (comments/warnings); other datasets are noted and skipped.
pub fn import_bytes(bytes: &[u8], path: &str) -> Result<ImportResult, ImportError> {
    let mut warnings = Vec::new();
    let blocks = walk_blocks(bytes, &mut warnings)?;
    if blocks.is_empty() {
        return Err(ImportError::Malformed {
            detail: "no UFF blocks found".into(),
            offset: Some(0),
        });
    }
    let mut functions = Vec::new();
    let mut seq = 0u64;
    let mut skipped: Vec<String> = Vec::new();
    for b in &blocks {
        match b.dataset.as_str() {
            "58" | "58b" => {
                let h = match parse_58_header(&b.lines, &mut warnings) {
                    Ok(h) => h,
                    Err(e) => {
                        warnings.push(format!("dataset at byte {}: {e} (skipped)", b.offset));
                        continue;
                    }
                };
                let complex = matches!(h.ord_type, 5 | 6);
                let (stride, _) = sample_layout(complex, h.even);
                // NVAL counts PAIRS when uneven, VALUES when even (the classic
                // trap): raw scalar count differs per case, through `stride`.
                let raw_needed = h.nval * stride;
                let raw = if let Some(payload) = &b.binary {
                    // Binary: ordinate width by type; abscissa always single.
                    // Mixed widths only occur for wide+uneven; handle by
                    // reading the interleave manually in that case.
                    let wide = matches!(h.ord_type, 4 | 6);
                    if wide && !h.even {
                        match read_mixed_uneven_wide(payload, h.nval, complex, &mut warnings) {
                            Ok(v) => v,
                            Err(e) => {
                                warnings.push(format!("58b at byte {}: {e} (skipped)", b.offset));
                                continue;
                            }
                        }
                    } else {
                        match read_binary_values(payload, raw_needed, wide, &mut warnings) {
                            Ok(v) => v,
                            Err(e) => {
                                warnings.push(format!("58b at byte {}: {e} (skipped)", b.offset));
                                continue;
                            }
                        }
                    }
                } else {
                    let wide = matches!(h.ord_type, 4 | 6);
                    let parsed = if h.even {
                        // Cases 1/3 (13-wide) and 5/7 (20-wide): uniform stride.
                        let width = if wide { 20 } else { 13 };
                        parse_58_ascii_values(&b.lines[11..], raw_needed, width, &mut warnings)
                    } else if wide {
                        // Cases 6/8: `2(E13.5,E20.12)` / `E13.5,2E20.12` -
                        // abscissa 13-wide, ordinates 20-wide, mixed per sample.
                        parse_58_ascii_mixed(
                            &b.lines[11..],
                            h.nval,
                            if complex { 2 } else { 1 },
                            &mut warnings,
                        )
                    } else {
                        // Cases 2/4: uniform 13-wide interleave.
                        parse_58_ascii_values(&b.lines[11..], raw_needed, 13, &mut warnings)
                    };
                    match parsed {
                        Ok(v) => v,
                        Err(e) => {
                            warnings
                                .push(format!("dataset 58 at byte {}: {e} (skipped)", b.offset));
                            continue;
                        }
                    }
                };
                let rec = assemble_record(h, raw, path, &mut seq, &mut warnings);
                if let Err(e) = rec.validate() {
                    warnings.push(format!("{e} (skipped)"));
                    continue;
                }
                functions.push(rec);
            }
            "151" => {
                if let Some(l) = b.lines.first() {
                    warnings.push(format!("header (151): {}", l.trim()));
                }
            }
            "164" => {
                warnings.push(
                    "units dataset (164) present; per-record unit labels take precedence".into(),
                );
            }
            other => skipped.push(other.to_string()),
        }
    }
    if !skipped.is_empty() {
        skipped.sort();
        skipped.dedup();
        warnings.push(format!(
            "skipped dataset types (geometry/other, Module B scope): {}",
            skipped.join(", ")
        ));
    }
    if functions.is_empty() {
        return Err(ImportError::Malformed {
            detail: "file contains no readable dataset 58 functions".into(),
            offset: Some(0),
        });
    }
    Ok(ImportResult {
        functions,
        warnings,
    })
}

/// ASCII cases 6/8: uneven spacing with DOUBLE ordinates - fields alternate
/// widths per sample (`E13.5` abscissa then `E20.12` ordinate(s)). Lines
/// always hold whole samples (the Fortran formats guarantee it), so the width
/// pattern restarts per line and a short final line is fine.
fn parse_58_ascii_mixed(
    lines: &[String],
    n_samples: usize,
    ord_n: usize,
    warnings: &mut Vec<String>,
) -> Result<Vec<f64>, String> {
    let pattern: Vec<usize> = std::iter::once(13)
        .chain(std::iter::repeat_n(20, ord_n))
        .collect();
    let mut out = Vec::with_capacity(n_samples * (1 + ord_n));
    let needed = n_samples * (1 + ord_n);
    'outer: for line in lines {
        let mut pos = 0usize;
        let n = line.len();
        while pos < n {
            for w in &pattern {
                if pos >= n {
                    break;
                }
                let end = (pos + w).min(n);
                let f = &line[pos..end];
                pos = end;
                if f.trim().is_empty() {
                    continue;
                }
                out.push(ffloat(f)?);
                if out.len() == needed {
                    break 'outer;
                }
            }
        }
    }
    if out.len() < needed {
        return Err(format!(
            "mixed data section ended early: {} of {needed} values",
            out.len()
        ));
    }
    let _ = warnings;
    Ok(out)
}

/// 58b case 6/8: uneven spacing with DOUBLE ordinates - abscissa stays single
/// precision: interleave [f32 x][f64 ord(s)].
fn read_mixed_uneven_wide(
    payload: &BinaryPayload,
    nval: usize,
    complex: bool,
    _warnings: &mut Vec<String>,
) -> Result<Vec<f64>, String> {
    let ord_n = if complex { 2 } else { 1 };
    let sample_bytes = 4 + 8 * ord_n;
    let need = nval * sample_bytes;
    if payload.bytes.len() < need {
        return Err(format!(
            "58b mixed payload holds {} bytes, header needs {need}",
            payload.bytes.len()
        ));
    }
    let le = payload.little_endian;
    let mut out = Vec::with_capacity(nval * (1 + ord_n));
    for s in 0..nval {
        let mut p = s * sample_bytes;
        let xb: [u8; 4] = payload.bytes[p..p + 4].try_into().unwrap();
        out.push(
            (if le {
                f32::from_le_bytes(xb)
            } else {
                f32::from_be_bytes(xb)
            }) as f64,
        );
        p += 4;
        for _ in 0..ord_n {
            let ob: [u8; 8] = payload.bytes[p..p + 8].try_into().unwrap();
            out.push(if le {
                f64::from_le_bytes(ob)
            } else {
                f64::from_be_bytes(ob)
            });
            p += 8;
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Writer (ASCII 58, strict)
// ---------------------------------------------------------------------------

fn dir_to_uff(d: Direction) -> i64 {
    d.code() as i64
}

/// Write one FunctionRecord as an ASCII dataset 58 block.
pub fn write_58(rec: &FunctionRecord, out: &mut String) -> Result<(), String> {
    rec.validate()?;
    let complex = rec.ordinate.is_complex();
    let even = matches!(rec.abscissa, Abscissa::Even { .. });
    let n = rec.ordinate.len();
    out.push_str("    -1\n");
    out.push_str("    58\n");
    // Records 1-5: name first, then up to 4 comments, NONE-padded.
    let mut ids: Vec<String> = vec![rec.name.clone()];
    ids.extend(rec.comments.iter().take(4).cloned());
    while ids.len() < 5 {
        ids.push("NONE".into());
    }
    for id in ids.iter().take(5) {
        let mut line = id.replace(['\n', '\r'], " ");
        if line.trim().is_empty() {
            line = "NONE".into();
        }
        line.truncate(80);
        out.push_str(&line);
        out.push('\n');
    }
    // Record 6.
    let (rnode, rdir) = rec
        .response
        .map(|d| (d.node.unwrap_or(0) as i64, dir_to_uff(d.dir)))
        .unwrap_or((0, 0));
    let (fnode, fdir) = rec
        .reference
        .map(|d| (d.node.unwrap_or(0) as i64, dir_to_uff(d.dir)))
        .unwrap_or((0, 0));
    out.push_str(&format!(
        "{:>5}{:>10}{:>5}{:>10} {:<10}{:>10}{:>4} {:<10}{:>10}{:>4}\n",
        uff_type_from_kind(rec.kind),
        0, // function id
        1, // version
        0, // load case
        "NONE",
        rnode,
        rdir,
        "NONE",
        fnode,
        fdir,
    ));
    // Record 7: always double-precision ordinates on write (types 4/6) - the
    // strict writer never downcasts.
    let ord_type = if complex { 6 } else { 4 };
    let (x_min, x_step) = match rec.abscissa {
        Abscissa::Even { start, step, .. } => (start, step),
        Abscissa::Explicit(_) => (0.0, 0.0),
    };
    out.push_str(&format!(
        "{}{}{}{}{}{}\n",
        i10(ord_type),
        i10(n as i64),
        i10(if even { 1 } else { 0 }),
        e13_5(x_min),
        e13_5(x_step),
        e13_5(0.0)
    ));
    // Records 8-11: abscissa, ordinate, denominator (none), z (none).
    let axis_rec = |specific: i64, label: &str, unit: &str| -> String {
        let lab = if label.is_empty() { "NONE" } else { label };
        let unt = if unit.is_empty() { "NONE" } else { unit };
        format!(
            "{:>10}{:>5}{:>5}{:>5} {:<20} {:<20}\n",
            specific,
            0,
            0,
            0,
            &lab[..lab.len().min(20)],
            &unt[..unt.len().min(20)]
        )
    };
    let absc_specific = match rec.kind {
        FunctionKind::TimeHistory => 17, // time
        _ => 18,                         // frequency
    };
    out.push_str(&axis_rec(
        absc_specific,
        rec.unit_abscissa.label.as_str(),
        rec.unit_abscissa.label.as_str(),
    ));
    out.push_str(&axis_rec(
        1,
        rec.unit_ordinate.label.as_str(),
        rec.unit_ordinate.label.as_str(),
    ));
    out.push_str(&axis_rec(0, "NONE", "NONE"));
    out.push_str(&axis_rec(0, "NONE", "NONE"));
    // Data: 4E20.12-family for wide even; 13-wide interleave for uneven.
    let xs = rec.abscissa.values();
    let mut scalars: Vec<f64> = Vec::new();
    match (&rec.ordinate, even) {
        (Ordinate::Real(ys), true) => scalars.extend(ys),
        (Ordinate::Real(ys), false) => {
            for (x, y) in xs.iter().zip(ys) {
                scalars.push(*x);
                scalars.push(*y);
            }
        }
        (Ordinate::Complex(cs), true) => {
            for (re, im) in cs {
                scalars.push(*re);
                scalars.push(*im);
            }
        }
        (Ordinate::Complex(cs), false) => {
            for (x, (re, im)) in xs.iter().zip(cs) {
                scalars.push(*x);
                scalars.push(*re);
                scalars.push(*im);
            }
        }
    }
    // Emission: even -> all values E20.12, 4/line. Uneven -> x in E13.5 then
    // ordinates in E20.12 (2(E13.5,E20.12) / E13.5,2E20.12 layouts).
    if even {
        for chunk in scalars.chunks(4) {
            for v in chunk {
                out.push_str(&e20_12(*v));
            }
            out.push('\n');
        }
    } else {
        let stride = if complex { 3 } else { 2 };
        let group_per_line = if complex { 1 } else { 2 };
        for group in scalars.chunks(stride * group_per_line) {
            for (i, v) in group.iter().enumerate() {
                if i % stride == 0 {
                    out.push_str(&e13_5(*v));
                } else {
                    out.push_str(&e20_12(*v));
                }
            }
            out.push('\n');
        }
    }
    out.push_str("    -1\n");
    Ok(())
}

/// E20.12 emission (` s1.234567890123E+00` in 20 columns).
fn e20_12(v: f64) -> String {
    if v == 0.0 {
        return format!("{:>20}", "0.000000000000E+00");
    }
    let neg = v < 0.0;
    let a = v.abs();
    let mut exp = a.log10().floor() as i32;
    let mut mant = a / 10f64.powi(exp);
    if mant >= 10.0 {
        mant /= 10.0;
        exp += 1;
    }
    let mant = format!("{mant:.12}");
    let (mant, exp) = if mant.starts_with("10.") {
        ("1.000000000000".to_string(), exp + 1)
    } else {
        (mant, exp)
    };
    let sign = if neg { "-" } else { "" };
    let esign = if exp < 0 { '-' } else { '+' };
    format!("{:>20}", format!("{sign}{mant}E{esign}{:02}", exp.abs()))
}

/// Write a whole file: dataset 151 header + one 58 block per record.
pub fn write_file(records: &[&FunctionRecord], origin: &str) -> Result<String, String> {
    let mut out = String::new();
    out.push_str("    -1\n   151\n");
    out.push_str(&format!("{origin}\n"));
    out.push_str("Eigen Insight export\n");
    out.push_str("Eigen\nNONE\nNONE\nEigen Insight\nNONE\n");
    out.push_str("    -1\n");
    for r in records {
        write_58(r, &mut out)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eigen_data::units::unit;

    fn mk(kind: FunctionKind, even: bool, complex: bool) -> FunctionRecord {
        let n = 16usize;
        let abscissa = if even {
            Abscissa::Even {
                start: 0.0,
                step: 2.5,
                n,
            }
        } else {
            Abscissa::Explicit((0..n).map(|i| 10.0 * 1.5f64.powi(i as i32)).collect())
        };
        let ordinate = if complex {
            Ordinate::Complex(
                (0..n)
                    .map(|i| (1.0 + i as f64 * 0.5, -0.25 * i as f64))
                    .collect(),
            )
        } else {
            Ordinate::Real(
                (0..n)
                    .map(|i| 0.001234 * (i as f64 + 1.0) * if i % 3 == 0 { -1.0 } else { 1.0 })
                    .collect(),
            )
        };
        FunctionRecord {
            id: FnId(1),
            name: "Run 12 control 101X+".into(),
            role: None,
            response: Some("101X+".parse().unwrap()),
            reference: Some("1Z+".parse().unwrap()),
            kind,
            abscissa,
            ordinate,
            unit_ordinate: unit("g").unwrap(),
            unit_abscissa: unit("Hz").unwrap(),
            t0: None,
            provenance: Provenance::Imported {
                format: "test".into(),
                path: "m".into(),
                detail: String::new(),
            },
            source: SourceRef {
                path: "m".into(),
                format: "test".into(),
                imported_at: 0,
            },
            comments: vec!["fixture".into()],
        }
    }

    fn roundtrip(rec: &FunctionRecord) -> FunctionRecord {
        let text = write_file(&[rec], "test").unwrap();
        let out = import_bytes(text.as_bytes(), "mem.unv").unwrap();
        assert_eq!(out.functions.len(), 1, "warnings: {:?}", out.warnings);
        out.functions.into_iter().next().unwrap()
    }

    #[test]
    fn round_trip_even_real_psd() {
        let rec = mk(FunctionKind::Psd, true, false);
        let back = roundtrip(&rec);
        assert_eq!(back.kind, FunctionKind::Psd);
        assert_eq!(back.response, rec.response);
        assert_eq!(back.reference, rec.reference);
        let (Ordinate::Real(a), Ordinate::Real(b)) = (&rec.ordinate, &back.ordinate) else {
            panic!("real ordinates expected")
        };
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() <= x.abs() * 1e-11 + 1e-15, "{x} vs {y}");
        }
        match back.abscissa {
            Abscissa::Even { start, step, n } => {
                assert_eq!(n, 16);
                assert!((start).abs() < 1e-12 && (step - 2.5).abs() < 1e-11);
            }
            _ => panic!("expected even abscissa"),
        }
    }

    #[test]
    fn round_trip_uneven_complex_frf() {
        let rec = mk(FunctionKind::Frf, false, true);
        let back = roundtrip(&rec);
        assert_eq!(back.kind, FunctionKind::Frf);
        let (Ordinate::Complex(a), Ordinate::Complex(b)) = (&rec.ordinate, &back.ordinate) else {
            panic!("complex ordinates expected")
        };
        for ((r1, i1), (r2, i2)) in a.iter().zip(b) {
            assert!((r1 - r2).abs() <= r1.abs() * 1e-11 + 1e-15);
            assert!((i1 - i2).abs() <= i1.abs() * 1e-11 + 1e-15);
        }
        // Uneven abscissa survives with E13.5 precision (~6 significant digits).
        let xa = rec.abscissa.values();
        let xb = back.abscissa.values();
        for (x, y) in xa.iter().zip(&xb) {
            assert!((x - y).abs() <= x.abs() * 1e-5, "{x} vs {y}");
        }
    }

    #[test]
    fn round_trip_uneven_real_srs() {
        let rec = mk(FunctionKind::Srs, false, false);
        let back = roundtrip(&rec);
        assert_eq!(back.kind, FunctionKind::Srs);
        assert_eq!(back.ordinate.len(), 16);
    }

    #[test]
    fn reader_handles_run_together_negatives_fixed_width() {
        // The most common wild failure: negative values consume the
        // separating space. Fixed-width slicing must recover all three.
        let mut text = String::new();
        text.push_str("    -1\n    58\nRUN\nNONE\nNONE\nNONE\nNONE\n");
        text.push_str(&format!(
            "{:>5}{:>10}{:>5}{:>10} {:<10}{:>10}{:>4} {:<10}{:>10}{:>4}\n",
            1, 0, 1, 0, "NONE", 7, 3, "NONE", 0, 0
        ));
        text.push_str(&format!(
            "{}{}{}{}{}{}\n",
            i10(2),
            i10(3),
            i10(1),
            e13_5(0.0),
            e13_5(0.5),
            e13_5(0.0)
        ));
        text.push_str("        17    0    0    0 s                    s                   \n");
        text.push_str("         1    0    0    0 g                    g                   \n");
        text.push_str("         0    0    0    0 NONE                 NONE                \n");
        text.push_str("         0    0    0    0 NONE                 NONE                \n");
        // Three E13.5 values with NO spaces between (negative signs eat them).
        text.push_str("  0.00000E+00 -4.11111E-02-1.11111E-02\n");
        text.push_str("    -1\n");
        let out = import_bytes(text.as_bytes(), "wild.unv").unwrap();
        let rec = &out.functions[0];
        let Ordinate::Real(v) = &rec.ordinate else {
            panic!()
        };
        assert_eq!(v.len(), 3);
        assert!((v[0] - 0.0).abs() < 1e-12);
        assert!((v[1] + 4.11111e-2).abs() < 1e-7);
        assert!((v[2] + 1.11111e-2).abs() < 1e-7);
    }

    #[test]
    fn reader_accepts_d_exponents_and_dash_one_in_id_lines() {
        let mut text = String::new();
        text.push_str("    -1\n    58\n");
        text.push_str("shaker run -1 retest\n"); // the delimiter-lookalike trap
        text.push_str("NONE\nNONE\nNONE\nNONE\n");
        text.push_str(&format!(
            "{:>5}{:>10}{:>5}{:>10} {:<10}{:>10}{:>4} {:<10}{:>10}{:>4}\n",
            1, 0, 1, 0, "NONE", 1, 1, "NONE", 0, 0
        ));
        text.push_str(&format!(
            "{}{}{}{}{}{}\n",
            i10(2),
            i10(2),
            i10(1),
            e13_5(0.0),
            e13_5(1.0),
            e13_5(0.0)
        ));
        for _ in 0..4 {
            text.push_str("         0    0    0    0 NONE                 NONE                \n");
        }
        text.push_str("  1.00000D+00  2.50000D-01\n"); // D exponents
        text.push_str("    -1\n");
        let out = import_bytes(text.as_bytes(), "d.unv").unwrap();
        let Ordinate::Real(v) = &out.functions[0].ordinate else {
            panic!()
        };
        assert!((v[0] - 1.0).abs() < 1e-12 && (v[1] - 0.25).abs() < 1e-12);
        assert!(
            out.functions[0].name.contains("-1 retest"),
            "ID line preserved"
        );
    }

    #[test]
    fn binary_58b_little_endian_even_single() {
        // Hand-built 58b: 4 f32 ordinates, even spacing.
        let mut text = String::new();
        text.push_str("    -1\n");
        text.push_str(&format!(
            "{:>6}b{:>6}{:>6}{:>12}{:>12}{:>6}{:>6}{:>12}{:>12}\n",
            58, 1, 2, 11, 16, 0, 0, 0, 0
        ));
        text.push_str("BIN\nNONE\nNONE\nNONE\nNONE\n");
        text.push_str(&format!(
            "{:>5}{:>10}{:>5}{:>10} {:<10}{:>10}{:>4} {:<10}{:>10}{:>4}\n",
            9, 0, 1, 0, "NONE", 3, 2, "NONE", 0, 0
        ));
        text.push_str(&format!(
            "{}{}{}{}{}{}\n",
            i10(2),
            i10(4),
            i10(1),
            e13_5(10.0),
            e13_5(2.0),
            e13_5(0.0)
        ));
        for _ in 0..4 {
            text.push_str("         0    0    0    0 NONE                 NONE                \n");
        }
        let mut bytes = text.into_bytes();
        for v in [1.5f32, -2.5, 3.5, 0.125] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.extend_from_slice(b"    -1\n");
        let out = import_bytes(&bytes, "bin.unv").unwrap();
        let rec = &out.functions[0];
        assert_eq!(rec.kind, FunctionKind::Psd);
        let Ordinate::Real(v) = &rec.ordinate else {
            panic!()
        };
        assert_eq!(v, &vec![1.5, -2.5, 3.5, 0.125]);
        match rec.abscissa {
            Abscissa::Even { start, step, n } => {
                assert_eq!((start, step, n), (10.0, 2.0, 4));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn binary_payload_containing_dash_one_is_not_split() {
        // Payload bytes that spell "\n    -1\n" must NOT terminate the block:
        // the walker counts bytes, it does not scan.
        let evil = *b"\n    -1\n"; // 8 bytes == two f32
        let mut text = String::new();
        text.push_str("    -1\n");
        text.push_str(&format!(
            "{:>6}b{:>6}{:>6}{:>12}{:>12}{:>6}{:>6}{:>12}{:>12}\n",
            58, 1, 2, 11, 8, 0, 0, 0, 0
        ));
        text.push_str("EVIL\nNONE\nNONE\nNONE\nNONE\n");
        text.push_str(&format!(
            "{:>5}{:>10}{:>5}{:>10} {:<10}{:>10}{:>4} {:<10}{:>10}{:>4}\n",
            1, 0, 1, 0, "NONE", 0, 0, "NONE", 0, 0
        ));
        text.push_str(&format!(
            "{}{}{}{}{}{}\n",
            i10(2),
            i10(2),
            i10(1),
            e13_5(0.0),
            e13_5(1.0),
            e13_5(0.0)
        ));
        for _ in 0..4 {
            text.push_str("         0    0    0    0 NONE                 NONE                \n");
        }
        let mut bytes = text.into_bytes();
        bytes.extend_from_slice(&evil);
        bytes.extend_from_slice(b"    -1\n");
        let out = import_bytes(&bytes, "evil.unv").unwrap();
        assert_eq!(out.functions.len(), 1);
        let Ordinate::Real(v) = &out.functions[0].ordinate else {
            panic!()
        };
        assert_eq!(v.len(), 2);
        // The two f32s decode from the evil bytes - garbage values, but the
        // STRUCTURE survived, which is the property under test.
    }

    #[test]
    fn nval_is_authoritative_over_padded_data() {
        let mut text = String::new();
        text.push_str("    -1\n    58\nPAD\nNONE\nNONE\nNONE\nNONE\n");
        text.push_str(&format!(
            "{:>5}{:>10}{:>5}{:>10} {:<10}{:>10}{:>4} {:<10}{:>10}{:>4}\n",
            1, 0, 1, 0, "NONE", 0, 0, "NONE", 0, 0
        ));
        text.push_str(&format!(
            "{}{}{}{}{}{}\n",
            i10(2),
            i10(3),
            i10(1),
            e13_5(0.0),
            e13_5(1.0),
            e13_5(0.0)
        ));
        for _ in 0..4 {
            text.push_str("         0    0    0    0 NONE                 NONE                \n");
        }
        // Six values on the line; NVAL says 3.
        text.push_str(
            "  1.00000E+00  2.00000E+00  3.00000E+00  0.00000E+00  0.00000E+00  0.00000E+00\n",
        );
        text.push_str("    -1\n");
        let out = import_bytes(text.as_bytes(), "pad.unv").unwrap();
        assert_eq!(out.functions[0].ordinate.len(), 3);
    }

    #[test]
    fn sniffer_accepts_uff_head() {
        assert_eq!(sniff_head(b"    -1\n    58\n"), Confidence::Yes);
        assert_eq!(sniff_head(b"    -1\n    58b     1\n"), Confidence::Yes);
        assert_eq!(sniff_head(b"hello\nworld\n"), Confidence::No);
    }

    #[test]
    fn e13_5_layout_is_stable() {
        assert_eq!(e13_5(0.0), "  0.00000E+00");
        assert_eq!(e13_5(-4.11111e-2), " -4.11111E-02");
        assert_eq!(e13_5(1.0), "  1.00000E+00");
        assert_eq!(e13_5(-0.25), " -2.50000E-01");
        assert_eq!(e13_5(9.999999e5), "  1.00000E+06", "decade-edge rounding");
        for v in [0.0, 1.0, -0.25, 3.7e-8, -9.99e9] {
            assert_eq!(e13_5(v).len(), 13, "{v}");
            assert_eq!(e20_12(v).len(), 20, "{v}");
        }
    }
}
